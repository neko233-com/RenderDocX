//! Analytics consent, replacing qrenderdoc's `AnalyticsConfirmDialog` and
//! `AnalyticsPromptDialog`.
//!
//! RenderDocX never transmits analytics data. Consent gates only a local,
//! opt-in session counter kept in the configuration directory, so the two
//! upstream consent dialogs have a native equivalent without any network
//! collection. `Unset` means the user has not decided yet and the prompt
//! should be shown.

use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Consent {
    /// Not decided yet: show the prompt.
    #[default]
    Unset,
    /// The user allowed local usage counting.
    Granted,
    /// The user declined; nothing is recorded.
    Denied,
}

impl Consent {
    pub fn as_str(self) -> &'static str {
        match self {
            Consent::Unset => "unset",
            Consent::Granted => "granted",
            Consent::Denied => "denied",
        }
    }

    /// Parse a persisted value; unknown strings fall back to `Unset`.
    pub fn parse(text: &str) -> Self {
        match text.trim().to_ascii_lowercase().as_str() {
            "granted" => Consent::Granted,
            "denied" => Consent::Denied,
            _ => Consent::Unset,
        }
    }

    /// Whether the consent prompt should be shown.
    pub fn should_prompt(self) -> bool {
        self == Consent::Unset
    }
}

/// Local counter file under the configuration directory.
pub fn counter_path(config_dir: &Path) -> PathBuf {
    config_dir.join("analytics_sessions.txt")
}

pub fn session_count(path: &Path) -> u64 {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

/// Increment the local session counter, but only when consent is granted.
/// Returns the new count (unchanged when denied/unset).
pub fn record_session(path: &Path, consent: Consent) -> Result<u64, String> {
    let current = session_count(path);
    if consent != Consent::Granted {
        return Ok(current);
    }
    let next = current.saturating_add(1);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(path, next.to_string()).map_err(|error| error.to_string())?;
    Ok(next)
}

/// Build a complete local analytics report (JSON). RenderDocX never uploads
/// this; it exists so users can inspect or share their own usage summary, which
/// is the local equivalent of the upstream analytics payload.
pub fn report_json(config_dir: &Path, consent: Consent) -> String {
    let sessions = session_count(&counter_path(config_dir));
    serde_json::json!({
        "product": "RenderDocX",
        "version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "consent": consent.as_str(),
        "sessions": sessions,
        "note": "Local-only report; RenderDocX does not transmit analytics.",
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consent_parsing_and_prompting() {
        assert_eq!(Consent::parse("granted"), Consent::Granted);
        assert_eq!(Consent::parse(" DENIED "), Consent::Denied);
        assert_eq!(Consent::parse("anything"), Consent::Unset);
        assert!(Consent::Unset.should_prompt());
        assert!(!Consent::Granted.should_prompt());
        assert_eq!(Consent::Granted.as_str(), "granted");
    }

    #[test]
    fn report_contains_version_and_local_counts() {
        let dir = tempfile::tempdir().unwrap();
        let path = counter_path(dir.path());
        record_session(&path, Consent::Granted).unwrap();
        let report: serde_json::Value =
            serde_json::from_str(&report_json(dir.path(), Consent::Granted)).unwrap();
        assert_eq!(report["product"], "RenderDocX");
        assert_eq!(report["consent"], "granted");
        assert_eq!(report["sessions"], 1);
        assert!(report["version"].as_str().is_some_and(|v| !v.is_empty()));
    }

    #[test]
    fn session_counter_only_increments_when_granted() {
        let dir = tempfile::tempdir().unwrap();
        let path = counter_path(dir.path());
        assert_eq!(record_session(&path, Consent::Denied).unwrap(), 0);
        assert!(!path.exists());
        assert_eq!(record_session(&path, Consent::Unset).unwrap(), 0);
        assert_eq!(record_session(&path, Consent::Granted).unwrap(), 1);
        assert_eq!(record_session(&path, Consent::Granted).unwrap(), 2);
        assert_eq!(session_count(&path), 2);
        // Revoking consent stops recording but keeps the historical total.
        assert_eq!(record_session(&path, Consent::Denied).unwrap(), 2);
    }
}
