//! Structured capture lookups for the event browser's `$param` predicates.
use crate::event_filter::{Filter, ParamQuery, ParameterMatches};
use crate::replay::{Action, STRUCTURE_PAGE_LIMIT, Session, StructurePage, StructuredNode};
use std::collections::{HashMap, HashSet};

/// Resolve every predicate completely or return an error; partial results are never applied.
pub fn resolve(
    session: &Session,
    actions: &[Action],
    expression: &str,
    cancelled: impl Fn() -> bool,
) -> Result<ParameterMatches, String> {
    let filter = Filter::parse(expression).map_err(|()| "Invalid event filter".to_owned())?;
    let queries = filter.parameter_queries();
    let mut matches: ParameterMatches = queries
        .iter()
        .cloned()
        .map(|q| (q, HashSet::new()))
        .collect();
    if queries.is_empty() {
        return Ok(matches);
    }
    let events: HashSet<u32> = actions
        .iter()
        .flat_map(|a| a.api_events.iter())
        .filter(|event| event.chunk_index.is_some())
        .map(|event| event.event_id)
        .collect();
    let mut by_name: HashMap<String, Vec<ParamQuery>> = HashMap::new();
    for query in &queries {
        by_name
            .entry(query.name.clone())
            .or_default()
            .push(query.clone());
    }
    let mut names: Vec<String> = by_name.keys().cloned().collect();
    names.sort();
    let mut fast_available = true;
    for event in events {
        if cancelled() {
            return Err("Event parameter lookup cancelled".into());
        }
        let mut read =
            |path: &[u64], offset| session.structure(event, path, offset, STRUCTURE_PAGE_LIMIT);
        let mut found = HashSet::new();
        if fast_available {
            for group in names.chunks(128) {
                if cancelled() {
                    return Err("Event parameter lookup cancelled".into());
                }
                let Some(results) = session.structure_find(event, group)? else {
                    fast_available = false;
                    break;
                };
                for (name, item) in group.iter().zip(results) {
                    let Some(item) = item else { continue };
                    let path: Vec<u64> = item
                        .path
                        .iter()
                        .map(|index| index.parse::<u64>())
                        .collect::<Result<_, _>>()
                        .map_err(|_| "Invalid structured search path")?;
                    let formatted = if item.node.basic_type == 2 {
                        None
                    } else {
                        Some(format_value(&item.node, item.parent_flags)?.to_lowercase())
                    };
                    for query in &by_name[name] {
                        let matched = if let Some(formatted) = &formatted {
                            formatted.contains(&query.value)
                        } else {
                            array_matches(&mut read, &path, &query.value, &cancelled)?
                        };
                        if matched {
                            found.insert(query.clone());
                        }
                    }
                }
            }
        }
        if !fast_available {
            found = matches_parameters(&mut read, &queries, &cancelled)?;
        }
        for query in found {
            matches
                .get_mut(&query)
                .expect("query inserted")
                .insert(event);
        }
    }
    Ok(matches)
}

fn matches_parameters(
    read: &mut impl FnMut(&[u64], u64) -> Result<StructurePage, String>,
    queries: &HashSet<ParamQuery>,
    cancelled: &impl Fn() -> bool,
) -> Result<HashSet<ParamQuery>, String> {
    // FindChildRecursively checks all direct children before descending into each child.
    let mut stack = vec![Vec::<u64>::new()];
    let mut remaining = queries.clone();
    let mut matched = HashSet::new();
    while let Some(path) = stack.pop() {
        let mut offset = 0u64;
        let mut children = Vec::new();
        loop {
            if cancelled() {
                return Err("Event parameter lookup cancelled".into());
            }
            let page = read(&path, offset)?;
            let count = page
                .node
                .child_count
                .parse::<u64>()
                .map_err(|_| "Invalid structured child count")?;
            if page.children.is_empty() && offset < count {
                return Err("Incomplete structured data".into());
            }
            for child in page.children {
                let mut child_path = path.clone();
                child_path.push(offset);
                offset += 1;
                let matching_queries: Vec<ParamQuery> = remaining
                    .iter()
                    .filter(|query| child.name == query.name)
                    .cloned()
                    .collect();
                if !matching_queries.is_empty() {
                    let formatted = if child.basic_type == 2 {
                        None
                    } else {
                        Some(format_value(&child, page.node.flags)?.to_lowercase())
                    };
                    for query in matching_queries {
                        let is_match = if let Some(formatted) = &formatted {
                            formatted.contains(&query.value)
                        } else {
                            array_matches(read, &child_path, &query.value, cancelled)?
                        };
                        remaining.remove(&query);
                        if is_match {
                            matched.insert(query);
                        }
                    }
                    if remaining.is_empty() {
                        return Ok(matched);
                    }
                }
                if child.child_count != "0" {
                    children.push(child_path);
                }
            }
            if offset >= count {
                break;
            }
        }
        stack.extend(children.into_iter().rev());
    }
    Ok(matched)
}

fn array_matches(
    read: &mut impl FnMut(&[u64], u64) -> Result<StructurePage, String>,
    path: &[u64],
    value: &str,
    cancelled: &impl Fn() -> bool,
) -> Result<bool, String> {
    let mut offset = 0;
    loop {
        if cancelled() {
            return Err("Event parameter lookup cancelled".into());
        }
        let page = read(path, offset)?;
        let count = page
            .node
            .child_count
            .parse::<u64>()
            .map_err(|_| "Invalid structured child count")?;
        if page.children.is_empty() && offset < count {
            return Err("Incomplete structured data".into());
        }
        for child in page.children {
            offset += 1;
            if format_value(&child, page.node.flags)?
                .to_lowercase()
                .contains(value)
            {
                return Ok(true);
            }
        }
        if offset >= count {
            return Ok(false);
        }
    }
}

fn format_value(node: &StructuredNode, parent_flags: u32) -> Result<String, String> {
    // Resource names and floats depend on UI state and preferences. Refuse to guess.
    if node.basic_type == 12 || node.basic_type == 13 {
        return Err("Resource and GPU address parameter formatting is not supported yet".into());
    }
    if node.flags & 8 != 0 {
        return Ok("NULL".into());
    }
    if node.flags & 1 != 0 {
        return node
            .custom_string
            .clone()
            .ok_or_else(|| "Missing custom parameter string".into());
    }
    let scalar = || {
        node.value
            .clone()
            .ok_or_else(|| "Missing structured parameter value".to_owned())
    };
    match node.basic_type {
        0 => Ok(String::new()),
        1 => Ok(format!("{}()", node.type_name)),
        2 => Ok(format!("{}[{}]", node.type_name, node.child_count)),
        3 => Ok("NULL".into()),
        4 => Ok(format!("({} bytes)", node.byte_size)),
        5 => {
            let text = scalar()?;
            let lines: Vec<_> = text.split('\n').collect();
            let mut result = lines
                .iter()
                .take(3)
                .map(|line| format!("{line}\n"))
                .collect::<String>();
            if lines.len() > 3 {
                result.push_str("...");
            }
            Ok(result.trim().to_owned())
        }
        6 | 7 => {
            if (node.flags | parent_flags) & 0x200 != 0 {
                return Err("Offset parameter formatting depends on display preferences".into());
            }
            let n = scalar()?
                .parse::<u64>()
                .map_err(|_| "Invalid unsigned parameter")?;
            Ok(match n {
                65535 => "UINT16_MAX".into(),
                4294967295 => "UINT32_MAX".into(),
                u64::MAX => "UINT64_MAX".into(),
                n if n > 0xffffff && n < u32::MAX as u64 => format!("0x{n:08X}"),
                n if n > 0xffffff => format!("0x{n:016X}"),
                n => n.to_string(),
            })
        }
        8 => scalar(),
        10 => match scalar()?.as_str() {
            "true" => Ok("True".into()),
            "false" => Ok("False".into()),
            _ => Err("Invalid boolean parameter".into()),
        },
        11 => {
            let n = scalar()?
                .parse::<u8>()
                .map_err(|_| "Invalid character parameter")?;
            Ok(char::from(n).to_string())
        }
        _ => Err("Parameter value formatting is not supported yet".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn node(name: &str, kind: u32, value: Option<&str>, count: usize) -> StructuredNode {
        StructuredNode {
            name: name.into(),
            type_name: "T".into(),
            basic_type: kind,
            flags: 0,
            byte_size: "0".into(),
            child_count: count.to_string(),
            value: value.map(str::to_owned),
            custom_string: None,
            resource_non_null: None,
            resource_index: None,
        }
    }

    #[test]
    fn conditions_share_one_traversal_and_direct_children_win_across_pages() {
        let mut root = vec![node("container", 1, None, 1)];
        root.extend((0..127).map(|_| node("other", 5, Some("x"), 0)));
        root.push(node("target", 5, Some("Direct"), 0));
        let mut data = HashMap::new();
        data.insert(vec![], root);
        data.insert(vec![0], vec![node("target", 5, Some("nested"), 0)]);
        let mut reads = 0;
        let mut read = |path: &[u64], offset: u64| {
            reads += 1;
            let children = &data[path];
            Ok(StructurePage {
                event_id: 1,
                offset: offset.to_string(),
                path: path.iter().map(u64::to_string).collect(),
                node: node("parent", 1, None, children.len()),
                children: children
                    .iter()
                    .skip(offset as usize)
                    .take(STRUCTURE_PAGE_LIMIT as usize)
                    .cloned()
                    .collect(),
                chunk_metadata: None,
            })
        };
        let queries: HashSet<_> = ["direct", "nested", "rect"]
            .into_iter()
            .map(|value| ParamQuery {
                name: "target".into(),
                value: value.into(),
            })
            .collect();
        let matched = matches_parameters(&mut read, &queries, &|| false).unwrap();
        assert_eq!(reads, 2); // One shared scan, including the second root page.
        assert!(matched.iter().any(|q| q.value == "direct"));
        assert!(matched.iter().any(|q| q.value == "rect"));
        assert!(!matched.iter().any(|q| q.value == "nested"));
    }

    #[test]
    fn cancellation_and_incomplete_pages_never_return_partial_matches() {
        let query = ParamQuery {
            name: "a".into(),
            value: "yes".into(),
        };
        let queries = [query].into();
        let mut calls = 0;
        let mut read = |_: &[u64], offset: u64| {
            calls += 1;
            Ok(StructurePage {
                event_id: 1,
                offset: offset.to_string(),
                path: vec![],
                node: node("parent", 1, None, 1),
                children: vec![],
                chunk_metadata: None,
            })
        };
        assert!(matches_parameters(&mut read, &queries, &|| true).is_err());
        assert!(matches_parameters(&mut read, &queries, &|| false).is_err());
        assert_eq!(calls, 1);
    }
}
