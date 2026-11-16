use renderdocx::replay::{DebugVariable, SourceVariableMapping};

pub fn line_at(text: &str, line: u32) -> Option<&str> {
    let index = usize::try_from(line.checked_sub(1)?).ok()?;
    text.lines().nth(index)
}

pub fn display_source_mapping(mapping: &SourceVariableMapping) -> String {
    let references = mapping
        .variables
        .iter()
        .map(|reference| {
            format!(
                "{}[{}] #{}",
                reference.name, reference.component, reference.variable_type
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{} [{}×{}, #{}] → {}{}",
        mapping.name,
        mapping.rows,
        mapping.columns,
        mapping.variable_type,
        references,
        if mapping.undefined { " (?)" } else { "" }
    )
}

/// Interpret only types whose layout is defined by RenderDoc's ShaderValue.
/// Unknown types and half-floats remain in hexadecimal rather than being guessed.
pub fn display_variable(variable: &DebugVariable) -> String {
    let bytes = variable.value_hex.as_bytes();
    let decoded: Option<Vec<u8>> = if bytes.len() % 2 == 0 {
        bytes
            .chunks_exact(2)
            .map(|pair| {
                std::str::from_utf8(pair)
                    .ok()
                    .and_then(|s| u8::from_str_radix(s, 16).ok())
            })
            .collect()
    } else {
        None
    };
    let count = usize::from(variable.rows) * usize::from(variable.columns);
    let value = if !variable.members.is_empty() {
        variable
            .members
            .iter()
            .map(display_variable)
            .collect::<Vec<_>>()
            .join(", ")
    } else if count == 0 {
        "—".to_owned()
    } else if let Some(bytes) = decoded {
        let width = match variable.variable_type {
            0 | 3 | 4 | 11 | 12 | 16..=18 => 4,
            1 | 7 | 8 | 14 => 8,
            2 | 5 | 6 => 2,
            9 | 10 => 1,
            _ => 0,
        };
        if width == 0 || count > 16 || bytes.len() < count * width {
            format!("0x{}", variable.value_hex)
        } else {
            bytes
                .chunks_exact(width)
                .take(count)
                .map(|part| match variable.variable_type {
                    0 => f32::from_ne_bytes(part.try_into().unwrap()).to_string(),
                    1 => f64::from_ne_bytes(part.try_into().unwrap()).to_string(),
                    3 => i32::from_ne_bytes(part.try_into().unwrap()).to_string(),
                    4 | 11 | 12 | 16..=18 => {
                        u32::from_ne_bytes(part.try_into().unwrap()).to_string()
                    }
                    5 => i16::from_ne_bytes(part.try_into().unwrap()).to_string(),
                    6 => u16::from_ne_bytes(part.try_into().unwrap()).to_string(),
                    7 => i64::from_ne_bytes(part.try_into().unwrap()).to_string(),
                    8 => u64::from_ne_bytes(part.try_into().unwrap()).to_string(),
                    9 => (part[0] as i8).to_string(),
                    10 => part[0].to_string(),
                    _ => format!(
                        "0x{}",
                        part.iter().map(|b| format!("{b:02x}")).collect::<String>()
                    ),
                })
                .collect::<Vec<_>>()
                .join(", ")
        }
    } else {
        format!("0x{}", variable.value_hex)
    };
    format!(
        "{} [{}×{}, #{}, 0x{:x}]: {value}",
        variable.name, variable.rows, variable.columns, variable.variable_type, variable.flags
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_native_float_and_keeps_unknown_bytes() {
        let make = |variable_type, value_hex: String| DebugVariable {
            name: "v".into(),
            rows: 1,
            columns: 1,
            variable_type,
            flags: 0,
            value_hex,
            members: vec![],
        };
        let raw = 1.5_f32
            .to_ne_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert!(display_variable(&make(0, raw)).ends_with(": 1.5"));
        let signed = (-7_i32)
            .to_ne_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert!(display_variable(&make(3, signed)).ends_with(": -7"));
        assert!(display_variable(&make(255, "1234".into())).ends_with(": 0x1234"));
        assert!(display_variable(&make(0, "z".into())).ends_with(": 0xz"));
    }

    #[test]
    fn source_lines_use_one_based_indices() {
        assert_eq!(line_at("alpha\r\nbeta\n", 1), Some("alpha"));
        assert_eq!(line_at("alpha\r\nbeta\n", 2), Some("beta"));
        assert_eq!(line_at("alpha", 0), None);
        assert_eq!(line_at("alpha", 2), None);
    }
}
