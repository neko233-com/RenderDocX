//! Parsing of RenderDoc's core log file for the Log View window.
//!
//! Format authority: `qrenderdoc/Windows/LogView.cpp::messages_refresh` uses
//! `RENDERDOC_GetLogFileContents` and matches each line against:
//! `^(AAAA) PID: [HH:MM:SS] file(123) - Type - message`
//! where the four-letter source is ADRD (Android), QTRD (UI) or RDOC (Core)
//! and Type is one of `LogType`'s Debug/Comment/Warning/Error/Fatal.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Comment,
    Warning,
    Error,
    Fatal,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "Debug",
            Self::Comment => "Comment",
            Self::Warning => "Warning",
            Self::Error => "Error",
            Self::Fatal => "Fatal",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogMessage {
    pub source: String,
    pub pid: u32,
    pub timestamp: String,
    pub location: String,
    pub level: LogLevel,
    pub message: String,
}

/// Parse all well-formed lines from a log snapshot. Malformed lines are
/// ignored, matching upstream's regex-based `match.hasMatch()` guard.
pub fn parse_log(text: &str) -> Vec<LogMessage> {
    text.split(['\r', '\n']).filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<LogMessage> {
    let bytes = line.as_bytes();
    if bytes.len() < 40 || !bytes[..4].iter().all(|byte| byte.is_ascii_uppercase()) {
        return None;
    }
    let source = &line[..4];
    let rest = line[4..].strip_prefix(' ')?;
    let (pid_text, rest) = rest.split_once(": ")?;
    if pid_text.is_empty() || !pid_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let pid: u32 = pid_text.parse().ok()?;
    let rest = rest.strip_prefix('[')?;
    let (time, rest) = rest.split_once("] ")?;
    let mut time_parts = time.split(':');
    let (hours, minutes, seconds) = (time_parts.next()?, time_parts.next()?, time_parts.next()?);
    if time_parts.next().is_some()
        || [hours, minutes, seconds]
            .iter()
            .any(|part| part.len() != 2 || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    let open = rest.find('(')?;
    let file = rest[..open].trim();
    let tail = &rest[open + 1..];
    let close = tail.find(')')?;
    let line_number = tail[..close].trim();
    if line_number.is_empty() || !line_number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let tail = tail[close + 1..].trim_start().strip_prefix("- ")?;
    let (level_text, tail) = tail.split_once(" - ")?;
    let level = match level_text.trim() {
        "Debug" => LogLevel::Debug,
        "Comment" => LogLevel::Comment,
        "Warning" => LogLevel::Warning,
        "Error" => LogLevel::Error,
        "Fatal" => LogLevel::Fatal,
        _ => return None,
    };
    if file.is_empty() {
        return None;
    }
    Some(LogMessage {
        source: source.to_owned(),
        pid,
        timestamp: format!("{hours}:{minutes}:{seconds}"),
        location: format!("{file}({line_number})"),
        level,
        message: tail.trim().to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_upstream_log_lines_and_ignores_malformed_ones() {
        let text = concat!(
            "RDOC 1234: [12:34:56] capture.cpp(42) - Comment - Opening capture\r\n",
            "QTRD 1234: [12:34:57] qrenderdoc.cpp(7) - Warning - UI warning\n",
            "ADRD 9: [00:01:02] android.cpp(3) - Error - device lost\n",
            "not a log line\n",
            "RDOC 1: [bad] broken.cpp(1) - Comment - ignored\n",
        );
        let messages = parse_log(text);
        assert_eq!(messages.len(), 3);
        assert_eq!(
            messages[0],
            LogMessage {
                source: "RDOC".into(),
                pid: 1234,
                timestamp: "12:34:56".into(),
                location: "capture.cpp(42)".into(),
                level: LogLevel::Comment,
                message: "Opening capture".into(),
            }
        );
        assert_eq!(messages[1].level, LogLevel::Warning);
        assert_eq!(messages[2].source, "ADRD");
        assert_eq!(messages[2].level, LogLevel::Error);
    }

    #[test]
    fn empty_snapshot_yields_no_messages() {
        assert!(parse_log("").is_empty());
        assert!(parse_log("garbage").is_empty());
    }
}
