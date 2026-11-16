use renderdocx::replay::Action;

/// A view of the upstream action hierarchy. Indices always refer to the snapshot.
#[derive(Default)]
pub struct EventTree {
    parents: Vec<Option<usize>>,
    children: Vec<bool>,
    collapsed: Vec<bool>,
    pub visible: Vec<usize>,
}

impl EventTree {
    pub fn reset(&mut self, actions: &[Action]) {
        self.parents.clear();
        self.children = vec![false; actions.len()];
        self.collapsed = vec![false; actions.len()];
        for (index, action) in actions.iter().enumerate() {
            let parent = usize::try_from(action.parent).ok().filter(|&p| p < index);
            if let Some(parent) = parent {
                self.children[parent] = true;
            }
            self.parents.push(parent);
        }
        self.rebuild();
    }

    pub fn len(&self) -> usize {
        self.parents.len()
    }

    pub fn has_children(&self, index: usize) -> bool {
        self.children.get(index).copied().unwrap_or(false)
    }

    pub fn is_collapsed(&self, index: usize) -> bool {
        self.collapsed.get(index).copied().unwrap_or(false)
    }

    pub fn toggle(&mut self, index: usize) {
        if self.has_children(index) {
            self.collapsed[index] = !self.collapsed[index];
            self.rebuild();
        }
    }

    fn rebuild(&mut self) {
        self.visible.clear();
        let mut shown = vec![false; self.parents.len()];
        for (index, parent) in self.parents.iter().enumerate() {
            shown[index] = parent.is_none_or(|parent| shown[parent] && !self.collapsed[parent]);
            if shown[index] {
                self.visible.push(index);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(parent: i64) -> Action {
        Action {
            event_id: 0,
            action_id: 0,
            depth: 0,
            parent,
            flags: 0,
            indices: 0,
            instances: 0,
            name: String::new(),
        }
    }

    #[test]
    fn nested_collapses_preserve_indices_and_independent_state() {
        let mut tree = EventTree::default();
        tree.reset(&[action(-1), action(0), action(1), action(0), action(-1)]);
        assert_eq!(tree.visible, [0, 1, 2, 3, 4]);
        tree.toggle(1);
        assert_eq!(tree.visible, [0, 1, 3, 4]);
        tree.toggle(0);
        assert_eq!(tree.visible, [0, 4]);
        tree.toggle(0);
        assert_eq!(tree.visible, [0, 1, 3, 4]);
        tree.toggle(1);
        assert_eq!(tree.visible, [0, 1, 2, 3, 4]);
        tree.toggle(2);
        assert_eq!(tree.visible, [0, 1, 2, 3, 4]);
        tree.reset(&[action(-1)]);
        assert_eq!(tree.visible, [0]);
        assert!(!tree.has_children(0));
    }
}
