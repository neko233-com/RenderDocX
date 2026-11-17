use crate::i18n::Language;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub language: Language,
    pub qrenderdoc: Option<PathBuf>,
    pub renderdoccmd: Option<PathBuf>,
    pub replay_bridge: Option<PathBuf>,
    pub replay_options: crate::replay::ReplayOptions,
    pub shader_processors: Vec<crate::shader_processor::NamedShaderProcessor>,
    pub texture_per_resource_settings: bool,
    pub texture_per_resource_flip_y: bool,
    pub texture_reset_range: bool,
    pub event_show_parameter_names: bool,
    pub event_show_all_parameters: bool,
    pub event_use_custom_action_names: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: Default::default(),
            qrenderdoc: None,
            renderdoccmd: None,
            replay_bridge: None,
            replay_options: Default::default(),
            shader_processors: Vec::new(),
            texture_per_resource_settings: true,
            texture_per_resource_flip_y: false,
            texture_reset_range: false,
            event_show_parameter_names: false,
            event_show_all_parameters: false,
            event_use_custom_action_names: true,
        }
    }
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
        self.save_to(&path)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
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
        assert!(settings.texture_per_resource_settings);
        assert!(!settings.texture_per_resource_flip_y);
        assert!(!settings.texture_reset_range);
        assert!(!settings.event_show_parameter_names);
        assert!(!settings.event_show_all_parameters);
        assert!(settings.event_use_custom_action_names);
        let mut updated = settings;
        updated.texture_per_resource_settings = false;
        updated.texture_per_resource_flip_y = true;
        updated.texture_reset_range = true;
        updated.event_show_parameter_names = true;
        updated.event_show_all_parameters = true;
        updated.event_use_custom_action_names = false;
        let reloaded: Settings =
            serde_json::from_str(&serde_json::to_string(&updated).unwrap()).unwrap();
        assert!(!reloaded.texture_per_resource_settings);
        assert!(reloaded.texture_per_resource_flip_y);
        assert!(reloaded.texture_reset_range);
        assert!(reloaded.event_show_parameter_names);
        assert!(reloaded.event_show_all_parameters);
        assert!(!reloaded.event_use_custom_action_names);
    }
}
