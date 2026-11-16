use crate::backend::{Settings, find_tool};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
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

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct CaptureOptions {
    pub allow_vsync: bool,
    pub allow_fullscreen: bool,
    pub api_validation: bool,
    pub capture_callstacks: bool,
    pub capture_callstacks_only_actions: bool,
    pub delay_for_debugger: u32,
    pub verify_buffer_access: bool,
    pub hook_into_children: bool,
    pub ref_all_resources: bool,
    pub capture_all_cmd_lists: bool,
    pub debug_output_mute: bool,
    pub soft_memory_limit: u32,
}
impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            allow_vsync: true,
            allow_fullscreen: true,
            api_validation: false,
            capture_callstacks: false,
            capture_callstacks_only_actions: false,
            delay_for_debugger: 0,
            verify_buffer_access: false,
            hook_into_children: false,
            ref_all_resources: false,
            capture_all_cmd_lists: false,
            debug_output_mute: true,
            soft_memory_limit: 0,
        }
    }
}
impl CaptureOptions {
    fn wire(&self) -> [u32; 12] {
        [
            self.allow_vsync.into(),
            self.allow_fullscreen.into(),
            self.api_validation.into(),
            self.capture_callstacks.into(),
            self.capture_callstacks_only_actions.into(),
            self.delay_for_debugger,
            self.verify_buffer_access.into(),
            self.hook_into_children.into(),
            self.ref_all_resources.into(),
            self.capture_all_cmd_lists.into(),
            self.debug_output_mute.into(),
            self.soft_memory_limit,
        ]
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EnvOperation {
    Set,
    Append,
    Prepend,
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EnvSeparator {
    Platform,
    SemiColon,
    Colon,
    NoSep,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentModification {
    pub name: String,
    pub value: String,
    pub operation: EnvOperation,
    pub separator: EnvSeparator,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LaunchConfiguration {
    pub arguments: String,
    pub working_directory: Option<PathBuf>,
    pub options: CaptureOptions,
    pub environment: Vec<EnvironmentModification>,
}
impl LaunchConfiguration {
    pub fn validate(&self) -> Result<(), String> {
        self.validate_remote()?;
        if self
            .working_directory
            .as_ref()
            .is_some_and(|path| !path.is_absolute() || !path.is_dir())
        {
            return Err("Invalid local working directory".into());
        }
        Ok(())
    }

    pub fn validate_remote(&self) -> Result<(), String> {
        if self.arguments.contains('\0')
            || self
                .working_directory
                .as_ref()
                .is_some_and(|path| path.to_str().is_none_or(|value| value.contains('\0')))
        {
            return Err("Invalid arguments or working directory".into());
        }
        if self.environment.len() > u32::MAX as usize
            || self.environment.iter().any(|entry| {
                entry.name.is_empty()
                    || entry.name.trim() != entry.name
                    || entry.name.contains(['\0', '='])
                    || entry.value.contains('\0')
            })
        {
            return Err("Invalid environment modification".into());
        }
        Ok(())
    }
}

struct Inner {
    input: Mutex<Option<ChildStdin>>,
    child: Mutex<Option<Child>>,
    events: Arc<Mutex<VecDeque<Value>>>,
    alive: Arc<AtomicBool>,
    pending_copies: Mutex<HashMap<u32, PathBuf>>,
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

pub fn parse_target_pid(value: &std::ffi::OsStr) -> Option<u32> {
    value.to_str()?.parse::<u32>().ok().filter(|pid| *pid != 0)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplayGpu {
    pub vendor: u32,
    pub device_id: u32,
    pub driver: String,
    pub name: String,
    pub apis: Vec<u32>,
}

pub fn enumerate_gpus(settings: &Settings, url: Option<&str>) -> Result<Vec<ReplayGpu>, String> {
    if url.is_some_and(|url| !valid_remote_url(url)) {
        return Err("Invalid remote server URL".into());
    }
    let mut args: Vec<&std::ffi::OsStr> = vec!["--gpus".as_ref()];
    if let Some(url) = url {
        args.push(url.as_ref());
    }
    let value = remote_request(settings, &args, "gpus")?;
    serde_json::from_value(value.get("gpus").cloned().ok_or("Missing GPU list")?)
        .map_err(|e| e.to_string())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteEntry {
    pub name: String,
    pub flags: u32,
    pub last_modified: u32,
    pub size: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteDirectory {
    pub home: String,
    pub path: String,
    pub drivers: Vec<String>,
    pub proxies: Vec<String>,
    pub entries: Vec<RemoteEntry>,
}

fn remote_request(
    settings: &Settings,
    args: &[&std::ffi::OsStr],
    expected: &str,
) -> Result<Value, String> {
    let mut command = Command::new(target_helper(settings)?);
    command
        .args(args)
        .stderr(Stdio::null())
        .env_remove("RENDERDOCX_CONNECT_PID")
        .env_remove("RENDERDOCX_OPEN_CAPTURE");
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
        .ok_or("Remote server produced no response")?;
    let value: Value = serde_json::from_str(payload).map_err(|e| e.to_string())?;
    if !output.status.success() || value.get("type").and_then(Value::as_str) != Some(expected) {
        return Err(value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Remote server request failed")
            .to_owned());
    }
    Ok(value)
}

pub fn browse_remote(
    settings: &Settings,
    url: &str,
    path: Option<&str>,
) -> Result<RemoteDirectory, String> {
    if !valid_remote_url(url) || path.is_some_and(|p| p.is_empty() || p.contains('\0')) {
        return Err("Invalid remote server URL or directory".into());
    }
    let mut args: Vec<&std::ffi::OsStr> = vec!["--remote-list".as_ref(), url.as_ref()];
    if let Some(path) = path {
        args.push(path.as_ref());
    }
    let result: RemoteDirectory =
        serde_json::from_value(remote_request(settings, &args, "remote_directory")?)
            .map_err(|e| e.to_string())?;
    if result
        .entries
        .iter()
        .any(|entry| entry.size.parse::<u64>().is_err())
    {
        return Err("Invalid remote directory response".into());
    }
    Ok(result)
}

pub fn download_remote_capture(
    settings: &Settings,
    url: &str,
    remote_path: &str,
    size: u64,
    root: &Path,
) -> Result<PathBuf, String> {
    if !valid_remote_url(url)
        || remote_path.is_empty()
        || remote_path.contains('\0')
        || size == 0
        || !root.is_absolute()
        || !root.is_dir()
    {
        return Err("Invalid remote capture path, size or download directory".into());
    }
    let folder = tempfile::Builder::new()
        .prefix("remote-file-")
        .tempdir_in(root)
        .map_err(|e| e.to_string())?;
    let path = folder.path().join("capture.rdc");
    remote_request(
        settings,
        &[
            "--remote-copy".as_ref(),
            url.as_ref(),
            remote_path.as_ref(),
            path.as_os_str(),
        ],
        "remote_copy",
    )?;
    let metadata = path.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() != size {
        return Err("Downloaded capture size differs from remote listing".into());
    }
    let _ = folder.keep();
    Ok(path)
}

pub fn remote_parent(directory: &str) -> Option<String> {
    let trimmed = directory.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() || (trimmed.len() == 2 && trimmed.as_bytes().get(1) == Some(&b':')) {
        return None;
    }
    let index = trimmed.rfind(['/', '\\'])?;
    if index == 0 {
        return Some(trimmed[..=index].to_owned());
    }
    if index == 2 && trimmed.as_bytes().get(1) == Some(&b':') {
        return Some(trimmed[..=index].to_owned());
    }
    Some(trimmed[..index].to_owned())
}

pub fn remote_join(directory: &str, name: &str) -> Option<String> {
    if directory == "/"
        && name.len() >= 2
        && name.as_bytes()[0].is_ascii_alphabetic()
        && name.as_bytes()[1] == b':'
        && name[2..].chars().all(|c| c == '/' || c == '\\')
    {
        return Some(format!("{}:\\", &name[..1]));
    }
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return None;
    }
    let windows = directory.as_bytes().get(1) == Some(&b':')
        || directory.starts_with("\\\\")
        || directory.contains('\\');
    let separator = if windows { '\\' } else { '/' };
    Some(
        format!(
            "{}{}{}",
            directory,
            if directory.ends_with(['/', '\\']) {
                ""
            } else if windows {
                "\\"
            } else {
                "/"
            },
            name
        )
        .replace('/', &separator.to_string()),
    )
}

pub fn enumerate(settings: &Settings) -> Result<Vec<u32>, String> {
    enumerate_target(settings, None)
}

fn valid_remote_url(url: &str) -> bool {
    !url.is_empty() && url.trim() == url && !url.contains('\0')
}

pub fn enumerate_target(settings: &Settings, url: Option<&str>) -> Result<Vec<u32>, String> {
    let mut command = Command::new(target_helper(settings)?);
    match url {
        None => {
            command.arg("--enumerate");
        }
        Some(url) if valid_remote_url(url) => {
            command.args(["--enumerate-remote", url]);
        }
        Some(_) => return Err("Invalid remote target URL".into()),
    }
    command
        .stderr(Stdio::null())
        .env_remove("RENDERDOCX_CONNECT_PID")
        .env_remove("RENDERDOCX_OPEN_CAPTURE");
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
    fn start(
        settings: &Settings,
        args: &[&std::ffi::OsStr],
        environment: Option<&[EnvironmentModification]>,
        startup_timeout: Duration,
    ) -> Result<Self, String> {
        let target = target_helper(settings)?;
        let mut command = Command::new(target);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env_remove("RENDERDOCX_CONNECT_PID")
            .env_remove("RENDERDOCX_OPEN_CAPTURE");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        let input = child.stdin.take().ok_or("Missing target helper stdin")?;
        let output = child.stdout.take().ok_or("Missing target helper stdout")?;
        let environment = environment.map(<[EnvironmentModification]>::to_vec);
        let (input_tx, input_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut input = input;
            let result = (|| -> std::io::Result<()> {
                let Some(environment) = environment else {
                    return Ok(());
                };
                fn hex(value: &str) -> String {
                    if value.is_empty() {
                        return "-".into();
                    }
                    const DIGITS: &[u8; 16] = b"0123456789abcdef";
                    let mut result = String::with_capacity(value.len() * 2);
                    for byte in value.bytes() {
                        result.push(DIGITS[(byte >> 4) as usize] as char);
                        result.push(DIGITS[(byte & 15) as usize] as char);
                    }
                    result
                }
                writeln!(input, "{}", environment.len())?;
                for entry in &environment {
                    let operation = match entry.operation {
                        EnvOperation::Set => 0,
                        EnvOperation::Append => 1,
                        EnvOperation::Prepend => 2,
                    };
                    let separator = match entry.separator {
                        EnvSeparator::Platform => 0,
                        EnvSeparator::SemiColon => 1,
                        EnvSeparator::Colon => 2,
                        EnvSeparator::NoSep => 3,
                    };
                    writeln!(
                        input,
                        "{operation} {separator} {} {}",
                        hex(&entry.name),
                        hex(&entry.value)
                    )?;
                }
                input.flush()
            })();
            let _ = input_tx.send(result.map(|()| input));
        });
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
        let connected = match startup_rx.recv_timeout(startup_timeout) {
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
        let input = match input_rx.recv_timeout(startup_timeout) {
            Ok(Ok(input)) => input,
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Could not send target configuration".into());
            }
        };
        Ok(Self {
            inner: Arc::new(Inner {
                input: Mutex::new(Some(input)),
                child: Mutex::new(Some(child)),
                events,
                alive,
                pending_copies: Mutex::new(HashMap::new()),
            }),
            connected,
            capture_template: None,
        })
    }

    pub fn launch(
        settings: &Settings,
        app: &Path,
        directory: &Path,
        config: &LaunchConfiguration,
    ) -> Result<Self, String> {
        if !app.is_file() || !directory.is_absolute() || !directory.is_dir() {
            return Err("Application and absolute capture directory must exist".into());
        }
        config.validate()?;
        // RenderDoc only checks captures from the current target, not pre-existing files.
        // A fresh directory prevents a new session from overwriting older captures.
        let folder = tempfile::Builder::new()
            .prefix("session-")
            .tempdir_in(directory)
            .map_err(|e| e.to_string())?
            .keep();
        let template = folder.join("capture");
        let mut args: Vec<std::ffi::OsString> = vec![
            "--launch2".into(),
            app.as_os_str().into(),
            template.as_os_str().into(),
            config.arguments.as_str().into(),
            config
                .working_directory
                .as_deref()
                .unwrap_or(Path::new(""))
                .as_os_str()
                .into(),
        ];
        args.extend(
            config
                .options
                .wire()
                .into_iter()
                .map(|value| value.to_string().into()),
        );
        let refs: Vec<&std::ffi::OsStr> = args.iter().map(|arg| arg.as_os_str()).collect();
        let timeout = Duration::from_secs(30 + u64::from(config.options.delay_for_debugger));
        let mut session = Self::start(settings, &refs, Some(&config.environment), timeout)?;
        session.capture_template = Some(template);
        Ok(session)
    }
    pub fn launch_remote(
        settings: &Settings,
        url: &str,
        app: &str,
        config: &LaunchConfiguration,
    ) -> Result<Self, String> {
        if !valid_remote_url(url) || app.trim().is_empty() || app.contains('\0') {
            return Err("Invalid remote server URL or application path".into());
        }
        config.validate_remote()?;
        let mut args: Vec<std::ffi::OsString> = vec![
            "--launch-remote2".into(),
            app.into(),
            url.into(),
            config.arguments.as_str().into(),
            config
                .working_directory
                .as_deref()
                .unwrap_or(Path::new(""))
                .as_os_str()
                .into(),
        ];
        args.extend(
            config
                .options
                .wire()
                .into_iter()
                .map(|value| value.to_string().into()),
        );
        let refs: Vec<&std::ffi::OsStr> = args.iter().map(|arg| arg.as_os_str()).collect();
        let timeout = Duration::from_secs(30 + u64::from(config.options.delay_for_debugger));
        Self::start(settings, &refs, Some(&config.environment), timeout)
    }

    pub fn connect(settings: &Settings, ident: u32) -> Result<Self, String> {
        if ident == 0 {
            return Err("Target ident must be nonzero".into());
        }
        let value = ident.to_string();
        Self::start(
            settings,
            &["--connect".as_ref(), value.as_ref()],
            None,
            Duration::from_secs(30),
        )
    }
    pub fn connect_remote(settings: &Settings, url: &str, ident: u32) -> Result<Self, String> {
        if ident == 0 || !valid_remote_url(url) {
            return Err("Invalid remote target URL or ident".into());
        }
        let value = ident.to_string();
        Self::start(
            settings,
            &["--connect-remote".as_ref(), url.as_ref(), value.as_ref()],
            None,
            Duration::from_secs(30),
        )
    }
    pub fn connect_pid(settings: &Settings, pid: u32) -> Result<Self, String> {
        if pid == 0 {
            return Err("Process ID must be nonzero".into());
        }
        let value = pid.to_string();
        Self::start(
            settings,
            &["--connect-pid".as_ref(), value.as_ref()],
            None,
            Duration::from_secs(30),
        )
    }
    pub fn is_alive(&self) -> bool {
        if !self.inner.alive.load(Ordering::SeqCst) {
            return false;
        }
        // An injected target may inherit the helper's stdout handle. In that case
        // EOF is not delivered when the helper crashes, so inspect the process too.
        let status = self.inner.child.lock().ok().and_then(|mut child| {
            child
                .as_mut()
                .and_then(|child| child.try_wait().ok().flatten())
        });
        if let Some(status) = status {
            if self.inner.alive.swap(false, Ordering::SeqCst) {
                if let Ok(mut events) = self.inner.events.lock() {
                    if !status.success() {
                        events.push_back(json!({"type":"error","message":format!("Target helper exited: {status}")}));
                    }
                    events.push_back(json!({"type":"disconnected"}));
                }
            }
            return false;
        }
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
    /// Start a copy into a fresh application-owned directory. Completion arrives via `drain`.
    pub fn copy_capture(&self, capture_id: u32) -> Result<PathBuf, String> {
        self.copy_capture_to(capture_id, &capture_directory()?)
    }
    pub fn copy_capture_to(&self, capture_id: u32, root: &Path) -> Result<PathBuf, String> {
        if !root.is_absolute() || !root.is_dir() {
            return Err("Capture directory must be an existing absolute path".into());
        }
        let mut pending = self
            .inner
            .pending_copies
            .lock()
            .map_err(|e| e.to_string())?;
        if pending.contains_key(&capture_id) {
            return Err("Capture copy is already pending".into());
        }
        let folder = tempfile::Builder::new()
            .prefix("remote-")
            .tempdir_in(root)
            .map_err(|e| e.to_string())?;
        let path = folder.path().join("capture.rdc");
        let text = path
            .to_str()
            .ok_or("Capture copy path is not valid UTF-8")?;
        let encoded: String = text
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self.send(&format!("copy {capture_id} {encoded}"))?;
        pending.insert(capture_id, path.clone());
        let _ = folder.keep();
        Ok(path)
    }
    pub fn drain(&self) -> Result<Vec<Value>, String> {
        self.is_alive();
        let events: Vec<Value> = self
            .inner
            .events
            .lock()
            .map_err(|e| e.to_string())?
            .drain(..)
            .collect();
        let mut pending = self
            .inner
            .pending_copies
            .lock()
            .map_err(|e| e.to_string())?;
        Ok(events.into_iter().map(|event| {
            if event.get("type").and_then(Value::as_str) != Some("copied") { return event; }
            let id = event.get("capture_id").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok());
            let expected = id.and_then(|id| pending.remove(&id));
            let valid = expected.as_ref().is_some_and(|path| event.get("path").and_then(Value::as_str).is_some_and(|actual| Path::new(actual) == path) && path.metadata().is_ok_and(|meta| meta.is_file() && meta.len() > 0));
            if valid { event } else { json!({"type":"error","message":"Capture copy completion did not match a pending local file"}) }
        }).collect())
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
    #[ignore = "subprocess fixture for helper liveness"]
    fn helper_exits_for_liveness_test() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "live::tests::hold_stdout_for_liveness_test",
                "--ignored",
            ])
            .stdout(Stdio::inherit())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        command.spawn().unwrap();
        std::process::exit(7);
    }

    #[test]
    #[ignore = "subprocess fixture that temporarily holds stdout open"]
    fn hold_stdout_for_liveness_test() {
        std::thread::sleep(Duration::from_secs(2));
    }

    #[test]
    fn exited_helper_is_disconnected_without_waiting_for_stdout_eof() {
        use std::time::Instant;
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "live::tests::helper_exits_for_liveness_test",
                "--ignored",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        // The fixture's descendant inherits stdout after the helper exits.
        let _held_stdout = child.stdout.take().unwrap();
        let session = LiveSession {
            inner: Arc::new(Inner {
                input: Mutex::new(Some(input)),
                child: Mutex::new(Some(child)),
                events: Arc::new(Mutex::new(VecDeque::new())),
                alive: Arc::new(AtomicBool::new(true)),
                pending_copies: Mutex::new(HashMap::new()),
            }),
            connected: json!({}),
            capture_template: None,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while session.is_alive() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!session.is_alive());
        assert!(session.trigger(1).is_err());
        let events = session.drain().unwrap();
        assert_eq!(events.iter().filter(|e| e["type"] == "error").count(), 1);
        assert_eq!(
            events
                .iter()
                .filter(|e| e["type"] == "disconnected")
                .count(),
            1
        );
        assert!(session.drain().unwrap().is_empty());
    }

    #[test]
    fn remote_urls_require_nonempty_clean_text() {
        assert!(valid_remote_url("127.0.0.1"));
        assert!(valid_remote_url("adb://device"));
        assert!(!valid_remote_url(""));
        assert!(!valid_remote_url(" host"));
        assert!(!valid_remote_url("host\0x"));
    }
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

    #[test]
    fn editor_pid_must_be_a_nonzero_u32() {
        assert_eq!(parse_target_pid(std::ffi::OsStr::new("1234")), Some(1234));
        for invalid in ["", "0", "-1", "4294967296", "42x"] {
            assert_eq!(parse_target_pid(std::ffi::OsStr::new(invalid)), None);
        }
    }

    #[test]
    fn launch_configuration_uses_upstream_defaults_and_rejects_bad_environment() {
        let configuration: LaunchConfiguration = serde_json::from_str("{}").unwrap();
        assert_eq!(
            configuration.options.wire(),
            [1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0]
        );
        assert!(configuration.validate().is_ok());
        assert!(
            serde_json::from_str::<LaunchConfiguration>(r#"{"options":{"unknown":true}}"#).is_err()
        );
        let invalid: LaunchConfiguration = serde_json::from_str(r#"{"environment":[{"name":"BAD=NAME","value":"value","operation":"set","separator":"no_sep"}]}"#).unwrap();
        assert!(invalid.validate().is_err());
        let whitespace: LaunchConfiguration = serde_json::from_str(r#"{"environment":[{"name":" NAME ","value":"value","operation":"set","separator":"no_sep"}]}"#).unwrap();
        assert!(whitespace.validate().is_err());
    }
}

#[cfg(test)]
mod remote_path_tests {
    use super::{remote_join, remote_parent};
    #[test]
    fn remote_paths_use_the_remote_syntax() {
        assert_eq!(
            remote_join("/home/user", "capture.rdc").as_deref(),
            Some("/home/user/capture.rdc")
        );
        assert_eq!(
            remote_join("C:\\Users\\Name", "capture.rdc").as_deref(),
            Some("C:\\Users\\Name\\capture.rdc")
        );
        assert_eq!(remote_join("/", "folder").as_deref(), Some("/folder"));
        assert!(remote_join("/tmp", "../other").is_none());
        assert_eq!(remote_join("/", "C:").as_deref(), Some("C:\\"));
        assert_eq!(remote_join("/", "C:/").as_deref(), Some("C:\\"));
        assert_eq!(remote_parent("/home").as_deref(), Some("/"));
        assert_eq!(remote_parent("C:\\Users").as_deref(), Some("C:\\"));
        assert_eq!(remote_parent("C:\\"), None);
    }
}
