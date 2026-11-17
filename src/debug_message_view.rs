//! Sorting and visibility rules for RenderDoc debug messages.
use crate::replay::{DebugMessage, debug_message_category_name, debug_message_source_name};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortKey {
    #[default]
    Event,
    Source,
    Severity,
    Category,
    Id,
    Description,
}

pub fn is_hidden(
    message: &DebugMessage,
    sources: &BTreeSet<u32>,
    severities: &BTreeSet<u32>,
    categories: &BTreeSet<u32>,
    types: &BTreeSet<(u32, u32, u32)>,
) -> bool {
    sources.contains(&message.source)
        || severities.contains(&message.severity)
        || categories.contains(&message.category)
        || types.contains(&(message.source, message.category, message.message_id))
}

/// Stable sorting preserves capture order when the selected column is equal.
pub fn sort_indices(
    indices: &mut [usize],
    messages: &[DebugMessage],
    key: SortKey,
    descending: bool,
) {
    indices.sort_by(|&a, &b| {
        let a = &messages[a];
        let b = &messages[b];
        let order = match key {
            SortKey::Event => a.event_id.cmp(&b.event_id),
            SortKey::Source => {
                debug_message_source_name(a.source).cmp(debug_message_source_name(b.source))
            }
            SortKey::Severity => a.severity.cmp(&b.severity),
            SortKey::Category => {
                debug_message_category_name(a.category).cmp(debug_message_category_name(b.category))
            }
            SortKey::Id => a.message_id.cmp(&b.message_id),
            SortKey::Description => a.description.cmp(&b.description),
        };
        if descending { order.reverse() } else { order }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(
        event_id: u32,
        source: u32,
        severity: u32,
        category: u32,
        message_id: u32,
        description: &str,
    ) -> DebugMessage {
        DebugMessage {
            event_id,
            source,
            severity,
            category,
            message_id,
            description: description.into(),
        }
    }

    #[test]
    fn sort_all_columns_numerically_or_by_their_text_and_stably() {
        let messages = vec![
            message(9, 2, 1, 4, 100, "z"),
            message(2, 0, 0, 2, 9, "a"),
            message(2, 1, 0, 1, 9, "b"),
        ];
        for (key, expected) in [
            (SortKey::Event, vec![1, 2, 0]),
            (SortKey::Source, vec![1, 0, 2]),
            (SortKey::Severity, vec![1, 2, 0]),
            (SortKey::Category, vec![0, 1, 2]),
            (SortKey::Id, vec![1, 2, 0]),
            (SortKey::Description, vec![1, 2, 0]),
        ] {
            let mut indices = vec![0, 1, 2];
            sort_indices(&mut indices, &messages, key, false);
            assert_eq!(indices, expected, "{key:?}");
        }
        let mut indices = vec![0, 1, 2];
        sort_indices(&mut indices, &messages, SortKey::Event, true);
        assert_eq!(indices, vec![0, 1, 2]);
    }

    #[test]
    fn hidden_type_uses_source_category_and_id_not_severity() {
        let msg = message(1, 2, 3, 4, 5, "example");
        let empty = BTreeSet::new();
        assert!(!is_hidden(
            &msg,
            &empty,
            &empty,
            &empty,
            &BTreeSet::from([(2, 4, 6)])
        ));
        assert!(is_hidden(
            &msg,
            &empty,
            &empty,
            &empty,
            &BTreeSet::from([(2, 4, 5)])
        ));
        assert!(is_hidden(
            &msg,
            &empty,
            &BTreeSet::from([3]),
            &empty,
            &BTreeSet::new()
        ));
    }
}
