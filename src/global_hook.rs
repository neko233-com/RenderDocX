//! Global capture hook, implemented natively through the Windows registry.
//!
//! Upstream `Process::StartGlobalHook` (win32_process.cpp:1195-1330) enables
//! capture for every subsequently-started process by setting the
//! `LoadAppInit_DLLs` and `AppInit_DLLs` values under
//! `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Windows`, after saving the
//! previous values so `StopGlobalHook` can restore them. This module does the
//! same with the Windows API directly; it replaces the C++-typed
//! `RENDERDOC_StartGlobalHook` entry point.
//!
//! Enabling requires administrator rights (HKLM write) and takes effect only
//! for processes started afterwards. The saved backup lets a later session
//! restore the machine even if the app was closed while the hook was active.

use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

const REG_PATH: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Windows";
const VALUE_LOAD: &str = "LoadAppInit_DLLs";
const VALUE_LIST: &str = "AppInit_DLLs";

/// Previous registry state, persisted to disk so the hook can always be undone.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct HookBackup {
    /// Whether AppInit DLL loading was enabled before the hook.
    pub load_app_init_dlls: Option<u32>,
    /// The AppInit_DLLs string before the hook.
    pub app_init_dlls: Option<String>,
}

impl HookBackup {
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(path, text).map_err(|error| format!("Could not save hook backup: {error}"))
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path)
            .map_err(|error| format!("Could not read hook backup: {error}"))?;
        serde_json::from_str(&text).map_err(|error| format!("Invalid hook backup: {error}"))
    }
}

/// The DLL path RenderDoc's global hook should load.
pub fn hook_dll(renderdoc_core: Option<&Path>) -> Option<std::path::PathBuf> {
    if let Some(core) = renderdoc_core {
        if core.is_file() {
            return Some(core.to_path_buf());
        }
    }
    // Fall back to a renderdoc.dll next to this executable, as installed.
    let next_to_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("renderdoc.dll")))?;
    next_to_exe.is_file().then_some(next_to_exe)
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::Storage::FileSystem::GetShortPathNameW;
    use windows::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_SET_VALUE, KEY_WOW64_64KEY, REG_DWORD,
        REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    };

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn open_key(write: bool) -> Result<HKEY, String> {
        let access = if write {
            KEY_QUERY_VALUE | KEY_SET_VALUE
        } else {
            KEY_QUERY_VALUE
        };
        let mut key = HKEY::default();
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(wide(REG_PATH).as_ptr()),
                0,
                access | KEY_WOW64_64KEY,
                &mut key,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!(
                "RegOpenKeyExW failed ({}). Administrator rights may be required.",
                status.0
            ));
        }
        Ok(key)
    }

    fn query_dword(key: HKEY, name: &str) -> Result<Option<u32>, String> {
        let mut kind = REG_VALUE_TYPE::default();
        let mut size = 0u32;
        let status = unsafe {
            RegQueryValueExW(
                key,
                PCWSTR(wide(name).as_ptr()),
                None,
                Some(&mut kind),
                None,
                Some(&mut size),
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err(format!("RegQueryValueExW({name}) failed ({})", status.0));
        }
        if kind != REG_DWORD || size < 4 {
            return Ok(None);
        }
        let mut data = 0u32;
        let mut size = 4u32;
        let status = unsafe {
            RegQueryValueExW(
                key,
                PCWSTR(wide(name).as_ptr()),
                None,
                Some(&mut kind),
                Some(&mut data as *mut u32 as *mut u8),
                Some(&mut size),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!("RegQueryValueExW({name}) read failed ({})", status.0));
        }
        Ok(Some(data))
    }

    fn query_sz(key: HKEY, name: &str) -> Result<Option<String>, String> {
        let mut kind = REG_VALUE_TYPE::default();
        let mut size = 0u32;
        let status = unsafe {
            RegQueryValueExW(
                key,
                PCWSTR(wide(name).as_ptr()),
                None,
                Some(&mut kind),
                None,
                Some(&mut size),
            )
        };
        if status == ERROR_FILE_NOT_FOUND || size == 0 {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err(format!("RegQueryValueExW({name}) failed ({})", status.0));
        }
        let mut buffer = vec![0u16; size as usize / 2 + 1];
        let mut size = (buffer.len() * 2) as u32;
        let status = unsafe {
            RegQueryValueExW(
                key,
                PCWSTR(wide(name).as_ptr()),
                None,
                Some(&mut kind),
                Some(buffer.as_mut_ptr() as *mut u8),
                Some(&mut size),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!("RegQueryValueExW({name}) read failed ({})", status.0));
        }
        let len = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());
        Ok(Some(String::from_utf16_lossy(&buffer[..len])))
    }

    fn set_dword(key: HKEY, name: &str, value: u32) -> Result<(), String> {
        let bytes = value.to_ne_bytes();
        let status = unsafe {
            RegSetValueExW(key, PCWSTR(wide(name).as_ptr()), 0, REG_DWORD, Some(&bytes))
        };
        if status != ERROR_SUCCESS {
            return Err(format!(
                "RegSetValueExW({name}) failed ({}). Administrator rights are required.",
                status.0
            ));
        }
        Ok(())
    }

    fn set_sz(key: HKEY, name: &str, value: &str) -> Result<(), String> {
        let data: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes =
            unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2) };
        let status = unsafe {
            RegSetValueExW(key, PCWSTR(wide(name).as_ptr()), 0, REG_SZ, Some(bytes))
        };
        if status != ERROR_SUCCESS {
            return Err(format!(
                "RegSetValueExW({name}) failed ({}). Administrator rights are required.",
                status.0
            ));
        }
        Ok(())
    }

    fn short_path(path: &Path) -> String {
        let long: Vec<u16> = path
            .as_os_str()
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut buffer = vec![0u16; 260];
        let len = unsafe { GetShortPathNameW(PCWSTR(long.as_ptr()), Some(&mut buffer)) };
        if len == 0 || len as usize >= buffer.len() {
            return path.display().to_string();
        }
        String::from_utf16_lossy(&buffer[..len as usize])
    }

    pub fn read_state() -> Result<(Option<u32>, Option<String>), String> {
        let key = open_key(false)?;
        let load = query_dword(key, VALUE_LOAD);
        let list = query_sz(key, VALUE_LIST);
        unsafe { let _ = RegCloseKey(key); }
        Ok((load?, list?))
    }

    pub fn is_active() -> bool {
        read_state()
            .ok()
            .and_then(|(_, list)| list)
            .is_some_and(|list| list.to_ascii_lowercase().contains("renderdoc"))
    }

    pub fn enable(dll: &Path) -> Result<HookBackup, String> {
        let (load, list) = read_state()?;
        let backup = HookBackup {
            load_app_init_dlls: load,
            app_init_dlls: list,
        };
        let key = open_key(true)?;
        let short = short_path(dll);
        let result = (|| {
            set_dword(key, VALUE_LOAD, 1)?;
            set_sz(key, VALUE_LIST, &short)
        })();
        unsafe { let _ = RegCloseKey(key); }
        result.map(|()| backup)
    }

    pub fn disable(backup: &HookBackup) -> Result<(), String> {
        let key = open_key(true)?;
        let result = (|| {
            set_dword(key, VALUE_LOAD, backup.load_app_init_dlls.unwrap_or(0))?;
            set_sz(key, VALUE_LIST, backup.app_init_dlls.as_deref().unwrap_or(""))
        })();
        unsafe { let _ = RegCloseKey(key); }
        result
    }
}

#[cfg(windows)]
pub use win::{disable, enable, is_active, read_state};

#[cfg(not(windows))]
pub fn is_active() -> bool {
    false
}

#[cfg(not(windows))]
pub fn enable(_dll: &Path) -> Result<HookBackup, String> {
    Err("The global capture hook is only implemented on Windows".to_owned())
}

#[cfg(not(windows))]
pub fn disable(_backup: &HookBackup) -> Result<(), String> {
    Err("The global capture hook is only implemented on Windows".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_round_trips_through_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hook.json");
        let backup = HookBackup {
            load_app_init_dlls: Some(1),
            app_init_dlls: Some("old.dll".to_owned()),
        };
        backup.save(&path).unwrap();
        assert_eq!(HookBackup::load(&path).unwrap(), backup);
    }

    #[test]
    fn missing_or_invalid_backup_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        assert!(HookBackup::load(&dir.path().join("absent.json")).is_err());
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "not json").unwrap();
        assert!(HookBackup::load(&bad).is_err());
    }

    #[test]
    fn hook_dll_prefers_an_existing_configured_core() {
        let dir = tempfile::tempdir().unwrap();
        let dll = dir.path().join("renderdoc.dll");
        std::fs::write(&dll, b"stub").unwrap();
        assert_eq!(hook_dll(Some(&dll)), Some(dll));
        assert_eq!(hook_dll(Some(&dir.path().join("missing.dll"))), None);
    }
}
