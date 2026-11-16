//! Named custom visualization shaders shared by the desktop and Agent CLI.
use crate::{buffer_formats::FormatStore, replay::ShaderCompileFlag};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
