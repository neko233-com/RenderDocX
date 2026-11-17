//! Resolve RenderDoc source mappings against the current debugger state.
use crate::replay::{
    DebugState, DebugTrace, DebugVariable, DebugVariableReference, SourceVariableMapping,
};
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentStatus {
    Value,
    Undefined,
    MissingReference,
    MissingVariable,
    Composite,
    UnsupportedType,
    InvalidData,
}

#[derive(Debug, Serialize)]
pub struct SourceComponent {
    pub status: ComponentStatus,
    /// Decimal representation, interpreted using the mapping's source type.
    pub value: Option<String>,
    /// Native-endian bytes used for this component.
    pub raw_hex: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ResolvedSourceVariable<'a> {
    pub scope: &'static str,
    pub mapping: &'a SourceVariableMapping,
    pub components: Vec<SourceComponent>,
}

// Match ShaderViewer's path lookup, including exact names containing dots or brackets.
fn find_variable<'a>(path: &str, variables: &'a [DebugVariable]) -> Option<&'a DebugVariable> {
    if path.is_empty() {
        return None;
    }
    let (base, rest) = if path.starts_with('[') {
        let end = path.find(']')? + 1;
        (
            &path[..end],
            path[end..].strip_prefix('.').unwrap_or(&path[end..]),
        )
    } else if let Some(end) = path.find(['[', '.']) {
        (
            &path[..end],
            path[end..].strip_prefix('.').unwrap_or(&path[end..]),
        )
    } else {
        (path, "")
    };
    for variable in variables {
        if variable.name == path {
            return Some(variable);
        }
        if variable.name == base {
            return if rest.is_empty() {
                Some(variable)
            } else {
                find_variable(rest, &variable.members)
            };
        }
    }
    None
}

fn lookup<'a>(
    reference: &DebugVariableReference,
    trace: &'a DebugTrace,
    mutable: &'a [DebugVariable],
) -> Option<&'a DebugVariable> {
    let variables = match reference.variable_type {
        1 => &trace.inputs,
        2 => &trace.constant_blocks,
        3 => &trace.samplers,
        4 => &trace.read_only_resources,
        5 => &trace.read_write_resources,
        6 => mutable,
        _ => return None,
    };
    find_variable(&reference.name, variables)
}

fn unavailable(status: ComponentStatus) -> SourceComponent {
    SourceComponent {
        status,
        value: None,
        raw_hex: None,
    }
}

fn component(
    mapping: &SourceVariableMapping,
    reference: &DebugVariableReference,
    trace: &DebugTrace,
    mutable: &[DebugVariable],
) -> SourceComponent {
    if mapping.undefined {
        return unavailable(ComponentStatus::Undefined);
    }
    if reference.name.is_empty() {
        return unavailable(ComponentStatus::MissingReference);
    }
    let Some(variable) = lookup(reference, trace, mutable) else {
        return unavailable(ComponentStatus::MissingVariable);
    };
    if !variable.members.is_empty() {
        return unavailable(ComponentStatus::Composite);
    }
    // ShaderValue is a union; ShaderViewer interprets the register using the source mapping type.
    // Opaque resources and GPU pointers require upstream descriptor/pointer decoding.
    let width = match mapping.variable_type {
        0 | 3 | 4 | 11 => 4,
        1 | 7 | 8 => 8,
        2 | 5 | 6 => 2,
        9 | 10 => 1,
        _ => return unavailable(ComponentStatus::UnsupportedType),
    };
    let Some(start) = (reference.component as usize).checked_mul(width * 2) else {
        return unavailable(ComponentStatus::InvalidData);
    };
    let Some(raw) = variable
        .value_hex
        .get(start..start.saturating_add(width * 2))
    else {
        return unavailable(ComponentStatus::InvalidData);
    };
    let Some(bytes) = raw
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|s| u8::from_str_radix(s, 16).ok())
        })
        .collect::<Option<Vec<_>>>()
    else {
        return unavailable(ComponentStatus::InvalidData);
    };
    let value = match mapping.variable_type {
        0 => f32::from_ne_bytes(bytes[..].try_into().unwrap()).to_string(),
        1 => f64::from_ne_bytes(bytes[..].try_into().unwrap()).to_string(),
        2 => half::f16::from_bits(u16::from_ne_bytes(bytes[..].try_into().unwrap()))
            .to_f32()
            .to_string(),
        3 => i32::from_ne_bytes(bytes[..].try_into().unwrap()).to_string(),
        4 => u32::from_ne_bytes(bytes[..].try_into().unwrap()).to_string(),
        5 => i16::from_ne_bytes(bytes[..].try_into().unwrap()).to_string(),
        6 => u16::from_ne_bytes(bytes[..].try_into().unwrap()).to_string(),
        7 => i64::from_ne_bytes(bytes[..].try_into().unwrap()).to_string(),
        8 => u64::from_ne_bytes(bytes[..].try_into().unwrap()).to_string(),
        9 => (bytes[0] as i8).to_string(),
        10 => bytes[0].to_string(),
        11 => (u32::from_ne_bytes(bytes[..].try_into().unwrap()) != 0).to_string(),
        _ => unreachable!(),
    };
    SourceComponent {
        status: ComponentStatus::Value,
        value: Some(value),
        raw_hex: Some(raw.to_owned()),
    }
}

/// Return all trace-wide and instruction-scoped mappings without inventing values for missing data.
pub fn resolve<'a>(
    trace: &'a DebugTrace,
    state: Option<&DebugState>,
    mutable: &[DebugVariable],
) -> Vec<ResolvedSourceVariable<'a>> {
    let Some(state) = state else {
        return Vec::new();
    };
    let locals = trace
        .instruction_info(state.next_instruction)
        .map(|info| info.source_variables.as_slice())
        .unwrap_or(&[]);
    trace
        .source_variables
        .iter()
        .map(|mapping| ("trace", mapping))
        .chain(locals.iter().map(|mapping| ("instruction", mapping)))
        .map(|(scope, mapping)| ResolvedSourceVariable {
            scope,
            mapping,
            components: mapping
                .variables
                .iter()
                .map(|reference| component(mapping, reference, trace, mutable))
                .collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::debug_navigation::DebugNavigator;
    use serde_json::json;

    fn variable(name: &str, hex: &str, members: Vec<DebugVariable>) -> DebugVariable {
        DebugVariable {
            name: name.into(),
            rows: 1,
            columns: 4,
            variable_type: 4,
            flags: 0,
            value_hex: hex.into(),
            members,
        }
    }
    fn mapping(name: &str, ty: u32, refs: &[(&str, u32, u32)]) -> SourceVariableMapping {
        SourceVariableMapping {
            name: name.into(),
            variable_type: ty,
            undefined: false,
            rows: 1,
            columns: refs.len() as u32,
            offset: 0,
            signature_index: -1,
            variables: refs
                .iter()
                .map(|(name, variable_type, component)| DebugVariableReference {
                    name: (*name).into(),
                    variable_type: *variable_type,
                    component: *component,
                })
                .collect(),
        }
    }
    fn trace() -> DebugTrace {
        serde_json::from_value(json!({"event_id":1,"stage":5,"inputs":[],"constant_blocks":[],"read_only_resources":[],"read_write_resources":[],"samplers":[],"source_variables":[],"instructions":[]})).unwrap()
    }
    fn state(step: u32, changes: Vec<crate::replay::DebugChange>) -> DebugState {
        DebugState {
            step,
            next_instruction: 0,
            flags: 0,
            callstack: vec![],
            changes,
        }
    }

    #[test]
    fn maps_input_and_mutable_components_across_forward_and_reverse_steps() {
        let mut trace = trace();
        trace
            .inputs
            .push(variable("input", "010000000200000003000000", vec![]));
        trace
            .source_variables
            .push(mapping("global", 4, &[("input", 1, 2)]));
        trace.instructions = serde_json::from_value(json!([{"instruction":0,"disassembly_line":1,"file_index":0,"line_start":1,"line_end":1,"column_start":0,"column_end":0,"source_variables":[mapping("local", 4, &[("reg", 6, 0)])]}])).unwrap();
        let empty = variable("", "", vec![]);
        let one = variable("reg", "01000000", vec![]);
        let two = variable("reg", "02000000", vec![]);
        let states = vec![
            state(
                0,
                vec![crate::replay::DebugChange {
                    before: empty,
                    after: one.clone(),
                }],
            ),
            state(
                1,
                vec![crate::replay::DebugChange {
                    before: one,
                    after: two,
                }],
            ),
        ];
        let mut nav = DebugNavigator::default();
        for (position, expected) in [(0, "1"), (1, "2"), (0, "1")] {
            nav.position = position;
            let mutable = nav.variables(&states);
            let values = resolve(&trace, states.get(position), &mutable);
            assert_eq!(values[0].components[0].value.as_deref(), Some("3"));
            assert_eq!(values[1].components[0].value.as_deref(), Some(expected));
        }
        assert!(resolve(&trace, None, &[]).is_empty());
    }

    #[test]
    fn resolves_nested_names_and_reports_undefined_missing_and_opaque_data() {
        let mut trace = trace();
        trace.constant_blocks.push(variable(
            "cb",
            "",
            vec![variable(
                "[0]",
                "",
                vec![variable("member", "0000c03f", vec![])],
            )],
        ));
        let mut undefined = mapping("undefined", 0, &[("cb[0].member", 2, 0)]);
        undefined.undefined = true;
        trace.source_variables = vec![
            mapping("nested", 0, &[("cb[0].member", 2, 0)]),
            undefined,
            mapping("missing", 4, &[("", 1, 0), ("no", 1, 0), ("cb", 2, 0)]),
            mapping("opaque", 14, &[("cb[0].member", 2, 0)]),
            mapping("short", 4, &[("cb[0].member", 2, u32::MAX)]),
        ];
        let values = resolve(&trace, Some(&state(0, vec![])), &[]);
        assert_eq!(values[0].components[0].value.as_deref(), Some("1.5"));
        assert!(matches!(
            values[1].components[0].status,
            ComponentStatus::Undefined
        ));
        assert!(matches!(
            values[2].components[0].status,
            ComponentStatus::MissingReference
        ));
        assert!(matches!(
            values[2].components[1].status,
            ComponentStatus::MissingVariable
        ));
        assert!(matches!(
            values[2].components[2].status,
            ComponentStatus::Composite
        ));
        assert!(matches!(
            values[3].components[0].status,
            ComponentStatus::UnsupportedType
        ));
        assert!(matches!(
            values[4].components[0].status,
            ComponentStatus::InvalidData
        ));
    }

    #[test]
    fn uses_mapping_type_and_preserves_raw_bytes() {
        let mut trace = trace();
        trace.inputs.push(variable("r", "00bc000000000080", vec![]));
        trace.source_variables = vec![
            mapping("half", 2, &[("r", 1, 0)]),
            mapping("signed", 3, &[("r", 1, 1)]),
            mapping("bool", 11, &[("r", 1, 0)]),
        ];
        let values = resolve(&trace, Some(&state(0, vec![])), &[]);
        assert_eq!(values[0].components[0].value.as_deref(), Some("-1"));
        assert_eq!(values[0].components[0].raw_hex.as_deref(), Some("00bc"));
        assert_eq!(
            values[1].components[0].value.as_deref(),
            Some("-2147483648")
        );
        assert_eq!(values[2].components[0].value.as_deref(), Some("true"));
    }
}
