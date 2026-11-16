//! A deliberately restricted, scalar-packed subset of RenderDoc buffer declarations.
use crate::buffer_format::ScalarFormat;
use serde::Serialize;
use std::collections::HashSet;

#[derive(Clone, Debug)]
struct Field {
    name: String,
    format: ScalarFormat,
    components: usize,
    count: usize,
    offset: usize,
}
#[derive(Clone, Debug)]
pub struct Layout {
    fields: Vec<Field>,
    stride: usize,
}
#[derive(Serialize)]
pub struct FieldValue {
    pub name: String,
    pub offset: String,
    pub values: Vec<Vec<String>>,
}
#[derive(Serialize)]
pub struct Row {
    pub offset: String,
    pub fields: Vec<FieldValue>,
}
#[derive(Serialize)]
pub struct Page {
    pub stride: String,
    pub rows: Vec<Row>,
    pub trailing_hex: String,
}

fn tokens(source: &str) -> Result<Vec<&str>, String> {
    let bytes = source.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/') {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            if i + 1 >= bytes.len() {
                return Err("Unterminated comment".into());
            }
            i += 2;
            continue;
        }
        let start = i;
        if bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
        } else if b"#()[];".contains(&bytes[i]) {
            i += 1;
        } else {
            return Err("Unsupported format syntax".into());
        }
        out.push(&source[start..i]);
    }
    Ok(out)
}
fn align(value: usize, alignment: usize) -> Option<usize> {
    value
        .checked_add(alignment - 1)
        .map(|v| v / alignment * alignment)
}
fn base_type(name: &str) -> Option<ScalarFormat> {
    ScalarFormat::parse(name).or_else(|| {
        Some(match name {
            "char" | "int8_t" => ScalarFormat::I8,
            "uint8_t" => ScalarFormat::U8,
            "int16_t" => ScalarFormat::I16,
            "uint16_t" => ScalarFormat::U16,
            "int32_t" => ScalarFormat::I32,
            "uint32_t" => ScalarFormat::U32,
            "int64_t" => ScalarFormat::I64,
            "uint64_t" => ScalarFormat::U64,
            "float16_t" => ScalarFormat::F16,
            "float32_t" => ScalarFormat::F32,
            "float64_t" => ScalarFormat::F64,
            _ => return None,
        })
    })
}
impl Layout {
    pub fn stride(&self) -> usize {
        self.stride
    }

    pub fn parse(source: &str) -> Result<Self, String> {
        let t = tokens(source)?;
        let mut i = 0;
        let mut fields = Vec::new();
        let mut names = HashSet::new();
        let mut cursor = 0usize;
        let mut alignment = 1usize;
        if t.first() == Some(&"#") {
            if t.get(..5) != Some(&["#", "pack", "(", "scalar", ")"][..]) {
                return Err("Only #pack(scalar) is supported".into());
            }
            i = 5;
        }
        while i < t.len() {
            let typename = t[i];
            i += 1;
            let (format, components) = if let Some(f) = base_type(typename) {
                (f, 1)
            } else {
                let (base, suffix) = typename.split_at(typename.len().saturating_sub(1));
                let n = match suffix {
                    "2" => 2,
                    "3" => 3,
                    "4" => 4,
                    _ => return Err("Unsupported field type".into()),
                };
                (base_type(base).ok_or("Unsupported field type")?, n)
            };
            let name = *t.get(i).ok_or("Missing field name")?;
            i += 1;
            if !name
                .bytes()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
                || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                || !names.insert(name)
            {
                return Err("Invalid or duplicate field name".into());
            }
            let mut count = 1usize;
            if t.get(i) == Some(&"[") {
                count = t
                    .get(i + 1)
                    .and_then(|v| v.parse::<usize>().ok())
                    .filter(|n| *n > 0)
                    .ok_or("Array size must be a positive integer")?;
                if t.get(i + 2) != Some(&"]") {
                    return Err("Invalid array declaration".into());
                }
                i += 3;
            }
            if t.get(i) != Some(&";") {
                return Err("Expected semicolon").map_err(String::from);
            }
            i += 1;
            let width = format.width();
            alignment = alignment.max(width);
            cursor = align(cursor, width).ok_or("Layout size overflow")?;
            let size = width
                .checked_mul(components)
                .and_then(|v| v.checked_mul(count))
                .ok_or("Layout size overflow")?;
            fields.push(Field {
                name: name.to_owned(),
                format,
                components,
                count,
                offset: cursor,
            });
            cursor = cursor.checked_add(size).ok_or("Layout size overflow")?;
        }
        if fields.is_empty() {
            return Err("Layout must contain fields".into());
        }
        let stride = align(cursor, alignment).ok_or("Layout size overflow")?;
        Ok(Self { fields, stride })
    }
    pub fn decode(&self, offset: u64, bytes: &[u8]) -> Result<Page, String> {
        offset
            .checked_add(bytes.len() as u64)
            .ok_or("Buffer range overflow")?;
        let mut chunks = bytes.chunks_exact(self.stride);
        let mut rows = Vec::new();
        for (index, bytes) in chunks.by_ref().enumerate() {
            let row_offset = offset + (index * self.stride) as u64;
            let mut fields = Vec::new();
            for field in &self.fields {
                let width = field.format.width();
                let mut values = Vec::new();
                for item in 0..field.count {
                    let base = field.offset + item * field.components * width;
                    let mut components = Vec::new();
                    for component in 0..field.components {
                        let start = base + component * width;
                        components.push(field.format.value(&bytes[start..start + width]));
                    }
                    values.push(components);
                }
                fields.push(FieldValue {
                    name: field.name.clone(),
                    offset: (row_offset + field.offset as u64).to_string(),
                    values,
                });
            }
            rows.push(Row {
                offset: row_offset.to_string(),
                fields,
            });
        }
        Ok(Page {
            stride: self.stride.to_string(),
            rows,
            trailing_hex: chunks
                .remainder()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scalar_layout_aligns_vectors_arrays_and_reports_unhandled_syntax() {
        let layout = Layout::parse(
            "#pack(scalar) /* comment */ ubyte tag; float3 position; ushort ids[2]; // end",
        )
        .unwrap();
        assert_eq!(layout.stride, 20);
        assert_eq!(
            layout.fields.iter().map(|f| f.offset).collect::<Vec<_>>(),
            [0, 4, 16]
        );
        let mut bytes = vec![0; 23];
        bytes[0] = 7;
        bytes[4..8].copy_from_slice(&1f32.to_le_bytes());
        bytes[16..18].copy_from_slice(&42u16.to_le_bytes());
        let page = layout.decode(100, &bytes).unwrap();
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].fields[1].values[0], ["1", "0", "0"]);
        assert_eq!(page.rows[0].fields[2].values[0], ["42"]);
        assert_eq!(page.trailing_hex, "000000");
        for bad in [
            "",
            "float x",
            "float x[];",
            "float3x4 x;",
            "#pack(std140) float x;",
            "float x; float x;",
            "struct S {float x;};",
            "/*",
        ] {
            assert!(Layout::parse(bad).is_err(), "{bad}");
        }
    }
}
