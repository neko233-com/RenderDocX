//! Camera movement key bindings for the mesh viewer, mirroring upstream
//! `KeyPressDirection` (6 directions × primary/secondary = 12 slots).
//!
//! Upstream stores Qt key codes as u32. RenderDocX stores normalized key names
//! from GPUI keystrokes instead (pure Rust, no Qt dependency), with the same
//! 12-slot layout and index math.

use serde::{Deserialize, Serialize};

/// 6 movement directions. `SLOT_COUNT = 6 * 2` (primary + secondary binding).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Forward,
    Back,
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub const ALL: [Direction; 6] = [
        Direction::Forward,
        Direction::Back,
        Direction::Left,
        Direction::Right,
        Direction::Up,
        Direction::Down,
    ];
    pub const SLOT_COUNT: usize = 12;

    pub fn label(self) -> &'static str {
        match self {
            Direction::Forward => "Forward",
            Direction::Back => "Back",
            Direction::Left => "Left",
            Direction::Right => "Right",
            Direction::Up => "Up",
            Direction::Down => "Down",
        }
    }
}

/// Upstream index math: `dir * 2 + (primary ? 0 : 1)`.
pub const fn slot_index(direction: Direction, primary: bool) -> usize {
    direction as usize * 2 + if primary { 0 } else { 1 }
}

/// Normalize a GPUI keystroke key into a stable stored binding string.
/// Empty/unbind keys return `None` (escape unbinds, matching upstream).
pub fn normalize_binding(key: &str) -> Option<String> {
    let trimmed = key.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("escape") {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

/// A 12-slot binding table with upstream defaults (WASD/QE + arrows).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct KeyBindings {
    pub slots: Vec<String>,
}

impl Default for KeyBindings {
    fn default() -> Self {
        let defaults = [
            "w", "s", "a", "d", "e", "q", "up", "down", "left", "right", "pageup", "pagedown",
        ];
        Self {
            slots: defaults.iter().map(|key| (*key).to_owned()).collect(),
        }
    }
}

impl KeyBindings {
    pub fn get(&self, direction: Direction, primary: bool) -> Option<&str> {
        self.slots
            .get(slot_index(direction, primary))
            .map(String::as_str)
    }

    /// Bind a key to a slot; `None` unbinds. Returns false if slot is invalid.
    pub fn bind(&mut self, direction: Direction, primary: bool, key: Option<String>) -> bool {
        let index = slot_index(direction, primary);
        if index >= self.slots.len() {
            return false;
        }
        self.slots[index] = key.unwrap_or_default();
        true
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_index_matches_upstream_math() {
        assert_eq!(slot_index(Direction::Forward, true), 0);
        assert_eq!(slot_index(Direction::Forward, false), 1);
        assert_eq!(slot_index(Direction::Down, false), 11);
    }

    #[test]
    fn default_bindings_cover_all_twelve_slots() {
        let bindings = KeyBindings::default();
        assert_eq!(bindings.slots.len(), Direction::SLOT_COUNT);
        assert_eq!(bindings.get(Direction::Forward, true), Some("w"));
        assert_eq!(bindings.get(Direction::Down, false), Some("pagedown"));
    }

    #[test]
    fn binding_and_unbinding() {
        let mut bindings = KeyBindings::default();
        assert!(bindings.bind(Direction::Forward, true, normalize_binding("T")));
        assert_eq!(bindings.get(Direction::Forward, true), Some("t"));
        assert!(bindings.bind(Direction::Forward, true, normalize_binding("Escape")));
        assert_eq!(bindings.get(Direction::Forward, true), Some(""));
        bindings.reset();
        assert_eq!(bindings.get(Direction::Forward, true), Some("w"));
    }

    #[test]
    fn normalize_handles_escape_and_empty() {
        assert_eq!(normalize_binding("  W  "), Some("w".to_owned()));
        assert_eq!(normalize_binding("escape"), None);
        assert_eq!(normalize_binding(""), None);
    }
}
