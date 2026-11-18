//! Remote-host bookmarks, parsed from the same schema qrenderdoc persists in
//! its config (`RemoteHost::operator QVariant` in `RemoteHost.cpp`):
//! `hostname`, `friendlyName`, `runCommand`, `lastCapturePath`.
//!
//! RenderDocX stores these as part of its own settings JSON, parsed natively.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RemoteHost {
    pub hostname: String,
    pub friendly_name: String,
    pub run_command: String,
    pub last_capture_path: String,
}

impl Default for RemoteHost {
    fn default() -> Self {
        Self {
            hostname: String::new(),
            friendly_name: String::new(),
            run_command: String::new(),
            last_capture_path: String::new(),
        }
    }
}

impl RemoteHost {
    pub fn new(hostname: impl Into<String>) -> Self {
        Self {
            hostname: hostname.into(),
            ..Default::default()
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.hostname.trim().is_empty() {
            return Err("Remote host needs a hostname");
        }
        if self.hostname.contains('\0') || self.run_command.contains('\0') {
            return Err("Remote host fields cannot contain NUL");
        }
        Ok(())
    }

    pub fn display_name(&self) -> &str {
        if self.friendly_name.is_empty() {
            &self.hostname
        } else {
            &self.friendly_name
        }
    }
}

/// Insert a host, replacing an existing entry with the same hostname and
/// keeping bookmark order. Returns false if validation fails.
pub fn upsert(hosts: &mut Vec<RemoteHost>, host: RemoteHost) -> bool {
    if host.validate().is_err() {
        return false;
    }
    if let Some(existing) = hosts.iter_mut().find(|item| item.hostname == host.hostname) {
        *existing = host;
    } else {
        hosts.push(host);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_upstream_remote_host_schema() {
        let json = r#"{"hostname":"127.0.0.1:39070","friendlyName":"Local VM","runCommand":"launch.cmd","lastCapturePath":"C:/cap.rdc"}"#;
        let host: RemoteHost = serde_json::from_str(json).unwrap();
        assert_eq!(host.hostname, "127.0.0.1:39070");
        assert_eq!(host.friendly_name, "Local VM");
        assert_eq!(host.display_name(), "Local VM");
        assert_eq!(host.last_capture_path, "C:/cap.rdc");
    }

    #[test]
    fn missing_fields_default_and_validation() {
        let host: RemoteHost = serde_json::from_str(r#"{"hostname":"host"}"#).unwrap();
        assert!(host.validate().is_ok());
        assert_eq!(host.display_name(), "host");
        assert!(RemoteHost::default().validate().is_err());
        assert!(RemoteHost::new("bad\0host").validate().is_err());
    }

    #[test]
    fn upsert_deduplicates_by_hostname() {
        let mut hosts = Vec::new();
        assert!(upsert(&mut hosts, RemoteHost::new("a")));
        assert!(upsert(&mut hosts, RemoteHost::new("b")));
        let mut updated = RemoteHost::new("a");
        updated.friendly_name = "A renamed".into();
        assert!(upsert(&mut hosts, updated));
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].friendly_name, "A renamed");
        assert!(!upsert(&mut hosts, RemoteHost::new("")));
    }
}
