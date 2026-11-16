//! Persistent, independently named buffer format definitions shared by GUI and CLI.
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedFormat {
    pub name: String,
    pub layout: String,
}

pub struct FormatStore {
    directory: PathBuf,
}
impl FormatStore {
    pub fn new() -> Result<Self, String> {
        let root = if let Some(path) = std::env::var_os("RENDERDOCX_CONFIG_DIR") {
            PathBuf::from(path)
        } else {
            ProjectDirs::from("com", "neko233", "RenderDocX")
                .ok_or("No configuration directory available")?
                .config_dir()
                .to_path_buf()
        };
        Ok(Self::at(root.join("buffer-formats")))
    }
    pub fn at(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }
    fn path(&self, name: &str) -> Result<PathBuf, String> {
        if name.is_empty() || name.trim() != name || name.chars().any(char::is_control) {
            return Err(
                "Format name must be nonempty, trimmed and contain no control characters".into(),
            );
        }
        let encoded: String = name
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok(self.directory.join(format!("{encoded}.json")))
    }
    pub fn save(&self, name: &str, layout: &str) -> Result<SavedFormat, String> {
        self.save_inner(name, layout, true)
    }
    pub fn save_new(&self, name: &str, layout: &str) -> Result<SavedFormat, String> {
        self.save_inner(name, layout, false)
    }
    fn save_inner(&self, name: &str, layout: &str, replace: bool) -> Result<SavedFormat, String> {
        let path = self.path(name)?;
        if layout.trim().is_empty() {
            return Err("Format layout cannot be empty".into());
        }
        let entry = SavedFormat {
            name: name.to_owned(),
            layout: layout.to_owned(),
        };
        let data = serde_json::to_vec_pretty(&entry).map_err(|e| e.to_string())?;
        fs::create_dir_all(&self.directory).map_err(|e| e.to_string())?;
        let mut tmp =
            tempfile::NamedTempFile::new_in(&self.directory).map_err(|e| e.to_string())?;
        tmp.write_all(&data)
            .and_then(|_| tmp.flush())
            .map_err(|e| e.to_string())?;
        if replace {
            tmp.persist(&path).map_err(|e| e.to_string())?;
        } else {
            tmp.persist_noclobber(&path).map_err(|e| e.to_string())?;
        }
        Ok(entry)
    }
    pub fn get(&self, name: &str) -> Result<SavedFormat, String> {
        let path = self.path(name)?;
        let data = fs::read(&path).map_err(|e| e.to_string())?;
        let entry: SavedFormat = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
        if entry.name != name {
            return Err("Saved format name does not match its file".into());
        }
        Ok(entry)
    }
    pub fn delete(&self, name: &str) -> Result<(), String> {
        fs::remove_file(self.path(name)?).map_err(|e| e.to_string())
    }
    pub fn list(&self) -> Result<Vec<SavedFormat>, String> {
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.to_string()),
        };
        let mut result = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json")
                || !entry.file_type().map_err(|e| e.to_string())?.is_file()
            {
                continue;
            }
            let value: SavedFormat =
                serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
            if self.path(&value.name)? != path {
                return Err(format!("Invalid saved format filename: {}", path.display()));
            }
            result.push(value);
        }
        result.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(result)
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn records_round_trip_overwrite_and_remove_without_path_escape() {
        let temp = tempfile::tempdir().unwrap();
        let store = FormatStore::at(temp.path().join("formats"));
        assert!(store.list().unwrap().is_empty());
        store.save("../纹理", "float4 color;").unwrap();
        assert_eq!(store.get("../纹理").unwrap().layout, "float4 color;");
        store.save("../纹理", "uint value;").unwrap();
        store.save("Alpha", "[[single]] uint x;").unwrap();
        assert_eq!(
            store
                .list()
                .unwrap()
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            ["../纹理", "Alpha"]
        );
        assert_eq!(store.get("../纹理").unwrap().layout, "uint value;");
        assert!(store.save(" bad ", "uint x;").is_err());
        assert!(store.save("valid", " ").is_err());
        store.delete("../纹理").unwrap();
        assert!(store.get("../纹理").is_err());
        assert_eq!(store.list().unwrap().len(), 1);
    }
}
