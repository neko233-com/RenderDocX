//! Decode regular floating-point mesh elements using the format reported by RenderDoc.
use crate::replay::MeshInfo;
use serde::Serialize;

#[derive(Debug, PartialEq, Serialize)]
pub struct MeshPosition {
    pub row: String,
    pub values: Vec<f32>,
}

pub fn supported(info: &MeshInfo) -> bool {
    info.format_type == 0
        && info.component_type == 1
        && matches!(info.component_byte_width, 2 | 4)
        && (1..=4).contains(&info.component_count)
        && info.vertex_stride > 0
        && info.component_count * info.component_byte_width <= info.vertex_stride
}

/// Returns complete elements starting in this byte range; incomplete trailing elements are omitted.
pub fn decode(info: &MeshInfo, bytes: &[u8], offset: u64) -> Option<Vec<MeshPosition>> {
    if !supported(info) {
        return None;
    }
    let stride = u64::from(info.vertex_stride);
    let mut relative = (stride - offset % stride) % stride;
    let width = u64::from(info.component_count) * u64::from(info.component_byte_width);
    let component_width = info.component_byte_width as usize;
    let mut positions = Vec::new();
    while relative.checked_add(width)? <= bytes.len() as u64 {
        let start = usize::try_from(relative).ok()?;
        let values = (0..info.component_count as usize)
            .map(|index| {
                let i = start + index * component_width;
                if component_width == 2 {
                    half::f16::from_bits(u16::from_le_bytes(bytes[i..i + 2].try_into().unwrap()))
                        .to_f32()
                } else {
                    f32::from_le_bytes(bytes[i..i + 4].try_into().unwrap())
                }
            })
            .collect();
        positions.push(MeshPosition {
            row: (offset.checked_add(relative)? / stride).to_string(),
            values,
        });
        relative = relative.checked_add(stride)?;
    }
    Some(positions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_unprojected_and_projected_positions_at_page_boundaries() {
        let mut info = MeshInfo {
            vertex_stride: 12,
            component_type: 1,
            component_byte_width: 4,
            component_count: 2,
            ..Default::default()
        };
        let mut bytes = Vec::new();
        for n in 0..3 {
            bytes.extend((n as f32).to_le_bytes());
            bytes.extend(2.0f32.to_le_bytes());
            bytes.extend([0; 4]);
        }
        let result = decode(&info, &bytes[4..], 4).unwrap();
        assert_eq!(
            result
                .iter()
                .map(|p| (p.row.as_str(), p.values.clone()))
                .collect::<Vec<_>>(),
            [("1", vec![1.0, 2.0]), ("2", vec![2.0, 2.0])]
        );
        info.unproject = true;
        assert_eq!(decode(&info, &bytes[4..], 4).unwrap(), result);
        info.component_byte_width = 8;
        assert!(decode(&info, &bytes, 0).is_none());
    }

    #[test]
    fn decodes_half_float_components_with_padding() {
        let info = MeshInfo {
            vertex_stride: 8,
            component_type: 1,
            component_byte_width: 2,
            component_count: 3,
            ..Default::default()
        };
        let bytes = [0x00, 0x3c, 0x00, 0xc0, 0x00, 0x38, 0, 0];
        assert_eq!(
            decode(&info, &bytes, 0).unwrap()[0].values,
            [1.0, -2.0, 0.5]
        );
        assert!(decode(&info, &bytes[..5], 0).unwrap().is_empty());
    }
}
