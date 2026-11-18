//! Pure-Rust translations of RenderDoc's `GraphicsAPI` and `GPUVendor`
//! enums (`renderdoc/api/replay/replay_enums.h`), used to interpret the API
//! and vendor integers exposed by the replay core / `.rdc` metadata.

/// `GraphicsAPI` enum values (enum class : uint32_t).
pub mod graphics_api {
    pub const D3D11: u32 = 0;
    pub const D3D12: u32 = 1;
    pub const OPENGL: u32 = 2;
    pub const VULKAN: u32 = 3;

    pub fn name(api: u32) -> &'static str {
        match api {
            D3D11 => "D3D11",
            D3D12 => "D3D12",
            OPENGL => "OpenGL",
            VULKAN => "Vulkan",
            _ => "Unknown",
        }
    }

    /// Upstream `IsD3D(GraphicsAPI)`.
    pub fn is_d3d(api: u32) -> bool {
        matches!(api, D3D11 | D3D12)
    }
}

/// `GPUVendor` enum values (enum class : uint32_t).
pub mod gpu_vendor {
    pub const UNKNOWN: u32 = 0;
    pub const ARM: u32 = 1;
    pub const AMD: u32 = 2;
    pub const BROADCOM: u32 = 3;
    pub const IMAGINATION: u32 = 4;
    pub const INTEL: u32 = 5;
    pub const NVIDIA: u32 = 6;
    pub const QUALCOMM: u32 = 7;
    pub const VERISILICON: u32 = 8;
    pub const SOFTWARE: u32 = 9;
    pub const SAMSUNG: u32 = 10;

    pub fn name(vendor: u32) -> &'static str {
        match vendor {
            UNKNOWN => "Unknown",
            ARM => "ARM",
            AMD => "AMD",
            BROADCOM => "Broadcom",
            IMAGINATION => "Imagination",
            INTEL => "Intel",
            NVIDIA => "nVidia",
            QUALCOMM => "Qualcomm",
            VERISILICON => "Verisilicon",
            SOFTWARE => "Software",
            SAMSUNG => "Samsung",
            _ => "Unknown",
        }
    }
}

/// Topology enum values / names from `replay_enums.h`, used by the mesh and
/// pipeline views. Numeric values match `enum class Topology : uint32_t`.
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
    pub const PATCH_LIST: u32 = 12;

    pub fn name(topology: u32) -> &'static str {
        match topology {
            UNKNOWN => "Unknown",
            POINT_LIST => "PointList",
            LINE_LIST => "LineList",
            LINE_STRIP => "LineStrip",
            LINE_LOOP => "LineLoop",
            TRIANGLE_LIST => "TriList",
            TRIANGLE_STRIP => "TriStrip",
            TRIANGLE_FAN => "TriFan",
            LINE_LIST_ADJ => "LineList_Adj",
            LINE_STRIP_ADJ => "LineStrip_Adj",
            TRIANGLE_LIST_ADJ => "TriList_Adj",
            TRIANGLE_STRIP_ADJ => "TriStrip_Adj",
            PATCH_LIST..=43 => "PatchList",
            _ => "Unknown",
        }
    }
}

/// LogType enum values / names from `replay_enums.h`.
pub mod log_type {
    pub const DEBUG: u32 = 0;
    pub const COMMENT: u32 = 1;
    pub const WARNING: u32 = 2;
    pub const ERROR: u32 = 3;
    pub const FATAL: u32 = 4;

    pub fn name(log_type: u32) -> &'static str {
        match log_type {
            DEBUG => "Debug",
            COMMENT => "Comment",
            WARNING => "Warning",
            ERROR => "Error",
            FATAL => "Fatal",
            _ => "Unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphics_api_names_and_is_d3d() {
        assert_eq!(graphics_api::name(graphics_api::D3D11), "D3D11");
        assert_eq!(graphics_api::name(graphics_api::VULKAN), "Vulkan");
        assert_eq!(graphics_api::name(999), "Unknown");
        assert!(graphics_api::is_d3d(graphics_api::D3D11));
        assert!(graphics_api::is_d3d(graphics_api::D3D12));
        assert!(!graphics_api::is_d3d(graphics_api::OPENGL));
        assert!(!graphics_api::is_d3d(graphics_api::VULKAN));
    }

    #[test]
    fn topology_and_log_type_names() {
        assert_eq!(topology::name(topology::TRIANGLE_LIST), "TriList");
        assert_eq!(topology::name(topology::POINT_LIST), "PointList");
        assert_eq!(topology::name(20), "PatchList");
        assert_eq!(topology::name(999), "Unknown");
        assert_eq!(log_type::name(log_type::WARNING), "Warning");
        assert_eq!(log_type::name(log_type::FATAL), "Fatal");
        assert_eq!(log_type::name(999), "Unknown");
    }

    #[test]
    fn gpu_vendor_names() {
        assert_eq!(gpu_vendor::name(gpu_vendor::NVIDIA), "nVidia");
        assert_eq!(gpu_vendor::name(gpu_vendor::AMD), "AMD");
        assert_eq!(gpu_vendor::name(gpu_vendor::INTEL), "Intel");
        assert_eq!(gpu_vendor::name(999), "Unknown");
    }
}
