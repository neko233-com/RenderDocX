//! Explicitly configured external shader processors; no shell is involved.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ShaderProcessor {
    pub executable: PathBuf,
    pub input_encoding: u32,
    pub output_encoding: u32,
    pub arguments: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NamedShaderProcessor {
    pub name: String,
    #[serde(flatten)]
    pub processor: ShaderProcessor,
}

impl NamedShaderProcessor {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty()
            || self.name == "Builtin"
            || self.name.contains('\0')
            || self.processor.executable.as_os_str().is_empty()
            || self.processor.input_encoding >= 10
            || self.processor.output_encoding >= 10
            || self.processor.timeout_ms == Some(0)
            || self
                .processor
                .arguments
                .iter()
                .any(|arg| arg.contains('\0'))
        {
            return Err("Invalid shader processor configuration".into());
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct ProcessorOutput {
    pub bytes: Vec<u8>,
    pub diagnostics: String,
}

fn vulkan_version(spirv: &str) -> &'static str {
    match spirv {
        "spirv1.0" | "spirv1.1" | "spirv1.2" => "vulkan1.0",
        "spirv1.3" => "vulkan1.1",
        "spirv1.4" => "vulkan1.1spirv1.4",
        "spirv1.5" => "vulkan1.2",
        _ => "vulkan1.3",
    }
}

pub fn run(
    tool: &ShaderProcessor,
    source: &[u8],
    stage: usize,
    entry: &str,
    spirv: &str,
) -> Result<ProcessorOutput, String> {
    const GLSL: [&str; 8] = [
        "vert", "tesc", "tese", "geom", "frag", "comp", "task", "mesh",
    ];
    const HLSL: [&str; 8] = ["vs", "hs", "ds", "gs", "ps", "cs", "as", "ms"];
    const FULL: [&str; 8] = [
        "vertex",
        "hull",
        "domain",
        "geometry",
        "pixel",
        "compute",
        "amplification",
        "mesh",
    ];
    if stage >= GLSL.len()
        || tool.executable.as_os_str().is_empty()
        || source.is_empty()
        || entry.is_empty()
        || tool.timeout_ms == Some(0)
    {
        return Err("Invalid shader processor request".into());
    }
    let directory = tempfile::tempdir()
        .map_err(|error| format!("Cannot create processor workspace: {error}"))?;
    let input_path = directory.path().join("shader_input");
    let output_path = directory.path().join("shader_output");
    let stdout_path = directory.path().join("stdout");
    let stderr_path = directory.path().join("stderr");
    fs::write(&input_path, source)
        .map_err(|error| format!("Cannot write processor input: {error}"))?;
    let spirv = if spirv.is_empty() { "spirv1.0" } else { spirv };
    let mut stdin = false;
    let mut output_file = false;
    let mut args = Vec::new();
    for arg in &tool.arguments {
        match arg.as_str() {
            "{stdin}" => {
                if stdin {
                    return Err("Duplicate stdin placeholder".into());
                }
                stdin = true;
                continue;
            }
            "{input_file}" => {
                args.push(input_path.as_os_str().to_owned());
                continue;
            }
            "{output_file}" => {
                output_file = true;
                args.push(output_path.as_os_str().to_owned());
                continue;
            }
            "{entry_point}" => {
                args.push(entry.into());
                continue;
            }
            _ => {}
        }
        let value = arg
            .replace("{glsl_stage4}", GLSL[stage])
            .replace("{hlsl_stage2}", HLSL[stage])
            .replace("{full_stage}", FULL[stage])
            .replace("{spirv_ver}", spirv)
            .replace("{vulkan_ver}", vulkan_version(spirv));
        args.push(value.into());
    }
    let executable = if tool.executable.is_relative() && tool.executable.components().count() > 1 {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(&tool.executable)
    } else {
        tool.executable.clone()
    };
    let mut command = Command::new(executable);
    command
        .current_dir(directory.path())
        .args(args)
        .stdin(if stdin {
            Stdio::from(fs::File::open(&input_path).map_err(|error| error.to_string())?)
        } else {
            Stdio::null()
        })
        .stdout(Stdio::from(
            fs::File::create(&stdout_path).map_err(|error| error.to_string())?,
        ))
        .stderr(Stdio::from(
            fs::File::create(&stderr_path).map_err(|error| error.to_string())?,
        ));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("Cannot start shader processor: {error}"))?;
    let status = if let Some(timeout) = tool.timeout_ms {
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() >= Duration::from_millis(timeout) => {
                    child.kill().map_err(|error| {
                        format!("Shader processor timed out after {timeout} ms and could not be stopped: {error}")
                    })?;
                    let _ = child.wait();
                    return Err(format!("Shader processor timed out after {timeout} ms"));
                }
                Ok(None) => std::thread::sleep(
                    Duration::from_millis(10)
                        .min(Duration::from_millis(timeout).saturating_sub(started.elapsed())),
                ),
                Err(error) => {
                    if child.kill().is_ok() {
                        let _ = child.wait();
                    }
                    return Err(format!("Cannot wait for shader processor: {error}"));
                }
            }
        }
    } else {
        child
            .wait()
            .map_err(|error| format!("Cannot wait for shader processor: {error}"))?
    };
    let stdout =
        fs::read(&stdout_path).map_err(|error| format!("Cannot read processor stdout: {error}"))?;
    let stderr =
        fs::read(&stderr_path).map_err(|error| format!("Cannot read processor stderr: {error}"))?;
    let mut diagnostics = String::from_utf8_lossy(&stderr).into_owned();
    if output_file && !stdout.is_empty() {
        if !diagnostics.is_empty() {
            diagnostics.push('\n');
        }
        diagnostics.push_str(&String::from_utf8_lossy(&stdout));
    }
    if !status.success() {
        return Err(format!(
            "Shader processor exited with {}: {diagnostics}",
            status
        ));
    }
    let bytes = if output_file {
        fs::read(&output_path)
            .map_err(|error| format!("Cannot read shader processor output: {error}"))?
    } else {
        stdout
    };
    if bytes.is_empty() {
        return Err("Shader processor returned empty output".into());
    }
    Ok(ProcessorOutput { bytes, diagnostics })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stage_and_vulkan_mappings_match_renderdoc() {
        assert_eq!(vulkan_version("spirv1.0"), "vulkan1.0");
        assert_eq!(vulkan_version("spirv1.4"), "vulkan1.1spirv1.4");
        assert_eq!(vulkan_version("spirv1.6"), "vulkan1.3");
        let tool = ShaderProcessor {
            executable: "unused".into(),
            input_encoding: 5,
            output_encoding: 3,
            arguments: vec![],
            timeout_ms: None,
        };
        assert!(run(&tool, b"source", 8, "main", "").is_err());
    }

    #[test]
    fn imported_configuration_validates_and_roundtrips() {
        let json = serde_json::json!({"name":"My compiler","executable":"compiler","input_encoding":5,"output_encoding":3,"arguments":["-o","{output_file}","{input_file}"]});
        let mut config: NamedShaderProcessor = serde_json::from_value(json.clone()).unwrap();
        config.validate().unwrap();
        assert_eq!(serde_json::to_value(&config).unwrap(), json);
        config.name = "Builtin".into();
        assert!(config.validate().is_err());
        config.name = "external".into();
        config.processor.timeout_ms = Some(0);
        assert!(config.validate().is_err());
        config.processor.timeout_ms = None;
        config.processor.arguments.push("bad\0argument".into());
        assert!(config.validate().is_err());
    }
}
