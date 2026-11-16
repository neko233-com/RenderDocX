//! Draw-order position rows built from RenderDoc post-transform mesh buffers.
use crate::{
    buffer_format::ScalarFormat,
    mesh_indices, mesh_positions,
    replay::{BUFFER_PAGE_LIMIT, MeshInfo},
};
use serde::Serialize;
use std::collections::HashMap;

pub const PAGE_LIMIT: u32 = 64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MeshFilter {
    #[default]
    All,
    Meshlet(u32),
    Task(u32),
}

/// Return the global index-row interval for a meshlet or task dispatch.
pub fn filter_range(
    info: &MeshInfo,
    tasks: &[[u32; 3]],
    filter: MeshFilter,
) -> Result<(u32, u32), String> {
    if filter == MeshFilter::All {
        return Ok((0, info.num_indices));
    }
    if info.stage != 4 {
        return Err("Meshlet filters require mesh output".into());
    }
    let (first, end) = match filter {
        MeshFilter::All => unreachable!(),
        MeshFilter::Meshlet(id) => {
            let index = id
                .checked_sub(info.meshlet_offset)
                .ok_or("Meshlet is out of range")? as usize;
            if index >= info.meshlets.len() {
                return Err("Meshlet is out of range".into());
            }
            (index, index + 1)
        }
        MeshFilter::Task(id) => {
            let index = usize::try_from(id).map_err(|_| "Task is out of range")?;
            if index >= tasks.len() {
                return Err("Task is out of range".into());
            }
            let mut first = 0usize;
            for dims in &tasks[..index] {
                let n = dims
                    .iter()
                    .try_fold(1usize, |n, &v| n.checked_mul(v as usize))
                    .ok_or("Task dispatch size overflow")?;
                first = first.checked_add(n).ok_or("Task dispatch size overflow")?;
            }
            let n = tasks[index]
                .iter()
                .try_fold(1usize, |n, &v| n.checked_mul(v as usize))
                .ok_or("Task dispatch size overflow")?;
            (
                first,
                first.checked_add(n).ok_or("Task dispatch size overflow")?,
            )
        }
    };
    if end > info.meshlets.len() {
        return Err("Task dispatch exceeds available meshlets".into());
    }
    let mut start = 0u32;
    let mut total = 0u32;
    for (index, meshlet) in info.meshlets.iter().enumerate() {
        if index < first {
            start = start
                .checked_add(meshlet.indices)
                .ok_or("Meshlet index count overflow")?;
        } else if index < end {
            total = total
                .checked_add(meshlet.indices)
                .ok_or("Meshlet index count overflow")?;
        }
    }
    if start
        .checked_add(total)
        .is_none_or(|n| n > info.num_indices)
    {
        return Err("Meshlet index counts exceed mesh output".into());
    }
    Ok((start, total))
}

/// Page inside the filtered range while preserving absolute mesh and vertex IDs.
pub fn filtered_page<F>(
    info: &MeshInfo,
    tasks: &[[u32; 3]],
    filter: MeshFilter,
    offset: u32,
    count: u32,
    read: F,
) -> Result<DrawPage, String>
where
    F: FnMut(&str, u64, u32) -> Result<Vec<u8>, String>,
{
    let (start, total) = filter_range(info, tasks, filter)?;
    if offset > total || count > PAGE_LIMIT {
        return Err("Invalid mesh draw page".into());
    }
    let mut result = page(info, start + offset, count.min(total - offset), read)?;
    result.offset = offset;
    result.total = total;
    Ok(result)
}

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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<AttributeValue>,
}

#[derive(Debug, Serialize)]
pub struct AttributeValue {
    pub name: String,
    pub per_primitive: bool,
    pub values: Option<Vec<String>>,
    pub hex: Option<String>,
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
                        attributes: Vec::new(),
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
            attributes: Vec::new(),
        });
    }
    for attr in &info.attributes {
        let width = attr.component_width;
        let len = width.checked_mul(attr.components).unwrap_or(0);
        let field_offset = attr.byte_offset.parse::<u64>().ok();
        let fmt = match (attr.base_type, width) {
            (0 | 2, 4) => Some(ScalarFormat::F32),
            (1, 8) => Some(ScalarFormat::F64),
            (3 | 5 | 9, 4) => Some(ScalarFormat::I32),
            (7, 8) => Some(ScalarFormat::I64),
            (4 | 6 | 10 | 11 | 12, 4) => Some(ScalarFormat::U32),
            (8 | 14, 8) => Some(ScalarFormat::U64),
            _ => None,
        };
        let primitive_base = info.per_primitive_offset.parse::<u64>().ok();
        let stride = if attr.per_primitive {
            info.per_primitive_stride
        } else {
            info.vertex_stride
        };
        let address = |row: &DrawRow| -> Option<u64> {
            if row.restart
                || row.vertex.is_none()
                || !info.vertex_bound
                || len == 0
                || len > BUFFER_PAGE_LIMIT
                || stride == 0
                || field_offset?.checked_add(u64::from(len))? > u64::from(stride)
            {
                return None;
            }
            let index = if attr.per_primitive {
                if info.primitive_vertices == 0 {
                    return None;
                }
                u64::from(row.row / info.primitive_vertices)
            } else {
                u64::from(row.vertex?)
            };
            let base = if attr.per_primitive {
                primitive_base?
            } else {
                0
            };
            base.checked_add(index.checked_mul(u64::from(stride))?)?
                .checked_add(field_offset?)
        };
        let mut offsets: Vec<u64> = rows.iter().filter_map(address).collect();
        offsets.sort_unstable();
        offsets.dedup();
        let mut decoded: HashMap<u64, (String, Option<Vec<String>>)> = HashMap::new();
        let mut first = 0;
        while first < offsets.len() {
            let mut end = first + 1;
            while end < offsets.len()
                && offsets[end]
                    .saturating_sub(offsets[first])
                    .saturating_add(u64::from(len))
                    <= u64::from(BUFFER_PAGE_LIMIT)
            {
                end += 1;
            }
            let amount = offsets[end - 1] - offsets[first] + u64::from(len);
            let data = read("vertex", offsets[first], amount as u32)?;
            for &absolute in &offsets[first..end] {
                let start = (absolute - offsets[first]) as usize;
                if let Some(bytes) = data.get(start..start + len as usize) {
                    let hex = bytes.iter().map(|b| format!("{b:02x}")).collect();
                    let values = fmt.map(|fmt| {
                        bytes
                            .chunks_exact(width as usize)
                            .map(|value| fmt.value(value))
                            .collect()
                    });
                    decoded.insert(absolute, (hex, values));
                }
            }
            first = end;
        }
        for row in &mut rows {
            let found = address(row).and_then(|offset| decoded.get(&offset));
            row.attributes.push(AttributeValue {
                name: attr.name.clone(),
                per_primitive: attr.per_primitive,
                hex: found.map(|item| item.0.clone()),
                values: found.and_then(|item| item.1.clone()),
                unavailable: if row.restart {
                    Some("restart")
                } else if found.is_none() {
                    Some("attribute_unavailable")
                } else if fmt.is_none() {
                    Some("unsupported_format")
                } else {
                    None
                },
            });
        }
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
    fn decodes_vertex_and_per_primitive_attributes_in_draw_order() {
        use crate::replay::MeshAttribute;
        let info = MeshInfo {
            stage: 4,
            vertex_bound: true,
            vertex_stride: 4,
            num_indices: 4,
            component_type: 1,
            component_byte_width: 4,
            component_count: 1,
            primitive_vertices: 2,
            per_primitive_offset: "16".into(),
            per_primitive_stride: 4,
            attributes: vec![
                MeshAttribute {
                    name: "position".into(),
                    base_type: 0,
                    components: 1,
                    component_width: 4,
                    byte_offset: "0".into(),
                    per_primitive: false,
                },
                MeshAttribute {
                    name: "primitive_id".into(),
                    base_type: 4,
                    components: 1,
                    component_width: 4,
                    byte_offset: "0".into(),
                    per_primitive: true,
                },
            ],
            ..Default::default()
        };
        let mut data: Vec<u8> = [1f32, 2f32, 3f32, 4f32]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        data.extend(7u32.to_le_bytes());
        data.extend(9u32.to_le_bytes());
        let result = page(&info, 0, 4, |_, offset, length| {
            let begin = offset as usize;
            Ok(data
                .get(begin..begin.saturating_add(length as usize))
                .unwrap_or_default()
                .to_vec())
        })
        .unwrap();
        assert_eq!(
            result.rows[0].attributes[0].values.as_ref().unwrap(),
            &["1"]
        );
        assert_eq!(
            result.rows[1].attributes[1].values.as_ref().unwrap(),
            &["7"]
        );
        assert_eq!(
            result.rows[2].attributes[1].values.as_ref().unwrap(),
            &["9"]
        );
    }
    #[test]
    fn filters_meshlets_and_task_groups_with_absolute_indices() {
        use crate::replay::MeshletInfo;
        let info = MeshInfo {
            stage: 4,
            num_indices: 6,
            meshlets: vec![
                MeshletInfo {
                    indices: 2,
                    vertices: 2,
                },
                MeshletInfo {
                    indices: 1,
                    vertices: 1,
                },
                MeshletInfo {
                    indices: 3,
                    vertices: 3,
                },
            ],
            ..Default::default()
        };
        let tasks = [[2, 1, 1], [0, 1, 1], [1, 1, 1]];
        assert_eq!(
            filter_range(&info, &tasks, MeshFilter::Task(0)).unwrap(),
            (0, 3)
        );
        assert_eq!(
            filter_range(&info, &tasks, MeshFilter::Task(1)).unwrap(),
            (3, 0)
        );
        assert_eq!(
            filter_range(&info, &tasks, MeshFilter::Task(2)).unwrap(),
            (3, 3)
        );
        assert_eq!(
            filter_range(&info, &tasks, MeshFilter::Meshlet(1)).unwrap(),
            (2, 1)
        );
        assert!(filter_range(&info, &tasks, MeshFilter::Meshlet(3)).is_err());
        assert!(filter_range(&info, &[[u32::MAX, u32::MAX, 2]], MeshFilter::Task(0)).is_err());
        let page = filtered_page(
            &info,
            &tasks,
            MeshFilter::Task(2),
            1,
            64,
            |_, _, _| unreachable!(),
        )
        .unwrap();
        assert_eq!((page.offset, page.total), (1, 3));
        assert_eq!(
            page.rows
                .iter()
                .map(|row| (row.row, row.meshlet, row.meshlet_row))
                .collect::<Vec<_>>(),
            [(4, Some(2), Some(1)), (5, Some(2), Some(2))]
        );
        assert!(
            filtered_page(
                &info,
                &tasks,
                MeshFilter::Task(2),
                4,
                1,
                |_, _, _| unreachable!()
            )
            .is_err()
        );
    }
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
