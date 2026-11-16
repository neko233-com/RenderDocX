//! A deliberately restricted, scalar-packed subset of RenderDoc buffer declarations.
use crate::{buffer_format::ScalarFormat, packed_format::PackedFormat};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

#[derive(Clone, Debug)]
struct Field {
    name: String,
    format: ScalarFormat,
    components: usize,
    matrix_rows: usize,
    row_major: bool,
    packed: Option<PackedFormat>,
    nested: Option<Arc<Layout>>,
    array_stride: usize,
    matrix_stride: usize,
    count: usize,
    offset: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Packing {
    Scalar,
    Structured,
    Std430,
    Std140,
    CBuffer,
}
impl Packing {
    fn tight_arrays(self) -> bool {
        !matches!(self, Self::Std140 | Self::CBuffer)
    }
    fn vector_align_component(self) -> bool {
        !matches!(self, Self::Std140 | Self::Std430)
    }
    fn vector_straddle(self) -> bool {
        matches!(self, Self::Scalar | Self::Structured)
    }
    fn trailing_overlap(self) -> bool {
        matches!(self, Self::Scalar | Self::CBuffer)
    }
    fn name(self) -> &'static str {
        match self {
            Self::Scalar => "scalar",
            Self::Structured => "structured",
            Self::Std430 => "std430",
            Self::Std140 => "std140",
            Self::CBuffer => "cbuffer",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Layout {
    fields: Vec<Field>,
    stride: usize,
    alignment: usize,
    natural_alignment: usize,
    used: usize,
    packing: Packing,
}
#[derive(Serialize)]
pub struct FieldValue {
    pub name: String,
    pub offset: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matrix_rows: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matrix_columns: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row_major: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub packed: Option<&'static str>,
    pub values: Vec<Vec<String>>,
}
#[derive(Serialize)]
pub struct Row {
    pub offset: String,
    pub fields: Vec<FieldValue>,
}
#[derive(Serialize)]
pub struct Page {
    pub packing: &'static str,
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
        } else if b"#()[];{}".contains(&bytes[i]) {
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
            "float64_t" | "dvec" => ScalarFormat::F64,
            "vec" | "mat" => ScalarFormat::F32,
            "ivec" | "imat" => ScalarFormat::I32,
            "uvec" | "umat" => ScalarFormat::U32,
            "dmat" => ScalarFormat::F64,
            _ => return None,
        })
    })
}
impl Layout {
    pub fn stride(&self) -> usize {
        self.stride
    }

    pub fn parse(source: &str) -> Result<Self, String> {
        let tokens = tokens(source)?;
        let mut i = 0;
        let mut packing = Packing::Scalar;
        if tokens.first() == Some(&"#") {
            if tokens.get(1) != Some(&"pack")
                || tokens.get(2) != Some(&"(")
                || tokens.get(4) != Some(&")")
            {
                return Err("Invalid packing directive".into());
            }
            packing = match tokens.get(3).map(|v| v.to_ascii_lowercase()).as_deref() {
                Some("scalar") => Packing::Scalar,
                Some("structured" | "d3duav" | "uav" | "c") => Packing::Structured,
                Some("std430" | "ssbo") => Packing::Std430,
                Some("std140" | "ubo" | "gl" | "gles" | "opengl" | "glsl") => Packing::Std140,
                Some("cbuffer" | "d3dcbuffer" | "cb") => Packing::CBuffer,
                _ => return Err("Unsupported packing directive".into()),
            };
            i = 5;
        }
        let mut definitions: HashMap<&str, Arc<Layout>> = HashMap::new();
        let mut last = None;
        while tokens.get(i) == Some(&"struct") {
            let name = *tokens.get(i + 1).ok_or("Missing struct name")?;
            if !name
                .bytes()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
                || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                || base_type(name).is_some()
                || definitions.contains_key(name)
                || tokens.get(i + 2) != Some(&"{")
            {
                return Err("Invalid or duplicate struct definition".into());
            }
            let body = i + 3;
            let end = (body..tokens.len())
                .find(|&n| tokens[n] == "}")
                .ok_or("Unterminated struct")?;
            if tokens.get(end + 1) != Some(&";") {
                return Err("Expected semicolon after struct".into());
            }
            let layout = Arc::new(Self::parse_body(
                &tokens[body..end],
                &definitions,
                packing,
                true,
            )?);
            definitions.insert(name, layout.clone());
            last = Some(layout);
            i = end + 2;
        }
        if i == tokens.len() {
            return last
                .map(|layout| (*layout).clone())
                .ok_or_else(|| "Layout must contain fields".into());
        }
        Self::parse_body(&tokens[i..], &definitions, packing, false)
    }

    fn parse_body(
        t: &[&str],
        definitions: &HashMap<&str, Arc<Layout>>,
        packing: Packing,
        is_struct: bool,
    ) -> Result<Self, String> {
        let mut i = 0;
        let mut fields = Vec::new();
        let mut names = HashSet::new();
        let mut cursor = 0usize;
        let mut alignment = 1usize;
        let mut last_full_end = 0usize;
        while i < t.len() {
            let mut specified_offset = None;
            let mut padding = false;
            let mut normalization = None;
            let mut packed_name = None;
            let mut major = None;
            while t.get(i) == Some(&"[") && t.get(i + 1) == Some(&"[") {
                let annotation = *t.get(i + 2).ok_or("Invalid annotation")?;
                i += 3;
                match annotation {
                    "offset" | "byte_offset" => {
                        if specified_offset.is_some() || t.get(i) != Some(&"(") {
                            return Err("Invalid offset annotation".into());
                        }
                        let value = t
                            .get(i + 1)
                            .and_then(|v| v.parse::<u32>().ok())
                            .filter(|v| *v != u32::MAX)
                            .ok_or("Invalid offset annotation")?;
                        if t.get(i + 2) != Some(&")") {
                            return Err("Invalid offset annotation".into());
                        }
                        specified_offset = Some(value as usize);
                        i += 3;
                    }
                    "packed" if packed_name.is_none() => {
                        if t.get(i) != Some(&"(") || t.get(i + 2) != Some(&")") {
                            return Err("Invalid packed annotation".into());
                        }
                        packed_name = Some(*t.get(i + 1).ok_or("Missing packed format")?);
                        i += 3;
                    }
                    "pad" | "padding" if !padding => padding = true,
                    "row_major" | "col_major" if major.is_none() => {
                        major = Some(annotation == "row_major")
                    }
                    "unorm" | "snorm" if normalization.is_none() => {
                        normalization = Some(annotation)
                    }
                    _ => return Err("Unsupported or duplicate annotation".into()),
                }
                if t.get(i) != Some(&"]") || t.get(i + 1) != Some(&"]") {
                    return Err("Invalid annotation".into());
                }
                i += 2;
            }
            let mut typename = *t.get(i).ok_or("Missing field type")?;
            i += 1;
            if typename == "row_major" || typename == "column_major" {
                if major.is_some() {
                    return Err("Duplicate matrix order".into());
                }
                major = Some(typename == "row_major");
                typename = *t.get(i).ok_or("Missing field type")?;
                i += 1;
            }
            let unsigned = typename == "unsigned";
            if unsigned {
                typename = *t.get(i).ok_or("Missing unsigned type")?;
                i += 1;
            }
            if let Some(nested) = definitions.get(typename) {
                if unsigned || normalization.is_some() || packed_name.is_some() || major.is_some() {
                    return Err("Unsupported annotation or qualifier on struct field".into());
                }
                let name = *t.get(i).ok_or("Missing field name")?;
                i += 1;
                if !name
                    .bytes()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
                    || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                    || (!padding && !names.insert(name))
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
                    return Err("Expected semicolon".into());
                }
                i += 1;
                if !padding {
                    alignment = alignment.max(nested.natural_alignment);
                }
                cursor = align(cursor, nested.alignment).ok_or("Layout size overflow")?;
                if let Some(offset) = specified_offset {
                    if offset < cursor {
                        return Err("Explicit offset overlaps earlier data".into());
                    }
                    cursor = offset;
                }
                let full_size = nested
                    .stride
                    .checked_mul(count)
                    .ok_or("Layout size overflow")?;
                let size = if packing.trailing_overlap() {
                    full_size
                        .checked_sub(nested.stride - nested.used)
                        .ok_or("Layout size overflow")?
                } else {
                    full_size
                };
                if !padding {
                    last_full_end = cursor
                        .checked_add(full_size)
                        .ok_or("Layout size overflow")?;
                    fields.push(Field {
                        name: name.to_owned(),
                        format: ScalarFormat::Bool,
                        components: 1,
                        matrix_rows: 1,
                        row_major: false,
                        packed: None,
                        nested: Some(nested.clone()),
                        array_stride: nested.stride,
                        matrix_stride: 0,
                        count,
                        offset: cursor,
                    });
                }
                cursor = cursor.checked_add(size).ok_or("Layout size overflow")?;
                continue;
            }
            let alias = match typename {
                "uintten" => Some(("r10g10b10a2_uint", ScalarFormat::U32, 4)),
                "unormten" => Some(("r10g10b10a2_unorm", ScalarFormat::U32, 4)),
                "floateleven" => Some(("r11g11b10", ScalarFormat::F32, 3)),
                _ => None,
            };
            if alias.is_some() && packed_name.is_some() {
                return Err("Duplicate packed format".into());
            }
            if let Some((name, _, _)) = alias {
                packed_name = Some(name);
            }
            let (mut format, matrix_rows, components) = if let Some((_, format, count)) = alias {
                (format, 1, count)
            } else if let Some((left, right)) = typename.split_once('x') {
                let (base, first) = left.split_at(left.len().saturating_sub(1));
                let dimension = |text: &str| match text {
                    "2" => Some(2),
                    "3" => Some(3),
                    "4" => Some(4),
                    _ => None,
                };
                let first = dimension(first).ok_or("Unsupported matrix dimension")?;
                let second = dimension(right).ok_or("Unsupported matrix dimension")?;
                let gl = matches!(base, "mat" | "imat" | "umat" | "dmat");
                let format = base_type(base).ok_or("Unsupported matrix type")?;
                if matches!(base, "vec" | "ivec" | "uvec" | "dvec") {
                    return Err("Unsupported matrix type".into());
                }
                let (rows, cols) = if gl { (second, first) } else { (first, second) };
                (format, rows, cols)
            } else if matches!(
                typename,
                "vec" | "ivec" | "uvec" | "dvec" | "mat" | "imat" | "umat" | "dmat"
            ) {
                return Err("Vector or matrix width required".into());
            } else if let Some(f) = base_type(typename) {
                (f, 1, 1)
            } else {
                let (base, suffix) = typename.split_at(typename.len().saturating_sub(1));
                let n = match suffix {
                    "2" => 2,
                    "3" => 3,
                    "4" => 4,
                    _ => return Err("Unsupported field type".into()),
                };
                let f = base_type(base).ok_or("Unsupported field type")?;
                if matches!(base, "mat" | "imat" | "umat" | "dmat") {
                    (f, n, n)
                } else {
                    (f, 1, n)
                }
            };
            if matrix_rows == 1 && major.is_some() {
                return Err("Matrix order requires a matrix".into());
            }
            let row_major = major.unwrap_or(false);
            if unsigned {
                format = match format {
                    ScalarFormat::I8 => ScalarFormat::U8,
                    ScalarFormat::I16 => ScalarFormat::U16,
                    ScalarFormat::I32 => ScalarFormat::U32,
                    ScalarFormat::I64 => ScalarFormat::U64,
                    _ => return Err("Invalid unsigned type".into()),
                };
            }
            let packed = packed_name
                .map(|name| {
                    PackedFormat::parse(name, format, components, matrix_rows, normalization)
                })
                .transpose()?;
            if let Some(normalization) = normalization.filter(|_| packed.is_none()) {
                format = match (normalization, format) {
                    ("unorm", ScalarFormat::I8 | ScalarFormat::U8) => ScalarFormat::UNorm8,
                    ("snorm", ScalarFormat::I8 | ScalarFormat::U8) => ScalarFormat::SNorm8,
                    ("unorm", ScalarFormat::I16 | ScalarFormat::U16) => ScalarFormat::UNorm16,
                    ("snorm", ScalarFormat::I16 | ScalarFormat::U16) => ScalarFormat::SNorm16,
                    _ => return Err("Normalization requires a byte or short integer".into()),
                };
            }
            let name = *t.get(i).ok_or("Missing field name")?;
            i += 1;
            if !name
                .bytes()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
                || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                || (!padding && !names.insert(name))
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
            let width = if packed.is_some() { 4 } else { format.width() };
            let vector_size = if matrix_rows > 1 && !row_major {
                matrix_rows
            } else {
                components
            };
            let vector_alignment = if !packing.vector_align_component() {
                width
                    .checked_mul(if vector_size >= 3 {
                        4
                    } else if vector_size == 2 {
                        2
                    } else {
                        1
                    })
                    .ok_or("Layout size overflow")?
            } else {
                width
            };
            let element_alignment = if packed.is_some() {
                4
            } else {
                vector_alignment
            };
            if !padding {
                alignment = alignment.max(vector_alignment);
            }
            cursor = align(cursor, element_alignment).ok_or("Layout size overflow")?;
            if !packing.tight_arrays() && (count > 1 || matrix_rows > 1) {
                cursor = align(cursor, 16).ok_or("Layout size overflow")?;
            }
            let element_size = if packed.is_some() {
                4
            } else if packing.vector_align_component() {
                element_alignment
                    .checked_mul(vector_size)
                    .ok_or("Layout size overflow")?
            } else {
                element_alignment
            };
            if !packing.vector_straddle()
                && cursor / 16
                    != cursor
                        .checked_add(element_size - 1)
                        .ok_or("Layout size overflow")?
                        / 16
            {
                cursor = align(cursor, 16).ok_or("Layout size overflow")?;
            }
            if let Some(offset) = specified_offset {
                if offset < cursor {
                    return Err("Explicit offset overlaps earlier data".into());
                }
                cursor = offset;
            }
            let mut vector_stride = if packed.is_some() {
                4
            } else if components > 1 || matrix_rows > 1 {
                element_size
            } else {
                element_alignment
            };
            if !packing.tight_arrays() {
                vector_stride = vector_stride.max(16);
            }
            let array_stride = if matrix_rows > 1 {
                vector_stride
                    .checked_mul(if row_major { matrix_rows } else { components })
                    .ok_or("Layout size overflow")?
            } else {
                vector_stride
            };
            let matrix_stride = if matrix_rows > 1 { vector_stride } else { 0 };
            let full_size = if count > 1 {
                array_stride.checked_mul(count)
            } else if matrix_rows > 1 {
                Some(array_stride)
            } else if packed.is_some() {
                Some(4)
            } else {
                width.checked_mul(components)
            }
            .ok_or("Layout size overflow")?;
            let size = if packing.trailing_overlap()
                && !packing.tight_arrays()
                && (count > 1 || matrix_rows > 1)
            {
                let padding = 16usize
                    .checked_sub(element_size)
                    .ok_or("Wide cbuffer array or matrix is not supported")?;
                full_size
                    .checked_sub(padding)
                    .ok_or("Layout size overflow")?
            } else {
                full_size
            };
            if !padding {
                last_full_end = cursor
                    .checked_add(full_size)
                    .ok_or("Layout size overflow")?;
            }
            if !padding {
                fields.push(Field {
                    name: name.to_owned(),
                    format,
                    components,
                    matrix_rows,
                    row_major,
                    packed,
                    nested: None,
                    array_stride,
                    matrix_stride,
                    count,
                    offset: cursor,
                });
            }
            cursor = cursor.checked_add(size).ok_or("Layout size overflow")?;
        }
        if fields.is_empty() {
            return Err("Layout must contain fields".into());
        }
        let natural_alignment = alignment;
        if is_struct && !packing.tight_arrays() {
            alignment = 16;
        }
        let end = if is_struct {
            cursor
        } else {
            cursor.max(last_full_end)
        };
        let stride = align(end, alignment).ok_or("Layout size overflow")?;
        Ok(Self {
            fields,
            stride,
            alignment,
            natural_alignment,
            used: cursor,
            packing,
        })
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
            let mut pending = Vec::new();
            for field in self.fields.iter().rev() {
                pending.push((field, 0usize, String::new()));
            }
            while let Some((field, parent, prefix)) = pending.pop() {
                let name = if prefix.is_empty() {
                    field.name.clone()
                } else {
                    format!("{prefix}.{}", field.name)
                };
                let field_start = parent + field.offset;
                if let Some(nested) = &field.nested {
                    for item in (0..field.count).rev() {
                        let prefix = if field.count > 1 {
                            format!("{name}[{item}]")
                        } else {
                            name.clone()
                        };
                        let start = field_start + item * nested.stride;
                        for child in nested.fields.iter().rev() {
                            pending.push((child, start, prefix.clone()));
                        }
                    }
                    continue;
                }
                let width = field.format.width();
                let mut values = Vec::new();
                for item in 0..field.count {
                    if let Some(packed) = field.packed {
                        let start = field_start + item * field.array_stride;
                        values.push(packed.decode(u32::from_le_bytes(
                            bytes[start..start + 4].try_into().unwrap(),
                        )));
                        continue;
                    }
                    let base = field_start + item * field.array_stride;
                    let mut components = Vec::new();
                    for row in 0..field.matrix_rows {
                        for column in 0..field.components {
                            let start = if field.matrix_rows == 1 {
                                base + column * width
                            } else if field.row_major {
                                base + row * field.matrix_stride + column * width
                            } else {
                                base + column * field.matrix_stride + row * width
                            };
                            components.push(field.format.value(&bytes[start..start + width]));
                        }
                    }
                    values.push(components);
                }
                fields.push(FieldValue {
                    name,
                    offset: (row_offset + field_start as u64).to_string(),
                    matrix_rows: (field.matrix_rows > 1).then_some(field.matrix_rows),
                    matrix_columns: (field.matrix_rows > 1).then_some(field.components),
                    row_major: (field.matrix_rows > 1).then_some(field.row_major),
                    packed: field.packed.map(PackedFormat::name),
                    values,
                });
            }
            rows.push(Row {
                offset: row_offset.to_string(),
                fields,
            });
        }
        Ok(Page {
            packing: self.packing.name(),
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
    fn std140_and_cbuffer_align_arrays_matrices_and_structs() {
        let std = Layout::parse("#pack(std140) float a; float2 b; float c[2]; mat2x3 m;").unwrap();
        assert_eq!(
            std.fields.iter().map(|f| f.offset).collect::<Vec<_>>(),
            [0, 8, 16, 48]
        );
        assert_eq!(std.stride(), 80);
        let mut bytes = vec![0; 80];
        for (at, value) in [
            (16, 1f32),
            (32, 2.),
            (48, 3.),
            (52, 4.),
            (56, 5.),
            (64, 6.),
            (68, 7.),
            (72, 8.),
        ] {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let page = std.decode(0, &bytes).unwrap();
        assert_eq!(page.rows[0].fields[2].values, [["1"], ["2"]]);
        assert_eq!(
            page.rows[0].fields[3].values[0],
            ["3", "6", "4", "7", "5", "8"]
        );
        let cb = Layout::parse("#pack(cbuffer) float3 p; float2 q; float a[2]; float b;").unwrap();
        assert_eq!(
            cb.fields.iter().map(|f| f.offset).collect::<Vec<_>>(),
            [0, 16, 32, 52]
        );
        assert_eq!(cb.stride(), 56);
        let cb_tail = Layout::parse("#pack(cb) float a[2];").unwrap();
        assert_eq!(cb_tail.stride(), 32);
        let nested = Layout::parse("#pack(ubo) struct A { float x; }; A a; float b;").unwrap();
        assert_eq!(nested.fields[1].offset, 16);
        assert_eq!(nested.stride(), 20);
        let packed = Layout::parse("#pack(cb) [[packed(r10g10b10a2_uint)]] uint4 p[2];").unwrap();
        let mut data = vec![0; 32];
        data[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            packed.decode(0, &data).unwrap().rows[0].fields[0].values[1],
            ["1023", "1023", "1023", "3"]
        );
        assert!(Layout::parse("#pack(cb) double3 a[2];").is_err());
    }

    #[test]
    fn structured_and_std430_packing_follow_upstream_layouts() {
        let scalar = Layout::parse("struct A { ushort x; ubyte y; }; A a; ubyte b;").unwrap();
        let structured =
            Layout::parse("#pack(structured) struct A { ushort x; ubyte y; }; A a; ubyte b;")
                .unwrap();
        assert_eq!((scalar.stride(), structured.stride()), (4, 6));
        assert_eq!(structured.fields[1].offset, 4);
        let layout =
            Layout::parse("#pack(std430) float3 pos; float w; float2 uv[2]; mat2x3 transform;")
                .unwrap();
        assert_eq!(
            layout.fields.iter().map(|f| f.offset).collect::<Vec<_>>(),
            [0, 12, 16, 32]
        );
        assert_eq!(layout.stride(), 64);
        let mut bytes = vec![0; 64];
        for (at, value) in [(32, 1f32), (36, 2.), (40, 3.), (48, 4.), (52, 5.), (56, 6.)] {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let page = layout.decode(0, &bytes).unwrap();
        assert_eq!(
            page.rows[0].fields[3].values[0],
            ["1", "4", "2", "5", "3", "6"]
        );
        let layout = Layout::parse("#pack(ssbo) float3 a[2]; float b;").unwrap();
        assert_eq!(layout.fields[1].offset, 32);
        assert_eq!(layout.stride(), 48);
        let nested =
            Layout::parse("#pack(std430) struct A { float3 p; float w; }; A a[2]; float tail;")
                .unwrap();
        assert_eq!(nested.fields[1].offset, 32);
        assert_eq!(nested.stride(), 48);
        let page = nested.decode(0, &[0; 48]).unwrap();
        assert_eq!(
            page.rows[0]
                .fields
                .iter()
                .map(|f| f.offset.as_str())
                .collect::<Vec<_>>(),
            ["0", "12", "16", "28", "32"]
        );
    }

    #[test]
    fn nested_struct_arrays_follow_scalar_trailing_overlap() {
        let source =
            "struct A { float3 pos; ubyte tag; }; struct B { A a[2]; ushort flag; }; B root;";
        let layout = Layout::parse(source).unwrap();
        assert_eq!(layout.stride(), 32);
        let mut bytes = vec![0; 32];
        bytes[12] = 1;
        bytes[28] = 2;
        bytes[30] = 7;
        let page = layout.decode(100, &bytes).unwrap();
        let fields = &page.rows[0].fields;
        assert_eq!(
            fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            [
                "root.a[0].pos",
                "root.a[0].tag",
                "root.a[1].pos",
                "root.a[1].tag",
                "root.flag"
            ]
        );
        assert_eq!(
            fields.iter().map(|f| f.offset.as_str()).collect::<Vec<_>>(),
            ["100", "112", "116", "128", "130"]
        );
        assert_eq!(fields[3].values[0][0], "2");
        assert_eq!(fields[4].values[0][0], "7");
        assert_eq!(
            Layout::parse("struct A { uint a; }; struct B { A a; }; ")
                .unwrap()
                .stride(),
            4
        );
        let unaligned =
            Layout::parse("struct A { ushort x; ubyte y; }; [[offset(1)]] A a;").unwrap();
        assert_eq!(unaligned.stride(), 6);
        let page = unaligned.decode(0, &[0, 2, 0, 3, 0, 0]).unwrap();
        assert_eq!(page.rows[0].fields[0].values[0][0], "2");
        assert_eq!(page.rows[0].fields[1].values[0][0], "3");
        for bad in [
            "struct A { A self; };",
            "struct A { B b; }; struct B { uint x; };",
            "struct A { uint x; }; struct A { uint y; };",
            "struct A { uint x; }; A a[];",
            "struct A { uint x; }; [[unorm]] A a;",
        ] {
            assert!(Layout::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn packed_layouts_use_four_byte_stride_and_preserve_array_elements() {
        let layout = Layout::parse(
            "ubyte tag; [[packed(r10g10b10a2_unorm)]] uint4 color[2]; floateleven light;",
        )
        .unwrap();
        assert_eq!(layout.stride(), 16);
        assert_eq!(
            layout.fields.iter().map(|f| f.offset).collect::<Vec<_>>(),
            [0, 4, 12]
        );
        let mut bytes = vec![0; 16];
        bytes[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        let page = layout.decode(0, &bytes).unwrap();
        assert_eq!(page.rows[0].fields[1].values[0], ["1", "1", "1", "1"]);
        assert_eq!(page.rows[0].fields[1].values[1], ["0", "0", "0", "0"]);
        assert_eq!(page.rows[0].fields[2].packed, Some("r11g11b10"));
        assert!(Layout::parse("[[packed(r11g11b10)]] float4 bad;").is_err());
        assert!(Layout::parse("[[packed(r10g10b10a2)]] [[snorm]] uint4 bad;").is_err());
    }

    #[test]
    fn matrices_follow_hlsl_glsl_dimensions_and_storage_order() {
        let bytes: Vec<u8> = (1..=6).flat_map(|n| (n as f32).to_le_bytes()).collect();
        for text in ["float3x2 m;", "mat2x3 m;", "[[col_major]] float3x2 m;"] {
            let layout = Layout::parse(text).unwrap();
            assert_eq!(layout.stride(), 24);
            let page = layout.decode(0, &bytes).unwrap();
            let field = &page.rows[0].fields[0];
            assert_eq!(
                (field.matrix_rows, field.matrix_columns, field.row_major),
                (Some(3), Some(2), Some(false))
            );
            assert_eq!(field.values[0], ["1", "4", "2", "5", "3", "6"]);
        }
        for text in ["[[row_major]] float3x2 m;", "row_major mat2x3 m;"] {
            let page = Layout::parse(text).unwrap().decode(0, &bytes).unwrap();
            assert_eq!(
                page.rows[0].fields[0].values[0],
                ["1", "2", "3", "4", "5", "6"]
            );
        }
        assert_eq!(Layout::parse("mat3 m[2];").unwrap().stride(), 72);
        for bad in [
            "[[row_major]] float2 v;",
            "float5x2 m;",
            "row_major [[col_major]] float2x2 m;",
        ] {
            assert!(Layout::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn explicit_offsets_padding_glsl_and_normalization_follow_scalar_rules() {
        let layout = Layout::parse("[[pad]] uint reserved; [[offset(8)]] vec2 pos; [[snorm]] byte4 normal; unsigned short id; bool visible;").unwrap();
        assert_eq!(layout.stride(), 28);
        assert_eq!(
            layout.fields.iter().map(|f| f.offset).collect::<Vec<_>>(),
            [8, 16, 20, 24]
        );
        let mut bytes = vec![0; 28];
        bytes[16] = 128;
        bytes[20] = 42;
        bytes[24] = 1;
        let page = layout.decode(0, &bytes).unwrap();
        assert_eq!(page.rows[0].fields[1].values[0][0], "-1");
        assert_eq!(page.rows[0].fields[2].values[0][0], "42");
        assert_eq!(page.rows[0].fields[3].values[0][0], "true");
        for bad in [
            "float a; [[offset(3)]] float b;",
            "[[offset(1)]] [[offset(4)]] float a;",
            "[[unorm]] float a;",
            "[[packed(r10g10b10a2)]] float4 a;",
            "vec pos;",
        ] {
            assert!(Layout::parse(bad).is_err(), "{bad}");
        }
        let pad = Layout::parse("[[pad]] ulong p; ubyte a;").unwrap();
        assert_eq!(pad.stride(), 9);
    }

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
            "#pack(invalid) float x;",
            "float x; float x;",
            "struct S {};",
            "/*",
        ] {
            assert!(Layout::parse(bad).is_err(), "{bad}");
        }
    }
}
