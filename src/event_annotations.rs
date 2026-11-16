//! RenderDoc API-event annotations, distinct from bookmarks and notes.
use crate::event_filter::{AnnotationCondition, AnnotationMatches, AnnotationQuery, Filter};
use crate::i18n::Language;
use crate::replay::{Action, ResourceInfo, Session, StructuredNode};
use std::collections::{HashMap, HashSet};

/// Resolve one annotation column completely before exposing it to the caller.
pub fn display_column(
    session: &Session,
    actions: &[Action],
    resources: &[ResourceInfo],
    language: Language,
    key: &str,
    cancelled: impl Fn() -> bool,
) -> Result<HashMap<u32, String>, String> {
    if key.is_empty() {
        return Ok(HashMap::new());
    }
    let context = crate::event_parameters::ResourceContext {
        resources,
        language,
        renames: session
            .annotation_read("renames")
            .and_then(|section| section.document()),
    };
    let events: HashSet<u32> = actions
        .iter()
        .flat_map(|action| action.api_events.iter())
        .map(|event| event.event_id)
        .collect();
    let mut result = HashMap::new();
    let keys = [key.to_owned()];
    for event in events {
        if cancelled() {
            return Err("Event annotation lookup cancelled".into());
        }
        let node = session
            .annotation_find(event, &keys)?
            .into_iter()
            .next()
            .flatten();
        if let Some(node) = node {
            let text = if matches!(node.basic_type, 0..=2) {
                "{...}".to_owned()
            } else {
                crate::event_parameters::format_value(&node, 0, &context)?
            };
            result.insert(event, text);
        }
    }
    Ok(result)
}

pub fn resolve(
    session: &Session,
    actions: &[Action],
    expression: &str,
    cancelled: impl Fn() -> bool,
) -> Result<AnnotationMatches, String> {
    let queries = Filter::parse(expression)
        .map_err(|()| "Invalid event filter")?
        .annotation_queries();
    let mut matches: AnnotationMatches = queries
        .iter()
        .cloned()
        .map(|query| (query, HashSet::new()))
        .collect();
    if queries.is_empty() {
        return Ok(matches);
    }
    let mut by_key: HashMap<String, Vec<AnnotationQuery>> = HashMap::new();
    for query in queries {
        by_key.entry(query.key.clone()).or_default().push(query);
    }
    let mut keys: Vec<_> = by_key.keys().cloned().collect();
    keys.sort();
    let events: HashSet<u32> = actions
        .iter()
        .flat_map(|action| action.api_events.iter())
        .map(|event| event.event_id)
        .collect();
    for event in events {
        for group in keys.chunks(128) {
            if cancelled() {
                return Err("Event annotation lookup cancelled".into());
            }
            let values = session.annotation_find(event, group)?;
            for (key, value) in group.iter().zip(values) {
                let Some(node) = value else {
                    continue;
                };
                for query in &by_key[key] {
                    if matches_annotation(&node, &query.condition)? {
                        matches
                            .get_mut(query)
                            .expect("query inserted")
                            .insert(event);
                    }
                }
            }
        }
    }
    Ok(matches)
}

fn compare<T: PartialOrd + PartialEq>(a: T, b: T, op: &str) -> bool {
    match op {
        "=" | "==" => a == b,
        "!=" => a != b,
        "<" => a < b,
        ">" => a > b,
        "<=" => a <= b,
        ">=" => a >= b,
        _ => false,
    }
}

fn matches_annotation(
    node: &StructuredNode,
    condition: &AnnotationCondition,
) -> Result<bool, String> {
    if matches!(condition, AnnotationCondition::Exists) {
        return Ok(true);
    }
    let value = node.value.as_deref().ok_or("Missing annotation value");
    match condition {
        AnnotationCondition::Exists => Ok(true),
        AnnotationCondition::Contains(needle) if node.basic_type == 5 => {
            Ok(value?.to_lowercase().contains(needle))
        }
        AnnotationCondition::Regex(pattern) if node.basic_type == 5 => {
            Ok(crate::event_filter::parse_regex(pattern)
                .map_err(|()| "Invalid annotation regex")?
                .is_match(value?.as_bytes())
                .map_err(|e| e.to_string())?)
        }
        AnnotationCondition::Compare(op, expected) => match node.basic_type {
            5 if op == "=" || op == "==" => Ok(value? == expected),
            6 | 7 | 10 | 11 => {
                let actual = match value? {
                    "true" => 1,
                    "false" => 0,
                    text => text
                        .parse::<u64>()
                        .map_err(|_| "Invalid unsigned annotation")?,
                };
                Ok(compare(actual, expected.parse::<u64>().unwrap_or(0), op))
            }
            8 => Ok(compare(
                value?
                    .parse::<i64>()
                    .map_err(|_| "Invalid signed annotation")?,
                expected.parse::<i64>().unwrap_or(0),
                op,
            )),
            9 => Ok(compare(
                value?
                    .parse::<f64>()
                    .map_err(|_| "Invalid floating-point annotation")?,
                expected.parse::<f64>().unwrap_or(0.0),
                op,
            )),
            _ => Ok(false),
        },
        _ => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_filter::AnnotationCondition::*;
    fn node(kind: u32, value: &str) -> StructuredNode {
        StructuredNode {
            name: "k".into(),
            type_name: "t".into(),
            basic_type: kind,
            flags: 0,
            byte_size: "0".into(),
            child_count: "0".into(),
            value: Some(value.into()),
            custom_string: None,
            resource_non_null: None,
            resource_index: None,
            resource_id: None,
            resource_edited: None,
        }
    }
    #[test]
    fn comparisons_preserve_types_and_upstream_string_rules() {
        assert!(matches_annotation(&node(5, "Render Pass"), &Contains("pass".into())).unwrap());
        assert!(matches_annotation(&node(5, "Render Pass"), &Regex("/Pass/".into())).unwrap());
        assert!(
            !matches_annotation(
                &node(5, "Render Pass"),
                &Compare("!=".into(), "other".into())
            )
            .unwrap()
        );
        assert!(matches_annotation(&node(8, "-2"), &Compare("<".into(), "1".into())).unwrap());
        assert!(matches_annotation(&node(9, "0.5"), &Compare(">=".into(), "0.5".into())).unwrap());
        assert!(!matches_annotation(&node(7, "5"), &Contains("5".into())).unwrap());
    }
}
