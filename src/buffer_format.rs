//! Scalar subset of RenderDoc BufferFormatter types, decoded from little-endian buffer bytes.
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarFormat {
    Bool,
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    F16,
    F32,
    F64,
    UNorm8,
    SNorm8,
    UNorm16,
    SNorm16,
}
impl ScalarFormat {
    pub const ALL: [Self; 16] = [
        Self::Bool,
        Self::U8,
        Self::I8,
        Self::U16,
        Self::I16,
        Self::U32,
        Self::I32,
        Self::U64,
        Self::I64,
        Self::F16,
        Self::F32,
        Self::F64,
        Self::UNorm8,
        Self::SNorm8,
        Self::UNorm16,
        Self::SNorm16,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::U8 => "ubyte",
            Self::I8 => "byte",
            Self::U16 => "ushort",
            Self::I16 => "short",
            Self::U32 => "uint",
            Self::I32 => "int",
            Self::U64 => "ulong",
            Self::I64 => "long",
            Self::F16 => "half",
            Self::F32 => "float",
            Self::F64 => "double",
            Self::UNorm8 => "unormb",
            Self::SNorm8 => "snormb",
            Self::UNorm16 => "unormh",
            Self::SNorm16 => "snormh",
        }
    }
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.name() == name)
    }
    pub fn width(self) -> usize {
        match self {
            Self::U8 | Self::I8 | Self::UNorm8 | Self::SNorm8 => 1,
            Self::U16 | Self::I16 | Self::F16 | Self::UNorm16 | Self::SNorm16 => 2,
            Self::Bool | Self::U32 | Self::I32 | Self::F32 => 4,
            Self::U64 | Self::I64 | Self::F64 => 8,
        }
    }
    pub(crate) fn value(self, b: &[u8]) -> String {
        match self {
            Self::Bool => (u32::from_le_bytes(b.try_into().unwrap()) != 0).to_string(),
            Self::U8 => b[0].to_string(),
            Self::I8 => (b[0] as i8).to_string(),
            Self::U16 => u16::from_le_bytes(b.try_into().unwrap()).to_string(),
            Self::I16 => i16::from_le_bytes(b.try_into().unwrap()).to_string(),
            Self::U32 => u32::from_le_bytes(b.try_into().unwrap()).to_string(),
            Self::I32 => i32::from_le_bytes(b.try_into().unwrap()).to_string(),
            Self::U64 => u64::from_le_bytes(b.try_into().unwrap()).to_string(),
            Self::I64 => i64::from_le_bytes(b.try_into().unwrap()).to_string(),
            Self::F16 => crate::half_convert::from_half(u16::from_le_bytes(b.try_into().unwrap()))
                .to_string(),
            Self::F32 => f32::from_le_bytes(b.try_into().unwrap()).to_string(),
            Self::F64 => f64::from_le_bytes(b.try_into().unwrap()).to_string(),
            Self::UNorm8 => (f32::from(b[0]) / 255.0).to_string(),
            Self::SNorm8 => (f32::from(b[0] as i8) / 127.0).max(-1.0).to_string(),
            Self::UNorm16 => {
                (f32::from(u16::from_le_bytes(b.try_into().unwrap())) / 65535.0).to_string()
            }
            Self::SNorm16 => (f32::from(i16::from_le_bytes(b.try_into().unwrap())) / 32767.0)
                .max(-1.0)
                .to_string(),
        }
    }
}
#[derive(Debug, Serialize)]
pub struct ScalarValue {
    pub offset: String,
    pub value: String,
}
#[derive(Debug, Serialize)]
pub struct DecodedBuffer {
    pub format: &'static str,
    pub values: Vec<ScalarValue>,
    pub trailing_hex: String,
}
pub fn decode(format: ScalarFormat, offset: u64, bytes: &[u8]) -> Result<DecodedBuffer, String> {
    offset
        .checked_add(bytes.len() as u64)
        .ok_or("Buffer range overflow")?;
    let mut chunks = bytes.chunks_exact(format.width());
    let values = chunks
        .by_ref()
        .enumerate()
        .map(|(index, value)| ScalarValue {
            offset: (offset + (index * format.width()) as u64).to_string(),
            value: format.value(value),
        })
        .collect();
    let trailing_hex = chunks
        .remainder()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(DecodedBuffer {
        format: format.name(),
        values,
        trailing_hex,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_scalars_without_losing_partial_bytes_or_u64() {
        let page = decode(ScalarFormat::U64, 7, &[255; 9]).unwrap();
        assert_eq!(page.values[0].offset, "7");
        assert_eq!(page.values[0].value, u64::MAX.to_string());
        assert_eq!(page.trailing_hex, "ff");
        assert_eq!(
            decode(ScalarFormat::F16, 0, &[0, 0x3c]).unwrap().values[0].value,
            "1"
        );
        assert_eq!(
            decode(ScalarFormat::F32, 0, &f32::NAN.to_le_bytes())
                .unwrap()
                .values[0]
                .value,
            "NaN"
        );
        assert_eq!(
            decode(ScalarFormat::SNorm8, 0, &[0x80]).unwrap().values[0].value,
            "-1"
        );
        assert_eq!(
            decode(ScalarFormat::UNorm16, 0, &[0xff, 0xff])
                .unwrap()
                .values[0]
                .value,
            "1"
        );
        for format in ScalarFormat::ALL {
            assert_eq!(ScalarFormat::parse(format.name()), Some(format));
        }
        assert!(decode(ScalarFormat::U8, u64::MAX, &[0]).is_err());
    }
}
