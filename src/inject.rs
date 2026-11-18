//! Native DLL injection into a running process, implemented directly with the
//! Windows API (no C++ `RENDERDOC_InjectIntoProcess` and no C++ ABI).
//!
//! This is the standard remote-thread technique:
//!   OpenProcess -> VirtualAllocEx -> WriteProcessMemory (DLL path) ->
//!   CreateRemoteThread(LoadLibraryW) -> wait -> VirtualFreeEx.
//!
//! The caller supplies the RenderDoc capture DLL path; injecting it makes a
//! running D3D/GL/Vulkan process capturable by RenderDoc, which is what the
//! upstream "Inject into Process" action does.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InjectOutcome {
    pub pid: u32,
    pub dll: String,
    pub thread_exit_code: u32,
}

/// Inject `dll_path` into the process `pid`.
///
/// Requires that the caller can open the target process (same architecture and
/// sufficient rights). On failure the returned string describes the step that
/// failed; no partial state is left behind beyond what the OS reaps on process
/// exit.
#[cfg(windows)]
pub fn inject_dll(pid: u32, dll_path: &std::path::Path) -> Result<InjectOutcome, String> {
    use windows::core::{PCSTR, s};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Diagnostics::Debug::WriteProcessMemory;
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAllocEx, VirtualFreeEx,
    };
    use windows::Win32::System::Threading::{
        CreateRemoteThread, INFINITE, LPTHREAD_START_ROUTINE, OpenProcess, PROCESS_CREATE_THREAD,
        PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,
        WaitForSingleObject,
    };

    let display = dll_path.display().to_string();
    if !dll_path.is_file() {
        return Err(format!("DLL not found: {display}"));
    }
    // The remote thread needs a NUL-terminated UTF-16 path.
    let mut wide: Vec<u16> = dll_path.as_os_str().to_string_lossy().encode_utf16().collect();
    wide.push(0);
    let byte_len = wide.len() * std::mem::size_of::<u16>();

    unsafe {
        let process = OpenProcess(
            PROCESS_CREATE_THREAD
                | PROCESS_QUERY_INFORMATION
                | PROCESS_VM_OPERATION
                | PROCESS_VM_WRITE
                | PROCESS_VM_READ,
            false,
            pid,
        )
        .map_err(|error| format!("OpenProcess({pid}) failed: {error}"))?;

        let remote = VirtualAllocEx(
            process,
            None,
            byte_len,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        );
        if remote.is_null() {
            let _ = CloseHandle(process);
            return Err("VirtualAllocEx in the target process failed".to_owned());
        }

        let written = WriteProcessMemory(
            process,
            remote,
            wide.as_ptr() as *const std::ffi::c_void,
            byte_len,
            None,
        );
        if let Err(error) = written {
            let _ = VirtualFreeEx(process, remote, 0, MEM_RELEASE);
            let _ = CloseHandle(process);
            return Err(format!("WriteProcessMemory failed: {error}"));
        }

        let kernel32 = GetModuleHandleW(windows::core::w!("kernel32.dll"))
            .map_err(|error| format!("GetModuleHandleW(kernel32.dll) failed: {error}"))?;
        let load_library = GetProcAddress(kernel32, s!("LoadLibraryW"));
        let Some(load_library) = load_library else {
            let _ = VirtualFreeEx(process, remote, 0, MEM_RELEASE);
            let _ = CloseHandle(process);
            return Err("GetProcAddress(LoadLibraryW) failed".to_owned());
        };
        let start: LPTHREAD_START_ROUTINE =
            Some(std::mem::transmute::<unsafe extern "system" fn() -> isize, unsafe extern "system" fn(*mut std::ffi::c_void) -> u32>(load_library));

        let thread = CreateRemoteThread(process, None, 0, start, Some(remote), 0, None)
            .map_err(|error| format!("CreateRemoteThread failed: {error}"))?;

        let wait = WaitForSingleObject(thread, INFINITE);
        let mut exit_code = 0u32;
        let _ = windows::Win32::System::Threading::GetExitCodeThread(thread, &mut exit_code);
        let _ = CloseHandle(thread);
        let _ = VirtualFreeEx(process, remote, 0, MEM_RELEASE);
        let _ = CloseHandle(process);

        if wait != WAIT_OBJECT_0 {
            return Err(format!("Remote thread wait returned {wait:?}"));
        }
        if exit_code == 0 {
            return Err(
                "LoadLibraryW returned NULL in the target (see its debug output; likely an architecture or dependency mismatch)".to_owned(),
            );
        }
        let _ = PCSTR::null();
        let _ = HANDLE::default();
        Ok(InjectOutcome {
            pid,
            dll: display,
            thread_exit_code: exit_code,
        })
    }
}

#[cfg(not(windows))]
pub fn inject_dll(_pid: u32, _dll_path: &std::path::Path) -> Result<InjectOutcome, String> {
    Err("DLL injection is only implemented on Windows".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_dll_is_rejected_before_any_process_work() {
        let result = inject_dll(0, std::path::Path::new("definitely-missing-renderdoc.dll"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("DLL not found"));
    }

    #[cfg(windows)]
    #[test]
    fn invalid_pid_reports_open_process_failure() {
        let dir = tempfile::tempdir().unwrap();
        let dll = dir.path().join("fake.dll");
        std::fs::write(&dll, b"not a real dll").unwrap();
        let result = inject_dll(u32::MAX, &dll);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("OpenProcess"));
    }
}
