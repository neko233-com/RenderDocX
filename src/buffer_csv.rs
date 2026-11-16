//! Streaming CSV export of decoded RenderDocX buffer layouts.
use crate::{
    buffer_layout::{FieldValue, Layout},
    replay::BUFFER_PAGE_LIMIT,
};
use serde::Serialize;
use std::{io::Write, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CsvScope {
    Repeating,
    Fixed,
}
#[derive(Serialize)]
pub struct CsvSummary {
    pub rows: String,
    pub trailing_bytes: String,
}

fn cell(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}
fn field_text(field: &FieldValue) -> String {
    let values = field.display_values.as_ref().unwrap_or(&field.values);
    values
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let value = item.join(", ");
            let names = field.enum_names.as_ref().and_then(|names| names.get(index));
            match names {
                Some(names) if !names.is_empty() => format!("{value} ({})", names.join(" | ")),
                _ => value,
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}
fn write_record(
    out: &mut impl Write,
    columns: impl IntoIterator<Item = String>,
) -> Result<(), String> {
    let line = columns
        .into_iter()
        .map(|column| cell(&column))
        .collect::<Vec<_>>()
        .join(",");
    writeln!(out, "{line}").map_err(|error| format!("Cannot write CSV export: {error}"))
}
fn write_header(out: &mut impl Write, fields: &[FieldValue]) -> Result<(), String> {
    write_record(
        out,
        std::iter::once("Byte Offset".to_owned())
            .chain(fields.iter().map(|field| field.name.clone())),
    )
}
fn write_row(out: &mut impl Write, offset: String, fields: &[FieldValue]) -> Result<(), String> {
    write_record(
        out,
        std::iter::once(offset).chain(fields.iter().map(field_text)),
    )
}

pub fn write_new<F, C>(
    path: &Path,
    origin: u64,
    length: u64,
    layout: &Layout,
    scope: CsvScope,
    mut read: F,
    check: C,
) -> Result<CsvSummary, String>
where
    F: FnMut(u64, u32) -> Result<Vec<u8>, String>,
    C: Fn() -> Result<(), String>,
{
    let end = origin.checked_add(length).ok_or("Invalid buffer range")?;
    let stride = layout.stride();
    let prefix = layout.repeat_offset().unwrap_or(0);
    if stride == 0
        || stride
            .checked_add(prefix)
            .is_none_or(|v| v > BUFFER_PAGE_LIMIT as usize)
    {
        return Err("Layout exceeds the buffer page limit".into());
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::Builder::new()
        .prefix(".renderdocx-buffer-csv-")
        .tempfile_in(parent)
        .map_err(|e| format!("Cannot create temporary CSV: {e}"))?;
    let mut rows = 0u64;
    let trailing;
    match scope {
        CsvScope::Fixed => {
            let count = if layout.is_single() {
                stride
            } else if layout.repeat_offset().is_some() {
                prefix
            } else {
                return Err("The layout has no fixed portion".into());
            };
            if count == 0 || count as u64 > length {
                return Err("The fixed portion is empty or incomplete".into());
            }
            check()?;
            let bytes = read(origin, count as u32)?;
            if bytes.len() != count {
                return Err("Buffer read returned incomplete data".into());
            }
            let page = layout.decode_at(origin, origin, &bytes)?;
            let fields = page.fixed.ok_or("The layout has no fixed portion")?;
            if fields.is_empty() {
                return Err("The fixed portion has no visible fields".into());
            }
            write_header(&mut file, &fields)?;
            write_row(&mut file, origin.to_string(), &fields)?;
            rows = 1;
            trailing = 0;
        }
        CsvScope::Repeating => {
            if layout.is_single() {
                return Err("Fixed layouts have no repeating rows".into());
            }
            if prefix as u64 > length {
                return Err("The fixed prefix is incomplete".into());
            }
            let sample = vec![0u8; prefix + stride];
            let schema = layout.decode_at(origin, origin, &sample)?;
            let fields = &schema
                .rows
                .first()
                .ok_or("Layout has no repeating fields")?
                .fields;
            write_header(&mut file, fields)?;
            let mut cursor = origin
                .checked_add(prefix as u64)
                .ok_or("Invalid buffer range")?;
            let page_size = (BUFFER_PAGE_LIMIT as usize / stride) * stride;
            while end - cursor >= stride as u64 {
                check()?;
                let available = (end - cursor) / stride as u64 * stride as u64;
                let count = available.min(page_size as u64) as u32;
                let bytes = read(cursor, count)?;
                if bytes.len() != count as usize {
                    return Err("Buffer read returned incomplete data".into());
                }
                let page = layout.decode_at(origin, cursor, &bytes)?;
                for row in page.rows {
                    write_row(&mut file, row.offset, &row.fields)?;
                    rows += 1;
                }
                cursor += u64::from(count);
            }
            trailing = end - cursor;
        }
    }
    file.flush()
        .map_err(|e| format!("Cannot finish CSV export: {e}"))?;
    check()?;
    file.persist_noclobber(path)
        .map_err(|e| format!("Cannot publish CSV export: {}", e.error))?;
    Ok(CsvSummary {
        rows: rows.to_string(),
        trailing_bytes: trailing.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exports_repeated_and_fixed_layouts_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::parse("struct S { uint header; uint2 data[]; }; S root;").unwrap();
        let bytes = [7u32, 1, 2, 3, 4]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .chain([0xaau8])
            .collect::<Vec<_>>();
        let read = |offset: u64, count: u32| {
            Ok(bytes[offset as usize..offset as usize + count as usize].to_vec())
        };
        let path = dir.path().join("rows.csv");
        let summary = write_new(
            &path,
            0,
            bytes.len() as u64,
            &layout,
            CsvScope::Repeating,
            read,
            || Ok(()),
        )
        .unwrap();
        assert_eq!(summary.rows, "2");
        assert_eq!(summary.trailing_bytes, "1");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "Byte Offset,root.data\n4,\"1, 2\"\n12,\"3, 4\"\n"
        );
        assert!(
            write_new(
                &path,
                0,
                bytes.len() as u64,
                &layout,
                CsvScope::Repeating,
                read,
                || Ok(())
            )
            .is_err()
        );
        let fixed = dir.path().join("fixed.csv");
        write_new(
            &fixed,
            0,
            bytes.len() as u64,
            &layout,
            CsvScope::Fixed,
            read,
            || Ok(()),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(fixed).unwrap(),
            "Byte Offset,root.header\n0,7\n"
        );
        let short = dir.path().join("short.csv");
        assert!(
            write_new(
                &short,
                0,
                bytes.len() as u64,
                &layout,
                CsvScope::Repeating,
                |_, _| Ok(vec![]),
                || Ok(())
            )
            .is_err()
        );
        assert!(!short.exists());
    }

    #[test]
    fn streams_large_exports_and_cleans_up_on_revision_change() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::parse("[[hex]] uint value;").unwrap();
        let reads = std::cell::Cell::new(0);
        let path = dir.path().join("large.csv");
        let summary = write_new(
            &path,
            0,
            u64::from(BUFFER_PAGE_LIMIT) + 8,
            &layout,
            CsvScope::Repeating,
            |_, count| {
                reads.set(reads.get() + 1);
                Ok(vec![0xff; count as usize])
            },
            || Ok(()),
        )
        .unwrap();
        assert_eq!(reads.get(), 2);
        assert_eq!(
            summary.rows,
            (u64::from(BUFFER_PAGE_LIMIT) / 4 + 2).to_string()
        );
        assert!(
            std::fs::read_to_string(path)
                .unwrap()
                .contains("0,FFFFFFFF")
        );
        let checks = std::cell::Cell::new(0);
        let cancelled = dir.path().join("cancelled.csv");
        assert!(
            write_new(
                &cancelled,
                0,
                u64::from(BUFFER_PAGE_LIMIT) + 4,
                &layout,
                CsvScope::Repeating,
                |_, count| Ok(vec![0; count as usize]),
                || {
                    checks.set(checks.get() + 1);
                    if checks.get() > 1 {
                        Err("Replay selection changed".into())
                    } else {
                        Ok(())
                    }
                }
            )
            .is_err()
        );
        assert!(!cancelled.exists());
    }
}
