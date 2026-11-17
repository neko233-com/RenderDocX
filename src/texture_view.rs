//! Coordinate mapping for RenderDoc's base-sized texture display.

/// RenderDoc uses a signed default range for SNorm source formats or casts.
pub fn default_range(component_type: u32, type_cast: u32) -> [f32; 2] {
    if component_type == 3 || type_cast == 3 {
        [-1.0, 1.0]
    } else {
        [0.0, 1.0]
    }
}

/// Fit the range to enabled channels in a RenderDoc min/max result.
pub fn fit_range(
    stats: &crate::replay::TextureStatistics,
    component_type: u32,
    type_cast: u32,
    component_count: u32,
    channels: [bool; 4],
    alpha_only: bool,
) -> Result<[f32; 2], &'static str> {
    let effective = if stats.custom_shader {
        1
    } else if component_type == 0 && matches!(type_cast, 4 | 5) {
        type_cast
    } else {
        component_type
    };
    let mut min = f32::MAX;
    let mut max = -f32::MAX;
    let mut found = false;
    for index in 0..4 {
        if !channels[index]
            || (index >= component_count as usize
                && !(alpha_only && !stats.custom_shader && index == 3))
        {
            continue;
        }
        let (lo, hi) = match effective {
            4 => (
                stats.min.uint_values[index] as f32,
                stats.max.uint_values[index] as f32,
            ),
            5 => (
                stats.min.int_values[index] as f32,
                stats.max.int_values[index] as f32,
            ),
            _ => (
                f32::from_bits(stats.min.bits[index]),
                f32::from_bits(stats.max.bits[index]),
            ),
        };
        if !lo.is_finite() || !hi.is_finite() {
            continue;
        }
        min = min.min(lo);
        max = max.max(hi);
        found = true;
    }
    if !found {
        return Err("No enabled channel has finite extrema");
    }
    // Matches RangeHistogram::setRange's minimum width.
    let floor = if min < 0.0 {
        (min - f32::EPSILON) * (1.0 - 1.0e-6)
    } else {
        (min + f32::EPSILON) * (1.0 + 1.0e-6)
    };
    max = max.max(floor);
    if !max.is_finite() || min >= max || !(max - min).is_finite() {
        return Err("Cannot represent fitted range");
    }
    Ok([min, max])
}

/// RenderDoc D3D replay draws 1D textures as 100-pixel-high strips.
/// Other renderer behavior is deliberately left unknown until verified.
pub fn display_extent(base: [u32; 2], dimension: u32, renderer: Option<u32>) -> Option<[u32; 2]> {
    if base.contains(&0) {
        return None;
    }
    if dimension == 1 {
        return match renderer {
            Some(0 | 1) => Some([base[0], 100]),
            _ => None,
        };
    }
    Some(base)
}

/// Map a point in the 512×360 preview output to a selected mip texel.
/// The preview displays the base extent, even when a smaller mip is selected.
pub fn texel_at(
    output: [f32; 2],
    scale: f32,
    offset: [f32; 2],
    base: [u32; 2],
    extent: [u32; 2],
    mip: u32,
    flip_y: bool,
) -> Option<[u32; 2]> {
    if !scale.is_finite()
        || scale <= 0.0
        || base.contains(&0)
        || extent.contains(&0)
        || !output.iter().chain(offset.iter()).all(|v| v.is_finite())
    {
        return None;
    }
    let mut coords = [0u32; 2];
    for axis in 0..2 {
        let position = (output[axis] - offset[axis]) / scale;
        if position < 0.0 || position >= extent[axis] as f32 {
            return None;
        }
        let mut pixel = (position * base[axis] as f32 / extent[axis] as f32).floor() as u32;
        pixel = pixel.min(base[axis] - 1);
        if axis == 1 && flip_y {
            pixel = base[axis] - 1 - pixel;
        }
        let mip_dim = base[axis].checked_shr(mip).unwrap_or(0).max(1);
        // Matches MipCoordFromBase in qrenderdoc, including its float error allowance.
        coords[axis] =
            ((mip_dim as f32 * (pixel as f32 / base[axis] as f32 + 1e-6)) as u32).min(mip_dim - 1);
    }
    Some(coords)
}

#[cfg(test)]
mod tests {
    use super::{display_extent, texel_at};
    #[test]
    fn fits_enabled_channels_with_integer_types_and_handles_constants() {
        use crate::replay::{HistogramRequest, TextureStatistics};
        let mut stats = TextureStatistics {
            custom_shader: false,
            event_id: 1,
            index: 0,
            mip: 0,
            slice: 0,
            sample: 0,
            request: HistogramRequest {
                min: 0.0,
                max: 1.0,
                channels: [true; 4],
            },
            min: [(-5i32) as u32, 3, 0, 0].into(),
            max: [10, 8, 0, 0].into(),
            histogram: vec![],
        };
        assert_eq!(super::default_range(3, 0), [-1.0, 1.0]);
        assert_eq!(super::default_range(0, 3), [-1.0, 1.0]);
        assert_eq!(super::default_range(2, 0), [0.0, 1.0]);
        assert_eq!(
            super::fit_range(&stats, 5, 0, 2, [true, true, false, false], false).unwrap(),
            [-5.0, 10.0]
        );
        assert_eq!(
            super::fit_range(&stats, 0, 4, 2, [false, true, false, false], false).unwrap(),
            [3.0, 8.0]
        );
        assert!(super::fit_range(&stats, 5, 0, 2, [false; 4], false).is_err());
        stats.min = [1f32.to_bits(); 4].into();
        stats.max = [1f32.to_bits(); 4].into();
        stats.custom_shader = true;
        let range = super::fit_range(&stats, 4, 0, 1, [true; 4], false).unwrap();
        assert_eq!(range[0], 1.0);
        assert!(range[1] > 1.0);
    }

    #[test]
    fn maps_d3d_1d_strip_and_rejects_unknown_renderer() {
        assert_eq!(display_extent([16, 1], 1, Some(0)), Some([16, 100]));
        assert_eq!(display_extent([16, 1], 1, Some(1)), Some([16, 100]));
        assert_eq!(display_extent([16, 1], 1, None), None);
        assert_eq!(display_extent([16, 1], 1, Some(3)), None);
        assert_eq!(
            texel_at([8.5, 99.5], 1.0, [0.0, 0.0], [16, 1], [16, 100], 0, true),
            Some([8, 0])
        );
        assert_eq!(
            texel_at([8.5, 100.0], 1.0, [0.0, 0.0], [16, 1], [16, 100], 0, false),
            None
        );
    }

    #[test]
    fn maps_base_extent_to_odd_mip_and_flipped_rows() {
        assert_eq!(
            texel_at([14.5, 8.5], 1.0, [0.0, 0.0], [15, 9], [15, 9], 1, false),
            Some([6, 3])
        );
        assert_eq!(
            texel_at([14.5, 0.5], 1.0, [0.0, 0.0], [15, 9], [15, 9], 1, true),
            Some([6, 3])
        );
        assert_eq!(
            texel_at([9.0, 20.0], 2.0, [10.0, 20.0], [15, 9], [15, 9], 1, false),
            None
        );
        assert_eq!(
            texel_at([40.0, 20.0], 2.0, [10.0, 20.0], [15, 9], [15, 9], 1, false),
            None
        );
        assert_eq!(
            texel_at([12.0, 22.0], 0.0, [10.0, 20.0], [15, 9], [15, 9], 1, false),
            None
        );
    }
}
