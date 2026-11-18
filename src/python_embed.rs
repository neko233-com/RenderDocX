//! Embedded CPython execution via the mature `pyo3` crate, replacing the
//! external-process Python path for interactive scripting and UI-extension
//! lifecycle calls. This is Rust-native orchestration; no C++ bridge involved.

use pyo3::{prelude::*, types::PyDict, Python};
use std::{ffi::CString, sync::Mutex};

// CPython's interpreter and sys.stdout redirection are process-global; serialize
// all embedded calls so concurrent scripts cannot clobber each other's capture.
static PY_LOCK: Mutex<()> = Mutex::new(());

const CAPTURE_SETUP: &str = "import sys, io\n_stdout = getattr(sys, '_rdx_orig_out', sys.stdout)\n_stderr = getattr(sys, '_rdx_orig_err', sys.stderr)\nsys._rdx_orig_out = _stdout\nsys._rdx_orig_err = _stderr\n_sysbuf_out = io.StringIO()\n_sysbuf_err = io.StringIO()\nsys.stdout = _sysbuf_out\nsys.stderr = _sysbuf_err\n";
const CAPTURE_RESTORE: &str = "sys.stdout = getattr(sys, '_rdx_orig_out', sys.stdout)\nsys.stderr = getattr(sys, '_rdx_orig_err', sys.stderr)\n";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EmbedOutput {
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
}

/// Read-only summary of the open capture exposed to Python scripts as the
/// `renderdocx` module (mirrors the most-used `pyrenderdoc.CaptureContext`
/// properties without embedding the full API binding layer).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaptureView {
    pub frame: u32,
    pub api: u32,
    pub action_count: usize,
    pub resource_count: usize,
    pub texture_count: usize,
    pub buffer_count: usize,
    pub selected_event: Option<u32>,
    pub resource_names: Vec<String>,
}

fn cstring(code: &str) -> Result<CString, String> {
    CString::new(code).map_err(|_| "Python source contains a NUL byte".to_owned())
}

/// Execute Python source in-process, capturing stdout/stderr.
pub fn run_script(code: &str) -> EmbedOutput {
    let _guard = PY_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    Python::attach(|py| {
        let globals = PyDict::new(py);
        let mut result = EmbedOutput::default();
        if let Err(error) = py.run(&cstring(CAPTURE_SETUP).unwrap(), Some(&globals), None) {
            result.error = Some(format!("capture setup failed: {error}"));
            return result;
        }
        match py.run(&cstring(code).unwrap_or_default(), Some(&globals), None) {
            Ok(()) => {}
            Err(error) => result.error = Some(format_py_error(py, &error)),
        }
        result.stdout = get_buffer(&globals, "_sysbuf_out");
        result.stderr = get_buffer(&globals, "_sysbuf_err");
        let _ = py.run(&cstring(CAPTURE_RESTORE).unwrap(), Some(&globals), None);
        result
    })
}

/// Execute Python with a read-only `renderdocx` capture view installed in
/// builtins, so scripts can inspect the open capture via
/// `renderdocx.frame`, `renderdocx.api_name`, `renderdocx.action_count`, etc.
pub fn run_script_with_capture(code: &str, view: &CaptureView) -> EmbedOutput {
    use pyo3::types::{PyList, PyModule};
    let _guard = PY_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    Python::attach(|py| {
        let globals = PyDict::new(py);
        let mut result = EmbedOutput::default();
        // Build the renderdocx module object as a dict-backed namespace.
        let module = PyDict::new(py);
        module.set_item("frame", view.frame).ok();
        module.set_item("api", view.api).ok();
        module
            .set_item("api_name", crate::api_enums::graphics_api::name(view.api))
            .ok();
        module.set_item("action_count", view.action_count).ok();
        module.set_item("resource_count", view.resource_count).ok();
        module.set_item("texture_count", view.texture_count).ok();
        module.set_item("buffer_count", view.buffer_count).ok();
        module.set_item("selected_event", view.selected_event).ok();
        let names = PyList::new(py, &view.resource_names).ok();
        if let Some(names) = names {
            module.set_item("resource_names", names).ok();
        }
        // Inject as a builtin module importable via `import renderdocx`.
        let modules = py.import("sys").and_then(|sys| sys.getattr("modules")).ok();
        if let Some(modules) = modules {
            let m = PyModule::new(py, "renderdocx").ok();
            if let Some(m) = m {
                let dict = m.dict();
                for (key, value) in &module {
                    dict.set_item(key, value).ok();
                }
                modules.set_item("renderdocx", m).ok();
            }
        }
        if let Err(error) = py.run(&cstring(CAPTURE_SETUP).unwrap(), Some(&globals), None) {
            result.error = Some(format!("capture setup failed: {error}"));
            return result;
        }
        match py.run(&cstring(code).unwrap_or_default(), Some(&globals), None) {
            Ok(()) => {}
            Err(error) => result.error = Some(format_py_error(py, &error)),
        }
        result.stdout = get_buffer(&globals, "_sysbuf_out");
        result.stderr = get_buffer(&globals, "_sysbuf_err");
        let _ = py.run(&cstring(CAPTURE_RESTORE).unwrap(), Some(&globals), None);
        result
    })
}

fn get_buffer(globals: &Bound<'_, PyDict>, name: &str) -> String {
    globals
        .get_item(name)
        .ok()
        .flatten()
        .and_then(|obj| obj.call_method0("getvalue").ok())
        .and_then(|value| value.extract::<String>().ok())
        .unwrap_or_default()
}

fn format_py_error(py: Python<'_>, error: &PyErr) -> String {
    error
        .value(py)
        .getattr("__str__")
        .ok()
        .and_then(|f| f.call0().ok())
        .and_then(|s| s.extract::<String>().ok())
        .unwrap_or_else(|| error.to_string())
}

/// Load an extension package's `__init__.py` and call one of its lifecycle
/// entry points (`register(version, pyrenderdoc)` / `unregister()`), capturing
/// output and exceptions. The extension directory is prepended to `sys.path`.
pub fn call_extension(dir: &std::path::Path, function: &str, version: &str) -> EmbedOutput {
    let _guard = PY_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    Python::attach(|py| {
        let globals = PyDict::new(py);
        let mut result = EmbedOutput::default();
        // Build a Python string literal for the directory via repr, avoiding
        // manual escaping of Windows separators.
        let dir_repr = py
            .eval(
                &cstring("repr(_dir)").unwrap(),
                Some(&{
                    let d = PyDict::new(py);
                    d.set_item("_dir", dir.to_string_lossy().as_ref()).ok();
                    d
                }),
                None,
            )
            .ok()
            .and_then(|obj| obj.extract::<String>().ok())
            .unwrap_or_else(|| format!("{:?}", dir.to_string_lossy()));
        let setup = format!("{CAPTURE_SETUP}import sys\nsys.path.insert(0, {dir_repr})\n");
        if let Err(error) = py.run(&cstring(&setup).unwrap_or_default(), Some(&globals), None) {
            result.error = Some(format!("setup failed: {error}"));
            return result;
        }
        let load = format!(
            "import importlib.util as _u\n_spec=_u.spec_from_file_location('_rdx_ext', {dir_repr}+'/__init__.py')\n_mod=_u.module_from_spec(_spec)\n_spec.loader.exec_module(_mod)\n"
        );
        if let Err(error) = py.run(&cstring(&load).unwrap_or_default(), Some(&globals), None) {
            result.error = Some(format_py_error(py, &error));
            return result;
        }
        let call = if function == "register" {
            format!("_mod.register({version:?}, None)")
        } else {
            "_mod.unregister()".to_owned()
        };
        if let Err(error) = py.run(&cstring(&call).unwrap_or_default(), Some(&globals), None) {
            result.error = Some(format_py_error(py, &error));
        }
        result.stdout = get_buffer(&globals, "_sysbuf_out");
        result.stderr = get_buffer(&globals, "_sysbuf_err");
        let _ = py.run(&cstring(CAPTURE_RESTORE).unwrap(), Some(&globals), None);
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executes_python_in_process_with_captured_output() {
        let result = run_script("print('hello from embedded python')");
        assert!(result.error.is_none(), "{:?}", result.error);
        assert!(result.stdout.contains("hello from embedded python"));
    }

    #[test]
    fn captures_python_exceptions() {
        let result = run_script("raise ValueError('boom')");
        assert!(result.error.is_some());
        assert!(result.error.unwrap().contains("boom"));
    }

    #[test]
    fn calls_extension_register_and_unregister_in_process() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("__init__.py"),
            "def register(version, pyrenderdoc):\n    print('registered', version)\ndef unregister():\n    print('unregistered')\n",
        )
        .unwrap();
        let registered = call_extension(dir.path(), "register", "1.48");
        assert!(registered.error.is_none(), "{:?}", registered.error);
        assert!(registered.stdout.contains("registered 1.48"));
        let unregistered = call_extension(dir.path(), "unregister", "1.48");
        assert!(unregistered.error.is_none(), "{:?}", unregistered.error);
        assert!(unregistered.stdout.contains("unregistered"));
    }

    #[test]
    fn scripts_can_access_capture_view() {
        let view = CaptureView {
            frame: 7,
            api: crate::api_enums::graphics_api::VULKAN,
            action_count: 42,
            resource_count: 3,
            texture_count: 2,
            buffer_count: 1,
            selected_event: Some(11),
            resource_names: vec!["texA".into(), "bufB".into()],
        };
        let result = run_script_with_capture(
            "import renderdocx\nprint(renderdocx.frame, renderdocx.api_name, renderdocx.action_count, renderdocx.resource_names[0])",
            &view,
        );
        assert!(result.error.is_none(), "{:?}", result.error);
        assert!(result.stdout.contains("7 Vulkan 42 texA"));
    }

    #[test]
    fn rejects_nul_bytes() {
        assert!(cstring("a\0b").is_err());
    }
}
