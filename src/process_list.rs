//! Pure-Rust process enumeration via the Windows Toolhelp32 snapshot API.
//!
//! This replaces qrenderdoc's process-list dependency for the "attach to a
//! running instance" workflow. Uses only the `windows` crate (Win32 API), no
//! C++ bridge and no RenderDoc core.

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
}

/// Enumerate processes visible to the current user, sorted by name.
#[cfg(windows)]
pub fn enumerate_processes() -> Result<Vec<ProcessInfo>, String> {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32First, Process32Next, PROCESSENTRY32, TH32CS_SNAPPROCESS,
    };

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
            .map_err(|error| format!("Could not create process snapshot: {error}"))?;
        let mut entry = PROCESSENTRY32 {
            dwSize: std::mem::size_of::<PROCESSENTRY32>() as u32,
            ..Default::default()
        };
        let mut processes = Vec::new();
        if Process32First(snapshot, &mut entry).is_ok() {
            loop {
                let name_bytes: Vec<u8> = entry
                    .szExeFile
                    .iter()
                    .take_while(|byte| **byte != 0)
                    .map(|byte| *byte as u8)
                    .collect();
                let name = String::from_utf8_lossy(&name_bytes).into_owned();
                if !name.is_empty() {
                    processes.push(ProcessInfo {
                        pid: entry.th32ProcessID,
                        name,
                    });
                }
                if Process32Next(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = windows::Win32::Foundation::CloseHandle(snapshot);
        processes.sort_by(|a, b| a.name.cmp(&b.name).then(a.pid.cmp(&b.pid)));
        Ok(processes)
    }
}

#[cfg(not(windows))]
pub fn enumerate_processes() -> Result<Vec<ProcessInfo>, String> {
    Ok(Vec::new())
}

/// Check whether a process has `renderdoc.dll` loaded (i.e. is already
/// injected/capturable). Pure Rust via PSAPI `EnumProcessModules`.
#[cfg(windows)]
pub fn has_renderdoc_module(pid: u32) -> bool {
    use windows::Win32::{
        Foundation::{CloseHandle, HMODULE},
        System::{
            ProcessStatus::{EnumProcessModules, GetModuleBaseNameW},
            Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ},
        },
    };

    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid)
        else {
            return false;
        };
        let mut modules: [HMODULE; 512] = [HMODULE(std::ptr::null_mut()); 512];
        let mut needed = 0u32;
        let has = EnumProcessModules(
            process,
            modules.as_mut_ptr(),
            (modules.len() * std::mem::size_of::<HMODULE>()) as u32,
            &mut needed,
        )
        .is_ok();
        let mut injected = false;
        if has {
            let count = (needed as usize / std::mem::size_of::<HMODULE>()).min(modules.len());
            for module in &modules[..count] {
                let mut name = [0u16; 260];
                let len = GetModuleBaseNameW(process, *module, &mut name) as usize;
                if len > 0 {
                    let module_name = String::from_utf16_lossy(&name[..len]);
                    if module_name.eq_ignore_ascii_case("renderdoc.dll") {
                        injected = true;
                        break;
                    }
                }
            }
        }
        let _ = CloseHandle(process);
        injected
    }
}

#[cfg(not(windows))]
pub fn has_renderdoc_module(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_info_ordering_is_stable() {
        let mut list = vec![
            ProcessInfo {
                pid: 2,
                name: "b".into(),
            },
            ProcessInfo {
                pid: 1,
                name: "a".into(),
            },
            ProcessInfo {
                pid: 3,
                name: "a".into(),
            },
        ];
        list.sort_by(|a, b| a.name.cmp(&b.name).then(a.pid.cmp(&b.pid)));
        assert_eq!(
            list[0],
            ProcessInfo {
                pid: 1,
                name: "a".into()
            }
        );
        assert_eq!(
            list[1],
            ProcessInfo {
                pid: 3,
                name: "a".into()
            }
        );
        assert_eq!(
            list[2],
            ProcessInfo {
                pid: 2,
                name: "b".into()
            }
        );
    }

    #[test]
    #[cfg(windows)]
    fn enumeration_contains_current_process() {
        let processes = enumerate_processes().unwrap();
        let current = std::process::id();
        assert!(processes.iter().any(|process| process.pid == current));
    }

    #[test]
    #[cfg(windows)]
    fn renderdoc_module_check_handles_current_process() {
        // The GUI process itself is not injected; the call must not panic or
        // report a false positive. A nonexistent PID is always false.
        let _ = has_renderdoc_module(std::process::id());
        assert!(!has_renderdoc_module(u32::MAX));
    }
}
