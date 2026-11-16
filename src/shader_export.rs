//! Export the text actually reported by RenderDoc, without overwriting files.
use crate::replay::ShaderReport;
use std::{fs::OpenOptions, io::Write, path::Path};

#[derive(Clone, Copy)]
pub enum ShaderText {
    Disassembly,
    Source(usize),
}

pub fn text(report: &ShaderReport, selection: ShaderText) -> Result<&str, String> {
    match selection {
        ShaderText::Disassembly => Ok(&report.disassembly),
        ShaderText::Source(index) => report
            .source_files
            .get(index)
            .map(|file| file.contents.as_str())
            .ok_or_else(|| "Shader source index is out of range".into()),
    }
}

pub fn write_new(path: &Path, contents: &str) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("Could not create output file: {error}"))?;
    file.write_all(contents.as_bytes()).map_err(|error| {
        format!("Could not finish writing output file; it may contain partial data: {error}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn writes_unicode_without_replacing_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("shader.txt");
        write_new(&path, "中文\nline\r\n").unwrap();
        assert!(write_new(&path, "overwrite").is_err());
        assert_eq!(std::fs::read(path).unwrap(), "中文\nline\r\n".as_bytes());
    }
}
