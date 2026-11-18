//! Extension discovery and manifest parsing for RenderDocX.
//!
//! Upstream qrenderdoc discovers UI extensions by scanning directories for
//! `extension.json` manifests (`PythonShell.cpp`). RenderDocX parses the same
//! schema natively with serde_json and tracks enable/disable state; actual
//! script execution uses the (separate) pure-Rust Python process path.

use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ExtensionManifest {
    #[serde(default)]
    pub extension_api: u32,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub minimum_renderdoc: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub url: String,
}

impl ExtensionManifest {
    pub fn parse(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|error| error.to_string())
    }

    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = dir.join("extension.json");
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
        Self::parse(&text)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledExtension {
    pub dir: PathBuf,
    pub manifest: ExtensionManifest,
}

/// Scan a directory for immediate subdirectories containing `extension.json`.
pub fn discover(root: &Path) -> Result<Vec<InstalledExtension>, String> {
    let mut result = Vec::new();
    let entries = fs::read_dir(root)
        .map_err(|error| format!("Could not scan {}: {error}", root.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || !path.join("extension.json").is_file() {
            continue;
        }
        if let Ok(manifest) = ExtensionManifest::load(&path) {
            result.push(InstalledExtension {
                dir: path,
                manifest,
            });
        }
    }
    result.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
    Ok(result)
}

/// Default per-user extension directory: `<config>/extensions`.
pub fn default_extension_root(config_dir: &Path) -> PathBuf {
    config_dir.join("extensions")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"{
        "extension_api": 1,
        "name": "My Extension",
        "version": "1.0",
        "minimum_renderdoc": "1.48",
        "description": "Template",
        "author": "Me",
        "url": "https://example.com"
    }"#;

    #[test]
    fn parses_upstream_manifest_schema() {
        let manifest = ExtensionManifest::parse(MANIFEST).unwrap();
        assert_eq!(manifest.extension_api, 1);
        assert_eq!(manifest.name, "My Extension");
        assert_eq!(manifest.minimum_renderdoc, "1.48");
        assert_eq!(manifest.url, "https://example.com");
    }

    #[test]
    fn missing_required_fields_are_rejected() {
        assert!(ExtensionManifest::parse("{}").is_err());
        assert!(ExtensionManifest::parse("not json").is_err());
    }

    #[test]
    fn discovery_finds_valid_extension_dirs() {
        let root = tempfile::tempdir().unwrap();
        let good = root.path().join("good");
        fs::create_dir(&good).unwrap();
        fs::write(good.join("extension.json"), MANIFEST).unwrap();
        let bad = root.path().join("bad");
        fs::create_dir(&bad).unwrap();
        let found = discover(root.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].manifest.name, "My Extension");
    }
}
