//! RenderDoc's resource and event references in diagnostic messages.
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Resource(usize),
    Event(u32),
    ShaderMessages {
        event: u32,
    },
    Buffer {
        index: usize,
        offset: u64,
        pointer_type: Option<u32>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub range: Range<usize>,
    pub target: Target,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    pub links: Vec<Link>,
}

/// Unknown references are kept verbatim. HTML rendering is not interpreted.
pub fn render(
    input: &str,
    resource: impl Fn(u64) -> Option<(usize, String)>,
    event: impl Fn(u32) -> bool,
    gpu_address: impl Fn(u64) -> Option<(usize, u64, String)>,
    shader_messages: impl Fn(u32, u32) -> Option<String>,
) -> Rendered {
    let mut result = Rendered {
        text: String::new(),
        links: Vec::new(),
    };
    if input.contains("GPUAddress::") {
        let parsed = input.trim().strip_prefix("GPUAddress::").and_then(|value| {
            let mut parts = value.split("::");
            let digits = parts.next()?;
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let address = digits.parse::<u64>().ok()?;
            let pointer_type = match parts.next() {
                Some(digits)
                    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) =>
                {
                    Some(digits.parse::<u32>().ok()?)
                }
                Some(_) => return None,
                None => None,
            };
            if parts.next().is_some() {
                return None;
            }
            Some((address, pointer_type))
        });
        if let Some((address, pointer_type)) = parsed {
            if let Some((index, offset, name)) =
                gpu_address(address).filter(|(_, _, name)| !name.is_empty())
            {
                result.text = format!("{name}+{offset}");
                result.links.push(Link {
                    range: 0..result.text.len(),
                    target: Target::Buffer {
                        index,
                        offset,
                        pointer_type,
                    },
                });
            } else {
                result.text = format!("0x{address:x}");
            }
            return result;
        }
        result.text.push_str(input);
        return result;
    }
    if input.trim().starts_with("<rdhtml>") && input.trim().ends_with("</rdhtml>") {
        result.text.push_str(input);
        return result;
    }
    let mut position = 0;
    let mut pending = 0;
    while position < input.len() {
        let rest = &input[position..];
        if let Some(after) = rest.strip_prefix("__rd_msgs::") {
            let event_len = after.bytes().take_while(u8::is_ascii_digit).count();
            let after_event = &after[event_len..];
            if event_len > 0 && after_event.starts_with(':') {
                let count_len = after_event[1..]
                    .bytes()
                    .take_while(u8::is_ascii_digit)
                    .count();
                if count_len > 0 {
                    let end = position + "__rd_msgs::".len() + event_len + 1 + count_len;
                    let resolved = after[..event_len]
                        .parse::<u32>()
                        .ok()
                        .zip(after_event[1..1 + count_len].parse::<u32>().ok())
                        .and_then(|(event, count)| {
                            shader_messages(event, count)
                                .filter(|label| !label.is_empty())
                                .map(|label| (event, label))
                        });
                    if let Some((event, label)) = resolved {
                        result.text.push_str(&input[pending..position]);
                        let start = result.text.len();
                        result.text.push_str(&label);
                        result.links.push(Link {
                            range: start..result.text.len(),
                            target: Target::ShaderMessages { event },
                        });
                        pending = end;
                    }
                    position = end;
                    continue;
                }
            }
        }
        let resource_prefix = "ResourceId::";
        let candidate = if rest.starts_with(resource_prefix) {
            Some((resource_prefix.len(), true))
        } else if rest.starts_with('@')
            && !input[..position]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            Some((1, false))
        } else {
            None
        };
        if let Some((prefix, is_resource)) = candidate {
            let digits = rest[prefix..]
                .bytes()
                .take_while(u8::is_ascii_digit)
                .count();
            let end = position + prefix + digits;
            let suffix_x = !is_resource && input.as_bytes().get(end) == Some(&b'x');
            if digits > 0 && !suffix_x {
                let number = &input[position + prefix..end];
                let resolved = if is_resource {
                    number
                        .parse::<u64>()
                        .ok()
                        .filter(|&id| id != 0)
                        .and_then(&resource)
                        .filter(|(_, name)| !name.is_empty())
                        .map(|(index, name)| (Target::Resource(index), name))
                } else {
                    number
                        .parse::<u32>()
                        .ok()
                        .filter(|&id| event(id))
                        .map(|id| (Target::Event(id), format!("EID @{id}")))
                };
                if let Some((target, label)) = resolved {
                    result.text.push_str(&input[pending..position]);
                    let start = result.text.len();
                    result.text.push_str(&label);
                    if !label.is_empty() {
                        result.links.push(Link {
                            range: start..result.text.len(),
                            target,
                        });
                    }
                    pending = end;
                }
                position = end;
                continue;
            }
        }
        position += rest.chars().next().unwrap().len_utf8();
    }
    result.text.push_str(&input[pending..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rendered(input: &str) -> Rendered {
        render(
            input,
            |id| (id == 42).then(|| (3, "纹理".into())),
            |id| id == 7,
            |address| (address == 4100).then(|| (4, 4, "buffer".into())),
            |event, count| (event == 7).then(|| format!("{count} messages")),
        )
    }
    #[test]
    fn resolves_multiple_links_and_preserves_utf8_byte_ranges() {
        let result = rendered("前 ResourceId::0042 在 @7 和 ResourceId::99");
        assert_eq!(result.text, "前 纹理 在 EID @7 和 ResourceId::99");
        assert_eq!(result.links.len(), 2);
        assert_eq!(&result.text[result.links[0].range.clone()], "纹理");
        assert_eq!(result.links[0].target, Target::Resource(3));
        assert_eq!(&result.text[result.links[1].range.clone()], "EID @7");
        assert_eq!(result.links[1].target, Target::Event(7));
    }
    #[test]
    fn ignores_false_positives_null_overflow_and_unsupported_markup() {
        for text in [
            "a@7 x@7 _@7 文@7 @7x ResourceId::0",
            "ResourceId::18446744073709551616 @4294967296",
            "GPUAddress::7 ResourceId::42 @7",
            "<rdhtml>ResourceId::42</rdhtml>",
        ] {
            let result = rendered(text);
            assert_eq!(result.text, text);
            assert!(result.links.is_empty());
        }
        let empty_name = render(
            "ResourceId::42",
            |_| Some((0, String::new())),
            |_| false,
            |_| None,
            |_, _| None,
        );
        assert_eq!(empty_name.text, "ResourceId::42");
        assert!(empty_name.links.is_empty());
    }
    #[test]
    fn standalone_gpu_addresses_resolve_or_show_hex_without_guessing_type() {
        let resolved = rendered("GPUAddress::4100::99");
        assert_eq!(resolved.text, "buffer+4");
        assert_eq!(
            resolved.links[0].target,
            Target::Buffer {
                index: 4,
                offset: 4,
                pointer_type: Some(99)
            }
        );
        assert_eq!(rendered("GPUAddress::4096").text, "0x1000");
        assert_eq!(rendered("GPUAddress::0").text, "0x0");
        for malformed in [
            "GPUAddress::4100::",
            "GPUAddress::18446744073709551616",
            "GPUAddress::4100::4294967296",
            "x GPUAddress::4100",
        ] {
            assert_eq!(rendered(malformed).text, malformed);
            assert!(rendered(malformed).links.is_empty());
        }
    }
    #[test]
    fn shader_message_links_require_valid_known_event_and_count() {
        let result = rendered("a __rd_msgs::7:3 b __rd_msgs::8:1");
        assert_eq!(result.text, "a 3 messages b __rd_msgs::8:1");
        assert_eq!(result.links[0].target, Target::ShaderMessages { event: 7 });
        assert!(rendered("__rd_msgs::7:4294967296").links.is_empty());
    }
}
