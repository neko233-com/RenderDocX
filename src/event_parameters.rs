//! Structured capture lookups for the event browser's `$param` predicates.
use crate::event_filter::{Filter, ParamQuery, ParameterMatches};
use crate::i18n::Language;
use crate::replay::{
    Action, Buffer, ResourceInfo, STRUCTURE_PAGE_LIMIT, Session, StructurePage, StructuredNode,
};
use std::collections::{HashMap, HashSet};
use unicode_segmentation::UnicodeSegmentation;

pub(crate) struct ResourceContext<'a> {
    pub(crate) resources: &'a [ResourceInfo],
    pub(crate) buffers: &'a [Buffer],
    pub(crate) renames: Result<serde_json::Value, String>,
    pub(crate) language: Language,
}

/// Format a selected API call using the upstream important/hidden parameter rules.
pub fn format_event(
    session: &Session,
    event: u32,
    resources: &[ResourceInfo],
    buffers: &[Buffer],
    language: Language,
    show_all_parameters: bool,
    show_parameter_names: bool,
) -> Result<String, String> {
    let context = ResourceContext {
        resources,
        buffers,
        language,
        renames: session
            .annotation_read("renames")
            .and_then(|section| section.document()),
    };
    let mut read =
        |path: &[u64], offset| session.structure(event, path, offset, STRUCTURE_PAGE_LIMIT);
    format_event_with(
        &mut read,
        &context,
        show_all_parameters,
        show_parameter_names,
    )
}

/// Format multiple independent events with one resource-name snapshot.
pub fn format_events(
    session: &Session,
    events: &[u32],
    resources: &[ResourceInfo],
    buffers: &[Buffer],
    language: Language,
    show_all_parameters: bool,
    show_parameter_names: bool,
    cancelled: impl Fn() -> bool,
) -> Result<Vec<(u32, Result<String, String>)>, String> {
    let context = ResourceContext {
        resources,
        buffers,
        language,
        renames: session
            .annotation_read("renames")
            .and_then(|section| section.document()),
    };
    let mut results = Vec::with_capacity(events.len());
    for &event in events {
        if cancelled() {
            return Err("Event formatting cancelled".into());
        }
        let mut read =
            |path: &[u64], offset| session.structure(event, path, offset, STRUCTURE_PAGE_LIMIT);
        results.push((
            event,
            format_event_with(
                &mut read,
                &context,
                show_all_parameters,
                show_parameter_names,
            ),
        ));
    }
    Ok(results)
}

/// Complete names for the already-selected export rows before publishing any file.
pub fn format_export_rows(
    session: &Session,
    rows: &mut [crate::event_tree::ExportRow],
    targets: &[u32],
    resources: &[ResourceInfo],
    buffers: &[Buffer],
    language: Language,
    show_all_parameters: bool,
    show_parameter_names: bool,
) -> Result<(), String> {
    if targets.is_empty() {
        return Ok(());
    }
    let results = format_events(
        session,
        targets,
        resources,
        buffers,
        language,
        show_all_parameters,
        show_parameter_names,
        || false,
    )?;
    let mut names = HashMap::new();
    for (event, result) in results {
        names.insert(
            event,
            result.map_err(|error| format!("Event {event}: {error}"))?,
        );
    }
    for row in rows {
        if let Some(name) = names.get(&row.event_id) {
            row.name.clone_from(name);
        }
    }
    Ok(())
}

fn format_event_with(
    read: &mut impl FnMut(&[u64], u64) -> Result<StructurePage, String>,
    context: &ResourceContext<'_>,
    show_all: bool,
    show_names: bool,
) -> Result<String, String> {
    let (root, children) = formatting_children(read, &[])?;
    let name = root
        .name
        .split_once("::")
        .map_or(root.name.as_str(), |(_, suffix)| suffix);
    let mut arguments = Vec::new();
    for (index, child) in children {
        if child.flags & 2 != 0 || (root.flags & 0x80 != 0 && child.flags & 0x40 == 0 && !show_all)
        {
            continue;
        }
        let value = format_inline(read, &[index], &child, root.flags, context)?;
        arguments.push(if show_names {
            format!("{}={value}", child.name)
        } else {
            value
        });
    }
    Ok(format!("{name}({})", arguments.join(", ")))
}

fn formatting_children(
    read: &mut impl FnMut(&[u64], u64) -> Result<StructurePage, String>,
    path: &[u64],
) -> Result<(StructuredNode, Vec<(u64, StructuredNode)>), String> {
    let mut offset = 0u64;
    let mut root = None;
    let mut all = Vec::new();
    loop {
        let page = read(path, offset)?;
        let count = page
            .node
            .child_count
            .parse::<u64>()
            .map_err(|_| "Invalid structured child count")?;
        if root.is_none() {
            root = Some(page.node.clone());
        }
        if page.children.is_empty() && offset < count {
            return Err("Incomplete structured data".into());
        }
        for (position, child) in page.children.into_iter().enumerate() {
            let index = offset
                .checked_add(position as u64)
                .ok_or("Structured index overflow")?;
            if index >= count {
                return Err("Invalid structured child range".into());
            }
            all.push((index, child));
        }
        offset = u64::try_from(all.len()).map_err(|_| "Structured index overflow")?;
        if offset >= count {
            break;
        }
    }
    Ok((root.ok_or("Missing structured node")?, all))
}

fn format_inline(
    read: &mut impl FnMut(&[u64], u64) -> Result<StructurePage, String>,
    path: &[u64],
    node: &StructuredNode,
    parent_flags: u32,
    context: &ResourceContext<'_>,
) -> Result<String, String> {
    // Keep the same precedence as SDObject2Variant: resource and custom values bypass traversal.
    if node.flags & (1 | 8) != 0 || !(node.basic_type == 1 || node.basic_type == 2) {
        return format_value(node, parent_flags, context);
    }
    let count = node
        .child_count
        .parse::<u64>()
        .map_err(|_| "Invalid structured child count")?;
    if node.basic_type == 2 && count > 4 {
        return format_value(node, parent_flags, context);
    }
    if count == 0 {
        return Ok("{}".into());
    }
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let (_, children) = formatting_children(read, path)?;
        let mut values = Vec::new();
        for (index, child) in children {
            if node.flags & 0x80 != 0 && child.flags & 0x40 == 0 {
                continue;
            }
            let mut child_path = path.to_vec();
            child_path.push(index);
            values.push(format_inline(
                read,
                &child_path,
                &child,
                node.flags,
                context,
            )?);
        }
        if node.basic_type == 1 && node.flags & 0x80 != 0 && values.len() == 1 {
            Ok(values.remove(0))
        } else {
            Ok(format!("{{ {} }}", values.join(", ")))
        }
    })
}

fn format_gpu_address(value: u64, context: &ResourceContext<'_>) -> Result<String, String> {
    if let Some((buffer_index, offset)) = crate::gpu_address::resolve(context.buffers, value)? {
        let buffer = &context.buffers[buffer_index];
        let Some(index) = buffer.resource_index else {
            return Ok(format!("0x{value:x}"));
        };
        let resource = context
            .resources
            .get(index)
            .ok_or("Invalid buffer resource index")?;
        let Some(id) = resource
            .rename_key
            .as_deref()
            .and_then(|key| key.strip_prefix("ResourceId::"))
        else {
            return Ok(format!("0x{value:x}"));
        };
        let node = StructuredNode {
            name: String::new(),
            type_name: String::new(),
            basic_type: 12,
            flags: 0,
            byte_size: "0".into(),
            child_count: "0".into(),
            value: None,
            custom_string: None,
            resource_non_null: Some(true),
            resource_index: Some(index),
            resource_id: Some(id.into()),
            resource_edited: Some(false),
        };
        return Ok(format!("{}+{offset}", resource_name(&node, context)?));
    }
    Ok(format!("0x{value:x}"))
}

/// Resolve every predicate completely or return an error; partial results are never applied.
pub fn resolve(
    session: &Session,
    actions: &[Action],
    resources: &[ResourceInfo],
    buffers: &[Buffer],
    language: Language,
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
    let context = ResourceContext {
        resources,
        buffers,
        language,
        renames: session
            .annotation_read("renames")
            .and_then(|section| section.document()),
    };
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
                        Some(format_value(&item.node, item.parent_flags, &context)?.to_lowercase())
                    };
                    for query in &by_name[name] {
                        let matched = if let Some(formatted) = &formatted {
                            formatted.contains(&query.value)
                        } else {
                            array_matches(&mut read, &path, &query.value, &cancelled, &context)?
                        };
                        if matched {
                            found.insert(query.clone());
                        }
                    }
                }
            }
        }
        if !fast_available {
            found = matches_parameters(&mut read, &queries, &cancelled, &context)?;
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
    context: &ResourceContext<'_>,
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
                        Some(format_value(&child, page.node.flags, context)?.to_lowercase())
                    };
                    for query in matching_queries {
                        let is_match = if let Some(formatted) = &formatted {
                            formatted.contains(&query.value)
                        } else {
                            array_matches(read, &child_path, &query.value, cancelled, context)?
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
    context: &ResourceContext<'_>,
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
            if format_value(&child, page.node.flags, context)?
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

pub(crate) fn format_value(
    node: &StructuredNode,
    _parent_flags: u32,
    context: &ResourceContext<'_>,
) -> Result<String, String> {
    if node.basic_type == 12 {
        return resource_name(node, context);
    }
    if node.basic_type == 13 {
        let value = node
            .value
            .as_deref()
            .ok_or("Missing GPU address")?
            .parse::<u64>()
            .map_err(|_| "Invalid GPU address")?;
        return format_gpu_address(value, context);
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
            // RenderDocX currently uses RenderDoc's default Auto offset/size mode.
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
        9 => format_float(&scalar()?),
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

/// RenderDoc's default Formatter configuration: 2–5 decimal places and
/// scientific notation outside [1e-5, 1e7].
fn format_float(value: &str) -> Result<String, String> {
    let value = value
        .parse::<f64>()
        .map_err(|_| "Invalid floating-point parameter")?;
    if value.is_nan() {
        return Ok("nan".into());
    }
    if value.is_infinite() {
        return Ok(if value.is_sign_negative() {
            "-inf"
        } else {
            "inf"
        }
        .into());
    }
    if value == 0.0 {
        return Ok("0.00".into());
    }
    if value.abs() < 0.00001 || value.abs() > 10_000_000.0 {
        let scientific = format!("{value:.5E}");
        let (mantissa, exponent) = scientific
            .split_once('E')
            .ok_or("Invalid formatted exponent")?;
        let exponent = exponent
            .parse::<i32>()
            .map_err(|_| "Invalid formatted exponent")?;
        return Ok(format!(
            "{mantissa}E{}{abs:02}",
            if exponent < 0 { '-' } else { '+' },
            abs = exponent.unsigned_abs()
        ));
    }
    let mut result = format!("{value:.5}");
    let min_length = result.find('.').ok_or("Invalid formatted float")? + 3;
    while result.len() > min_length && result.ends_with('0') {
        result.pop();
    }
    Ok(result)
}

fn resource_name(node: &StructuredNode, context: &ResourceContext<'_>) -> Result<String, String> {
    let id = node
        .resource_id
        .as_deref()
        .ok_or("Resource identity is unavailable in this bridge")?
        .parse::<u64>()
        .map_err(|_| "Invalid resource identity")?;
    if node.resource_non_null != Some(id != 0) {
        return Err("Inconsistent resource identity".into());
    }
    if id == 0 {
        return Ok(context.language.no_resource().into());
    }
    let edited = node
        .resource_edited
        .ok_or("Resource replacement state is unavailable in this bridge")?;
    let mut name = if let Some(index) = node.resource_index {
        let resource = context
            .resources
            .get(index)
            .ok_or("Invalid resource index")?;
        let key = format!("ResourceId::{id}");
        if resource.rename_key.as_deref() != Some(&key) {
            return Err("Resource identity changed during lookup".into());
        }
        let document = context.renames.as_ref().map_err(Clone::clone)?;
        crate::annotations::resource_name(document, &key)?
            .unwrap_or(&resource.name)
            .to_owned()
    } else {
        format!("{} {id}", context.language.unknown_resource())
    };
    if edited {
        name.push_str(context.language.edited_resource_suffix());
    }
    if name.encode_utf16().count() > 64 {
        let mut length = 0;
        let mut end = 0;
        for (index, grapheme) in name.grapheme_indices(true) {
            let next = length + grapheme.encode_utf16().count();
            if next > 64 {
                break;
            }
            length = next;
            end = index + grapheme.len();
        }
        name.truncate(end);
        name.push_str("...");
    }
    Ok(name)
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
            resource_id: None,
            resource_edited: None,
        }
    }

    #[test]
    fn gpu_addresses_resolve_first_matching_buffer_and_fall_back_safely() {
        let resources = vec![ResourceInfo {
            name: "original".into(),
            rename_key: Some("ResourceId::7".into()),
            resource_type: 0,
            autogenerated_name: false,
            initialisation_chunks: vec![],
            parents: vec![],
            derived: vec![],
        }];
        let buffers = vec![
            Buffer {
                name: "buffer".into(),
                resource_index: Some(0),
                length: "16".into(),
                gpu_address: Some("4096".into()),
                creation_flags: 0,
            },
            Buffer {
                name: "overlap".into(),
                resource_index: None,
                length: "16".into(),
                gpu_address: Some("4098".into()),
                creation_flags: 0,
            },
            Buffer {
                name: "high".into(),
                resource_index: Some(0),
                length: "8".into(),
                gpu_address: Some((u64::MAX - 3).to_string()),
                creation_flags: 0,
            },
        ];
        let context = ResourceContext {
            resources: &resources,
            buffers: &buffers,
            renames: Ok(serde_json::json!({"CustomResourceNames":{"ResourceId::7":"renamed"}})),
            language: Language::English,
        };
        assert_eq!(format_gpu_address(4096, &context).unwrap(), "renamed+0");
        assert_eq!(format_gpu_address(4100, &context).unwrap(), "renamed+4");
        assert_eq!(format_gpu_address(4112, &context).unwrap(), "0x1010");
        assert_eq!(format_gpu_address(u64::MAX, &context).unwrap(), "renamed+3");
        assert_eq!(format_gpu_address(0, &context).unwrap(), "0x0");
        let value = node("address", 13, Some("4100"), 0);
        assert_eq!(format_value(&value, 0, &context).unwrap(), "renamed+4");
        let old: Buffer = serde_json::from_value(
            serde_json::json!({"name":"old","resource_index":0,"length":"4","creation_flags":0}),
        )
        .unwrap();
        assert!(old.gpu_address.is_none());
        let old_buffers = [old];
        let old_context = ResourceContext {
            resources: &resources,
            buffers: &old_buffers,
            renames: Ok(serde_json::json!({})),
            language: Language::English,
        };
        assert_eq!(format_gpu_address(4100, &old_context).unwrap(), "0x1004");
        let mixed = [old_buffers[0].clone(), buffers[0].clone()];
        let mixed_context = ResourceContext {
            resources: &resources,
            buffers: &mixed,
            renames: Ok(serde_json::json!({})),
            language: Language::English,
        };
        assert_eq!(format_gpu_address(4100, &mixed_context).unwrap(), "0x1004");
    }

    #[test]
    #[cfg(feature = "ui-tests")]
    fn export_completes_uncached_names_before_writing() {
        let (session, _, _) = crate::replay::test_support::recording_remote_session();
        let mut rows = vec![crate::event_tree::ExportRow {
            event_id: 41,
            action_id: None,
            action_range: None,
            name: "Call()".into(),
            depth: 0,
            first_child: false,
            duration: None,
        }];
        format_export_rows(
            &session,
            &mut rows,
            &[41],
            &[],
            &[],
            Language::English,
            false,
            true,
        )
        .unwrap();
        assert!(
            rows[0]
                .name
                .starts_with("Call(Topology=4, Upload=(300 bytes)")
        );
        assert!(rows[0].name.ends_with("Param129=1)"));
    }

    #[test]
    fn formats_calls_with_upstream_visibility_and_nested_rules() {
        let mut root = node("ID3D11Device::Draw", 0, None, 4);
        root.flags = 0x80;
        let mut count = node("count", 7, Some("7"), 0);
        count.flags = 0x40;
        let mut hidden = node("hidden", 7, Some("9"), 0);
        hidden.flags = 0x42;
        let mut nested = node("nested", 1, None, 2);
        nested.flags = 0xc0;
        let array = node("data", 2, None, 5);
        let mut value = node("value", 7, Some("3"), 0);
        value.flags = 0x40;
        let other = node("ignored", 7, Some("4"), 0);
        let root_children = vec![count, hidden, nested.clone(), array];
        let nested_children = vec![value, other];
        let mut read = |path: &[u64], offset: u64| {
            let (parent, children) = match path {
                [] => (&root, &root_children),
                [2] => (&nested, &nested_children),
                _ => panic!("unexpected path"),
            };
            Ok(StructurePage {
                event_id: 1,
                path: path.iter().map(u64::to_string).collect(),
                offset: offset.to_string(),
                node: parent.clone(),
                children: children
                    .iter()
                    .skip(offset as usize)
                    .take(STRUCTURE_PAGE_LIMIT as usize)
                    .cloned()
                    .collect(),
                chunk_metadata: None,
            })
        };
        let context = ResourceContext {
            resources: &[],
            buffers: &[],
            renames: Ok(serde_json::json!({})),
            language: Language::English,
        };
        assert_eq!(
            format_event_with(&mut read, &context, false, false).unwrap(),
            "Draw(7, 3)"
        );
        assert_eq!(
            format_event_with(&mut read, &context, true, true).unwrap(),
            "Draw(count=7, nested=3, data=T[5])"
        );
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
        let context = ResourceContext {
            resources: &[],
            buffers: &[],
            renames: Ok(serde_json::json!({})),
            language: Language::English,
        };
        let matched = matches_parameters(&mut read, &queries, &|| false, &context).unwrap();
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
        let context = ResourceContext {
            resources: &[],
            buffers: &[],
            renames: Ok(serde_json::json!({})),
            language: Language::English,
        };
        assert!(matches_parameters(&mut read, &queries, &|| true, &context).is_err());
        assert!(matches_parameters(&mut read, &queries, &|| false, &context).is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn resource_format_uses_renames_replacement_state_and_utf16_grapheme_boundaries() {
        let resource = ResourceInfo {
            name: "original".into(),
            rename_key: Some("ResourceId::7".into()),
            resource_type: 0,
            autogenerated_name: false,
            initialisation_chunks: vec![],
            parents: vec![],
            derived: vec![],
        };
        let resources = vec![resource];
        let context = ResourceContext {
            resources: &resources,
            buffers: &[],
            renames: Ok(serde_json::json!({"CustomResourceNames":{"ResourceId::7":"renamed"}})),
            language: Language::English,
        };
        let mut value = node("r", 12, None, 0);
        value.resource_id = Some("7".into());
        value.resource_index = Some(0);
        value.resource_non_null = Some(true);
        value.resource_edited = Some(true);
        assert_eq!(resource_name(&value, &context).unwrap(), "renamed (Edited)");
        value.resource_edited = None;
        assert!(resource_name(&value, &context).is_err());
        value.resource_id = Some("0".into());
        value.resource_non_null = Some(false);
        value.resource_index = None;
        assert_eq!(resource_name(&value, &context).unwrap(), "No Resource");
        let long_name = format!("{}👩‍💻tail", "a".repeat(63));
        let context = ResourceContext {
            resources: &resources,
            buffers: &[],
            renames: Ok(serde_json::json!({"CustomResourceNames":{"ResourceId::7":long_name}})),
            language: Language::English,
        };
        value.resource_id = Some("7".into());
        value.resource_non_null = Some(true);
        value.resource_index = Some(0);
        value.resource_edited = Some(false);
        assert_eq!(
            resource_name(&value, &context).unwrap(),
            format!("{}...", "a".repeat(63))
        );
    }

    #[test]
    fn default_numeric_format_matches_renderdoc_thresholds_and_offsets() {
        let context = ResourceContext {
            resources: &[],
            buffers: &[],
            renames: Ok(serde_json::json!({})),
            language: Language::English,
        };
        for (input, expected) in [
            ("0", "0.00"),
            ("0.25", "0.25"),
            ("1", "1.00"),
            ("0.00001", "0.00001"),
            ("0.000001", "1.00000E-06"),
            ("10000000", "10000000.00"),
            ("10000001", "1.00000E+07"),
            ("-0.125", "-0.125"),
            ("inf", "inf"),
            ("-inf", "-inf"),
            ("nan", "nan"),
        ] {
            assert_eq!(format_float(input).unwrap(), expected, "{input}");
        }
        let mut offset = node("offset", 7, Some("16777216"), 0);
        offset.flags = 0x200;
        assert_eq!(format_value(&offset, 0, &context).unwrap(), "0x01000000");
        offset.value = Some("65535".into());
        assert_eq!(format_value(&offset, 0, &context).unwrap(), "UINT16_MAX");
        offset.value = Some("123".into());
        assert_eq!(format_value(&offset, 0, &context).unwrap(), "123");
    }
}
