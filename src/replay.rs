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
    #[serde(default)]
    pub base_vertex: Option<i32>,
    #[serde(default)]
    pub index_offset: Option<u32>,
    #[serde(default)]
    pub vertex_offset: Option<u32>,
    #[serde(default)]
    pub instance_offset: Option<u32>,
    #[serde(default)]
    pub dispatch_groups: Option<[u32; 3]>,
    #[serde(default)]
    pub dispatch_threads: Option<[u32; 3]>,
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
    #[serde(default)]
    pub index_restart: Option<u32>,
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
    #[serde(default)]
    pub frame_info: Option<FrameInfo>,
    #[serde(default)]
    pub debug_messages: Vec<DebugMessage>,
    #[serde(default)]
    pub frame_statistics: Option<serde_json::Value>,
    #[serde(default)]
    pub counters: Vec<CounterDescription>,
    #[serde(default)]
    pub descriptor_stores: Vec<DescriptorStore>,
    pub actions: Vec<Action>,
    pub resource_count: usize,
    pub texture_count: usize,
    pub buffer_count: usize,
    #[serde(default)]
    pub resources: Vec<ResourceInfo>,
    #[serde(default)]
    pub textures: Vec<Texture>,
    #[serde(default)]
    pub buffers: Vec<Buffer>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DescriptorStore {
    pub resource: Option<usize>,
    pub descriptor_byte_size: u32,
    pub first_offset: u32,
    pub count: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DescriptorPage {
    pub event_id: u32,
    pub kind: String,
    pub store_index: Option<usize>,
    pub offset: String,
    pub total: String,
    pub entries: Vec<serde_json::Value>,
}

pub const DESCRIPTOR_PAGE_LIMIT: u32 = 128;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AnnotationSection {
    pub name: String,
    pub exists: bool,
    pub version: String,
    pub hex: String,
}

impl AnnotationSection {
    pub fn document(&self) -> Result<serde_json::Value, String> {
        if !self.exists {
            return Ok(serde_json::json!({}));
        }
        if self.version != "1" {
            return Err("Unsupported annotation section version".into());
        }
        let len =
            u32::try_from(self.hex.len() / 2).map_err(|_| "Annotation section is too large")?;
        let bytes = decode_buffer_hex(&self.hex, len)?;
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("Invalid annotation JSON: {e}"))?;
        if !value.is_object() {
            return Err("Annotation section must contain a JSON object".into());
        }
        Ok(value)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CounterDescription {
    pub id: u32,
    pub name: String,
    pub category: String,
    pub description: String,
    pub result_type: u32,
    pub result_byte_width: u32,
    pub unit: u32,
    pub uuid: [u32; 4],
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CounterFetch {
    pub generation: String,
    pub total: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CounterEntry {
    pub event_id: u32,
    pub counter: u32,
    pub bits: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CounterPage {
    pub generation: String,
    pub offset: String,
    pub total: String,
    pub entries: Vec<CounterEntry>,
}

pub const COUNTER_PAGE_LIMIT: u32 = 128;

impl CounterEntry {
    pub fn display_value(&self, description: &CounterDescription) -> String {
        let Ok(bits) = self.bits.parse::<u64>() else {
            return "?".into();
        };
        match (description.result_type, description.result_byte_width) {
            (4, 4) => (bits as u32).to_string(),
            (4, 8) => bits.to_string(),
            (_, 4) => f32::from_bits(bits as u32).to_string(),
            (_, 8) => f64::from_bits(bits).to_string(),
            _ => "?".into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FrameInfo {
    pub file_offset: String,
    pub uncompressed_size: String,
    pub compressed_size: String,
    pub persistent_size: String,
    pub initial_data_size: String,
    pub capture_time: String,
    pub contains_annotations: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DebugMessage {
    pub event_id: u32,
    pub category: u32,
    pub severity: u32,
    pub source: u32,
    pub message_id: u32,
    pub description: String,
}

pub fn debug_message_source_name(source: u32) -> &'static str {
    const NAMES: &[&str] = &[
        "API",
        "Redundant API use",
        "Incorrect API use",
        "General performance",
        "GCN performance",
        "Runtime warning",
        "Unsupported configuration",
    ];
    NAMES.get(source as usize).copied().unwrap_or("Unknown")
}

pub fn debug_message_category_name(category: u32) -> &'static str {
    const NAMES: &[&str] = &[
        "Application defined",
        "Miscellaneous",
        "Initialization",
        "Cleanup",
        "Compilation",
        "State creation",
        "State setting",
        "State getting",
        "Resource manipulation",
        "Execution",
        "Shaders",
        "Deprecated",
        "Undefined",
        "Portability",
        "Performance",
    ];
    NAMES.get(category as usize).copied().unwrap_or("Unknown")
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResourceInfo {
    pub name: String,
    #[serde(default)]
    pub rename_key: Option<String>,
    #[serde(rename = "type")]
    pub resource_type: u32,
    pub autogenerated_name: bool,
    pub initialisation_chunks: Vec<u32>,
    pub parents: Vec<Option<usize>>,
    pub derived: Vec<Option<usize>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResourceUse {
    pub event_id: u32,
    pub usage: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResourceUsagePage {
    pub index: u32,
    pub offset: String,
    pub total: String,
    pub tracked: bool,
    pub entries: Vec<ResourceUse>,
}

pub const RESOURCE_USAGE_PAGE_LIMIT: u32 = 128;

pub fn resource_usage_name(usage: u32) -> &'static str {
    const NAMES: &[&str] = &[
        "Unused",
        "VertexBuffer",
        "IndexBuffer",
        "VS_Constants",
        "HS_Constants",
        "DS_Constants",
        "GS_Constants",
        "PS_Constants",
        "CS_Constants",
        "TS_Constants",
        "MS_Constants",
        "All_Constants",
        "StreamOut",
        "VS_Resource",
        "HS_Resource",
        "DS_Resource",
        "GS_Resource",
        "PS_Resource",
        "CS_Resource",
        "TS_Resource",
        "MS_Resource",
        "All_Resource",
        "VS_RWResource",
        "HS_RWResource",
        "DS_RWResource",
        "GS_RWResource",
        "PS_RWResource",
        "CS_RWResource",
        "TS_RWResource",
        "MS_RWResource",
        "All_RWResource",
        "InputTarget",
        "ColorTarget",
        "DepthStencilTarget",
        "Indirect",
        "Clear",
        "Discard",
        "GenMips",
        "Resolve",
        "ResolveSrc",
        "ResolveDst",
        "Copy",
        "CopySrc",
        "CopyDst",
        "Barrier",
        "CPUWrite",
    ];
    NAMES.get(usage as usize).copied().unwrap_or("Unknown")
}

pub fn resource_type_name(kind: u32) -> &'static str {
    const NAMES: &[&str] = &[
        "Unknown",
        "Device",
        "Queue",
        "CommandBuffer",
        "Texture",
        "Buffer",
        "View",
        "Sampler",
        "SwapchainImage",
        "Memory",
        "Shader",
        "ShaderBinding",
        "PipelineState",
        "StateObject",
        "RenderPass",
        "Query",
        "Sync",
        "Pool",
        "AccelerationStructure",
        "DescriptorStore",
    ];
    NAMES.get(kind as usize).copied().unwrap_or("Unknown")
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
pub struct ReflectedType {
    pub name: String,
    pub flags: u32,
    pub pointer_type_id: u32,
    pub elements: u32,
    pub array_stride: u32,
    pub base_type: u32,
    pub rows: u32,
    pub columns: u32,
    pub matrix_stride: u32,
    pub members: Vec<ReflectedConstant>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReflectedConstant {
    pub name: String,
    pub byte_offset: u32,
    pub bit_offset: u32,
    pub bit_size: u32,
    pub default_value: String,
    #[serde(rename = "type")]
    pub variable_type: ReflectedType,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReflectedConstantBlock {
    pub name: String,
    pub bind: u32,
    pub space: u32,
    pub array_size: u32,
    pub byte_size: u32,
    pub buffer_backed: bool,
    pub inline_data: bool,
    pub compile_constants: bool,
    pub variables: Vec<ReflectedConstant>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReflectedSampler {
    pub name: String,
    pub bind: u32,
    pub space: u32,
    pub array_size: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReflectedResource {
    pub name: String,
    pub bind: u32,
    pub space: u32,
    pub array_size: u32,
    pub texture_type: u32,
    pub descriptor_type: u32,
    pub is_texture: bool,
    pub has_sampler: bool,
    pub input_attachment: bool,
    pub read_only: bool,
    pub variable_type: ReflectedType,
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
    #[serde(default)]
    pub constant_block_details: Vec<ReflectedConstantBlock>,
    #[serde(default)]
    pub sampler_details: Vec<ReflectedSampler>,
    #[serde(default)]
    pub read_only_details: Vec<ReflectedResource>,
    #[serde(default)]
    pub read_write_details: Vec<ReflectedResource>,
    #[serde(default)]
    pub dispatch_threads: Option<[u32; 3]>,
    #[serde(default)]
    pub debuggable: Option<bool>,
    #[serde(default)]
    pub source_debug_info: Option<bool>,
    #[serde(default)]
    pub debug_status: Option<String>,
    #[serde(default)]
    pub source_files: Vec<ShaderSourceFile>,
    #[serde(default)]
    pub target_encodings: Vec<u32>,
    #[serde(default)]
    pub source_encoding: Option<u32>,
    #[serde(default)]
    pub source_entry: Option<String>,
    #[serde(default)]
    pub compile_flags: Vec<ShaderCompileFlag>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ShaderConstants {
    pub event_id: u32,
    pub stage: u32,
    pub block: u32,
    pub element: u32,
    pub byte_offset: String,
    pub byte_size: String,
    pub variables: Vec<DebugVariable>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ShaderBindings {
    pub event_id: u32,
    pub stage: u32,
    pub kind: String,
    pub offset: String,
    pub total: String,
    pub entries: Vec<serde_json::Value>,
}

pub const SHADER_BINDING_PAGE_LIMIT: u32 = 128;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ShaderCompileFlag {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ShaderEdit {
    pub event_id: u32,
    pub stage: u32,
    pub applied: bool,
    pub diagnostics: String,
    pub pipeline: Pipeline,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ShaderSourceFile {
    pub filename: String,
    pub contents: String,
}

/// RenderDoc variable data as raw native-endian ShaderValue bytes, including nested members.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DebugVariable {
    pub name: String,
    pub rows: u8,
    pub columns: u8,
    #[serde(rename = "type")]
    pub variable_type: u32,
    pub flags: u32,
    pub value_hex: String,
    pub members: Vec<DebugVariable>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DebugTrace {
    pub event_id: u32,
    pub stage: u32,
    pub inputs: Vec<DebugVariable>,
    pub constant_blocks: Vec<DebugVariable>,
    pub read_only_resources: Vec<DebugVariable>,
    pub read_write_resources: Vec<DebugVariable>,
    pub samplers: Vec<DebugVariable>,
    #[serde(default)]
    pub source_variables: Vec<SourceVariableMapping>,
    #[serde(default)]
    pub instructions: Vec<InstructionSourceInfo>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DebugVariableReference {
    pub name: String,
    #[serde(rename = "type")]
    pub variable_type: u32,
    pub component: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SourceVariableMapping {
    pub name: String,
    #[serde(rename = "type")]
    pub variable_type: u32,
    pub undefined: bool,
    pub rows: u32,
    pub columns: u32,
    pub offset: u32,
    pub signature_index: i32,
    pub variables: Vec<DebugVariableReference>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InstructionSourceInfo {
    pub instruction: u32,
    pub disassembly_line: u32,
    pub file_index: i32,
    pub line_start: u32,
    pub line_end: u32,
    pub column_start: u32,
    pub column_end: u32,
    pub source_variables: Vec<SourceVariableMapping>,
}

impl DebugTrace {
    /// Match ShaderViewer's lower-bound lookup, including its first-entry fallback.
    pub fn instruction_info(&self, instruction: u32) -> Option<&InstructionSourceInfo> {
        self.instructions
            .iter()
            .find(|entry| entry.instruction >= instruction)
            .or_else(|| self.instructions.first())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DebugChange {
    pub before: DebugVariable,
    pub after: DebugVariable,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DebugState {
    pub step: u32,
    pub next_instruction: u32,
    pub flags: u32,
    pub callstack: Vec<String>,
    pub changes: Vec<DebugChange>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DebugStates {
    pub event_id: u32,
    pub finished: bool,
    pub states: Vec<DebugState>,
}

/// `None` uses RenderDoc's NoPreference sentinel for the corresponding selector.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
pub struct PixelDebugSelection {
    pub x: u32,
    pub y: u32,
    pub sample: Option<u32>,
    pub primitive: Option<u32>,
    pub view: Option<u32>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct ComputeDebugSelection {
    pub group: [u32; 3],
    pub thread: [u32; 3],
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
    #[serde(default)]
    pub resource_index: Option<usize>,
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
    /// RenderDoc CompType for PNG preview/export.
    pub preview_type_cast: u32,
    /// RenderDoc CompType for picking, statistics and pixel history.
    pub analysis_type_cast: u32,
}

fn cast_suffix(value: u32) -> String {
    if value == 0 {
        String::new()
    } else {
        format!(" {value}")
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Pixel {
    pub event_id: u32,
    pub index: u32,
    pub mip: u32,
    pub slice: u32,
    pub sample: u32,
    pub x: u32,
    pub y: u32,
    pub bits: [u32; 4],
    pub float_values: [String; 4],
    pub uint_values: [u32; 4],
    pub int_values: [i32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistogramRequest {
    pub min: f32,
    pub max: f32,
    pub channels: [bool; 4],
}

impl HistogramRequest {
    pub fn validate(&self) -> Result<(), String> {
        if !self.min.is_finite()
            || !self.max.is_finite()
            || self.min >= self.max
            || !(self.max - self.min).is_finite()
            || !(self.max + self.max * 1e-6).is_finite()
        {
            return Err("Invalid histogram range".into());
        }
        Ok(())
    }
    fn mask(self) -> u32 {
        self.channels
            .into_iter()
            .enumerate()
            .fold(0, |mask, (index, enabled)| {
                mask | (u32::from(enabled) << index)
            })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PixelComponents {
    pub bits: [u32; 4],
    pub float_values: [String; 4],
    pub uint_values: [u32; 4],
    pub int_values: [i32; 4],
}

impl From<[u32; 4]> for PixelComponents {
    fn from(bits: [u32; 4]) -> Self {
        Self {
            bits,
            float_values: bits.map(|v| f32::from_bits(v).to_string()),
            uint_values: bits,
            int_values: bits.map(|v| v as i32),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextureStatistics {
    pub event_id: u32,
    pub index: u32,
    pub mip: u32,
    pub slice: u32,
    pub sample: u32,
    pub request: HistogramRequest,
    pub min: PixelComponents,
    pub max: PixelComponents,
    pub histogram: Vec<u32>,
}

pub const PIXEL_HISTORY_PAGE_LIMIT: u32 = 128;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryValue {
    pub color_bits: [u32; 4],
    pub depth_bits: u32,
    pub stencil: i32,
    #[serde(default)]
    pub color: Option<PixelComponents>,
    #[serde(default)]
    pub depth: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FragmentTests {
    pub sample_masked: bool,
    pub backface_culled: bool,
    pub depth_clipped: bool,
    pub depth_bounds_failed: bool,
    pub view_clipped: bool,
    pub scissor_clipped: bool,
    pub shader_discarded: bool,
    pub depth_test_failed: bool,
    pub stencil_test_failed: bool,
    pub predication_skipped: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PixelModification {
    pub event_id: u32,
    pub fragment: u32,
    pub primitive: u32,
    pub direct_shader_write: bool,
    pub unbound_pixel_shader: bool,
    pub passed: bool,
    pub pre: Option<HistoryValue>,
    pub shader_output: Option<HistoryValue>,
    pub post: Option<HistoryValue>,
    pub tests: FragmentTests,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PixelHistoryPage {
    pub event_id: u32,
    pub index: u32,
    pub mip: u32,
    pub slice: u32,
    pub sample: u32,
    pub x: u32,
    pub y: u32,
    pub offset: String,
    pub total: String,
    pub entries: Vec<PixelModification>,
}

impl TextureSelection {
    pub fn validate(&self, texture: &Texture) -> Result<(), String> {
        let slices = if texture.dimension == 3 {
            texture.depth.checked_shr(self.mip).unwrap_or(0).max(1)
        } else {
            texture.array_size
        };
        if self.mip >= texture.mips
            || self.preview_type_cast > 9
            || self.analysis_type_cast > 9
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

    pub fn validate_pixel(&self, texture: &Texture, x: u32, y: u32) -> Result<(), String> {
        self.validate(texture)?;
        if self.mip >= 32 || texture.width == 0 || texture.height == 0 || texture.samples == 0 {
            return Err("Invalid texture dimensions or sample count".into());
        }
        if x >= (texture.width >> self.mip).max(1) || y >= (texture.height >> self.mip).max(1) {
            return Err("Pixel coordinates are out of range".into());
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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PipelineAttachment {
    pub resource: PipelineResource,
    pub view: PipelineResource,
    #[serde(rename = "type")]
    pub descriptor_type: u32,
    pub flags: u32,
    pub format: String,
    #[serde(default)]
    pub format_component_type: Option<u32>,
    pub texture_type: u32,
    pub first_mip: u32,
    pub mip_count: u32,
    pub first_slice: u32,
    pub slice_count: u32,
    pub min_lod_clamp: Option<f32>,
    pub swizzle: [u32; 4],
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
    #[serde(default)]
    pub format_type: Option<u32>,
    #[serde(default)]
    pub component_type: Option<u32>,
    #[serde(default)]
    pub component_count: Option<u32>,
    #[serde(default)]
    pub component_byte_width: Option<u32>,
    #[serde(default)]
    pub element_size: Option<u32>,
    #[serde(default)]
    pub bgra_order: Option<bool>,
    #[serde(default)]
    pub float_cast_wrong: Option<bool>,
    #[serde(default)]
    pub generic_bits: Option<Vec<u32>>,
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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PipelineScissor {
    pub enabled: bool,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PipelineRaster {
    pub front_ccw: bool,
    pub fill: u32,
    pub cull: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PipelineDepth {
    pub enabled: bool,
    pub function: u32,
    pub writes: bool,
    pub bounds_enabled: bool,
    pub min_bounds: Option<f64>,
    pub max_bounds: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PipelineStencilFace {
    pub fail: u32,
    pub depth_fail: u32,
    pub pass: u32,
    pub function: u32,
    pub reference: u32,
    pub compare_mask: u32,
    pub write_mask: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PipelineBlendEquation {
    pub source: u32,
    pub destination: u32,
    pub operation: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PipelineBlend {
    pub enabled: bool,
    pub logic_enabled: bool,
    pub logic_operation: u32,
    pub write_mask: u32,
    pub color: PipelineBlendEquation,
    pub alpha: PipelineBlendEquation,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PipelineFixedState {
    pub raster: PipelineRaster,
    pub depth: PipelineDepth,
    pub stencil_enabled: bool,
    pub stencil_front: PipelineStencilFace,
    pub stencil_back: PipelineStencilFace,
    pub independent_blend: bool,
    pub blend_factor: [Option<f32>; 4],
    pub blends: Vec<PipelineBlend>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Pipeline {
    pub event_id: u32,
    pub available: bool,
    pub topology: u32,
    #[serde(default)]
    pub multiview_count: Option<u32>,
    #[serde(default)]
    pub restart_enabled: Option<bool>,
    #[serde(default)]
    pub restart_index: Option<u32>,
    pub shaders: Vec<PipelineShader>,
    pub outputs: Vec<PipelineResource>,
    #[serde(default)]
    pub color_attachments: Option<Vec<PipelineAttachment>>,
    #[serde(default)]
    pub depth_attachment: Option<PipelineAttachment>,
    #[serde(default)]
    pub depth_resolve_attachment: Option<PipelineAttachment>,
    #[serde(default)]
    pub index_buffer: Option<PipelineBuffer>,
    #[serde(default)]
    pub vertex_buffers: Option<Vec<PipelineBuffer>>,
    #[serde(default)]
    pub vertex_inputs: Option<Vec<VertexInput>>,
    #[serde(default)]
    pub viewports: Option<Vec<PipelineViewport>>,
    #[serde(default)]
    pub scissors: Option<Vec<PipelineScissor>>,
    #[serde(default)]
    pub fixed_state: Option<PipelineFixedState>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Reply {
    Snapshot(Snapshot),
    AnnotationSection(AnnotationSection),
    Descriptors(DescriptorPage),
    DebugMessages {
        messages: Vec<DebugMessage>,
    },
    CounterFetch(CounterFetch),
    CounterPage(CounterPage),
    ResourceUsage(ResourceUsagePage),
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
    ShaderConstants(ShaderConstants),
    ShaderBindings(ShaderBindings),
    ShaderEdit(ShaderEdit),
    DebugStarted(DebugTrace),
    DebugStates(DebugStates),
    DebugStopped,
    Structure(StructurePage),
    Mesh(MeshInfo),
    MeshData {
        event_id: u32,
        kind: String,
        offset: String,
        hex: String,
    },
    Pixel {
        event_id: u32,
        index: u32,
        mip: u32,
        slice: u32,
        sample: u32,
        x: u32,
        y: u32,
        bits: [u32; 4],
    },
    TextureStats {
        event_id: u32,
        index: u32,
        mip: u32,
        slice: u32,
        sample: u32,
        range_min_bits: u32,
        range_max_bits: u32,
        channel_mask: u32,
        min_bits: [u32; 4],
        max_bits: [u32; 4],
        histogram: Vec<u32>,
    },
    PixelHistory(PixelHistoryPage),
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
    if snapshot.descriptor_stores.iter().any(|store| {
        store
            .resource
            .is_some_and(|index| index >= snapshot.resource_count)
    }) {
        return Err("Invalid descriptor store resource".into());
    }
    if !snapshot.resources.is_empty() {
        if snapshot.resources.len() != snapshot.resource_count {
            return Err("Invalid resource count".into());
        }
        for resource in &snapshot.resources {
            if resource
                .parents
                .iter()
                .chain(&resource.derived)
                .flatten()
                .any(|index| *index >= snapshot.resources.len())
            {
                return Err("Invalid resource relationship".into());
            }
        }
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
    fn annotation_read(&mut self, _name: &str) -> Result<AnnotationSection, String> {
        Err("Annotation sections are not supported by this transport".into())
    }
    fn annotation_write(
        &mut self,
        _section: &AnnotationSection,
        _document: &serde_json::Value,
    ) -> Result<AnnotationSection, String> {
        Err("Annotation sections are not supported by this transport".into())
    }
    fn descriptors(
        &mut self,
        _event: u32,
        _store: Option<usize>,
        _offset: u64,
        _limit: u32,
    ) -> Result<DescriptorPage, String> {
        Err("Descriptor inspection is not supported by this transport".into())
    }
    fn debug_messages(&mut self) -> Result<Vec<DebugMessage>, String> {
        Err("Debug messages are not supported by this transport".into())
    }
    fn counter_fetch(&mut self, _ids: &[u32]) -> Result<CounterFetch, String> {
        Err("GPU counters are not supported by this transport".into())
    }
    fn counter_page(
        &mut self,
        _generation: u64,
        _offset: u64,
        _limit: u32,
    ) -> Result<CounterPage, String> {
        Err("GPU counters are not supported by this transport".into())
    }
    fn resource_usage(
        &mut self,
        _index: u32,
        _offset: u64,
        _limit: u32,
    ) -> Result<ResourceUsagePage, String> {
        Err("Resource usage is not supported by this transport".into())
    }
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
    fn pick_pixel(
        &mut self,
        _event: u32,
        _index: u32,
        _selection: TextureSelection,
        _sample: u32,
        _x: u32,
        _y: u32,
    ) -> Result<Pixel, String> {
        Err("Pixel picking is not supported by this transport".into())
    }
    fn texture_statistics(
        &mut self,
        _event: u32,
        _index: u32,
        _selection: TextureSelection,
        _sample: u32,
        _request: HistogramRequest,
    ) -> Result<TextureStatistics, String> {
        Err("Texture statistics are not supported by this transport".into())
    }
    fn pixel_history(
        &mut self,
        _event: u32,
        _index: u32,
        _selection: TextureSelection,
        _sample: u32,
        _x: u32,
        _y: u32,
        _offset: u64,
        _limit: u32,
    ) -> Result<PixelHistoryPage, String> {
        Err("Pixel history is not supported by this transport".into())
    }
    fn shader(&mut self, _stage: u32, _target: Option<u32>) -> Result<ShaderReport, String> {
        Err("Shader inspection is not supported by this transport".into())
    }
    fn shader_constants(
        &mut self,
        _event: u32,
        _stage: u32,
        _block: u32,
        _element: u32,
    ) -> Result<ShaderConstants, String> {
        Err("Shader constant inspection is not supported by this transport".into())
    }
    fn shader_bindings(
        &mut self,
        _event: u32,
        _stage: u32,
        _kind: &str,
        _offset: u64,
        _limit: u32,
    ) -> Result<ShaderBindings, String> {
        Err("Shader binding inspection is not supported by this transport".into())
    }
    fn shader_edit(
        &mut self,
        _event: u32,
        _stage: u32,
        _replacement: Option<(u32, &str, &[u8])>,
        _flags: Option<&[ShaderCompileFlag]>,
    ) -> Result<ShaderEdit, String> {
        Err("Shader editing is not supported by this transport".into())
    }
    fn debug_vertex(
        &mut self,
        _vertex: u32,
        _instance: u32,
        _index: u32,
        _view: u32,
    ) -> Result<DebugTrace, String> {
        Err("Shader debugging is not supported by this transport".into())
    }
    fn debug_pixel(&mut self, _selection: PixelDebugSelection) -> Result<DebugTrace, String> {
        Err("Shader debugging is not supported by this transport".into())
    }
    fn debug_compute(&mut self, _selection: ComputeDebugSelection) -> Result<DebugTrace, String> {
        Err("Shader debugging is not supported by this transport".into())
    }
    fn debug_next(&mut self) -> Result<DebugStates, String> {
        Err("Shader debugging is not supported by this transport".into())
    }
    fn debug_stop(&mut self) -> Result<(), String> {
        Err("Shader debugging is not supported by this transport".into())
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
    fn annotation_read(&mut self, name: &str) -> Result<AnnotationSection, String> {
        if capture_stamp(&self.capture).as_ref() != self.capture_stamp.as_ref()
            || self.capture_stamp.is_none()
        {
            return Err("Capture changed since it was opened".into());
        }
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        writeln!(input, "section_read {name}")
            .and_then(|_| input.flush())
            .map_err(|e| e.to_string())?;
        let section = match self.read()? {
            Reply::AnnotationSection(section) if section.name == name => section,
            Reply::Error { message } => return Err(message),
            _ => return Err("Unexpected annotation section response".into()),
        };
        if capture_stamp(&self.capture).as_ref() != self.capture_stamp.as_ref() {
            return Err("Capture changed while reading annotations".into());
        }
        section.document()?;
        Ok(section)
    }

    fn annotation_write(
        &mut self,
        section: &AnnotationSection,
        document: &serde_json::Value,
    ) -> Result<AnnotationSection, String> {
        if !document.is_object() {
            return Err("Annotation document must be an object".into());
        }
        section.document()?;
        if capture_stamp(&self.capture).as_ref() != self.capture_stamp.as_ref()
            || self.capture_stamp.is_none()
        {
            return Err("Capture changed since it was opened".into());
        }
        self.structure_worker = None;
        let before = self.capture_stamp.clone();
        if capture_stamp(&self.capture) != before {
            return Err("Capture changed while preparing annotation save".into());
        }
        let bytes = serde_json::to_vec(document).map_err(|e| e.to_string())?;
        let encoded: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let expected = if !section.exists {
            "-"
        } else if section.hex.is_empty() {
            "_"
        } else {
            &section.hex
        };
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        writeln!(
            input,
            "section_write {} {expected} {}",
            section.name,
            if encoded.is_empty() { "_" } else { &encoded }
        )
        .and_then(|_| input.flush())
        .map_err(|e| e.to_string())?;
        let response = self.read();
        let after = capture_stamp(&self.capture);
        match response {
            Ok(Reply::AnnotationSection(saved))
                if saved.name == section.name
                    && saved.exists
                    && saved.version == "1"
                    && saved.hex == encoded
                    && after.is_some() =>
            {
                self.capture_stamp = after;
                Ok(saved)
            }
            other => {
                if after != before {
                    self.capture_stamp = None;
                }
                match other {
                    Err(message) | Ok(Reply::Error { message }) => Err(message),
                    _ => Err("Unexpected annotation save response".into()),
                }
            }
        }
    }
    fn descriptors(
        &mut self,
        event: u32,
        store: Option<usize>,
        offset: u64,
        limit: u32,
    ) -> Result<DescriptorPage, String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        let (kind, index) = store.map_or(("access", 0), |index| ("store", index));
        writeln!(input, "descriptors {kind} {index} {offset} {limit}")
            .and_then(|_| input.flush())
            .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::Descriptors(page) => {
                let total = page
                    .total
                    .parse::<u64>()
                    .map_err(|_| "Invalid descriptor count")?;
                if page.event_id != event
                    || page.kind != kind
                    || page.store_index != store
                    || page.offset != offset.to_string()
                    || offset > total
                    || page.entries.len() as u64 != (total - offset).min(u64::from(limit))
                {
                    return Err("Invalid descriptor page".into());
                }
                Ok(page)
            }
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected descriptor page response".into()),
        }
    }
    fn debug_messages(&mut self) -> Result<Vec<DebugMessage>, String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        writeln!(input, "debug_messages")
            .and_then(|_| input.flush())
            .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::DebugMessages { messages } => Ok(messages),
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected debug messages response".into()),
        }
    }
    fn counter_fetch(&mut self, ids: &[u32]) -> Result<CounterFetch, String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        write!(input, "counter_fetch").map_err(|e| e.to_string())?;
        for id in ids {
            write!(input, " {id}").map_err(|e| e.to_string())?;
        }
        writeln!(input)
            .and_then(|_| input.flush())
            .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::CounterFetch(fetch)
                if fetch.generation.parse::<u64>().is_ok()
                    && fetch.total.parse::<u64>().is_ok() =>
            {
                Ok(fetch)
            }
            Reply::Error { message } => Err(message),
            _ => Err("Invalid GPU counter fetch response".into()),
        }
    }

    fn counter_page(
        &mut self,
        generation: u64,
        offset: u64,
        limit: u32,
    ) -> Result<CounterPage, String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        writeln!(input, "counter_page {generation} {offset} {limit}")
            .and_then(|_| input.flush())
            .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::CounterPage(page) => {
                let total = page
                    .total
                    .parse::<u64>()
                    .map_err(|_| "Invalid GPU counter total")?;
                if page.generation != generation.to_string()
                    || page.offset != offset.to_string()
                    || offset > total
                    || page.entries.len() as u64 != (total - offset).min(u64::from(limit))
                    || page
                        .entries
                        .iter()
                        .any(|entry| entry.bits.parse::<u64>().is_err())
                {
                    return Err("Invalid GPU counter page".into());
                }
                Ok(page)
            }
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected GPU counter page response".into()),
        }
    }
    fn pixel_history(
        &mut self,
        event: u32,
        index: u32,
        selection: TextureSelection,
        sample: u32,
        x: u32,
        y: u32,
        offset: u64,
        limit: u32,
    ) -> Result<PixelHistoryPage, String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        let requested = selection
            .sample
            .map(|v| v.to_string())
            .unwrap_or_else(|| "resolve".into());
        writeln!(
            input,
            "history {index} {} {} {requested} {x} {y} {offset} {limit}{}",
            selection.mip,
            selection.slice,
            cast_suffix(selection.analysis_type_cast)
        )
        .and_then(|_| input.flush())
        .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::PixelHistory(mut page) => {
                let total = page
                    .total
                    .parse::<u64>()
                    .map_err(|_| "Invalid pixel history total")?;
                if page.event_id != event
                    || page.index != index
                    || page.mip != selection.mip
                    || page.slice != selection.slice
                    || page.sample != sample
                    || page.x != x
                    || page.y != y
                    || page.offset != offset.to_string()
                    || offset > total
                    || page.entries.len() as u64 != (total - offset).min(u64::from(limit))
                    || page.entries.iter().any(|entry| entry.event_id > event)
                {
                    return Err("Invalid pixel history response".into());
                }
                for entry in &mut page.entries {
                    for value in [&mut entry.pre, &mut entry.shader_output, &mut entry.post]
                        .into_iter()
                        .flatten()
                    {
                        value.color = Some(value.color_bits.into());
                        value.depth = Some(f32::from_bits(value.depth_bits).to_string());
                    }
                }
                Ok(page)
            }
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected pixel history response".into()),
        }
    }
    fn texture_statistics(
        &mut self,
        event: u32,
        index: u32,
        selection: TextureSelection,
        sample: u32,
        request: HistogramRequest,
    ) -> Result<TextureStatistics, String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        let requested = selection
            .sample
            .map(|v| v.to_string())
            .unwrap_or_else(|| "resolve".into());
        writeln!(
            input,
            "stats {index} {} {} {requested} {} {} {}{}",
            selection.mip,
            selection.slice,
            request.min.to_bits(),
            request.max.to_bits(),
            request.mask(),
            cast_suffix(selection.analysis_type_cast)
        )
        .and_then(|_| input.flush())
        .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::TextureStats {
                event_id,
                index: actual,
                mip,
                slice,
                sample: actual_sample,
                range_min_bits,
                range_max_bits,
                channel_mask,
                min_bits,
                max_bits,
                histogram,
            } if event_id == event
                && actual == index
                && mip == selection.mip
                && slice == selection.slice
                && actual_sample == sample
                && range_min_bits == request.min.to_bits()
                && range_max_bits == request.max.to_bits()
                && channel_mask == request.mask() =>
            {
                Ok(TextureStatistics {
                    event_id,
                    index,
                    mip,
                    slice,
                    sample,
                    request,
                    min: min_bits.into(),
                    max: max_bits.into(),
                    histogram,
                })
            }
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected texture statistics response".into()),
        }
    }
    fn resource_usage(
        &mut self,
        index: u32,
        offset: u64,
        limit: u32,
    ) -> Result<ResourceUsagePage, String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        writeln!(input, "usage {index} {offset} {limit}")
            .and_then(|_| input.flush())
            .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::ResourceUsage(page) => {
                let total = page
                    .total
                    .parse::<u64>()
                    .map_err(|_| "Invalid resource usage total")?;
                if page.index != index
                    || page.offset != offset.to_string()
                    || offset > total
                    || page.entries.len() as u64 != (total - offset).min(u64::from(limit))
                    || (!page.tracked
                        && (total != 1
                            || (offset == 0
                                && (page.entries[0].event_id != 0 || page.entries[0].usage != 0))))
                {
                    return Err("Invalid resource usage response".into());
                }
                Ok(page)
            }
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected resource usage response".into()),
        }
    }
    fn pick_pixel(
        &mut self,
        event: u32,
        index: u32,
        selection: TextureSelection,
        sample: u32,
        x: u32,
        y: u32,
    ) -> Result<Pixel, String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        let requested = selection
            .sample
            .map(|sample| sample.to_string())
            .unwrap_or_else(|| "resolve".into());
        writeln!(
            input,
            "pixel {index} {} {} {requested} {x} {y}{}",
            selection.mip,
            selection.slice,
            cast_suffix(selection.analysis_type_cast)
        )
        .and_then(|_| input.flush())
        .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::Pixel {
                event_id,
                index: actual,
                mip,
                slice,
                sample: actual_sample,
                x: actual_x,
                y: actual_y,
                bits,
            } if event_id == event
                && actual == index
                && mip == selection.mip
                && slice == selection.slice
                && actual_sample == sample
                && actual_x == x
                && actual_y == y =>
            {
                Ok(Pixel {
                    event_id,
                    index,
                    mip,
                    slice,
                    sample,
                    x,
                    y,
                    bits,
                    float_values: bits.map(|bits| f32::from_bits(bits).to_string()),
                    uint_values: bits,
                    int_values: bits.map(|bits| bits as i32),
                })
            }
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected pixel response".into()),
        }
    }
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
            let suffix = cast_suffix(selection.preview_type_cast);
            format!(
                "save2 {index} {} {} {sample} {channel} {encoded}{suffix}",
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

    fn shader_constants(
        &mut self,
        event: u32,
        stage: u32,
        block: u32,
        element: u32,
    ) -> Result<ShaderConstants, String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        writeln!(input, "consts {stage} {block} {element}")
            .and_then(|_| input.flush())
            .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::ShaderConstants(values)
                if values.event_id == event
                    && values.stage == stage
                    && values.block == block
                    && values.element == element =>
            {
                Ok(values)
            }
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected shader constants response".into()),
        }
    }

    fn shader_bindings(
        &mut self,
        event: u32,
        stage: u32,
        kind: &str,
        offset: u64,
        limit: u32,
    ) -> Result<ShaderBindings, String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        writeln!(input, "bindings {stage} {kind} {offset} {limit}")
            .and_then(|_| input.flush())
            .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::ShaderBindings(page) => {
                let total = page
                    .total
                    .parse::<u64>()
                    .map_err(|_| "Invalid shader binding total")?;
                if page.event_id != event
                    || page.stage != stage
                    || page.kind != kind
                    || page.offset != offset.to_string()
                    || offset > total
                    || page.entries.len() as u64 != (total - offset).min(u64::from(limit))
                {
                    return Err("Invalid shader bindings response".into());
                }
                Ok(page)
            }
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected shader bindings response".into()),
        }
    }

    fn shader_edit(
        &mut self,
        event: u32,
        stage: u32,
        replacement: Option<(u32, &str, &[u8])>,
        flags: Option<&[ShaderCompileFlag]>,
    ) -> Result<ShaderEdit, String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        if let Some((encoding, entry, source)) = replacement {
            let hex = |bytes: &[u8]| {
                const DIGITS: &[u8; 16] = b"0123456789abcdef";
                let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
                for &byte in bytes {
                    encoded.push(DIGITS[(byte >> 4) as usize] as char);
                    encoded.push(DIGITS[(byte & 15) as usize] as char);
                }
                encoded
            };
            let mut command = format!(
                "shader_apply {event} {stage} {encoding} {} {}",
                hex(entry.as_bytes()),
                hex(source)
            );
            if let Some(flags) = flags {
                command.push_str(&format!(" {}", flags.len()));
                for flag in flags {
                    let name = if flag.name.is_empty() {
                        "_".into()
                    } else {
                        hex(flag.name.as_bytes())
                    };
                    let value = if flag.value.is_empty() {
                        "_".into()
                    } else {
                        hex(flag.value.as_bytes())
                    };
                    command.push_str(&format!(" {name} {value}"));
                }
            }
            writeln!(input, "{command}")
        } else {
            writeln!(input, "shader_revert {event} {stage}")
        }
        .and_then(|_| input.flush())
        .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::ShaderEdit(edit)
                if edit.event_id == event
                    && edit.stage == stage
                    && edit.pipeline.event_id == event =>
            {
                Ok(edit)
            }
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected shader edit response".into()),
        }
    }

    fn debug_vertex(
        &mut self,
        vertex: u32,
        instance: u32,
        index: u32,
        view: u32,
    ) -> Result<DebugTrace, String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        writeln!(input, "debug_vertex {vertex} {instance} {index} {view}")
            .and_then(|_| input.flush())
            .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::DebugStarted(trace) if trace.stage == 0 => Ok(trace),
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected shader debug response".into()),
        }
    }

    fn debug_pixel(&mut self, selection: PixelDebugSelection) -> Result<DebugTrace, String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        writeln!(
            input,
            "debug_pixel {} {} {} {} {}",
            selection.x,
            selection.y,
            selection.sample.unwrap_or(u32::MAX),
            selection.primitive.unwrap_or(u32::MAX),
            selection.view.unwrap_or(u32::MAX)
        )
        .and_then(|_| input.flush())
        .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::DebugStarted(trace) if trace.stage == 4 => Ok(trace),
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected shader debug response".into()),
        }
    }

    fn debug_compute(&mut self, selection: ComputeDebugSelection) -> Result<DebugTrace, String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        let [gx, gy, gz] = selection.group;
        let [tx, ty, tz] = selection.thread;
        writeln!(input, "debug_compute {gx} {gy} {gz} {tx} {ty} {tz}")
            .and_then(|_| input.flush())
            .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::DebugStarted(trace) if trace.stage == 5 => Ok(trace),
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected shader debug response".into()),
        }
    }

    fn debug_next(&mut self) -> Result<DebugStates, String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        writeln!(input, "debug_next")
            .and_then(|_| input.flush())
            .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::DebugStates(states) => Ok(states),
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected shader debug states response".into()),
        }
    }

    fn debug_stop(&mut self) -> Result<(), String> {
        let input = self.input.as_mut().ok_or("Replay session has closed")?;
        writeln!(input, "debug_stop")
            .and_then(|_| input.flush())
            .map_err(|e| e.to_string())?;
        match self.read()? {
            Reply::DebugStopped => Ok(()),
            Reply::Error { message } => Err(message),
            _ => Err("Unexpected shader debug stop response".into()),
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
    pub fn annotation_read(&self, name: &str) -> Result<AnnotationSection, String> {
        if name != "notes" && name != "bookmarks" && name != "renames" && name != "edits" {
            return Err("Unknown annotation section".into());
        }
        self.process
            .lock()
            .map_err(|e| e.to_string())?
            .annotation_read(name)
    }
    pub fn annotation_write(
        &self,
        section: &AnnotationSection,
        document: &serde_json::Value,
    ) -> Result<AnnotationSection, String> {
        if section.name != "notes"
            && section.name != "bookmarks"
            && section.name != "renames"
            && section.name != "edits"
        {
            return Err("Unknown annotation section".into());
        }
        if section.name == "bookmarks" {
            crate::annotations::bookmarks(&section.document()?)?;
            crate::annotations::bookmarks(document)?;
        } else if section.name == "renames" {
            crate::annotations::resource_name(&section.document()?, "ResourceId::1")?;
            crate::annotations::resource_name(document, "ResourceId::1")?;
        } else if section.name == "edits" {
            crate::annotations::shader_editors(&section.document()?)?;
            crate::annotations::shader_editors(document)?;
        } else if section.name == "notes"
            && document
                .get("comments")
                .is_some_and(|value| !value.is_string())
        {
            return Err("Comments must be a string".into());
        }
        self.process
            .lock()
            .map_err(|e| e.to_string())?
            .annotation_write(section, document)
    }
    pub fn descriptors(
        &self,
        event: u32,
        store: Option<usize>,
        offset: u64,
        limit: u32,
    ) -> Result<DescriptorPage, String> {
        if limit == 0 || limit > DESCRIPTOR_PAGE_LIMIT {
            return Err("Invalid descriptor page limit".into());
        }
        self.process
            .lock()
            .map_err(|e| e.to_string())?
            .descriptors(event, store, offset, limit)
    }
    /// Drain messages queued by the replay driver since the previous read.
    pub fn debug_messages(&self) -> Result<Vec<DebugMessage>, String> {
        self.process
            .lock()
            .map_err(|e| e.to_string())?
            .debug_messages()
    }
    pub fn counter_fetch(&self, ids: &[u32]) -> Result<CounterFetch, String> {
        if ids.is_empty()
            || ids
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != ids.len()
        {
            return Err("GPU counter selection must not be empty or contain duplicates".into());
        }
        self.process
            .lock()
            .map_err(|e| e.to_string())?
            .counter_fetch(ids)
    }

    pub fn counter_page(
        &self,
        generation: u64,
        offset: u64,
        limit: u32,
    ) -> Result<CounterPage, String> {
        if limit == 0 || limit > COUNTER_PAGE_LIMIT {
            return Err("Invalid GPU counter page limit".into());
        }
        self.process
            .lock()
            .map_err(|e| e.to_string())?
            .counter_page(generation, offset, limit)
    }
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

    pub fn resource_usage(
        &self,
        index: u32,
        offset: u64,
        limit: u32,
    ) -> Result<ResourceUsagePage, String> {
        if limit == 0 || limit > RESOURCE_USAGE_PAGE_LIMIT {
            return Err("Invalid resource usage page limit".into());
        }
        self.process
            .lock()
            .map_err(|e| e.to_string())?
            .resource_usage(index, offset, limit)
    }

    pub fn pick_pixel(
        &self,
        event: u32,
        index: u32,
        texture: &Texture,
        selection: TextureSelection,
        x: u32,
        y: u32,
    ) -> Result<Pixel, String> {
        selection.validate_pixel(texture, x, y)?;
        let sample = selection
            .sample
            .unwrap_or(if texture.samples > 1 { u32::MAX } else { 0 });
        let revision = self.revision.load(Ordering::SeqCst);
        let mut process = self.process.lock().map_err(|e| e.to_string())?;
        if self.revision.load(Ordering::SeqCst) != revision {
            return Err("Replay selection changed".into());
        }
        let pixel = process.pick_pixel(event, index, selection, sample, x, y)?;
        if self.revision.load(Ordering::SeqCst) != revision {
            return Err("Replay selection changed".into());
        }
        Ok(pixel)
    }

    pub fn texture_statistics(
        &self,
        event: u32,
        index: u32,
        texture: &Texture,
        selection: TextureSelection,
        request: HistogramRequest,
    ) -> Result<TextureStatistics, String> {
        selection.validate_pixel(texture, 0, 0)?;
        request.validate()?;
        let sample = selection
            .sample
            .unwrap_or(if texture.samples > 1 { u32::MAX } else { 0 });
        let revision = self.revision.load(Ordering::SeqCst);
        let mut process = self.process.lock().map_err(|e| e.to_string())?;
        if self.revision.load(Ordering::SeqCst) != revision {
            return Err("Replay selection changed".into());
        }
        let result = process.texture_statistics(event, index, selection, sample, request)?;
        if self.revision.load(Ordering::SeqCst) != revision {
            return Err("Replay selection changed".into());
        }
        Ok(result)
    }

    pub fn pixel_history(
        &self,
        event: u32,
        index: u32,
        texture: &Texture,
        selection: TextureSelection,
        x: u32,
        y: u32,
        offset: u64,
        limit: u32,
    ) -> Result<PixelHistoryPage, String> {
        selection.validate_pixel(texture, x, y)?;
        if limit == 0 || limit > PIXEL_HISTORY_PAGE_LIMIT {
            return Err("Invalid pixel history page limit".into());
        }
        let sample = selection
            .sample
            .unwrap_or(if texture.samples > 1 { u32::MAX } else { 0 });
        let revision = self.revision.load(Ordering::SeqCst);
        let mut process = self.process.lock().map_err(|e| e.to_string())?;
        if self.revision.load(Ordering::SeqCst) != revision {
            return Err("Replay selection changed".into());
        }
        let result = process.pixel_history(event, index, selection, sample, x, y, offset, limit)?;
        if self.revision.load(Ordering::SeqCst) != revision {
            return Err("Replay selection changed".into());
        }
        Ok(result)
    }

    pub fn vertex_input_page(
        &self,
        action: &Action,
        pipeline: &Pipeline,
        buffers: &[Buffer],
        instance: u32,
        offset: u32,
        limit: u32,
    ) -> Result<crate::vertex_input::InputPage, String> {
        let revision = self.revision.load(Ordering::SeqCst);
        let mut process = self.process.lock().map_err(|e| e.to_string())?;
        if self.revision.load(Ordering::SeqCst) != revision {
            return Err("Replay selection changed".into());
        }
        let page = crate::vertex_input::inspect(
            action,
            pipeline,
            buffers,
            instance,
            offset,
            limit,
            |index, offset, length| process.read_buffer(index, offset, length),
        )?;
        if self.revision.load(Ordering::SeqCst) != revision {
            return Err("Replay selection changed".into());
        }
        Ok(page)
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

    pub fn shader_constants(
        &self,
        event: u32,
        stage: u32,
        block: u32,
        element: u32,
    ) -> Result<ShaderConstants, String> {
        if stage as usize >= SHADER_STAGES.len() {
            return Err("Invalid shader stage".into());
        }
        self.process
            .lock()
            .map_err(|e| e.to_string())?
            .shader_constants(event, stage, block, element)
    }

    pub fn shader_bindings(
        &self,
        event: u32,
        stage: u32,
        kind: &str,
        offset: u64,
        limit: u32,
    ) -> Result<ShaderBindings, String> {
        if stage as usize >= SHADER_STAGES.len()
            || !matches!(kind, "constants" | "read_only" | "read_write" | "samplers")
            || limit == 0
            || limit > SHADER_BINDING_PAGE_LIMIT
        {
            return Err("Invalid shader bindings request".into());
        }
        self.process
            .lock()
            .map_err(|e| e.to_string())?
            .shader_bindings(event, stage, kind, offset, limit)
    }

    pub fn shader_edit(
        &self,
        event: u32,
        stage: u32,
        replacement: Option<(u32, &str, &[u8])>,
    ) -> Result<ShaderEdit, String> {
        self.shader_edit_with_flags(event, stage, replacement, None)
    }

    pub fn shader_edit_with_flags(
        &self,
        event: u32,
        stage: u32,
        replacement: Option<(u32, &str, &[u8])>,
        flags: Option<&[ShaderCompileFlag]>,
    ) -> Result<ShaderEdit, String> {
        if stage as usize >= SHADER_STAGES.len() {
            return Err("Invalid shader stage".into());
        }
        if replacement.is_some_and(|(_, entry, source)| {
            entry.is_empty() || entry.contains('\0') || source.is_empty()
        }) {
            return Err("Shader entry point and source must be nonempty".into());
        }
        self.next_revision();
        self.process.lock().map_err(|e| e.to_string())?.shader_edit(
            event,
            stage,
            replacement,
            flags,
        )
    }

    pub fn debug_vertex(
        &self,
        vertex: u32,
        instance: u32,
        index: u32,
        view: u32,
    ) -> Result<DebugTrace, String> {
        self.process
            .lock()
            .map_err(|e| e.to_string())?
            .debug_vertex(vertex, instance, index, view)
    }

    pub fn debug_pixel(&self, selection: PixelDebugSelection) -> Result<DebugTrace, String> {
        if [selection.sample, selection.primitive, selection.view]
            .into_iter()
            .flatten()
            .any(|value| value == u32::MAX)
        {
            return Err("Use null for RenderDoc's NoPreference selector".into());
        }
        self.process
            .lock()
            .map_err(|e| e.to_string())?
            .debug_pixel(selection)
    }

    pub fn debug_compute(&self, selection: ComputeDebugSelection) -> Result<DebugTrace, String> {
        self.process
            .lock()
            .map_err(|e| e.to_string())?
            .debug_compute(selection)
    }

    pub fn debug_next(&self) -> Result<DebugStates, String> {
        self.process.lock().map_err(|e| e.to_string())?.debug_next()
    }

    pub fn debug_stop(&self) -> Result<(), String> {
        self.process.lock().map_err(|e| e.to_string())?.debug_stop()
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
        fn pixel_history(
            &mut self,
            event: u32,
            index: u32,
            selection: TextureSelection,
            sample: u32,
            x: u32,
            y: u32,
            offset: u64,
            limit: u32,
        ) -> Result<PixelHistoryPage, String> {
            if offset > 2 {
                return Err("Invalid history offset".into());
            }
            let bits = [
                0.25_f32.to_bits(),
                0.5_f32.to_bits(),
                0.75_f32.to_bits(),
                1.0_f32.to_bits(),
            ];
            let value = HistoryValue {
                color_bits: bits,
                depth_bits: (-1.0_f32).to_bits(),
                stencil: -1,
                color: Some(bits.into()),
                depth: Some("-1".into()),
            };
            let all = [
                PixelModification {
                    event_id: 41,
                    fragment: 0,
                    primitive: 0,
                    direct_shader_write: false,
                    unbound_pixel_shader: false,
                    passed: true,
                    pre: Some(value.clone()),
                    shader_output: None,
                    post: Some(value.clone()),
                    tests: FragmentTests::default(),
                },
                PixelModification {
                    event_id: 42,
                    fragment: 0,
                    primitive: 1,
                    direct_shader_write: false,
                    unbound_pixel_shader: false,
                    passed: false,
                    pre: Some(value.clone()),
                    shader_output: Some(value.clone()),
                    post: Some(value),
                    tests: FragmentTests {
                        scissor_clipped: true,
                        ..FragmentTests::default()
                    },
                },
            ];
            Ok(PixelHistoryPage {
                event_id: event,
                index,
                mip: selection.mip,
                slice: selection.slice,
                sample,
                x,
                y,
                offset: offset.to_string(),
                total: "2".into(),
                entries: all
                    .into_iter()
                    .skip(offset as usize)
                    .take(limit as usize)
                    .collect(),
            })
        }
        fn texture_statistics(
            &mut self,
            event: u32,
            index: u32,
            selection: TextureSelection,
            sample: u32,
            request: HistogramRequest,
        ) -> Result<TextureStatistics, String> {
            let mut histogram = vec![0; 256];
            histogram[64] = 16;
            let min = [
                0.25_f32.to_bits(),
                0.5_f32.to_bits(),
                0.75_f32.to_bits(),
                1.0_f32.to_bits(),
            ];
            Ok(TextureStatistics {
                event_id: event,
                index,
                mip: selection.mip,
                slice: selection.slice,
                sample,
                request,
                min: min.into(),
                max: min.into(),
                histogram,
            })
        }
        fn resource_usage(
            &mut self,
            index: u32,
            offset: u64,
            limit: u32,
        ) -> Result<ResourceUsagePage, String> {
            if index > 1 || offset > 2 {
                return Err("Invalid resource".into());
            }
            let all = [
                ResourceUse {
                    event_id: 42,
                    usage: 1,
                },
                ResourceUse {
                    event_id: 41,
                    usage: 2,
                },
            ];
            Ok(ResourceUsagePage {
                index,
                offset: offset.to_string(),
                total: "2".into(),
                tracked: true,
                entries: all
                    .into_iter()
                    .skip(offset as usize)
                    .take(limit as usize)
                    .collect(),
            })
        }
        fn pick_pixel(
            &mut self,
            event: u32,
            index: u32,
            selection: TextureSelection,
            sample: u32,
            x: u32,
            y: u32,
        ) -> Result<Pixel, String> {
            let bits = [
                0.25_f32.to_bits(),
                0.5_f32.to_bits(),
                0.75_f32.to_bits(),
                1.0_f32.to_bits(),
            ];
            Ok(Pixel {
                event_id: event,
                index,
                mip: selection.mip,
                slice: selection.slice,
                sample,
                x,
                y,
                bits,
                float_values: bits.map(|v| f32::from_bits(v).to_string()),
                uint_values: bits,
                int_values: bits.map(|v| v as i32),
            })
        }
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
                constant_blocks: 1,
                read_only_resources: 0,
                read_write_resources: 0,
                samplers: 0,
                constant_block_details: vec![ReflectedConstantBlock {
                    name: "Fixture constants".into(),
                    bind: 0,
                    space: 0,
                    array_size: 2,
                    byte_size: 16,
                    buffer_backed: true,
                    inline_data: false,
                    compile_constants: false,
                    variables: Vec::new(),
                }],
                sampler_details: Vec::new(),
                read_only_details: Vec::new(),
                read_write_details: Vec::new(),
                dispatch_threads: None,
                debuggable: None,
                source_debug_info: None,
                debug_status: None,
                target_encodings: vec![2, 5],
                source_encoding: Some(5),
                source_entry: Some("main".into()),
                compile_flags: Vec::new(),
                source_files: vec![
                    ShaderSourceFile {
                        filename: "fixture.hlsl".into(),
                        contents: (0..205).map(|i| format!("line {i}\n")).collect(),
                    },
                    ShaderSourceFile {
                        filename: "include.hlsl".into(),
                        contents: "included\n".into(),
                    },
                ],
            })
        }

        fn shader_constants(
            &mut self,
            event: u32,
            stage: u32,
            block: u32,
            element: u32,
        ) -> Result<ShaderConstants, String> {
            if stage != 0 || block != 0 || element >= 2 {
                return Err("Invalid fixture constants".into());
            }
            Ok(ShaderConstants {
                event_id: event,
                stage,
                block,
                element,
                byte_offset: "0".into(),
                byte_size: "16".into(),
                variables: vec![DebugVariable {
                    name: "fixture_value".into(),
                    rows: 1,
                    columns: 1,
                    variable_type: 4,
                    flags: 0,
                    value_hex: format!("{:02x}000000", element + 42),
                    members: Vec::new(),
                }],
            })
        }

        fn shader_bindings(
            &mut self,
            event: u32,
            stage: u32,
            kind: &str,
            offset: u64,
            _limit: u32,
        ) -> Result<ShaderBindings, String> {
            if stage != 0 || kind != "read_write" || offset != 0 {
                return Err("Invalid fixture bindings".into());
            }
            Ok(ShaderBindings {
                event_id: event,
                stage,
                kind: kind.into(),
                offset: "0".into(),
                total: "1".into(),
                entries: vec![
                    serde_json::json!({"binding":0,"array_element":0,"statically_unused":false,"descriptor":{"resource":0}}),
                ],
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

    #[test]
    fn reflected_constants_preserve_nested_layout_and_u64_defaults() {
        let scalar = serde_json::json!({"name":"float","flags":0,"pointer_type_id":u32::MAX,"elements":1,"array_stride":0,"base_type":1,"rows":1,"columns":1,"matrix_stride":0,"members":[]});
        let member = serde_json::json!({"name":"value","byte_offset":4,"bit_offset":0,"bit_size":0,"default_value":"18446744073709551615","type":scalar});
        let outer = serde_json::json!({"name":"struct","flags":0,"pointer_type_id":u32::MAX,"elements":1,"array_stride":0,"base_type":0,"rows":1,"columns":1,"matrix_stride":0,"members":[member]});
        let value: ReflectedType = serde_json::from_value(outer.clone()).unwrap();
        assert_eq!(value.members[0].byte_offset, 4);
        assert_eq!(value.members[0].default_value, "18446744073709551615");
        assert_eq!(serde_json::to_value(value).unwrap(), outer);
    }
    use std::{io::Cursor, sync::Mutex};

    const SNAPSHOT: &str = r#"{"type":"snapshot","protocol":1,"api":3,"frame":7,"degraded":false,"actions":[{"event_id":10,"action_id":1,"depth":0,"parent":-1,"flags":0,"indices":0,"instances":0,"name":"pass"},{"event_id":11,"action_id":2,"depth":1,"parent":0,"flags":2,"indices":3,"instances":1,"name":"draw"}],"resource_count":4,"texture_count":2,"buffer_count":1}"#;

    #[test]
    fn instruction_mapping_matches_upstream_lower_bound_and_fallback() {
        let trace: DebugTrace = serde_json::from_value(serde_json::json!({
            "event_id":1,"stage":5,"inputs":[],"constant_blocks":[],"read_only_resources":[],"read_write_resources":[],"samplers":[],
            "instructions":[
                {"instruction":3,"disassembly_line":8,"file_index":-1,"line_start":0,"line_end":0,"column_start":0,"column_end":0,"source_variables":[]},
                {"instruction":7,"disassembly_line":12,"file_index":0,"line_start":4,"line_end":4,"column_start":1,"column_end":5,"source_variables":[]}
            ]
        })).unwrap();
        assert_eq!(trace.instruction_info(2).unwrap().disassembly_line, 8);
        assert_eq!(trace.instruction_info(3).unwrap().disassembly_line, 8);
        assert_eq!(trace.instruction_info(6).unwrap().disassembly_line, 12);
        assert_eq!(trace.instruction_info(7).unwrap().disassembly_line, 12);
        assert_eq!(trace.instruction_info(9).unwrap().disassembly_line, 8);
    }

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
            resource_index: None,
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
            preview_type_cast: 0,
            analysis_type_cast: 0,
        };
        assert!(selection.validate(&texture).is_ok());
        selection.preview_type_cast = 10;
        assert!(selection.validate(&texture).is_err());
        selection.preview_type_cast = 0;
        selection.analysis_type_cast = 10;
        assert!(selection.validate(&texture).is_err());
        selection.analysis_type_cast = 0;
        assert!(selection.validate_pixel(&texture, 1, 1).is_ok());
        assert!(selection.validate_pixel(&texture, 2, 1).is_err());
        assert!(selection.validate_pixel(&texture, 1, 2).is_err());
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
    fn histogram_rejects_nonfinite_or_degenerate_ranges() {
        let mut request = HistogramRequest {
            min: 0.0,
            max: 1.0,
            channels: [true, false, true, false],
        };
        assert!(request.validate().is_ok());
        assert_eq!(request.mask(), 5);
        request.max = 0.0;
        assert!(request.validate().is_err());
        request.max = f32::INFINITY;
        assert!(request.validate().is_err());
        request.min = -f32::MAX;
        request.max = f32::MAX;
        assert!(request.validate().is_err());
    }

    #[test]
    fn gpu_counter_values_preserve_integer_and_float_bits() {
        let mut description = CounterDescription {
            id: 1,
            name: "counter".into(),
            category: String::new(),
            description: String::new(),
            result_type: 4,
            result_byte_width: 8,
            unit: 0,
            uuid: [0; 4],
        };
        let entry = CounterEntry {
            event_id: 3,
            counter: 1,
            bits: u64::MAX.to_string(),
        };
        assert_eq!(entry.display_value(&description), u64::MAX.to_string());
        description.result_type = 1;
        description.result_byte_width = 4;
        let entry = CounterEntry {
            bits: f32::INFINITY.to_bits().to_string(),
            ..entry
        };
        assert_eq!(entry.display_value(&description), "inf");
        let reply = read_reply(&mut Cursor::new("RENDERDOCX/1 {\"type\":\"counter_page\",\"generation\":\"1\",\"offset\":\"0\",\"total\":\"1\",\"entries\":[{\"event_id\":3,\"counter\":1,\"bits\":\"18446744073709551615\"}]}\n")).unwrap();
        let Reply::CounterPage(page) = reply else {
            panic!("expected counter page")
        };
        assert_eq!(page.entries[0].bits, u64::MAX.to_string());
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
    fn debug_messages_reply_preserves_metadata_and_text() {
        let reply = read_reply(&mut Cursor::new("RENDERDOCX/1 {\"type\":\"debug_messages\",\"messages\":[{\"event_id\":42,\"category\":14,\"severity\":1,\"source\":3,\"message_id\":99,\"description\":\"性能 warning\"}]}\n")).unwrap();
        let Reply::DebugMessages { messages } = reply else {
            panic!("expected messages")
        };
        assert_eq!(messages[0].event_id, 42);
        assert_eq!(messages[0].description, "性能 warning");
        assert_eq!(
            debug_message_source_name(messages[0].source),
            "General performance"
        );
        assert_eq!(
            debug_message_category_name(messages[0].category),
            "Performance"
        );
    }

    #[test]
    fn descriptor_page_preserves_64_bit_ranges_and_unmapped_resources() {
        let json = "RENDERDOCX/1 {\"type\":\"descriptors\",\"event_id\":15,\"kind\":\"access\",\"store_index\":null,\"offset\":\"0\",\"total\":\"1\",\"entries\":[{\"byte_offset\":3,\"access\":{\"stage\":4,\"statically_unused\":false},\"descriptor\":{\"resource\":null,\"resource_present\":true,\"byte_size\":\"18446744073709551615\"},\"sampler\":null,\"location\":null}]}\n";
        let Reply::Descriptors(page) = read_reply(&mut Cursor::new(json)).unwrap() else {
            panic!("expected descriptors")
        };
        assert_eq!(page.event_id, 15);
        assert_eq!(page.entries[0]["descriptor"]["resource_present"], true);
        assert_eq!(
            page.entries[0]["descriptor"]["byte_size"],
            u64::MAX.to_string()
        );
    }

    #[test]
    fn resource_relationships_are_checked_and_usage_names_match_upstream() {
        let mut snapshot: Snapshot = serde_json::from_str(SNAPSHOT).unwrap();
        snapshot.resource_count = 2;
        snapshot.resources = serde_json::from_value(serde_json::json!([
            {"name":"buffer", "type":5, "autogenerated_name":false, "initialisation_chunks":[7], "parents":[null], "derived":[1]},
            {"name":"view", "type":6, "autogenerated_name":true, "initialisation_chunks":[], "parents":[0], "derived":[]}
        ])).unwrap();
        assert!(validate_snapshot(&snapshot).is_ok());
        assert_eq!(resource_usage_name(1), "VertexBuffer");
        assert_eq!(resource_usage_name(35), "Clear");
        assert_eq!(resource_usage_name(u32::MAX), "Unknown");
        snapshot.resources[0].derived.push(Some(2));
        assert!(validate_snapshot(&snapshot).is_err());
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
