use crate::backend::{Settings, find_tool};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
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
    #[serde(default)]
    pub api_events: Vec<ApiEvent>,
}

/// An API event belonging to an action, in upstream capture order.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ApiEvent {
    pub event_id: u32,
    pub chunk_index: Option<u32>,
    /// Relative capture-stream offset, encoded as a decimal string to preserve u64 precision.
    pub file_offset: String,
    /// Structured chunk name; empty when no chunk is available.
    pub name: String,
}

/// A page of direct children in RenderDoc's structured capture data.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StructurePage {
    pub event_id: u32,
    pub offset: String,
    pub path: Vec<String>,
    pub node: StructuredNode,
    pub children: Vec<StructuredNode>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StructuredNode {
    pub name: String,
    pub type_name: String,
    pub basic_type: u32,
    pub flags: u32,
    pub byte_size: String,
    pub child_count: String,
    pub value: Option<String>,
    pub custom_string: Option<String>,
    pub resource_non_null: Option<bool>,
    pub resource_index: Option<usize>,
}

pub const STRUCTURE_PAGE_LIMIT: u32 = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct MeshSelection {
    pub instance: u32,
    pub view: u32,
    /// RenderDoc MeshDataStage: 1 = VSOut, 2 = GSOut, 3 = TaskOut, 4 = MeshOut.
    pub stage: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MeshletInfo {
    pub indices: u32,
    pub vertices: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct MeshInfo {
    pub event_id: u32,
    pub instance: u32,
    pub view: u32,
    pub stage: u32,
    pub index_bound: bool,
    pub index_offset: String,
    pub index_size: String,
    pub index_stride: u32,
    pub base_vertex: i32,
    pub vertex_bound: bool,
    pub vertex_offset: String,
    pub vertex_size: String,
    pub vertex_stride: u32,
    pub per_primitive_offset: String,
    pub per_primitive_stride: u32,
    pub format_name: String,
    pub format_type: u32,
    pub component_type: u32,
    pub component_count: u32,
    pub component_byte_width: u32,
    pub topology: u32,
    pub num_indices: u32,
    pub restart_index: u32,
    pub allow_restart: bool,
    pub unproject: bool,
    pub flip_y: bool,
    pub near_plane: Option<f32>,
    pub far_plane: Option<f32>,
    pub meshlet_offset: u32,
    pub meshlet_index_offset: u32,
    pub dispatch_size: Option<[u32; 3]>,
    pub meshlets: Vec<MeshletInfo>,
    pub tasks: Vec<[u32; 3]>,
    pub status: String,
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
pub const SHADER_STAGES: [&str; 8] = [
    "vertex",
    "tess_control",
    "tess_eval",
    "geometry",
    "fragment",
    "compute",
    "task",
    "mesh",
];

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ShaderSignature {
    pub name: String,
    pub semantic: String,
    pub semantic_index: u32,
    pub register: u32,
    pub system_value: u32,
    #[serde(rename = "type")]
    pub variable_type: u32,
    pub components: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ShaderReport {
    pub event_id: u32,
    pub stage: u32,
    pub entry_point: String,
    pub encoding: u32,
    pub target: String,
    pub targets: Vec<String>,
    pub disassembly: String,
    pub inputs: Vec<ShaderSignature>,
    pub outputs: Vec<ShaderSignature>,
    pub constant_blocks: usize,
    pub read_only_resources: usize,
    pub read_write_resources: usize,
    pub samplers: usize,
}

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct TextureSelection {
    pub mip: u32,
    pub slice: u32,
    pub sample: Option<u32>,
    pub channel: Option<u32>,
}

impl TextureSelection {
    pub fn validate(&self, texture: &Texture) -> Result<(), String> {
        let slices = if texture.dimension == 3 {
            texture.depth.checked_shr(self.mip).unwrap_or(0).max(1)
        } else {
            texture.array_size
        };
        if self.mip >= texture.mips
            || self.mip > i32::MAX as u32
            || self.slice >= slices
            || self.slice > i32::MAX as u32
            || self.sample.is_some_and(|value| value >= texture.samples)
            || self
                .channel
                .is_some_and(|value| value >= texture.component_count || value > i32::MAX as u32)
        {
            return Err("Texture selection is out of range".into());
        }
        Ok(())
    }
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
    Shader(ShaderReport),
    Structure(StructurePage),
    Mesh(MeshInfo),
    MeshData {
        event_id: u32,
        kind: String,
        offset: String,
        hex: String,
    },
    StructuredBuffer {
        event_id: u32,
        path: Vec<String>,
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
    executable: PathBuf,
    capture: PathBuf,
    capture_stamp: Option<CaptureStamp>,
    events: HashMap<u32, ApiEvent>,
    structure_worker: Option<Box<Process>>,
}

#[derive(Clone, PartialEq, Eq)]
struct CaptureStamp {
    length: u64,
    modified: std::time::SystemTime,
    created: Option<std::time::SystemTime>,
}

fn capture_stamp(path: &Path) -> Option<CaptureStamp> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(CaptureStamp {
        length: metadata.len(),
        modified: metadata.modified().ok()?,
        created: metadata.created().ok(),
    })
}

impl Process {
    fn read(&mut self) -> Result<Reply, String> {
        read_reply(self.output.as_mut().ok_or("Replay session has closed")?)
    }

    fn open(executable: &Path, capture: &Path, worker: bool) -> Result<(Self, Snapshot), String> {
        let capture = std::fs::canonicalize(capture).unwrap_or_else(|_| capture.to_path_buf());
        let before = capture_stamp(&capture);
        let mut command = Command::new(executable);
        if worker {
            command.arg("--structure-worker");
        }
        command
            .arg(&capture)
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
        let input = child.stdin.take().ok_or("Missing replay bridge input")?;
        let output = child.stdout.take().ok_or("Missing replay bridge output")?;
        let mut process = Self {
            child: Some(child),
            input: Some(input),
            output: Some(BufReader::new(output)),
            executable: executable.to_path_buf(),
            capture: capture.clone(),
            capture_stamp: None,
            events: HashMap::new(),
            structure_worker: None,
        };
        let snapshot = match process.read()? {
            Reply::Snapshot(snapshot) => {
                validate_snapshot(&snapshot)?;
                snapshot
            }
            Reply::Error { message } => return Err(message),
            _ => return Err("Unexpected replay bridge response".into()),
        };
        let after = capture_stamp(&capture);
        if before.is_some() && before == after {
            process.capture_stamp = before;
        }
        for event in snapshot
            .actions
            .iter()
            .flat_map(|action| &action.api_events)
        {
            process
                .events
                .entry(event.event_id)
                .or_insert_with(|| event.clone());
        }
        Ok((process, snapshot))
    }

    fn read_structure_buffer_raw(
        &mut self,
        event: u32,
        path: &[u64],
        offset: u64,
        length: u32,
    ) -> Result<Vec<u8>, String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        write!(input, "structure_buffer {event} {offset} {length}").map_err(|e| e.to_string())?;
        for index in path {
            write!(input, " {index}").map_err(|e| e.to_string())?;
        }
        writeln!(input)
            .and_then(|_| input.flush())
            .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::StructuredBuffer {
                event_id,
                path: actual_path,
                offset: actual_offset,
                hex,
            } if event_id == event
                && actual_offset.parse::<u64>().ok() == Some(offset)
                && actual_path
                    .iter()
                    .map(|item| item.parse::<u64>())
                    .collect::<Result<Vec<_>, _>>()
                    .ok()
                    .as_deref()
                    == Some(path) =>
            {
                decode_buffer_hex(&hex, length)
            }
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected structured buffer response".into()),
        }
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
    fn save_texture(
        &mut self,
        _index: u32,
        _selection: TextureSelection,
        _path: &Path,
    ) -> Result<(), String> {
        Err("Texture export is not supported by this transport".into())
    }
    fn read_buffer(&mut self, _index: u32, _offset: u64, _length: u32) -> Result<Vec<u8>, String> {
        Err("Buffer reading is not supported by this transport".into())
    }
    fn shader(&mut self, _stage: u32, _target: Option<u32>) -> Result<ShaderReport, String> {
        Err("Shader inspection is not supported by this transport".into())
    }
    fn structure(
        &mut self,
        _event: u32,
        _path: &[u64],
        _offset: u64,
        _limit: u32,
    ) -> Result<StructurePage, String> {
        Err("Structured event inspection is not supported by this transport".into())
    }
    fn structure_buffer(
        &mut self,
        _event: u32,
        _path: &[u64],
        _offset: u64,
        _length: u32,
    ) -> Result<Vec<u8>, String> {
        Err("Structured buffer reading is not supported by this transport".into())
    }
    fn mesh(&mut self, _event: u32, _selection: MeshSelection) -> Result<MeshInfo, String> {
        Err("Mesh inspection is not supported by this transport".into())
    }
    fn mesh_read(
        &mut self,
        _event: u32,
        _selection: MeshSelection,
        _kind: &str,
        _offset: u64,
        _length: u32,
    ) -> Result<Vec<u8>, String> {
        Err("Mesh buffer reading is not supported by this transport".into())
    }
    fn close(&mut self) -> Result<(), String> {
        Ok(())
    }
}

impl Transport for Process {
    fn mesh(&mut self, event: u32, selection: MeshSelection) -> Result<MeshInfo, String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        writeln!(
            input,
            "mesh {} {} {}",
            selection.instance, selection.view, selection.stage
        )
        .and_then(|_| input.flush())
        .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::Mesh(info)
                if info.event_id == event
                    && info.instance == selection.instance
                    && info.view == selection.view
                    && info.stage == selection.stage
                    && [
                        &info.index_offset,
                        &info.index_size,
                        &info.vertex_offset,
                        &info.vertex_size,
                        &info.per_primitive_offset,
                    ]
                    .iter()
                    .all(|v| v.parse::<u64>().is_ok()) =>
            {
                Ok(info)
            }
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected mesh response".into()),
        }
    }
    fn mesh_read(
        &mut self,
        event: u32,
        selection: MeshSelection,
        kind: &str,
        offset: u64,
        length: u32,
    ) -> Result<Vec<u8>, String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        writeln!(
            input,
            "mesh_read {} {} {} {kind} {offset} {length}",
            selection.instance, selection.view, selection.stage
        )
        .and_then(|_| input.flush())
        .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::MeshData {
                event_id,
                kind: actual_kind,
                offset: actual_offset,
                hex,
            } if event_id == event
                && actual_kind == kind
                && actual_offset.parse::<u64>().ok() == Some(offset)
                && hex.len() % 2 == 0
                && hex.len() / 2 <= length as usize =>
            {
                decode_buffer_hex(&hex, (hex.len() / 2) as u32)
            }
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected mesh buffer response".into()),
        }
    }

    fn structure_buffer(
        &mut self,
        event: u32,
        path: &[u64],
        offset: u64,
        length: u32,
    ) -> Result<Vec<u8>, String> {
        let stamp = self
            .capture_stamp
            .as_ref()
            .ok_or("Capture identity is unavailable")?;
        if capture_stamp(&self.capture).as_ref() != Some(stamp) {
            return Err("Capture changed since it was opened".into());
        }
        let original_event = self
            .events
            .get(&event)
            .ok_or("Structured data is unavailable for this event")?
            .clone();
        if self.structure_worker.is_none() {
            let (worker, _) = Process::open(&self.executable, &self.capture, true)?;
            if worker.capture_stamp.as_ref() != Some(stamp) {
                return Err("Capture changed while opening structured data".into());
            }
            self.structure_worker = Some(Box::new(worker));
        }
        let worker = self
            .structure_worker
            .as_mut()
            .ok_or("Structured worker is unavailable")?;
        if worker.events.get(&event) != Some(&original_event) {
            return Err("Structured worker event does not match the open capture".into());
        }
        let bytes = worker.read_structure_buffer_raw(event, path, offset, length)?;
        if capture_stamp(&self.capture).as_ref() != Some(stamp) {
            self.structure_worker = None;
            return Err("Capture changed while reading structured data".into());
        }
        Ok(bytes)
    }

    fn structure(
        &mut self,
        event: u32,
        path: &[u64],
        offset: u64,
        limit: u32,
    ) -> Result<StructurePage, String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        write!(input, "structure {event} {offset} {limit}").map_err(|e| e.to_string())?;
        for index in path {
            write!(input, " {index}").map_err(|e| e.to_string())?;
        }
        writeln!(input)
            .and_then(|_| input.flush())
            .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::Structure(page) => {
                validate_structure_page(&page, event, path, offset, limit)?;
                Ok(page)
            }
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected structured event response".into()),
        }
    }

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

    fn save_texture(
        &mut self,
        index: u32,
        selection: TextureSelection,
        path: &Path,
    ) -> Result<(), String> {
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
        let command = if selection == TextureSelection::default() {
            format!("save {index} {encoded}")
        } else {
            let sample = selection
                .sample
                .map(|v| v.to_string())
                .unwrap_or_else(|| "resolve".into());
            let channel = selection
                .channel
                .map(|v| v.to_string())
                .unwrap_or_else(|| "all".into());
            format!(
                "save2 {index} {} {} {sample} {channel} {encoded}",
                selection.mip, selection.slice
            )
        };
        writeln!(input, "{command}")
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

    fn shader(&mut self, stage: u32, target: Option<u32>) -> Result<ShaderReport, String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        let target = target
            .map(|v| v.to_string())
            .unwrap_or_else(|| "default".into());
        writeln!(input, "shader {stage} {target}")
            .and_then(|_| input.flush())
            .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::Shader(report) if report.stage == stage => Ok(report),
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected shader response".into()),
        }
    }

    fn close(&mut self) -> Result<(), String> {
        let worker_result = self
            .structure_worker
            .take()
            .map(|mut worker| worker.close())
            .transpose();
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
        worker_result?;
        Ok(())
    }
}

fn validate_structure_page(
    page: &StructurePage,
    event: u32,
    path: &[u64],
    offset: u64,
    limit: u32,
) -> Result<(), String> {
    let page_path: Result<Vec<u64>, _> = page.path.iter().map(|s| s.parse()).collect();
    let count = page
        .node
        .child_count
        .parse::<u64>()
        .map_err(|_| "Invalid structured child count")?;
    if page.event_id != event
        || page.offset.parse::<u64>().ok() != Some(offset)
        || page_path.ok().as_deref() != Some(path)
        || offset > count
        || page.children.len() as u64 != (count - offset).min(u64::from(limit))
        || std::iter::once(&page.node)
            .chain(&page.children)
            .any(|node| {
                node.child_count.parse::<u64>().is_err() || node.byte_size.parse::<u64>().is_err()
            })
    {
        return Err("Invalid structured event response".into());
    }
    Ok(())
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
        let executable = std::fs::canonicalize(&executable).unwrap_or(executable);
        let (process, snapshot) = Process::open(&executable, capture, false)?;
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

    pub fn shader(&self, stage: u32, target: Option<u32>) -> Result<ShaderReport, String> {
        if stage as usize >= SHADER_STAGES.len() {
            return Err("Invalid shader stage".into());
        }
        self.process
            .lock()
            .map_err(|e| e.to_string())?
            .shader(stage, target)
    }

    pub fn mesh(&self, event: u32, selection: MeshSelection) -> Result<MeshInfo, String> {
        if !(1..=4).contains(&selection.stage) {
            return Err("Invalid mesh stage".into());
        }
        self.process
            .lock()
            .map_err(|e| e.to_string())?
            .mesh(event, selection)
    }

    pub fn mesh_read(
        &self,
        event: u32,
        selection: MeshSelection,
        kind: &str,
        offset: u64,
        length: u32,
    ) -> Result<Vec<u8>, String> {
        if !(1..=4).contains(&selection.stage)
            || !matches!(kind, "vertex" | "index")
            || length > BUFFER_PAGE_LIMIT
            || offset.checked_add(u64::from(length)).is_none()
        {
            return Err("Invalid mesh buffer request".into());
        }
        self.process
            .lock()
            .map_err(|e| e.to_string())?
            .mesh_read(event, selection, kind, offset, length)
    }

    pub fn structure(
        &self,
        event: u32,
        path: &[u64],
        offset: u64,
        limit: u32,
    ) -> Result<StructurePage, String> {
        if limit == 0 || limit > STRUCTURE_PAGE_LIMIT {
            return Err("Invalid structured page size".into());
        }
        self.process
            .lock()
            .map_err(|e| e.to_string())?
            .structure(event, path, offset, limit)
    }

    pub fn structure_buffer(
        &self,
        event: u32,
        path: &[u64],
        offset: u64,
        length: u32,
    ) -> Result<Vec<u8>, String> {
        if length > BUFFER_PAGE_LIMIT || offset.checked_add(u64::from(length)).is_none() {
            return Err("Invalid structured buffer range".into());
        }
        self.process
            .lock()
            .map_err(|e| e.to_string())?
            .structure_buffer(event, path, offset, length)
    }

    pub fn close(&self) -> Result<(), String> {
        self.process.lock().map_err(|e| e.to_string())?.close()
    }

    /// Export a PNG without replacing an existing destination, including symlinks.
    pub fn export_texture(&self, index: u32, destination: &Path) -> Result<(), String> {
        self.export_texture_selected(index, TextureSelection::default(), destination)
    }

    pub fn export_texture_selected(
        &self,
        index: u32,
        selection: TextureSelection,
        destination: &Path,
    ) -> Result<(), String> {
        if selection.mip > i32::MAX as u32
            || selection.slice > i32::MAX as u32
            || selection.channel.is_some_and(|v| v > i32::MAX as u32)
        {
            return Err("Texture selection is out of range".into());
        }
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
            .save_texture(index, selection, temporary.as_ref())?;
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
    struct RecordingTransport(Arc<Mutex<Vec<u32>>>, Arc<Mutex<Vec<TextureSelection>>>);
    impl Transport for RecordingTransport {
        fn mesh(&mut self, event: u32, selection: MeshSelection) -> Result<MeshInfo, String> {
            Ok(MeshInfo {
                event_id: event,
                instance: selection.instance,
                view: selection.view,
                stage: selection.stage,
                index_offset: "0".into(),
                index_size: "0".into(),
                vertex_bound: true,
                vertex_offset: "0".into(),
                vertex_size: "48".into(),
                vertex_stride: 16,
                per_primitive_offset: "0".into(),
                format_name: "R32G32B32A32_FLOAT".into(),
                component_type: 1,
                component_count: 4,
                component_byte_width: 4,
                num_indices: 3,
                unproject: true,
                ..MeshInfo::default()
            })
        }

        fn mesh_read(
            &mut self,
            _event: u32,
            _selection: MeshSelection,
            kind: &str,
            offset: u64,
            length: u32,
        ) -> Result<Vec<u8>, String> {
            if kind != "vertex" {
                return Err("Unavailable mesh buffer".into());
            }
            let mut bytes = Vec::new();
            for _ in 0..3 {
                for value in [3.0_f32, 3.0, 0.0, 1.0] {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
            }
            Ok(bytes
                .into_iter()
                .skip(usize::try_from(offset).unwrap_or(usize::MAX))
                .take(length as usize)
                .collect())
        }

        fn structure(
            &mut self,
            event: u32,
            path: &[u64],
            offset: u64,
            limit: u32,
        ) -> Result<StructurePage, String> {
            let make =
                |name: &str, basic_type: u32, children: &str, value: Option<&str>| StructuredNode {
                    name: name.into(),
                    type_name: "fixture".into(),
                    basic_type,
                    flags: 0,
                    byte_size: "4".into(),
                    child_count: children.into(),
                    value: value.map(str::to_owned),
                    custom_string: None,
                    resource_non_null: None,
                    resource_index: None,
                };
            let (node, all) = match path {
                [] => (
                    make("Call", 0, "130", None),
                    (0..130)
                        .map(|index| {
                            if index == 0 {
                                make("Topology", 6, "0", Some("4"))
                            } else if index == 1 {
                                let mut buffer = make("Upload", 4, "0", None);
                                buffer.byte_size = "300".into();
                                buffer
                            } else {
                                make(&format!("Param{index}"), 7, "0", Some("1"))
                            }
                        })
                        .collect(),
                ),
                [0] => (make("Topology", 6, "0", Some("4")), vec![]),
                [1] => {
                    let mut buffer = make("Upload", 4, "0", None);
                    buffer.byte_size = "300".into();
                    (buffer, vec![])
                }
                [index] if *index < 130 => {
                    (make(&format!("Param{index}"), 7, "0", Some("1")), vec![])
                }
                _ => return Err("Invalid path".into()),
            };
            if offset > all.len() as u64 || limit == 0 {
                return Err("Invalid page".into());
            }
            let children = all
                .into_iter()
                .skip(offset as usize)
                .take(limit as usize)
                .collect();
            Ok(StructurePage {
                event_id: event,
                path: path.iter().map(u64::to_string).collect(),
                offset: offset.to_string(),
                node,
                children,
            })
        }

        fn structure_buffer(
            &mut self,
            _event: u32,
            path: &[u64],
            offset: u64,
            length: u32,
        ) -> Result<Vec<u8>, String> {
            if path != [1] || offset > 300 || u64::from(length) > 300 - offset {
                return Err("Invalid structured buffer".into());
            }
            Ok((0..length)
                .map(|index| (offset + u64::from(index)) as u8)
                .collect())
        }

        fn select(&mut self, event: u32) -> Result<Pipeline, String> {
            self.0.lock().map_err(|e| e.to_string())?.push(event);
            Ok(Pipeline {
                event_id: event,
                available: true,
                topology: 5,
                shaders: SHADER_STAGES
                    .iter()
                    .enumerate()
                    .map(|(index, stage)| PipelineShader {
                        stage: (*stage).into(),
                        resource: PipelineResource {
                            bound: index == 0,
                            resource_index: None,
                            name: "fixture shader".into(),
                        },
                    })
                    .collect(),
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

        fn save_texture(
            &mut self,
            _index: u32,
            selection: TextureSelection,
            path: &Path,
        ) -> Result<(), String> {
            self.1.lock().map_err(|e| e.to_string())?.push(selection);
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

        fn shader(&mut self, stage: u32, target: Option<u32>) -> Result<ShaderReport, String> {
            if stage != 0 {
                return Err("No shader is bound to this stage".into());
            }
            if target.is_some_and(|value| value > 0) {
                return Err("Invalid shader target".into());
            }
            let event = *self
                .0
                .lock()
                .map_err(|e| e.to_string())?
                .last()
                .ok_or("No event")?;
            Ok(ShaderReport {
                event_id: event,
                stage,
                entry_point: "main".into(),
                encoding: 1,
                target: if target.is_some() {
                    "native".into()
                } else {
                    String::new()
                },
                targets: vec!["native".into()],
                disassembly: "fixture disassembly".into(),
                inputs: vec![],
                outputs: vec![],
                constant_blocks: 0,
                read_only_resources: 0,
                read_write_resources: 0,
                samplers: 0,
            })
        }
    }

    pub fn recording_session() -> (
        Session,
        Arc<Mutex<Vec<u32>>>,
        Arc<Mutex<Vec<TextureSelection>>>,
    ) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let selections = Arc::new(Mutex::new(Vec::new()));
        let session = Session {
            process: Arc::new(Mutex::new(Box::new(RecordingTransport(
                events.clone(),
                selections.clone(),
            )))),
            revision: Arc::new(AtomicU64::new(0)),
        };
        (session, events, selections)
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
    fn texture_selection_validates_mip_dependent_depth_and_components() {
        let texture = Texture {
            name: String::new(),
            width: 8,
            height: 8,
            depth: 8,
            dimension: 3,
            mips: 4,
            array_size: 1,
            samples: 1,
            bytes: "0".into(),
            format_name: String::new(),
            format_type: 0,
            component_type: 0,
            component_count: 4,
            component_byte_width: 1,
        };
        let mut selection = TextureSelection {
            mip: 2,
            slice: 1,
            sample: Some(0),
            channel: Some(3),
        };
        assert!(selection.validate(&texture).is_ok());
        selection.slice = 2;
        assert!(selection.validate(&texture).is_err());
        selection.slice = 0;
        selection.channel = Some(4);
        assert!(selection.validate(&texture).is_err());
        selection.channel = None;
        selection.sample = Some(1);
        assert!(selection.validate(&texture).is_err());
        selection.sample = None;
        selection.mip = u32::MAX;
        assert!(selection.validate(&texture).is_err());
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
        assert!(snapshot.actions[1].api_events.is_empty());
        assert_eq!(snapshot.texture_count, 2);
    }

    #[test]
    fn api_events_preserve_chunk_absence_and_large_offsets() {
        let mut value: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
        value["actions"][1]["api_events"] = serde_json::json!([
            {"event_id": 9, "chunk_index": null, "file_offset": "18446744073709551615", "name": ""},
            {"event_id": 11, "chunk_index": 23, "file_offset": "42", "name": "Draw()"}
        ]);
        let snapshot: Snapshot = serde_json::from_value(value).unwrap();
        validate_snapshot(&snapshot).unwrap();
        assert_eq!(snapshot.actions[1].api_events[0].chunk_index, None);
        assert_eq!(
            snapshot.actions[1].api_events[0].file_offset,
            u64::MAX.to_string()
        );
        assert_eq!(snapshot.actions[1].api_events[1].event_id, 11);
        assert_eq!(snapshot.actions[1].api_events[1].name, "Draw()");
    }

    #[test]
    fn capture_stamp_detects_file_changes() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(temp.path(), b"one").unwrap();
        let original = capture_stamp(temp.path()).expect("file timestamp");
        assert!(capture_stamp(temp.path()).as_ref() == Some(&original));
        std::fs::write(temp.path(), b"changed").unwrap();
        assert!(capture_stamp(temp.path()).as_ref() != Some(&original));
    }

    #[test]
    fn structured_pages_reject_mismatched_identity_and_malformed_ranges() {
        let node = serde_json::json!({"name":"x","type_name":"int","basic_type":7,"flags":0,
            "byte_size":"4","child_count":"2","value":"4","custom_string":null,
            "resource_non_null":null,"resource_index":null});
        let mut page: StructurePage = serde_json::from_value(serde_json::json!({"event_id":9,
            "offset":"1","path":["18446744073709551615"],"node":node.clone(),"children":[node]}))
        .unwrap();
        assert!(validate_structure_page(&page, 9, &[u64::MAX], 1, 128).is_ok());
        assert!(validate_structure_page(&page, 10, &[u64::MAX], 1, 128).is_err());
        assert!(validate_structure_page(&page, 9, &[0], 1, 128).is_err());
        assert!(validate_structure_page(&page, 9, &[u64::MAX], 2, 128).is_err());
        page.children[0].byte_size = "not-a-number".into();
        assert!(validate_structure_page(&page, 9, &[u64::MAX], 1, 128).is_err());
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
