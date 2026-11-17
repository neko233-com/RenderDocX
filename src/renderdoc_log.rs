//! Discover and read RenderDoc's on-disk log file without the C++ replay bridge.
//!
//! Upstream computes the log path in `os/win32/win32_stringio.cpp::GetDefaultFiles`:
//! `%TEMP%\RenderDoc\RenderDoc_YYYY.MM.DD_HH.MM.SS.log` (replay app) or
//! `RenderDoc_app_...log` (in-app). `RENDERDOC_GetLogFile()` simply returns
//! this path and `GetLogFileContents` reads it, so a native GUI can do the same
//! with `std::fs` — no bridge, no C++ ABI.

use std::{env, fs, path::PathBuf};

/// Return candidate log directories in priority order.
pub fn log_directories() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(explicit) = env::var("RENDERDOC_LOG_FILE") {
        dirs.push(PathBuf::from(explicit));
    }
    if let Some(temp) = env::var_os("TMPDIR")
        .or_else(|| env::var_os("TEMP"))
        .or_else(|| env::var_os("TMP"))
    {
        dirs.push(PathBuf::from(temp).join("RenderDoc"));
    }
    #[cfg(windows)]
    {
        if let Some(local) = env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("RenderDoc"));
        }
    }
    dirs
}

/// Find the most recently modified RenderDoc log file, newest first by name
/// (timestamps are embedded in the filename) with mtime as tie-breaker.
pub fn find_latest_log() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    for dir in log_directories() {
        // An explicit file path points directly at the log.
        if dir.is_file() {
            candidates.push((fs::metadata(&dir).ok().and_then(|m| m.modified().ok()), dir));
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_log = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    let lower = name.to_ascii_lowercase();
                    lower.starts_with("renderdoc") && lower.ends_with(".log")
                });
            if is_log && path.is_file() {
                let mtime = fs::metadata(&path).ok().and_then(|m| m.modified().ok());
                candidates.push((mtime, path));
            }
        }
    }
    candidates.sort_by(|a, b| {
        // Later embedded timestamp / newer mtime first.
        let name_cmp = b.1.file_name().cmp(&a.1.file_name());
        name_cmp.then_with(|| b.0.cmp(&a.0))
    });
    candidates.into_iter().next().map(|(_, path)| path)
}

/// Read the full contents of a log file as UTF-8 (with lossy fallback).
pub fn read_log(path: &PathBuf) -> Result<String, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Convenience: locate and read the newest RenderDoc log without a bridge.
pub fn load_latest_log() -> Result<(PathBuf, String), String> {
    let path = find_latest_log().ok_or("No RenderDoc log file found")?;
    let text = read_log(&path)?;
    Ok((path, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_filename_filter_matches_upstream_patterns() {
        let matches = |name: &str| {
            let lower = name.to_ascii_lowercase();
            lower.starts_with("renderdoc") && lower.ends_with(".log")
        };
        assert!(matches("RenderDoc_2026.11.17_12.30.01.log"));
        assert!(matches("RenderDoc_app_2026.11.17_12.30.01.log"));
        assert!(!matches("capture.rdc"));
        assert!(!matches("RenderDoc_2026.11.17_12.30.01.log.bak"));
    }

    #[test]
    fn read_log_reports_missing_file_without_panic() {
        let missing = PathBuf::from("missing-renderdocx-log-file.log");
        assert!(read_log(&missing).is_err());
    }
}
