//! Primitive vertex counts, matching upstream `RENDERDOC_NumVerticesPerPrimitive`
//! (`replay/entry_points.cpp`). This is a pure function of the topology value,
//! so RenderDocX computes it natively instead of relying on the C++ bridge's
//! `primitive_vertices` field.

use crate::replay::MeshInfo;

/// Topology enum values from `renderdoc/api/replay/replay_enums.h`.
pub mod topology {
    pub const UNKNOWN: u32 = 0;
    pub const POINT_LIST: u32 = 1;
    pub const LINE_LIST: u32 = 2;
    pub const LINE_STRIP: u32 = 3;
    pub const LINE_LOOP: u32 = 4;
    pub const TRIANGLE_LIST: u32 = 5;
    pub const TRIANGLE_STRIP: u32 = 6;
    pub const TRIANGLE_FAN: u32 = 7;
    pub const LINE_LIST_ADJ: u32 = 8;
    pub const LINE_STRIP_ADJ: u32 = 9;
    pub const TRIANGLE_LIST_ADJ: u32 = 10;
    pub const TRIANGLE_STRIP_ADJ: u32 = 11;
    pub const PATCH_LIST_1CPS: u32 = 12;
    pub const PATCH_LIST_32CPS: u32 = 43;
}

/// Number of vertices in one primitive for a topology, or `None` for unknown.
/// Patch topologies encode their control-point count as `topology - 11`.
pub fn vertices_per_primitive(topology: u32) -> Option<u32> {
    use topology::*;
    match topology {
        POINT_LIST => Some(1),
        LINE_LIST | LINE_STRIP | LINE_LOOP => Some(2),
        TRIANGLE_LIST | TRIANGLE_STRIP | TRIANGLE_FAN => Some(3),
        LINE_LIST_ADJ | LINE_STRIP_ADJ => Some(4),
        TRIANGLE_LIST_ADJ | TRIANGLE_STRIP_ADJ => Some(6),
        PATCH_LIST_1CPS..=PATCH_LIST_32CPS => Some(topology - 11),
        _ => None,
    }
}

/// Resolve the primitive size for a mesh: prefer the native computation, fall
/// back to a bridge-provided value only when the topology is unrecognised.
pub fn mesh_primitive_vertices(info: &MeshInfo) -> Option<u32> {
    vertices_per_primitive(info.topology)
        .or((info.primitive_vertices != 0).then_some(info.primitive_vertices))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_upstream_vertex_counts() {
        assert_eq!(vertices_per_primitive(topology::POINT_LIST), Some(1));
        assert_eq!(vertices_per_primitive(topology::LINE_LIST), Some(2));
        assert_eq!(vertices_per_primitive(topology::LINE_STRIP), Some(2));
        assert_eq!(vertices_per_primitive(topology::TRIANGLE_LIST), Some(3));
        assert_eq!(vertices_per_primitive(topology::TRIANGLE_STRIP), Some(3));
        assert_eq!(vertices_per_primitive(topology::LINE_LIST_ADJ), Some(4));
        assert_eq!(vertices_per_primitive(topology::TRIANGLE_LIST_ADJ), Some(6));
        assert_eq!(vertices_per_primitive(topology::PATCH_LIST_1CPS), Some(1));
        assert_eq!(vertices_per_primitive(topology::PATCH_LIST_32CPS), Some(32));
        assert_eq!(vertices_per_primitive(topology::UNKNOWN), None);
        assert_eq!(vertices_per_primitive(999), None);
    }
}
