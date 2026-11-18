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

/// Result of statically inspecting an extension's `__init__.py` for the
/// upstream UI-extension entry points. No CPython is embedded: this is a
/// lightweight source scan, enough to validate the extension before enabling.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtensionApi {
    pub has_register: bool,
    pub has_unregister: bool,
    pub syntax_hint: String,
}

/// Scan Python source for top-level `def register`/`def unregister` definitions.
/// Handles optional `async def` and leading whitespace/indentation.
pub fn inspect_source(source: &str) -> ExtensionApi {
    let has_def = |name: &str| -> bool {
        source.lines().any(|line| {
            let trimmed = line.trim_start();
            let body = trimmed.strip_prefix("async ").unwrap_or(trimmed);
            body.strip_prefix("def ")
                .and_then(|rest| rest.split('(').next())
                .is_some_and(|identifier| identifier.trim() == name)
        })
    };
    let has_register = has_def("register");
    let has_unregister = has_def("unregister");
    let syntax_hint = if source.trim().is_empty() {
        "empty source".to_owned()
    } else if has_register && has_unregister {
        "valid entry points".to_owned()
    } else {
        "missing entry point".to_owned()
    };
    ExtensionApi {
        has_register,
        has_unregister,
        syntax_hint,
    }
}

/// Read and statically inspect an extension's `__init__.py`.
pub fn inspect_extension(dir: &Path) -> ExtensionApi {
    match fs::read_to_string(dir.join("__init__.py")) {
        Ok(source) => inspect_source(&source),
        Err(_) => ExtensionApi::default(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledExtension {
    pub dir: PathBuf,
    pub manifest: ExtensionManifest,
    pub api: ExtensionApi,
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
            let api = inspect_extension(&path);
            result.push(InstalledExtension {
                dir: path,
                manifest,
                api,
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
    fn inspects_python_entry_points_without_cpython() {
        let source = r#"
import renderdoc as rd
def register(version, pyrenderdoc):
    print("loaded")
def unregister():
    pass
"#;
        let api = inspect_source(source);
        assert!(api.has_register && api.has_unregister);
        assert_eq!(api.syntax_hint, "valid entry points");

        let async_source = "async def register(v, p):\n    pass\ndef unregister():\n    pass\n";
        let async_api = inspect_source(async_source);
        assert!(async_api.has_register && async_api.has_unregister);

        let missing = inspect_source("def register():\n    pass\n");
        assert!(!missing.has_unregister);
        assert_eq!(missing.syntax_hint, "missing entry point");
        assert_eq!(inspect_source("").syntax_hint, "empty source");
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
