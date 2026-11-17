//! Saving a copy of an open `.rdc` capture.
//!
//! Upstream `action_Save_Capture_As` persists the current capture to a new path.
//! RenderDocX uses a byte-for-byte copy (the capture's embedded dependencies
//! travel with it), keeping the currently open replay session untouched.

use std::path::Path;

/// Copy an open capture to a new destination without disturbing the session.
pub fn save_copy(source: &Path, destination: &Path) -> Result<(), String> {
    if source == destination {
        return Err("The destination must differ from the open capture".into());
    }
    if source.as_os_str().is_empty() || destination.as_os_str().is_empty() {
        return Err("A source and destination path are required".into());
    }
    if !source.is_file() {
        return Err(format!("Capture file does not exist: {}", source.display()));
    }
    std::fs::copy(source, destination)
        .map(|_| ())
        .map_err(|error| {
            format!(
                "Could not save capture to {}: {error}",
                destination.display()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_copy_rejects_invalid_paths_and_writes_a_valid_copy() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.rdc");
        std::fs::write(&source, b"rdc-fixture").unwrap();
        assert!(save_copy(&source, &source).is_err());
        assert!(save_copy(&dir.path().join("missing.rdc"), &dir.path().join("out.rdc")).is_err());
        assert!(save_copy(Path::new(""), &dir.path().join("out.rdc")).is_err());

        let destination = dir.path().join("nested").join("copy.rdc");
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        save_copy(&source, &destination).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"rdc-fixture");
        assert_eq!(std::fs::read(&source).unwrap(), b"rdc-fixture");
    }
}
