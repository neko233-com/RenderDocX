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
    enum_def: Option<Arc<EnumDefinition>>,
    nested: Option<Arc<Layout>>,
    array_stride: usize,
    matrix_stride: usize,
    bit_offset: u32,
    bit_size: Option<u32>,
    count: usize,
    offset: usize,
}
#[derive(Clone, Debug)]
struct EnumDefinition {
    name: String,
    format: ScalarFormat,
    flags: bool,
    variants: Vec<(String, u64)>,
}
impl EnumDefinition {
    fn names(&self, value: u64) -> Vec<String> {
        if self.flags {
            if value == 0 {
                return self
                    .variants
                    .iter()
                    .find(|(_, bits)| *bits == 0)
                    .map(|(name, _)| vec![name.clone()])
                    .unwrap_or_default();
            }
            self.variants
                .iter()
                .filter(|(_, bits)| value & *bits != 0)
                .map(|(name, _)| name.clone())
                .collect()
        } else {
            self.variants
                .iter()
                .find(|(_, bits)| value == *bits)
                .map(|(name, _)| vec![name.clone()])
                .unwrap_or_default()
        }
    }
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
    repeat: Option<Repeat>,
    single: bool,
}
#[derive(Clone, Debug)]
struct Repeat {
    offset: usize,
    fixed_fields: Vec<Field>,
    has_fixed_prefix: bool,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enum_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enum_flags: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enum_names: Option<Vec<Vec<String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_offset: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_size: Option<u32>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed: Option<Vec<FieldValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat_offset: Option<String>,
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
        } else if b"#()[];{}:=,-".contains(&bytes[i]) {
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
fn enum_integer(text: &str, negative: bool, format: ScalarFormat) -> Result<u64, String> {
    let signed = matches!(
        format,
        ScalarFormat::I8 | ScalarFormat::I16 | ScalarFormat::I32 | ScalarFormat::I64
    );
    if negative && !signed {
        return Err("Unsigned enum value cannot be negative".into());
    }
    let (digits, radix) = if let Some(v) = text.strip_prefix("0x") {
        (v, 16)
    } else if text.len() > 1 && text.starts_with('0') {
        (&text[1..], 8)
    } else {
        (text, 10)
    };
    let magnitude = u128::from_str_radix(digits, radix).map_err(|_| "Invalid enum value")?;
    let width = format.width() * 8;
    let limit = if signed {
        1u128 << (width - 1)
    } else {
        1u128 << width
    };
    if (signed && ((!negative && magnitude >= limit) || (negative && magnitude > limit)))
        || (!signed && magnitude >= limit)
    {
        return Err("Enum value is out of range".into());
    }
    let mask = (1u128 << width) - 1;
    Ok(if negative {
        (0u128.wrapping_sub(magnitude) & mask) as u64
    } else {
        magnitude as u64
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
        let mut enums: HashMap<&str, Arc<EnumDefinition>> = HashMap::new();
        let mut last = None;
        let mut single_definition = false;
        loop {
            let mut flags = false;
            if tokens.get(i..i + 5).is_some_and(|v| {
                (v == ["[", "[", "single", "]", "]"] || v == ["[", "[", "fixed", "]", "]"])
                    && tokens.get(i + 5) == Some(&"struct")
            }) {
                if single_definition {
                    return Err("Only one fixed definition is supported".into());
                }
                single_definition = true;
                i += 5;
            }
            if tokens.get(i..i + 4).is_some_and(|slice| {
                slice == ["[", "[", "flags", "]"] || slice == ["[", "[", "mask", "]"]
            }) && tokens.get(i + 4) == Some(&"]")
            {
                flags = true;
                i += 5;
                if tokens.get(i) != Some(&"enum") {
                    return Err("Flags annotation requires an enum".into());
                }
            }
            let kind = tokens.get(i).copied();
            if kind != Some("struct") && kind != Some("enum") {
                break;
            }
            let name = *tokens.get(i + 1).ok_or("Missing type name")?;
            if !name
                .bytes()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
                || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                || base_type(name).is_some()
                || definitions.contains_key(name)
                || enums.contains_key(name)
            {
                return Err("Invalid or duplicate type definition".into());
            }
            if kind == Some("struct") {
                if tokens.get(i + 2) != Some(&"{") {
                    return Err("Invalid struct definition".into());
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
                    &enums,
                    packing,
                    true,
                )?);
                definitions.insert(name, layout.clone());
                last = Some(layout);
                i = end + 2;
                if single_definition && i != tokens.len() {
                    return Err("A fixed struct cannot be used as a variable".into());
                }
            } else {
                if tokens.get(i + 2) != Some(&":") || tokens.get(i + 4) != Some(&"{") {
                    return Err("Enum requires an integer base type".into());
                }
                let format = base_type(tokens[i + 3])
                    .filter(|v| {
                        matches!(
                            v,
                            ScalarFormat::U8
                                | ScalarFormat::I8
                                | ScalarFormat::U16
                                | ScalarFormat::I16
                                | ScalarFormat::U32
                                | ScalarFormat::I32
                                | ScalarFormat::U64
                                | ScalarFormat::I64
                        )
                    })
                    .ok_or("Invalid enum base type")?;
                let mut variants = Vec::new();
                i += 5;
                while tokens.get(i) != Some(&"}") {
                    let variant = *tokens.get(i).ok_or("Unterminated enum")?;
                    if !variant
                        .bytes()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
                        || !variant
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
                        || tokens.get(i + 1) != Some(&"=")
                    {
                        return Err("Invalid enum value".into());
                    }
                    i += 2;
                    let negative = tokens.get(i) == Some(&"-");
                    if negative {
                        i += 1;
                    }
                    let bits =
                        enum_integer(tokens.get(i).ok_or("Missing enum value")?, negative, format)?;
                    i += 1;
                    variants.push((variant.to_owned(), bits));
                    if tokens.get(i) == Some(&",") {
                        i += 1;
                    } else if tokens.get(i) != Some(&"}") {
                        return Err("Expected comma in enum".into());
                    }
                }
                if tokens.get(i + 1) != Some(&";") {
                    return Err("Expected semicolon after enum".into());
                }
                enums.insert(
                    name,
                    Arc::new(EnumDefinition {
                        name: name.to_owned(),
                        format,
                        flags,
                        variants,
                    }),
                );
                i += 2;
            }
        }
        if i == tokens.len() {
            return last
                .map(|layout| {
                    let mut layout = (*layout).clone();
                    layout.single = single_definition;
                    layout
                })
                .ok_or_else(|| "Layout must contain fields".into());
        }
        let mut body = &tokens[i..];
        let single = body.get(..5).is_some_and(|v| {
            v == ["[", "[", "single", "]", "]"] || v == ["[", "[", "fixed", "]", "]"]
        });
        if single {
            body = &body[5..];
        }
        if body.ends_with(&["[", "]", ";"]) {
            if single {
                return Err("Fixed variables cannot be unbounded arrays".into());
            }
            let name = *body
                .get(body.len().saturating_sub(4))
                .ok_or("Missing array name")?;
            let mut sized = body.to_vec();
            sized.insert(sized.len() - 2, "2");
            let mut layout = Self::parse_body(&sized, &definitions, &enums, packing, false)?;
            let mut field = layout.fields.pop().ok_or("Missing repeated field")?;
            if field.name != name || field.count != 2 || field.bit_size.is_some() {
                return Err("Invalid unbounded array".into());
            }
            let offset = field.offset;
            field.offset = 0;
            field.count = 1;
            layout.stride = field.array_stride;
            layout.repeat = Some(Repeat {
                offset,
                fixed_fields: std::mem::take(&mut layout.fields),
                has_fixed_prefix: body.iter().filter(|token| **token == ";").count() > 1,
            });
            layout.fields.push(field);
            Ok(layout)
        } else {
            let mut layout = Self::parse_body(body, &definitions, &enums, packing, false)?;
            if single
                && (layout.fields.len() != 1
                    || body.iter().filter(|token| **token == ";").count() != 1)
            {
                return Err("A fixed variable must be the only root variable".into());
            }
            layout.single = single;
            Ok(layout)
        }
    }

    fn parse_body(
        t: &[&str],
        definitions: &HashMap<&str, Arc<Layout>>,
        enums: &HashMap<&str, Arc<EnumDefinition>>,
        packing: Packing,
        is_struct: bool,
    ) -> Result<Self, String> {
        let mut i = 0;
        let mut fields = Vec::new();
        let mut names = HashSet::new();
        let mut cursor = 0usize;
        let mut alignment = 1usize;
        let mut last_full_end = 0usize;
        let mut bit_cursor: Option<usize> = None;
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
                if let Some(bits) = bit_cursor.take() {
                    cursor = cursor
                        .checked_add(bits.div_ceil(8))
                        .ok_or("Layout size overflow")?;
                }
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
                        enum_def: None,
                        nested: Some(nested.clone()),
                        array_stride: nested.stride,
                        matrix_stride: 0,
                        bit_offset: 0,
                        bit_size: None,
                        count,
                        offset: cursor,
                    });
                }
                cursor = cursor.checked_add(size).ok_or("Layout size overflow")?;
                continue;
            }
            let enum_def = enums.get(typename).cloned();
            if enum_def.is_some()
                && (unsigned || normalization.is_some() || packed_name.is_some() || major.is_some())
            {
                return Err("Unsupported qualifier on enum field".into());
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
            let (mut format, matrix_rows, components) = if let Some(definition) = &enum_def {
                (definition.format, 1, 1)
            } else if let Some((_, format, count)) = alias {
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
            let anonymous = t.get(i) == Some(&":");
            let name = if anonymous {
                padding = true;
                ""
            } else {
                let name = *t.get(i).ok_or("Missing field name")?;
                i += 1;
                name
            };
            if !anonymous
                && (!name
                    .bytes()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
                    || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                    || (!padding && !names.insert(name)))
            {
                return Err("Invalid or duplicate field name".into());
            }
            let mut count = 1usize;
            let mut array_declared = false;
            if t.get(i) == Some(&"[") {
                array_declared = true;
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
            let bit_size = if t.get(i) == Some(&":") {
                let bits = t
                    .get(i + 1)
                    .and_then(|v| v.parse::<u32>().ok())
                    .filter(|v| *v > 0)
                    .ok_or("Invalid bitfield width")?;
                i += 2;
                Some(bits)
            } else {
                None
            };
            if anonymous && bit_size.is_none() {
                return Err("Anonymous field requires a bit width".into());
            }
            if t.get(i) != Some(&";") {
                return Err("Expected semicolon".into());
            }
            i += 1;
            let width = if packed.is_some() { 4 } else { format.width() };
            if let Some(bits) = bit_size {
                if components != 1
                    || matrix_rows != 1
                    || count != 1
                    || array_declared
                    || packed.is_some()
                    || !matches!(
                        format,
                        ScalarFormat::U8
                            | ScalarFormat::I8
                            | ScalarFormat::U16
                            | ScalarFormat::I16
                            | ScalarFormat::U32
                            | ScalarFormat::I32
                            | ScalarFormat::U64
                            | ScalarFormat::I64
                    )
                    || bits as usize > width * 8
                {
                    return Err(
                        "Bitfields require a scalar integer or enum and a valid width".into(),
                    );
                }
                if !padding {
                    alignment = alignment.max(width);
                }
                if enum_def.is_some() && bit_cursor.is_none() {
                    cursor = align(cursor, width).ok_or("Layout size overflow")?;
                }
                let mut position = bit_cursor.unwrap_or(0);
                let capacity = width * 8;
                if position + bits as usize > capacity {
                    cursor = cursor
                        .checked_add(
                            position
                                .div_ceil(capacity)
                                .checked_mul(width)
                                .ok_or("Layout size overflow")?,
                        )
                        .ok_or("Layout size overflow")?;
                    position = 0;
                }
                if let Some(offset) = specified_offset {
                    if offset < cursor {
                        return Err("Explicit offset overlaps earlier data".into());
                    }
                    if offset > cursor {
                        position = 0;
                    }
                    cursor = offset;
                }
                bit_cursor = Some(position + bits as usize);
                if !padding {
                    last_full_end = cursor.checked_add(width).ok_or("Layout size overflow")?;
                    fields.push(Field {
                        name: name.to_owned(),
                        format,
                        components: 1,
                        matrix_rows: 1,
                        row_major: false,
                        packed: None,
                        enum_def,
                        nested: None,
                        array_stride: width,
                        matrix_stride: 0,
                        bit_offset: position as u32,
                        bit_size: Some(bits),
                        count: 1,
                        offset: cursor,
                    });
                }
                continue;
            }
            if let Some(bits) = bit_cursor.take() {
                cursor = cursor
                    .checked_add(bits.div_ceil(8))
                    .ok_or("Layout size overflow")?;
            }
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
                    enum_def,
                    nested: None,
                    array_stride,
                    matrix_stride,
                    bit_offset: 0,
                    bit_size: None,
                    count,
                    offset: cursor,
                });
            }
            cursor = cursor.checked_add(size).ok_or("Layout size overflow")?;
        }
        if let Some(bits) = bit_cursor {
            cursor = cursor
                .checked_add(bits.div_ceil(8))
                .ok_or("Layout size overflow")?;
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
            repeat: None,
            single: false,
        })
    }
    fn decode_fields(
        source: &[Field],
        row_offset: u64,
        bytes: &[u8],
    ) -> Result<Vec<FieldValue>, String> {
        let mut fields = Vec::new();
        let mut pending = Vec::new();
        for field in source.iter().rev() {
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
            if let Some(bits) = field.bit_size {
                let byte_count =
                    ((field.bit_offset as usize + bits as usize).div_ceil(8)).min(width);
                let mut raw = [0u8; 8];
                let data = bytes
                    .get(field_start..field_start + byte_count)
                    .ok_or("Incomplete bitfield")?;
                raw[..byte_count].copy_from_slice(data);
                let mask = if bits == 64 {
                    u64::MAX
                } else {
                    (1u64 << bits) - 1
                };
                let value = (u64::from_le_bytes(raw) >> field.bit_offset) & mask;
                let signed = matches!(
                    field.format,
                    ScalarFormat::I8 | ScalarFormat::I16 | ScalarFormat::I32 | ScalarFormat::I64
                );
                let signed_value = if signed {
                    ((value << (64 - bits)) as i64) >> (64 - bits)
                } else {
                    0
                };
                let numeric = if signed {
                    signed_value.to_string()
                } else {
                    value.to_string()
                };
                let enum_bits = if signed {
                    (signed_value as u64)
                        & if width == 8 {
                            u64::MAX
                        } else {
                            (1u64 << (width * 8)) - 1
                        }
                } else {
                    value
                };
                fields.push(FieldValue {
                    name,
                    offset: (row_offset + field_start as u64).to_string(),
                    matrix_rows: None,
                    matrix_columns: None,
                    row_major: None,
                    packed: None,
                    enum_type: field
                        .enum_def
                        .as_ref()
                        .map(|definition| definition.name.clone()),
                    enum_flags: field.enum_def.as_ref().map(|definition| definition.flags),
                    enum_names: field
                        .enum_def
                        .as_ref()
                        .map(|definition| vec![definition.names(enum_bits)]),
                    bit_offset: Some(field.bit_offset),
                    bit_size: Some(bits),
                    values: vec![vec![numeric]],
                });
                continue;
            }
            let mut values = Vec::new();
            let mut enum_names = field.enum_def.as_ref().map(|_| Vec::new());
            for item in 0..field.count {
                if let Some(packed) = field.packed {
                    let start = field_start + item * field.array_stride;
                    values.push(
                        packed.decode(u32::from_le_bytes(
                            bytes
                                .get(start..start + 4)
                                .ok_or("Incomplete packed field")?
                                .try_into()
                                .unwrap(),
                        )),
                    );
                    continue;
                }
                let base = field_start + item * field.array_stride;
                if let (Some(definition), Some(names)) = (&field.enum_def, &mut enum_names) {
                    let mut raw = [0u8; 8];
                    raw[..width].copy_from_slice(
                        bytes
                            .get(base..base + width)
                            .ok_or("Incomplete enum field")?,
                    );
                    names.push(definition.names(u64::from_le_bytes(raw)));
                }
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
                        components.push(
                            field
                                .format
                                .value(bytes.get(start..start + width).ok_or("Incomplete field")?),
                        );
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
                enum_type: field
                    .enum_def
                    .as_ref()
                    .map(|definition| definition.name.clone()),
                enum_flags: field.enum_def.as_ref().map(|definition| definition.flags),
                enum_names,
                bit_offset: None,
                bit_size: None,
                values,
            });
        }
        Ok(fields)
    }

    pub fn repeat_offset(&self) -> Option<usize> {
        self.repeat.as_ref().map(|repeat| repeat.offset)
    }

    pub fn is_single(&self) -> bool {
        self.single
    }

    /// GraphicsAPI values come from the matching RenderDoc replay API.
    pub fn validate_graphics_api(&self, api: u32) -> Result<(), String> {
        if matches!(api, 0 | 1)
            && self
                .repeat
                .as_ref()
                .is_some_and(|repeat| repeat.has_fixed_prefix)
        {
            return Err("On D3D an unbounded array must be the only root variable".into());
        }
        Ok(())
    }

    pub fn page_offset_for_position(&self, origin: u64, position: u64) -> Option<u64> {
        if self.single {
            return (position >= origin && position < origin.checked_add(self.stride as u64)?)
                .then_some(origin);
        }
        let Some(repeat) = &self.repeat else {
            return Some(position);
        };
        let start = origin.checked_add(repeat.offset as u64)?;
        if position < origin {
            None
        } else if position < start {
            Some(origin)
        } else {
            Some(start + (position - start) / self.stride as u64 * self.stride as u64)
        }
    }

    pub fn decode(&self, offset: u64, bytes: &[u8]) -> Result<Page, String> {
        self.decode_at(offset, offset, bytes)
    }

    /// `origin` is the beginning of the complete layout, even for later pages.
    pub fn decode_at(&self, origin: u64, offset: u64, bytes: &[u8]) -> Result<Page, String> {
        offset
            .checked_add(bytes.len() as u64)
            .ok_or("Buffer range overflow")?;
        if self.single {
            if offset != origin {
                return Err("A fixed layout must start at its origin".into());
            }
            let data = bytes.get(..self.stride).ok_or("Incomplete fixed layout")?;
            return Ok(Page {
                packing: self.packing.name(),
                stride: self.stride.to_string(),
                fixed: Some(Self::decode_fields(&self.fields, origin, data)?),
                repeat_offset: None,
                rows: Vec::new(),
                trailing_hex: String::new(),
            });
        }
        let mut fixed = None;
        let mut repeat_offset = None;
        let mut row_offset = offset;
        let mut row_bytes = bytes;
        if let Some(repeat) = &self.repeat {
            let start = origin
                .checked_add(repeat.offset as u64)
                .ok_or("Buffer range overflow")?;
            repeat_offset = Some(start.to_string());
            if offset == origin {
                if bytes.len() < repeat.offset {
                    return Err("Incomplete fixed prefix".into());
                }
                fixed = Some(Self::decode_fields(
                    &repeat.fixed_fields,
                    origin,
                    &bytes[..repeat.offset],
                )?);
                row_bytes = &bytes[repeat.offset..];
                row_offset = start;
            } else if offset < start || (offset - start) % self.stride as u64 != 0 {
                return Err("Page offset is not aligned to the repeated array".into());
            }
        }
        let mut chunks = row_bytes.chunks_exact(self.stride);
        let mut rows = Vec::new();
        for (index, row) in chunks.by_ref().enumerate() {
            let offset = row_offset + (index * self.stride) as u64;
            rows.push(Row {
                offset: offset.to_string(),
                fields: Self::decode_fields(&self.fields, offset, row)?,
            });
        }
        Ok(Page {
            packing: self.packing.name(),
            stride: self.stride.to_string(),
            fixed,
            repeat_offset,
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
    fn fixed_layout_and_repeat_navigation_follow_the_original_origin() {
        for source in [
            "[[single]] struct Data { uint x; float y; };",
            "struct Data { uint x; float y; }; [[fixed]] Data data;",
        ] {
            let layout = Layout::parse(source).unwrap();
            assert!(layout.is_single());
            let page = layout
                .decode_at(12, 12, &[1, 0, 0, 0, 0, 0, 128, 63, 99])
                .unwrap();
            assert_eq!(page.fixed.unwrap().len(), 2);
            assert!(page.rows.is_empty());
            assert_eq!(layout.page_offset_for_position(12, 19), Some(12));
            assert_eq!(layout.page_offset_for_position(12, 20), None);
            assert!(layout.decode_at(12, 13, &[0; 8]).is_err());
            assert!(layout.decode_at(12, 12, &[0; 4]).is_err());
        }
        for source in [
            "[[fixed]] struct S { uint x; }; S s;",
            "[[single]] uint x; uint y;",
            "[[single]] float x[];",
        ] {
            assert!(Layout::parse(source).is_err(), "{source}");
        }
        let layout = Layout::parse("uint header; float2 data[];").unwrap();
        assert!(layout.validate_graphics_api(0).is_err());
        assert!(layout.validate_graphics_api(1).is_err());
        assert!(layout.validate_graphics_api(2).is_ok());
        assert!(layout.validate_graphics_api(3).is_ok());
        assert!(
            Layout::parse("float2 data[];")
                .unwrap()
                .validate_graphics_api(0)
                .is_ok()
        );
        assert_eq!(layout.page_offset_for_position(100, 99), None);
        assert_eq!(layout.page_offset_for_position(100, 103), Some(100));
        assert_eq!(layout.page_offset_for_position(100, 111), Some(104));
        assert_eq!(layout.page_offset_for_position(100, 112), Some(112));
    }
    #[test]
    fn unbounded_root_arrays_keep_the_original_prefix_across_pages() {
        let layout = Layout::parse("#pack(std430) uint header; vec2 data[];").unwrap();
        assert_eq!(layout.repeat_offset(), Some(8));
        assert_eq!(layout.stride(), 8);
        let bytes = [
            7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128, 63, 0, 0, 0, 64, 0, 0, 64, 64, 0, 0, 128, 64,
        ];
        let first = layout.decode_at(0, 0, &bytes[..16]).unwrap();
        assert_eq!(first.fixed.unwrap()[0].values[0][0], "7");
        assert_eq!(first.repeat_offset.as_deref(), Some("8"));
        assert_eq!(first.rows[0].offset, "8");
        assert_eq!(first.rows[0].fields[0].values[0], ["1", "2"]);
        let next = layout.decode_at(0, 16, &bytes[16..]).unwrap();
        assert!(next.fixed.is_none());
        assert_eq!(next.rows[0].offset, "16");
        assert_eq!(next.rows[0].fields[0].values[0], ["3", "4"]);
        assert!(layout.decode_at(0, 9, &bytes[..8]).is_err());
        assert!(layout.decode_at(0, 0, &bytes[..4]).is_err());
        assert!(Layout::parse("struct S { float v[]; }; S value;").is_err());
        assert!(Layout::parse("float a[]; uint b;").is_err());
        let structures =
            Layout::parse("struct S { float a; uint b; }; uint header; S data[];").unwrap();
        assert_eq!(structures.repeat_offset(), Some(4));
        assert_eq!(structures.stride(), 8);
        assert_eq!(
            structures.decode_at(0, 0, &[0; 12]).unwrap().rows[0]
                .fields
                .len(),
            2
        );
    }
    #[test]
    fn bitfields_follow_upstream_rollover_and_sign_extension() {
        let layout =
            Layout::parse("uint first:8; uint :4; uint upper:4; int signed:4; uint tail:12;")
                .unwrap();
        assert_eq!(layout.stride(), 4);
        let fields = &layout.decode(0, &[0x12, 0xa0, 0x0f, 0x00]).unwrap().rows[0].fields;
        assert_eq!(
            fields
                .iter()
                .map(|f| f.values[0][0].as_str())
                .collect::<Vec<_>>(),
            ["18", "10", "-1", "0"]
        );
        assert_eq!(
            fields
                .iter()
                .map(|f| f.bit_offset.unwrap())
                .collect::<Vec<_>>(),
            [0, 12, 16, 20]
        );
        let mixed =
            Layout::parse("uint first:24; ubyte small:4; ubyte next:4; uint last:1;").unwrap();
        assert_eq!(
            mixed
                .fields
                .iter()
                .map(|f| (f.offset, f.bit_offset))
                .collect::<Vec<_>>(),
            [(0, 0), (3, 0), (3, 4), (3, 8)]
        );
        let shifted = Layout::parse("uint a:13; [[offset(8)]] uint b:14;").unwrap();
        assert_eq!(
            (
                shifted.fields[1].offset,
                shifted.fields[1].bit_offset,
                shifted.stride()
            ),
            (8, 0, 12)
        );
        let nested = Layout::parse("struct A { [[offset(1)]] uint bit:1; }; A a;").unwrap();
        let page = nested.decode(0, &[0, 1, 0, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(page.rows[0].fields[0].values[0][0], "1");
        let full = Layout::parse("ulong bits:64;")
            .unwrap()
            .decode(0, &u64::MAX.to_le_bytes())
            .unwrap();
        assert_eq!(full.rows[0].fields[0].values[0][0], u64::MAX.to_string());
        let en = Layout::parse("enum E : byte { Minus = -1 }; E value:3;")
            .unwrap()
            .decode(0, &[7])
            .unwrap();
        assert_eq!(
            en.rows[0].fields[0].enum_names.as_ref().unwrap()[0],
            ["Minus"]
        );
        for bad in [
            "float f:2;",
            "uint4 v:2;",
            "uint n:33;",
            "uint n:0;",
            "uint n[2]:1;",
            "uint n[1]:1;",
        ] {
            assert!(Layout::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn enum_and_flags_preserve_numeric_values_and_names() {
        let layout = Layout::parse("enum State : byte { Missing = -1, Ready = 2 }; [[flags]] enum Bits : uint { None = 0, A = 1, B = 2 }; struct Data { State s; Bits mask; }; Data d;").unwrap();
        assert_eq!(layout.stride(), 8);
        let page = layout.decode(0, &[255, 0, 0, 0, 3, 0, 0, 0]).unwrap();
        assert_eq!(page.rows[0].fields[0].values[0], ["-1"]);
        assert_eq!(
            page.rows[0].fields[0].enum_names.as_ref().unwrap()[0],
            ["Missing"]
        );
        assert_eq!(
            page.rows[0].fields[1].enum_names.as_ref().unwrap()[0],
            ["A", "B"]
        );
        let signed =
            Layout::parse("enum Signed : long { Min = -0x8000000000000000 }; Signed value;")
                .unwrap();
        let page = signed.decode(0, &i64::MIN.to_le_bytes()).unwrap();
        assert_eq!(page.rows[0].fields[0].values[0][0], i64::MIN.to_string());
        assert_eq!(
            page.rows[0].fields[0].enum_names.as_ref().unwrap()[0],
            ["Min"]
        );
        let unknown = signed.decode(0, &0i64.to_le_bytes()).unwrap();
        assert!(unknown.rows[0].fields[0].enum_names.as_ref().unwrap()[0].is_empty());
        let zero = layout.decode(0, &[2, 0, 0, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(
            zero.rows[0].fields[1].enum_names.as_ref().unwrap()[0],
            ["None"]
        );
        for bad in [
            "enum E : ubyte { A = -1 }; E v;",
            "enum E : byte { A = 128 }; E v;",
            "enum E : float { A = 1 }; E v;",
            "enum E : uint { A }; E v;",
            "enum E : uint { A = 0x100000000 }; E v;",
        ] {
            assert!(Layout::parse(bad).is_err(), "{bad}");
        }
    }

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
            "float x[]; uint y;",
            "#pack(invalid) float x;",
            "float x; float x;",
            "struct S {};",
            "/*",
        ] {
            assert!(Layout::parse(bad).is_err(), "{bad}");
        }
    }
}
