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
    fn gpu_vendor_names() {
        assert_eq!(gpu_vendor::name(gpu_vendor::NVIDIA), "nVidia");
        assert_eq!(gpu_vendor::name(gpu_vendor::AMD), "AMD");
        assert_eq!(gpu_vendor::name(gpu_vendor::INTEL), "Intel");
        assert_eq!(gpu_vendor::name(999), "Unknown");
    }
}
