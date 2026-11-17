//! Presentation helpers for upstream shader-printed messages.
use crate::{
    i18n::Language,
    replay::{SHADER_MESSAGE_PAGE_LIMIT, ShaderMessage, ShaderMessagePage},
};
use std::io::Write;

#[derive(Clone)]
pub struct Filter {
    pub stage_mask: u16,
    pub query: String,
    pub language: Language,
}

impl Filter {
    pub fn matches(&self, message: &ShaderMessage) -> bool {
        self.matches_normalized(message, &self.query.trim().to_lowercase())
    }

    fn matches_normalized(&self, message: &ShaderMessage, query: &str) -> bool {
        let stage_matches = if message.stage < 16 {
            self.stage_mask & (1u16 << message.stage) != 0
        } else {
            self.stage_mask == u16::MAX
        };
        if !stage_matches {
            return false;
        }
        query.is_empty()
            || message.message.to_lowercase().contains(query)
            || location(message, self.language)
                .to_lowercase()
                .contains(query)
    }
}

/// Reads the full event in bounded pages. No partial result is accepted if the
/// event, offset, total, or page size changes during the scan.
pub fn scan(
    event: u32,
    filter: &Filter,
    mut fetch: impl FnMut(u64, u32) -> Result<ShaderMessagePage, String>,
    mut visit: impl FnMut(&ShaderMessage) -> Result<(), String>,
) -> Result<u64, String> {
    let mut offset = 0u64;
    let mut expected_total = None;
    let mut matched = 0u64;
    let query = filter.query.trim().to_lowercase();
    loop {
        let page = fetch(offset, SHADER_MESSAGE_PAGE_LIMIT)?;
        let total = page
            .total
            .parse::<u64>()
            .map_err(|_| "Invalid shader message count")?;
        if page.event_id != event
            || page.offset != offset.to_string()
            || expected_total.is_some_and(|value| value != total)
            || offset > total
            || page.messages.len() as u64
                != (total - offset).min(u64::from(SHADER_MESSAGE_PAGE_LIMIT))
        {
            return Err("Shader message pages changed during the scan".into());
        }
        expected_total = Some(total);
        for message in &page.messages {
            if filter.matches_normalized(message, &query) {
                visit(message)?;
                matched = matched
                    .checked_add(1)
                    .ok_or("Shader message count overflow")?;
            }
        }
        offset = offset
            .checked_add(page.messages.len() as u64)
            .ok_or("Shader message offset overflow")?;
        if offset == total {
            return Ok(matched);
        }
    }
}

pub fn filtered_page(
    event: u32,
    offset: u64,
    limit: u32,
    filter: &Filter,
    mut fetch: impl FnMut(u64, u32) -> Result<ShaderMessagePage, String>,
) -> Result<ShaderMessagePage, String> {
    if limit == 0 || limit > SHADER_MESSAGE_PAGE_LIMIT {
        return Err("Invalid shader message page limit".into());
    }
    if filter.stage_mask == u16::MAX && filter.query.trim().is_empty() {
        return fetch(offset, limit);
    }
    let mut seen = 0u64;
    let mut messages = Vec::new();
    let total = scan(event, filter, &mut fetch, |message| {
        if seen >= offset && messages.len() < limit as usize {
            messages.push(message.clone());
        }
        seen = seen.checked_add(1).ok_or("Shader message count overflow")?;
        Ok(())
    })?;
    if offset > total {
        return Err("Shader message offset is out of range".into());
    }
    Ok(ShaderMessagePage {
        event_id: event,
        offset: offset.to_string(),
        total: total.to_string(),
        messages,
    })
}

/// Export every matching row, not just the displayed page.
pub fn write_report(
    event: u32,
    filter: &Filter,
    csv: bool,
    mut writer: impl Write,
    fetch: impl FnMut(u64, u32) -> Result<ShaderMessagePage, String>,
) -> Result<u64, String> {
    if csv {
        writeln!(
            writer,
            "\"{}\",\"{}\",\"{}\"",
            filter.language.text("Stage", "阶段"),
            filter.language.text("Location", "位置"),
            filter
                .language
                .text("Message", "消息")
        )
        .map_err(|error| error.to_string())?;
    }
    let result = scan(event, filter, fetch, |message| {
        let stage = stage_name(message.stage, filter.language);
        let location = location(message, filter.language);
        if csv {
            let quote = |value: &str| format!("\"{}\"", value.replace('"', "\"\""));
            writeln!(
                writer,
                "{},{},{}",
                quote(stage),
                quote(&location),
                quote(&message.message)
            )
        } else {
            writeln!(writer, "{stage}\t{location}\t{}", message.message)
        }
        .map_err(|error| error.to_string())
    })?;
    writer.flush().map_err(|error| error.to_string())?;
    Ok(result)
}

/// Publish a complete report only after every page and write succeeds.
pub fn export_report(
    event: u32,
    filter: &Filter,
    csv: bool,
    destination: &std::path::Path,
    fetch: impl FnMut(u64, u32) -> Result<ShaderMessagePage, String>,
) -> Result<u64, String> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut temporary = tempfile::Builder::new()
        .prefix(".renderdocx-shader-messages-")
        .tempfile_in(parent)
        .map_err(|error| error.to_string())?;
    let count = {
        let writer = std::io::BufWriter::new(temporary.as_file_mut());
        write_report(event, filter, csv, writer, fetch)?
    };
    temporary
        .persist_noclobber(destination)
        .map_err(|error| error.error.to_string())?;
    Ok(count)
}

#[derive(Clone, Copy, Debug)]
pub enum DebugInvocation {
    Pixel(crate::replay::PixelDebugSelection),
    Compute(crate::replay::ComputeDebugSelection),
    Mesh(crate::replay::ComputeDebugSelection),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Navigation {
    Vertex {
        reported: u32,
        instance: u32,
        view: u32,
    },
    Mesh {
        stage: u32,
        row: u32,
        instance: u32,
        view: u32,
    },
    TaskGroup {
        group: [u32; 3],
    },
    MeshGroup {
        group: [u32; 3],
        task_group: [u32; 3],
    },
    Pixel {
        x: u32,
        y: u32,
        sample: Option<u32>,
        primitive: Option<u32>,
        view: u32,
    },
}

/// Vulkan post-task and post-mesh buffers use x-major dispatch group order.
fn dispatch_index(group: [u32; 3], dims: [u32; 3]) -> Result<u32, &'static str> {
    if dims.contains(&0) || group.iter().zip(dims).any(|(&g, d)| g >= d) {
        return Err("Dispatch group is out of range");
    }
    group[2]
        .checked_mul(dims[1])
        .and_then(|z| z.checked_add(group[1]))
        .and_then(|yz| yz.checked_mul(dims[0]))
        .and_then(|v| v.checked_add(group[0]))
        .ok_or("Dispatch group index overflow")
}

pub fn task_group_row(
    api: u32,
    info: &crate::replay::MeshInfo,
    group: [u32; 3],
) -> Result<u32, &'static str> {
    if api != 3 || info.stage != 3 {
        return Err("Task group mapping is only available for Vulkan task output");
    }
    let dims = info
        .dispatch_size
        .ok_or("Task dispatch dimensions are unavailable")?;
    let count = dims
        .iter()
        .try_fold(1u32, |n, &d| n.checked_mul(d))
        .ok_or("Task dispatch size overflow")?;
    if count != info.num_indices || info.tasks.len() != count as usize {
        return Err("Task output does not match dispatch dimensions");
    }
    dispatch_index(group, dims)
}

pub fn direct_meshlet(
    api: u32,
    action: &crate::replay::Action,
    pipeline: &crate::replay::Pipeline,
    info: &crate::replay::MeshInfo,
    group: [u32; 3],
    task_group: [u32; 3],
) -> Result<u32, &'static str> {
    if api != 3 || info.stage != 4 {
        return Err("Mesh group mapping is only available for Vulkan mesh output");
    }
    // Upstream Vulkan feedback currently writes a placeholder task ID when a
    // task shader is present; it cannot identify the parent task group.
    if task_group != [u32::MAX; 3] {
        return Err("Mesh message parent task group is unavailable");
    }
    if !pipeline.available
        || pipeline.event_id != action.event_id
        || info.event_id != action.event_id
    {
        return Err("Mesh pipeline belongs to another event");
    }
    let mut task_shaders = pipeline
        .shaders
        .iter()
        .filter(|shader| shader.stage == "task");
    let Some(task_shader) = task_shaders.next() else {
        return Err("Task shader binding is unavailable");
    };
    if task_shaders.next().is_some() {
        return Err("Task shader binding is ambiguous");
    }
    if task_shader.resource.bound {
        return Err("Mesh message parent task group is unavailable");
    }
    let dims = action
        .dispatch_groups
        .ok_or("Mesh dispatch dimensions are unavailable")?;
    let total = dims
        .iter()
        .try_fold(1u32, |n, &d| n.checked_mul(d))
        .ok_or("Mesh dispatch size overflow")?;
    let meshlet = dispatch_index(group, dims)?;
    let first = info.meshlet_offset;
    let end = first
        .checked_add(u32::try_from(info.meshlets.len()).map_err(|_| "Meshlet count overflow")?)
        .ok_or("Meshlet range overflow")?;
    if end > total || meshlet < first || meshlet >= end {
        return Err("Mesh group is unavailable in this output");
    }
    Ok(meshlet)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelOutput {
    pub texture: usize,
    pub mip: u32,
    pub slice: u32,
    pub type_cast: u32,
    pub sample: Option<u32>,
}

/// Follow color slot zero when any color output is bound, otherwise depth,
/// matching ShaderMessageViewer's output selection.
pub fn pixel_output(
    snapshot: &crate::replay::Snapshot,
    pipeline: &crate::replay::Pipeline,
    view: u32,
    sample: Option<u32>,
) -> Option<PixelOutput> {
    let colors = pipeline.color_attachments.as_ref()?;
    let attachment = if colors.iter().any(|attachment| attachment.resource.bound) {
        colors.first()?
    } else {
        pipeline.depth_attachment.as_ref()?
    };
    if !attachment.resource.bound {
        return None;
    }
    let resource = attachment.resource.resource_index?;
    let index = snapshot
        .textures
        .iter()
        .position(|texture| texture.resource_index == Some(resource))?;
    let texture = &snapshot.textures[index];
    if attachment.first_mip >= texture.mips || texture.samples == 0 {
        return None;
    }
    let slices = if texture.dimension == 3 {
        texture
            .depth
            .checked_shr(attachment.first_mip)
            .unwrap_or(0)
            .max(1)
    } else {
        texture.array_size
    };
    if slices == 0 {
        return None;
    }
    Some(PixelOutput {
        texture: index,
        mip: attachment.first_mip,
        slice: view.min(slices - 1),
        type_cast: attachment.format_component_type.unwrap_or(0),
        sample: if texture.samples > 1 {
            // SetSelectedSubresource clamps the upstream NoLocation sentinel.
            Some(sample.unwrap_or(u32::MAX).min(texture.samples - 1))
        } else {
            None
        },
    })
}

/// Mirrors RenderDoc's RENDERDOC_VertexOffset for supported topology values.
/// Reject unknown topology or arithmetic overflow rather than choosing another row.
pub fn primitive_vertex_offset(topology: u32, primitive: u32) -> Option<u32> {
    match topology {
        1 => Some(primitive),
        2 => primitive.checked_mul(2),
        3 | 4 | 6 | 7 | 9 => Some(primitive),
        5 => primitive.checked_mul(3),
        8 => primitive.checked_mul(4),
        10 => primitive.checked_mul(6),
        11 => primitive.checked_mul(2),
        12..=43 => primitive.checked_mul(topology - 11),
        _ => None,
    }
}

pub fn navigation(message: &ShaderMessage, topology: u32) -> Option<Navigation> {
    let valid = |key| number(message, key).filter(|value| *value != u32::MAX);
    match message.stage {
        0 => Some(Navigation::Vertex {
            reported: valid("vertex")?,
            instance: valid("instance")?,
            view: valid("view")?,
        }),
        3 => Some(Navigation::Mesh {
            stage: 2,
            row: primitive_vertex_offset(topology, valid("primitive")?)?,
            instance: 0,
            view: valid("view")?,
        }),
        4 => {
            let sample = number(message, "sample")?;
            let primitive = number(message, "primitive")?;
            Some(Navigation::Pixel {
                x: valid("x")?,
                y: valid("y")?,
                sample: (sample != u32::MAX).then_some(sample),
                primitive: (primitive != u32::MAX).then_some(primitive),
                view: valid("view")?,
            })
        }
        6 => Some(Navigation::TaskGroup {
            group: triple(message, "task_group")?,
        }),
        7 => Some(Navigation::MeshGroup {
            group: triple(message, "mesh_group")?,
            task_group: triple(message, "task_group")?,
        }),
        _ => None,
    }
}

pub fn debug_invocation(message: &ShaderMessage) -> Option<DebugInvocation> {
    use crate::replay::{ComputeDebugSelection, PixelDebugSelection};
    match message.stage {
        // A vertex message's location is not necessarily its draw-row index.
        // Resolve it against the index buffer before creating a debug request.
        0 => None,
        4 => {
            let sample = number(message, "sample")?;
            let primitive = number(message, "primitive")?;
            Some(DebugInvocation::Pixel(PixelDebugSelection {
                x: number(message, "x")?,
                y: number(message, "y")?,
                sample: (sample != u32::MAX).then_some(sample),
                primitive: (primitive != u32::MAX).then_some(primitive),
                view: Some(number(message, "view")?),
            }))
        }
        5 => Some(DebugInvocation::Compute(ComputeDebugSelection {
            group: triple(message, "workgroup")?,
            thread: triple(message, "thread")?,
        })),
        7 => {
            // Vulkan's current feedback cannot identify the parent of a mesh
            // invocation launched by a task group; do not debug a guessed one.
            if triple(message, "task_group")? != [u32::MAX; 3] {
                return None;
            }
            let group = triple(message, "mesh_group")?;
            let thread = triple(message, "thread")?;
            if group.contains(&u32::MAX) || thread.contains(&u32::MAX) {
                return None;
            }
            Some(DebugInvocation::Mesh(ComputeDebugSelection {
                group,
                thread,
            }))
        }
        _ => None,
    }
}

pub fn debug_invocation_for_pipeline(
    message: &ShaderMessage,
    pipeline: &crate::replay::Pipeline,
) -> Option<DebugInvocation> {
    if !pipeline.available {
        return None;
    }
    if message.stage == 7 {
        let mut tasks = pipeline
            .shaders
            .iter()
            .filter(|shader| shader.stage == "task");
        let task = tasks.next()?;
        if task.resource.bound || tasks.next().is_some() {
            return None;
        }
    }
    debug_invocation(message)
}

pub fn stage_name(stage: u32, language: Language) -> &'static str {
    match stage {
        0 => language.text("Vertex", "顶点"),
        1 => language.text("Hull", "外壳"),
        2 => language.text("Domain", "域"),
        3 => language.text("Geometry", "几何"),
        4 => language.text(
            "Pixel / Fragment",
            "像素 / 片元",
        ),
        5 => language.text("Compute", "计算"),
        6 => "Task",
        7 => "Mesh",
        8 => "RayGen",
        9 => "Intersection",
        10 => "AnyHit",
        11 => "ClosestHit",
        12 => "Miss",
        13 => "Callable",
        _ => language.text("Unknown", "未知"),
    }
}

pub fn number(message: &ShaderMessage, key: &str) -> Option<u32> {
    message
        .location
        .as_ref()?
        .get(key)?
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
}
pub fn triple(message: &ShaderMessage, key: &str) -> Option<[u32; 3]> {
    let values = message.location.as_ref()?.get(key)?.as_array()?;
    if values.len() != 3 {
        return None;
    }
    Some([
        u32::try_from(values[0].as_u64()?).ok()?,
        u32::try_from(values[1].as_u64()?).ok()?,
        u32::try_from(values[2].as_u64()?).ok()?,
    ])
}

pub fn location(message: &ShaderMessage, language: Language) -> String {
    let v = |key| {
        number(message, key).map_or_else(
            || "?".into(),
            |n| {
                if n == u32::MAX {
                    "?".into()
                } else {
                    n.to_string()
                }
            },
        )
    };
    let triplet = |key| {
        triple(message, key).map_or_else(
            || "?".into(),
            |values| {
                values
                    .into_iter()
                    .filter(|&v| v != u32::MAX)
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        )
    };
    match message.stage {
        0 => format!(
            "{} {} · {} {} · {} {}",
            language.text("Vertex", "顶点"),
            v("vertex"),
            language.text("Instance", "实例"),
            v("instance"),
            language.text("View", "视图"),
            v("view")
        ),
        3 => format!(
            "{} {} · {} {}",
            language.text("Primitive", "图元"),
            v("primitive"),
            language.text("View", "视图"),
            v("view")
        ),
        4 => format!(
            "({}, {}) · {} {} · {} {} · {} {}",
            v("x"),
            v("y"),
            language.text("Primitive", "图元"),
            v("primitive"),
            language.text("Sample", "采样"),
            v("sample"),
            language.text("View", "视图"),
            v("view")
        ),
        5 => format!(
            "{} [{}] · {} [{}]",
            language.text("Workgroup", "工作组"),
            triplet("workgroup"),
            language.text("Thread", "线程"),
            triplet("thread")
        ),
        6 => format!(
            "Task [{}] · {} [{}]",
            triplet("task_group"),
            language.text("Thread", "线程"),
            triplet("thread")
        ),
        7 => {
            let task = triple(message, "task_group");
            let reliable = message
                .location
                .as_ref()
                .and_then(|v| v.get("task_group_reliable"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let parent = if task == Some([u32::MAX; 3]) {
                String::new()
            } else if reliable {
                format!("Task [{}] · ", triplet("task_group"))
            } else {
                format!(
                    "{} · ",
                    language.text(
                        "Task group unavailable",
                        "Task 组不可用",
                    )
                )
            };
            format!(
                "{parent}Mesh [{}] · {} [{}]",
                triplet("mesh_group"),
                language.text("Thread", "线程"),
                triplet("thread")
            )
        }
        _ => "—".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn page(offset: u64, limit: u32) -> Result<ShaderMessagePage, String> {
        const TOTAL: u64 = 260;
        if offset > TOTAL {
            return Err("bad offset".into());
        }
        let messages = (offset..(offset + u64::from(limit)).min(TOTAL))
            .map(|index| ShaderMessage {
                stage: if index % 2 == 0 { 5 } else { 4 },
                disassembly_line: -1,
                message: if index == 258 {
                    "MATCH, \"quoted\"\n下一行".into()
                } else if index == 128 {
                    "match first".into()
                } else {
                    format!("other {index}")
                },
                location: None,
            })
            .collect();
        Ok(ShaderMessagePage {
            event_id: 7,
            offset: offset.to_string(),
            total: TOTAL.to_string(),
            messages,
        })
    }

    #[test]
    fn filter_scans_every_page_and_export_quotes_all_rows() {
        let filter = Filter {
            stage_mask: 1 << 5,
            query: "match".into(),
            language: Language::English,
        };
        let page = filtered_page(7, 1, 1, &filter, self::page).unwrap();
        assert_eq!(page.total, "2");
        assert_eq!(page.messages[0].message, "MATCH, \"quoted\"\n下一行");
        let mut csv = Vec::new();
        assert_eq!(
            write_report(7, &filter, true, &mut csv, self::page).unwrap(),
            2
        );
        let csv = String::from_utf8(csv).unwrap();
        assert!(csv.contains("\"MATCH, \"\"quoted\"\"\n下一行\""));
        assert!(csv.contains("match first"));
        assert!(filtered_page(7, 3, 1, &filter, self::page).is_err());
    }

    #[test]
    fn scan_rejects_changed_pages_instead_of_exporting_partial_results() {
        let filter = Filter {
            stage_mask: u16::MAX,
            query: String::new(),
            language: Language::English,
        };
        let result = scan(
            7,
            &filter,
            |offset, limit| {
                let mut result = page(offset, limit)?;
                if offset > 0 {
                    result.total = "261".into();
                }
                Ok(result)
            },
            |_| Ok(()),
        );
        assert!(result.is_err());
    }

    #[test]
    fn export_never_overwrites_or_publishes_incomplete_output() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("messages.csv");
        let filter = Filter {
            stage_mask: u16::MAX,
            query: String::new(),
            language: Language::English,
        };
        let result = export_report(7, &filter, true, &destination, |offset, limit| {
            if offset > 0 {
                Err("read failed".into())
            } else {
                page(offset, limit)
            }
        });
        assert!(result.is_err());
        assert!(!destination.exists());
        std::fs::write(&destination, b"existing").unwrap();
        assert!(export_report(7, &filter, true, &destination, page).is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"existing");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[test]
    fn debug_invocations_keep_coordinates_and_upstream_sentinels() {
        let mut message = ShaderMessage {
            stage: 5,
            disassembly_line: 3,
            message: String::new(),
            location: Some(serde_json::json!({"workgroup":[1,2,3],"thread":[4,5,6]})),
        };
        assert!(
            matches!(debug_invocation(&message), Some(DebugInvocation::Compute(v)) if v.group == [1,2,3] && v.thread == [4,5,6])
        );
        message.stage = 4;
        message.location = Some(
            serde_json::json!({"x":11,"y":12,"sample":u32::MAX,"primitive":u32::MAX,"view":2}),
        );
        assert!(
            matches!(debug_invocation(&message), Some(DebugInvocation::Pixel(v)) if (v.x,v.y,v.sample,v.primitive,v.view) == (11,12,None,None,Some(2)))
        );
        message.stage = 3;
        assert!(debug_invocation(&message).is_none());
    }

    #[test]
    fn navigation_uses_upstream_topology_offsets_and_keeps_pixel_selectors() {
        for (topology, primitive, row) in [
            (1, 3, 3),
            (2, 3, 6),
            (3, 3, 3),
            (5, 3, 9),
            (8, 3, 12),
            (10, 3, 18),
            (11, 3, 6),
            (12, 3, 3),
            (43, 3, 96),
        ] {
            assert_eq!(primitive_vertex_offset(topology, primitive), Some(row));
        }
        assert_eq!(primitive_vertex_offset(0, 3), None);
        assert_eq!(primitive_vertex_offset(10, u32::MAX), None);
        let vertex = ShaderMessage {
            stage: 0,
            disassembly_line: -1,
            message: String::new(),
            location: Some(serde_json::json!({"vertex": 4, "instance": 1, "view": 2})),
        };
        assert_eq!(
            navigation(&vertex, 5),
            Some(Navigation::Vertex {
                reported: 4,
                instance: 1,
                view: 2
            })
        );
        assert!(debug_invocation(&vertex).is_none());
        let mut message = ShaderMessage {
            stage: 3,
            disassembly_line: -1,
            message: String::new(),
            location: Some(serde_json::json!({"primitive": 3, "view": 2})),
        };
        assert_eq!(
            navigation(&message, 5),
            Some(Navigation::Mesh {
                stage: 2,
                row: 9,
                instance: 0,
                view: 2
            })
        );
        message.stage = 4;
        message.location =
            Some(serde_json::json!({"x":11,"y":12,"sample":u32::MAX,"primitive":7,"view":2}));
        assert_eq!(
            navigation(&message, 5),
            Some(Navigation::Pixel {
                x: 11,
                y: 12,
                sample: None,
                primitive: Some(7),
                view: 2
            })
        );
        message.stage = 7;
        assert_eq!(navigation(&message, 5), None);
    }

    #[test]
    fn vulkan_workgroup_resolution_checks_dimensions_and_untrusted_task_parent() {
        let task = crate::replay::MeshInfo {
            stage: 3,
            dispatch_size: Some([3, 2, 2]),
            num_indices: 12,
            tasks: vec![[1, 1, 1]; 12],
            ..Default::default()
        };
        assert_eq!(task_group_row(3, &task, [2, 1, 1]), Ok(11));
        assert!(task_group_row(3, &task, [3, 0, 0]).is_err());
        assert!(task_group_row(1, &task, [0, 0, 0]).is_err());
        let bad = crate::replay::MeshInfo {
            num_indices: 11,
            ..task.clone()
        };
        assert!(task_group_row(3, &bad, [0, 0, 0]).is_err());
        assert!(dispatch_index([u32::MAX - 1, u32::MAX - 1, 1], [u32::MAX, u32::MAX, 2]).is_err());
        let action: crate::replay::Action = serde_json::from_value(serde_json::json!({"event_id":7,"action_id":1,"depth":0,"parent":-1,"flags":2,"indices":0,"instances":0,"name":"mesh","dispatch_groups":[3,2,2]})).unwrap();
        let info = crate::replay::MeshInfo {
            event_id: 7,
            stage: 4,
            meshlet_offset: 2,
            meshlets: (0..10)
                .map(|_| crate::replay::MeshletInfo {
                    indices: 3,
                    vertices: 3,
                })
                .collect(),
            ..Default::default()
        };
        let pipeline = crate::replay::Pipeline {
            event_id: 7,
            available: true,
            shaders: vec![crate::replay::PipelineShader {
                stage: "task".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(
            direct_meshlet(3, &action, &pipeline, &info, [2, 1, 1], [u32::MAX; 3]),
            Ok(11)
        );
        assert!(direct_meshlet(3, &action, &pipeline, &info, [1, 0, 0], [u32::MAX; 3]).is_err());
        assert!(direct_meshlet(3, &action, &pipeline, &info, [2, 1, 1], [0, 0, 0]).is_err());
        let mut bound_pipeline = pipeline.clone();
        bound_pipeline.shaders[0].resource.bound = true;
        assert!(
            direct_meshlet(3, &action, &bound_pipeline, &info, [2, 1, 1], [u32::MAX; 3]).is_err()
        );
        let mut message = ShaderMessage {
            stage: 6,
            disassembly_line: -1,
            message: String::new(),
            location: Some(
                serde_json::json!({"task_group":[2,1,1],"mesh_group":[u32::MAX,u32::MAX,u32::MAX],"thread":[0,0,0]}),
            ),
        };
        assert_eq!(
            navigation(&message, 0),
            Some(Navigation::TaskGroup { group: [2, 1, 1] })
        );
        assert!(!location(&message, Language::English).contains("Mesh"));
        message.stage = 7;
        message.location = Some(
            serde_json::json!({"task_group":[u32::MAX,u32::MAX,u32::MAX],"mesh_group":[2,1,1],"thread":[0,0,0]}),
        );
        assert_eq!(
            navigation(&message, 0),
            Some(Navigation::MeshGroup {
                group: [2, 1, 1],
                task_group: [u32::MAX; 3]
            })
        );
        assert!(matches!(
            debug_invocation_for_pipeline(&message, &pipeline),
            Some(DebugInvocation::Mesh(_))
        ));
        assert!(debug_invocation_for_pipeline(&message, &bound_pipeline).is_none());
        message.location =
            Some(serde_json::json!({"task_group":[0,0,0],"mesh_group":[2,1,1],"thread":[0,0,0]}));
        assert!(debug_invocation(&message).is_none());
        assert!(location(&message, Language::English).contains("Task group unavailable"));
        assert!(!location(&message, Language::English).contains("Task [0, 0, 0]"));
        message.location.as_mut().unwrap()["task_group_reliable"] = serde_json::json!(true);
        assert!(location(&message, Language::English).contains("Task [0, 0, 0]"));
        message.location = Some(
            serde_json::json!({"task_group":[u32::MAX,u32::MAX,u32::MAX],"mesh_group":[2,1,1],"thread":[u32::MAX,0,0]}),
        );
        assert!(debug_invocation(&message).is_none());
    }

    #[test]
    fn pixel_handoff_prefers_color_zero_then_depth_without_guessing() {
        let snapshot: crate::replay::Snapshot = serde_json::from_value(serde_json::json!({
            "protocol":1,"api":0,"frame":1,"degraded":false,"actions":[],"resource_count":1,"texture_count":1,"buffer_count":0,
            "textures":[{"name":"color","resource_index":0,"width":16,"height":16,"depth":1,"dimension":2,"mips":2,"array_size":2,"samples":4,"bytes":"1024","format_type":0,"component_type":0,"component_count":4,"component_byte_width":1}]
        })).unwrap();
        let attachment: crate::replay::PipelineAttachment = serde_json::from_value(serde_json::json!({
            "resource":{"bound":true,"resource_index":0,"name":"color"}, "view":{"bound":false,"resource_index":null,"name":""},
            "type":0,"flags":0,"format":"x","format_component_type":2,"texture_type":0,"first_mip":1,"mip_count":1,"first_slice":0,"slice_count":2,"min_lod_clamp":0,"swizzle":[0,1,2,3]
        })).unwrap();
        let mut pipeline = crate::replay::Pipeline {
            color_attachments: Some(vec![attachment.clone()]),
            depth_attachment: Some(attachment.clone()),
            ..Default::default()
        };
        assert_eq!(
            pixel_output(&snapshot, &pipeline, 8, Some(9)),
            Some(PixelOutput {
                texture: 0,
                mip: 1,
                slice: 1,
                type_cast: 2,
                sample: Some(3)
            })
        );
        assert_eq!(
            pixel_output(&snapshot, &pipeline, 0, None).unwrap().sample,
            Some(3)
        );
        pipeline.color_attachments = Some(vec![]);
        assert!(pixel_output(&snapshot, &pipeline, 0, None).is_some());
        let mut unbound = attachment.clone();
        unbound.resource.bound = false;
        pipeline.color_attachments = Some(vec![unbound, attachment]);
        assert_eq!(pixel_output(&snapshot, &pipeline, 0, None), None);
        pipeline.color_attachments = None;
        assert_eq!(pixel_output(&snapshot, &pipeline, 0, None), None);
    }
}
