//! Coordinate mapping for RenderDoc's base-sized texture display.

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
