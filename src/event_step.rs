//! RenderDoc's previous/next action chain, reconstructed from its flattened snapshot.
use crate::replay::Action;

#[derive(Default)]
pub struct ActionLinks {
    first_child: Vec<Option<usize>>,
    previous: Vec<Option<usize>>,
    next: Vec<Option<usize>>,
}

impl ActionLinks {
    pub fn from_actions(actions: &[Action]) -> Self {
        let mut links = Self {
            first_child: vec![None; actions.len()],
            previous: vec![None; actions.len()],
            next: vec![None; actions.len()],
        };
        for (index, action) in actions.iter().enumerate() {
            if let Ok(parent) = usize::try_from(action.parent) {
                if parent < index && links.first_child[parent].is_none() {
                    links.first_child[parent] = Some(index);
                }
            }
        }
        let excluded = |index: usize| actions[index].flags & (0x40 | 0x80 | 0x20 | 0x200) != 0;
        let mut previous = None;
        for index in 0..actions.len() {
            if links.first_child[index].is_none() && !excluded(index) {
                if let Some(prev) = previous {
                    links.next[prev] = Some(index);
                }
                links.previous[index] = previous;
                previous = Some(index);
            }
        }
        let mut order: Vec<usize> = (0..actions.len()).collect();
        order.sort_by_key(|&index| actions[index].event_id);
        previous = None;
        let mut markers = Vec::new();
        for index in order {
            if excluded(index) {
                links.previous[index] = previous;
                markers.push(index);
            } else {
                previous = Some(index);
                for marker in markers.drain(..) {
                    links.next[marker] = Some(index);
                }
            }
        }
        links
    }

    /// Returns an action admitted by the filter, or none at the chain boundary.
    pub fn step(&self, current: usize, forward: bool, eligible: &[bool]) -> Option<usize> {
        let child = *self.first_child.get(current)?;
        let mut next = if forward {
            child.or(*self.next.get(current)?)
        } else {
            self.previous
                .get(child.unwrap_or(current))
                .copied()
                .flatten()
        };
        let mut visited = 0;
        while let Some(index) = next {
            if visited >= self.next.len() {
                return None;
            }
            if eligible.get(index).copied().unwrap_or(false) {
                return Some(index);
            }
            next = if forward {
                self.next.get(index)?
            } else {
                self.previous.get(index)?
            }
            .to_owned();
            visited += 1;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn action(event_id: u32, parent: i64, flags: u32) -> Action {
        Action {
            event_id,
            action_id: event_id,
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
    fn marker_groups_and_filtered_actions_follow_upstream_links() {
        let actions = [
            action(1, -1, 2),
            action(2, -1, 0x40),
            action(3, 1, 2),
            action(4, 1, 0x20),
            action(5, 1, 2),
            action(6, -1, 2),
        ];
        let links = ActionLinks::from_actions(&actions);
        let mut visible = vec![true; actions.len()];
        assert_eq!(links.step(1, true, &visible), Some(2));
        assert_eq!(links.step(1, false, &visible), Some(0));
        assert_eq!(links.step(3, true, &visible), Some(4));
        assert_eq!(links.step(3, false, &visible), Some(2));
        visible[4] = false;
        assert_eq!(links.step(2, true, &visible), Some(5));
        assert_eq!(links.step(0, false, &visible), None);
        assert_eq!(links.step(5, true, &visible), None);
    }
}
