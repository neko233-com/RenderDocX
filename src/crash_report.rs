//! Local crash capture and reporting, replacing qrenderdoc's `CrashDialog`
//! with a privacy-preserving native implementation.
//!
//! On panic the message, source location and a captured backtrace are appended
//! to a JSON-lines file. The GUI can list, copy and clear these reports, and
//! open the containing folder. No data is uploaded anywhere: unlike the
//! upstream crash reporter there is no network transmission unless the user
//! manually shares the file.

use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CrashReport {
    /// Unix timestamp in seconds.
    pub timestamp: u64,
    pub message: String,
    pub location: Option<String>,
    pub backtrace: Option<String>,
}

static CRASH_LOG: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Default crash-log path under the RenderDocX configuration directory.
pub fn default_log_path() -> Option<PathBuf> {
    crate::backend::Settings::config_dir().map(|dir| dir.join("crash.log"))
}

/// Install a panic hook that records crashes to `path` and then runs the
/// previous hook (so the normal panic output is preserved).
pub fn install_panic_hook(path: PathBuf) {
    let mut guard = CRASH_LOG.lock().unwrap_or_else(|error| error.into_inner());
    if guard.is_some() {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    *guard = Some(path);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        record_panic(info);
        previous(info);
    }));
}

/// Record one panic into the configured crash log (no-op if not installed).
pub fn record_panic(info: &std::panic::PanicHookInfo<'_>) {
    let path = CRASH_LOG
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    let Some(path) = path else { return };
    let message = if let Some(text) = info.payload().downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = info.payload().downcast_ref::<String>() {
        text.clone()
    } else {
        "non-string panic payload".to_owned()
    };
    let location = info
        .location()
        .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()));
    let report = CrashReport {
        timestamp: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        message,
        location,
        backtrace: Some(std::backtrace::Backtrace::force_capture().to_string()),
    };
    append_report(&path, &report);
}

/// Append one report as a JSON line; failures are swallowed (crash handling
/// must never panic).
pub fn append_report(path: &Path, report: &CrashReport) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut line) = serde_json::to_string(report) {
        line.push('\n');
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = file.write_all(line.as_bytes());
        }
    }
}

/// Read all crash reports, skipping malformed lines.
pub fn read_reports(path: &Path) -> Vec<CrashReport> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<CrashReport>(line).ok())
        .collect()
}

/// Delete the crash log, returning whether a file was removed.
pub fn clear(path: &Path) -> Result<bool, String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(timestamp: u64, message: &str) -> CrashReport {
        CrashReport {
            timestamp,
            message: message.to_owned(),
            location: Some("src/main.rs:1:1".to_owned()),
            backtrace: None,
        }
    }

    #[test]
    fn reports_round_trip_and_skip_bad_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crash.log");
        append_report(&path, &sample(100, "boom"));
        append_report(&path, &sample(200, "second"));
        // A malformed line must be ignored rather than break parsing.
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"not json\n").unwrap();
        let reports = read_reports(&path);
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].message, "boom");
        assert_eq!(reports[1].timestamp, 200);
    }

    #[test]
    fn missing_log_reads_empty_and_clear_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.log");
        assert!(read_reports(&path).is_empty());
        assert_eq!(clear(&path), Ok(false));
        append_report(&path, &sample(1, "x"));
        assert_eq!(clear(&path), Ok(true));
        assert!(!path.exists());
    }
}
