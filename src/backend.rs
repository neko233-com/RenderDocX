use crate::i18n::Language;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub language: Language,
    pub qrenderdoc: Option<PathBuf>,
    pub renderdoccmd: Option<PathBuf>,
}

impl Settings {
    fn path() -> Option<PathBuf> {
        ProjectDirs::from("com", "neko233", "RenderDocX")
            .map(|p| p.config_dir().join("settings.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path().ok_or("No configuration directory available")?;
        fs::create_dir_all(path.parent().ok_or("Invalid configuration path")?)
            .map_err(|e| e.to_string())?;
        let data = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        fs::write(path, data).map_err(|e| e.to_string())
    }
}

fn find_tool(configured: &Option<PathBuf>, name: &str) -> Result<PathBuf, String> {
    if let Some(path) = configured {
        return if path.is_file() {
            Ok(path.clone())
        } else {
            Err(format!("{}: {}", name, path.display()))
        };
    }
    let executable = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    if let Ok(current) = std::env::current_exe() {
        if let Some(parent) = current.parent() {
            let candidate = parent.join(&executable);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    if cfg!(windows) {
        if let Some(root) = std::env::var_os("ProgramFiles") {
            let candidate = PathBuf::from(root).join("RenderDoc").join(&executable);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    which::which(&executable).map_err(|_| format!("{executable}: not found"))
}

pub fn open_capture(settings: &Settings, capture: &Path) -> Result<(), String> {
    let program = find_tool(&settings.qrenderdoc, "qrenderdoc")?;
    Command::new(program)
        .arg(capture)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

pub fn open_upstream(settings: &Settings) -> Result<(), String> {
    let program = find_tool(&settings.qrenderdoc, "qrenderdoc")?;
    Command::new(program)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

pub fn launch_capture(settings: &Settings, executable: &Path) -> Result<(), String> {
    let program = find_tool(&settings.renderdoccmd, "renderdoccmd")?;
    let frontend = find_tool(&settings.qrenderdoc, "qrenderdoc")?;
    let mut child = Command::new(program)
        .arg("capture")
        .arg(executable)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    // A successful capture launch returns the target ident as the process exit
    // code, so an ordinary success-status check would reject valid launches.
    let mut stderr = BufReader::new(child.stderr.take().ok_or("Missing renderdoccmd stderr")?);
    let mut detail = String::new();
    let ident = loop {
        let mut bytes = Vec::new();
        match stderr.read_until(b'\n', &mut bytes) {
            Ok(0) => {
                let status = child.wait().map_err(|e| e.to_string())?;
                return Err(if detail.is_empty() {
                    format!("renderdoccmd exited: {status}")
                } else {
                    detail
                });
            }
            Ok(_) => {
                let line = String::from_utf8_lossy(&bytes);
                if let Some(id) = line
                    .trim()
                    .strip_prefix("Launched as ID ")
                    .and_then(|s| s.trim().parse::<u32>().ok())
                    .filter(|id| *id != 0)
                {
                    break id;
                }
                if detail.len() < 8192 {
                    detail.push_str(&line);
                }
            }
            Err(error) => {
                let _ = child.wait();
                return Err(error.to_string());
            }
        }
    };
    // On Unix the target may inherit stderr. Drain it until all writers close,
    // so a long-running target cannot block on a full pipe or delay handoff.
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
    });
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Command::new(frontend)
        .arg("--targetcontrol")
        .arg(format!("localhost:{ident}"))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Target {ident} started, but qrenderdoc could not be opened: {e}"))
}
