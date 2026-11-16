use crate::i18n::Language;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub language: Language,
    pub qrenderdoc: Option<PathBuf>,
    pub renderdoccmd: Option<PathBuf>,
    pub replay_bridge: Option<PathBuf>,
    pub replay_options: crate::replay::ReplayOptions,
    pub shader_processors: Vec<crate::shader_processor::NamedShaderProcessor>,
}

impl Settings {
    fn path() -> Option<PathBuf> {
        ProjectDirs::from("com", "neko233", "RenderDocX")
            .map(|p| p.config_dir().join("settings.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path().ok_or("No configuration directory available")?;
        fs::create_dir_all(path.parent().ok_or("Invalid configuration path")?)
            .map_err(|e| e.to_string())?;
        let data = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        fs::write(path, data).map_err(|e| e.to_string())
    }
}

pub(crate) fn find_tool(configured: &Option<PathBuf>, name: &str) -> Result<PathBuf, String> {
    if let Some(path) = configured {
        return if path.is_file() {
            Ok(path.clone())
        } else {
            Err(format!("{}: {}", name, path.display()))
        };
    }
    let executable = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    if let Ok(current) = std::env::current_exe() {
        if let Some(parent) = current.parent() {
            let candidate = parent.join(&executable);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    if cfg!(windows) {
        if let Some(root) = std::env::var_os("ProgramFiles") {
            let candidate = PathBuf::from(root).join("RenderDoc").join(&executable);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    which::which(&executable).map_err(|_| format!("{executable}: not found"))
}

pub fn open_capture(settings: &Settings, capture: &Path) -> Result<(), String> {
    let program = find_tool(&settings.qrenderdoc, "qrenderdoc")?;
    Command::new(program)
        .arg(capture)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

pub fn open_upstream(settings: &Settings) -> Result<(), String> {
    let program = find_tool(&settings.qrenderdoc, "qrenderdoc")?;
    Command::new(program)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::Settings;

    #[test]
    fn older_settings_default_to_no_external_processors() {
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert!(settings.shader_processors.is_empty());
    }
}
