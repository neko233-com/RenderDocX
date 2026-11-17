//! Resolve capture-time GPU virtual addresses against RenderDoc buffer ranges.
use crate::replay::Buffer;

/// Uses the first matching buffer in capture order. Missing legacy base fields
/// prevent a later match because an earlier buffer could have contained it.
pub fn resolve(buffers: &[Buffer], address: u64) -> Result<Option<(usize, u64)>, &'static str> {
    for (index, buffer) in buffers.iter().enumerate() {
        let Some(base) = buffer.gpu_address.as_deref() else {
            return Ok(None);
        };
        let base = base
            .parse::<u64>()
            .map_err(|_| "Invalid buffer GPU address")?;
        if base == 0 || address < base {
            continue;
        }
        let length = buffer
            .length
            .parse::<u64>()
            .map_err(|_| "Invalid buffer length")?;
        let offset = address - base;
        if offset < length {
            return Ok(Some((index, offset)));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn buffer(base: Option<&str>, length: &str) -> Buffer {
        Buffer {
            name: String::new(),
            resource_index: None,
            length: length.into(),
            gpu_address: base.map(str::to_owned),
            creation_flags: 0,
        }
    }
    #[test]
    fn resolves_first_match_without_address_overflow() {
        let buffers = [
            buffer(Some("4096"), "16"),
            buffer(Some("4098"), "16"),
            buffer(Some("18446744073709551612"), "8"),
        ];
        assert_eq!(resolve(&buffers, 4100), Ok(Some((0, 4))));
        assert_eq!(resolve(&buffers, 4112), Ok(Some((1, 14))));
        assert_eq!(resolve(&buffers, u64::MAX), Ok(Some((2, 3))));
        assert_eq!(resolve(&buffers, 0), Ok(None));
    }
    #[test]
    fn incomplete_or_invalid_earlier_buffers_do_not_create_false_links() {
        assert_eq!(
            resolve(&[buffer(None, "5"), buffer(Some("5"), "10")], 6),
            Ok(None)
        );
        assert!(resolve(&[buffer(Some("bad"), "5"), buffer(Some("5"), "10")], 6).is_err());
        assert!(resolve(&[buffer(Some("5"), "bad")], 6).is_err());
        assert_eq!(resolve(&[buffer(Some("5"), "1")], 6), Ok(None));
    }
}
