//! Stream draw-order positions to a new CSV file.
use crate::{mesh_draw, replay::MeshInfo};
use std::{
    io::{BufWriter, Write},
    path::Path,
};

pub fn write_csv<F>(path: &Path, info: &MeshInfo, mut read: F) -> Result<u32, String>
where
    F: FnMut(&str, u64, u32) -> Result<Vec<u8>, String>,
{
    write_csv_filtered(path, info, &[], mesh_draw::MeshFilter::All, &mut read)
}

pub fn write_csv_filtered<F>(
    path: &Path,
    info: &MeshInfo,
    tasks: &[[u32; 3]],
    filter: mesh_draw::MeshFilter,
    mut read: F,
) -> Result<u32, String>
where
    F: FnMut(&str, u64, u32) -> Result<Vec<u8>, String>,
{
    if !matches!(info.stage, 1 | 2 | 4) {
        return Err("Draw-order export supports VS, GS and mesh output".into());
    }
    let (_, total) = mesh_draw::filter_range(info, tasks, filter)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("Cannot create temporary CSV: {error}"))?;
    {
        let mut output = BufWriter::new(temp.as_file_mut());
        writeln!(
            output,
            "{}",
            if info.stage == 4 {
                "row,raw_index,vertex,restart,x,y,z,w,unavailable,meshlet,meshlet_row"
            } else {
                "row,raw_index,vertex,restart,x,y,z,w,unavailable"
            }
        )
        .map_err(|error| format!("Cannot write CSV: {error}"))?;
        let mut offset = 0;
        while offset < total {
            let page = mesh_draw::filtered_page(
                info,
                tasks,
                filter,
                offset,
                mesh_draw::PAGE_LIMIT,
                &mut read,
            )?;
            if page.rows.is_empty() {
                return Err("Mesh export made no progress".into());
            }
            for row in &page.rows {
                let raw = row.raw_index.map(|v| v.to_string()).unwrap_or_default();
                let vertex = row.vertex.map(|v| v.to_string()).unwrap_or_default();
                write!(output, "{},{},{},{}", row.row, raw, vertex, row.restart)
                    .map_err(|error| format!("Cannot write CSV: {error}"))?;
                for index in 0..4 {
                    let value = row
                        .position
                        .as_ref()
                        .and_then(|p| p.get(index))
                        .map(|v| v.to_string())
                        .unwrap_or_default();
                    write!(output, ",{value}")
                        .map_err(|error| format!("Cannot write CSV: {error}"))?;
                }
                write!(output, ",{}", row.unavailable.unwrap_or_default())
                    .map_err(|error| format!("Cannot write CSV: {error}"))?;
                if info.stage == 4 {
                    write!(
                        output,
                        ",{},{}",
                        row.meshlet.map(|v| v.to_string()).unwrap_or_default(),
                        row.meshlet_row.map(|v| v.to_string()).unwrap_or_default()
                    )
                    .map_err(|error| format!("Cannot write CSV: {error}"))?;
                }
                writeln!(output).map_err(|error| format!("Cannot write CSV: {error}"))?;
            }
            offset = offset
                .checked_add(page.rows.len() as u32)
                .ok_or("Mesh export offset overflow")?;
        }
        output
            .flush()
            .map_err(|error| format!("Cannot finish CSV: {error}"))?;
    }
    temp.persist_noclobber(path)
        .map_err(|error| format!("Cannot create CSV destination: {}", error.error))?;
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exports_rows_without_replacing_existing_files_or_leaving_partial_output() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mesh.csv");
        let info = MeshInfo {
            stage: 1,
            vertex_bound: true,
            vertex_stride: 4,
            component_type: 1,
            component_count: 1,
            component_byte_width: 4,
            num_indices: 2,
            ..Default::default()
        };
        let read = |_: &str, offset: u64, length: u32| {
            Ok((0..length / 4)
                .flat_map(|i| ((offset + u64::from(i * 4)) as f32).to_le_bytes())
                .collect())
        };
        assert_eq!(write_csv(&path, &info, read).unwrap(), 2);
        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("0,,0,false,0,,,,\n1,,1,false,4,,,,\n"));
        assert!(write_csv(&path, &info, read).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), result);
        let failure = dir.path().join("failed.csv");
        assert!(write_csv(&failure, &info, |_, _, _| Err("read failed".into())).is_err());
        assert!(!failure.exists());
    }
}
