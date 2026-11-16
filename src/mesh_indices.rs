//! Decode RenderDoc post-transform index buffers without changing their byte order.
use crate::replay::MeshInfo;
use serde::Serialize;

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct MeshIndex {
    pub row: String,
    pub raw: u32,
    pub restart: bool,
    pub vertex: Option<u32>,
}

pub fn decode(info: &MeshInfo, bytes: &[u8], offset: u64) -> Option<Vec<MeshIndex>> {
    let stride = usize::try_from(info.index_stride).ok()?;
    if !matches!(stride, 1 | 2 | 4) || offset % stride as u64 != 0 || bytes.len() % stride != 0 {
        return None;
    }
    bytes
        .chunks_exact(stride)
        .enumerate()
        .map(|(i, bytes)| {
            let raw = match stride {
                1 => u32::from(bytes[0]),
                2 => u32::from(u16::from_le_bytes(bytes.try_into().ok()?)),
                4 => u32::from_le_bytes(bytes.try_into().ok()?),
                _ => return None,
            };
            // BufferViewer checks restart before applying the base, and treats zero as disabled.
            let is_restart = info.index_restart == Some(raw);
            let vertex = (!is_restart).then(|| {
                if info.base_vertex < 0 {
                    raw.saturating_sub(info.base_vertex.unsigned_abs())
                } else {
                    raw.wrapping_add(info.base_vertex as u32)
                }
            });
            Some(MeshIndex {
                row: (offset / stride as u64).checked_add(i as u64)?.to_string(),
                raw,
                restart: is_restart,
                vertex,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_precedes_base_and_negative_base_clamps() {
        let info = MeshInfo {
            index_stride: 2,
            index_restart: Some(65535),
            base_vertex: -3,
            ..Default::default()
        };
        let values = decode(&info, &[1, 0, 8, 0, 255, 255], 4).unwrap();
        assert_eq!(
            values
                .iter()
                .map(|x| (x.row.as_str(), x.vertex, x.restart))
                .collect::<Vec<_>>(),
            [
                ("2", Some(0), false),
                ("3", Some(5), false),
                ("4", None, true)
            ]
        );
        assert!(decode(&info, &[1], 0).is_none());
        assert!(decode(&info, &[1, 0], 1).is_none());
    }
}
