//! IEEE 754 half-precision conversion, bit-for-bit equivalent to upstream
//! `renderdoc/maths/half_convert.h::ConvertToHalf`/`ConvertFromHalf`.
//!
//! These are the exact algorithms exported as `RENDERDOC_HalfToFloat` /
//! `RENDERDOC_FloatToHalf`. Reimplementing them natively removes the need to
//! call those C++ exports, with no C++ bridge or C ABI involved.

/// Convert a 16-bit half to f32 (upstream `ConvertFromHalf`).
pub fn from_half(bits: u16) -> f32 {
    let sign = (bits & 0x8000) != 0;
    let exponent = ((bits & 0x7c00) >> 10) as i32;
    let mantissa = (bits & 0x03ff) as u32;

    if exponent == 0 {
        if mantissa == 0 {
            return if sign { -0.0 } else { 0.0 };
        }
        // subnormal: scale mantissa by 2^-24
        let value = mantissa as f32;
        let bits = value.to_bits();
        // Upstream: (sign ? 0x80000000 : 0) | (*alias - (24 << 23)); no mask.
        let adjusted = bits.wrapping_sub(24 << 23);
        f32::from_bits((if sign { 0x8000_0000 } else { 0 }) | adjusted)
    } else if exponent < 0x1f {
        let exp = exponent - 15;
        let value =
            (if sign { 0x8000_0000 } else { 0 }) | (((exp + 127) as u32) << 23) | (mantissa << 13);
        f32::from_bits(value)
    } else {
        if mantissa == 0 {
            f32::from_bits((if sign { 0x8000_0000 } else { 0 }) | 0x7f80_0000)
        } else {
            // NaN: preserve payload in the quiet-NaN range
            let payload = mantissa << 13;
            f32::from_bits(
                (if sign { 0x8000_0000 } else { 0 }) | 0x7f80_0000 | 0x0040_0000 | payload,
            )
        }
    }
}

/// Convert an f32 to a 16-bit half (upstream `ConvertToHalf`, round-to-nearest-even).
pub fn to_half(value: f32) -> u16 {
    let i = value.to_bits() as i32;
    let sign = ((i >> 16) & 0x0000_8000) as u16;
    let exponent = ((i >> 23) & 0x0000_00ff) - (127 - 15);
    let mantissa = i & 0x007f_ffff;

    if exponent <= 0 {
        if exponent < -10 {
            return sign;
        }
        let mantissa = mantissa | 0x0080_0000;
        let t = 14 - exponent;
        let a = (1 << (t - 1)) - 1;
        let b = (mantissa >> t) & 1;
        let m = (mantissa + a + b) >> t;
        (sign as i32 | m) as u16
    } else if exponent == 0xff - (127 - 15) {
        if mantissa == 0 {
            (sign as i32 | 0x7c00) as u16
        } else {
            let m = mantissa >> 13;
            (sign as i32 | 0x7c00 | m | i32::from(m == 0)) as u16
        }
    } else {
        let mut mantissa = mantissa + 0x0000_0fff + ((mantissa >> 13) & 1);
        let mut exponent = exponent;
        if mantissa & 0x0080_0000 != 0 {
            mantissa = 0;
            exponent += 1;
        }
        if exponent > 30 {
            (sign as i32 | 0x7c00) as u16
        } else {
            (sign as i32 | (exponent << 10) | (mantissa >> 13)) as u16
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_half_matches_known_ieee_values() {
        assert_eq!(from_half(0x3c00), 1.0);
        assert_eq!(from_half(0xbc00), -1.0);
        assert_eq!(from_half(0x0000), 0.0);
        assert_eq!(from_half(0x8000), -0.0);
        assert_eq!(from_half(0x7c00), f32::INFINITY);
        assert_eq!(from_half(0xfc00), f32::NEG_INFINITY);
        assert_eq!(from_half(0x3800), 0.5);
    }

    #[test]
    fn to_half_matches_known_ieee_values() {
        assert_eq!(to_half(1.0), 0x3c00);
        assert_eq!(to_half(-1.0), 0xbc00);
        assert_eq!(to_half(0.0), 0x0000);
        assert_eq!(to_half(f32::INFINITY), 0x7c00);
        assert_eq!(to_half(f32::NEG_INFINITY), 0xfc00);
        assert_eq!(to_half(0.5), 0x3800);
    }

    #[test]
    fn round_trip_representable_values() {
        for value in [1.0, -1.0, 0.5, 2.0, 3.0, -4.0, 0.125] {
            assert_eq!(from_half(to_half(value)), value);
        }
    }

    #[test]
    fn from_half_matches_half_crate_bitwise() {
        // Cross-check our upstream-exact algorithm against the mature `half`
        // crate across representative bit patterns (zero, normals, subnormals,
        // infinities and NaNs).
        const PATTERNS: [u16; 12] = [
            0x0000, 0x8000, 0x3c00, 0xbc00, 0x3800, 0x7bff, 0x0001, 0x8001, 0x03ff, 0x7c00, 0xfc00,
            0x7e00,
        ];
        for bits in PATTERNS {
            let ours_value = from_half(bits);
            let ours = ours_value.to_bits();
            let theirs = half::f16::from_bits(bits).to_f32().to_bits();
            // NaNs can carry different payloads; compare bit-exact for all others.
            if !ours_value.is_nan() {
                assert_eq!(ours, theirs, "bits {bits:#06x}");
            }
        }
    }
}
