use crate::replay::Action;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ActionRange {
    pub first_event: u32,
    pub last_event: u32,
    pub first_action: u32,
    pub last_action: Option<u32>,
}

/// Reconstruct the event browser's effective EID for nested action groups.
pub fn calculate(actions: &[Action]) -> Vec<Option<ActionRange>> {
    let mut first = vec![None; actions.len()];
    let mut last = vec![None; actions.len()];
    let mut eid_action = HashMap::new();
    for (index, action) in actions.iter().enumerate() {
        eid_action.insert(action.event_id, index);
        for event in &action.api_events {
            eid_action.insert(event.event_id, index);
        }
        if let Ok(parent) = usize::try_from(action.parent) {
            if parent < index {
                if first[parent].is_none() {
                    first[parent] = Some(index);
                }
                last[parent] = Some(index);
            }
        }
    }
    let mut effective = vec![0u32; actions.len()];
    let mut result = vec![None; actions.len()];
    for index in (0..actions.len()).rev() {
        let Some(last_child) = last[index] else {
            effective[index] = actions[index].event_id;
            continue;
        };
        let end_action = &actions[last_child];
        let mut end = end_action.event_id;
        if end_action.flags & 0x80 != 0 {
            end = end.saturating_sub(1);
        }
        if end == end_action.event_id && last[last_child].is_some() {
            end = effective[last_child];
        }
        effective[index] = end;
        let action = &actions[index];
        let fake = action.api_events.len() == 1 && action.api_events[0].chunk_index.is_none();
        let first_child = first[index].expect("a node with a last child has a first child");
        let start_event = if fake {
            actions[first_child]
                .api_events
                .first()
                .map_or(actions[first_child].event_id, |event| event.event_id)
        } else {
            action.event_id
        };
        let start_action = if fake {
            actions[first_child].action_id
        } else {
            action.action_id
        };
        result[index] = Some(ActionRange {
            first_event: start_event,
            last_event: end,
            first_action: start_action,
            last_action: eid_action.get(&end).map(|&i| actions[i].action_id),
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::ApiEvent;
    fn action(event_id: u32, action_id: u32, parent: i64, flags: u32) -> Action {
        Action {
            event_id,
            action_id,
            depth: 0,
            parent,
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
            name: String::new(),
            api_events: Vec::new(),
        }
    }
    #[test]
    fn nested_groups_pop_markers_and_fake_markers_match_upstream_rules() {
        let mut outer = action(100, 8, -1, 0x40);
        outer.api_events.push(ApiEvent {
            event_id: 100,
            chunk_index: None,
            file_offset: "0".into(),
            name: String::new(),
        });
        let mut group = action(2, 10, 0, 0x40);
        group.api_events.push(ApiEvent {
            event_id: 1,
            chunk_index: Some(0),
            file_offset: "0".into(),
            name: String::new(),
        });
        let draw = action(4, 11, 1, 2);
        let pop = action(5, 12, 1, 0x80);
        let leaf = action(6, 12, 0, 2);
        let ranges = calculate(&[outer, group, draw, pop, leaf]);
        assert_eq!(
            ranges[1],
            Some(ActionRange {
                first_event: 2,
                last_event: 4,
                first_action: 10,
                last_action: Some(11)
            })
        );
        assert_eq!(
            ranges[0],
            Some(ActionRange {
                first_event: 1,
                last_event: 6,
                first_action: 10,
                last_action: Some(12)
            })
        );
        assert_eq!(ranges[2], None);
    }
}
