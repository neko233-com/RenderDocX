//! Recently viewed texture thumbnails and resource preview cards.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ThumbnailEntry {
    pub texture: usize,
    pub mip: u32,
    pub slice: u32,
    pub sample: u32,
}

/// Cap for the thumbnail strip, matching a compact one-row dock widget.
pub const THUMBNAIL_LIMIT: usize = 8;

/// Move an entry to the front, drop duplicates and cap the history length.
pub fn record(history: &mut Vec<ThumbnailEntry>, entry: ThumbnailEntry) {
    history.retain(|existing| *existing != entry);
    history.insert(0, entry);
    history.truncate(THUMBNAIL_LIMIT);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_history_deduplicates_and_caps() {
        let mut history = Vec::new();
        for index in 0..THUMBNAIL_LIMIT + 2 {
            record(
                &mut history,
                ThumbnailEntry {
                    texture: index,
                    mip: 0,
                    slice: 0,
                    sample: 0,
                },
            );
        }
        assert_eq!(history.len(), THUMBNAIL_LIMIT);
        assert_eq!(history[0].texture, THUMBNAIL_LIMIT + 1);
        record(
            &mut history,
            ThumbnailEntry {
                texture: 3,
                mip: 1,
                slice: 0,
                sample: 0,
            },
        );
        assert_eq!(history[0].texture, 3);
        assert_eq!(history.iter().filter(|entry| entry.texture == 3).count(), 2);
    }
}
