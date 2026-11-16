use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SliceLayout {
    #[default]
    Selected,
    All,
    Grid,
    Cubemap,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AlphaMapping {
    Discard,
    BlendToColor,
    BlendToCheckerboard,
    #[default]
    Preserve,
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct TextureExportOptions {
    pub all_mips: bool,
    pub slices: SliceLayout,
    pub grid_width: u32,
    pub samples_as_array: bool,
    pub alpha: AlphaMapping,
    pub alpha_color: [f32; 4],
    pub jpeg_quality: u32,
}
impl Default for TextureExportOptions {
    fn default() -> Self {
        Self {
            all_mips: false,
            slices: SliceLayout::Selected,
            grid_width: 1,
            samples_as_array: false,
            alpha: AlphaMapping::Preserve,
            alpha_color: [0.0, 0.0, 0.0, 1.0],
            jpeg_quality: 90,
        }
    }
}
impl TextureExportOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.grid_width == 0
            || self.grid_width > i32::MAX as u32
            || !(1..=100).contains(&self.jpeg_quality)
            || self
                .alpha_color
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err("Invalid texture export options".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sparse_options_default_and_invalid_values_are_rejected() {
        let opts: TextureExportOptions =
            serde_json::from_str(r#"{"slices":"grid","grid_width":2}"#).unwrap();
        assert_eq!(opts.slices, SliceLayout::Grid);
        assert_eq!(opts.jpeg_quality, 90);
        assert!(opts.validate().is_ok());
        assert!(
            TextureExportOptions {
                jpeg_quality: 0,
                ..opts
            }
            .validate()
            .is_err()
        );
        assert!(
            TextureExportOptions {
                grid_width: 0,
                ..opts
            }
            .validate()
            .is_err()
        );
        assert!(
            TextureExportOptions {
                alpha_color: [f32::NAN, 0.0, 0.0, 1.0],
                ..opts
            }
            .validate()
            .is_err()
        );
    }
}
