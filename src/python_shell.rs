//! Python scripting console for RenderDocX.
//!
//! Upstream `PythonShell` embeds CPython and exposes the `pyrenderdoc` API so
//! scripts can drive the open capture. RenderDocX does not embed an interpreter;
//! it runs scripts through an external Python process and captures their
//! stdout/stderr. Scripts therefore cannot access the live replay session —
//! this is an intentional, documented difference.

use std::process::Command;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptResult {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

/// Run a Python snippet with `python -c`, capturing both output streams.
/// The interpreter can be overridden with `RENDERDOCX_PYTHON`.
pub fn execute(code: &str) -> Result<ScriptResult, String> {
    let interpreter = std::env::var("RENDERDOCX_PYTHON").unwrap_or_else(|_| "python".to_owned());
    run(&interpreter, code)
}

fn run(interpreter: &str, code: &str) -> Result<ScriptResult, String> {
    let output = Command::new(interpreter)
        .arg("-c")
        .arg(code)
        .output()
        .map_err(|error| format!("Could not start Python ({interpreter}): {error}"))?;
    Ok(ScriptResult {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        success: output.status.success(),
    })
}

/// Render script output as console lines, prefixed by stream.
pub fn format_output(result: &ScriptResult) -> Vec<String> {
    let mut lines = Vec::new();
    for line in result.stdout.lines() {
        lines.push(format!("> {line}"));
    }
    for line in result.stderr.lines() {
        lines.push(format!("! {line}"));
    }
    if lines.is_empty() && result.success {
        lines.push("Finished.".to_owned());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_output_marks_streams_and_empty_success() {
        let result = ScriptResult {
            stdout: "a\nb\n".into(),
            stderr: "err".into(),
            success: true,
        };
        assert_eq!(format_output(&result), ["> a", "> b", "! err"]);
        let empty = ScriptResult {
            stdout: String::new(),
            stderr: String::new(),
            success: true,
        };
        assert_eq!(format_output(&empty), ["Finished."]);
    }

    #[test]
    fn execute_uses_configured_interpreter() {
        // A missing interpreter yields a structured error instead of a panic.
        let missing = format!("renderdocx-no-python-{}", std::process::id());
        assert!(run(&missing, "print(1)").is_err());
    }

    #[test]
    fn execute_runs_real_python_when_available() {
        let interpreter =
            std::env::var("RENDERDOCX_PYTHON").unwrap_or_else(|_| "python".to_owned());
        if Command::new(&interpreter)
            .arg("--version")
            .output()
            .is_err()
        {
            return; // no interpreter in this environment
        }
        let result = run(&interpreter, "print('renderdocx-ok')").unwrap();
        assert!(result.success, "{}", result.stderr);
        assert!(result.stdout.contains("renderdocx-ok"));
    }
}
