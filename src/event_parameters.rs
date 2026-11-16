//! Structured capture lookups for the event browser's `$param` predicates.
use crate::event_filter::{Filter, ParamQuery, ParameterMatches};
use crate::replay::{Action, STRUCTURE_PAGE_LIMIT, Session, StructuredNode};
use std::collections::HashSet;

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
    for event in events {
        for query in &queries {
            if cancelled() {
                return Err("Event parameter lookup cancelled".into());
            }
            if matches_parameter(session, event, query, &cancelled)? {
                matches
                    .get_mut(query)
                    .expect("query inserted")
                    .insert(event);
            }
        }
    }
    Ok(matches)
}

fn matches_parameter(
    session: &Session,
    event: u32,
    query: &ParamQuery,
    cancelled: &impl Fn() -> bool,
) -> Result<bool, String> {
    // FindChildRecursively checks all direct children before descending into each child.
    let mut stack = vec![Vec::<u64>::new()];
    while let Some(path) = stack.pop() {
        let mut offset = 0u64;
        let mut children = Vec::new();
        loop {
            if cancelled() {
                return Err("Event parameter lookup cancelled".into());
            }
            let page = session.structure(event, &path, offset, STRUCTURE_PAGE_LIMIT)?;
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
                if child.name == query.name {
                    if child.basic_type == 2 {
                        return array_matches(session, event, &child_path, &query.value, cancelled);
                    }
                    return Ok(format_value(&child, page.node.flags)?
                        .to_lowercase()
                        .contains(&query.value));
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
    Ok(false)
}

fn array_matches(
    session: &Session,
    event: u32,
    path: &[u64],
    value: &str,
    cancelled: &impl Fn() -> bool,
) -> Result<bool, String> {
    let mut offset = 0;
    loop {
        if cancelled() {
            return Err("Event parameter lookup cancelled".into());
        }
        let page = session.structure(event, path, offset, STRUCTURE_PAGE_LIMIT)?;
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
