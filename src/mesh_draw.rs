//! Draw-order position rows built from RenderDoc post-transform mesh buffers.
use crate::{mesh_indices, mesh_positions, replay::MeshInfo};
use serde::Serialize;
use std::collections::HashMap;

pub const PAGE_LIMIT: u32 = 64;

#[derive(Debug, Serialize)]
pub struct DrawRow {
    pub row: u32,
    pub raw_index: Option<u32>,
    pub vertex: Option<u32>,
    pub restart: bool,
    pub position: Option<Vec<f32>>,
    pub unavailable: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct DrawPage {
    pub offset: u32,
    pub total: u32,
    pub rows: Vec<DrawRow>,
}

pub fn page<F>(info: &MeshInfo, offset: u32, count: u32, mut read: F) -> Result<DrawPage, String>
where
    F: FnMut(&str, u64, u32) -> Result<Vec<u8>, String>,
{
    if count > PAGE_LIMIT || offset > info.num_indices || !(1..=2).contains(&info.stage) {
        return Err("Invalid mesh draw page".into());
    }
    let count = count.min(info.num_indices - offset);
    let indices = if info.index_bound {
        if !matches!(info.index_stride, 1 | 2 | 4) {
            return Err("Unsupported mesh index stride".into());
        }
        let start = u64::from(offset) * u64::from(info.index_stride);
        let bytes = read("index", start, count * info.index_stride)?;
        let whole = bytes.len() / info.index_stride as usize * info.index_stride as usize;
        Some(mesh_indices::decode(info, &bytes[..whole], start).ok_or("Invalid mesh index data")?)
    } else {
        None
    };
    let supported = mesh_positions::supported(info);
    let width = info
        .component_count
        .saturating_mul(info.component_byte_width);
    let mut cache: HashMap<u32, Option<Vec<f32>>> = HashMap::new();
    let mut rows = Vec::with_capacity(count as usize);
    for i in 0..count {
        let row = offset + i;
        let (raw_index, vertex, restart) = match &indices {
            Some(indices) => match indices.get(i as usize) {
                Some(index) => (Some(index.raw), index.vertex, index.restart),
                None => {
                    rows.push(DrawRow {
                        row,
                        raw_index: None,
                        vertex: None,
                        restart: false,
                        position: None,
                        unavailable: Some("index_out_of_bounds"),
                    });
                    continue;
                }
            },
            None => (None, Some(row), false),
        };
        let mut unavailable = None;
        let position = if restart {
            None
        } else if !info.vertex_bound {
            unavailable = Some("vertex_buffer_unavailable");
            None
        } else if !supported {
            unavailable = Some("unsupported_format");
            None
        } else if let Some(vertex) = vertex {
            if !cache.contains_key(&vertex) {
                let start = u64::from(vertex) * u64::from(info.vertex_stride);
                let bytes = read("vertex", start, width)?;
                let position = mesh_positions::decode(info, &bytes, start)
                    .and_then(|mut positions| positions.pop())
                    .map(|position| position.values);
                cache.insert(vertex, position);
            }
            let result = cache.get(&vertex).cloned().flatten();
            if result.is_none() {
                unavailable = Some("vertex_out_of_bounds");
            }
            result
        } else {
            None
        };
        rows.push(DrawRow {
            row,
            raw_index,
            vertex,
            restart,
            position,
            unavailable,
        });
    }
    Ok(DrawPage {
        offset,
        total: info.num_indices,
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn follows_indices_and_reports_restart_and_short_reads() {
        let info = MeshInfo {
            stage: 1,
            index_bound: true,
            index_stride: 2,
            index_restart: Some(65535),
            vertex_bound: true,
            vertex_stride: 4,
            component_type: 1,
            component_byte_width: 4,
            component_count: 1,
            num_indices: 4,
            ..Default::default()
        };
        let indices = [1, 0, 0xff, 0xff, 0, 0];
        let vertices: Vec<u8> = [2.0f32, 5.0]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        let result = page(&info, 0, 4, |kind, offset, length| {
            let data = if kind == "index" {
                &indices[..]
            } else {
                &vertices[..]
            };
            let start = (offset as usize).min(data.len());
            Ok(data[start..start.saturating_add(length as usize).min(data.len())].to_vec())
        })
        .unwrap();
        assert_eq!(result.rows[0].position, Some(vec![5.0]));
        assert!(result.rows[1].restart);
        assert_eq!(result.rows[2].position, Some(vec![2.0]));
        assert_eq!(result.rows[3].unavailable, Some("index_out_of_bounds"));
    }

    #[test]
    fn nonindexed_pagination_and_limits() {
        let info = MeshInfo {
            stage: 2,
            vertex_bound: true,
            vertex_stride: 4,
            component_type: 1,
            component_byte_width: 4,
            component_count: 1,
            num_indices: 3,
            ..Default::default()
        };
        let page = page(&info, 1, 4, |kind, offset, length| {
            assert_eq!(kind, "vertex");
            assert_eq!(length, 4);
            Ok((offset as f32).to_le_bytes().to_vec())
        })
        .unwrap();
        assert_eq!(page.rows.len(), 2);
        assert_eq!(page.rows[0].vertex, Some(1));
        assert_eq!(page.rows[1].position, Some(vec![8.0]));
        assert!(super::page(&info, 0, PAGE_LIMIT + 1, |_, _, _| unreachable!()).is_err());
    }
}
