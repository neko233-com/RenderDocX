//! Decode TaskOut payload records using the captured task shader reflection.
use crate::{
    buffer_format::ScalarFormat,
    replay::{BUFFER_PAGE_LIMIT, MeshInfo, ReflectedConstant},
};
use serde::Serialize;

pub const PAGE_LIMIT: u32 = 32;
const VALUE_LIMIT: usize = 4096;

#[derive(Debug, Serialize)]
pub struct Field {
    pub name: String,
    pub byte_offset: u32,
    pub base_type: u32,
    pub rows: u32,
    pub columns: u32,
    pub values: Vec<String>,
}
#[derive(Debug, Serialize)]
pub struct TaskRow {
    pub row: u32,
    pub dispatch: Option<[u32; 3]>,
    pub fields: Vec<Field>,
    pub error: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct TaskPage {
    pub offset: u32,
    pub total: u32,
    pub rows: Vec<TaskRow>,
}

fn format(t: u32) -> Option<ScalarFormat> {
    Some(match t {
        0 => ScalarFormat::F32,
        1 => ScalarFormat::F64,
        2 => ScalarFormat::F16,
        3 => ScalarFormat::I32,
        4 | 12 => ScalarFormat::U32,
        5 => ScalarFormat::I16,
        6 => ScalarFormat::U16,
        7 => ScalarFormat::I64,
        8 | 14 => ScalarFormat::U64,
        9 => ScalarFormat::I8,
        10 => ScalarFormat::U8,
        11 => ScalarFormat::Bool,
        _ => return None,
    })
}

// Vulkan post-task replay rewrites the payload to a scalar-packed storage
// struct. Derive its layout as LayOutStorageStruct/LayOutStorageArray do.
fn vulkan_storage_layout(vars: &mut [ReflectedConstant], depth: usize) -> Result<u32, String> {
    if depth > 16 {
        return Err("Task payload nesting exceeds limit".into());
    }
    let mut offset = 0u32;
    for var in vars {
        let ty = &mut var.variable_type;
        let elements = ty.elements.max(1);
        if elements as usize > VALUE_LIMIT {
            return Err("Task payload exceeds value limit".into());
        }
        let size = if !ty.members.is_empty() {
            vulkan_storage_layout(&mut ty.members, depth + 1)?
        } else {
            if ty.rows != 1 || ty.columns == 0 {
                return Err(format!("Unsupported Vulkan payload type: {}", var.name));
            }
            let width = format(ty.base_type)
                .ok_or_else(|| format!("Unsupported type for {}", var.name))?
                .width() as u32;
            if elements == 1 {
                offset = offset
                    .checked_add(width - 1)
                    .ok_or("Task payload offset overflow")?
                    / width
                    * width;
            }
            width
                .checked_mul(ty.columns)
                .ok_or("Task payload size overflow")?
        };
        var.byte_offset = offset;
        if elements > 1 {
            ty.array_stride = size;
        }
        offset = offset
            .checked_add(
                size.checked_mul(elements)
                    .ok_or("Task payload size overflow")?,
            )
            .ok_or("Task payload size overflow")?;
    }
    Ok(offset)
}

fn decode_fields(
    vars: &[ReflectedConstant],
    bytes: &[u8],
    base: usize,
    prefix: &str,
    depth: usize,
    out: &mut Vec<Field>,
    budget: &mut usize,
) -> Result<(), String> {
    if depth > 16 {
        return Err("Task payload nesting exceeds limit".into());
    }
    for var in vars {
        let ty = &var.variable_type;
        let count = ty.elements.max(1) as usize;
        if count > *budget {
            return Err("Task payload exceeds value limit".into());
        }
        let name = if prefix.is_empty() {
            var.name.clone()
        } else {
            format!("{prefix}.{}", var.name)
        };
        let start = base
            .checked_add(var.byte_offset as usize)
            .ok_or("Task payload offset overflow")?;
        if count > 1 && ty.array_stride == 0 {
            return Err(format!("Missing array stride for {name}"));
        }
        for element in 0..count {
            if *budget == 0 {
                return Err("Task payload exceeds value limit".into());
            }
            *budget -= 1;
            let offset = start
                .checked_add(
                    element
                        .checked_mul(ty.array_stride as usize)
                        .ok_or("Task payload offset overflow")?,
                )
                .ok_or("Task payload offset overflow")?;
            let element_name = if count > 1 {
                format!("{name}[{element}]")
            } else {
                name.clone()
            };
            if !ty.members.is_empty() {
                decode_fields(
                    &ty.members,
                    bytes,
                    offset,
                    &element_name,
                    depth + 1,
                    out,
                    budget,
                )?;
                continue;
            }
            let scalar = format(ty.base_type)
                .ok_or_else(|| format!("Unsupported type for {element_name}"))?;
            if var.bit_size != 0 {
                return Err(format!("Bitfield payload is not supported: {element_name}"));
            }
            let rows = ty.rows as usize;
            let cols = ty.columns as usize;
            let components = rows
                .checked_mul(cols)
                .ok_or("Task payload dimensions overflow")?;
            if rows == 0 || cols == 0 || components > *budget {
                return Err(format!("Invalid dimensions for {element_name}"));
            }
            if rows > 1 && ty.matrix_stride == 0 {
                return Err(format!("Missing matrix stride for {element_name}"));
            }
            let width = scalar.width();
            let mut values = Vec::with_capacity(components);
            for row in 0..rows {
                for col in 0..cols {
                    let relative = if rows == 1 {
                        col * width
                    } else if ty.flags & 1 != 0 {
                        row * ty.matrix_stride as usize + col * width
                    } else {
                        col * ty.matrix_stride as usize + row * width
                    };
                    let start = offset
                        .checked_add(relative)
                        .ok_or("Task payload offset overflow")?;
                    let end = start
                        .checked_add(width)
                        .ok_or("Task payload offset overflow")?;
                    values.push(
                        scalar.value(
                            bytes
                                .get(start..end)
                                .ok_or_else(|| format!("Incomplete payload for {element_name}"))?,
                        ),
                    );
                }
            }
            *budget -= components;
            out.push(Field {
                name: element_name,
                byte_offset: u32::try_from(offset).map_err(|_| "Task payload offset overflow")?,
                base_type: ty.base_type,
                rows: ty.rows,
                columns: ty.columns,
                values,
            });
        }
    }
    Ok(())
}

pub fn page<F>(info: &MeshInfo, offset: u32, count: u32, mut read: F) -> Result<TaskPage, String>
where
    F: FnMut(u64, u32) -> Result<Vec<u8>, String>,
{
    if info.stage != 3
        || count > PAGE_LIMIT
        || offset > info.num_indices
        || !info.vertex_bound
        || info.vertex_stride == 0
        || info.vertex_stride > BUFFER_PAGE_LIMIT
    {
        return Err("Invalid or unavailable task payload page".into());
    }
    let count = count
        .min(info.num_indices - offset)
        .min(BUFFER_PAGE_LIMIT / info.vertex_stride);
    let mut layout = info.task_payload.clone();
    if info.task_payload_scalar_layout {
        vulkan_storage_layout(&mut layout, 0)?;
    }
    let start = u64::from(offset) * u64::from(info.vertex_stride);
    let bytes = read(start, count * info.vertex_stride)?;
    let mut rows = Vec::with_capacity(count as usize);
    for i in 0..count {
        let begin = i as usize * info.vertex_stride as usize;
        let end = begin + info.vertex_stride as usize;
        let mut fields = Vec::new();
        let mut budget = VALUE_LIMIT;
        let error = match bytes.get(begin..end) {
            Some(data) if !layout.is_empty() => {
                decode_fields(&layout, data, 0, "", 0, &mut fields, &mut budget).err()
            }
            Some(_) => Some("Task payload reflection is unavailable".into()),
            None => Some("Incomplete task payload record".into()),
        };
        rows.push(TaskRow {
            row: offset + i,
            dispatch: info.tasks.get((offset + i) as usize).copied(),
            fields,
            error,
        });
    }
    Ok(TaskPage {
        offset,
        total: info.num_indices,
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scalar(name: &str, ty: u32, elements: u32) -> ReflectedConstant {
        serde_json::from_value(serde_json::json!({"name":name,"byte_offset":0,"bit_offset":0,"bit_size":0,"default_value":"0","type":{"name":"","flags":0,"pointer_type_id":0,"elements":elements,"array_stride":0,"base_type":ty,"rows":1,"columns":1,"matrix_stride":0,"members":[]}})).unwrap()
    }
    #[test]
    fn vulkan_layout_decodes_arrays_and_aligns_following_scalars() {
        let info = MeshInfo {
            stage: 3,
            vertex_bound: true,
            vertex_stride: 16,
            num_indices: 1,
            task_payload_scalar_layout: true,
            task_payload: vec![scalar("bytes", 10, 3), scalar("word", 4, 1)],
            tasks: vec![[2, 1, 1]],
            ..Default::default()
        };
        let bytes = [1, 2, 3, 0, 42, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let result = page(&info, 0, 1, |_, _| Ok(bytes.to_vec())).unwrap();
        let row = &result.rows[0];
        assert!(row.error.is_none(), "{:?}", row.error);
        assert_eq!(row.fields[2].values, ["3"]);
        assert_eq!(row.fields[3].byte_offset, 4);
        assert_eq!(row.fields[3].values, ["42"]);
        assert_eq!(row.dispatch, Some([2, 1, 1]));
        assert!(page(&info, 0, PAGE_LIMIT + 1, |_, _| unreachable!()).is_err());
        let incomplete = page(&info, 0, 1, |_, _| Ok(vec![0; 2])).unwrap();
        assert!(incomplete.rows[0].error.is_some());
    }
    #[test]
    fn unsupported_vulkan_matrices_are_not_misdecoded() {
        let mut field = scalar("matrix", 0, 1);
        field.variable_type.rows = 2;
        field.variable_type.columns = 2;
        let info = MeshInfo {
            stage: 3,
            vertex_bound: true,
            vertex_stride: 16,
            num_indices: 1,
            task_payload_scalar_layout: true,
            task_payload: vec![field],
            ..Default::default()
        };
        assert!(page(&info, 0, 1, |_, _| unreachable!()).is_err());
    }
}
