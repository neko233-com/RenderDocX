//! Event-ID timeline matching RenderDoc's marker and leaf-action semantics.
use crate::replay::Action;
use serde::Serialize;

const SET_MARKER: u32 = 0x20;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct TimelineItem {
    pub action_index: usize,
    pub event_id: u32,
    pub start: u32,
    pub end: u32,
    pub depth: usize,
    pub parent: i64,
    pub marker: bool,
    pub name: String,
    pub color_bits: Option<[u32; 4]>,
}

#[derive(Clone, Debug, Default)]
pub struct Timeline {
    pub items: Vec<TimelineItem>,
    pub actions: Vec<(u32, usize)>,
    pub max_event: u32,
    pub action_count: usize,
}

impl Timeline {
    pub fn from_actions(actions: &[Action]) -> Self {
        let mut has_child = vec![false; actions.len()];
        let mut first_child = vec![None; actions.len()];
        let mut end: Vec<u32> = actions.iter().map(|a| a.event_id).collect();
        for (index, action) in actions.iter().enumerate() {
            if let Ok(parent) = usize::try_from(action.parent) {
                if parent < index {
                    has_child[parent] = true;
                    if first_child[parent].is_none() {
                        first_child[parent] = Some(index);
                    }
                }
            }
        }
        for index in (0..actions.len()).rev() {
            if let Ok(parent) = usize::try_from(actions[index].parent) {
                if parent < index {
                    end[parent] = end[parent].max(end[index]);
                }
            }
        }
        let mut result = Self {
            action_count: actions.len(),
            ..Default::default()
        };
        for (index, action) in actions.iter().enumerate() {
            let marker = has_child[index];
            if !marker && action.flags & SET_MARKER != 0 {
                continue;
            }
            let fake = marker
                && action.api_events.len() == 1
                && action.api_events[0].chunk_index.is_none();
            let start = if fake {
                first_child[index]
                    .map(|child| {
                        actions[child]
                            .api_events
                            .first()
                            .map_or(actions[child].event_id, |e| e.event_id)
                    })
                    .unwrap_or(action.event_id)
            } else {
                action.event_id
            };
            result.items.push(TimelineItem {
                action_index: index,
                event_id: action.event_id,
                start,
                end: end[index].max(start),
                depth: action.depth,
                parent: action.parent,
                marker,
                name: if marker {
                    action
                        .marker_name
                        .clone()
                        .unwrap_or_else(|| action.name.clone())
                } else {
                    action.name.clone()
                },
                color_bits: action.marker_color_bits,
            });
            if !marker {
                result.actions.push((action.event_id, index));
                result.max_event = result.max_event.max(action.event_id);
            }
        }
        result.actions.sort_unstable();
        result
    }
    pub fn next_action(&self, event: u32) -> Option<(u32, usize)> {
        let index = self.actions.partition_point(|(id, _)| *id < event);
        self.actions
            .get(index)
            .or_else(|| self.actions.last())
            .copied()
    }
    pub fn page(&self, start: u32, end: u32, offset: usize, limit: usize) -> Vec<TimelineItem> {
        self.items
            .iter()
            .filter(|item| item.end >= start && item.start <= end)
            .skip(offset)
            .take(limit)
            .cloned()
            .collect()
    }
    pub fn count(&self, start: u32, end: u32) -> usize {
        self.items
            .iter()
            .filter(|item| item.end >= start && item.start <= end)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::ApiEvent;
    fn action(id: u32, parent: i64, depth: usize, flags: u32) -> Action {
        Action {
            event_id: id,
            action_id: id,
            parent,
            depth,
            flags,
            marker_color_bits: None,
            marker_name: None,
            indices: 0,
            instances: 0,
            base_vertex: None,
            index_offset: None,
            vertex_offset: None,
            instance_offset: None,
            dispatch_groups: None,
            dispatch_threads: None,
            name: format!("event {id}"),
            api_events: vec![],
        }
    }
    #[test]
    fn markers_and_leaf_actions_follow_upstream_rules() {
        let mut marker = action(8, -1, 0, 0x40);
        marker.api_events = vec![ApiEvent {
            event_id: 8,
            chunk_index: None,
            file_offset: "0".into(),
            name: "".into(),
        }];
        let mut child = action(11, 0, 1, 2);
        child.api_events = vec![ApiEvent {
            event_id: 9,
            chunk_index: Some(1),
            file_offset: "0".into(),
            name: "".into(),
        }];
        let model =
            Timeline::from_actions(&[marker, child, action(12, 0, 1, 0x20), action(20, -1, 0, 2)]);
        assert_eq!((model.items[0].start, model.items[0].end), (9, 12));
        assert_eq!(model.actions, [(11, 1), (20, 3)]);
        assert_eq!(model.next_action(12), Some((20, 3)));
        assert_eq!(model.next_action(u32::MAX), Some((20, 3)));
        assert_eq!(model.count(10, 10), 1);
        assert_eq!(model.page(10, 20, 1, 1)[0].event_id, 11);
    }
}
