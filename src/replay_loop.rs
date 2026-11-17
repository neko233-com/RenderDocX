//! State machine for looping replay of capture events.
//!
//! Upstream `action_Start_Replay_Loop` drives `ReplayLoop` on a native window
//! owned by the replay backend. RenderDocX has no bridge-side window handle,
//! so its loop advances the selected event on a timer and previews it through
//! the normal replay path. This is an intentional, documented difference.

/// A replay loop over a contiguous range of events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayLoop {
    pub events: Vec<u32>,
    pub position: usize,
    pub interval_ms: u64,
    pub playing: bool,
}

impl ReplayLoop {
    pub fn new(events: Vec<u32>, interval_ms: u64) -> Self {
        Self {
            events,
            position: 0,
            interval_ms: interval_ms.clamp(16, 10_000),
            playing: false,
        }
    }

    pub fn current(&self) -> Option<u32> {
        self.events.get(self.position).copied()
    }

    pub fn toggle(&mut self) -> bool {
        self.playing = !self.playing;
        self.playing
    }

    pub fn stop(&mut self) {
        self.playing = false;
    }

    /// Advance one step, wrapping around at the end. Returns the new event.
    pub fn advance(&mut self) -> Option<u32> {
        if self.events.is_empty() {
            self.playing = false;
            return None;
        }
        self.position = (self.position + 1) % self.events.len();
        self.current()
    }

    /// Move to an arbitrary event; returns false if it is not in the loop.
    pub fn seek(&mut self, event: u32) -> bool {
        if let Some(position) = self.events.iter().position(|value| *value == event) {
            self.position = position;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_advances_wraps_and_clamps_interval() {
        let mut loop_state = ReplayLoop::new(vec![10, 20, 30], 0);
        assert_eq!(loop_state.interval_ms, 16);
        assert_eq!(loop_state.current(), Some(10));
        assert!(loop_state.toggle());
        assert_eq!(loop_state.advance(), Some(20));
        assert_eq!(loop_state.advance(), Some(30));
        assert_eq!(loop_state.advance(), Some(10));
        assert!(loop_state.seek(30));
        assert_eq!(loop_state.current(), Some(30));
        assert!(!loop_state.seek(99));
        loop_state.stop();
        assert!(!loop_state.playing);
    }

    #[test]
    fn empty_loop_cannot_play() {
        let mut loop_state = ReplayLoop::new(Vec::new(), 100);
        assert_eq!(loop_state.current(), None);
        assert_eq!(loop_state.advance(), None);
        assert!(!loop_state.playing);
    }
}
