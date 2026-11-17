//! Capture action paths in RenderDoc's root-to-leaf order.
use crate::replay::Action;

pub fn ancestry(actions: &[Action], index: usize) -> Option<Vec<usize>> {
    let mut result = Vec::new();
    let mut current = index;
    loop {
        let action = actions.get(current)?;
        result.push(current);
        match usize::try_from(action.parent) {
            Ok(parent) if parent < current => current = parent,
            Ok(_) => return None,
            Err(_) if action.parent == -1 => break,
            Err(_) => return None,
        }
    }
    result.reverse();
    Some(result)
}

pub fn label(action: &Action) -> String {
    if action.name.is_empty() {
        format!("unnamed{}", action.event_id)
    } else {
        action.name.clone()
    }
}

pub fn format(actions: &[Action], index: usize) -> Option<String> {
    Some(
        ancestry(actions, index)?
            .into_iter()
            .map(|i| label(&actions[i]).replace('\\', "\\\\").replace('/', "\\/"))
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// Resolve a path against sibling actions, preferring exact names over unnamed EID aliases.
pub fn resolve(actions: &[Action], path: &str) -> Option<Option<usize>> {
    let mut parts = vec![String::new()];
    let mut chars = path.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' if matches!(chars.peek(), Some('/' | '\\')) => {
                parts.last_mut()?.push(chars.next()?);
            }
            '/' => parts.push(String::new()),
            _ => parts.last_mut()?.push(ch),
        }
    }
    let mut parent = None;
    for part in parts.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let siblings = || {
            actions
                .iter()
                .enumerate()
                .filter(|(i, action)| match parent {
                    Some(p) => usize::try_from(action.parent).ok() == Some(p) && p < *i,
                    None => action.parent == -1,
                })
        };
        parent = siblings()
            .find(|(_, action)| action.name == part)
            .or_else(|| {
                siblings().find(|(_, action)| format!("unnamed{}", action.event_id) == part)
            })
            .map(|(index, _)| index);
        parent?;
    }
    Some(parent)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn action(event: u32, parent: i64, name: &str) -> Action {
        Action {
            event_id: event,
            action_id: event,
            depth: 0,
            parent,
            flags: 0,
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
            name: name.into(),
            api_events: vec![],
        }
    }
    #[test]
    fn paths_resolve_nested_names_escapes_aliases_and_root() {
        let actions = [
            action(1, -1, "Pass/one"),
            action(2, 0, "draw\\mesh"),
            action(3, 0, ""),
            action(4, -1, "unnamed3"),
        ];
        assert_eq!(ancestry(&actions, 1), Some(vec![0, 1]));
        let path = format(&actions, 1).unwrap();
        assert_eq!(path, "Pass\\/one/draw\\\\mesh");
        assert_eq!(resolve(&actions, &path), Some(Some(1)));
        assert_eq!(resolve(&actions, "/Pass\\/one//unnamed3/"), Some(Some(2)));
        assert_eq!(resolve(&actions, "unnamed3"), Some(Some(3)));
        assert_eq!(resolve(&actions, "/"), Some(None));
        assert_eq!(resolve(&actions, "missing"), None);
    }
}
