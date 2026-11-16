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
    #[serde(default)]
    pub textures: Vec<Texture>,
    #[serde(default)]
    pub buffers: Vec<Buffer>,
}

/// The maximum bytes per IPC response; larger buffers can be read in pages.
pub const BUFFER_PAGE_LIMIT: u32 = 65_536;

#[derive(Clone, Deserialize, Serialize)]
pub struct Buffer {
    pub name: String,
    pub resource_index: Option<usize>,
    pub length: String,
    pub creation_flags: u32,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Texture {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub dimension: u32,
    pub mips: u32,
    pub array_size: u32,
    pub samples: u32,
    /// Decimal text preserves the full 64-bit value in JavaScript agents.
    pub bytes: String,
    #[serde(default)]
    pub format_name: String,
    pub format_type: u32,
    pub component_type: u32,
    pub component_count: u32,
    pub component_byte_width: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct PipelineResource {
    pub bound: bool,
    pub resource_index: Option<usize>,
    pub name: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct PipelineShader {
    pub stage: String,
    #[serde(flatten)]
    pub resource: PipelineResource,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct PipelineBuffer {
    #[serde(flatten)]
    pub resource: PipelineResource,
    pub byte_offset: String,
    pub byte_size: String,
    pub byte_stride: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct VertexInput {
    pub name: String,
    pub vertex_buffer: i32,
    pub byte_offset: u32,
    pub per_instance: bool,
    pub instance_rate: i32,
    pub format: String,
    pub generic_enabled: bool,
    pub used: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PipelineViewport {
    pub enabled: bool,
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub min_depth: Option<f32>,
    pub max_depth: Option<f32>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Pipeline {
    pub event_id: u32,
    pub available: bool,
    pub topology: u32,
    pub shaders: Vec<PipelineShader>,
    pub outputs: Vec<PipelineResource>,
    #[serde(default)]
    pub index_buffer: Option<PipelineBuffer>,
    #[serde(default)]
    pub vertex_buffers: Option<Vec<PipelineBuffer>>,
    #[serde(default)]
    pub vertex_inputs: Option<Vec<VertexInput>>,
    #[serde(default)]
    pub viewports: Option<Vec<PipelineViewport>>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Reply {
    Snapshot(Snapshot),
    Selected {
        event_id: u32,
        pipeline: Pipeline,
    },
    TextureSaved {
        index: u32,
    },
    BufferData {
        index: u32,
        offset: String,
        hex: String,
    },
    Error {
        message: String,
    },
}

struct Process {
    child: Option<Child>,
    input: Option<ChildStdin>,
    output: Option<BufReader<ChildStdout>>,
}

impl Process {
    fn read(&mut self) -> Result<Reply, String> {
        read_reply(self.output.as_mut().ok_or("Replay session has closed")?)
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
    fn select(&mut self, event: u32) -> Result<Pipeline, String>;
    fn save_texture(&mut self, _index: u32, _path: &Path) -> Result<(), String> {
        Err("Texture export is not supported by this transport".into())
    }
    fn read_buffer(&mut self, _index: u32, _offset: u64, _length: u32) -> Result<Vec<u8>, String> {
        Err("Buffer reading is not supported by this transport".into())
    }
    fn close(&mut self) -> Result<(), String> {
        Ok(())
    }
}

impl Transport for Process {
    fn select(&mut self, event: u32) -> Result<Pipeline, String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        writeln!(input, "select {event}")
            .and_then(|_| input.flush())
            .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::Selected { event_id, pipeline }
                if event_id == event && pipeline.event_id == event =>
            {
                Ok(pipeline)
            }
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected replay selection response".into()),
        }
    }

    fn save_texture(&mut self, index: u32, path: &Path) -> Result<(), String> {
        let path = path
            .to_str()
            .ok_or("Texture output path is not valid UTF-8")?;
        let mut encoded = String::with_capacity(path.len() * 2);
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for &byte in path.as_bytes() {
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 15) as usize] as char);
        }
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        writeln!(input, "save {index} {encoded}")
            .and_then(|_| input.flush())
            .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::TextureSaved { index: saved } if saved == index => Ok(()),
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected texture export response".into()),
        }
    }

    fn read_buffer(&mut self, index: u32, offset: u64, length: u32) -> Result<Vec<u8>, String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        writeln!(input, "buffer {index} {offset} {length}")
            .and_then(|_| input.flush())
            .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::BufferData {
                index: actual,
                offset: actual_offset,
                hex,
            } if actual == index && actual_offset == offset.to_string() => {
                decode_buffer_hex(&hex, length)
            }
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected buffer response".into()),
        }
    }

    fn close(&mut self) -> Result<(), String> {
        if let Some(mut input) = self.input.take() {
            let _ = writeln!(input, "quit");
        }
        if let Some(mut output) = self.output.take() {
            std::thread::spawn(move || {
                let _ = std::io::copy(&mut output, &mut std::io::sink());
            });
        }
        if let Some(mut child) = self.child.take() {
            let status = child.wait().map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!("Replay bridge exited: {status}"));
            }
        }
        Ok(())
    }
}

fn decode_buffer_hex(hex: &str, length: u32) -> Result<Vec<u8>, String> {
    if hex.len() != length as usize * 2 {
        return Err("Invalid buffer response length".into());
    }
    fn digit(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        }
    }
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            Ok((digit(pair[0]).ok_or("Invalid buffer response hex")? << 4)
                | digit(pair[1]).ok_or("Invalid buffer response hex")?)
        })
        .collect()
}

impl Drop for Process {
    fn drop(&mut self) {
        if let Some(mut input) = self.input.take() {
            let _ = writeln!(input, "quit");
        }
        if let Some(mut output) = self.output.take() {
            std::thread::spawn(move || {
                let _ = std::io::copy(&mut output, &mut std::io::sink());
            });
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
            output: Some(BufReader::new(output)),
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

    pub fn read_buffer(&self, index: u32, offset: u64, length: u32) -> Result<Vec<u8>, String> {
        if length > BUFFER_PAGE_LIMIT || offset.checked_add(u64::from(length)).is_none() {
            return Err("Invalid buffer range".into());
        }
        self.process
            .lock()
            .map_err(|e| e.to_string())?
            .read_buffer(index, offset, length)
    }

    pub fn close(&self) -> Result<(), String> {
        self.process.lock().map_err(|e| e.to_string())?.close()
    }

    /// Export a PNG without replacing an existing destination, including symlinks.
    pub fn export_texture(&self, index: u32, destination: &Path) -> Result<(), String> {
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let temporary = tempfile::Builder::new()
            .prefix(".renderdocx-")
            .suffix(".png")
            .tempfile_in(parent)
            .map_err(|e| format!("Cannot create temporary export: {e}"))?
            .into_temp_path();
        self.process
            .lock()
            .map_err(|e| e.to_string())?
            .save_texture(index, temporary.as_ref())?;
        temporary
            .persist_noclobber(destination)
            .map_err(|e| format!("Cannot publish texture export: {}", e.error))?;
        Ok(())
    }

    pub fn select(&self, event: u32, revision: u64) -> Result<bool, String> {
        Ok(self.select_pipeline(event, revision)?.is_some())
    }

    pub fn select_pipeline(&self, event: u32, revision: u64) -> Result<Option<Pipeline>, String> {
        let mut process = self.process.lock().map_err(|e| e.to_string())?;
        if self.revision.load(Ordering::SeqCst) != revision {
            return Ok(None);
        }
        let pipeline = process.select(event)?;
        if self.revision.load(Ordering::SeqCst) != revision {
            return Ok(None);
        }
        Ok(Some(pipeline))
    }
}

/// In-memory replay transport for GPUI interaction tests; never enabled by default.
#[cfg(feature = "ui-tests")]
pub mod test_support {
    use super::*;
    struct RecordingTransport(Arc<Mutex<Vec<u32>>>);
    impl Transport for RecordingTransport {
        fn select(&mut self, event: u32) -> Result<Pipeline, String> {
            self.0.lock().map_err(|e| e.to_string())?.push(event);
            Ok(Pipeline {
                event_id: event,
                available: true,
                topology: 5,
                index_buffer: Some(PipelineBuffer {
                    resource: PipelineResource {
                        bound: true,
                        resource_index: Some(0),
                        name: "Index".into(),
                    },
                    byte_offset: "2".into(),
                    byte_size: "18446744073709551615".into(),
                    byte_stride: 2,
                }),
                vertex_buffers: Some(vec![]),
                vertex_inputs: Some(vec![]),
                viewports: Some(vec![
                    PipelineViewport {
                        enabled: true,
                        x: Some(0.),
                        y: Some(0.),
                        width: Some(16.),
                        height: Some(16.),
                        min_depth: Some(0.),
                        max_depth: Some(1.),
                    };
                    20
                ]),
                ..Default::default()
            })
        }

        fn save_texture(&mut self, _index: u32, path: &Path) -> Result<(), String> {
            const PIXEL: &[u8] = &[
                137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0,
                1, 8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 112,
                104, 56, 240, 31, 0, 5, 4, 2, 128, 60, 158, 3, 128, 0, 0, 0, 0, 73, 69, 78, 68,
                174, 66, 96, 130,
            ];
            std::fs::write(path, PIXEL).map_err(|error| error.to_string())
        }

        fn read_buffer(
            &mut self,
            _index: u32,
            offset: u64,
            length: u32,
        ) -> Result<Vec<u8>, String> {
            Ok((0..length).map(|i| (offset + u64::from(i)) as u8).collect())
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
    fn pipeline_keeps_large_buffer_values_and_unavailable_fields() {
        let json = r#"{"event_id":7,"available":true,"topology":5,"shaders":[],"outputs":[],"index_buffer":{"bound":true,"resource_index":null,"name":"","byte_offset":"18446744073709551615","byte_size":"18446744073709551615","byte_stride":4},"vertex_buffers":[],"vertex_inputs":null,"viewports":[{"enabled":true,"x":null,"y":0,"width":16,"height":16,"min_depth":0,"max_depth":1}]}"#;
        let pipeline: Pipeline = serde_json::from_str(json).unwrap();
        assert_eq!(
            pipeline.index_buffer.unwrap().byte_offset,
            u64::MAX.to_string()
        );
        assert!(pipeline.vertex_inputs.is_none());
        assert!(pipeline.viewports.unwrap()[0].x.is_none());
    }

    #[test]
    fn buffer_response_requires_exact_lowercase_hex_and_length() {
        assert_eq!(decode_buffer_hex("00ff20", 3).unwrap(), vec![0, 255, 32]);
        assert!(decode_buffer_hex("00fg", 2).is_err());
        assert!(decode_buffer_hex("00", 2).is_err());
        assert!(decode_buffer_hex("FF", 1).is_err());
        assert!(decode_buffer_hex("", 0).unwrap().is_empty());
    }

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
        fn select(&mut self, event: u32) -> Result<Pipeline, String> {
            self.calls.lock().unwrap().push(event);
            if self.fails {
                Err("replay failed".into())
            } else {
                Ok(Pipeline {
                    event_id: event,
                    ..Default::default()
                })
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
    fn selection_superseded_during_replay_does_not_publish_stale_pipeline() {
        struct BlockingTransport {
            started: std::sync::mpsc::Sender<()>,
            resume: std::sync::mpsc::Receiver<()>,
        }
        impl Transport for BlockingTransport {
            fn select(&mut self, event: u32) -> Result<Pipeline, String> {
                if event == 10 {
                    self.started.send(()).unwrap();
                    self.resume.recv().unwrap();
                }
                Ok(Pipeline {
                    event_id: event,
                    ..Default::default()
                })
            }
        }
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let session = Session {
            process: Arc::new(Mutex::new(Box::new(BlockingTransport {
                started: started_tx,
                resume: resume_rx,
            }))),
            revision: Arc::new(AtomicU64::new(0)),
        };
        let revision = session.next_revision();
        let old_session = session.clone();
        let old = std::thread::spawn(move || old_session.select_pipeline(10, revision).unwrap());
        started_rx.recv().unwrap();
        let current = session.next_revision();
        resume_tx.send(()).unwrap();
        assert!(old.join().unwrap().is_none());
        assert_eq!(
            session
                .select_pipeline(11, current)
                .unwrap()
                .unwrap()
                .event_id,
            11
        );
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
