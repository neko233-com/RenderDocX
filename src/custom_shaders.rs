//! Named custom visualization shaders shared by the desktop and Agent CLI.
use crate::{buffer_formats::FormatStore, replay::ShaderCompileFlag};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedCustomShader {
    pub name: String,
    pub encoding: u32,
    pub entry: String,
    pub source_hex: String,
    #[serde(default)]
    pub flags: Vec<ShaderCompileFlag>,
    pub apply_prefix: bool,
}

pub fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len().saturating_mul(2));
    for &byte in bytes {
        result.push(DIGITS[(byte >> 4) as usize] as char);
        result.push(DIGITS[(byte & 15) as usize] as char);
    }
    result
}
pub fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if value.is_empty() || !value.is_ascii() || value.len() % 2 != 0 {
        return Err("Shader source must be nonempty hexadecimal bytes".into());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |byte: u8| (byte as char).to_digit(16);
            Ok((digit(pair[0]).ok_or("Invalid shader hex")? * 16
                + digit(pair[1]).ok_or("Invalid shader hex")?) as u8)
        })
        .collect()
}
impl SavedCustomShader {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty()
            || self.name.trim() != self.name
            || self.name.chars().any(char::is_control)
            || !(1..=9).contains(&self.encoding)
            || self.entry.is_empty()
            || self.entry.contains('\0')
        {
            return Err("Invalid shader name, encoding or entry".into());
        }
        decode_hex(&self.source_hex)?;
        Ok(())
    }
    pub fn source(&self) -> Result<Vec<u8>, String> {
        decode_hex(&self.source_hex)
    }
}
/// The extensions accepted by qrenderdoc's TextureViewer. Its extension map
/// resolves the duplicate spvasm entry to OpenGLSPIRVAsm.
pub fn renderdoc_source_encoding(path: &Path) -> Option<u32> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "hlsl" => Some(5),
        "glsl" | "frag" => Some(2),
        "spvasm" => Some(8),
        "slang" => Some(9),
        _ => None,
    }
}

#[derive(Debug, Serialize)]
pub struct ImportResult {
    pub path: PathBuf,
    pub name: String,
    pub imported: bool,
    pub error: Option<String>,
}

/// Import a snapshot without modifying the source or replacing a record.
pub fn import_renderdoc_file(
    store: &ShaderStore,
    path: &Path,
) -> Result<SavedCustomShader, String> {
    let encoding =
        renderdoc_source_encoding(path).ok_or("Unsupported RenderDoc shader extension")?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Shader filename is not UTF-8")?
        .to_owned();
    let text = std::fs::read_to_string(path).map_err(|e| format!("Cannot read shader: {e}"))?;
    let source = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let shader = SavedCustomShader {
        name,
        encoding,
        entry: "main".into(),
        source_hex: encode_hex(source.as_bytes()),
        flags: Vec::new(),
        apply_prefix: true,
    };
    store.save(&shader, false)
}

/// Report failures per file so a collision or malformed file does not hide others.
pub fn import_renderdoc_directory(
    store: &ShaderStore,
    directory: &Path,
) -> Result<Vec<ImportResult>, String> {
    let mut paths = std::fs::read_dir(directory)
        .map_err(|e| format!("Cannot read shader directory: {e}"))?
        .map(|entry| entry.map(|entry| entry.path()).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    let mut results = Vec::new();
    for path in paths {
        if renderdoc_source_encoding(&path).is_none() || !path.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_default();
        let result = import_renderdoc_file(store, &path);
        results.push(ImportResult {
            path,
            name,
            imported: result.is_ok(),
            error: result.err(),
        });
    }
    Ok(results)
}

pub struct ShaderStore(FormatStore);
impl ShaderStore {
    pub fn new() -> Result<Self, String> {
        let root = FormatStore::new()?;
        let parent = root
            .directory()
            .parent()
            .ok_or("No configuration directory available")?;
        Ok(Self::at(parent.join("custom-shaders")))
    }
    pub fn at(directory: impl Into<PathBuf>) -> Self {
        Self(FormatStore::at(directory))
    }
    pub fn save(
        &self,
        shader: &SavedCustomShader,
        replace: bool,
    ) -> Result<SavedCustomShader, String> {
        shader.validate()?;
        let layout = serde_json::to_string(shader).map_err(|e| e.to_string())?;
        if replace {
            self.0.save(&shader.name, &layout)?;
        } else {
            self.0.save_new(&shader.name, &layout)?;
        }
        Ok(shader.clone())
    }
    pub fn get(&self, name: &str) -> Result<SavedCustomShader, String> {
        let value = self.0.get(name)?;
        let shader: SavedCustomShader =
            serde_json::from_str(&value.layout).map_err(|e| e.to_string())?;
        shader.validate()?;
        if shader.name != name {
            return Err("Saved shader name does not match its file".into());
        }
        Ok(shader)
    }
    pub fn list(&self) -> Result<Vec<SavedCustomShader>, String> {
        self.0
            .list()?
            .into_iter()
            .map(|value| self.get(&value.name))
            .collect()
    }
    pub fn delete(&self, name: &str) -> Result<(), String> {
        self.0.delete(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn imports_upstream_sources_without_overwriting_or_touching_files() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        std::fs::create_dir(&source).unwrap();
        let text = "\u{feff}float4 main() : SV_Target { return 1; }";
        std::fs::write(source.join("sample.HLSL"), text).unwrap();
        std::fs::write(source.join("bad.glsl"), [255]).unwrap();
        std::fs::write(source.join("other.txt"), "ignored").unwrap();
        let store = ShaderStore::at(root.path().join("library"));
        let results = import_renderdoc_directory(&store, &source).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results.iter().filter(|r| r.imported).count(), 1);
        let saved = store.get("sample.HLSL").unwrap();
        assert_eq!(saved.encoding, 5);
        assert_eq!(saved.entry, "main");
        assert_eq!(
            saved.source().unwrap(),
            text.trim_start_matches('\u{feff}').as_bytes()
        );
        assert!(saved.apply_prefix);
        assert!(
            import_renderdoc_directory(&store, &source)
                .unwrap()
                .iter()
                .all(|r| !r.imported)
        );
        assert_eq!(
            std::fs::read_to_string(source.join("sample.HLSL")).unwrap(),
            text
        );
        assert_eq!(renderdoc_source_encoding(Path::new("x.spvasm")), Some(8));
    }

    #[test]
    fn preserves_binary_metadata_and_requires_explicit_replace() {
        let dir = tempfile::tempdir().unwrap();
        let store = ShaderStore::at(dir.path());
        let mut shader = SavedCustomShader {
            name: "../测试".into(),
            encoding: 3,
            entry: "main".into(),
            source_hex: encode_hex(&[0, 255, 7]),
            flags: vec![ShaderCompileFlag {
                name: "test".into(),
                value: "x".into(),
            }],
            apply_prefix: false,
        };
        store.save(&shader, false).unwrap();
        assert_eq!(
            store.get(&shader.name).unwrap().source().unwrap(),
            [0, 255, 7]
        );
        assert!(store.save(&shader, false).is_err());
        shader.entry = "second".into();
        store.save(&shader, true).unwrap();
        assert_eq!(store.list().unwrap(), vec![shader.clone()]);
        shader.source_hex = "xyz".into();
        assert!(store.save(&shader, true).is_err());
        store.delete("../测试").unwrap();
        assert!(store.list().unwrap().is_empty());
    }
}
