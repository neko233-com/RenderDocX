//! RenderDoc's packed 32-bit buffer formats.
use crate::buffer_format::ScalarFormat;

#[derive(Clone, Copy, Debug)]
pub enum PackedFormat {
    R10Uint,
    R10UNorm,
    R10SNorm,
    R11Float,
}
impl PackedFormat {
    pub fn parse(
        name: &str,
        format: ScalarFormat,
        components: usize,
        rows: usize,
        norm: Option<&str>,
    ) -> Result<Self, String> {
        if rows != 1 {
            return Err("Packed matrices are not supported".into());
        }
        let name = name.to_ascii_lowercase();
        let result = match name.as_str() {
            "r11g11b10" if format == ScalarFormat::F32 && components == 3 && norm.is_none() => {
                Self::R11Float
            }
            "r10g10b10a2" if components == 4 => match (format, norm) {
                (ScalarFormat::U32, None) => Self::R10Uint,
                (ScalarFormat::U32, Some("unorm")) => Self::R10UNorm,
                (ScalarFormat::I32, Some("snorm")) => Self::R10SNorm,
                _ => return Err("Packed type and normalization do not match".into()),
            },
            "r10g10b10a2_uint" if format == ScalarFormat::U32 && components == 4 => match norm {
                None => Self::R10Uint,
                Some("unorm") => Self::R10UNorm,
                Some("snorm") => Self::R10SNorm,
                _ => return Err("Invalid normalization".into()),
            },
            "r10g10b10a2_unorm"
                if format == ScalarFormat::U32 && components == 4 && norm.is_none() =>
            {
                Self::R10UNorm
            }
            "r10g10b10a2_snorm"
                if format == ScalarFormat::I32 && components == 4 && norm.is_none() =>
            {
                Self::R10SNorm
            }
            _ => return Err("Unsupported packed format or field type".into()),
        };
        Ok(result)
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::R10Uint => "r10g10b10a2_uint",
            Self::R10UNorm => "r10g10b10a2_unorm",
            Self::R10SNorm => "r10g10b10a2_snorm",
            Self::R11Float => "r11g11b10",
        }
    }
    pub fn decode(self, bits: u32) -> Vec<String> {
        if let Self::R11Float = self {
            return [
                (bits & 0x7ff, 6),
                ((bits >> 11) & 0x7ff, 6),
                ((bits >> 22) & 0x3ff, 5),
            ]
            .into_iter()
            .map(|(value, mantissa_bits)| {
                let mantissa = value & ((1 << mantissa_bits) - 1);
                let exponent = value >> mantissa_bits;
                let value = match exponent {
                    0 if mantissa == 0 => 0.0,
                    0 => (mantissa as f32 / (1u32 << mantissa_bits) as f32) * 2f32.powi(-14),
                    31 if mantissa == 0 => f32::INFINITY,
                    31 => f32::NAN,
                    _ => {
                        (1.0 + mantissa as f32 / (1u32 << mantissa_bits) as f32)
                            * 2f32.powi(exponent as i32 - 15)
                    }
                };
                value.to_string()
            })
            .collect();
        }
        [
            (bits & 1023, 10),
            ((bits >> 10) & 1023, 10),
            ((bits >> 20) & 1023, 10),
            ((bits >> 30) & 3, 2),
        ]
        .into_iter()
        .map(|(value, width)| match self {
            Self::R10Uint => value.to_string(),
            Self::R10UNorm => (value as f32 / ((1u32 << width) - 1) as f32).to_string(),
            Self::R10SNorm => {
                let signed = ((value << (32 - width)) as i32) >> (32 - width);
                (signed as f32 / ((1u32 << (width - 1)) - 1) as f32)
                    .max(-1.0)
                    .to_string()
            }
            Self::R11Float => unreachable!(),
        })
        .collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packed_formats_handle_extrema_and_special_floats() {
        assert_eq!(
            PackedFormat::R10Uint.decode(0xffff_ffff),
            ["1023", "1023", "1023", "3"]
        );
        assert_eq!(
            PackedFormat::R10UNorm.decode(0xffff_ffff),
            ["1", "1", "1", "1"]
        );
        assert_eq!(
            PackedFormat::R10SNorm.decode(0x8000_0200),
            ["-1", "0", "0", "-1"]
        );
        assert_eq!(
            PackedFormat::R11Float.decode((31 << 6) | (31 << 17) | (1 << 11)),
            ["inf", "NaN", "0"]
        );
        assert_eq!(
            PackedFormat::R11Float.decode((15 << 6) | (15 << 17) | (15 << 27)),
            ["1", "1", "1"]
        );
        assert_eq!(
            PackedFormat::R11Float.decode(1)[0].parse::<f32>().unwrap(),
            2f32.powi(-20)
        );
    }
}
