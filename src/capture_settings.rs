//! Import/export of RenderDoc `.cap` capture-settings files.
//!
//! Format authority (this repository's pinned qrenderdoc snapshot):
//! - `qrenderdoc/Windows/Dialogs/CaptureDialog.cpp`: `JSON_ID = "rdocCaptureSettings"`,
//!   `JSON_VER = 1`, `SaveSettings` / `LoadSettingsFromDisk` write a document of
//!   `{ "<magic>": 1, "settings": <CaptureSettings> }`.
//! - `qrenderdoc/Code/Interface/QRDInterface.cpp`: `CaptureSettings::operator QVariant`
//!   and `CaptureSettings(const QVariant&)` define the `settings` object keys and
//!   the 12 camelCase `options` keys (including the legacy `verifyMapWrites`
//!   fallback and the `captureCallstacksOnlyDraws` -> `capture_callstacks_only_actions`
//!   rename). Missing keys read as `false` / `0` via Qt's `toBool()` / `toUInt()`.
//! - `qrenderdoc/Code/QRDUtils.cpp`: `SaveToJSON` / `LoadFromJSON` validate the
//!   magic identifier and version before returning the settings object.
//!
//! RenderDocX intentionally ignores `inject`, `autoStart`, `queuedFrameCap` and
//! `numQueuedFrames` on import (there is no corresponding GUI); they are exposed
//! so callers can warn, and are written as upstream defaults on export.

use crate::live::{CaptureOptions, EnvOperation, EnvSeparator, EnvironmentModification};
use serde_json::{json, Map, Value};
use std::path::Path;

pub const MAGIC: &str = "rdocCaptureSettings";
pub const MAGIC_VERSION: u64 = 1;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaptureSettings {
    pub executable: String,
    pub working_directory: String,
    pub command_line: String,
    pub environment: Vec<EnvironmentModification>,
    pub options: CaptureOptions,
    /// Upstream-only: inject into a running process. Not representable in the GUI.
    pub inject: bool,
    /// Upstream-only: start the launch immediately after loading the settings.
    pub auto_start: bool,
    /// Upstream-only: queued frame capture settings.
    pub queued_frame_cap: u32,
    pub num_queued_frames: u32,
}

impl CaptureSettings {
    /// Build the exact upstream JSON document (`SaveToJSON` uses a sorted map,
    /// which is why serde_json's default BTreeMap-backed `Map` is used here).
    pub fn to_json(&self) -> Value {
        let mut options = Map::new();
        options.insert("allowVSync".into(), json!(self.options.allow_vsync));
        options.insert(
            "allowFullscreen".into(),
            json!(self.options.allow_fullscreen),
        );
        options.insert("apiValidation".into(), json!(self.options.api_validation));
        options.insert(
            "captureCallstacks".into(),
            json!(self.options.capture_callstacks),
        );
        options.insert(
            "captureCallstacksOnlyDraws".into(),
            json!(self.options.capture_callstacks_only_actions),
        );
        options.insert(
            "delayForDebugger".into(),
            json!(self.options.delay_for_debugger),
        );
        options.insert(
            "verifyBufferAccess".into(),
            json!(self.options.verify_buffer_access),
        );
        options.insert(
            "hookIntoChildren".into(),
            json!(self.options.hook_into_children),
        );
        options.insert(
            "refAllResources".into(),
            json!(self.options.ref_all_resources),
        );
        options.insert(
            "captureAllCmdLists".into(),
            json!(self.options.capture_all_cmd_lists),
        );
        options.insert(
            "debugOutputMute".into(),
            json!(self.options.debug_output_mute),
        );
        options.insert(
            "softMemoryLimit".into(),
            json!(self.options.soft_memory_limit),
        );

        let environment: Vec<Value> = self
            .environment
            .iter()
            .map(|entry| {
                let operation = match entry.operation {
                    EnvOperation::Set => "Set",
                    EnvOperation::Append => "Append",
                    EnvOperation::Prepend => "Prepend",
                };
                let separator = match entry.separator {
                    EnvSeparator::Platform => "Platform",
                    EnvSeparator::SemiColon => "SemiColon",
                    EnvSeparator::Colon => "Colon",
                    EnvSeparator::NoSep => "NoSep",
                };
                json!({
                    "variable": entry.name,
                    "value": entry.value,
                    "type": operation,
                    "separator": separator,
                })
            })
            .collect();

        let mut settings = Map::new();
        settings.insert("inject".into(), json!(self.inject));
        settings.insert("autoStart".into(), json!(self.auto_start));
        settings.insert("executable".into(), json!(self.executable));
        settings.insert("workingDir".into(), json!(self.working_directory));
        settings.insert("commandLine".into(), json!(self.command_line));
        settings.insert("environment".into(), Value::Array(environment));
        settings.insert("options".into(), Value::Object(options));
        settings.insert("queuedFrameCap".into(), json!(self.queued_frame_cap));
        settings.insert("numQueuedFrames".into(), json!(self.num_queued_frames));

        let mut document = Map::new();
        document.insert(MAGIC.into(), json!(MAGIC_VERSION));
        document.insert("settings".into(), Value::Object(settings));
        Value::Object(document)
    }

    pub fn to_json_string(&self) -> String {
        // Serializing a serde_json::Value never fails.
        serde_json::to_string_pretty(&self.to_json()).unwrap_or_default()
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let document: Value =
            serde_json::from_str(text).map_err(|e| format!("Invalid JSON: {e}"))?;
        let Some(version) = document.get(MAGIC) else {
            return Err("Unrecognised capture settings file".into());
        };
        if !version_is_one(version) {
            return Err("Unsupported capture settings version".into());
        }
        // Upstream treats a missing/invalid `settings` object as an empty map,
        // i.e. every field falls back to Qt's toBool()/toUInt() default.
        let settings = document.get("settings").and_then(Value::as_object);
        let get = |key: &str| settings.and_then(|map| map.get(key));
        let option = |key: &str| {
            get("options")
                .and_then(Value::as_object)
                .and_then(|map| map.get(key))
        };

        let mut options = CaptureOptions::default();
        options.allow_vsync = to_bool(option("allowVSync"));
        options.allow_fullscreen = to_bool(option("allowFullscreen"));
        options.api_validation = to_bool(option("apiValidation"));
        options.capture_callstacks = to_bool(option("captureCallstacks"));
        options.capture_callstacks_only_actions = to_bool(option("captureCallstacksOnlyDraws"));
        options.delay_for_debugger = to_uint(option("delayForDebugger"));
        // Old name for verifyBufferAccess was verifyMapWrites.
        options.verify_buffer_access = if option("verifyBufferAccess").is_some() {
            to_bool(option("verifyBufferAccess"))
        } else {
            to_bool(option("verifyMapWrites"))
        };
        options.hook_into_children = to_bool(option("hookIntoChildren"));
        options.ref_all_resources = to_bool(option("refAllResources"));
        options.capture_all_cmd_lists = to_bool(option("captureAllCmdLists"));
        options.debug_output_mute = to_bool(option("debugOutputMute"));
        options.soft_memory_limit = to_uint(option("softMemoryLimit"));

        let mut environment = Vec::new();
        if let Some(Value::Array(entries)) = get("environment") {
            for entry in entries {
                let entry = entry.as_object();
                let field = |key: &str| entry.and_then(|map| map.get(key)).and_then(Value::as_str);
                environment.push(EnvironmentModification {
                    name: field("variable").unwrap_or_default().to_owned(),
                    value: field("value").unwrap_or_default().to_owned(),
                    operation: match field("type") {
                        Some("Append") => EnvOperation::Append,
                        Some("Prepend") => EnvOperation::Prepend,
                        _ => EnvOperation::Set,
                    },
                    separator: match field("separator") {
                        Some("SemiColon") => EnvSeparator::SemiColon,
                        Some("Colon") => EnvSeparator::Colon,
                        Some("Platform") => EnvSeparator::Platform,
                        _ => EnvSeparator::NoSep,
                    },
                });
            }
        }

        Ok(Self {
            executable: as_text(get("executable")),
            working_directory: as_text(get("workingDir")),
            command_line: as_text(get("commandLine")),
            environment,
            options,
            inject: to_bool(get("inject")),
            auto_start: to_bool(get("autoStart")),
            queued_frame_cap: to_uint(get("queuedFrameCap")),
            num_queued_frames: to_uint(get("numQueuedFrames")),
        })
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        Self::parse(&text)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let mut text = self.to_json_string();
        text.push('\n');
        std::fs::write(path, text).map_err(|e| format!("Could not write {}: {e}", path.display()))
    }

    /// Upstream fields that the RenderDocX GUI cannot represent; callers should
    /// mention them when importing so nothing is silently dropped.
    pub fn unsupported_fields(&self) -> Vec<&'static str> {
        let mut fields = Vec::new();
        if self.inject {
            fields.push("inject");
        }
        if self.auto_start {
            fields.push("autoStart");
        }
        if self.queued_frame_cap != 0 {
            fields.push("queuedFrameCap");
        }
        if self.num_queued_frames != 0 {
            fields.push("numQueuedFrames");
        }
        fields
    }
}

fn version_is_one(value: &Value) -> bool {
    match value {
        Value::Number(number) => number.as_f64() == Some(1.0),
        Value::String(text) => text == "1",
        _ => false,
    }
}

fn as_text(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn to_bool(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_f64().is_some_and(|value| value != 0.0),
        _ => false,
    }
}

fn to_uint(value: Option<&Value>) -> u32 {
    match value {
        Some(Value::Number(value)) => value
            .as_u64()
            .map(|value| value.min(u32::MAX as u64) as u32)
            .or_else(|| {
                value
                    .as_f64()
                    .filter(|value| value.is_finite() && *value >= 0.0)
                    .map(|value| (value.min(u32::MAX as f64)) as u32)
            })
            .unwrap_or(0),
        Some(Value::Bool(true)) => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn export_document_matches_upstream_schema() {
        let settings = CaptureSettings {
            executable: "C:\\games\\app.exe".into(),
            working_directory: "C:\\games".into(),
            command_line: "--windowed".into(),
            environment: vec![EnvironmentModification {
                name: "VK_INSTANCE_LAYERS".into(),
                value: "VK_LAYER_RENDERDOC_Capture".into(),
                operation: EnvOperation::Append,
                separator: EnvSeparator::SemiColon,
            }],
            options: CaptureOptions {
                capture_callstacks_only_actions: true,
                delay_for_debugger: 3,
                verify_buffer_access: true,
                soft_memory_limit: 4096,
                ..Default::default()
            },
            ..Default::default()
        };
        let document = settings.to_json();
        assert_eq!(document[MAGIC], json!(1));
        let object = document["settings"].as_object().unwrap();
        let mut keys: Vec<&String> = object.keys().collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "autoStart",
                "commandLine",
                "environment",
                "executable",
                "inject",
                "numQueuedFrames",
                "options",
                "queuedFrameCap",
                "workingDir",
            ]
        );
        assert_eq!(object["executable"], json!("C:\\games\\app.exe"));
        assert_eq!(object["workingDir"], json!("C:\\games"));
        assert_eq!(object["commandLine"], json!("--windowed"));
        assert_eq!(object["inject"], json!(false));
        assert_eq!(object["autoStart"], json!(false));
        assert_eq!(object["queuedFrameCap"], json!(0));
        assert_eq!(object["numQueuedFrames"], json!(0));
        assert_eq!(
            object["environment"],
            json!([{
                "variable": "VK_INSTANCE_LAYERS",
                "value": "VK_LAYER_RENDERDOC_Capture",
                "type": "Append",
                "separator": "SemiColon",
            }])
        );
        let options = object["options"].as_object().unwrap();
        let mut option_keys: Vec<&String> = options.keys().collect();
        option_keys.sort_unstable();
        assert_eq!(
            option_keys,
            [
                "allowFullscreen",
                "allowVSync",
                "apiValidation",
                "captureAllCmdLists",
                "captureCallstacks",
                "captureCallstacksOnlyDraws",
                "debugOutputMute",
                "delayForDebugger",
                "hookIntoChildren",
                "refAllResources",
                "softMemoryLimit",
                "verifyBufferAccess",
            ]
        );
        assert_eq!(options["captureCallstacksOnlyDraws"], json!(true));
        assert_eq!(options["delayForDebugger"], json!(3));
        assert_eq!(options["verifyBufferAccess"], json!(true));
        assert_eq!(options["softMemoryLimit"], json!(4096));
    }

    #[test]
    fn imports_upstream_literal_with_legacy_names() {
        let text = r#"{
            "rdocCaptureSettings": 1,
            "settings": {
                "inject": true,
                "autoStart": true,
                "executable": "/opt/app",
                "workingDir": "/opt",
                "commandLine": "-a -b",
                "environment": [
                    {"variable": "A", "value": "1", "type": "Prepend", "separator": "Colon"},
                    {"variable": "B", "value": "2", "type": "Set", "separator": "Platform"},
                    {"variable": "C", "value": "3", "type": "Other", "separator": "Other"}
                ],
                "options": {
                    "allowVSync": false,
                    "captureCallstacksOnlyDraws": true,
                    "delayForDebugger": 12,
                    "verifyMapWrites": true,
                    "softMemoryLimit": 2048
                },
                "queuedFrameCap": 5,
                "numQueuedFrames": 2
            }
        }"#;
        let settings = CaptureSettings::parse(text).unwrap();
        assert_eq!(settings.executable, "/opt/app");
        assert_eq!(settings.working_directory, "/opt");
        assert_eq!(settings.command_line, "-a -b");
        assert_eq!(settings.environment.len(), 3);
        assert_eq!(settings.environment[0].operation, EnvOperation::Prepend);
        assert_eq!(settings.environment[0].separator, EnvSeparator::Colon);
        assert_eq!(settings.environment[1].separator, EnvSeparator::Platform);
        // Unknown enum strings fall back like EnvModFromVariant / EnvModFromVariant's sep chain.
        assert_eq!(settings.environment[2].operation, EnvOperation::Set);
        assert_eq!(settings.environment[2].separator, EnvSeparator::NoSep);
        assert!(!settings.options.allow_vsync);
        assert!(settings.options.capture_callstacks_only_actions);
        assert_eq!(settings.options.delay_for_debugger, 12);
        assert!(settings.options.verify_buffer_access);
        assert_eq!(settings.options.soft_memory_limit, 2048);
        // Missing option keys read as false, including those whose default is true.
        assert!(!settings.options.allow_fullscreen);
        assert!(!settings.options.debug_output_mute);
        assert_eq!(
            settings.unsupported_fields(),
            ["inject", "autoStart", "queuedFrameCap", "numQueuedFrames"]
        );
    }

    #[test]
    fn verify_buffer_access_key_takes_precedence_over_legacy_name() {
        let settings = CaptureSettings::parse(
            r#"{"rdocCaptureSettings":1,"settings":{"options":{"verifyBufferAccess":false,"verifyMapWrites":true}}}"#,
        )
        .unwrap();
        assert!(!settings.options.verify_buffer_access);
    }

    #[test]
    fn rejects_wrong_magic_or_version() {
        assert!(CaptureSettings::parse("{}").is_err());
        assert!(CaptureSettings::parse(r#"{"otherMagic":1}"#).is_err());
        assert!(CaptureSettings::parse(r#"{"rdocCaptureSettings":2}"#).is_err());
        assert!(CaptureSettings::parse("not json").is_err());
        assert!(CaptureSettings::parse(r#"{"rdocCaptureSettings":1}"#).is_ok());
    }

    #[test]
    fn missing_keys_read_as_upstream_defaults() {
        let settings =
            CaptureSettings::parse(r#"{"rdocCaptureSettings":1,"settings":{}}"#).unwrap();
        assert_eq!(settings.executable, "");
        assert_eq!(settings.working_directory, "");
        assert_eq!(settings.command_line, "");
        assert!(settings.environment.is_empty());
        assert_eq!(
            settings.options,
            CaptureOptions {
                allow_vsync: false,
                allow_fullscreen: false,
                api_validation: false,
                capture_callstacks: false,
                capture_callstacks_only_actions: false,
                delay_for_debugger: 0,
                verify_buffer_access: false,
                hook_into_children: false,
                ref_all_resources: false,
                capture_all_cmd_lists: false,
                debug_output_mute: false,
                soft_memory_limit: 0,
            }
        );
    }

    #[test]
    fn round_trip_through_json_and_disk() {
        let settings = CaptureSettings {
            executable: "app.exe".into(),
            command_line: "--flag".into(),
            environment: vec![EnvironmentModification {
                name: "X".into(),
                value: "y".into(),
                operation: EnvOperation::Prepend,
                separator: EnvSeparator::NoSep,
            }],
            options: CaptureOptions {
                allow_vsync: false,
                capture_callstacks: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let text = settings.to_json_string();
        assert_eq!(CaptureSettings::parse(&text).unwrap(), settings);

        let path = std::env::temp_dir().join(format!(
            "renderdocx-cap-roundtrip-{}-{}.cap",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        settings.save(&path).unwrap();
        assert_eq!(CaptureSettings::load(&path).unwrap(), settings);
        std::fs::remove_file(&path).unwrap();
    }
}
