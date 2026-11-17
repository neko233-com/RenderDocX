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

/// Convert top-left normalized coordinates to a selected mip texel.
pub fn uv_to_texel(base: [u32; 2], mip: u32, uv: [f64; 2]) -> Option<[u32; 2]> {
    if base.contains(&0) || mip >= 32 || uv.iter().any(|v| !v.is_finite() || *v < 0.0 || *v >= 1.0)
    {
        return None;
    }
    Some(std::array::from_fn(|axis| {
        let size = (base[axis] >> mip).max(1);
        ((uv[axis] * f64::from(size)).floor() as u32).min(size - 1)
    }))
}

/// Fit the native display extent into an output viewport.
pub fn fitted_transform(extent: [u32; 2], viewport: [u32; 2]) -> Option<(f32, [f32; 2])> {
    if extent.contains(&0) || viewport.contains(&0) {
        return None;
    }
    let scale = (viewport[0] as f32 / extent[0] as f32).min(viewport[1] as f32 / extent[1] as f32);
    Some((
        scale,
        [
            (viewport[0] as f32 - extent[0] as f32 * scale) / 2.0,
            (viewport[1] as f32 - extent[1] as f32 * scale) / 2.0,
        ],
    ))
}

/// Match RenderDoc's eight-pixel context zoom including odd-sized mip adjustment.
pub fn context_scale(base: [u32; 2], mip: u32) -> Option<f32> {
    if base.contains(&0) || mip >= 32 {
        return None;
    }
    let mip_width = (base[0] >> mip).max(1) as f32;
    let mip_height = (base[1] >> mip).max(1) as f32;
    Some(8.0 * (mip_width / base[0] as f32).max(mip_height / base[1] as f32))
}

/// Place the selected mip texel's upper-left corner at the context viewport center.
pub fn context_offset(
    base: [u32; 2],
    extent: [u32; 2],
    mip: u32,
    texel: [u32; 2],
    flip_y: bool,
    scale: f32,
    viewport: [u32; 2],
) -> Option<[f32; 2]> {
    let mut offset = center_texel_in_viewport(base, extent, mip, texel, flip_y, scale, viewport)?;
    for axis in 0..2 {
        let mip_size = (base[axis] >> mip).max(1);
        let value = f64::from(offset[axis])
            + 0.5 * f64::from(extent[axis]) / f64::from(mip_size) * f64::from(scale);
        if !value.is_finite() || value.abs() > f64::from(f32::MAX) {
            return None;
        }
        offset[axis] = value as f32;
    }
    Some(offset)
}

/// Center a texel in the 512×360 headless preview at the current scale.
pub fn center_texel(
    base: [u32; 2],
    extent: [u32; 2],
    mip: u32,
    texel: [u32; 2],
    flip_y: bool,
    scale: f32,
) -> Option<[f32; 2]> {
    center_texel_in_viewport(base, extent, mip, texel, flip_y, scale, [512, 360])
}

/// Center a selected texel in an arbitrary native preview output.
pub fn center_texel_in_viewport(
    base: [u32; 2],
    extent: [u32; 2],
    mip: u32,
    texel: [u32; 2],
    flip_y: bool,
    scale: f32,
    viewport: [u32; 2],
) -> Option<[f32; 2]> {
    if base.contains(&0)
        || extent.contains(&0)
        || viewport.contains(&0)
        || mip >= 32
        || !scale.is_finite()
        || scale <= 0.0
    {
        return None;
    }
    let mut offset = [0.0; 2];
    for axis in 0..2 {
        let size = (base[axis] >> mip).max(1);
        if texel[axis] >= size {
            return None;
        }
        let coordinate = if axis == 1 && flip_y {
            size - 1 - texel[axis]
        } else {
            texel[axis]
        };
        let position = (coordinate as f64 + 0.5) * f64::from(extent[axis]) / f64::from(size);
        let target = f64::from(viewport[axis]) / 2.0;
        let value = target - position * f64::from(scale);
        if !value.is_finite() || value.abs() > f64::from(f32::MAX) {
            return None;
        }
        offset[axis] = value as f32;
    }
    Some(offset)
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
    fn uv_and_center_handle_odd_mips_flips_and_invalid_input() {
        assert_eq!(super::context_scale([16, 8], 0), Some(8.0));
        assert_eq!(super::context_scale([16, 8], 1), Some(4.0));
        assert!((super::context_scale([15, 9], 1).unwrap() - 8.0 * 7.0 / 15.0).abs() < 0.0001);
        assert_eq!(super::context_scale([0, 8], 0), None);
        assert_eq!(super::context_scale([16, 8], 32), None);
        let context =
            super::context_offset([2, 2], [2, 2], 0, [0, 1], true, 8.0, [160, 160]).unwrap();
        assert_eq!(context, [80.0, 80.0]);
        assert_eq!(
            texel_at([84.0, 84.0], 8.0, context, [2, 2], [2, 2], 0, true),
            Some([0, 1])
        );
        let scale = super::context_scale([15, 9], 1).unwrap();
        let context =
            super::context_offset([15, 9], [15, 9], 1, [6, 2], false, scale, [160, 160]).unwrap();
        assert_eq!(
            texel_at([84.0, 84.0], scale, context, [15, 9], [15, 9], 1, false),
            Some([6, 2])
        );
        assert_eq!(
            super::fitted_transform([2, 2], [64, 64]),
            Some((32.0, [0.0, 0.0]))
        );
        assert_eq!(
            super::center_texel_in_viewport([2, 2], [2, 2], 0, [0, 1], true, 32.0, [64, 64]),
            Some([16.0, 16.0])
        );
        assert!(super::fitted_transform([0, 2], [64, 64]).is_none());
        assert_eq!(super::uv_to_texel([15, 9], 1, [0.999, 0.5]), Some([6, 2]));
        assert_eq!(super::uv_to_texel([15, 9], 1, [1.0, 0.5]), None);
        assert_eq!(super::uv_to_texel([15, 9], 1, [f64::NAN, 0.5]), None);
        for flip in [false, true] {
            let offset = super::center_texel([15, 9], [15, 9], 1, [6, 2], flip, 3.0).unwrap();
            assert_eq!(
                texel_at([256.0, 180.0], 3.0, offset, [15, 9], [15, 9], 1, flip),
                Some([6, 2])
            );
        }
        let offset = super::center_texel([16, 1], [16, 100], 0, [3, 0], true, 2.0).unwrap();
        assert_eq!(
            texel_at([256.0, 180.0], 2.0, offset, [16, 1], [16, 100], 0, true),
            Some([3, 0])
        );
        assert!(super::center_texel([16, 1], [16, 100], 0, [16, 0], false, 2.0).is_none());
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
