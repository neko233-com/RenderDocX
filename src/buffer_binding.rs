//! Resolve a descriptor's byte range against the captured buffer resource.
use crate::replay::Buffer;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferBindingRange {
    pub index: usize,
    pub start: u64,
    pub end: u64,
}

/// Parse an absolute byte offset, accepting decimal or a 0x-prefixed hexadecimal value.
pub fn parse_offset(value: &str) -> Option<u64> {
    let value = value.trim();
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u64::from_str_radix(hex, 16).ok()
    } else {
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        value.parse().ok()
    }
}

pub fn resolve(descriptor: &Value, buffers: &[Buffer]) -> Result<BufferBindingRange, String> {
    let resource = descriptor
        .get("resource")
        .and_then(Value::as_u64)
        .and_then(|v| usize::try_from(v).ok())
        .ok_or("Descriptor has no mapped resource")?;
    let (index, buffer) = buffers
        .iter()
        .enumerate()
        .find(|(_, buffer)| buffer.resource_index == Some(resource))
        .ok_or("Descriptor resource is not a captured buffer")?;
    let total = buffer
        .length
        .parse::<u64>()
        .map_err(|_| "Invalid captured buffer size")?;
    let start = descriptor
        .get("byte_offset")
        .and_then(Value::as_str)
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or("Invalid descriptor buffer offset")?;
    let size = descriptor
        .get("byte_size")
        .and_then(Value::as_str)
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or("Invalid descriptor buffer size")?;
    if start > total {
        return Err("Descriptor buffer offset is out of range".into());
    }
    Ok(BufferBindingRange {
        index,
        start,
        end: start.saturating_add(size).min(total),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_decimal_and_hex_offsets_without_overflow() {
        assert_eq!(parse_offset(" 255 "), Some(255));
        assert_eq!(parse_offset("0xFf"), Some(255));
        assert_eq!(parse_offset("0Xffffffffffffffff"), Some(u64::MAX));
        for value in [
            "",
            "0x",
            "-1",
            "+1",
            "0x-1",
            "1 2",
            "18446744073709551616",
            "0x10000000000000000",
        ] {
            assert_eq!(parse_offset(value), None, "{value}");
        }
    }

    #[test]
    fn clips_unbounded_sizes_and_rejects_invalid_offsets() {
        let buffers = [Buffer {
            name: "buffer".into(),
            resource_index: Some(7),
            length: "300".into(),
            creation_flags: 0,
        }];
        let mut value =
            serde_json::json!({"resource":7,"byte_offset":"20","byte_size":"18446744073709551615"});
        assert_eq!(
            resolve(&value, &buffers).unwrap(),
            BufferBindingRange {
                index: 0,
                start: 20,
                end: 300
            }
        );
        value["byte_size"] = "12".into();
        assert_eq!(resolve(&value, &buffers).unwrap().end, 32);
        value["byte_offset"] = "301".into();
        assert!(resolve(&value, &buffers).is_err());
    }
}
