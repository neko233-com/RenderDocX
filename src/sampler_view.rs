//! Presentation of the version-matched RenderDoc sampler descriptor protocol.
use crate::i18n::Language;
use serde_json::Value;

fn number(value: Option<&Value>) -> Option<u32> {
    value?.as_u64().and_then(|value| u32::try_from(value).ok())
}

fn component(value: &Value, key: &str, index: usize) -> Option<u32> {
    number(value.get(key)?.as_array()?.get(index))
}

fn enum_name(value: Option<u32>, names: &[&str]) -> String {
    value
        .map(|value| {
            names
                .get(value as usize)
                .map_or_else(|| format!("#{value}"), |name| (*name).to_owned())
        })
        .unwrap_or_else(|| "—".into())
}

fn float_bits(value: Option<&Value>) -> String {
    number(value)
        .map(|bits| f32::from_bits(bits).to_string())
        .unwrap_or_else(|| "—".into())
}

fn lod_bits(value: Option<&Value>, minimum: bool) -> String {
    let Some(bits) = number(value) else {
        return "—".into();
    };
    let value = f32::from_bits(bits);
    if minimum && value == -f32::MAX {
        "0".into()
    } else if !minimum && value == f32::MAX {
        "FLT_MAX".into()
    } else if (minimum && value == -1000.0) || (!minimum && value == 1000.0) {
        "VK_LOD_CLAMP_NONE".into()
    } else {
        value.to_string()
    }
}

fn boolean(value: Option<&Value>, language: Language) -> &'static str {
    match value.and_then(Value::as_bool) {
        Some(true) => language.text("Yes", "是"),
        Some(false) => language.text("No", "否"),
        None => "—",
    }
}

fn border(sampler: &Value) -> Option<String> {
    let values = sampler.get("border_bits")?.as_array()?;
    if values.len() != 4 {
        return None;
    }
    let bits: Vec<u32> = values
        .iter()
        .map(|value| number(Some(value)))
        .collect::<Option<_>>()?;
    let kind = number(sampler.get("border_type"));
    let values = bits
        .into_iter()
        .map(|bits| match kind {
            Some(1) => f32::from_bits(bits).to_string(),
            Some(4) => bits.to_string(),
            Some(5) => (bits as i32).to_string(),
            _ => format!("0x{bits:08X}"),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let kind = match kind {
        Some(1) => "Float".into(),
        Some(4) => "UInt".into(),
        Some(5) => "SInt".into(),
        Some(kind) => format!("#{kind}"),
        None => "—".into(),
    };
    Some(format!("{kind}: [{values}]"))
}

/// None means the bridge has no valid sampler descriptor. Unknown enum values stay numeric.
pub fn lines(sampler: &Value, language: Language) -> Option<Vec<String>> {
    if !matches!(number(sampler.get("type")), Some(2 | 3)) {
        return None;
    }
    const ADDRESS: [&str; 5] = ["Wrap", "Mirror", "MirrorOnce", "ClampEdge", "ClampBorder"];
    const FILTER: [&str; 5] = ["NoFilter", "Point", "Linear", "Cubic", "Anisotropic"];
    const FUNCTION: [&str; 4] = ["Normal", "Comparison", "Minimum", "Maximum"];
    const COMPARE: [&str; 8] = [
        "Never",
        "AlwaysTrue",
        "Less",
        "LessEqual",
        "Greater",
        "GreaterEqual",
        "Equal",
        "NotEqual",
    ];
    let mut lines = vec![
        format!(
            "{} U/V/W: {} / {} / {}",
            language.text("Address", "寻址"),
            enum_name(component(sampler, "address", 0), &ADDRESS),
            enum_name(component(sampler, "address", 1), &ADDRESS),
            enum_name(component(sampler, "address", 2), &ADDRESS)
        ),
        format!(
            "{}: {} / {} / {} · {}: {} · {}: {}",
            language.text(
                "Min/Mag/Mip filter",
                "缩小/放大/Mip 过滤",
            ),
            enum_name(component(sampler, "filter", 0), &FILTER),
            enum_name(component(sampler, "filter", 1), &FILTER),
            enum_name(component(sampler, "filter", 2), &FILTER),
            language.text("Function", "函数"),
            enum_name(component(sampler, "filter", 3), &FUNCTION),
            language.text("Compare", "比较"),
            enum_name(number(sampler.get("compare")), &COMPARE)
        ),
        format!(
            "LOD: {} – {} · {}: {} · {}: {}",
            lod_bits(sampler.get("min_lod_bits"), true),
            lod_bits(sampler.get("max_lod_bits"), false),
            language.text("Mip bias", "Mip 偏移"),
            float_bits(sampler.get("mip_bias_bits")),
            language.text(
                "Max anisotropy",
                "最大各向异性",
            ),
            float_bits(sampler.get("max_anisotropy_bits"))
        ),
        format!(
            "{}: {} · {}: {} · {}: {} · {}: {}",
            language.text("Immutable", "不可变"),
            boolean(sampler.get("creation_time_constant"), language),
            language.text(
                "Seamless cubemaps",
                "无缝立方体贴图",
            ),
            boolean(sampler.get("seamless_cubemaps"), language),
            language.text("Unnormalized", "非归一化"),
            boolean(sampler.get("unnormalized"), language),
            language.text("sRGB border", "sRGB 边界"),
            boolean(sampler.get("srgb_border"), language)
        ),
    ];
    if let Some(value) = border(sampler) {
        lines.push(format!(
            "{}: {value}",
            language.text("Border color", "边界颜色")
        ));
    }
    if let Some(swizzle) = crate::texture_view::descriptor_swizzle(sampler) {
        lines.push(format!(
            "{}: {swizzle}",
            language.text(
                "Sampler swizzle",
                "采样器通道映射",
            )
        ));
    }
    if let Some(ycbcr) = sampler.get("ycbcr").filter(|value| value.is_object()) {
        const MODEL: [&str; 5] = ["Raw", "RangeOnly", "BT709", "BT601", "BT2020"];
        const RANGE: [&str; 2] = ["ITUFull", "ITUNarrow"];
        const CHROMA: [&str; 2] = ["CositedEven", "Midpoint"];
        lines.push(format!(
            "YCbCr: {} · {} · {}: {} [{}, {}] · {}: {}",
            enum_name(number(ycbcr.get("model")), &MODEL),
            enum_name(number(ycbcr.get("range")), &RANGE),
            language.text("Chroma filter", "色度过滤"),
            enum_name(number(ycbcr.get("chroma_filter")), &FILTER),
            enum_name(component(ycbcr, "chroma_offsets", 0), &CHROMA),
            enum_name(component(ycbcr, "chroma_offsets", 1), &CHROMA),
            language.text(
                "Explicit reconstruction",
                "显式重建",
            ),
            boolean(ycbcr.get("force_explicit_reconstruction"), language)
        ));
    }
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sampler_decoding_keeps_types_missing_values_and_nonfinite_bits() {
        let mut sampler = serde_json::json!({"type":2,"address":[0,4,99],"filter":[1,2,0,1],"compare":2,"min_lod_bits":(-1000.0f32).to_bits(),"max_lod_bits":f32::MAX.to_bits(),"mip_bias_bits":f32::INFINITY.to_bits(),"border_type":5,"border_bits":[u32::MAX,0,1,2],"unnormalized":true,"ycbcr":{"model":2,"range":0,"chroma_filter":1,"chroma_offsets":[0,1],"force_explicit_reconstruction":true}});
        let lines = lines(&sampler, Language::English).unwrap().join("\n");
        assert!(lines.contains("Wrap / ClampBorder / #99"));
        assert!(lines.contains("Point / Linear / NoFilter"));
        assert!(lines.contains("VK_LOD_CLAMP_NONE – FLT_MAX"));
        assert!(lines.contains("Mip bias: inf"));
        assert!(lines.contains("SInt: [-1, 0, 1, 2]"));
        assert!(lines.contains("BT709 · ITUFull"));
        assert!(lines.contains("Immutable: —"));
        sampler["type"] = 0.into();
        assert!(super::lines(&sampler, Language::English).is_none());
        sampler["type"] = 2.into();
        sampler["border_type"] = 99.into();
        assert!(
            super::lines(&sampler, Language::English)
                .unwrap()
                .join("\n")
                .contains("#99: [0xFFFFFFFF")
        );
        sampler["border_bits"] = serde_json::json!([0, 1]);
        assert!(
            !super::lines(&sampler, Language::English)
                .unwrap()
                .join("\n")
                .contains("Border color")
        );
    }
}
