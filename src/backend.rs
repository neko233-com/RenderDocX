use crate::{
    camera_keys::KeyBindings,
    i18n::Language,
    remote_hosts::RemoteHost,
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
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct WindowLayout {
    /// Stable serialization name of the active capture tab (see `CaptureTab::name`).
    pub tab: String,
    pub timeline_shown: bool,
    pub launch_config_shown: bool,
    pub remote_browser_shown: bool,
    pub frame_stats_raw: bool,
    pub texture_auto_fit: bool,
    /// Fraction of the capture workspace used by the left dock column.
    #[serde(default = "default_left_ratio")]
    pub left_ratio: f32,
    /// Fraction of the left column used by the Event Browser (rest is API
    /// Inspector), matching qrenderdoc's default 0.7/0.3 split.
    #[serde(default = "default_api_ratio")]
    pub api_ratio: f32,
    /// Fraction of the capture row used by the central panel; the remainder is
    /// the action-details panel on the right.
    #[serde(default = "default_details_ratio")]
    pub details_ratio: f32,
    /// Fraction of the capture window used by the Timeline top dock.
    #[serde(default = "default_timeline_ratio")]
    pub timeline_ratio: f32,
    /// Fraction of the right details column used by right-docked panels.
    #[serde(default = "default_right_ratio")]
    pub right_ratio: f32,
    /// Fraction of the capture row used by the bottom dock row.
    #[serde(default = "default_bottom_ratio")]
    pub bottom_ratio: f32,
    /// Stable serialization names of the main tab order.
    #[serde(default)]
    pub main_tab_order: Vec<String>,
    /// Stable serialization names of the auxiliary panel tab order.
    #[serde(default)]
    pub aux_tab_order: Vec<String>,
    /// Panels moved from the left dock to the right details column.
    #[serde(default)]
    pub dock_right: Vec<String>,
    /// Panels moved from the left dock to the bottom row.
    #[serde(default)]
    pub dock_bottom: Vec<String>,
    #[serde(default = "default_true")]
    pub textures_shown: bool,
    #[serde(default = "default_true")]
    pub pipeline_shown: bool,
    #[serde(default = "default_true")]
    pub mesh_shown: bool,
    #[serde(default = "default_true")]
    pub capture_dialog_shown: bool,
    #[serde(default = "default_true")]
    pub event_browser_shown: bool,
    #[serde(default = "default_true")]
    pub api_inspector_shown: bool,
}

impl Default for WindowLayout {
    fn default() -> Self {
        Self {
            tab: String::new(),
            timeline_shown: false,
            launch_config_shown: false,
            remote_browser_shown: false,
            frame_stats_raw: false,
            texture_auto_fit: false,
            left_ratio: default_left_ratio(),
            api_ratio: default_api_ratio(),
            details_ratio: default_details_ratio(),
            timeline_ratio: default_timeline_ratio(),
            right_ratio: default_right_ratio(),
            bottom_ratio: default_bottom_ratio(),
            main_tab_order: Vec::new(),
            aux_tab_order: Vec::new(),
            dock_right: Vec::new(),
            dock_bottom: Vec::new(),
            textures_shown: true,
            pipeline_shown: true,
            mesh_shown: true,
            capture_dialog_shown: true,
            event_browser_shown: true,
            api_inspector_shown: true,
        }
    }
}

fn default_left_ratio() -> f32 {
    0.25
}

fn default_api_ratio() -> f32 {
    0.7
}

fn default_details_ratio() -> f32 {
    0.72
}

fn default_timeline_ratio() -> f32 {
    0.20
}

fn default_right_ratio() -> f32 {
    0.45
}

fn default_bottom_ratio() -> f32 {
    0.30
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub language: Language,
    pub qrenderdoc: Option<PathBuf>,
    pub renderdoccmd: Option<PathBuf>,
    pub replay_bridge: Option<PathBuf>,
    /// Optional path to renderdoc.dll for reading core info via stable C ABI.
    pub renderdoc_core: Option<PathBuf>,
    /// Manifest names of extensions the user has enabled.
    pub enabled_extensions: Vec<String>,
    /// Override for the directory where new captures are stored.
    pub capture_directory: Option<PathBuf>,
    /// Default directory offered when saving captures.
    pub default_save_directory: Option<PathBuf>,
    /// Remote server bookmarks (upstream RemoteHost schema).
    pub remote_hosts: Vec<RemoteHost>,
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
    pub camera_keys: KeyBindings,
    /// UI theme: "light" or "dark" (upstream UIStyle equivalent).
    pub theme: String,
    /// Analytics consent: "unset" | "granted" | "denied" (local counter only).
    pub analytics_consent: String,
    /// Check GitHub releases for updates at startup (read-only, no download).
    pub auto_check_updates: bool,
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
            renderdoc_core: None,
            enabled_extensions: Vec::new(),
            capture_directory: None,
            default_save_directory: None,
            remote_hosts: Vec::new(),
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
            camera_keys: KeyBindings::default(),
            theme: "dark".to_owned(),
            analytics_consent: "unset".to_owned(),
            auto_check_updates: true,
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

    /// Directory holding settings.json; also the parent of user extensions.
    pub fn config_dir() -> Option<PathBuf> {
        ProjectDirs::from("com", "neko233", "RenderDocX").map(|p| p.config_dir().to_path_buf())
    }

    fn path() -> Option<PathBuf> {
        Self::config_dir().map(|dir| dir.join("settings.json"))
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
    // A stale configured path (for example a developer checkout that has moved)
    // must not make the packaged application unusable. Fall through to the
    // version-matched runtime installed next to the executable.
    if let Some(path) = configured {
        if path.is_file() {
            return Ok(path.clone());
        }
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
            // Portable/dev builds keep the version-matched runtime in
            // target/renderdocx-runtime while the executable sits in
            // target/release or target/debug.
            if let Some(target_dir) = parent.parent() {
                let candidate = target_dir.join("renderdocx-runtime").join(&executable);
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }
    // `cargo run` from the repository root.
    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd
            .join("target")
            .join("renderdocx-runtime")
            .join(&executable);
        if candidate.is_file() {
            return Ok(candidate);
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
                ..Default::default()
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
        let mut with_host = Settings::default();
        with_host.remote_hosts.push(super::RemoteHost::new("host:39070"));
        let reloaded = Settings::from_json(&with_host.to_json_string()).unwrap();
        assert_eq!(reloaded.remote_hosts, with_host.remote_hosts);
        with_host.capture_directory = Some(PathBuf::from("C:/caps"));
        with_host.default_save_directory = Some(PathBuf::from("C:/saves"));
        let reloaded = Settings::from_json(&with_host.to_json_string()).unwrap();
        assert_eq!(reloaded.capture_directory, with_host.capture_directory);
        assert_eq!(reloaded.default_save_directory, with_host.default_save_directory);
        with_host.theme = "light".to_owned();
        assert_eq!(Settings::from_json(&with_host.to_json_string()).unwrap().theme, "light");
    }
}
