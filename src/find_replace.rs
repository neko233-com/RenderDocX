//! Literal find/replace used by the shader source viewer.
//!
//! Upstream `FindReplace` wraps Scintilla's find API with match-case,
//! whole-word and direction options. The regular-expression mode is not part
//! of this module; regex users can still search with an exact literal.

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
}

/// Return non-overlapping byte ranges of every match.
pub fn find_all(text: &str, needle: &str, options: FindOptions) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
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

/// Replace every match, returning the new text and number of replacements.
pub fn replace_all(
    text: &str,
    needle: &str,
    replacement: &str,
    options: FindOptions,
) -> (String, usize) {
    let matches = find_all(text, needle, options);
    if matches.is_empty() {
        return (text.to_owned(), 0);
    }
    let mut result = String::with_capacity(text.len());
    let mut last = 0;
    for (start, end) in &matches {
        result.push_str(&text[last..*start]);
        result.push_str(replacement);
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
}
