//! Update checking, replacing qrenderdoc's `UpdateDialog`.
//!
//! Queries the GitHub releases API for the latest tag and compares it with the
//! running version. The request is explicit and user-triggered; RenderDocX does
//! not check for updates automatically. Version parsing/comparison is pure and
//! unit-tested; the network call is isolated behind `check_latest`.

use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    pub fn to_string(self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Parse `1.2.3`, `v1.2.3`, `1.2`, or `1.2.3-beta` (suffix ignored).
pub fn parse_version(text: &str) -> Option<Version> {
    let text = text.trim().trim_start_matches(['v', 'V']);
    let core = text.split(['-', '+']).next()?.trim();
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(Version {
        major,
        minor,
        patch,
    })
}

/// Whether `latest` is a strictly newer release than `current`.
pub fn is_newer(latest: Version, current: Version) -> bool {
    latest > current
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    html_url: String,
}

/// Query the latest GitHub release for `owner/repo`. Returns the latest version
/// and its URL. Network/API failures are returned so the GUI can report them.
pub fn check_latest(repo: &str) -> Result<(Version, String), String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let mut response = ureq::get(&url)
        .header("User-Agent", "RenderDocX-update-check")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|error| format!("Update check request failed: {error}"))?;
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|error| format!("Could not read update response: {error}"))?;
    let release: Release =
        serde_json::from_str(&body).map_err(|error| format!("Invalid update response: {error}"))?;
    let version = parse_version(&release.tag_name)
        .ok_or_else(|| format!("Unrecognized release tag: {}", release.tag_name))?;
    Ok((version, release.html_url))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_tag_shapes() {
        assert_eq!(
            parse_version("1.2.3"),
            Some(Version { major: 1, minor: 2, patch: 3 })
        );
        assert_eq!(
            parse_version("v1.48.0"),
            Some(Version { major: 1, minor: 48, patch: 0 })
        );
        assert_eq!(
            parse_version("0.1"),
            Some(Version { major: 0, minor: 1, patch: 0 })
        );
        assert_eq!(
            parse_version(" 2.0.0-beta.1 "),
            Some(Version { major: 2, minor: 0, patch: 0 })
        );
        assert!(parse_version("not-a-version").is_none());
        assert!(parse_version("1.2.3.4").is_none());
        assert!(parse_version("").is_none());
    }

    #[test]
    fn compares_versions_semantically() {
        let v = |s| parse_version(s).unwrap();
        assert!(is_newer(v("0.2.0"), v("0.1.0")));
        assert!(is_newer(v("1.0.0"), v("0.99.99")));
        assert!(!is_newer(v("0.1.0"), v("0.1.0")));
        assert!(!is_newer(v("0.0.9"), v("0.1.0")));
    }
}
