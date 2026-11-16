//! Draw-order position rows built from RenderDoc post-transform mesh buffers.
use crate::{
    mesh_indices, mesh_positions,
    replay::{BUFFER_PAGE_LIMIT, MeshInfo},
};
use serde::Serialize;
use std::collections::HashMap;

pub const PAGE_LIMIT: u32 = 64;

#[derive(Debug, Serialize)]
pub struct DrawRow {
    pub row: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meshlet: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meshlet_row: Option<u32>,
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
    if count > PAGE_LIMIT || offset > info.num_indices || !matches!(info.stage, 1 | 2 | 4) {
        return Err("Invalid mesh draw page".into());
    }
    let count = count.min(info.num_indices - offset);
    // MeshOut index values already address the combined vertex buffer. The
    // meshlet prefix is for display and must not be added to those indices.
    let mut meshlet_ranges = Vec::new();
    if info.stage == 4 {
        let mut start = 0u64;
        for (index, meshlet) in info.meshlets.iter().enumerate() {
            let end = start
                .checked_add(u64::from(meshlet.indices))
                .ok_or("Meshlet index count overflow")?;
            let meshlet_id = u32::try_from(index)
                .ok()
                .and_then(|i| info.meshlet_offset.checked_add(i))
                .ok_or("Meshlet ID overflow")?;
            if end > start {
                meshlet_ranges.push((start, end, meshlet_id));
            }
            start = end;
        }
        if !info.meshlets.is_empty() && start != u64::from(info.num_indices) {
            return Err("Meshlet index counts do not match mesh output".into());
        }
    }
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
    if supported && info.vertex_bound {
        let mut vertices: Vec<u32> = match &indices {
            Some(indices) => indices.iter().filter_map(|index| index.vertex).collect(),
            None => (offset..offset + count).collect(),
        };
        vertices.sort_unstable();
        vertices.dedup();
        let mut first = 0;
        while first < vertices.len() {
            let mut end = first + 1;
            while end < vertices.len()
                && vertices[end - 1].checked_add(1) == Some(vertices[end])
                && u64::from(vertices[end] - vertices[first]) * u64::from(info.vertex_stride)
                    + u64::from(width)
                    <= u64::from(BUFFER_PAGE_LIMIT)
            {
                end += 1;
            }
            let start = u64::from(vertices[first]) * u64::from(info.vertex_stride);
            let length = u64::from(vertices[end - 1] - vertices[first])
                * u64::from(info.vertex_stride)
                + u64::from(width);
            let bytes = read("vertex", start, length as u32)?;
            for &vertex in &vertices[first..end] {
                let relative = u64::from(vertex - vertices[first]) * u64::from(info.vertex_stride);
                let position = usize::try_from(relative)
                    .ok()
                    .and_then(|begin| {
                        let finish = begin.checked_add(width as usize)?;
                        let element = bytes.get(begin..finish)?;
                        mesh_positions::decode(info, element, start.checked_add(relative)?)
                    })
                    .and_then(|mut positions| positions.pop())
                    .map(|position| position.values);
                cache.insert(vertex, position);
            }
            first = end;
        }
    }
    let mut rows = Vec::with_capacity(count as usize);
    for i in 0..count {
        let row = offset + i;
        let range_index = meshlet_ranges.partition_point(|range| range.1 <= u64::from(row));
        let group = meshlet_ranges
            .get(range_index)
            .filter(|range| range.0 <= u64::from(row));
        let meshlet = group.map(|range| range.2);
        let meshlet_row = group.map(|range| (u64::from(row) - range.0) as u32);
        let (raw_index, vertex, restart) = match &indices {
            Some(indices) => match indices.get(i as usize) {
                Some(index) => (Some(index.raw), index.vertex, index.restart),
                None => {
                    rows.push(DrawRow {
                        row,
                        meshlet,
                        meshlet_row,
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
            meshlet,
            meshlet_row,
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
    fn mesh_output_tracks_meshlet_boundaries_without_rebasing_indices() {
        use crate::replay::MeshletInfo;
        let info = MeshInfo {
            stage: 4,
            index_bound: true,
            index_stride: 1,
            vertex_bound: true,
            vertex_stride: 4,
            component_type: 1,
            component_byte_width: 4,
            component_count: 1,
            num_indices: 4,
            meshlet_offset: 7,
            meshlets: vec![
                MeshletInfo {
                    indices: 2,
                    vertices: 2,
                },
                MeshletInfo {
                    indices: 0,
                    vertices: 0,
                },
                MeshletInfo {
                    indices: 2,
                    vertices: 2,
                },
            ],
            ..Default::default()
        };
        let result = page(&info, 1, 3, |kind, offset, length| {
            if kind == "index" {
                return Ok(vec![1, 3, 2]);
            }
            Ok((0..length / 4)
                .flat_map(|i| ((offset / 4 + u64::from(i)) as f32).to_le_bytes())
                .collect())
        })
        .unwrap();
        assert_eq!(
            (result.rows[0].meshlet, result.rows[0].meshlet_row),
            (Some(7), Some(1))
        );
        assert_eq!(
            (result.rows[1].meshlet, result.rows[1].meshlet_row),
            (Some(9), Some(0))
        );
        assert_eq!(result.rows[1].vertex, Some(3));
        assert_eq!(result.rows[1].position, Some(vec![3.0]));
        let mut invalid = info.clone();
        invalid.meshlets[0].indices = 1;
        assert!(page(&invalid, 0, 1, |_, _, _| unreachable!()).is_err());
        invalid.stage = 3;
        assert!(page(&invalid, 0, 1, |_, _, _| unreachable!()).is_err());
    }

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
            assert_eq!(length, 8);
            Ok((0..length / 4)
                .flat_map(|i| ((offset + u64::from(i * 4)) as f32).to_le_bytes())
                .collect())
        })
        .unwrap();
        assert_eq!(page.rows.len(), 2);
        assert_eq!(page.rows[0].vertex, Some(1));
        assert_eq!(page.rows[1].position, Some(vec![8.0]));
        assert!(super::page(&info, 0, PAGE_LIMIT + 1, |_, _, _| unreachable!()).is_err());
    }

    #[test]
    fn batches_adjacent_vertices_but_not_sparse_ranges() {
        let info = MeshInfo {
            stage: 1,
            index_bound: true,
            index_stride: 1,
            vertex_bound: true,
            vertex_stride: 4,
            component_type: 1,
            component_byte_width: 4,
            component_count: 1,
            num_indices: 4,
            ..Default::default()
        };
        let mut reads = Vec::new();
        let result = page(&info, 0, 4, |kind, offset, length| {
            if kind == "index" {
                return Ok(vec![2, 1, 2, 9]);
            }
            reads.push((offset, length));
            Ok((0..length / 4)
                .flat_map(|i| ((offset / 4 + u64::from(i)) as f32).to_le_bytes())
                .collect())
        })
        .unwrap();
        assert_eq!(reads, [(4, 8), (36, 4)]);
        assert_eq!(
            result
                .rows
                .iter()
                .map(|row| row.position.clone())
                .collect::<Vec<_>>(),
            [
                Some(vec![2.0]),
                Some(vec![1.0]),
                Some(vec![2.0]),
                Some(vec![9.0])
            ]
        );
    }

    #[test]
    fn grouped_short_read_preserves_individual_bounds() {
        let info = MeshInfo {
            stage: 1,
            vertex_bound: true,
            vertex_stride: 4,
            component_type: 1,
            component_byte_width: 4,
            component_count: 1,
            num_indices: 2,
            ..Default::default()
        };
        let result = page(&info, 0, 2, |_, _, length| {
            assert_eq!(length, 8);
            Ok(7.0f32.to_le_bytes().to_vec())
        })
        .unwrap();
        assert_eq!(result.rows[0].position, Some(vec![7.0]));
        assert_eq!(result.rows[1].unavailable, Some("vertex_out_of_bounds"));
    }
}
