use crate::event_filter::{EventContext, Filter};
use crate::replay::Action;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventRow {
    Action(usize),
    Api { action: usize, event: usize },
}

#[derive(Clone, Debug)]
pub struct ExportRow {
    pub event_id: u32,
    pub action_id: Option<u32>,
    pub name: String,
    pub depth: usize,
    pub first_child: bool,
    pub duration: Option<f64>,
}

/// Rows retain their snapshot identity even when filtering or expansion changes.
#[derive(Default)]
pub struct EventTree {
    action_count: usize,
    rows: Vec<EventRow>,
    order: Vec<usize>,
    show_api: bool,
    parents: Vec<Option<usize>>,
    names: Vec<String>,
    lower_names: Vec<String>,
    marker_names: Vec<String>,
    filter: Filter,
    filter_error: bool,
    actions: Vec<Action>,
    event_ids: Vec<u32>,
    durations: Option<std::collections::HashMap<u32, f64>>,
    matching_children: Vec<bool>,
    direct_matching: Vec<bool>,
    eligible: Vec<bool>,
    collapsed: Vec<bool>,
    pub visible: Vec<usize>,
}

impl EventTree {
    pub fn reset(&mut self, actions: &[Action]) {
        self.durations = None;
        self.action_count = actions.len();
        self.actions = actions.to_vec();
        self.marker_names = actions
            .iter()
            .map(|a| a.marker_name.as_deref().unwrap_or("").to_lowercase())
            .collect();
        self.event_ids = actions.iter().map(|action| action.event_id).collect();
        self.rows = (0..actions.len()).map(EventRow::Action).collect();
        self.order.clear();
        self.parents.clear();
        self.names = actions.iter().map(|action| action.name.clone()).collect();
        self.lower_names = actions
            .iter()
            .map(|action| action.name.to_lowercase())
            .collect();
        for (index, action) in actions.iter().enumerate() {
            let parent = usize::try_from(action.parent).ok().filter(|&p| p < index);
            self.parents.push(parent);
        }
        for (action_index, action) in actions.iter().enumerate() {
            for (event_index, event) in action.api_events.iter().enumerate() {
                // The action itself occupies the row for its own API event.
                if event.event_id == action.event_id {
                    continue;
                }
                self.order.push(self.rows.len());
                self.rows.push(EventRow::Api {
                    action: action_index,
                    event: event_index,
                });
                self.parents.push(self.parents[action_index]);
                self.names.push(event.name.clone());
                self.lower_names.push(event.name.to_lowercase());
                self.event_ids.push(event.event_id);
            }
            self.order.push(action_index);
        }
        self.collapsed = vec![false; self.parents.len()];
        self.rebuild();
    }

    pub fn len(&self) -> usize {
        self.action_count
    }

    pub fn row(&self, index: usize) -> Option<EventRow> {
        self.rows.get(index).copied()
    }

    pub fn directly_matches(&self, index: usize) -> bool {
        self.direct_matching.get(index).copied().unwrap_or(false)
    }

    pub fn set_durations(&mut self, durations: std::collections::HashMap<u32, f64>) {
        self.durations = Some(durations);
        self.rebuild();
    }

    pub fn duration(&self, event_id: u32) -> Option<f64> {
        self.durations.as_ref()?.get(&event_id).copied()
    }

    pub fn has_durations(&self) -> bool {
        self.durations.is_some()
    }

    pub fn clear_durations(&mut self) {
        self.durations = None;
        self.rebuild();
    }

    pub fn show_api(&self) -> bool {
        self.show_api
    }

    pub fn set_show_api(&mut self, show: bool) {
        self.show_api = show;
        self.rebuild();
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
        match Filter::parse(text) {
            Ok(filter) => {
                self.filter = filter;
                self.filter_error = false;
                self.rebuild();
            }
            Err(()) => self.filter_error = true,
        }
    }

    pub fn filter_error(&self) -> bool {
        self.filter_error
    }

    /// Find among rows admitted by the current filter, including collapsed rows.
    pub fn find(&self, query: &str, current: Option<u32>, forward: bool) -> Option<usize> {
        if query.is_empty() {
            return None;
        }
        let explicit_eid = query.starts_with('@');
        let number = query.strip_prefix('@').unwrap_or(query).trim();
        let numeric = number
            .parse::<u32>()
            .ok()
            .filter(|id| *id > 0 && number == id.to_string());
        let known_eid = numeric.filter(|id| self.event_ids.contains(id));
        if let Some(id) = known_eid {
            // Upstream advances by EID when an exact EID is filtered out.
            return self
                .order
                .iter()
                .copied()
                .filter(|&row| self.eligible[row] && self.event_ids[row] >= id)
                .min_by_key(|&row| self.event_ids[row]);
        }
        if explicit_eid {
            return None;
        }
        let text = query.to_lowercase();
        let matches: Vec<usize> = self
            .order
            .iter()
            .copied()
            .filter(|&row| self.eligible[row] && self.lower_names[row].contains(&text))
            .collect();
        if matches.is_empty() {
            return None;
        }
        if let Some(current) = current {
            if let Some(index) = matches
                .iter()
                .position(|&row| self.event_ids[row] == current)
            {
                return Some(
                    matches[if forward {
                        (index + 1) % matches.len()
                    } else {
                        (index + matches.len() - 1) % matches.len()
                    }],
                );
            }
            if forward {
                return matches
                    .iter()
                    .copied()
                    .find(|&row| self.event_ids[row] >= current)
                    .or_else(|| matches.first().copied());
            }
            return matches
                .iter()
                .rev()
                .copied()
                .find(|&row| self.event_ids[row] <= current)
                .or_else(|| matches.last().copied());
        }
        if forward {
            matches.first().copied()
        } else {
            matches.last().copied()
        }
    }

    pub fn reveal_filtered(&mut self, row: usize) -> Option<usize> {
        if !self.eligible.get(row).copied().unwrap_or(false) {
            return None;
        }
        let mut parent = self.parents[row];
        while let Some(index) = parent {
            self.collapsed[index] = false;
            parent = self.parents[index];
        }
        self.rebuild();
        self.visible.iter().position(|&index| index == row)
    }

    pub fn event_id(&self, row: usize) -> Option<u32> {
        self.event_ids.get(row).copied()
    }

    /// Export the filter-admitted tree, independent of interactive collapse state.
    pub fn export_rows(&self) -> Vec<ExportRow> {
        let mut seen_parents = std::collections::HashSet::new();
        let parents: std::collections::HashSet<usize> =
            self.parents.iter().flatten().copied().collect();
        self.order
            .iter()
            .copied()
            .filter(|&row| self.eligible[row])
            .map(|row| {
                let (action, is_action) = match self.rows[row] {
                    EventRow::Action(index) => (&self.actions[index], true),
                    EventRow::Api { action, .. } => (&self.actions[action], false),
                };
                let first_child =
                    self.parents[row].is_some_and(|parent| seen_parents.insert(parent));
                ExportRow {
                    event_id: self.event_ids[row],
                    action_id: (is_action
                        && action.flags & (0x20 | 0x40 | 0x80) == 0
                        && !parents.contains(&row))
                    .then_some(action.action_id),
                    name: self.names[row].clone(),
                    depth: action.depth,
                    first_child,
                    duration: self.duration(self.event_ids[row]),
                }
            })
            .collect()
    }

    pub fn reveal(&mut self, action: usize, api: Option<usize>) -> Option<usize> {
        let row = match api {
            Some(event) => {
                self.show_api = true;
                self.rows
                    .iter()
                    .position(|row| *row == (EventRow::Api { action, event }))?
            }
            None => action,
        };
        if row >= self.parents.len() {
            return None;
        }
        self.filter = Filter::default();
        self.filter_error = false;
        let mut parent = self.parents[row];
        while let Some(index) = parent {
            self.collapsed[index] = false;
            parent = self.parents[index];
        }
        self.rebuild();
        self.visible.iter().position(|index| *index == row)
    }

    fn rebuild(&mut self) {
        self.visible.clear();
        let mut matching: Vec<bool> = self
            .names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                let action = match self.rows[index] {
                    EventRow::Action(action) => self.actions.get(action),
                    EventRow::Api { .. } => None,
                };
                (self.show_api || index < self.action_count)
                    && self.filter.matches(&EventContext {
                        name,
                        lower_name: &self.lower_names[index],
                        eid: self.event_ids[index],
                        action,
                        parent: self.parents[index],
                        parents: &self.parents,
                        marker_names: &self.marker_names,
                        duration: self.duration(self.event_ids[index]),
                    })
            })
            .collect();
        self.direct_matching = matching.clone();
        self.matching_children = vec![false; self.parents.len()];
        for index in (0..self.parents.len()).rev() {
            if matching[index] {
                if let Some(parent) = self.parents[index] {
                    matching[parent] = true;
                    self.matching_children[parent] = true;
                }
            }
        }
        self.eligible = matching.clone();
        let mut shown = vec![false; self.parents.len()];
        for (index, parent) in self.parents.iter().enumerate() {
            shown[index] = matching[index]
                && parent.is_none_or(|parent| shown[parent] && !self.collapsed[parent]);
        }
        self.visible
            .extend(self.order.iter().copied().filter(|&index| shown[index]));
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
            marker_color_bits: None,
            marker_name: None,
            indices: 0,
            instances: 0,
            base_vertex: Some(0),
            index_offset: Some(0),
            vertex_offset: Some(0),
            instance_offset: Some(0),
            dispatch_groups: None,
            dispatch_threads: None,
            name: String::new(),
            api_events: Vec::new(),
        }
    }

    #[test]
    fn reveal_clears_filter_and_expands_collapsed_ancestors() {
        let mut tree = EventTree::default();
        tree.reset(&[action(-1), action(0), action(1)]);
        tree.toggle(0);
        tree.set_filter("does not match");
        assert!(tree.visible.is_empty());
        assert_eq!(tree.reveal(2, None), Some(2));
        assert_eq!(tree.visible, [0, 1, 2]);
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

    #[test]
    fn api_calls_are_siblings_before_the_action_and_follow_its_parent_visibility() {
        use crate::replay::ApiEvent;
        let mut parent = action(-1);
        parent.event_id = 1;
        parent.name = "Parent".into();
        let mut draw = action(0);
        draw.event_id = 4;
        draw.name = "Draw".into();
        draw.api_events = [(2, "Bind()"), (3, "State()"), (4, "Draw()")]
            .into_iter()
            .map(|(event_id, name)| ApiEvent {
                event_id,
                chunk_index: Some(event_id),
                file_offset: "0".into(),
                name: name.into(),
            })
            .collect();
        let mut tree = EventTree::default();
        tree.reset(&[parent, draw]);
        assert_eq!(tree.visible, [0, 1]);
        tree.set_show_api(true);
        assert_eq!(tree.visible, [0, 2, 3, 1]);
        assert_eq!(
            tree.row(2),
            Some(EventRow::Api {
                action: 1,
                event: 0
            })
        );
        tree.toggle(0);
        assert_eq!(tree.visible, [0]);
        tree.toggle(0);
        tree.set_filter("BIND");
        assert_eq!(tree.visible, [0, 2]);
        tree.set_show_api(false);
        assert!(tree.visible.is_empty());
        tree.set_filter("");
        assert_eq!(tree.visible, [0, 1]);
    }

    #[test]
    fn upstream_expression_subset_filters_actions_and_api_events() {
        use crate::replay::ApiEvent;
        let mut parent = action(-1);
        parent.event_id = 1;
        parent.name = "Frame".into();
        let mut draw = action(0);
        draw.event_id = 4;
        draw.action_id = 2;
        draw.indices = 12;
        draw.name = "Draw indexed".into();
        draw.api_events.push(ApiEvent {
            event_id: 3,
            chunk_index: Some(3),
            file_offset: "0".into(),
            name: "Bind texture".into(),
        });
        let mut clear = action(0);
        clear.event_id = 5;
        clear.name = "Clear".into();
        let mut tree = EventTree::default();
        tree.reset(&[parent, draw, clear]);
        tree.set_show_api(true);
        tree.set_filter("+(\"draw indexed\" bind) +$event(EID >= 0x3) -$event(EID=5)");
        assert!(!tree.filter_error());
        assert_eq!(tree.visible, [0, 3, 1]);
        tree.set_filter("$action(numVertices >= 12)");
        assert_eq!(tree.visible, [0, 1]);
        tree.set_filter("$event(EID = 3)");
        assert_eq!(tree.visible, [0, 3]);
        tree.set_filter("$action(unknown = 1)");
        assert!(tree.filter_error());
        assert_eq!(tree.visible, [0, 3]);
        tree.set_filter("$event(EID = 4294967296)");
        assert!(tree.filter_error());
        tree.set_filter("$action()");
        assert!(!tree.filter_error());
        assert_eq!(tree.visible, [0, 1, 2]);
    }

    #[test]
    fn action_flags_dispatch_and_pcre_filters() {
        let mut frame = action(-1);
        frame.name = "Frame".into();
        let mut draw = action(0);
        draw.name = "Draw Draw".into();
        draw.flags = 0x2 | 0x10000;
        let mut dispatch = action(0);
        dispatch.name = "Compute 日本".into();
        dispatch.flags = 0x4;
        dispatch.dispatch_groups = Some([2, 3, 4]);
        let mut pop = action(0);
        pop.name = "Pop".into();
        pop.flags = 0x80;
        let mut tree = EventTree::default();
        tree.reset(&[frame, draw, dispatch, pop]);
        tree.set_filter("$action(flags & Drawcall|PopMarker)");
        assert_eq!(tree.visible, [0, 1, 3]);
        tree.set_filter("$dispatch(size=24)");
        assert_eq!(tree.visible, [0, 2]);
        tree.set_filter("$action(dispatchSize=24)");
        assert_eq!(tree.visible, [0, 2]);
        tree.set_filter(r"$regex(/^(Draw) \1$/)");
        assert_eq!(tree.visible, [0, 1]);
        tree.set_filter(r"$regex(/(*UCP)(?i)^compute \w+$/)");
        assert_eq!(tree.visible, [0, 2]);
        tree.set_filter(r"$regex(/^compute/iu)");
        assert!(tree.visible.is_empty());
        tree.set_filter(r"$regex(/draw/)");
        assert!(tree.visible.is_empty());
        tree.set_filter("$action(flags & Drawcall|)");
        assert!(tree.filter_error());
        assert!(tree.visible.is_empty());
        tree.set_filter("$regex(/(/)");
        assert!(tree.filter_error());
    }

    #[test]
    fn parent_filter_uses_custom_names_and_handles_api_siblings() {
        use crate::replay::ApiEvent;
        let mut outer = action(-1);
        outer.event_id = 1;
        outer.name = "Display name".into();
        outer.marker_name = Some("Main Pass".into());
        let mut inner = action(0);
        inner.event_id = 2;
        inner.marker_name = Some("Shadow".into());
        let mut draw = action(1);
        draw.event_id = 4;
        draw.api_events.push(ApiEvent {
            event_id: 3,
            chunk_index: None,
            file_offset: "0".into(),
            name: "Bind".into(),
        });
        let mut root_draw = action(-1);
        root_draw.event_id = 5;
        let mut tree = EventTree::default();
        tree.reset(&[outer, inner, draw, root_draw]);
        tree.set_show_api(true);
        tree.set_filter("$parent(mAiN)");
        assert_eq!(tree.visible, [0, 1, 4, 2]);
        assert!(!tree.directly_matches(0));
        assert!(tree.directly_matches(1));
        assert!(tree.directly_matches(4));
        tree.set_filter("+$childOf(Shadow) +$event(EID=3)");
        assert_eq!(tree.visible, [0, 1, 4]);
        tree.set_filter("$parent(Display)");
        assert!(tree.visible.is_empty());
        tree.set_filter("$parent()");
        assert_eq!(tree.visible, [0, 1, 4, 2]);
    }

    #[test]
    fn duration_filters_require_collected_results_and_respect_dispatch() {
        let mut parent = action(-1);
        parent.event_id = 1;
        let mut draw = action(0);
        draw.event_id = 2;
        draw.flags = 2;
        let mut dispatch = action(0);
        dispatch.event_id = 3;
        dispatch.flags = 4;
        let mut tree = EventTree::default();
        tree.reset(&[parent, draw, dispatch]);
        tree.set_filter("$action(duration >= 1.5ms)");
        assert!(tree.visible.is_empty());
        tree.set_durations([(1, 0.003), (2, 0.001), (3, 0.002)].into());
        assert_eq!(tree.visible, [0, 2]);
        tree.set_filter("$dispatch(duration > 1000us)");
        assert_eq!(tree.visible, [0, 2]);
        tree.set_filter("$dispatch(duration == 2ms)");
        assert!(tree.filter_error());
        assert_eq!(tree.visible, [0, 2]);
        tree.clear_durations();
        assert!(tree.visible.is_empty());
    }

    #[test]
    fn find_wraps_reveals_collapsed_rows_and_obeys_filter() {
        use crate::replay::ApiEvent;
        let mut parent = action(-1);
        parent.event_id = 1;
        parent.name = "Frame".into();
        let mut first = action(0);
        first.event_id = 4;
        first.name = "Draw first".into();
        first.api_events.push(ApiEvent {
            event_id: 3,
            chunk_index: None,
            file_offset: "0".into(),
            name: "Bind texture".into(),
        });
        let mut second = action(0);
        second.event_id = 5;
        second.name = "DRAW second".into();
        let mut tree = EventTree::default();
        tree.reset(&[parent, first, second]);
        tree.toggle(0);
        let row = tree.find("draw", None, true).unwrap();
        assert_eq!(tree.event_id(row), Some(4));
        assert_eq!(tree.reveal_filtered(row), Some(1));
        assert_eq!(
            tree.event_id(tree.find("draw", Some(4), true).unwrap()),
            Some(5)
        );
        assert_eq!(
            tree.event_id(tree.find("draw", Some(4), false).unwrap()),
            Some(5)
        );
        assert_eq!(tree.event_id(tree.find("@3", None, true).unwrap()), Some(4));
        tree.set_show_api(true);
        assert_eq!(tree.event_id(tree.find("@3", None, true).unwrap()), Some(3));
        tree.set_filter("second");
        assert_eq!(
            tree.event_id(tree.find("draw", None, true).unwrap()),
            Some(5)
        );
        assert_eq!(tree.event_id(tree.find("@3", None, true).unwrap()), Some(5));
        assert!(tree.find("absent", None, true).is_none());
    }

    #[test]
    fn export_includes_filtered_collapsed_descendants_and_api_siblings() {
        use crate::replay::ApiEvent;
        let mut parent = action(-1);
        parent.event_id = 1;
        parent.flags = 0x40;
        parent.name = "Pass".into();
        let mut draw = action(0);
        draw.event_id = 3;
        draw.action_id = 1;
        draw.depth = 1;
        draw.name = "Draw".into();
        draw.api_events.push(ApiEvent {
            event_id: 2,
            chunk_index: None,
            file_offset: "0".into(),
            name: "Bind".into(),
        });
        let mut tree = EventTree::default();
        tree.reset(&[parent, draw]);
        tree.set_show_api(true);
        tree.toggle(0);
        assert_eq!(tree.visible, [0]);
        let rows = tree.export_rows();
        assert_eq!(
            rows.iter().map(|r| r.event_id).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(rows[0].action_id, None);
        assert!(rows[1].first_child);
        assert!(!rows[2].first_child);
        assert_eq!(rows[2].action_id, Some(1));
        tree.set_filter("draw");
        assert_eq!(
            tree.export_rows()
                .iter()
                .map(|r| r.event_id)
                .collect::<Vec<_>>(),
            [1, 3]
        );
    }
}
