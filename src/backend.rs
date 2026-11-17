use crate::{
    i18n::Language,
    viewer_settings::{AxisMapping, CameraControls, ProjectionGuess},
};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct EventColumns {
    pub name: bool,
    pub event_id: bool,
    pub action: bool,
    pub duration: bool,
    pub annotation: bool,
}

impl Default for EventColumns {
    fn default() -> Self {
        Self {
            name: true,
            event_id: true,
            action: true,
            duration: true,
            annotation: true,
        }
    }
}

/// A saved window layout. Upstream qrenderdoc stores one default layout plus
/// six numbered slots; RenderDocX persists the same slots in `settings.json`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct WindowLayout {
    /// Stable serialization name of the active capture tab (see `CaptureTab::name`).
    pub tab: String,
    pub timeline_shown: bool,
    pub launch_config_shown: bool,
    pub remote_browser_shown: bool,
    pub frame_stats_raw: bool,
    pub texture_auto_fit: bool,
}

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
    pub event_auto_format: bool,
    pub event_columns: EventColumns,
    pub window_layouts: BTreeMap<String, WindowLayout>,
    pub recent_captures: Vec<PathBuf>,
    pub recent_capture_settings: Vec<PathBuf>,
    pub axis_mapping: AxisMapping,
    pub projection: ProjectionGuess,
    pub camera: CameraControls,
    pub always_replay_locally: bool,
}

/// Maximum number of entries kept in each recent-files list.
pub const RECENT_LIMIT: usize = 10;

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
            event_auto_format: true,
            event_columns: EventColumns::default(),
            window_layouts: BTreeMap::new(),
            recent_captures: Vec::new(),
            recent_capture_settings: Vec::new(),
            axis_mapping: AxisMapping::default(),
            projection: ProjectionGuess::default(),
            camera: CameraControls::default(),
            always_replay_locally: false,
        }
    }
}

impl Settings {
    /// Slot 0 is the default layout; slots 1..=6 are the numbered fast slots.
    pub fn layout_key(slot: usize) -> String {
        if slot == 0 {
            "default".to_owned()
        } else {
            slot.to_string()
        }
    }

    pub fn record_capture(&mut self, path: &Path) {
        record_recent(&mut self.recent_captures, path);
    }

    pub fn record_capture_settings(&mut self, path: &Path) {
        record_recent(&mut self.recent_capture_settings, path);
    }

    pub fn clear_recent_captures(&mut self) {
        self.recent_captures.clear();
    }

    pub fn clear_recent_capture_settings(&mut self) {
        self.recent_capture_settings.clear();
    }

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

    /// Parse a settings document edited by the in-app configuration editor.
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|error| error.to_string())
    }

    pub fn to_json_string(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
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

/// Insert the most recently used path at the front, drop duplicates and cap
/// the list length. Kept as a free function so it can be tested in isolation.
fn record_recent(list: &mut Vec<PathBuf>, path: &Path) {
    list.retain(|entry| entry.as_path() != path);
    list.insert(0, path.to_path_buf());
    list.truncate(RECENT_LIMIT);
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
    use super::{Settings, WindowLayout};
    use crate::viewer_settings::AxisDirection;
    use std::path::{Path, PathBuf};

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
        assert!(settings.event_auto_format);
        assert!(
            settings.event_columns.name
                && settings.event_columns.event_id
                && settings.event_columns.action
                && settings.event_columns.duration
                && settings.event_columns.annotation
        );
        let mut updated = settings;
        updated.texture_per_resource_settings = false;
        updated.texture_per_resource_flip_y = true;
        updated.texture_reset_range = true;
        updated.event_show_parameter_names = true;
        updated.event_show_all_parameters = true;
        updated.event_use_custom_action_names = false;
        updated.event_auto_format = false;
        updated.event_columns.action = false;
        updated.event_columns.annotation = false;
        let reloaded: Settings =
            serde_json::from_str(&serde_json::to_string(&updated).unwrap()).unwrap();
        assert!(!reloaded.texture_per_resource_settings);
        assert!(reloaded.texture_per_resource_flip_y);
        assert!(reloaded.texture_reset_range);
        assert!(reloaded.event_show_parameter_names);
        assert!(reloaded.event_show_all_parameters);
        assert!(!reloaded.event_use_custom_action_names);
        assert!(!reloaded.event_auto_format);
        assert!(!reloaded.event_columns.action && !reloaded.event_columns.annotation);
        assert!(
            reloaded.event_columns.name
                && reloaded.event_columns.event_id
                && reloaded.event_columns.duration
        );
    }

    #[test]
    fn window_layouts_survive_settings_round_trip() {
        let empty: Settings = serde_json::from_str("{}").unwrap();
        assert!(empty.window_layouts.is_empty());
        assert_eq!(Settings::layout_key(0), "default");
        assert_eq!(Settings::layout_key(3), "3");

        let mut settings = Settings::default();
        settings.window_layouts.insert(
            Settings::layout_key(2),
            WindowLayout {
                tab: "Textures".into(),
                timeline_shown: true,
                launch_config_shown: false,
                remote_browser_shown: true,
                frame_stats_raw: true,
                texture_auto_fit: false,
            },
        );
        let reloaded: Settings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(
            reloaded.window_layouts.get("2"),
            settings.window_layouts.get("2")
        );
    }

    #[test]
    fn recent_files_are_deduplicated_bounded_and_persisted() {
        let empty: Settings = serde_json::from_str("{}").unwrap();
        assert!(empty.recent_captures.is_empty());
        assert!(empty.recent_capture_settings.is_empty());

        let mut settings = Settings::default();
        settings.record_capture(Path::new("a.rdc"));
        settings.record_capture(Path::new("b.rdc"));
        settings.record_capture(Path::new("a.rdc")); // move to front, no duplicate
        assert_eq!(
            settings.recent_captures,
            vec![PathBuf::from("a.rdc"), PathBuf::from("b.rdc")]
        );
        for index in 0..super::RECENT_LIMIT + 3 {
            settings.record_capture_settings(Path::new(&format!("{index}.cap")));
        }
        assert_eq!(settings.recent_capture_settings.len(), super::RECENT_LIMIT);
        assert_eq!(
            settings.recent_capture_settings[0],
            PathBuf::from(format!("{}.cap", super::RECENT_LIMIT + 2))
        );
        let reloaded: Settings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(reloaded.recent_captures, settings.recent_captures);
        assert_eq!(reloaded.recent_capture_settings, settings.recent_capture_settings);
        settings.clear_recent_captures();
        settings.clear_recent_capture_settings();
        assert!(settings.recent_captures.is_empty());
        assert!(settings.recent_capture_settings.is_empty());
    }

    #[test]
    fn viewer_settings_round_trip_with_old_settings() {
        let empty: Settings = serde_json::from_str("{}").unwrap();
        assert!(empty.axis_mapping.validate().is_ok());
        assert!(empty.camera.validate().is_ok());
        let mut settings = Settings::default();
        settings.axis_mapping.x = AxisDirection::Left;
        settings.camera.near_plane = 0.5;
        settings.projection.fov = 60.0;
        let json = serde_json::to_string(&settings).unwrap();
        let reloaded: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(reloaded.axis_mapping, settings.axis_mapping);
        assert_eq!(reloaded.camera, settings.camera);
        assert_eq!(reloaded.projection, settings.projection);
    }

    #[test]
    fn configuration_editor_round_trips_and_rejects_invalid_json() {
        let settings = Settings::default();
        let text = settings.to_json_string();
        let parsed = Settings::from_json(&text).unwrap();
        assert_eq!(parsed.language, settings.language);
        assert_eq!(parsed.axis_mapping, settings.axis_mapping);
        assert_eq!(parsed.projection, settings.projection);
        assert_eq!(parsed.recent_captures, settings.recent_captures);
        assert!(Settings::from_json("not json").is_err());
        assert!(Settings::from_json("{\"language\": 42}").is_err());
        // Unknown fields are tolerated, matching serde(default) on Settings.
        assert!(Settings::from_json("{\"future_option\":true}").is_ok());
        assert!(!Settings::from_json("{}").unwrap().always_replay_locally);
        assert!(Settings::from_json("{\"always_replay_locally\":true}").unwrap().always_replay_locally);
    }
}
