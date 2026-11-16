//! Interpret draw inputs using the common RenderDoc pipeline state.
use crate::replay::{Action, Buffer, Pipeline, PipelineBuffer, VertexInput};
use serde::Serialize;
use std::collections::HashMap;

pub const INPUT_PAGE_LIMIT: u32 = 64;
const DRAW: u32 = 0x2;
const INDEXED: u32 = 0x10000;

#[derive(Debug, Serialize)]
pub struct InputPage {
    pub event_id: u32,
    pub instance: u32,
    pub offset: u32,
    pub total: u32,
    pub rows: Vec<InputRow>,
}

#[derive(Debug, Serialize)]
pub struct InputRow {
    pub row: u32,
    pub raw_index: Option<u32>,
    pub vertex_index: Option<u32>,
    pub restart: bool,
    pub attributes: Vec<InputValue>,
}

#[derive(Debug, Serialize)]
pub struct InputValue {
    pub name: String,
    pub format: String,
    pub hex: Option<String>,
    pub values: Option<Vec<String>>,
    pub unavailable: Option<&'static str>,
}

fn decode(input: &VertexInput, bytes: &[u8]) -> Option<Vec<String>> {
    if input.format_type? != 0 {
        return None;
    }
    let kind = input.component_type?;
    let width = input.component_byte_width? as usize;
    let count = input.component_count? as usize;
    if count == 0 || count.checked_mul(width)? != bytes.len() {
        return None;
    }
    bytes
        .chunks_exact(width)
        .map(|b| match (kind, width) {
            (1, 4) => Some(f32::from_le_bytes(b.try_into().ok()?).to_string()),
            (4, 1) => Some(b[0].to_string()),
            (4, 2) => Some(u16::from_le_bytes(b.try_into().ok()?).to_string()),
            (4, 4) => Some(u32::from_le_bytes(b.try_into().ok()?).to_string()),
            (5, 1) => Some((b[0] as i8).to_string()),
            (5, 2) => Some(i16::from_le_bytes(b.try_into().ok()?).to_string()),
            (5, 4) => Some(i32::from_le_bytes(b.try_into().ok()?).to_string()),
            _ => None,
        })
        .collect()
}

/// Each read is checked against both the binding range and the actual resource size.
fn fetch<F>(
    binding: &PipelineBuffer,
    relative: u64,
    len: u32,
    buffers: &[Buffer],
    cache: &mut HashMap<(u32, u64, u32), Vec<u8>>,
    read: &mut F,
) -> Result<Option<Vec<u8>>, String>
where
    F: FnMut(u32, u64, u32) -> Result<Vec<u8>, String>,
{
    let Some(resource) = binding
        .resource
        .resource_index
        .filter(|_| binding.resource.bound)
    else {
        return Ok(None);
    };
    let Some((index, buffer)) = buffers
        .iter()
        .enumerate()
        .find(|(_, b)| b.resource_index == Some(resource))
    else {
        return Ok(None);
    };
    let size = binding
        .byte_size
        .parse::<u64>()
        .map_err(|_| "Invalid bound buffer size")?;
    let base = binding
        .byte_offset
        .parse::<u64>()
        .map_err(|_| "Invalid bound buffer offset")?;
    let end = relative
        .checked_add(u64::from(len))
        .ok_or("Buffer range overflow")?;
    if end > size {
        return Ok(None);
    }
    let offset = base.checked_add(relative).ok_or("Buffer offset overflow")?;
    let total = buffer
        .length
        .parse::<u64>()
        .map_err(|_| "Invalid resource size")?;
    if offset
        .checked_add(u64::from(len))
        .is_none_or(|end| end > total)
    {
        return Ok(None);
    }
    let index = u32::try_from(index).map_err(|_| "Buffer index overflow")?;
    // Coalesce nearby indexed vertices into bounded reads rather than issuing a
    // GPU readback for every cell. Blocks are scoped to this request only.
    const BLOCK: u64 = 4096;
    let mut result = Vec::with_capacity(len as usize);
    let mut cursor = offset;
    let finish = offset + u64::from(len);
    while cursor < finish {
        let start = cursor / BLOCK * BLOCK;
        let block_len = (total - start).min(BLOCK) as u32;
        let key = (index, start, block_len);
        if let std::collections::hash_map::Entry::Vacant(entry) = cache.entry(key) {
            let bytes = read(index, start, block_len)?;
            if bytes.len() != block_len as usize {
                return Err("Short vertex input read".into());
            }
            entry.insert(bytes);
        }
        let block = cache.get(&key).unwrap();
        let begin = (cursor - start) as usize;
        let take = ((finish - cursor) as usize).min(block.len() - begin);
        result.extend_from_slice(&block[begin..begin + take]);
        cursor += take as u64;
    }
    Ok(Some(result))
}

pub fn inspect<F>(
    action: &Action,
    pipeline: &Pipeline,
    buffers: &[Buffer],
    instance: u32,
    offset: u32,
    limit: u32,
    mut read: F,
) -> Result<InputPage, String>
where
    F: FnMut(u32, u64, u32) -> Result<Vec<u8>, String>,
{
    if action.flags & DRAW == 0 || pipeline.event_id != action.event_id || !pipeline.available {
        return Err("Select a draw action first".into());
    }
    if instance >= action.instances.max(1)
        || limit == 0
        || limit > INPUT_PAGE_LIMIT
        || offset > action.indices
    {
        return Err("Invalid vertex input page".into());
    }
    let base_vertex = action.base_vertex.ok_or("Bridge lacks draw offsets")?;
    let index_offset = action.index_offset.ok_or("Bridge lacks draw offsets")?;
    let vertex_offset = action.vertex_offset.ok_or("Bridge lacks draw offsets")?;
    let instance_offset = action.instance_offset.ok_or("Bridge lacks draw offsets")?;
    let inputs = pipeline
        .vertex_inputs
        .as_ref()
        .ok_or("Vertex inputs unavailable for this API")?;
    let vertex_buffers = pipeline
        .vertex_buffers
        .as_ref()
        .ok_or("Vertex buffers unavailable")?;
    let indexed = action.flags & INDEXED != 0;
    let mut cache = HashMap::new();
    let mut rows = Vec::new();
    for row in offset..offset.saturating_add(limit).min(action.indices) {
        let mut restart = false;
        let raw_index = if indexed {
            let ib = pipeline
                .index_buffer
                .as_ref()
                .ok_or("Index buffer unavailable")?;
            if !matches!(ib.byte_stride, 1 | 2 | 4) {
                return Err("Unsupported index stride".into());
            }
            let relative = (u64::from(index_offset) + u64::from(row))
                .checked_mul(u64::from(ib.byte_stride))
                .ok_or("Index offset overflow")?;
            let bytes = fetch(ib, relative, ib.byte_stride, buffers, &mut cache, &mut read)?;
            bytes.map(|bytes| {
                let mut raw = [0u8; 4];
                raw[..bytes.len()].copy_from_slice(&bytes);
                u32::from_le_bytes(raw)
            })
        } else {
            None
        };
        if let Some(index) = raw_index {
            let enabled = pipeline
                .restart_enabled
                .ok_or("Bridge lacks primitive restart metadata")?;
            let mask = match pipeline.index_buffer.as_ref().unwrap().byte_stride {
                1 => 0xff,
                2 => 0xffff,
                _ => u32::MAX,
            };
            restart = enabled
                && index
                    == (pipeline
                        .restart_index
                        .ok_or("Bridge lacks primitive restart metadata")?
                        & mask);
        }
        let vertex_index = if restart {
            None
        } else if indexed {
            raw_index.map(|index| {
                if base_vertex < 0 {
                    index.saturating_sub(base_vertex.unsigned_abs())
                } else {
                    index.wrapping_add(base_vertex as u32)
                }
            })
        } else {
            Some(vertex_offset.wrapping_add(row))
        };
        let mut attributes = Vec::new();
        if !restart {
            for input in inputs.iter().filter(|input| input.used) {
                let mut value = InputValue {
                    name: input.name.clone(),
                    format: input.format.clone(),
                    hex: None,
                    values: None,
                    unavailable: None,
                };
                let element = input
                    .element_size
                    .ok_or("Bridge lacks vertex format metadata")?;
                let slot = usize::try_from(input.vertex_buffer)
                    .ok()
                    .and_then(|index| vertex_buffers.get(index));
                let idx = if input.per_instance {
                    Some(
                        u64::from(instance_offset)
                            + u64::from(if input.instance_rate > 0 {
                                instance / input.instance_rate as u32
                            } else {
                                0
                            }),
                    )
                } else {
                    vertex_index.map(u64::from)
                };
                if input.generic_enabled {
                    value.unavailable = Some("generic_value_unavailable");
                } else if element == 0 || element > crate::replay::BUFFER_PAGE_LIMIT {
                    value.unavailable = Some("format_unavailable");
                } else if let (Some(binding), Some(idx)) = (slot, idx) {
                    let relative = idx
                        .checked_mul(u64::from(binding.byte_stride))
                        .and_then(|n| n.checked_add(u64::from(input.byte_offset)))
                        .ok_or("Vertex offset overflow")?;
                    if let Some(bytes) =
                        fetch(binding, relative, element, buffers, &mut cache, &mut read)?
                    {
                        value.hex = Some(bytes.iter().map(|b| format!("{b:02x}")).collect());
                        value.values = decode(input, &bytes);
                    } else {
                        value.unavailable = Some("out_of_bounds_or_unbound");
                    }
                } else {
                    value.unavailable = Some("out_of_bounds_or_unbound");
                }
                attributes.push(value);
            }
        }
        rows.push(InputRow {
            row,
            raw_index,
            vertex_index,
            restart,
            attributes,
        });
    }
    Ok(InputPage {
        event_id: action.event_id,
        instance,
        offset,
        total: action.indices,
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn indexed_inputs_follow_offsets_instance_rate_and_restart() {
        let action: Action = serde_json::from_value(
            json!({"event_id": 7, "action_id": 1, "depth": 0, "parent": -1,
            "flags": 65538, "indices": 3, "instances": 3, "base_vertex": -1, "index_offset": 1,
            "vertex_offset": 0, "instance_offset": 1, "name": "draw"}),
        )
        .unwrap();
        let pipeline: Pipeline = serde_json::from_value(json!({"event_id":7,"available":true,"topology":0,
            "shaders":[],"outputs":[],"restart_enabled":true,"restart_index":4294967295u32,
            "index_buffer":{"bound":true,"resource_index":0,"name":"","byte_offset":"2","byte_size":"20","byte_stride":2},
            "vertex_buffers":[{"bound":true,"resource_index":1,"name":"","byte_offset":"4","byte_size":"40","byte_stride":8},
                              {"bound":true,"resource_index":2,"name":"","byte_offset":"4","byte_size":"40","byte_stride":4}],
            "vertex_inputs":[{"name":"POSITION","vertex_buffer":0,"byte_offset":0,"per_instance":false,"instance_rate":0,"format":"float2","format_type":0,"component_type":1,"component_count":2,"component_byte_width":4,"element_size":8,"generic_enabled":false,"used":true},
                             {"name":"INSTANCE","vertex_buffer":1,"byte_offset":0,"per_instance":true,"instance_rate":2,"format":"uint","format_type":0,"component_type":4,"component_count":1,"component_byte_width":4,"element_size":4,"generic_enabled":false,"used":true}]})).unwrap();
        let buffers: Vec<Buffer> = (0..3)
            .map(|i| {
                serde_json::from_value(
                    json!({"name":"", "resource_index":i, "length":"64", "creation_flags":0}),
                )
                .unwrap()
            })
            .collect();
        let mut data = vec![vec![0u8; 64]; 3];
        data[0][4..10].copy_from_slice(&[0, 0, 4, 0, 255, 255]);
        for index in 0..4 {
            let pos = 4 + index * 8;
            data[1][pos..pos + 4].copy_from_slice(&(index as f32).to_le_bytes());
            data[1][pos + 4..pos + 8].copy_from_slice(&9f32.to_le_bytes());
        }
        data[2][12..16].copy_from_slice(&123u32.to_le_bytes());
        let mut reads = 0;
        let page = inspect(
            &action,
            &pipeline,
            &buffers,
            2,
            0,
            3,
            |index, offset, length| {
                reads += 1;
                Ok(
                    data[index as usize][offset as usize..(offset + u64::from(length)) as usize]
                        .to_vec(),
                )
            },
        )
        .unwrap();
        assert_eq!(page.rows[0].raw_index, Some(0));
        assert_eq!(page.rows[0].vertex_index, Some(0));
        assert_eq!(page.rows[1].vertex_index, Some(3));
        assert_eq!(
            page.rows[1].attributes[0].values.as_ref().unwrap(),
            &["3", "9"]
        );
        assert_eq!(
            page.rows[1].attributes[1].values.as_ref().unwrap(),
            &["123"]
        );
        assert!(page.rows[2].restart);
        assert!(page.rows[2].attributes.is_empty());
        assert_eq!(reads, 3, "nearby reads share one block per resource");
        let mut nonindexed = action.clone();
        nonindexed.flags &= !INDEXED;
        nonindexed.vertex_offset = Some(1);
        let mut limited = pipeline.clone();
        limited.vertex_buffers.as_mut().unwrap()[0].byte_size = "16".into();
        limited.vertex_inputs.as_mut().unwrap()[0].format_type = Some(15);
        let page = inspect(
            &nonindexed,
            &limited,
            &buffers,
            0,
            0,
            2,
            |index, offset, length| {
                Ok(
                    data[index as usize][offset as usize..(offset + u64::from(length)) as usize]
                        .to_vec(),
                )
            },
        )
        .unwrap();
        assert_eq!(page.rows[0].vertex_index, Some(1));
        assert!(page.rows[0].attributes[0].hex.is_some());
        assert!(page.rows[0].attributes[0].values.is_none());
        assert_eq!(
            page.rows[1].attributes[0].unavailable,
            Some("out_of_bounds_or_unbound")
        );
        assert!(
            inspect(
                &action,
                &pipeline,
                &buffers,
                3,
                0,
                1,
                |_, _, _| unreachable!()
            )
            .is_err()
        );
    }
}
