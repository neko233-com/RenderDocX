use crate::backend::{Settings, find_tool};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

struct Inner {
    input: Mutex<Option<ChildStdin>>,
    child: Mutex<Option<Child>>,
    events: Arc<Mutex<VecDeque<Value>>>,
    alive: Arc<AtomicBool>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Ok(input) = self.input.get_mut() {
            if let Some(mut input) = input.take() {
                let _ = writeln!(input, "quit");
            }
        }
        if let Ok(child) = self.child.get_mut() {
            if let Some(mut child) = child.take() {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
        }
    }
}

#[derive(Clone)]
pub struct LiveSession {
    inner: Arc<Inner>,
    pub connected: Value,
    pub capture_template: Option<PathBuf>,
}

fn target_helper(settings: &Settings) -> Result<PathBuf, String> {
    if let Some(bridge) = &settings.replay_bridge {
        let name = if cfg!(windows) {
            "renderdocx-target.exe"
        } else {
            "renderdocx-target"
        };
        let sibling = bridge
            .parent()
            .ok_or("Invalid replay bridge path")?
            .join(name);
        if !sibling.is_file() {
            return Err(format!("Target helper not found: {}", sibling.display()));
        }
        Ok(sibling)
    } else {
        find_tool(&None, "renderdocx-target")
    }
}

pub fn capture_notice(event: &Value) -> Option<(u32, PathBuf, bool)> {
    if event.get("type")?.as_str()? != "capture" {
        return None;
    }
    let frame = u32::try_from(event.get("frame")?.as_u64()?).ok()?;
    let path = event.get("path")?.as_str()?;
    if path.is_empty() {
        return None;
    }
    Some((frame, PathBuf::from(path), event.get("local")?.as_bool()?))
}

pub fn enumerate(settings: &Settings) -> Result<Vec<u32>, String> {
    let mut command = Command::new(target_helper(settings)?);
    command.arg("--enumerate").stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command.output().map_err(|e| e.to_string())?;
    let stdout = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
    let payload = stdout
        .lines()
        .find_map(|line| line.strip_prefix("RENDERDOCX-TARGET/1 "))
        .ok_or("Target discovery produced no response")?;
    let value: Value = serde_json::from_str(payload).map_err(|e| e.to_string())?;
    if !output.status.success() || value.get("type").and_then(Value::as_str) != Some("targets") {
        return Err(value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Target discovery failed")
            .to_owned());
    }
    serde_json::from_value(
        value
            .get("idents")
            .cloned()
            .ok_or("Missing target identifiers")?,
    )
    .map_err(|e| e.to_string())
}

impl LiveSession {
    fn start(settings: &Settings, args: &[&std::ffi::OsStr]) -> Result<Self, String> {
        let target = target_helper(settings)?;
        let mut command = Command::new(target);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        let input = child.stdin.take().ok_or("Missing target helper stdin")?;
        let output = child.stdout.take().ok_or("Missing target helper stdout")?;
        let events = Arc::new(Mutex::new(VecDeque::<Value>::new()));
        let alive = Arc::new(AtomicBool::new(false));
        let (startup_tx, startup_rx) = mpsc::channel();
        let event_queue = events.clone();
        let active = alive.clone();
        std::thread::spawn(move || {
            let mut startup = Some(startup_tx);
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else {
                    break;
                };
                let Some(payload) = line.strip_prefix("RENDERDOCX-TARGET/1 ") else {
                    continue;
                };
                let value: Value = match serde_json::from_str(payload) {
                    Ok(value) => value,
                    Err(error) => {
                        if let Some(sender) = startup.take() {
                            let _ = sender.send(Err(format!("Invalid target response: {error}")));
                        } else if let Ok(mut queue) = event_queue.lock() {
                            queue.push_back(json!({"type":"error","message":format!("Invalid target response: {error}")}));
                        }
                        break;
                    }
                };
                let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
                if let Some(sender) = startup.take() {
                    if kind == "connected" {
                        active.store(true, Ordering::SeqCst);
                        let _ = sender.send(Ok(value));
                        continue;
                    }
                    let _ = sender.send(Err(value
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("Target connection failed")
                        .to_owned()));
                    break;
                }
                if kind == "disconnected" {
                    active.store(false, Ordering::SeqCst);
                }
                if let Ok(mut queue) = event_queue.lock() {
                    // A slow GUI only needs the latest progress, not thousands of obsolete updates.
                    if kind == "progress" {
                        queue.retain(|item| {
                            item.get("type").and_then(Value::as_str) != Some("progress")
                        });
                    }
                    queue.push_back(value);
                }
            }
            if let Some(sender) = startup {
                let _ = sender.send(Err("Target helper stopped before connecting".into()));
            }
            if active.swap(false, Ordering::SeqCst) {
                if let Ok(mut queue) = event_queue.lock() {
                    queue.push_back(json!({"type":"disconnected"}));
                }
            }
        });
        let connected = match startup_rx.recv_timeout(Duration::from_secs(30)) {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => {
                let _ = child.wait();
                return Err(error);
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(
                    "Target connection timed out; the application may already have started".into(),
                );
            }
        };
        Ok(Self {
            inner: Arc::new(Inner {
                input: Mutex::new(Some(input)),
                child: Mutex::new(Some(child)),
                events,
                alive,
            }),
            connected,
            capture_template: None,
        })
    }

    pub fn launch(
        settings: &Settings,
        app: &Path,
        directory: &Path,
        arguments: &str,
    ) -> Result<Self, String> {
        if !app.is_file() || !directory.is_absolute() || !directory.is_dir() {
            return Err("Application and absolute capture directory must exist".into());
        }
        // RenderDoc only checks captures from the current target, not pre-existing files.
        // A fresh directory prevents a new session from overwriting older captures.
        let folder = tempfile::Builder::new()
            .prefix("session-")
            .tempdir_in(directory)
            .map_err(|e| e.to_string())?
            .keep();
        let template = folder.join("capture");
        let mut session = Self::start(
            settings,
            &[
                "--launch".as_ref(),
                app.as_os_str(),
                template.as_os_str(),
                arguments.as_ref(),
            ],
        )?;
        session.capture_template = Some(template);
        Ok(session)
    }
    pub fn connect(settings: &Settings, ident: u32) -> Result<Self, String> {
        if ident == 0 {
            return Err("Target ident must be nonzero".into());
        }
        let value = ident.to_string();
        Self::start(settings, &["--connect".as_ref(), value.as_ref()])
    }
    pub fn is_alive(&self) -> bool {
        self.inner.alive.load(Ordering::SeqCst)
    }
    fn send(&self, command: &str) -> Result<(), String> {
        if !self.is_alive() {
            return Err("Target is disconnected".into());
        }
        let mut guard = self.inner.input.lock().map_err(|e| e.to_string())?;
        let input = guard.as_mut().ok_or("Target is closed")?;
        writeln!(input, "{command}")
            .and_then(|_| input.flush())
            .map_err(|e| e.to_string())
    }
    pub fn trigger(&self, frames: u32) -> Result<(), String> {
        if frames == 0 {
            return Err("Frame count must be nonzero".into());
        }
        self.send(&format!("trigger {frames}"))
    }
    pub fn queue(&self, frame: u32, frames: u32) -> Result<(), String> {
        if frames == 0 {
            return Err("Frame count must be nonzero".into());
        }
        self.send(&format!("queue {frame} {frames}"))
    }
    pub fn cycle(&self) -> Result<(), String> {
        self.send("cycle")
    }
    pub fn drain(&self) -> Result<Vec<Value>, String> {
        Ok(self
            .inner
            .events
            .lock()
            .map_err(|e| e.to_string())?
            .drain(..)
            .collect())
    }
    pub fn close(&self) -> Result<(), String> {
        if let Some(mut input) = self.inner.input.lock().map_err(|e| e.to_string())?.take() {
            let _ = writeln!(input, "quit");
        }
        if let Some(mut child) = self.inner.child.lock().map_err(|e| e.to_string())?.take() {
            let status = child.wait().map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!("Target helper exited: {status}"));
            }
        }
        self.inner.alive.store(false, Ordering::SeqCst);
        Ok(())
    }
}

pub fn capture_directory() -> Result<PathBuf, String> {
    let root = directories::ProjectDirs::from("com", "neko233", "RenderDocX")
        .ok_or("No application data directory")?
        .data_local_dir()
        .join("captures");
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_notice_rejects_incomplete_or_non_capture_events() {
        let event = json!({"type":"capture","frame":42,"path":"frame.rdc","local":true});
        assert_eq!(
            capture_notice(&event),
            Some((42, PathBuf::from("frame.rdc"), true))
        );
        assert!(
            capture_notice(&json!({"type":"capture","frame":42,"path":"","local":true})).is_none()
        );
        assert!(
            capture_notice(&json!({"type":"api","frame":42,"path":"frame.rdc","local":true}))
                .is_none()
        );
        assert!(
            capture_notice(
                &json!({"type":"capture","frame":4294967296u64,"path":"x","local":true})
            )
            .is_none()
        );
    }
}
