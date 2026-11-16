use renderdocx::replay::Action;

/// A view of the upstream action hierarchy. Indices always refer to the snapshot.
#[derive(Default)]
pub struct EventTree {
    parents: Vec<Option<usize>>,
    names: Vec<String>,
    filter: String,
    matching_children: Vec<bool>,
    collapsed: Vec<bool>,
    pub visible: Vec<usize>,
}

impl EventTree {
    pub fn reset(&mut self, actions: &[Action]) {
        self.parents.clear();
        self.names = actions
            .iter()
            .map(|action| action.name.to_lowercase())
            .collect();
        self.collapsed = vec![false; actions.len()];
        for (index, action) in actions.iter().enumerate() {
            let parent = usize::try_from(action.parent).ok().filter(|&p| p < index);
            self.parents.push(parent);
        }
        self.rebuild();
    }

    pub fn len(&self) -> usize {
        self.parents.len()
    }

    pub fn has_children(&self, index: usize) -> bool {
        self.matching_children.get(index).copied().unwrap_or(false)
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

    pub fn set_filter(&mut self, text: &str) {
        self.filter = text.to_lowercase();
        self.rebuild();
    }

    fn rebuild(&mut self) {
        self.visible.clear();
        let mut matching: Vec<bool> = self
            .names
            .iter()
            .map(|name| name.contains(&self.filter))
            .collect();
        self.matching_children = vec![false; self.parents.len()];
        for index in (0..self.parents.len()).rev() {
            if matching[index] {
                if let Some(parent) = self.parents[index] {
                    matching[parent] = true;
                    self.matching_children[parent] = true;
                }
            }
        }
        let mut shown = vec![false; self.parents.len()];
        for (index, parent) in self.parents.iter().enumerate() {
            shown[index] = matching[index]
                && parent.is_none_or(|parent| shown[parent] && !self.collapsed[parent]);
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
            api_events: Vec::new(),
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

    #[test]
    fn filtering_is_case_insensitive_keeps_ancestors_and_restores_collapse_state() {
        let mut actions = [action(-1), action(0), action(1), action(0), action(-1)];
        for (action, name) in
            actions
                .iter_mut()
                .zip(["Frame", "Pass", "Draw 日本語", "Clear", "Other"])
        {
            action.name = name.into();
        }
        let mut tree = EventTree::default();
        tree.reset(&actions);
        tree.toggle(1);
        tree.set_filter("DRAW 日");
        assert_eq!(tree.visible, [0, 1]);
        tree.toggle(1);
        assert_eq!(tree.visible, [0, 1, 2]);
        assert!(!tree.has_children(2));
        tree.set_filter("missing");
        assert!(tree.visible.is_empty());
        tree.set_filter("pass");
        assert_eq!(tree.visible, [0, 1]);
        assert!(!tree.has_children(1));
        tree.set_filter("");
        assert_eq!(tree.visible, [0, 1, 2, 3, 4]);
        tree.toggle(0);
        tree.set_filter("clear");
        assert_eq!(tree.visible, [0]);
        tree.set_filter("");
        assert_eq!(tree.visible, [0, 4]);
        tree.set_filter("other");
        let mut replacement = action(-1);
        replacement.name = "OTHER frame".into();
        tree.reset(&[replacement]);
        assert_eq!(tree.visible, [0]);
    }
}
