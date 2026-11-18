//! Find/replace used by the shader source viewer.
//!
//! Mirrors upstream `FindReplace`'s match-case, whole-word, direction and
//! regular-expression options. Regex mode is backed by `pcre2` (already a
//! project dependency), so all four upstream search modes are available.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FindDirection {
    #[default]
    Down,
    Up,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FindOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub direction: FindDirection,
    /// Treat the search text as a PCRE2 regular expression.
    pub regex: bool,
}

/// Return non-overlapping byte ranges of every match. In regex mode a PCRE2
/// pattern is compiled and matched against the UTF-8 bytes; an invalid pattern
/// simply yields no matches (matching upstream's fail-safe behaviour).
pub fn find_all(text: &str, needle: &str, options: FindOptions) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }
    if options.regex {
        return find_all_regex(text, needle, options.case_sensitive);
    }
    let haystack = if options.case_sensitive {
        text.to_owned()
    } else {
        text.to_ascii_lowercase()
    };
    let target = if options.case_sensitive {
        needle.to_owned()
    } else {
        needle.to_ascii_lowercase()
    };
    let mut matches = Vec::new();
    let mut offset = 0;
    while let Some(relative) = haystack[offset..].find(&target) {
        let start = offset + relative;
        let end = start + target.len();
        if !options.whole_word || is_word_boundary(text, start, end) {
            matches.push((start, end));
        }
        offset = if end > start { end } else { start + 1 };
        if offset >= haystack.len() {
            break;
        }
    }
    matches
}

fn find_all_regex(text: &str, pattern: &str, case_sensitive: bool) -> Vec<(usize, usize)> {
    let pattern = if case_sensitive {
        pattern.to_owned()
    } else {
        format!("(?i){pattern}")
    };
    let mut builder = pcre2::bytes::RegexBuilder::new();
    builder.utf(true);
    let Ok(regex) = builder.build(&pattern) else {
        return Vec::new();
    };
    regex
        .find_iter(text.as_bytes())
        .filter_map(|matched| matched.ok())
        .map(|matched| (matched.start(), matched.end()))
        .collect()
}

fn is_word_boundary(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
}

fn is_word_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// Find the next match after `cursor` in the requested direction, wrapping
/// around at the end/beginning of the text.
pub fn find_next(
    text: &str,
    needle: &str,
    options: FindOptions,
    cursor: usize,
) -> Option<(usize, usize)> {
    let matches = find_all(text, needle, options);
    if matches.is_empty() {
        return None;
    }
    match options.direction {
        FindDirection::Down => matches
            .iter()
            .copied()
            .find(|(start, _)| *start >= cursor)
            .or_else(|| matches.first().copied()),
        FindDirection::Up => matches
            .iter()
            .rev()
            .copied()
            .find(|(start, _)| *start < cursor)
            .or_else(|| matches.last().copied()),
    }
}

/// Expand `$0`-`$9` capture references in a replacement string. `$0` is the
/// whole match; unknown groups expand to an empty string (upstream behaves
/// similarly for out-of-range captures).
pub fn expand_replacement(replacement: &str, matched: &str, groups: &[&str]) -> String {
    let bytes = replacement.as_bytes();
    let mut result = String::with_capacity(replacement.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            let group = (bytes[i + 1] - b'0') as usize;
            let value = if group == 0 {
                Some(matched)
            } else {
                groups.get(group - 1).copied()
            };
            result.push_str(value.unwrap_or(""));
            i += 2;
        } else {
            let ch = replacement[i..].chars().next().unwrap();
            result.push(ch);
            i += ch.len_utf8();
        }
    }
    result
}

/// Replace every match, returning the new text and number of replacements.
/// In regex mode, `$0`-`$9` in `replacement` expand to captured groups.
pub fn replace_all(
    text: &str,
    needle: &str,
    replacement: &str,
    options: FindOptions,
) -> (String, usize) {
    if options.regex {
        let pattern = if options.case_sensitive {
            needle.to_owned()
        } else {
            format!("(?i){needle}")
        };
        let mut builder = pcre2::bytes::RegexBuilder::new();
        builder.utf(true);
        let Ok(regex) = builder.build(&pattern) else {
            return (text.to_owned(), 0);
        };
        let mut result = String::with_capacity(text.len());
        let mut last = 0;
        let mut count = 0;
        for captured in regex.captures_iter(text.as_bytes()).flatten() {
            let Some(whole) = captured.get(0) else {
                continue;
            };
            let (start, end) = (whole.start(), whole.end());
            if start < last {
                continue;
            }
            result.push_str(&text[last..start]);
            let matched = &text[start..end];
            let groups: Vec<&str> = (1..captured.len())
                .filter_map(|index| captured.get(index))
                .filter_map(|m| text.get(m.start()..m.end()))
                .collect();
            result.push_str(&expand_replacement(replacement, matched, &groups));
            last = end;
            count += 1;
        }
        result.push_str(&text[last..]);
        return (result, count);
    }
    let matches = find_all(text, needle, options);
    if matches.is_empty() {
        return (text.to_owned(), 0);
    }
    let mut result = String::with_capacity(text.len());
    let mut last = 0;
    for (start, end) in &matches {
        result.push_str(&text[last..*start]);
        result.push_str(&replacement.replace("$0", &text[*start..*end]));
        last = *end;
    }
    result.push_str(&text[last..]);
    (result, matches.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "Draw draw DRAW\nDrawn draw_ draw";

    #[test]
    fn find_all_honours_case_and_whole_word() {
        let matches = find_all(TEXT, "draw", FindOptions::default());
        assert_eq!(matches.len(), 6); // includes 'Drawn' and 'draw_'
        let options = FindOptions {
            whole_word: true,
            ..Default::default()
        };
        let matches = find_all(TEXT, "draw", options);
        assert_eq!(matches.len(), 4);
        let options = FindOptions {
            case_sensitive: true,
            whole_word: true,
            direction: FindDirection::Down,
            ..Default::default()
        };
        let matches = find_all(TEXT, "draw", options);
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn find_next_wraps_in_both_directions() {
        let options = FindOptions::default();
        assert_eq!(find_next("a a a", "a", options, 0), Some((0, 1)));
        assert_eq!(find_next("a a a", "a", options, 4), Some((4, 5)));
        assert_eq!(find_next("a a a", "a", options, 5), Some((0, 1)));
        let up = FindOptions {
            direction: FindDirection::Up,
            ..Default::default()
        };
        assert_eq!(find_next("a a a", "a", up, 5), Some((4, 5)));
        assert_eq!(find_next("a a a", "a", up, 0), Some((4, 5)));
        assert_eq!(find_next("abc", "", options, 0), None);
    }

    #[test]
    fn replace_all_replaces_only_matches() {
        let options = FindOptions {
            whole_word: true,
            ..Default::default()
        };
        let (text, count) = replace_all("Draw draw Drawn", "draw", "DIP", options);
        assert_eq!(count, 2);
        assert_eq!(text, "DIP DIP Drawn");
    }

    #[test]
    fn regex_mode_finds_patterns_and_is_case_sensitive_on_request() {
        let options = FindOptions {
            regex: true,
            case_sensitive: true,
            ..Default::default()
        };
        let matches = find_all("foo123 bar456", r"\d+", options);
        assert_eq!(matches.len(), 2);
        assert_eq!(&"foo123 bar456"[matches[0].0..matches[0].1], "123");
        assert_eq!(&"foo123 bar456"[matches[1].0..matches[1].1], "456");

        let insensitive = FindOptions {
            regex: true,
            case_sensitive: false,
            ..Default::default()
        };
        assert_eq!(find_all("Draw draw DRAW", "draw", insensitive).len(), 3);

        let (replaced, count) = replace_all("a1 b22", r"\d+", "#", options);
        assert_eq!(count, 2);
        assert_eq!(replaced, "a# b#");
    }

    #[test]
    fn regex_replacement_expands_capture_groups() {
        let options = FindOptions {
            regex: true,
            case_sensitive: true,
            ..Default::default()
        };
        let (text, count) = replace_all("foo=12 bar=34", r"(\w+)=(\d+)", "$1:$2", options);
        assert_eq!(count, 2);
        assert_eq!(text, "foo:12 bar:34");
        let (text, _) = replace_all("ab", r"(a)(b)", "$0/$2$1", options);
        assert_eq!(text, "ab/ba");
    }

    #[test]
    fn invalid_regex_yields_no_matches() {
        let options = FindOptions {
            regex: true,
            ..Default::default()
        };
        assert!(find_all("abc", "[", options).is_empty());
    }
}
