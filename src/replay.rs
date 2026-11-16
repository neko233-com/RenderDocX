use crate::backend::{Settings, find_tool};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone, Deserialize, Serialize)]
pub struct Action {
    pub event_id: u32,
    pub action_id: u32,
    pub depth: usize,
    pub parent: i64,
    pub flags: u32,
    pub indices: u32,
    pub instances: u32,
    pub name: String,
}

#[derive(Deserialize, Serialize)]
pub struct Snapshot {
    pub protocol: u32,
    pub api: u32,
    pub frame: u32,
    pub degraded: bool,
    pub actions: Vec<Action>,
    pub resource_count: usize,
    pub texture_count: usize,
    pub buffer_count: usize,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Reply {
    Snapshot(Snapshot),
    Selected { event_id: u32 },
    Error { message: String },
}

struct Process {
    child: Option<Child>,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
}

impl Process {
    fn read(&mut self) -> Result<Reply, String> {
        read_reply(&mut self.output)
    }
}

fn read_reply(reader: &mut impl BufRead) -> Result<Reply, String> {
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Err("Replay bridge stopped before replying".into());
        }
        if let Some(payload) = line.strip_prefix("RENDERDOCX/1 ") {
            return serde_json::from_str(payload)
                .map_err(|e| format!("Invalid replay bridge response: {e}"));
        }
    }
}

fn validate_snapshot(snapshot: &Snapshot) -> Result<(), String> {
    if snapshot.protocol != 1 {
        return Err("Unsupported replay bridge protocol".into());
    }
    for (index, action) in snapshot.actions.iter().enumerate() {
        if action.parent == -1 {
            if action.depth != 0 {
                return Err("Invalid root action depth".into());
            }
        } else {
            let parent = usize::try_from(action.parent).map_err(|_| "Invalid action parent")?;
            if parent >= index
                || snapshot.actions[parent].depth.checked_add(1) != Some(action.depth)
            {
                return Err("Invalid action hierarchy".into());
            }
        }
    }
    Ok(())
}

trait Transport: Send {
    fn select(&mut self, event: u32) -> Result<(), String>;
}

impl Transport for Process {
    fn select(&mut self, event: u32) -> Result<(), String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        writeln!(input, "select {event}")
            .and_then(|_| input.flush())
            .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::Selected { event_id } if event_id == event => Ok(()),
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected replay selection response".into()),
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if let Some(mut input) = self.input.take() {
            let _ = writeln!(input, "quit");
        }
        if let Some(mut child) = self.child.take() {
            // Replay shutdown can involve GPU driver work; never block the UI thread.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

#[derive(Clone)]
pub struct Session {
    process: Arc<Mutex<Box<dyn Transport>>>,
    revision: Arc<AtomicU64>,
}

impl Session {
    pub fn open(settings: &Settings, capture: &Path) -> Result<(Self, Snapshot), String> {
        let executable = find_tool(&settings.replay_bridge, "renderdocx-replay")?;
        let mut command = Command::new(executable);
        command
            .arg(capture)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        let input = child.stdin.take().ok_or("Missing replay bridge input")?;
        let output = child.stdout.take().ok_or("Missing replay bridge output")?;
        let mut process = Process {
            child: Some(child),
            input: Some(input),
            output: BufReader::new(output),
        };
        let snapshot = match process.read()? {
            Reply::Snapshot(snapshot) => {
                validate_snapshot(&snapshot)?;
                snapshot
            }
            Reply::Error { message } => return Err(message),
            _ => return Err("Unexpected replay bridge response".into()),
        };
        Ok((
            Self {
                process: Arc::new(Mutex::new(Box::new(process))),
                revision: Arc::new(AtomicU64::new(0)),
            },
            snapshot,
        ))
    }

    pub fn next_revision(&self) -> u64 {
        self.revision.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn select(&self, event: u32, revision: u64) -> Result<bool, String> {
        let mut process = self.process.lock().map_err(|e| e.to_string())?;
        if self.revision.load(Ordering::SeqCst) != revision {
            return Ok(false);
        }
        process.select(event)?;
        Ok(true)
    }
}

/// In-memory replay transport for GPUI interaction tests; never enabled by default.
#[cfg(feature = "ui-tests")]
pub mod test_support {
    use super::*;
    struct RecordingTransport(Arc<Mutex<Vec<u32>>>);
    impl Transport for RecordingTransport {
        fn select(&mut self, event: u32) -> Result<(), String> {
            self.0.lock().map_err(|e| e.to_string())?.push(event);
            Ok(())
        }
    }

    pub fn recording_session() -> (Session, Arc<Mutex<Vec<u32>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let session = Session {
            process: Arc::new(Mutex::new(Box::new(RecordingTransport(events.clone())))),
            revision: Arc::new(AtomicU64::new(0)),
        };
        (session, events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Cursor, sync::Mutex};

    const SNAPSHOT: &str = r#"{"type":"snapshot","protocol":1,"api":3,"frame":7,"degraded":false,"actions":[{"event_id":10,"action_id":1,"depth":0,"parent":-1,"flags":0,"indices":0,"instances":0,"name":"pass"},{"event_id":11,"action_id":2,"depth":1,"parent":0,"flags":2,"indices":3,"instances":1,"name":"draw"}],"resource_count":4,"texture_count":2,"buffer_count":1}"#;

    #[test]
    fn decodes_bridge_snapshot_after_foreign_output() {
        let input = format!("driver diagnostic\n\nRENDERDOCX/1 {SNAPSHOT}\n");
        let Reply::Snapshot(snapshot) = read_reply(&mut Cursor::new(input)).unwrap() else {
            panic!("expected snapshot");
        };
        validate_snapshot(&snapshot).unwrap();
        assert_eq!(snapshot.actions[1].event_id, 11);
        assert_eq!(snapshot.actions[1].parent, 0);
        assert_eq!(snapshot.texture_count, 2);
    }

    #[test]
    fn rejects_bad_protocol_hierarchy_and_truncated_responses() {
        let Reply::Snapshot(mut snapshot) =
            read_reply(&mut Cursor::new(format!("RENDERDOCX/1 {SNAPSHOT}\n"))).unwrap()
        else {
            panic!("expected snapshot");
        };
        snapshot.actions[1].parent = 1;
        assert!(validate_snapshot(&snapshot).is_err());
        snapshot.actions[1].parent = 0;
        snapshot.actions[1].depth = 7;
        assert!(validate_snapshot(&snapshot).is_err());
        snapshot.protocol = 2;
        assert!(validate_snapshot(&snapshot).is_err());
        assert!(read_reply(&mut Cursor::new("RENDERDOCX/1 {\"type\":\n")).is_err());
        assert!(read_reply(&mut Cursor::new("driver output only\n")).is_err());
    }

    struct FakeTransport {
        calls: Arc<Mutex<Vec<u32>>>,
        fails: bool,
    }
    impl Transport for FakeTransport {
        fn select(&mut self, event: u32) -> Result<(), String> {
            self.calls.lock().unwrap().push(event);
            if self.fails {
                Err("replay failed".into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn superseded_selection_never_reaches_the_replay_transport() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let session = Session {
            process: Arc::new(Mutex::new(Box::new(FakeTransport {
                calls: calls.clone(),
                fails: false,
            }))),
            revision: Arc::new(AtomicU64::new(0)),
        };
        let old = session.next_revision();
        let current = session.next_revision();
        assert!(!session.select(10, old).unwrap());
        assert!(session.select(11, current).unwrap());
        assert_eq!(*calls.lock().unwrap(), vec![11]);
    }

    #[test]
    fn transport_errors_are_not_reported_as_success() {
        let session = Session {
            process: Arc::new(Mutex::new(Box::new(FakeTransport {
                calls: Arc::new(Mutex::new(Vec::new())),
                fails: true,
            }))),
            revision: Arc::new(AtomicU64::new(0)),
        };
        let revision = session.next_revision();
        assert_eq!(session.select(4, revision).unwrap_err(), "replay failed");
    }
}
