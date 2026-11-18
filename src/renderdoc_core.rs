//! Pure-Rust access to RenderDoc core's stable C ABI exports.
//!
//! `renderdoc/replay/entry_points.cpp` exports a set of `extern "C"` functions
//! with `__cdecl` calling convention on Windows (Rust's `extern "C"` matches).
//! These are stable C symbols — ABI-safe to call directly from Rust via
//! `libloading`, with **no C++ bridge compilation and no C++ ABI involved**.
//!
//! Only functions whose signature uses plain C types are exposed here:
//! `const char*`/bool/u64 in, no C++ objects or vtables out.

use libloading::Library;
use std::path::Path;

/// Information obtainable from RenderDoc's stable C exports.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CoreInfo {
    pub version: Option<String>,
    pub commit_hash: Option<String>,
    pub release_build: Option<bool>,
    pub memory_usage: Option<u64>,
}

type FnVersion = unsafe extern "C" fn() -> *const std::ffi::c_char;
type FnBool = unsafe extern "C" fn() -> bool;
type FnU64 = unsafe extern "C" fn() -> u64;

/// Call RenderDoc's stable C exports from a loaded library.
///
/// # Safety
/// `library` must be a valid RenderDoc core library matching this checkout.
pub unsafe fn core_info(library: &Library) -> CoreInfo {
    unsafe fn c_str(ptr: *const std::ffi::c_char) -> Option<String> {
        if ptr.is_null() {
            return None;
        }
        // SAFETY: caller guarantees a valid NUL-terminated string from the DLL.
        Some(
            unsafe { std::ffi::CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned(),
        )
    }

    let version = unsafe { library.get::<FnVersion>(b"RENDERDOC_GetVersionString\0") }
        .ok()
        .and_then(|f| unsafe { c_str(f()) });
    let commit_hash = unsafe { library.get::<FnVersion>(b"RENDERDOC_GetCommitHash\0") }
        .ok()
        .and_then(|f| unsafe { c_str(f()) });
    let release_build = unsafe { library.get::<FnBool>(b"RENDERDOC_IsReleaseBuild\0") }
        .ok()
        .map(|f| unsafe { f() });
    let memory_usage = unsafe { library.get::<FnU64>(b"RENDERDOC_GetCurrentProcessMemoryUsage\0") }
        .ok()
        .map(|f| unsafe { f() });

    CoreInfo {
        version,
        commit_hash,
        release_build,
        memory_usage,
    }
}

/// Load a RenderDoc core library and read its stable C-export info.
pub fn load_core_info(path: &Path) -> Result<CoreInfo, String> {
    unsafe {
        let library = Library::new(path)
            .map_err(|error| format!("Could not load {}: {error}", path.display()))?;
        Ok(core_info(&library))
    }
}

/// Well-known RenderDoc library names used to locate the core on Windows.
pub fn library_candidates() -> Vec<&'static str> {
    vec!["renderdoc.dll", "qrenderdoc.dll"]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_candidates_cover_windows_core() {
        assert!(library_candidates().contains(&"renderdoc.dll"));
    }

    #[test]
    fn missing_library_reports_error_not_panic() {
        let result = load_core_info(Path::new("definitely-missing-renderdoc.dll"));
        assert!(result.is_err());
    }

    #[test]
    fn core_info_defaults_are_all_none() {
        let info = CoreInfo::default();
        assert_eq!(
            info,
            CoreInfo {
                version: None,
                commit_hash: None,
                release_build: None,
                memory_usage: None,
            }
        );
    }
}
