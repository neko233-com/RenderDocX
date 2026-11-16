//! Export the currently filtered event hierarchy as a text file.
use crate::event_tree::ExportRow;
use std::{
    io::{BufWriter, Write},
    path::Path,
};

pub fn write_new(
    path: &Path,
    capture: &str,
    frame: u32,
    rows: &[ExportRow],
    has_times: bool,
) -> Result<usize, String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temp = tempfile::Builder::new()
        .prefix(".renderdocx-events-")
        .tempfile_in(parent)
        .map_err(|e| format!("Cannot create temporary event export: {e}"))?;
    let names: Vec<String> = rows
        .iter()
        .map(|row| {
            let indent = row
                .depth
                .saturating_mul(2)
                .saturating_sub(usize::from(row.first_child));
            format!(
                "{}{}- {}",
                " ".repeat(indent),
                if row.first_child { "\\" } else { "" },
                row.name
            )
        })
        .collect();
    let width = names
        .iter()
        .map(|s| s.encode_utf16().count())
        .max()
        .unwrap_or(0)
        .max(5);
    {
        let mut output = BufWriter::new(temp.as_file_mut());
        writeln!(output, "{capture} - Frame #{frame}\n").map_err(|e| e.to_string())?;
        write!(output, " EID  | Event{} | Action #", " ".repeat(width - 5))
            .map_err(|e| e.to_string())?;
        if has_times {
            write!(output, " | Duration (ms)").map_err(|e| e.to_string())?;
        }
        writeln!(output).map_err(|e| e.to_string())?;
        writeln!(
            output,
            "{}",
            "-".repeat(width + if has_times { 34 } else { 19 })
        )
        .map_err(|e| e.to_string())?;
        for (row, name) in rows.iter().zip(&names) {
            let name_pad = width.saturating_sub(name.encode_utf16().count());
            let action = if let Some(range) = &row.action_range {
                range
                    .last_action
                    .filter(|end| *end != range.first_action)
                    .map(|end| format!("{}-{end}", range.first_action))
                    .unwrap_or_default()
            } else {
                row.action_id.map(|id| id.to_string()).unwrap_or_default()
            };
            write!(
                output,
                "{:<5} | {}{} | {:<6}",
                row.event_id,
                name,
                " ".repeat(name_pad),
                action
            )
            .map_err(|e| e.to_string())?;
            if has_times {
                write!(output, " |").map_err(|e| e.to_string())?;
                if let Some(seconds) = row.duration {
                    write!(output, " {:.3}", seconds * 1000.0).map_err(|e| e.to_string())?;
                }
            }
            writeln!(output).map_err(|e| e.to_string())?;
        }
        output
            .flush()
            .map_err(|e| format!("Cannot finish event export: {e}"))?;
    }
    temp.persist_noclobber(path)
        .map_err(|e| format!("Cannot publish event export: {}", e.error))?;
    Ok(rows.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn writes_hierarchy_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("事件.txt");
        let rows = vec![
            ExportRow {
                event_id: 4,
                action_id: None,
                action_range: Some(crate::event_ranges::ActionRange {
                    first_event: 4,
                    last_event: 5,
                    first_action: 2,
                    last_action: Some(3),
                }),
                name: "帧".into(),
                depth: 0,
                first_child: false,
                duration: Some(0.001),
            },
            ExportRow {
                event_id: 5,
                action_id: Some(1),
                action_range: None,
                name: "Draw".into(),
                depth: 1,
                first_child: true,
                duration: None,
            },
        ];
        assert_eq!(write_new(&path, "测试.rdc", 3, &rows, true).unwrap(), 2);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("测试.rdc - Frame #3"));
        assert!(text.contains("\\- Draw"));
        assert!(text.contains("1.000"));
        assert!(text.contains("2-3"));
        assert!(write_new(&path, "other", 4, &rows, false).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }
}
