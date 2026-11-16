use gpui_kit as gpui;
use gpui_kit::TestSupportExt;
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    theme::{Theme, ThemeMode},
};
use gpui_kit::{
    App, Bounds, Context, Entity, SharedString, UniformListScrollHandle, Window, WindowBounds,
    WindowOptions, div, img, prelude::*, px, rgb, size, uniform_list,
};
use renderdocx::{
    backend::{self, Settings},
    i18n::Language,
    live::{
        CaptureOptions, EnvOperation, EnvSeparator, EnvironmentModification, LaunchConfiguration,
        LiveSession, capture_directory, capture_notice, enumerate, parse_target_pid,
    },
    replay::{
        DebugState, DebugTrace, HistogramRequest, MeshInfo, MeshSelection,
        PIXEL_HISTORY_PAGE_LIMIT, Pipeline, Pixel, PixelDebugSelection, PixelHistoryPage,
        RESOURCE_USAGE_PAGE_LIMIT, ResourceUsagePage, SHADER_STAGES, STRUCTURE_PAGE_LIMIT, Session,
        ShaderReport, Snapshot, StructurePage, StructuredNode, TextureSelection, TextureStatistics,
        resource_type_name, resource_usage_name,
    },
    vertex_input::{INPUT_PAGE_LIMIT, InputPage},
};
use std::path::PathBuf;

mod event_tree;
mod shader_debug;
use event_tree::{EventRow, EventTree};

struct TexturePreview {
    path: PathBuf,
    _directory: tempfile::TempDir,
}

#[derive(Clone, Copy)]
enum ShaderDebugContext {
    Vertex {
        vertex: u32,
        instance: u32,
        index: u32,
    },
    Pixel(PixelDebugSelection),
}

struct ShaderDebugView {
    context: ShaderDebugContext,
    trace: DebugTrace,
    states: Vec<DebugState>,
    position: usize,
    disassembly: String,
}

fn collect_shader_debug(
    session: &Session,
    event: u32,
    context: ShaderDebugContext,
) -> Result<ShaderDebugView, String> {
    let stage = match context {
        ShaderDebugContext::Vertex { .. } => 0,
        ShaderDebugContext::Pixel(_) => 4,
    };
    let shader = session.shader(stage, None)?;
    if shader.event_id != event {
        return Err("Shader response belongs to another event".into());
    }
    let trace = match context {
        ShaderDebugContext::Vertex {
            vertex,
            instance,
            index,
        } => session.debug_vertex(vertex, instance, index, 0)?,
        ShaderDebugContext::Pixel(selection) => session.debug_pixel(selection)?,
    };
    let collected = (|| {
        if trace.event_id != event {
            return Err("Debug trace belongs to another event".to_owned());
        }
        let mut states = Vec::new();
        loop {
            let batch = session.debug_next()?;
            if batch.event_id != event {
                return Err("Debug states belong to another event".to_owned());
            }
            states.extend(batch.states);
            if batch.finished {
                break;
            }
        }
        Ok(ShaderDebugView {
            context,
            trace,
            states,
            position: 0,
            disassembly: shader.disassembly,
        })
    })();
    let stopped = session.debug_stop();
    match (collected, stopped) {
        (Ok(view), Ok(())) => Ok(view),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

#[derive(Default)]
struct StructureView {
    page: Option<StructurePage>,
    path: Vec<u64>,
    offset: u64,
    bytes: Option<Vec<u8>>,
    bytes_offset: u64,
}

struct MeshView {
    selection: MeshSelection,
    info: Option<MeshInfo>,
    bytes: Option<Vec<u8>>,
    offset: u64,
}

impl Default for MeshView {
    fn default() -> Self {
        Self {
            selection: MeshSelection {
                instance: 0,
                view: 0,
                stage: 1,
            },
            info: None,
            bytes: None,
            offset: 0,
        }
    }
}

fn mesh_position_format(info: &MeshInfo) -> bool {
    cfg!(target_endian = "little")
        && info.unproject
        && info.format_type == 0
        && info.component_type == 1
        && info.component_byte_width == 4
        && (1..=4).contains(&info.component_count)
        && info.vertex_stride > 0
        && u64::from(info.vertex_stride) <= BUFFER_VIEW_PAGE
        && info.component_count * 4 <= info.vertex_stride
}

fn mesh_page_size(info: &MeshInfo) -> u64 {
    if mesh_position_format(info) {
        BUFFER_VIEW_PAGE / u64::from(info.vertex_stride) * u64::from(info.vertex_stride)
    } else {
        BUFFER_VIEW_PAGE
    }
}

fn mesh_positions(info: &MeshInfo, bytes: &[u8], offset: u64) -> Option<Vec<(u64, Vec<f32>)>> {
    if !mesh_position_format(info) {
        return None;
    }
    let stride = u64::from(info.vertex_stride);
    let mut relative = (stride - offset % stride) % stride;
    let mut positions = Vec::new();
    while relative.checked_add(u64::from(info.component_count) * 4)? <= bytes.len() as u64 {
        let start = usize::try_from(relative).ok()?;
        let components = (0..info.component_count as usize)
            .map(|index| {
                let i = start + index * 4;
                f32::from_le_bytes(bytes[i..i + 4].try_into().unwrap())
            })
            .collect();
        positions.push((offset.checked_add(relative)? / stride, components));
        relative = relative.checked_add(stride)?;
    }
    Some(positions)
}

#[cfg(test)]
mod mesh_tests {
    use super::*;

    #[test]
    fn position_pages_preserve_vertices_when_stride_does_not_divide_page() {
        let info = MeshInfo {
            vertex_stride: 20,
            component_type: 1,
            component_count: 4,
            component_byte_width: 4,
            unproject: true,
            ..MeshInfo::default()
        };
        assert_eq!(mesh_page_size(&info), 240);
        let mut bytes = Vec::new();
        for n in 0..24 {
            for value in [n as f32, 3.0, 0.0, 1.0] {
                bytes.extend(value.to_le_bytes());
            }
            bytes.extend([0; 4]);
        }
        let first = mesh_positions(&info, &bytes[..240], 0).unwrap();
        let second = mesh_positions(&info, &bytes[240..], 240).unwrap();
        assert_eq!(first.len(), 12);
        assert_eq!(second.len(), 12);
        assert_eq!(first[11].0, 11);
        assert_eq!(second[0], (12, vec![12.0, 3.0, 0.0, 1.0]));
        let unsupported = MeshInfo {
            vertex_stride: 512,
            ..info
        };
        assert!(mesh_positions(&unsupported, &bytes, 0).is_none());
        assert_eq!(mesh_page_size(&unsupported), BUFFER_VIEW_PAGE);
    }
}

fn structured_value(node: &StructuredNode) -> String {
    match (&node.custom_string, &node.value) {
        (Some(custom), Some(value)) if custom != value => format!("{custom} ({value})"),
        (Some(custom), _) => custom.clone(),
        (_, Some(value)) => value.clone(),
        _ if node.basic_type == 3
            || node.flags & 8 != 0
            || node.resource_non_null == Some(false) =>
        {
            "NULL".into()
        }
        _ if node.resource_non_null == Some(true) => node
            .resource_index
            .map(|index| format!("#{index}"))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CaptureTab {
    Events,
    ApiCalls,
    Parameters,
    Textures,
    Buffers,
    Shaders,
    Mesh,
    VertexInput,
    Resources,
}
#[derive(Clone, Copy)]
enum TextureField {
    Mip,
    Slice,
    Sample,
    Channel,
}
const BUFFER_VIEW_PAGE: u64 = 256;

struct Workspace {
    settings: Settings,
    status: Option<String>,
    capture: Option<PathBuf>,
    session: Option<Session>,
    snapshot: Option<Snapshot>,
    event_tree: EventTree,
    event_scroll: UniformListScrollHandle,
    event_search: Entity<InputState>,
    _event_search_subscription: gpui::Subscription,
    resource_search: Entity<InputState>,
    _resource_search_subscription: gpui::Subscription,
    resource_query: String,
    resource_visible: Vec<usize>,
    selected: Option<usize>,
    pipeline: Option<Pipeline>,
    tab: CaptureTab,
    selected_texture: Option<usize>,
    texture_selection: TextureSelection,
    pixel_x: Entity<InputState>,
    pixel_y: Entity<InputState>,
    pixel: Option<Pixel>,
    pixel_history: Option<PixelHistoryPage>,
    history_selected: Option<usize>,
    stats_min: Entity<InputState>,
    stats_max: Entity<InputState>,
    statistics: Option<TextureStatistics>,
    selected_buffer: Option<usize>,
    buffer_offset: u64,
    buffer_data: Option<Vec<u8>>,
    resource_pending: bool,
    selected_shader: Option<usize>,
    shader_data: Option<ShaderReport>,
    structure: StructureView,
    mesh: MeshView,
    input_page: Option<InputPage>,
    debug_vertex_selection: Option<(u32, u32, u32)>,
    debug_view: Option<ShaderDebugView>,
    selected_resource: Option<usize>,
    usage_page: Option<ResourceUsagePage>,
    open_revision: u64,
    selection_revision: u64,
    replay_pending: bool,
    export_pending: bool,
    preview: Option<TexturePreview>,
    live: Option<LiveSession>,
    live_generation: u64,
    live_starting: bool,
    live_status: Option<String>,
    live_captures: Vec<(u32, PathBuf, bool)>,
    live_targets: Vec<u32>,
    show_launch_config: bool,
    launch_arguments: Entity<InputState>,
    launch_delay: Entity<InputState>,
    launch_memory: Entity<InputState>,
    launch_working_directory: Option<PathBuf>,
    capture_options: CaptureOptions,
    launch_env_name: Entity<InputState>,
    launch_env_value: Entity<InputState>,
    launch_environment: Vec<EnvironmentModification>,
    launch_env_operation: EnvOperation,
    launch_env_separator: EnvSeparator,
}

impl Workspace {
    fn watch_resource_search(
        input: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) -> gpui::Subscription {
        cx.subscribe(input, |this, input, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.resource_query = input.read(cx).value().to_lowercase();
                this.refresh_resources();
                cx.notify();
            }
        })
    }

    fn refresh_resources(&mut self) {
        let query = &self.resource_query;
        let exact_index = query
            .strip_prefix('#')
            .and_then(|value| value.parse::<usize>().ok());
        self.resource_visible = self
            .snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .resources
                    .iter()
                    .enumerate()
                    .filter_map(|(index, resource)| {
                        let matches = if let Some(exact) = exact_index {
                            index == exact
                        } else {
                            resource.name.to_lowercase().contains(query)
                                || resource_type_name(resource.resource_type)
                                    .to_lowercase()
                                    .contains(query)
                        };
                        matches.then_some(index)
                    })
                    .collect()
            })
            .unwrap_or_default();
    }

    fn watch_event_search(
        input: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) -> gpui::Subscription {
        cx.subscribe(input, |this, input, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.event_tree.set_filter(input.read(cx).value().as_ref());
                cx.notify();
            }
        })
    }

    fn discover_live(&mut self, cx: &mut Context<Self>) {
        if self.live.is_some() || self.live_starting {
            self.status = Some(
                self.t(
                    "Close the current target first.",
                    "请先关闭当前目标连接。",
                    "現在のターゲット接続を先に閉じてください。",
                    "현재 대상 연결을 먼저 닫아 주세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        }
        self.live_starting = true;
        self.live_generation += 1;
        let generation = self.live_generation;
        let settings = self.settings.clone();
        self.status = Some(
            self.t(
                "Discovering local targets…",
                "正在查找本机目标…",
                "ローカルターゲットを検索中…",
                "로컬 대상을 찾는 중…",
            )
            .to_owned(),
        );
        cx.notify();
        let task = cx
            .background_executor()
            .spawn(async move { enumerate(&settings) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.live_generation != generation {
                    return;
                }
                this.live_starting = false;
                match result {
                    Ok(targets) => {
                        this.status = Some(format!(
                            "{}: {}",
                            this.t(
                                "Local targets",
                                "本机目标",
                                "ローカルターゲット",
                                "로컬 대상"
                            ),
                            targets.len()
                        ));
                        this.live_targets = targets;
                    }
                    Err(error) => {
                        this.status = Some(format!(
                            "{}: {error}",
                            this.t(
                                "Discovery failed",
                                "目标查找失败",
                                "検索に失敗しました",
                                "대상 검색 실패"
                            )
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn connect_live(&mut self, ident: u32, cx: &mut Context<Self>) {
        self.connect_target(ident, false, cx);
    }

    fn connect_target(&mut self, identifier: u32, by_pid: bool, cx: &mut Context<Self>) {
        if self.live.is_some() || self.live_starting {
            return;
        }
        self.live_starting = true;
        self.live_generation += 1;
        let generation = self.live_generation;
        let settings = self.settings.clone();
        self.status = Some(
            self.t(
                "Connecting to target…",
                "正在连接目标…",
                "ターゲットに接続中…",
                "대상에 연결 중…",
            )
            .to_owned(),
        );
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            if by_pid {
                LiveSession::connect_pid(&settings, identifier)
            } else {
                LiveSession::connect(&settings, identifier)
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.live_generation != generation {
                    return;
                }
                this.live_starting = false;
                match result {
                    Ok(session) => {
                        this.live_status = Some(format!(
                            "{}: {}",
                            this.t("Connected", "已连接", "接続済み", "연결됨"),
                            session
                                .connected
                                .get("ident")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or_default()
                        ));
                        this.live_targets.clear();
                        this.live = Some(session.clone());
                        this.status = None;
                        this.poll_live(session, generation, cx);
                    }
                    Err(error) => {
                        this.status = Some(format!(
                            "{}: {error}",
                            this.t(
                                "Connection failed",
                                "连接失败",
                                "接続に失敗しました",
                                "연결 실패"
                            )
                        ))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn launch_live(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.live.is_some() || self.live_starting {
            self.status = Some(
                self.t(
                    "Close the current target first.",
                    "请先关闭当前目标连接。",
                    "現在のターゲット接続を先に閉じてください。",
                    "현재 대상 연결을 먼저 닫아 주세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        }
        let delay = self.launch_delay.read(cx).value().parse::<u32>();
        let memory = self.launch_memory.read(cx).value().parse::<u32>();
        let (Ok(delay), Ok(memory)) = (delay, memory) else {
            self.status = Some(
                self.t(
                    "Debugger delay and memory limit must be unsigned integers.",
                    "调试器等待时间和内存限制必须是非负整数。",
                    "デバッガー待機時間とメモリ制限は非負整数で入力してください。",
                    "디버거 대기 시간과 메모리 제한은 음이 아닌 정수여야 합니다.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        };
        let mut options = self.capture_options.clone();
        options.delay_for_debugger = delay;
        options.soft_memory_limit = memory;
        let config = LaunchConfiguration {
            arguments: self.launch_arguments.read(cx).value().to_string(),
            working_directory: self.launch_working_directory.clone(),
            options,
            environment: self.launch_environment.clone(),
        };
        if config.validate().is_err() {
            self.status = Some(
                self.t(
                    "Invalid working directory or launch arguments.",
                    "工作目录或启动参数无效。",
                    "作業ディレクトリまたは起動引数が無効です。",
                    "작업 디렉터리 또는 실행 인수가 올바르지 않습니다.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        }
        self.live_starting = true;
        self.live_generation += 1;
        let generation = self.live_generation;
        let settings = self.settings.clone();
        self.status = Some(
            self.t(
                "Starting capture target…",
                "正在启动捕获目标…",
                "キャプチャ対象を起動中…",
                "캡처 대상을 시작하는 중…",
            )
            .to_owned(),
        );
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            let directory = capture_directory()?;
            LiveSession::launch(&settings, &path, &directory, &config)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.live_generation != generation {
                    return;
                }
                this.live_starting = false;
                match result {
                    Ok(session) => {
                        this.live_status = Some(format!(
                            "{} · {}",
                            this.t("Connected", "已连接", "接続済み", "연결됨"),
                            session
                                .capture_template
                                .as_ref()
                                .map(|path| path.display().to_string())
                                .unwrap_or_default()
                        ));
                        this.live = Some(session.clone());
                        this.status = None;
                        this.poll_live(session, generation, cx);
                    }
                    Err(error) => {
                        this.status = Some(format!(
                            "{}: {error}",
                            this.t(
                                "Target launch failed",
                                "目标启动失败",
                                "ターゲットの起動に失敗しました",
                                "대상 실행 실패"
                            )
                        ))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn poll_live(&mut self, session: LiveSession, generation: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(150))
                    .await;
                let events = session.drain().unwrap_or_default();
                let alive = session.is_alive();
                let updated = this.update(cx, |this, cx| {
                    if this.live_generation != generation {
                        return false;
                    }
                    let changed = !events.is_empty() || !alive;
                    let mut error_seen = false;
                    for event in events {
                        match event.get("type").and_then(serde_json::Value::as_str) {
                            Some("capture") => {
                                if let Some(capture) = capture_notice(&event) {
                                    if !this.live_captures.iter().any(|existing| {
                                        existing.1 == capture.1 && existing.2 == capture.2
                                    }) {
                                        this.live_captures.push(capture);
                                    }
                                }
                            }
                            Some("api") => {
                                let name = event
                                    .get("name")
                                    .and_then(serde_json::Value::as_str)
                                    .unwrap_or("");
                                let supported = event
                                    .get("supported")
                                    .and_then(serde_json::Value::as_bool)
                                    .unwrap_or(false);
                                this.live_status = Some(format!(
                                    "{}: {name} · {}",
                                    this.t("API", "图形接口", "API", "API"),
                                    if supported {
                                        this.t("supported", "支持捕获", "対応", "지원")
                                    } else {
                                        this.t("unsupported", "不支持捕获", "非対応", "지원 안 됨")
                                    }
                                ));
                            }
                            Some("progress") => {
                                if let Some(value) =
                                    event.get("value").and_then(serde_json::Value::as_f64)
                                {
                                    if (0.0..=1.0).contains(&value) {
                                        this.live_status = Some(format!(
                                            "{}: {:.0}%",
                                            this.t(
                                                "Capturing",
                                                "正在捕获",
                                                "キャプチャ中",
                                                "캡처 중"
                                            ),
                                            value * 100.
                                        ));
                                    }
                                }
                            }
                            Some("error") => {
                                error_seen = true;
                                this.live_status = Some(format!(
                                    "{}: {}",
                                    this.t(
                                        "Target error",
                                        "目标错误",
                                        "ターゲットエラー",
                                        "대상 오류"
                                    ),
                                    event
                                        .get("message")
                                        .and_then(serde_json::Value::as_str)
                                        .unwrap_or("")
                                ))
                            }
                            _ => {}
                        }
                    }
                    if !alive && !error_seen {
                        this.live_status = Some(
                            this.t(
                                "Target disconnected",
                                "目标已断开",
                                "ターゲット切断",
                                "대상 연결 끊김",
                            )
                            .to_owned(),
                        );
                    }
                    if changed {
                        cx.notify();
                    }
                    alive
                });
                if !updated.unwrap_or(false) {
                    break;
                }
            }
        })
        .detach();
    }

    fn t(
        &self,
        en: &'static str,
        zh: &'static str,
        ja: &'static str,
        ko: &'static str,
    ) -> &'static str {
        self.settings.language.text(en, zh, ja, ko)
    }

    fn result(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        self.status = Some(match result {
            Ok(()) => self.t("Process started. Capture and replay results are reported by RenderDoc.", "进程已启动，捕获与回放结果由 RenderDoc 报告。", "プロセスを起動しました。キャプチャとリプレイの結果は RenderDoc に表示されます。", "프로세스를 시작했습니다. 캡처 및 재생 결과는 RenderDoc에서 확인하세요.").to_owned(),
            Err(e) => format!("{}: {e}", self.t("Could not start", "启动失败", "起動できません", "시작할 수 없음")),
        });
        cx.notify();
    }

    fn save(&mut self) {
        if let Err(error) = self.settings.save() {
            self.status = Some(format!(
                "{}: {error}",
                self.t(
                    "Could not save settings",
                    "无法保存设置",
                    "設定を保存できません",
                    "설정을 저장할 수 없습니다"
                )
            ));
        }
    }

    fn open(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.replay_pending || self.export_pending || self.resource_pending {
            self.status = Some(
                self.t(
                    "Wait for the current replay operation to finish.",
                    "请等待当前回放操作完成。",
                    "現在のリプレイ処理が完了するまでお待ちください。",
                    "현재 리플레이 작업이 끝날 때까지 기다려 주세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        }
        if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("cap"))
        {
            let result = backend::open_capture(&self.settings, &path);
            self.result(result, cx);
            return;
        }
        self.open_revision += 1;
        let revision = self.open_revision;
        let settings = self.settings.clone();
        let capture = path.clone();
        self.status = Some(
            self.t(
                "Opening capture for native replay…",
                "正在打开捕获并初始化原生回放…",
                "キャプチャを開いてリプレイを初期化中…",
                "캡처를 열고 리플레이를 초기화하는 중…",
            )
            .to_owned(),
        );
        cx.notify();
        let task = cx
            .background_executor()
            .spawn(async move { Session::open(&settings, &path) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.open_revision != revision {
                    return;
                }
                match result {
                    Ok((session, snapshot)) => {
                        // Invalidate work started against the old session while this open was pending.
                        this.open_revision += 1;
                        this.capture = Some(capture);
                        this.event_tree.reset(&snapshot.actions);
                        this.snapshot = Some(snapshot);
                        this.session = Some(session);
                        this.selected = None;
                        this.pipeline = None;
                        this.selected_texture = None;
                        this.texture_selection = TextureSelection::default();
                        this.tab = CaptureTab::Events;
                        this.selected_buffer = None;
                        this.buffer_data = None;
                        this.resource_pending = false;
                        this.selected_shader = None;
                        this.shader_data = None;
                        this.structure = StructureView::default();
                        this.mesh = MeshView::default();
                        this.input_page = None;
                        this.debug_vertex_selection = None;
                        this.debug_view = None;
                        this.selected_resource = None;
                        this.usage_page = None;
                        this.replay_pending = false;
                        this.export_pending = false;
                        this.preview = None;
                        this.pixel = None;
                        this.pixel_history = None;
                        this.history_selected = None;
                        this.statistics = None;
                        this.status = None;
                    }
                    Err(error) => {
                        this.status = Some(format!(
                            "{}: {error}",
                            this.t(
                                "Could not open capture",
                                "无法打开捕获文件",
                                "キャプチャを開けません",
                                "캡처를 열 수 없습니다"
                            )
                        ))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn select_event(&mut self, index: usize, api_index: Option<usize>, cx: &mut Context<Self>) {
        if self.export_pending || self.resource_pending {
            self.status = Some(
                self.t(
                    "Wait for the resource operation to finish.",
                    "请等待资源操作完成。",
                    "リソース処理が完了するまでお待ちください。",
                    "리소스 작업이 완료될 때까지 기다려 주세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(action) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.actions.get(index))
        else {
            return;
        };
        let event = match api_index {
            Some(api_index) => match action.api_events.get(api_index) {
                Some(event) => event.event_id,
                None => return,
            },
            None => action.event_id,
        };
        let revision = session.next_revision();
        self.selection_revision = revision;
        self.replay_pending = true;
        self.preview = None;
        self.pixel = None;
        self.pixel_history = None;
        self.history_selected = None;
        self.statistics = None;
        self.buffer_data = None;
        self.shader_data = None;
        self.structure = StructureView::default();
        self.mesh = MeshView::default();
        self.input_page = None;
        self.debug_vertex_selection = None;
        self.debug_view = None;
        let open_revision = self.open_revision;
        self.status = Some(
            self.t(
                "Replaying event…",
                "正在回放事件…",
                "イベントをリプレイ中…",
                "이벤트를 리플레이하는 중…",
            )
            .to_owned(),
        );
        cx.notify();
        let task = cx
            .background_executor()
            .spawn(async move { session.select_pipeline(event, revision) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.open_revision != open_revision || this.selection_revision != revision {
                    return;
                }
                this.replay_pending = false;
                match result {
                    Ok(Some(pipeline)) => {
                        this.selected = Some(index);
                        this.pipeline = Some(pipeline);
                        this.status = None;
                    }
                    Ok(None) => return,
                    Err(error) => {
                        this.status = Some(format!(
                            "{}: {error}",
                            this.t(
                                "Replay failed",
                                "回放失败",
                                "リプレイに失敗しました",
                                "리플레이 실패"
                            )
                        ))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn read_buffer(&mut self, index: usize, offset: u64, cx: &mut Context<Self>) {
        if self.replay_pending || self.export_pending || self.resource_pending {
            self.status = Some(
                self.t(
                    "Wait for the current operation.",
                    "请等待当前操作完成。",
                    "現在の処理が完了するまでお待ちください。",
                    "현재 작업이 끝날 때까지 기다려 주세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        }
        self.selected_buffer = Some(index);
        self.buffer_offset = offset;
        self.buffer_data = None;
        if self.selected.is_none() {
            self.status = Some(
                self.t(
                    "Select an event before reading a buffer.",
                    "请先选择事件再读取缓冲区。",
                    "バッファを読む前にイベントを選択してください。",
                    "버퍼를 읽기 전에 이벤트를 선택하세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(size) = self
            .snapshot
            .as_ref()
            .and_then(|s| s.buffers.get(index))
            .and_then(|b| b.length.parse::<u64>().ok())
        else {
            return;
        };
        if offset > size {
            return;
        }
        let Ok(buffer_index) = u32::try_from(index) else {
            return;
        };
        let length = (size - offset).min(BUFFER_VIEW_PAGE) as u32;
        self.resource_pending = true;
        self.status = Some(
            self.t(
                "Reading buffer…",
                "正在读取缓冲区…",
                "バッファを読み込み中…",
                "버퍼를 읽는 중…",
            )
            .to_owned(),
        );
        let revision = self.open_revision;
        cx.notify();
        let task = cx
            .background_executor()
            .spawn(async move { session.read_buffer(buffer_index, offset, length) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.open_revision != revision {
                    return;
                }
                this.resource_pending = false;
                match result {
                    Ok(bytes) => {
                        this.buffer_data = Some(bytes);
                        this.status = None;
                    }
                    Err(error) => {
                        this.status = Some(format!(
                            "{}: {error}",
                            this.t(
                                "Buffer read failed",
                                "缓冲区读取失败",
                                "バッファを読み込めません",
                                "버퍼 읽기 실패"
                            )
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn read_shader(&mut self, stage: usize, target: Option<u32>, cx: &mut Context<Self>) {
        if self.replay_pending || self.export_pending || self.resource_pending {
            self.status = Some(
                self.t(
                    "Wait for the current operation.",
                    "请等待当前操作完成。",
                    "現在の処理が完了するまでお待ちください。",
                    "현재 작업이 끝날 때까지 기다려 주세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        }
        let Some(event) = self.pipeline.as_ref().map(|pipeline| pipeline.event_id) else {
            self.status = Some(
                self.t(
                    "Select an event before inspecting shaders.",
                    "请先选择事件再查看着色器。",
                    "シェーダーを見る前にイベントを選択してください。",
                    "셰이더를 확인하기 전에 이벤트를 선택하세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        };
        let Some(session) = self.session.clone() else {
            return;
        };
        if stage >= SHADER_STAGES.len() {
            return;
        }
        self.selected_shader = Some(stage);
        self.shader_data = None;
        self.resource_pending = true;
        self.status = Some(
            self.t(
                "Loading shader…",
                "正在加载着色器…",
                "シェーダーを読み込み中…",
                "셰이더를 불러오는 중…",
            )
            .to_owned(),
        );
        let revision = self.open_revision;
        cx.notify();
        let task = cx
            .background_executor()
            .spawn(async move { session.shader(stage as u32, target) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.open_revision != revision {
                    return;
                }
                this.resource_pending = false;
                match result {
                    Ok(report) if report.event_id == event => {
                        this.shader_data = Some(report);
                        this.status = None;
                    }
                    Ok(_) => {
                        this.status = Some(
                            this.t(
                                "Shader response belongs to another event.",
                                "着色器结果属于其他事件。",
                                "シェーダーの結果は別のイベントのものです。",
                                "셰이더 결과가 다른 이벤트에 속합니다.",
                            )
                            .to_owned(),
                        );
                    }
                    Err(error) => {
                        this.status = Some(format!(
                            "{}: {error}",
                            this.t(
                                "Shader inspection failed",
                                "着色器查看失败",
                                "シェーダーを表示できません",
                                "셰이더 확인 실패"
                            )
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn read_resource_usage(&mut self, index: usize, offset: u64, cx: &mut Context<Self>) {
        if self.replay_pending || self.export_pending || self.resource_pending {
            self.status = Some(
                self.t(
                    "Wait for the current operation.",
                    "请等待当前操作完成。",
                    "現在の処理が完了するまでお待ちください。",
                    "현재 작업이 끝날 때까지 기다려 주세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        }
        if self
            .snapshot
            .as_ref()
            .is_none_or(|snapshot| index >= snapshot.resources.len())
        {
            return;
        }
        let (Some(session), Ok(resource_index)) = (self.session.clone(), u32::try_from(index))
        else {
            return;
        };
        if self.selected_resource != Some(index) {
            self.usage_page = None;
        }
        self.selected_resource = Some(index);
        self.resource_pending = true;
        let revision = self.open_revision;
        self.status = Some(
            self.t(
                "Loading resource usage…",
                "正在加载资源使用记录…",
                "リソース使用履歴を読み込み中…",
                "리소스 사용 기록을 불러오는 중…",
            )
            .to_owned(),
        );
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            session.resource_usage(resource_index, offset, RESOURCE_USAGE_PAGE_LIMIT)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.open_revision != revision {
                    return;
                }
                this.resource_pending = false;
                if this.selected_resource != Some(index) {
                    this.status = None;
                    cx.notify();
                    return;
                }
                match result {
                    Ok(page) => {
                        this.usage_page = Some(page);
                        this.status = None;
                    }
                    Err(error) => {
                        this.status = Some(format!(
                            "{}: {error}",
                            this.t(
                                "Could not load resource usage",
                                "无法加载资源使用记录",
                                "リソース使用履歴を取得できません",
                                "리소스 사용 기록을 불러올 수 없습니다"
                            )
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn jump_to_usage(&mut self, event: u32, window: &mut Window, cx: &mut Context<Self>) {
        if self.replay_pending || self.export_pending || self.resource_pending {
            self.status = Some(
                self.t(
                    "Wait for the current operation.",
                    "请等待当前操作完成。",
                    "現在の処理が完了するまでお待ちください。",
                    "현재 작업이 끝날 때까지 기다려 주세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let target = snapshot
            .actions
            .iter()
            .enumerate()
            .find(|(_, action)| action.event_id == event)
            .map(|(index, _)| (index, None))
            .or_else(|| {
                snapshot
                    .actions
                    .iter()
                    .enumerate()
                    .find_map(|(index, action)| {
                        action
                            .api_events
                            .iter()
                            .position(|api| api.event_id == event)
                            .map(|api| (index, Some(api)))
                    })
            });
        let Some((action, api)) = target else {
            self.status = Some(
                self.t(
                    "This usage has no navigable event.",
                    "该使用记录没有可跳转的事件。",
                    "この使用履歴には移動可能なイベントがありません。",
                    "이 사용 기록에는 이동할 수 있는 이벤트가 없습니다.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        };
        self.event_search
            .update(cx, |state, cx| state.set_value("", window, cx));
        if let Some(row) = self.event_tree.reveal(action, api) {
            self.event_scroll
                .scroll_to_item(row, gpui::ScrollStrategy::Nearest);
        }
        self.tab = CaptureTab::Events;
        self.select_event(action, api, cx);
    }

    fn debug_selected_vertex(&mut self, cx: &mut Context<Self>) {
        if let Some((vertex, instance, index)) = self.debug_vertex_selection {
            self.start_shader_debug(
                ShaderDebugContext::Vertex {
                    vertex,
                    instance,
                    index,
                },
                cx,
            );
        }
    }

    fn debug_selected_pixel(&mut self, cx: &mut Context<Self>) {
        let coordinates = self
            .pixel_x
            .read(cx)
            .value()
            .parse::<u32>()
            .ok()
            .zip(self.pixel_y.read(cx).value().parse::<u32>().ok());
        let Some((x, y)) = coordinates else {
            self.status = Some(
                self.t(
                    "Enter nonnegative integer pixel coordinates.",
                    "请输入非负整数像素坐标。",
                    "非負の整数ピクセル座標を入力してください。",
                    "음이 아닌 정수 픽셀 좌표를 입력하세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        };
        let selection = PixelDebugSelection {
            x,
            y,
            sample: self.texture_selection.sample,
            primitive: None,
            view: None,
        };
        self.start_shader_debug(ShaderDebugContext::Pixel(selection), cx);
    }

    fn start_shader_debug(&mut self, context: ShaderDebugContext, cx: &mut Context<Self>) {
        if self.replay_pending || self.export_pending || self.resource_pending {
            return;
        }
        let (Some(session), Some(event)) = (
            self.session.clone(),
            self.pipeline.as_ref().map(|pipeline| pipeline.event_id),
        ) else {
            return;
        };
        let revision = self.open_revision;
        let selection_revision = self.selection_revision;
        self.debug_view = None;
        self.mesh = MeshView::default();
        self.resource_pending = true;
        self.status = Some(
            self.t(
                "Collecting shader debug trace…",
                "正在收集着色器调试轨迹…",
                "シェーダーデバッグのトレースを収集中…",
                "셰이더 디버그 추적을 수집하는 중…",
            )
            .to_owned(),
        );
        cx.notify();
        let task = cx
            .background_executor()
            .spawn(async move { collect_shader_debug(&session, event, context) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.open_revision != revision || this.selection_revision != selection_revision {
                    return;
                }
                this.resource_pending = false;
                match result {
                    Ok(view) => {
                        this.debug_view = Some(view);
                        this.status = None;
                    }
                    Err(error) => {
                        this.status = Some(format!(
                            "{}: {error}",
                            this.t(
                                "Shader debugging failed",
                                "着色器调试失败",
                                "シェーダーデバッグに失敗しました",
                                "셰이더 디버깅 실패"
                            )
                        ))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn read_vertex_input(&mut self, offset: u32, cx: &mut Context<Self>) {
        if self.replay_pending || self.export_pending || self.resource_pending {
            self.status = Some(
                self.t(
                    "Wait for the current operation.",
                    "请等待当前操作完成。",
                    "現在の処理が完了するまでお待ちください。",
                    "현재 작업이 끝날 때까지 기다려 주세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(action) = self
            .selected
            .and_then(|index| snapshot.actions.get(index))
            .cloned()
        else {
            self.status = Some(
                self.t(
                    "Select a draw action first.",
                    "请先选择绘制操作。",
                    "先に描画アクションを選択してください。",
                    "먼저 그리기 작업을 선택하세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        };
        let Some(pipeline) = self.pipeline.clone() else {
            return;
        };
        let Some(session) = self.session.clone() else {
            return;
        };
        let buffers = snapshot.buffers.clone();
        let revision = self.open_revision;
        let selection_revision = self.selection_revision;
        self.resource_pending = true;
        self.status = Some(
            self.t(
                "Loading vertex inputs…",
                "正在加载顶点输入…",
                "頂点入力を読み込み中…",
                "정점 입력을 불러오는 중…",
            )
            .to_owned(),
        );
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            session.vertex_input_page(&action, &pipeline, &buffers, 0, offset, INPUT_PAGE_LIMIT)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.open_revision != revision || this.selection_revision != selection_revision {
                    return;
                }
                this.resource_pending = false;
                match result {
                    Ok(page) => {
                        this.debug_vertex_selection = None;
                        this.input_page = Some(page);
                        this.status = None;
                    }
                    Err(error) => {
                        this.input_page = None;
                        this.status = Some(format!(
                            "{}: {error}",
                            this.t(
                                "Could not inspect vertex inputs",
                                "无法查看顶点输入",
                                "頂点入力を表示できません",
                                "정점 입력을 확인할 수 없습니다"
                            )
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn read_mesh(&mut self, selection: MeshSelection, offset: u64, cx: &mut Context<Self>) {
        if self.replay_pending || self.export_pending || self.resource_pending {
            self.status = Some(
                self.t(
                    "Wait for the current operation.",
                    "请等待当前操作完成。",
                    "現在の処理が完了するまでお待ちください。",
                    "현재 작업이 끝날 때까지 기다려 주세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        }
        let Some(event) = self.pipeline.as_ref().map(|pipeline| pipeline.event_id) else {
            self.status = Some(
                self.t(
                    "Select a draw or mesh event first.",
                    "请先选择绘制或网格事件。",
                    "先に描画またはメッシュイベントを選択してください。",
                    "먼저 그리기 또는 메시 이벤트를 선택하세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        };
        let Some(session) = self.session.clone() else {
            return;
        };
        let revision = self.open_revision;
        let selection_revision = self.selection_revision;
        self.resource_pending = true;
        self.status = Some(
            self.t(
                "Loading mesh output…",
                "正在加载网格输出…",
                "メッシュ出力を読み込み中…",
                "메시 출력을 불러오는 중…",
            )
            .to_owned(),
        );
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            let info = session.mesh(event, selection)?;
            let bytes = if info.vertex_bound {
                Some(session.mesh_read(
                    event,
                    selection,
                    "vertex",
                    offset,
                    mesh_page_size(&info) as u32,
                )?)
            } else {
                None
            };
            Ok::<_, String>((info, bytes))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.open_revision != revision || this.selection_revision != selection_revision {
                    return;
                }
                this.resource_pending = false;
                match result {
                    Ok((info, bytes)) => {
                        this.mesh = MeshView {
                            selection,
                            info: Some(info),
                            bytes,
                            offset,
                        };
                        this.status = None;
                    }
                    Err(error) => {
                        this.mesh = MeshView {
                            selection,
                            ..MeshView::default()
                        };
                        this.status = Some(format!(
                            "{}: {error}",
                            this.t(
                                "Could not inspect mesh output",
                                "无法查看网格输出",
                                "メッシュ出力を表示できません",
                                "메시 출력을 확인할 수 없습니다"
                            )
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn read_structure(&mut self, path: Vec<u64>, offset: u64, cx: &mut Context<Self>) {
        if self.replay_pending || self.export_pending || self.resource_pending {
            self.status = Some(
                self.t(
                    "Wait for the current operation.",
                    "请等待当前操作完成。",
                    "現在の処理が完了するまでお待ちください。",
                    "현재 작업이 끝날 때까지 기다려 주세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        }
        let Some(event) = self.pipeline.as_ref().map(|pipeline| pipeline.event_id) else {
            self.status = Some(
                self.t(
                    "Select an event before inspecting parameters.",
                    "请先选择事件再查看参数。",
                    "パラメーターを見る前にイベントを選択してください。",
                    "매개변수를 확인하기 전에 이벤트를 선택하세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        };
        let Some(session) = self.session.clone() else {
            return;
        };
        let revision = self.open_revision;
        let selection_revision = self.selection_revision;
        let request_path = path.clone();
        self.resource_pending = true;
        self.status = Some(
            self.t(
                "Loading event parameters…",
                "正在加载事件参数…",
                "イベントパラメーターを読み込み中…",
                "이벤트 매개변수를 불러오는 중…",
            )
            .to_owned(),
        );
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            session.structure(event, &request_path, offset, STRUCTURE_PAGE_LIMIT)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.open_revision != revision || this.selection_revision != selection_revision {
                    return;
                }
                this.resource_pending = false;
                match result {
                    Ok(page) => {
                        this.structure = StructureView {
                            page: Some(page),
                            path,
                            offset,
                            bytes: None,
                            bytes_offset: 0,
                        };
                        this.status = None;
                    }
                    Err(error) => {
                        this.status = Some(format!(
                            "{}: {error}",
                            this.t(
                                "Could not inspect event parameters",
                                "无法查看事件参数",
                                "イベントパラメーターを表示できません",
                                "이벤트 매개변수를 확인할 수 없습니다"
                            )
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn read_structure_buffer(&mut self, offset: u64, cx: &mut Context<Self>) {
        if self.replay_pending || self.export_pending || self.resource_pending {
            return;
        }
        let Some(event) = self.pipeline.as_ref().map(|state| state.event_id) else {
            return;
        };
        let Some(node) = self.structure.page.as_ref().map(|page| &page.node) else {
            return;
        };
        if node.basic_type != 4 {
            return;
        } // upstream SDBasic::Buffer
        let Some(size) = node.byte_size.parse::<u64>().ok() else {
            return;
        };
        if offset > size {
            return;
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        let length = (size - offset).min(BUFFER_VIEW_PAGE) as u32;
        let path = self.structure.path.clone();
        let revision = self.open_revision;
        let selection_revision = self.selection_revision;
        self.resource_pending = true;
        self.status = Some(self.t(
            "Loading bytes in an isolated worker; full capture export can use substantial memory…",
            "正在隔离进程中读取字节；完整捕获导出可能占用较多内存…",
            "分離プロセスでバイトを読み込み中。完全なキャプチャーの出力には多くのメモリが必要な場合があります…",
            "분리된 프로세스에서 바이트를 불러오는 중입니다. 전체 캡처 내보내기에 많은 메모리가 필요할 수 있습니다…",
        ).to_owned());
        cx.notify();
        let task = cx
            .background_executor()
            .spawn(async move { session.structure_buffer(event, &path, offset, length) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.open_revision != revision || this.selection_revision != selection_revision {
                    return;
                }
                this.resource_pending = false;
                match result {
                    Ok(bytes) => {
                        this.structure.bytes = Some(bytes);
                        this.structure.bytes_offset = offset;
                        this.status = None;
                    }
                    Err(error) => {
                        this.status = Some(format!(
                            "{}: {error}",
                            this.t(
                                "Could not read structured bytes",
                                "无法读取结构化字节",
                                "構造化バイトを読み込めません",
                                "구조화된 바이트를 읽을 수 없습니다"
                            )
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn advance_texture(&mut self, field: TextureField, cx: &mut Context<Self>) {
        if self.replay_pending || self.export_pending || self.resource_pending {
            self.status = Some(
                self.t(
                    "Wait for the current operation.",
                    "请等待当前操作完成。",
                    "現在の処理が完了するまでお待ちください。",
                    "현재 작업이 끝날 때까지 기다려 주세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        }
        let Some(texture) = self
            .selected_texture
            .and_then(|index| self.snapshot.as_ref()?.textures.get(index))
        else {
            return;
        };
        let next = |value: u32, count: u32| {
            value
                .checked_add(1)
                .filter(|next| *next < count && *next <= i32::MAX as u32)
                .unwrap_or(0)
        };
        match field {
            TextureField::Mip => {
                self.texture_selection.mip = next(self.texture_selection.mip, texture.mips);
                self.texture_selection.slice = 0;
            }
            TextureField::Slice => {
                let count = if texture.dimension == 3 {
                    texture
                        .depth
                        .checked_shr(self.texture_selection.mip)
                        .unwrap_or(0)
                        .max(1)
                } else {
                    texture.array_size
                };
                self.texture_selection.slice = next(self.texture_selection.slice, count);
            }
            TextureField::Sample => {
                self.texture_selection.sample = match self.texture_selection.sample {
                    None if texture.samples > 0 => Some(0),
                    Some(sample) => sample
                        .checked_add(1)
                        .filter(|value| *value < texture.samples),
                    _ => None,
                };
            }
            TextureField::Channel => {
                self.texture_selection.channel = match self.texture_selection.channel {
                    None if texture.component_count > 0 => Some(0),
                    Some(channel) => channel.checked_add(1).filter(|value| {
                        *value < texture.component_count && *value <= i32::MAX as u32
                    }),
                    _ => None,
                };
            }
        }
        self.preview = None;
        self.pixel = None;
        self.statistics = None;
        self.pixel_history = None;
        self.history_selected = None;
        self.status = None;
        cx.notify();
    }

    fn export_texture(&mut self, cx: &mut Context<Self>) {
        if self.replay_pending
            || self.export_pending
            || self.resource_pending
            || self.selected.is_none()
        {
            self.status = Some(
                self.t(
                    "Select an event and wait for replay to finish before exporting.",
                    "请先选择事件并等待回放完成，再导出纹理。",
                    "イベントを選択し、リプレイ完了後に書き出してください。",
                    "이벤트를 선택하고 리플레이가 끝난 뒤 내보내세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        }
        let (Some(session), Some(index)) = (self.session.clone(), self.selected_texture) else {
            return;
        };
        let Ok(index) = u32::try_from(index) else {
            return;
        };
        let selection = self.texture_selection;
        let Some(texture) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.textures.get(index as usize))
        else {
            return;
        };
        if selection.validate(texture).is_err() {
            self.status = Some(
                self.t(
                    "Texture selection is out of range.",
                    "纹理子资源选择超出范围。",
                    "テクスチャの選択範囲が無効です。",
                    "텍스처 선택 범위가 올바르지 않습니다.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        }
        let Some(destination) = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .save_file()
        else {
            return;
        };
        let destination_label = destination.display().to_string();
        self.export_pending = true;
        let revision = self.open_revision;
        self.status = Some(
            self.t(
                "Exporting texture…",
                "正在导出纹理…",
                "テクスチャを書き出し中…",
                "텍스처를 내보내는 중…",
            )
            .to_owned(),
        );
        cx.notify();
        let task = cx
            .background_executor()
            .spawn(async move { session.export_texture_selected(index, selection, &destination) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.open_revision != revision {
                    return;
                }
                this.export_pending = false;
                this.status = Some(match result {
                    Ok(()) => format!(
                        "{}: {destination_label}",
                        this.t(
                            "Texture exported",
                            "纹理已导出",
                            "テクスチャを書き出しました",
                            "텍스처를 내보냈습니다",
                        )
                    ),
                    Err(error) => format!(
                        "{}: {error}",
                        this.t(
                            "Texture export failed",
                            "纹理导出失败",
                            "テクスチャを書き出せません",
                            "텍스처 내보내기 실패"
                        )
                    ),
                });
                cx.notify();
            });
        })
        .detach();
    }

    fn read_pixel_history(&mut self, offset: u64, reuse: bool, cx: &mut Context<Self>) {
        if self.replay_pending || self.export_pending || self.resource_pending {
            self.status = Some(
                self.t(
                    "Wait for the current operation.",
                    "请等待当前操作完成。",
                    "現在の処理が完了するまでお待ちください。",
                    "현재 작업이 끝날 때까지 기다려 주세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        }
        let Some(event) = self.pipeline.as_ref().map(|pipeline| pipeline.event_id) else {
            self.status = Some(
                self.t(
                    "Select an event before inspecting pixel history.",
                    "请先选择事件再查看像素历史。",
                    "ピクセル履歴を見る前にイベントを選択してください。",
                    "픽셀 기록을 확인하기 전에 이벤트를 선택하세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        };
        let coordinates = if reuse {
            self.pixel_history.as_ref().map(|page| (page.x, page.y))
        } else {
            self.pixel_x
                .read(cx)
                .value()
                .parse::<u32>()
                .ok()
                .zip(self.pixel_y.read(cx).value().parse::<u32>().ok())
        };
        let Some((x, y)) = coordinates else {
            self.status = Some(
                self.t(
                    "Enter nonnegative integer pixel coordinates.",
                    "请输入非负整数像素坐标。",
                    "非負の整数ピクセル座標を入力してください。",
                    "음이 아닌 정수 픽셀 좌표를 입력하세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        };
        let (Some(session), Some(index)) = (self.session.clone(), self.selected_texture) else {
            return;
        };
        let Some(texture) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.textures.get(index))
            .cloned()
        else {
            return;
        };
        let Ok(texture_index) = u32::try_from(index) else {
            return;
        };
        let selection = self.texture_selection;
        let revision = self.open_revision;
        let selection_revision = self.selection_revision;
        if !reuse {
            self.pixel_history = None;
            self.history_selected = None;
        }
        self.resource_pending = true;
        self.status = Some(
            self.t(
                "Replaying pixel history; this may take time…",
                "正在回放像素历史，可能需要一些时间…",
                "ピクセル履歴をリプレイ中です。時間がかかる場合があります…",
                "픽셀 기록을 리플레이하는 중입니다. 시간이 걸릴 수 있습니다…",
            )
            .to_owned(),
        );
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            session.pixel_history(
                event,
                texture_index,
                &texture,
                selection,
                x,
                y,
                offset,
                PIXEL_HISTORY_PAGE_LIMIT,
            )
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.open_revision != revision || this.selection_revision != selection_revision {
                    return;
                }
                this.resource_pending = false;
                if this.selected_texture != Some(index) || this.texture_selection != selection {
                    this.status = None;
                    cx.notify();
                    return;
                }
                match result {
                    Ok(page) => {
                        this.history_selected = (!page.entries.is_empty()).then_some(0);
                        this.pixel_history = Some(page);
                        this.status = None;
                    }
                    Err(error) => {
                        this.status = Some(format!(
                            "{}: {error}",
                            this.t(
                                "Could not inspect pixel history",
                                "无法查看像素历史",
                                "ピクセル履歴を表示できません",
                                "픽셀 기록을 확인할 수 없습니다"
                            )
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn compute_texture_statistics(&mut self, cx: &mut Context<Self>) {
        if self.replay_pending || self.export_pending || self.resource_pending {
            self.status = Some(
                self.t(
                    "Wait for the current operation.",
                    "请等待当前操作完成。",
                    "現在の処理が完了するまでお待ちください。",
                    "현재 작업이 끝날 때까지 기다려 주세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        }
        let Some(event) = self.pipeline.as_ref().map(|pipeline| pipeline.event_id) else {
            self.status = Some(
                self.t(
                    "Select an event before analyzing a texture.",
                    "请先选择事件再分析纹理。",
                    "テクスチャを解析する前にイベントを選択してください。",
                    "텍스처를 분석하기 전에 이벤트를 선택하세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        };
        let (Ok(min), Ok(max)) = (
            self.stats_min.read(cx).value().parse::<f32>(),
            self.stats_max.read(cx).value().parse::<f32>(),
        ) else {
            self.status = Some(
                self.t(
                    "Enter a finite histogram range.",
                    "请输入有限的直方图范围。",
                    "有限のヒストグラム範囲を入力してください。",
                    "유한한 히스토그램 범위를 입력하세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        };
        let (Some(session), Some(index)) = (self.session.clone(), self.selected_texture) else {
            return;
        };
        let Some(texture) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.textures.get(index))
            .cloned()
        else {
            return;
        };
        let Ok(texture_index) = u32::try_from(index) else {
            return;
        };
        let selection = self.texture_selection;
        let channels = std::array::from_fn(|channel| {
            selection
                .channel
                .map_or(channel < texture.component_count as usize, |selected| {
                    selected as usize == channel
                })
        });
        let request = HistogramRequest { min, max, channels };
        if request.validate().is_err() || selection.channel.is_some_and(|channel| channel >= 4) {
            self.status = Some(
                self.t(
                    "Invalid histogram range or channel.",
                    "直方图范围或通道无效。",
                    "ヒストグラムの範囲またはチャンネルが無効です。",
                    "히스토그램 범위 또는 채널이 올바르지 않습니다.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        }
        let revision = self.open_revision;
        let selection_revision = self.selection_revision;
        self.statistics = None;
        self.resource_pending = true;
        self.status = Some(
            self.t(
                "Analyzing texture…",
                "正在分析纹理…",
                "テクスチャを解析中…",
                "텍스처 분석 중…",
            )
            .to_owned(),
        );
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            session.texture_statistics(event, texture_index, &texture, selection, request)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.open_revision != revision || this.selection_revision != selection_revision {
                    return;
                }
                this.resource_pending = false;
                if this.selected_texture != Some(index) || this.texture_selection != selection {
                    this.status = None;
                    cx.notify();
                    return;
                }
                match result {
                    Ok(stats) => {
                        this.statistics = Some(stats);
                        this.status = None;
                    }
                    Err(error) => {
                        this.status = Some(format!(
                            "{}: {error}",
                            this.t(
                                "Could not analyze texture",
                                "无法分析纹理",
                                "テクスチャを解析できません",
                                "텍스처를 분석할 수 없습니다"
                            )
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn pick_texture_pixel(&mut self, cx: &mut Context<Self>) {
        if self.replay_pending || self.export_pending || self.resource_pending {
            self.status = Some(
                self.t(
                    "Wait for the current operation.",
                    "请等待当前操作完成。",
                    "現在の処理が完了するまでお待ちください。",
                    "현재 작업이 끝날 때까지 기다려 주세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        }
        let Some(event) = self.pipeline.as_ref().map(|pipeline| pipeline.event_id) else {
            self.status = Some(
                self.t(
                    "Select an event before picking a pixel.",
                    "请先选择事件再读取像素。",
                    "ピクセルを取得する前にイベントを選択してください。",
                    "픽셀을 읽기 전에 이벤트를 선택하세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        };
        let (Ok(x), Ok(y)) = (
            self.pixel_x.read(cx).value().parse::<u32>(),
            self.pixel_y.read(cx).value().parse::<u32>(),
        ) else {
            self.status = Some(
                self.t(
                    "Enter nonnegative integer pixel coordinates.",
                    "请输入非负整数像素坐标。",
                    "非負の整数ピクセル座標を入力してください。",
                    "음이 아닌 정수 픽셀 좌표를 입력하세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        };
        let (Some(session), Some(index)) = (self.session.clone(), self.selected_texture) else {
            return;
        };
        let Some(texture) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.textures.get(index))
            .cloned()
        else {
            return;
        };
        let Ok(texture_index) = u32::try_from(index) else {
            return;
        };
        let selection = self.texture_selection;
        let revision = self.open_revision;
        let selection_revision = self.selection_revision;
        self.pixel = None;
        self.pixel_history = None;
        self.history_selected = None;
        self.resource_pending = true;
        self.status = Some(
            self.t(
                "Picking pixel…",
                "正在读取像素…",
                "ピクセルを取得中…",
                "픽셀을 읽는 중…",
            )
            .to_owned(),
        );
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            session.pick_pixel(event, texture_index, &texture, selection, x, y)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.open_revision != revision || this.selection_revision != selection_revision {
                    return;
                }
                this.resource_pending = false;
                if this.selected_texture != Some(index) || this.texture_selection != selection {
                    this.status = None;
                    cx.notify();
                    return;
                }
                match result {
                    Ok(pixel) => {
                        this.pixel = Some(pixel);
                        this.status = None;
                    }
                    Err(error) => {
                        this.status = Some(format!(
                            "{}: {error}",
                            this.t(
                                "Could not pick pixel",
                                "无法读取像素",
                                "ピクセルを取得できません",
                                "픽셀을 읽을 수 없습니다"
                            )
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn preview_texture(&mut self, cx: &mut Context<Self>) {
        if self.replay_pending
            || self.export_pending
            || self.resource_pending
            || self.selected.is_none()
        {
            self.status = Some(
                self.t(
                    "Select an event and wait for replay to finish before previewing.",
                    "请先选择事件并等待回放完成，再预览纹理。",
                    "イベントを選択し、リプレイ完了後にプレビューしてください。",
                    "이벤트를 선택하고 리플레이가 끝난 뒤 미리 보세요.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        }
        let (Some(session), Some(index)) = (self.session.clone(), self.selected_texture) else {
            return;
        };
        let Ok(texture_index) = u32::try_from(index) else {
            return;
        };
        let selection = self.texture_selection;
        let Some(texture) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.textures.get(index))
        else {
            return;
        };
        if selection.validate(texture).is_err() {
            self.status = Some(
                self.t(
                    "Texture selection is out of range.",
                    "纹理子资源选择超出范围。",
                    "テクスチャの選択範囲が無効です。",
                    "텍스처 선택 범위가 올바르지 않습니다.",
                )
                .to_owned(),
            );
            cx.notify();
            return;
        }
        let revision = self.open_revision;
        self.preview = None;
        self.pixel = None;
        self.export_pending = true;
        self.status = Some(
            self.t(
                "Loading texture preview…",
                "正在加载纹理预览…",
                "テクスチャのプレビューを読み込み中…",
                "텍스처 미리 보기를 불러오는 중…",
            )
            .to_owned(),
        );
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            let directory = tempfile::tempdir().map_err(|e| e.to_string())?;
            let path = directory.path().join("preview.png");
            session.export_texture_selected(texture_index, selection, &path)?;
            Ok::<_, String>(TexturePreview {
                path,
                _directory: directory,
            })
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.open_revision != revision {
                    return;
                }
                this.export_pending = false;
                if this.selected_texture != Some(index) || this.texture_selection != selection {
                    this.status = None;
                    cx.notify();
                    return;
                }
                match result {
                    Ok(preview) => {
                        this.preview = Some(preview);
                        this.status = None;
                    }
                    Err(error) => {
                        this.status = Some(format!(
                            "{}: {error}",
                            this.t(
                                "Texture preview failed",
                                "纹理预览失败",
                                "テクスチャをプレビューできません",
                                "텍스처 미리 보기 실패"
                            )
                        ))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn shader_debug_panel(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let debug = self.debug_view.as_ref()?;
        let previous = button(
            "vertex-debug-previous",
            self.t("Previous step", "上一步", "前のステップ", "이전 단계"),
            false,
            |this, cx| {
                if let Some(debug) = &mut this.debug_view {
                    debug.position = debug.position.saturating_sub(1);
                    cx.notify();
                }
            },
            cx,
        );
        let next = button(
            "vertex-debug-next",
            self.t("Next step", "下一步", "次のステップ", "다음 단계"),
            false,
            |this, cx| {
                if let Some(debug) = &mut this.debug_view {
                    if debug.position + 1 < debug.states.len() {
                        debug.position += 1;
                        cx.notify();
                    }
                }
            },
            cx,
        );
        let context = match debug.context {
            ShaderDebugContext::Vertex {
                vertex,
                instance,
                index,
            } => format!(
                "{} {vertex} · {} {instance} · {} {index}",
                self.t("Vertex", "顶点", "頂点", "정점"),
                self.t("Instance", "实例", "インスタンス", "인스턴스"),
                self.t("Index", "索引", "インデックス", "인덱스")
            ),
            ShaderDebugContext::Pixel(selection) => format!(
                "{} ({}, {}) · {} {} · {} {} · {} {}",
                self.t("Pixel", "像素", "ピクセル", "픽셀"),
                selection.x,
                selection.y,
                self.t("Sample", "采样", "サンプル", "샘플"),
                selection
                    .sample
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into()),
                self.t("Primitive", "图元", "プリミティブ", "프리미티브"),
                selection
                    .primitive
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into()),
                self.t("View", "视图", "ビュー", "뷰"),
                selection
                    .view
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into())
            ),
        };
        let mut view = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_sm().text_color(rgb(0x82aaff)).child(self.t(
                "SHADER DEBUG TRACE",
                "着色器调试轨迹",
                "シェーダーデバッグトレース",
                "셰이더 디버그 추적",
            )))
            .child(format!(
                "{context} · {} {}",
                self.t("Event", "事件", "イベント", "이벤트"),
                debug.trace.event_id
            ))
            .child(div().flex().gap_2().child(previous).child(next));
        if let Some(state) = debug.states.get(debug.position) {
            view = view
                .child(div().id("vertex-debug-step").test_support().child(format!(
                    "{} {} / {} · {} {} · {} {}",
                    self.t("Step", "步骤", "ステップ", "단계"),
                    state.step,
                    debug.states.len(),
                    self.t("Next instruction", "下一条指令", "次の命令", "다음 명령"),
                    state.next_instruction,
                    self.t("Flags", "标志", "フラグ", "플래그"),
                    state.flags
                )))
                .child(format!(
                    "{}: {}",
                    self.t("Call stack", "调用栈", "呼び出し履歴", "호출 스택"),
                    state.callstack.join(" → ")
                ))
                .child(self.t("Variable changes", "变量变化", "変数の変化", "변수 변경"))
                .children(state.changes.iter().map(|change| {
                    div().text_sm().child(format!(
                        "{} → {}",
                        shader_debug::display_variable(&change.before),
                        shader_debug::display_variable(&change.after)
                    ))
                }));
        } else {
            view = view.child(self.t(
                "No debug states were returned.",
                "未返回调试状态。",
                "デバッグ状態が返されませんでした。",
                "디버그 상태가 반환되지 않았습니다.",
            ));
        }
        let groups = [
            (
                self.t("Inputs", "输入", "入力", "입력"),
                &debug.trace.inputs,
            ),
            (
                self.t("Constant blocks", "常量块", "定数ブロック", "상수 블록"),
                &debug.trace.constant_blocks,
            ),
            (
                self.t(
                    "Read-only resources",
                    "只读资源",
                    "読み取り専用リソース",
                    "읽기 전용 리소스",
                ),
                &debug.trace.read_only_resources,
            ),
            (
                self.t(
                    "Read-write resources",
                    "读写资源",
                    "読み書きリソース",
                    "읽기/쓰기 리소스",
                ),
                &debug.trace.read_write_resources,
            ),
            (
                self.t("Samplers", "采样器", "サンプラー", "샘플러"),
                &debug.trace.samplers,
            ),
        ];
        view = view.children(groups.into_iter().filter(|(_, variables)| !variables.is_empty()).map(|(label, variables)| {
            div().flex().flex_col().gap_2().child(label).children(variables.iter().map(|variable| div().text_sm().child(shader_debug::display_variable(variable))))
        }))
            .child(div().text_sm().text_color(rgb(0x9eacc0)).child(self.t("Unsupported value types are shown as native-endian bytes; source mapping is not yet available.", "未解码的类型显示原生字节序的原始字节；暂不支持源码映射。", "未対応の型はネイティブエンディアンの生バイトで表示します。ソース対応は未実装です。", "지원하지 않는 유형은 네이티브 엔디언 원시 바이트로 표시합니다. 소스 매핑은 아직 지원되지 않습니다.")))
            .child(div().id("vertex-debug-disassembly").test_support().text_sm().child(debug.disassembly.clone()));
        Some(view.into_any_element())
    }

    fn capture_view(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(snapshot) = &self.snapshot {
            if self.event_tree.len() != snapshot.actions.len() {
                self.event_tree.reset(&snapshot.actions);
            }
        }
        let snapshot = self
            .snapshot
            .as_ref()
            .expect("capture view requires a snapshot");
        let mesh_lines = match (&self.mesh.info, &self.mesh.bytes) {
            (Some(info), Some(bytes)) => {
                if let Some(positions) = mesh_positions(info, bytes, self.mesh.offset) {
                    positions
                        .into_iter()
                        .map(|(index, values)| format!("{index}: {values:?}"))
                        .collect::<Vec<_>>()
                } else {
                    bytes
                        .chunks(16)
                        .enumerate()
                        .map(|(index, chunk)| {
                            let hex: String =
                                chunk.iter().map(|byte| format!("{byte:02x} ")).collect();
                            format!("{:08X}: {hex}", self.mesh.offset + (index as u64) * 16)
                        })
                        .collect()
                }
            }
            _ => Vec::new(),
        };
        let count = match self.tab {
            CaptureTab::Resources => self.resource_visible.len(),
            CaptureTab::VertexInput => self.input_page.as_ref().map_or(0, |page| page.rows.len()),
            CaptureTab::Mesh => mesh_lines.len(),
            CaptureTab::Textures => snapshot.textures.len(),
            CaptureTab::Buffers => snapshot.buffers.len(),
            CaptureTab::Events => self.event_tree.visible.len(),
            CaptureTab::ApiCalls => self
                .selected
                .and_then(|i| snapshot.actions.get(i))
                .map_or(0, |action| action.api_events.len()),
            CaptureTab::Parameters => self
                .structure
                .page
                .as_ref()
                .map_or(0, |page| page.children.len()),
            CaptureTab::Shaders => {
                if self.pipeline.is_some() {
                    SHADER_STAGES.len()
                } else {
                    0
                }
            }
        };
        let api = match snapshot.api {
            0 => "D3D11",
            1 => "D3D12",
            2 => "OpenGL",
            3 => "Vulkan",
            _ => "Unknown",
        };
        let title = self
            .capture
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let detail = self.selected.and_then(|i| snapshot.actions.get(i)).cloned();
        let texture_detail = self
            .selected_texture
            .and_then(|i| snapshot.textures.get(i))
            .cloned();
        let buffer_detail = self
            .selected_buffer
            .and_then(|i| snapshot.buffers.get(i))
            .cloned();
        let counts = format!(
            "{}  ·  {} {}  ·  {} {}  ·  {} {}",
            api,
            snapshot.resource_count,
            self.t("resources", "资源", "リソース", "리소스"),
            snapshot.texture_count,
            self.t("textures", "纹理", "テクスチャ", "텍스처"),
            snapshot.buffer_count,
            self.t("buffers", "缓冲区", "バッファ", "버퍼")
        );
        let frame = format!(
            "{} {}{}",
            self.t("Frame", "帧", "フレーム", "프레임"),
            snapshot.frame,
            if snapshot.degraded {
                self.t(
                    " · degraded replay",
                    " · 降级回放",
                    " · 機能制限あり",
                    " · 제한된 리플레이",
                )
            } else {
                ""
            }
        );
        let back = self.t("Workspace", "工作台", "ワークスペース", "작업 공간");
        let open = self.t(
            "Open capture",
            "打开捕获文件",
            "キャプチャを開く",
            "캡처 열기",
        );
        let upstream = self.t(
            "Open in RenderDoc",
            "在 RenderDoc 中打开",
            "RenderDoc で開く",
            "RenderDoc에서 열기",
        );
        let events = self.t("EVENTS", "事件", "イベント", "이벤트");
        let api_calls = self.t("API calls", "API 调用", "API 呼び出し", "API 호출");
        let parameters_label = self.t("Parameters", "参数", "パラメーター", "매개변수");
        let parent_label = self.t("Parent", "上一级", "親", "상위");
        let previous_label = self.t("Previous", "上一页", "前へ", "이전");
        let next_label = self.t("Next", "下一页", "次へ", "다음");
        let load_structure_bytes = self.t(
            "Load bytes (full capture export)",
            "读取字节（完整导出捕获）",
            "バイトを読み込む（キャプチャー全体を出力）",
            "바이트 읽기 (전체 캡처 내보내기)",
        );
        let previous_bytes = self.t("Previous bytes", "上一段字节", "前のバイト", "이전 바이트");
        let next_bytes = self.t("Next bytes", "下一段字节", "次のバイト", "다음 바이트");
        let is_structured_buffer = self
            .structure
            .page
            .as_ref()
            .is_some_and(|page| page.node.basic_type == 4);
        let selected_event_label = self.t(
            "Selected event",
            "当前事件",
            "選択イベント",
            "선택한 이벤트",
        );
        let event_filter_label = self.t(
            "Filter event names",
            "筛选事件名称",
            "イベント名で絞り込み",
            "이벤트 이름 필터",
        );
        let show_api_events = if self.event_tree.show_api() {
            self.t(
                "Hide API events",
                "隐藏 API 事件",
                "API イベントを隠す",
                "API 이벤트 숨기기",
            )
        } else {
            self.t(
                "Show API events",
                "显示 API 事件",
                "API イベントを表示",
                "API 이벤트 표시",
            )
        };
        let textures_label = self.t("Textures", "纹理", "テクスチャ", "텍스처");
        let next_mip = self.t("Next mip", "切换 mip", "mip 切替", "mip 전환");
        let next_slice = self.t("Next slice", "切换层", "スライス切替", "슬라이스 전환");
        let next_sample = self.t("Next sample", "切换采样", "サンプル切替", "샘플 전환");
        let next_channel = self.t("Next channel", "切换通道", "チャンネル切替", "채널 전환");
        let slice_label = self.t("Slice", "层", "スライス", "슬라이스");
        let channel_label = self.t("Channel", "通道", "チャンネル", "채널");
        let resolve_label = self.t("Resolve", "解析", "解決", "리졸브");
        let all_label = self.t("All", "全部", "すべて", "전체");
        let buffers_label = self.t("Buffers", "缓冲区", "バッファ", "버퍼");
        let shaders_tab_label = self.t("Shaders", "着色器", "シェーダー", "셰이더");
        let shader_details = self.t(
            "SHADER DETAILS",
            "着色器详情",
            "シェーダー詳細",
            "셰이더 상세",
        );
        let shader_empty = self.t(
            "Select an event and a bound shader stage.",
            "选择事件及已绑定的着色器阶段。",
            "イベントとバインドされたシェーダーステージを選択してください。",
            "이벤트와 바인딩된 셰이더 단계를 선택하세요.",
        );
        let next_target = self.t("Next target", "切换目标", "ターゲット切替", "대상 전환");
        let entry_label = self.t("Entry point", "入口点", "エントリポイント", "진입점");
        let disasm_label = self.t("Disassembly", "反汇编", "逆アセンブル", "역어셈블리");
        let shader_inputs_label = self.t("Inputs", "输入", "入力", "입력");
        let shader_outputs_label = self.t("Outputs", "输出", "出力", "출력");
        let constants_label = self.t("Constant blocks", "常量块", "定数ブロック", "상수 블록");
        let target_label = self.t("Target", "目标", "ターゲット", "대상");
        let default_label = self.t("Default", "默认", "既定", "기본값");
        let bound_label = self.t("bound", "已绑定", "バインド済み", "바인딩됨");
        let unbound_label = self.t("unbound", "未绑定", "未バインド", "바인딩 안 됨");
        let buffer_label = self.t("Buffer", "缓冲区", "バッファ", "버퍼");
        let buffer_details = self.t(
            "BUFFER DETAILS",
            "缓冲区详情",
            "バッファの詳細",
            "버퍼 상세",
        );
        let previous = self.t("Previous", "上一页", "前へ", "이전");
        let next = self.t("Next", "下一页", "次へ", "다음");
        let read = self.t("Read", "读取", "読み込み", "읽기");
        let buffer_empty = self.t(
            "Select a buffer to inspect its bytes.",
            "选择缓冲区查看字节。",
            "バッファを選択してバイトを表示します。",
            "버퍼를 선택해 바이트를 확인하세요.",
        );
        let texture_label = self.t("Texture", "纹理", "テクスチャ", "텍스처");
        let array_label = self.t("Array", "数组", "配列", "배열");
        let samples_label = self.t("Samples", "采样", "サンプル", "샘플");
        let bytes_label = self.t("Bytes", "字节", "バイト", "바이트");
        let format_label = self.t("Format", "格式", "形式", "형식");
        let component_label = self.t("Component", "分量", "成分", "구성 요소");
        let texture_details = self.t(
            "TEXTURE DETAILS",
            "纹理详情",
            "テクスチャの詳細",
            "텍스처 상세",
        );
        let export_label = self.t("Export PNG", "导出 PNG", "PNG を書き出す", "PNG 내보내기");
        let preview_label = self.t("Preview", "预览", "プレビュー", "미리 보기");
        let preview_note = self.t(
            "8-bit PNG with RenderDoc's default mapping.",
            "8 位 PNG，使用 RenderDoc 默认映射。",
            "RenderDoc の既定のマッピングによる 8 ビット PNG。",
            "RenderDoc 기본 매핑을 사용한 8비트 PNG입니다.",
        );
        let export_note = self.t(
            "Existing files are never replaced.",
            "不会覆盖已有文件。",
            "既存のファイルは上書きしません。",
            "기존 파일을 덮어쓰지 않습니다.",
        );
        let texture_empty = self.t(
            "Select a texture to inspect its metadata.",
            "选择纹理查看元数据。",
            "テクスチャを選択して情報を表示します。",
            "텍스처를 선택해 정보를 확인하세요.",
        );
        let details = self.t("ACTION DETAILS", "操作详情", "アクション詳細", "작업 상세");
        let empty = self.t(
            "Select an action to replay its event.",
            "选择操作以回放对应事件。",
            "アクションを選択してイベントをリプレイします。",
            "작업을 선택하여 이벤트를 리플레이합니다.",
        );
        let event_label = self.t("Event", "事件", "イベント", "이벤트");
        let action_label = self.t("Action", "操作", "アクション", "작업");
        let flags_label = self.t("Flags", "标志", "フラグ", "플래그");
        let indices_label = self.t(
            "Indices / vertices",
            "索引 / 顶点",
            "インデックス / 頂点",
            "인덱스 / 정점",
        );
        let instances_label = self.t("Instances", "实例", "インスタンス", "인스턴스");
        let pipeline_label = self.t("PIPELINE", "管线", "パイプライン", "파이프라인");
        let topology_label = self.t("Topology", "拓扑", "トポロジー", "토폴로지");
        let shaders_label = self.t(
            "Bound shaders",
            "已绑定着色器",
            "バインド済みシェーダー",
            "바인딩된 셰이더",
        );
        let outputs_label = self.t(
            "Bound outputs",
            "已绑定输出",
            "バインド済み出力",
            "바인딩된 출력",
        );
        let pipeline_unavailable = self.t(
            "Pipeline state unavailable",
            "管线状态不可用",
            "パイプライン状態を取得できません",
            "파이프라인 상태를 사용할 수 없습니다",
        );
        let vertex_buffers_label =
            self.t("Vertex buffers", "顶点缓冲区", "頂点バッファ", "정점 버퍼");
        let index_buffer_label = self.t(
            "Index buffer",
            "索引缓冲区",
            "インデックスバッファ",
            "인덱스 버퍼",
        );
        let inputs_label = self.t("Vertex inputs", "顶点输入", "頂点入力", "정점 입력");
        let viewport_label = self.t("Viewports", "视口", "ビューポート", "뷰포트");
        let unavailable = self.t("Unavailable", "不可用", "利用不可", "사용할 수 없음");
        let offset_label = self.t("offset", "偏移", "オフセット", "오프셋");
        let stride_label = self.t("stride", "步长", "ストライド", "스트라이드");
        let enabled_label = self.t("enabled", "启用", "有効", "활성");
        let disabled_label = self.t("disabled", "禁用", "無効", "비활성");
        let depth_label = self.t("depth", "深度", "深度", "깊이");
        let pipeline = self.pipeline.clone();
        let selected_event = pipeline.as_ref().map(|state| state.event_id);
        let status = self.status.clone();
        let preview_path = self.preview.as_ref().map(|preview| preview.path.clone());

        let list = if self.tab == CaptureTab::Resources {
            let visible = std::sync::Arc::new(self.resource_visible.clone());
            uniform_list(
                "capture-resources",
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .filter_map(|row| {
                            let index = *visible.get(row)?;
                            let resource = this.snapshot.as_ref()?.resources.get(index)?;
                            let name = resource.name.clone();
                            let kind = resource_type_name(resource.resource_type);
                            Some(
                                div()
                                    .id(("resource", index))
                                    .test_support()
                                    .h(px(32.))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .cursor_pointer()
                                    .bg(rgb(if this.selected_resource == Some(index) {
                                        0x304c7c
                                    } else {
                                        0x192332
                                    }))
                                    .hover(|row| row.bg(rgb(0x263950)))
                                    .child(
                                        div()
                                            .text_color(rgb(0x91a7c0))
                                            .child(format!("#{index} · {kind}")),
                                    )
                                    .child(div().overflow_hidden().child(name))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.read_resource_usage(index, 0, cx)
                                    })),
                            )
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .flex_1()
            .into_any_element()
        } else if self.tab == CaptureTab::VertexInput {
            uniform_list(
                "capture-vertex-input",
                count,
                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .filter_map(|index| {
                            let row = this.input_page.as_ref()?.rows.get(index)?;
                            let selection = row.vertex_index.map(|vertex_index| {
                                (
                                    row.row,
                                    this.input_page.as_ref().unwrap().instance,
                                    vertex_index,
                                )
                            });
                            let values = row
                                .attributes
                                .iter()
                                .map(|attribute| {
                                    let value = attribute
                                        .values
                                        .as_ref()
                                        .map(|v| v.join(", "))
                                        .or_else(|| attribute.hex.clone())
                                        .unwrap_or_else(|| "—".into());
                                    format!("{}: {value}", attribute.name)
                                })
                                .collect::<Vec<_>>()
                                .join(" · ");
                            let display = if row.restart {
                                format!("{}: ↻", row.row)
                            } else {
                                format!(
                                    "{}  [{}]  {values}",
                                    row.row,
                                    row.vertex_index
                                        .map(|v| v.to_string())
                                        .unwrap_or_else(|| "—".into())
                                )
                            };
                            Some(
                                div()
                                    .id(("vertex-input-row", index))
                                    .test_support()
                                    .h(px(32.))
                                    .px_3()
                                    .overflow_hidden()
                                    .bg(rgb(
                                        if selection.is_some()
                                            && selection == this.debug_vertex_selection
                                        {
                                            0x304c7c
                                        } else {
                                            0x192332
                                        },
                                    ))
                                    .child(display)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if !this.resource_pending
                                            && !this.replay_pending
                                            && !this.export_pending
                                        {
                                            if let Some(selection) = selection {
                                                this.debug_vertex_selection = Some(selection);
                                                this.debug_view = None;
                                                cx.notify();
                                            }
                                        }
                                    })),
                            )
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .flex_1()
            .into_any_element()
        } else if self.tab == CaptureTab::Mesh {
            let lines = std::sync::Arc::new(mesh_lines);
            uniform_list(
                "capture-mesh",
                count,
                cx.processor(move |_, range: std::ops::Range<usize>, _, _| {
                    range
                        .filter_map(|index| {
                            lines.get(index).map(|line| {
                                div()
                                    .id(("mesh-row", index))
                                    .test_support()
                                    .h(px(32.))
                                    .px_3()
                                    .child(line.clone())
                            })
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .flex_1()
            .into_any_element()
        } else if self.tab == CaptureTab::Parameters {
            uniform_list(
                "capture-structured-children",
                count,
                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .filter_map(|index| {
                            let node = this.structure.page.as_ref()?.children.get(index)?;
                            let mut path = this.structure.path.clone();
                            let offset = this.structure.offset.checked_add(index as u64)?;
                            path.push(offset);
                            let name = if node.name.is_empty() {
                                format!("[{offset}]")
                            } else {
                                node.name.clone()
                            };
                            let kind = node.type_name.clone();
                            let value = structured_value(node);
                            Some(
                                div()
                                    .id(SharedString::from(format!("structure-child-{index}")))
                                    .test_support()
                                    .h(px(32.))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .cursor_pointer()
                                    .hover(|row| row.bg(rgb(0x263950)))
                                    .child(div().w(px(150.)).overflow_hidden().child(name))
                                    .child(
                                        div()
                                            .w(px(120.))
                                            .overflow_hidden()
                                            .text_color(rgb(0x91a7c0))
                                            .child(kind),
                                    )
                                    .child(div().flex_1().overflow_hidden().child(value))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.read_structure(path.clone(), 0, cx)
                                    })),
                            )
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .flex_1()
            .into_any_element()
        } else if self.tab == CaptureTab::ApiCalls && count == 0 {
            let message = if self.selected.is_none() {
                self.t(
                    "Select an action to view its API calls.",
                    "选择操作以查看其 API 调用。",
                    "アクションを選択して API 呼び出しを表示します。",
                    "작업을 선택하여 API 호출을 확인하세요.",
                )
            } else {
                self.t(
                    "No API calls are available for this action.",
                    "此操作没有可用的 API 调用。",
                    "このアクションに利用可能な API 呼び出しはありません。",
                    "이 작업에 사용 가능한 API 호출이 없습니다.",
                )
            };
            div()
                .p_3()
                .flex_1()
                .text_color(rgb(0x9eacc0))
                .child(message)
                .into_any_element()
        } else if self.tab == CaptureTab::ApiCalls {
            uniform_list(
                "capture-api-events",
                count,
                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .filter_map(|index| {
                            let action_index = this.selected?;
                            let event = this
                                .snapshot
                                .as_ref()?
                                .actions
                                .get(action_index)?
                                .api_events
                                .get(index)?;
                            let event_id = event.event_id;
                            let name = if event.name.is_empty() {
                                this.t(
                                    "Unnamed API event",
                                    "未命名 API 事件",
                                    "名前のない API イベント",
                                    "이름 없는 API 이벤트",
                                )
                                .to_owned()
                            } else {
                                event.name.clone()
                            };
                            let selected = this
                                .pipeline
                                .as_ref()
                                .is_some_and(|state| state.event_id == event_id);
                            Some(
                                div()
                                    .id(SharedString::from(format!("api-event-{index}")))
                                    .test_support()
                                    .h(px(32.))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .cursor_pointer()
                                    .bg(rgb(if selected { 0x304c7c } else { 0x192332 }))
                                    .hover(|row| row.bg(rgb(0x263950)))
                                    .child(
                                        div()
                                            .w(px(64.))
                                            .flex_none()
                                            .text_color(rgb(0x91a7c0))
                                            .child(event_id.to_string()),
                                    )
                                    .child(div().overflow_hidden().child(name))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select_event(action_index, Some(index), cx)
                                    })),
                            )
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .flex_1()
            .into_any_element()
        } else if self.tab == CaptureTab::Shaders {
            uniform_list(
                "capture-shaders",
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .filter_map(|index| {
                            let stage = *SHADER_STAGES.get(index)?;
                            let shader = this
                                .pipeline
                                .as_ref()?
                                .shaders
                                .iter()
                                .find(|shader| shader.stage == stage)?;
                            let bound = shader.resource.bound;
                            let name = shader.resource.name.clone();
                            Some(
                                div()
                                    .id(SharedString::from(format!("shader-{index}")))
                                    .test_support()
                                    .h(px(32.))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .cursor_pointer()
                                    .bg(rgb(if this.selected_shader == Some(index) {
                                        0x304c7c
                                    } else {
                                        0x192332
                                    }))
                                    .hover(|row| row.bg(rgb(0x263950)))
                                    .child(format!(
                                        "{stage} · {}",
                                        if bound { bound_label } else { unbound_label }
                                    ))
                                    .child(div().overflow_hidden().child(name))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.read_shader(index, None, cx)
                                    })),
                            )
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .flex_1()
            .into_any_element()
        } else if self.tab == CaptureTab::Buffers {
            uniform_list(
                "capture-buffers",
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .filter_map(|index| {
                            let buffer = this.snapshot.as_ref()?.buffers.get(index)?;
                            let name = if buffer.name.is_empty() {
                                format!("{buffer_label} {index}")
                            } else {
                                buffer.name.clone()
                            };
                            let size = buffer.length.clone();
                            Some(
                                div()
                                    .id(SharedString::from(format!("buffer-{index}")))
                                    .test_support()
                                    .h(px(32.))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .cursor_pointer()
                                    .bg(rgb(if this.selected_buffer == Some(index) {
                                        0x304c7c
                                    } else {
                                        0x192332
                                    }))
                                    .hover(|row| row.bg(rgb(0x263950)))
                                    .child(div().overflow_hidden().child(name))
                                    .child(div().text_color(rgb(0x91a7c0)).child(size))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.read_buffer(index, 0, cx)
                                    })),
                            )
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .flex_1()
            .into_any_element()
        } else if self.tab == CaptureTab::Textures {
            uniform_list(
                "capture-textures",
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .filter_map(|index| {
                            let texture = this.snapshot.as_ref()?.textures.get(index)?;
                            let name = if texture.name.is_empty() {
                                format!("{texture_label} {index}")
                            } else {
                                texture.name.clone()
                            };
                            let dimensions = format!(
                                "{} × {} × {}",
                                texture.width, texture.height, texture.depth
                            );
                            Some(
                                div()
                                    .id(SharedString::from(format!("texture-{index}")))
                                    .test_support()
                                    .h(px(32.))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .cursor_pointer()
                                    .bg(rgb(if this.selected_texture == Some(index) {
                                        0x304c7c
                                    } else {
                                        0x192332
                                    }))
                                    .hover(|row| row.bg(rgb(0x263950)))
                                    .child(div().overflow_hidden().child(name))
                                    .child(div().text_color(rgb(0x91a7c0)).child(dimensions))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.selected_texture = Some(index);
                                        this.texture_selection = TextureSelection::default();
                                        this.preview = None;
                                        this.pixel = None;
                                        this.pixel_history = None;
                                        this.history_selected = None;
                                        this.statistics = None;
                                        cx.notify();
                                    })),
                            )
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .flex_1()
            .into_any_element()
        } else {
            uniform_list(
                "capture-actions",
                count,
                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .filter_map(|row| {
                            let node = *this.event_tree.visible.get(row)?;
                            let (index, api_index) = match this.event_tree.row(node)? {
                                EventRow::Action(index) => (index, None),
                                EventRow::Api { action, event } => (action, Some(event)),
                            };
                            let action = this.snapshot.as_ref()?.actions.get(index)?;
                            let (event, name, id): (u32, String, gpui::ElementId) = match api_index
                            {
                                Some(api_index) => {
                                    let api = action.api_events.get(api_index)?;
                                    let name = if api.name.is_empty() {
                                        this.t(
                                            "Unnamed API event",
                                            "未命名 API 事件",
                                            "名前のない API イベント",
                                            "이름 없는 API 이벤트",
                                        )
                                        .to_owned()
                                    } else {
                                        api.name.clone()
                                    };
                                    (
                                        api.event_id,
                                        name,
                                        SharedString::from(format!(
                                            "event-api-{index}-{api_index}"
                                        ))
                                        .into(),
                                    )
                                }
                                None => (action.event_id, action.name.clone(), index.into()),
                            };
                            let selected = this.selected == Some(index)
                                && this
                                    .pipeline
                                    .as_ref()
                                    .is_some_and(|state| state.event_id == event);
                            let indent = action.depth as f32 * 14.0 + 12.0;
                            Some(
                                div()
                                    .h(px(32.))
                                    .pl(px(indent))
                                    .pr_3()
                                    .flex()
                                    .items_center()
                                    .bg(rgb(if selected { 0x304c7c } else { 0x192332 }))
                                    .hover(|row| row.bg(rgb(0x263950)))
                                    .child(if this.event_tree.has_children(node) {
                                        div()
                                            .id(SharedString::from(format!("event-toggle-{index}")))
                                            .test_support()
                                            .w(px(22.))
                                            .flex_none()
                                            .cursor_pointer()
                                            .child(if this.event_tree.is_collapsed(node) {
                                                "▸"
                                            } else {
                                                "▾"
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.event_tree.toggle(node);
                                                cx.notify();
                                            }))
                                            .into_any_element()
                                    } else {
                                        div().w(px(22.)).flex_none().into_any_element()
                                    })
                                    .child(
                                        div()
                                            .id(id)
                                            .test_support()
                                            .flex_1()
                                            .flex()
                                            .items_center()
                                            .gap_3()
                                            .cursor_pointer()
                                            .child(
                                                div()
                                                    .w(px(64.))
                                                    .flex_none()
                                                    .text_color(rgb(0x91a7c0))
                                                    .child(event.to_string()),
                                            )
                                            .child(div().overflow_hidden().child(name))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.select_event(index, api_index, cx)
                                            })),
                                    ),
                            )
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.event_scroll)
            .flex_1()
            .into_any_element()
        };

        let detail_view = if self.tab == CaptureTab::Resources {
            let heading = self.t(
                "RESOURCE INSPECTOR",
                "资源检查器",
                "リソースインスペクター",
                "리소스 검사기",
            );
            let empty = self.t(
                "Select a resource to inspect its relationships and usage.",
                "选择资源以查看关联关系和使用记录。",
                "リソースを選択して関連情報と使用履歴を表示します。",
                "리소스를 선택하여 관계와 사용 기록을 확인하세요.",
            );
            let parents = self.t("Parents", "父资源", "親リソース", "상위 리소스");
            let derived = self.t(
                "Derived resources",
                "派生资源",
                "派生リソース",
                "파생 리소스",
            );
            let chunks = self.t(
                "Initialisation chunks",
                "初始化数据块",
                "初期化チャンク",
                "초기화 청크",
            );
            let usage_label = self.t("Usage", "使用记录", "使用履歴", "사용 기록");
            let untracked = self.t(
                "Usage is not tracked for this resource type.",
                "此类资源未跟踪使用记录。",
                "この種類のリソースでは使用履歴を追跡しません。",
                "이 리소스 유형의 사용 기록은 추적되지 않습니다.",
            );
            let no_usage = self.t(
                "No static usage observed.",
                "未观察到静态使用记录。",
                "静的な使用履歴はありません。",
                "관찰된 정적 사용 기록이 없습니다.",
            );
            let generated_name = self.t(
                "Automatically generated name",
                "自动生成的名称",
                "自動生成された名前",
                "자동 생성된 이름",
            );
            let application_name = self.t(
                "Application-provided name",
                "应用提供的名称",
                "アプリによる名前",
                "애플리케이션에서 지정한 이름",
            );
            let mut view = div()
                .flex()
                .flex_col()
                .gap_3()
                .child(div().text_sm().text_color(rgb(0x82aaff)).child(heading));
            if let Some((index, resource)) = self.selected_resource.and_then(|index| {
                snapshot
                    .resources
                    .get(index)
                    .map(|resource| (index, resource))
            }) {
                view = view
                    .child(format!(
                        "#{index} · {} ({}) · {}",
                        resource_type_name(resource.resource_type),
                        resource.resource_type,
                        resource.name
                    ))
                    .child(if resource.autogenerated_name {
                        generated_name
                    } else {
                        application_name
                    })
                    .child(format!("{chunks}: {:?}", resource.initialisation_chunks));
                for (label, links) in [(parents, &resource.parents), (derived, &resource.derived)] {
                    view = view.child(format!("{label}: {}", links.len()));
                    for (ordinal, link) in links.iter().enumerate() {
                        if let Some(target) = *link {
                            let name = snapshot
                                .resources
                                .get(target)
                                .map(|resource| resource.name.clone())
                                .unwrap_or_default();
                            view = view.child(
                                div()
                                    .id((
                                        if label == parents {
                                            "resource-parent"
                                        } else {
                                            "resource-derived"
                                        },
                                        ordinal,
                                    ))
                                    .test_support()
                                    .text_color(rgb(0x82aaff))
                                    .cursor_pointer()
                                    .child(format!("#{target} · {name}"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.read_resource_usage(target, 0, cx)
                                    })),
                            );
                        } else {
                            view = view.child("—");
                        }
                    }
                }
                view = view.child(div().text_sm().text_color(rgb(0x82aaff)).child(usage_label));
                if let Some(page) = &self.usage_page {
                    if !page.tracked {
                        view = view.child(untracked);
                    } else if page.total == "0" {
                        view = view.child(no_usage);
                    } else {
                        view = view
                            .child(format!("{} / {}", page.offset, page.total))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(button(
                                        "usage-previous",
                                        previous_label,
                                        false,
                                        |this, cx| {
                                            if let (Some(index), Some(page)) =
                                                (this.selected_resource, &this.usage_page)
                                            {
                                                if let Ok(offset) = page.offset.parse::<u64>() {
                                                    if offset > 0 {
                                                        this.read_resource_usage(
                                                            index,
                                                            offset.saturating_sub(u64::from(
                                                                RESOURCE_USAGE_PAGE_LIMIT,
                                                            )),
                                                            cx,
                                                        );
                                                    }
                                                }
                                            }
                                        },
                                        cx,
                                    ))
                                    .child(button(
                                        "usage-next",
                                        next_label,
                                        false,
                                        |this, cx| {
                                            if let (Some(index), Some(page)) =
                                                (this.selected_resource, &this.usage_page)
                                            {
                                                if let (Ok(offset), Ok(total)) = (
                                                    page.offset.parse::<u64>(),
                                                    page.total.parse::<u64>(),
                                                ) {
                                                    if let Some(next) = offset
                                                        .checked_add(page.entries.len() as u64)
                                                    {
                                                        if next < total {
                                                            this.read_resource_usage(
                                                                index, next, cx,
                                                            );
                                                        }
                                                    }
                                                }
                                            }
                                        },
                                        cx,
                                    )),
                            );
                        for (ordinal, entry) in page.entries.iter().enumerate() {
                            let event = entry.event_id;
                            view = view.child(
                                div()
                                    .id(("resource-usage", ordinal))
                                    .test_support()
                                    .cursor_pointer()
                                    .text_color(rgb(0x82aaff))
                                    .child(format!(
                                        "{event_label} {event} · {} ({})",
                                        resource_usage_name(entry.usage),
                                        entry.usage
                                    ))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.jump_to_usage(event, window, cx)
                                    })),
                            );
                        }
                    }
                }
            } else {
                view = view.child(empty);
            }
            view
        } else if self.tab == CaptureTab::VertexInput {
            let heading = self.t("VERTEX INPUT", "顶点输入", "頂点入力", "정점 입력");
            let note = self.t("Rows follow draw order. Unsupported formats are shown as raw bytes; unavailable attributes use —.", "行按绘制顺序排列；尚未解码的格式显示原始字节，不可用的属性显示 —。", "行は描画順です。未対応形式はバイトで、利用できない属性は — で表示します。", "행은 그리기 순서입니다. 지원하지 않는 형식은 바이트로, 사용할 수 없는 속성은 —로 표시합니다.");
            let mut view = div()
                .flex()
                .flex_col()
                .gap_3()
                .child(div().text_sm().text_color(rgb(0x82aaff)).child(heading))
                .child(note);
            if let Some(page) = &self.input_page {
                view = view
                    .child(format!(
                        "{event_label}: {} · {instances_label}: {}",
                        page.event_id, page.instance
                    ))
                    .child(format!("{offset_label}: {} / {}", page.offset, page.total))
                    .child(self.t(
                        "Select a row to debug its vertex shader (instance 0).",
                        "选择一行以调试其顶点着色器（实例 0）。",
                        "行を選択して頂点シェーダーをデバッグします（インスタンス 0）。",
                        "행을 선택해 정점 셰이더를 디버깅합니다 (인스턴스 0).",
                    ));
                if let Some((vertex, instance, index)) = self.debug_vertex_selection {
                    view = view
                        .child(format!(
                            "{} {vertex} · {} {instance} · {} {index}",
                            self.t("Vertex", "顶点", "頂点", "정점"),
                            self.t("Instance", "实例", "インスタンス", "인스턴스"),
                            self.t("Index", "索引", "インデックス", "인덱스")
                        ))
                        .child(button(
                            "vertex-debug-start",
                            self.t("Debug vertex", "调试顶点", "頂点をデバッグ", "정점 디버깅"),
                            true,
                            |this, cx| this.debug_selected_vertex(cx),
                            cx,
                        ));
                }
            }
            if self
                .debug_view
                .as_ref()
                .is_some_and(|debug| matches!(debug.context, ShaderDebugContext::Vertex { .. }))
            {
                if let Some(panel) = self.shader_debug_panel(cx) {
                    view = view.child(panel);
                }
            }
            view
        } else if self.tab == CaptureTab::Mesh {
            let heading = self.t("MESH OUTPUT", "网格输出", "メッシュ出力", "메시 출력");
            let empty = self.t(
                "Select a draw or mesh event.",
                "选择绘制或网格事件。",
                "描画またはメッシュイベントを選択してください。",
                "그리기 또는 메시 이벤트를 선택하세요.",
            );
            let note = self.t("Positions are shown in buffer order, not draw index order. Other formats are shown as bytes.", "位置按缓冲区顺序显示，非绘制索引顺序；其他格式显示为字节。", "位置は描画インデックス順ではなくバッファ順です。他の形式はバイトで表示します。", "위치는 그리기 인덱스 순서가 아닌 버퍼 순서입니다. 다른 형식은 바이트로 표시합니다.");
            let mut view = div()
                .flex()
                .flex_col()
                .gap_3()
                .child(div().text_sm().text_color(rgb(0x82aaff)).child(heading));
            if let Some(info) = &self.mesh.info {
                let stage = match info.stage {
                    1 => "VSOut",
                    2 => "GSOut",
                    3 => "TaskOut",
                    4 => "MeshOut",
                    _ => "?",
                };
                view = view
                    .child(format!("{event_label}: {} · {stage}", info.event_id))
                    .child(format!(
                        "{instances_label}: {} · View: {}",
                        info.instance, info.view
                    ))
                    .child(format!("{indices_label}: {}", info.num_indices))
                    .child(format!(
                        "{format_label}: {} · {stride_label}: {}",
                        info.format_name, info.vertex_stride
                    ))
                    .child(format!("{topology_label}: {}", info.topology))
                    .child(format!("{offset_label}: {}", self.mesh.offset))
                    .when(!info.status.is_empty(), |view| {
                        view.child(info.status.clone())
                    })
                    .child(note);
            } else {
                view = view.child(empty);
            }
            view
        } else if self.tab == CaptureTab::Parameters {
            let page = self.structure.page.clone();
            let bytes_lines = self
                .structure
                .bytes
                .as_ref()
                .map(|bytes| {
                    bytes
                        .chunks(16)
                        .enumerate()
                        .map(|(index, chunk)| {
                            let hex: String =
                                chunk.iter().map(|byte| format!("{byte:02x} ")).collect();
                            format!(
                                "{:08X}: {hex}",
                                self.structure.bytes_offset + (index as u64) * 16
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let heading = self.t(
                "STRUCTURED DATA",
                "结构化数据",
                "構造化データ",
                "구조화된 데이터",
            );
            let type_label = self.t("Type", "类型", "型", "유형");
            let children_label = self.t("Children", "子项", "子要素", "하위 항목");
            let size_label = self.t("Bytes", "字节", "バイト", "바이트");
            let resource_label = self.t(
                "Resource index",
                "资源索引",
                "リソースインデックス",
                "리소스 인덱스",
            );
            let resource_value_label = self.t("Resource", "资源", "リソース", "리소스");
            let non_null_label = self.t("non-null", "非空", "非 null", "널 아님");
            let null_label = self.t("null", "空", "null", "널");
            let pending_label = self.t(
                "Select an event to inspect its parameters.",
                "选择事件以查看参数。",
                "イベントを選択してパラメーターを表示します。",
                "이벤트를 선택하여 매개변수를 확인하세요.",
            );
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(div().text_sm().text_color(rgb(0x82aaff)).child(heading))
                .when_some(page, |view, page| {
                    let value = structured_value(&page.node);
                    view.child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(page.node.name),
                    )
                    .child(format!("{type_label}: {}", page.node.type_name))
                    .child(format!("{flags_label}: 0x{:08X}", page.node.flags))
                    .child(format!("{size_label}: {}", page.node.byte_size))
                    .child(format!("{children_label}: {}", page.node.child_count))
                    .when_some(page.node.resource_index, |view, index| {
                        view.child(format!("{resource_label}: {index}"))
                    })
                    .when_some(page.node.resource_non_null, |view, non_null| {
                        view.child(format!(
                            "{resource_value_label}: {}",
                            if non_null { non_null_label } else { null_label }
                        ))
                    })
                    .when(!value.is_empty(), |view| view.child(value))
                })
                .when(self.structure.page.is_none(), |view| {
                    view.child(pending_label)
                })
                .children(bytes_lines.into_iter().enumerate().map(|(index, line)| {
                    div()
                        .id(("structure-bytes-line", index))
                        .test_support()
                        .text_sm()
                        .child(line)
                }))
        } else if self.tab == CaptureTab::Shaders {
            let report = self.shader_data.clone();
            let target_button = button(
                "shader-next-target",
                next_target,
                false,
                |this, cx| {
                    if let (Some(stage), Some(report)) =
                        (this.selected_shader, this.shader_data.as_ref())
                    {
                        if report.targets.is_empty() {
                            return;
                        }
                        let target = if report.target.is_empty() {
                            Some(0)
                        } else {
                            report
                                .targets
                                .iter()
                                .position(|target| *target == report.target)
                                .and_then(|index| u32::try_from(index + 1).ok())
                                .filter(|index| (*index as usize) < report.targets.len())
                        };
                        this.read_shader(stage, target, cx);
                    }
                },
                cx,
            );
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(0x82aaff))
                        .child(shader_details),
                )
                .when_some(report, |view, shader| {
                    let inputs = shader
                        .inputs
                        .iter()
                        .map(|parameter| {
                            format!(
                                "{} · r{} · {}",
                                if parameter.name.is_empty() {
                                    &parameter.semantic
                                } else {
                                    &parameter.name
                                },
                                parameter.register,
                                parameter.components
                            )
                        })
                        .collect::<Vec<_>>();
                    let outputs = shader
                        .outputs
                        .iter()
                        .map(|parameter| {
                            format!(
                                "{} · r{} · {}",
                                if parameter.name.is_empty() {
                                    &parameter.semantic
                                } else {
                                    &parameter.name
                                },
                                parameter.register,
                                parameter.components
                            )
                        })
                        .collect::<Vec<_>>();
                    view.child(format!("{entry_label}: {}", shader.entry_point))
                        .child(format!(
                            "{target_label}: {}",
                            if shader.target.is_empty() {
                                default_label
                            } else {
                                &shader.target
                            }
                        ))
                        .child(target_button)
                        .child(format!("{constants_label}: {}", shader.constant_blocks))
                        .child(format!("{shader_inputs_label}: {}", inputs.len()))
                        .children(inputs.into_iter().map(|line| div().text_sm().child(line)))
                        .child(format!("{shader_outputs_label}: {}", outputs.len()))
                        .children(outputs.into_iter().map(|line| div().text_sm().child(line)))
                        .child(format!("{disasm_label} · {}", shader.encoding))
                        .child(
                            div()
                                .id("shader-disassembly")
                                .test_support()
                                .text_sm()
                                .child(shader.disassembly),
                        )
                })
                .when(self.shader_data.is_none(), |view| {
                    view.child(div().text_color(rgb(0x9eacc0)).child(shader_empty))
                })
        } else if self.tab == CaptureTab::Buffers {
            let rows = self.buffer_data.as_ref().map(|data| {
                data.chunks(8)
                    .enumerate()
                    .map(|(line, bytes)| {
                        let hex: String = bytes.iter().map(|byte| format!("{byte:02x} ")).collect();
                        format!("{:016x}  {hex}", self.buffer_offset + (line as u64 * 8))
                    })
                    .collect::<Vec<_>>()
            });
            let prev = button(
                "buffer-prev",
                previous,
                false,
                |this, cx| {
                    if let Some(index) = this.selected_buffer {
                        this.read_buffer(
                            index,
                            this.buffer_offset.saturating_sub(BUFFER_VIEW_PAGE),
                            cx,
                        );
                    }
                },
                cx,
            );
            let next_button = button(
                "buffer-next",
                next,
                false,
                |this, cx| {
                    if let Some(index) = this.selected_buffer {
                        let size = this
                            .snapshot
                            .as_ref()
                            .and_then(|s| s.buffers.get(index))
                            .and_then(|b| b.length.parse::<u64>().ok())
                            .unwrap_or(0);
                        let offset = this.buffer_offset.saturating_add(BUFFER_VIEW_PAGE);
                        if offset < size {
                            this.read_buffer(index, offset, cx);
                        }
                    }
                },
                cx,
            );
            let read_button = button(
                "buffer-read",
                read,
                true,
                |this, cx| {
                    if let Some(index) = this.selected_buffer {
                        this.read_buffer(index, this.buffer_offset, cx);
                    }
                },
                cx,
            );
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(0x82aaff))
                        .child(buffer_details),
                )
                .when_some(buffer_detail, |view, buffer| {
                    view.child(div().font_weight(gpui::FontWeight::BOLD).child(buffer.name))
                        .child(format!("{bytes_label}: {}", buffer.length))
                        .child(format!("{flags_label}: 0x{:08X}", buffer.creation_flags))
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_2()
                                .child(prev)
                                .child(read_button)
                                .child(next_button),
                        )
                        .children(rows.unwrap_or_default().into_iter().enumerate().map(
                            |(index, line)| {
                                div()
                                    .id(("buffer-line", index))
                                    .test_support()
                                    .text_sm()
                                    .child(line)
                            },
                        ))
                })
                .when(self.selected_buffer.is_none(), |view| {
                    view.child(div().text_color(rgb(0x9eacc0)).child(buffer_empty))
                })
        } else if self.tab == CaptureTab::Textures {
            let selection = self.texture_selection;
            let mip_button = button(
                "texture-next-mip",
                next_mip,
                false,
                |this, cx| this.advance_texture(TextureField::Mip, cx),
                cx,
            );
            let slice_button = button(
                "texture-next-slice",
                next_slice,
                false,
                |this, cx| this.advance_texture(TextureField::Slice, cx),
                cx,
            );
            let sample_button = button(
                "texture-next-sample",
                next_sample,
                false,
                |this, cx| this.advance_texture(TextureField::Sample, cx),
                cx,
            );
            let channel_button = button(
                "texture-next-channel",
                next_channel,
                false,
                |this, cx| this.advance_texture(TextureField::Channel, cx),
                cx,
            );
            let export = button(
                "texture-export",
                export_label,
                true,
                |this, cx| this.export_texture(cx),
                cx,
            );
            let preview = button(
                "texture-preview",
                preview_label,
                false,
                |this, cx| this.preview_texture(cx),
                cx,
            );
            let pixel = self.pixel.clone();
            let history = self.pixel_history.clone();
            let history_selected = self.history_selected;
            let pixel_debug_panel = if self
                .debug_view
                .as_ref()
                .is_some_and(|debug| matches!(debug.context, ShaderDebugContext::Pixel(_)))
            {
                self.shader_debug_panel(cx)
            } else {
                None
            };
            let pixel_title = self.t(
                "Pixel coordinates (top-left)",
                "像素坐标（左上角为原点）",
                "ピクセル座標（左上原点）",
                "픽셀 좌표 (왼쪽 위 원점)",
            );
            let pick = self.t("Pick pixel", "读取像素", "ピクセルを取得", "픽셀 읽기");
            let history_label = self.t("Pixel history", "像素历史", "ピクセル履歴", "픽셀 기록");
            let history_empty = self.t(
                "No modifications were returned through this event.",
                "截至当前事件未返回修改记录。",
                "このイベントまでの変更履歴はありません。",
                "이 이벤트까지 반환된 수정 기록이 없습니다.",
            );
            let history_note = self.t(
                "An empty result can mean the backend could not inspect this pixel; RenderDoc excludes CPU writes and resource discards.",
                "空结果也可能表示后端无法检查此像素；RenderDoc 不包含 CPU 写入和资源丢弃。",
                "空の結果は検査できなかった場合もあります。RenderDoc は CPU 書き込みとリソース破棄を含めません。",
                "빈 결과는 픽셀을 검사할 수 없다는 뜻일 수도 있습니다. RenderDoc은 CPU 쓰기와 리소스 폐기를 제외합니다.",
            );
            let history_sample_note = self.t("For multisampling, choose a sample explicitly; backend default handling varies.", "多重采样请明确选择样本；后端默认处理可能不同。", "マルチサンプルの場合はサンプルを明示してください。既定の処理はバックエンドにより異なります。", "멀티샘플링에서는 샘플을 명시적으로 선택하세요. 기본 처리는 백엔드마다 다를 수 있습니다.");
            let backend_default = self.t(
                "backend default",
                "后端默认",
                "バックエンド既定",
                "백엔드 기본값",
            );
            let fragment_label = self.t("Fragment", "片元", "フラグメント", "프래그먼트");
            let primitive_label = self.t("Primitive", "图元", "プリミティブ", "프리미티브");
            let passed_label = self.t("Passed", "通过", "通過", "통과");
            let failed_label = self.t("Failed", "未通过", "失敗", "실패");
            let before_label = self.t("Before", "修改前", "変更前", "변경 전");
            let shader_output_label = self.t(
                "Shader output",
                "着色器输出",
                "シェーダー出力",
                "셰이더 출력",
            );
            let after_label = self.t("After", "修改后", "変更後", "변경 후");
            let jump_label = self.t(
                "Go to event",
                "跳转到事件",
                "イベントへ移動",
                "이벤트로 이동",
            );
            let unavailable_history = self.t("Unavailable", "不可用", "利用不可", "사용할 수 없음");
            let direct_write_label = self.t(
                "Direct shader write",
                "着色器直接写入",
                "シェーダー直接書き込み",
                "셰이더 직접 쓰기",
            );
            let unbound_shader_label = self.t(
                "Pixel shader unbound",
                "像素着色器未绑定",
                "ピクセルシェーダー未バインド",
                "픽셀 셰이더 바인딩 안 됨",
            );
            let stencil_label = self.t("Stencil", "模板", "ステンシル", "스텐실");
            let float_label = self.t(
                "Float interpretation",
                "浮点解释",
                "浮動小数点として解釈",
                "부동소수점 해석",
            );
            let uint_label = self.t(
                "Unsigned interpretation",
                "无符号整数解释",
                "符号なし整数として解釈",
                "부호 없는 정수 해석",
            );
            let int_label = self.t(
                "Signed interpretation",
                "有符号整数解释",
                "符号付き整数として解釈",
                "부호 있는 정수 해석",
            );
            let statistics = self.statistics.clone();
            let histogram_label = self.t(
                "Histogram range",
                "直方图范围",
                "ヒストグラム範囲",
                "히스토그램 범위",
            );
            let analyze_label = self.t("Analyze", "分析", "解析", "분석");
            let minimum_label = self.t("Minimum", "最小值", "最小値", "최솟값");
            let maximum_label = self.t("Maximum", "最大值", "最大値", "최댓값");
            let no_histogram = self.t(
                "Histogram unavailable or empty.",
                "直方图不可用或为空。",
                "ヒストグラムが利用できないか空です。",
                "히스토그램을 사용할 수 없거나 비어 있습니다.",
            );
            let stats_note = self.t(
                "RenderDoc does not report a separate success status for minimum/maximum.",
                "RenderDoc 不提供最小值／最大值的单独成功状态。",
                "RenderDoc は最小値・最大値の個別の成功状態を返しません。",
                "RenderDoc은 최솟값과 최댓값의 별도 성공 상태를 반환하지 않습니다.",
            );
            let channels_note = self.t(
                "The histogram uses the selected channel; minimum and maximum cover RGBA.",
                "直方图使用选中通道，最小值和最大值覆盖 RGBA。",
                "ヒストグラムは選択チャンネルを使用し、最小値と最大値は RGBA を対象にします。",
                "히스토그램은 선택한 채널을 사용하며 최솟값과 최댓값은 RGBA를 포함합니다.",
            );
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(0x82aaff))
                        .child(texture_details),
                )
                .when_some(texture_detail, |view, texture| {
                    view.child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(texture.name),
                    )
                    .child(format!(
                        "{} × {} × {}",
                        texture.width, texture.height, texture.depth
                    ))
                    .child(format!(
                        "Mip: {}  ·  {array_label}: {}  ·  {samples_label}: {}",
                        texture.mips, texture.array_size, texture.samples
                    ))
                    .child(format!("{bytes_label}: {}", texture.bytes))
                    .child(format!(
                        "Mip: {} · {slice_label}: {}",
                        selection.mip, selection.slice
                    ))
                    .child(format!(
                        "{samples_label}: {} · {channel_label}: {}",
                        selection
                            .sample
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| resolve_label.to_owned()),
                        selection
                            .channel
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| all_label.to_owned())
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(mip_button)
                            .child(slice_button)
                            .child(sample_button)
                            .child(channel_button),
                    )
                    .child(format!(
                        "{format_label}: {}  ·  {component_label}: {} / {} × {}",
                        if texture.format_name.is_empty() {
                            texture.format_type.to_string()
                        } else {
                            texture.format_name
                        },
                        texture.component_type,
                        texture.component_count,
                        texture.component_byte_width
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(preview)
                            .child(export),
                    )
                    .child(div().text_sm().text_color(rgb(0x91a7c0)).child(pixel_title))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(div().child("X"))
                            .child(
                                Input::new(&self.pixel_x)
                                    .id("pixel-x")
                                    .aria_label("X")
                                    .w(px(75.)),
                            )
                            .child(div().child("Y"))
                            .child(
                                Input::new(&self.pixel_y)
                                    .id("pixel-y")
                                    .aria_label("Y")
                                    .w(px(75.)),
                            )
                            .child(button(
                                "pixel-pick",
                                pick,
                                false,
                                |this, cx| this.pick_texture_pixel(cx),
                                cx,
                            ))
                            .child(button("pixel-history", history_label, false, |this, cx| this.read_pixel_history(0, false, cx), cx))
                            .child(button("pixel-debug-start", self.t("Debug pixel", "调试像素", "ピクセルをデバッグ", "픽셀 디버깅"), true, |this, cx| this.debug_selected_pixel(cx), cx)),
                    )
                    .child(div().text_sm().text_color(rgb(0x91a7c0)).child(self.t("Pixel debugging uses the selected draw and sample; RenderDoc chooses the primitive and view. The selected texture need not be bound to that draw.", "像素调试使用当前绘制事件和采样；图元及视图由 RenderDoc 选择。所选纹理不一定绑定到该绘制。", "ピクセルデバッグは選択中の描画とサンプルを使用し、プリミティブとビューは RenderDoc が選択します。選択したテクスチャが描画にバインドされているとは限りません。", "픽셀 디버깅은 선택한 그리기와 샘플을 사용하며 프리미티브와 뷰는 RenderDoc이 선택합니다. 선택한 텍스처가 해당 그리기에 바인딩되어 있지 않을 수 있습니다.")))
                    .when_some(pixel_debug_panel, |view, panel| view.child(panel))
                    .when_some(pixel, |view, pixel| {
                        let bits = pixel
                            .bits
                            .iter()
                            .map(|v| format!("{v:08X}"))
                            .collect::<Vec<_>>()
                            .join(" ");
                        view.child(
                            div()
                                .id("pixel-result")
                                .test_support()
                                .child(format!("({}, {}) · RGBA: {bits}", pixel.x, pixel.y)),
                        )
                        .child(format!("{float_label}: {}", pixel.float_values.join(", ")))
                        .child(format!("{uint_label}: {:?}", pixel.uint_values))
                        .child(format!("{int_label}: {:?}", pixel.int_values))
                    })
                    .when_some(history, |view, page| {
                        let history_sample = if page.sample == u32::MAX { backend_default.to_owned() } else { page.sample.to_string() };
                        let mut view = view.child(div().id("pixel-history-result").test_support().text_color(rgb(0x82aaff))
                            .child(format!("{history_label} · {event_label} {} · ({}, {}) · {samples_label}: {history_sample} · {} / {}", page.event_id, page.x, page.y, page.offset, page.total)))
                            .child(div().text_sm().text_color(rgb(0x91a7c0)).child(history_note));
                        if page.sample == u32::MAX { view = view.child(div().text_sm().text_color(rgb(0x91a7c0)).child(history_sample_note)); }
                        if page.total == "0" { view = view.child(history_empty); }
                        else {
                            view = view.child(div().flex().gap_2()
                                .child(button("history-previous", previous_label, false, |this, cx| {
                                    if let Some(page) = &this.pixel_history { if let Ok(offset) = page.offset.parse::<u64>() { if offset > 0 { this.read_pixel_history(offset.saturating_sub(u64::from(PIXEL_HISTORY_PAGE_LIMIT)), true, cx); } } }
                                }, cx))
                                .child(button("history-next", next_label, false, |this, cx| {
                                    if let Some(page) = &this.pixel_history {
                                        if let (Ok(offset), Ok(total)) = (page.offset.parse::<u64>(), page.total.parse::<u64>()) {
                                            if let Some(next) = offset.checked_add(page.entries.len() as u64) { if next < total { this.read_pixel_history(next, true, cx); } }
                                        }
                                    }
                                }, cx)));
                            for (row, entry) in page.entries.iter().enumerate() {
                                view = view.child(div().id(("pixel-history-row", row)).test_support().cursor_pointer().px_2().py_1()
                                    .bg(rgb(if history_selected == Some(row) { 0x304c7c } else { 0x192332 }))
                                    .child(format!("{event_label} {} · {fragment_label} {} · {}", entry.event_id, entry.fragment, if entry.passed { passed_label } else { failed_label }))
                                    .on_click(cx.listener(move |this, _, _, cx| { this.history_selected = Some(row); cx.notify(); })));
                            }
                            if let Some(entry) = history_selected.and_then(|index| page.entries.get(index)) {
                                let mut failures = Vec::new();
                                let tests = &entry.tests;
                                for (failed, label) in [
                                    (tests.sample_masked, self.t("Sample mask", "采样掩码", "サンプルマスク", "샘플 마스크")),
                                    (tests.backface_culled, self.t("Backface culling", "背面剔除", "背面カリング", "후면 컬링")),
                                    (tests.depth_clipped, self.t("Depth clip", "深度裁剪", "深度クリップ", "깊이 클립")),
                                    (tests.depth_bounds_failed, self.t("Depth bounds", "深度范围", "深度範囲", "깊이 범위")),
                                    (tests.view_clipped, self.t("Viewport clip", "视口裁剪", "ビューポートクリップ", "뷰포트 클립")),
                                    (tests.scissor_clipped, self.t("Scissor clip", "裁剪矩形", "シザークリップ", "시저 클립")),
                                    (tests.shader_discarded, self.t("Shader discard", "着色器丢弃", "シェーダー破棄", "셰이더 폐기")),
                                    (tests.depth_test_failed, self.t("Depth test", "深度测试", "深度テスト", "깊이 테스트")),
                                    (tests.stencil_test_failed, self.t("Stencil test", "模板测试", "ステンシルテスト", "스텐실 테스트")),
                                    (tests.predication_skipped, self.t("Predication", "谓词跳过", "プレディケーション", "조건부 렌더링")),
                                ] { if failed { failures.push(label); } }
                                let event = entry.event_id;
                                let primitive = if entry.primitive == u32::MAX { unavailable_history.to_owned() } else { entry.primitive.to_string() };
                                view = view.child(div().id("pixel-history-detail").test_support().child(format!("{event_label} {event} · {primitive_label} {primitive} · {}", if failures.is_empty() { passed_label.to_owned() } else { failures.join(", ") })))
                                    .child(div().id("pixel-history-jump").test_support().text_color(rgb(0x82aaff)).cursor_pointer().child(jump_label)
                                        .on_click(cx.listener(move |this, _, window, cx| this.jump_to_usage(event, window, cx))));
                                if entry.event_id == page.event_id {
                                    let selection = PixelDebugSelection { x: page.x, y: page.y, sample: (page.sample != u32::MAX).then_some(page.sample), primitive: (entry.primitive != u32::MAX).then_some(entry.primitive), view: None };
                                    view = view.child(button("pixel-history-debug", self.t("Debug this fragment", "调试此片元", "このフラグメントをデバッグ", "이 프래그먼트 디버깅"), true,
                                        move |this, cx| this.start_shader_debug(ShaderDebugContext::Pixel(selection), cx), cx));
                                }
                                if entry.direct_shader_write { view = view.child(direct_write_label); }
                                if entry.unbound_pixel_shader { view = view.child(unbound_shader_label); }
                                for (label, value) in [(before_label, &entry.pre), (shader_output_label, &entry.shader_output), (after_label, &entry.post)] {
                                    if let Some(value) = value {
                                        let color = value.color.as_ref().map(|color| color.float_values.join(", ")).unwrap_or_default();
                                        view = view.child(format!("{label} · {float_label}: {color} · {depth_label}: {} · {stencil_label}: {}", value.depth.as_deref().unwrap_or("—"), value.stencil))
                                            .child(format!("{uint_label}: {:?} · {int_label}: {:?}", value.color_bits, value.color_bits.map(|bits| bits as i32)));
                                    } else { view = view.child(format!("{label}: {unavailable_history}")); }
                                }
                            }
                        }
                        view
                    })
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x91a7c0))
                            .child(histogram_label),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x91a7c0))
                            .child(channels_note),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(div().text_sm().child(minimum_label))
                            .child(
                                Input::new(&self.stats_min)
                                    .id("histogram-min")
                                    .aria_label(minimum_label)
                                    .w(px(80.)),
                            )
                            .child(div().text_sm().child(maximum_label))
                            .child(
                                Input::new(&self.stats_max)
                                    .id("histogram-max")
                                    .aria_label(maximum_label)
                                    .w(px(80.)),
                            )
                            .child(button(
                                "histogram-analyze",
                                analyze_label,
                                false,
                                |this, cx| this.compute_texture_statistics(cx),
                                cx,
                            )),
                    )
                    .when_some(statistics, |view, stats| {
                        let highest = stats.histogram.iter().copied().max().unwrap_or(0);
                        let channels = ["R", "G", "B", "A"]
                            .into_iter()
                            .zip(stats.request.channels)
                            .filter_map(|(name, enabled)| enabled.then_some(name))
                            .collect::<Vec<_>>()
                            .join(" ");
                        let mut view = view
                            .child(div().id("texture-statistics").test_support().child(format!(
                                "{event_label} {} · {} — {} · {channel_label}: {channels}",
                                stats.event_id, stats.request.min, stats.request.max
                            )))
                            .child(format!(
                                "{minimum_label} · {float_label}: {}",
                                stats.min.float_values.join(", ")
                            ))
                            .child(format!(
                                "{maximum_label} · {float_label}: {}",
                                stats.max.float_values.join(", ")
                            ))
                            .child(format!(
                                "{minimum_label} · {uint_label}: {:?}",
                                stats.min.uint_values
                            ))
                            .child(format!(
                                "{maximum_label} · {uint_label}: {:?}",
                                stats.max.uint_values
                            ))
                            .child(format!(
                                "{minimum_label} · {int_label}: {:?}",
                                stats.min.int_values
                            ))
                            .child(format!(
                                "{maximum_label} · {int_label}: {:?}",
                                stats.max.int_values
                            ))
                            .child(div().text_sm().text_color(rgb(0x91a7c0)).child(stats_note));
                        if stats.histogram.is_empty() {
                            view = view.child(no_histogram);
                        } else {
                            view = view.child(
                                div()
                                    .id("texture-histogram")
                                    .test_support()
                                    .h(px(80.))
                                    .w_full()
                                    .overflow_hidden()
                                    .flex()
                                    .items_end()
                                    .children(stats.histogram.iter().map(|count| {
                                        let height = if highest == 0 {
                                            0.0
                                        } else {
                                            80.0 * (*count as f32 / highest as f32)
                                        };
                                        div().flex_1().min_w_0().h(px(height)).bg(rgb(0x82aaff))
                                    })),
                            );
                        }
                        view
                    })
                    .child(div().text_sm().text_color(rgb(0x91a7c0)).child(export_note))
                    .when_some(preview_path, |view, path| {
                        view.child(
                            div()
                                .id("texture-preview-image")
                                .test_support()
                                .w_full()
                                .h(px(180.))
                                .child(img(path).size_full().object_fit(gpui::ObjectFit::Contain)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(0x91a7c0))
                                .child(preview_note),
                        )
                    })
                })
                .when(self.selected_texture.is_none(), |view| {
                    view.child(div().text_color(rgb(0x9eacc0)).child(texture_empty))
                })
        } else {
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(div().text_sm().text_color(rgb(0x82aaff)).child(details))
                .when_some(detail, |view, action| {
                    view.child(div().font_weight(gpui::FontWeight::BOLD).child(action.name))
                        .child(format!("{event_label}: {}", action.event_id))
                        .when_some(selected_event, |view, event| {
                            view.child(format!("{selected_event_label}: {event}"))
                        })
                        .child(format!("{action_label}: {}", action.action_id))
                        .child(format!("{flags_label}: 0x{:08X}", action.flags))
                        .child(format!("{indices_label}: {}", action.indices))
                        .child(format!("{instances_label}: {}", action.instances))
                })
                .when_some(pipeline, |view, state| {
                    if !state.available {
                        return view.child(
                            div()
                                .text_sm()
                                .text_color(rgb(0x82aaff))
                                .child(pipeline_unavailable),
                        );
                    }
                    let names = [
                        "Unknown",
                        "PointList",
                        "LineList",
                        "LineStrip",
                        "LineLoop",
                        "TriangleList",
                        "TriangleStrip",
                        "TriangleFan",
                        "LineList_Adj",
                        "LineStrip_Adj",
                        "TriangleList_Adj",
                        "TriangleStrip_Adj",
                    ];
                    let topology = names
                        .get(state.topology as usize)
                        .map(|name| name.to_string())
                        .unwrap_or_else(|| format!("{}", state.topology));
                    let shaders = state
                        .shaders
                        .iter()
                        .filter(|shader| shader.resource.bound)
                        .map(|shader| {
                            format!(
                                "{}: {}",
                                shader.stage,
                                if shader.resource.name.is_empty() {
                                    "—"
                                } else {
                                    &shader.resource.name
                                }
                            )
                        })
                        .collect::<Vec<_>>();
                    let outputs = state
                        .outputs
                        .iter()
                        .enumerate()
                        .filter(|(_, output)| output.bound)
                        .map(|(index, output)| {
                            format!(
                                "{}: {}",
                                index,
                                if output.name.is_empty() {
                                    "—"
                                } else {
                                    &output.name
                                }
                            )
                        })
                        .collect::<Vec<_>>();
                    let index = state
                        .index_buffer
                        .as_ref()
                        .map(|buffer| {
                            if buffer.resource.bound {
                                format!(
                                    "{} · {offset_label} {} · {stride_label} {}",
                                    buffer.resource.name, buffer.byte_offset, buffer.byte_stride
                                )
                            } else {
                                "—".into()
                            }
                        })
                        .unwrap_or_else(|| unavailable.to_owned());
                    let vertex_buffers = state.vertex_buffers.as_ref().map(|buffers| {
                        buffers
                            .iter()
                            .enumerate()
                            .filter(|(_, buffer)| buffer.resource.bound)
                            .map(|(slot, buffer)| {
                                format!(
                                    "{slot}: {} · {offset_label} {} · {stride_label} {}",
                                    buffer.resource.name, buffer.byte_offset, buffer.byte_stride
                                )
                            })
                            .collect::<Vec<_>>()
                    });
                    let inputs = state.vertex_inputs.as_ref().map(|inputs| {
                        inputs
                            .iter()
                            .map(|input| {
                                format!(
                                    "{} · {} · {} + {}",
                                    input.name,
                                    input.format,
                                    input.vertex_buffer,
                                    input.byte_offset
                                )
                            })
                            .collect::<Vec<_>>()
                    });
                    let viewports = state.viewports.as_ref().map(|viewports| {
                        viewports
                            .iter()
                            .enumerate()
                            .map(|(slot, viewport)| {
                                let value = |number: Option<f32>| {
                                    number
                                        .map(|v| v.to_string())
                                        .unwrap_or_else(|| "—".to_owned())
                                };
                                format!(
                                    "{slot}: ({}, {}) {} × {} · {} · {depth_label} {}–{}",
                                    value(viewport.x),
                                    value(viewport.y),
                                    value(viewport.width),
                                    value(viewport.height),
                                    if viewport.enabled {
                                        enabled_label
                                    } else {
                                        disabled_label
                                    },
                                    value(viewport.min_depth),
                                    value(viewport.max_depth)
                                )
                            })
                            .collect::<Vec<_>>()
                    });
                    view.child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x82aaff))
                            .child(pipeline_label),
                    )
                    .child(format!("{topology_label}: {topology}"))
                    .child(format!("{shaders_label}: {}", shaders.len()))
                    .children(shaders.into_iter().map(|line| div().text_sm().child(line)))
                    .child(format!("{outputs_label}: {}", outputs.len()))
                    .children(outputs.into_iter().map(|line| div().text_sm().child(line)))
                    .child(format!("{index_buffer_label}: {index}"))
                    .child(format!(
                        "{vertex_buffers_label}: {}",
                        vertex_buffers.as_ref().map_or_else(
                            || unavailable.to_owned(),
                            |values| values.len().to_string()
                        )
                    ))
                    .children(
                        vertex_buffers
                            .unwrap_or_default()
                            .into_iter()
                            .map(|line| div().text_sm().child(line)),
                    )
                    .child(format!(
                        "{inputs_label}: {}",
                        inputs.as_ref().map_or_else(
                            || unavailable.to_owned(),
                            |values| values.len().to_string()
                        )
                    ))
                    .children(
                        inputs
                            .unwrap_or_default()
                            .into_iter()
                            .map(|line| div().text_sm().child(line)),
                    )
                    .child(format!(
                        "{viewport_label}: {}",
                        viewports.as_ref().map_or_else(
                            || unavailable.to_owned(),
                            |values| values.len().to_string()
                        )
                    ))
                    .children(
                        viewports.unwrap_or_default().into_iter().enumerate().map(
                            |(slot, line)| {
                                div()
                                    .id(("pipeline-viewport", slot))
                                    .test_support()
                                    .text_sm()
                                    .child(line)
                            },
                        ),
                    )
                })
                .when(self.selected.is_none(), |view| {
                    view.child(div().text_color(rgb(0x9eacc0)).child(empty))
                })
        };

        div()
            .size_full()
            .bg(rgb(0x101722))
            .text_color(rgb(0xe6edf6))
            .flex()
            .flex_col()
            .gap_4()
            .p_5()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(button(
                        "capture-back",
                        back,
                        false,
                        |this, cx| {
                            this.open_revision += 1;
                            this.session = None;
                            this.snapshot = None;
                            this.event_tree.reset(&[]);
                            this.capture = None;
                            this.selected = None;
                            this.replay_pending = false;
                            this.export_pending = false;
                            this.resource_pending = false;
                            this.buffer_data = None;
                            this.shader_data = None;
                            this.structure = StructureView::default();
                            this.mesh = MeshView::default();
                            this.input_page = None;
                            this.debug_vertex_selection = None;
                            this.debug_view = None;
                            this.selected_resource = None;
                            this.usage_page = None;
                            this.preview = None;
                            this.pixel = None;
                            this.pixel_history = None;
                            this.history_selected = None;
                            this.statistics = None;
                            this.status = None;
                            cx.notify();
                        },
                        cx,
                    ))
                    .child(button(
                        "capture-open",
                        open,
                        true,
                        |this, cx| {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("RenderDoc", &["rdc", "cap"])
                                .pick_file()
                            {
                                this.open(path, cx);
                            }
                        },
                        cx,
                    ))
                    .child(button(
                        "capture-upstream",
                        upstream,
                        false,
                        |this, cx| {
                            if let Some(path) = this.capture.as_ref() {
                                let result = backend::open_capture(&this.settings, path);
                                this.result(result, cx);
                            }
                        },
                        cx,
                    )),
            )
            .child(div().text_sm().text_color(rgb(0x9eacc0)).child(title))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(div().text_xl().child(frame))
                    .child(div().text_sm().text_color(rgb(0x9eacc0)).child(counts)),
            )
            .when_some(status, |view, text| {
                view.child(div().text_color(rgb(0xd4b77c)).child(text))
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .bg(rgb(0x192332))
                            .rounded_lg()
                            .overflow_hidden()
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap_2()
                                    .px_3()
                                    .py_2()
                                    .child(button(
                                        "events-tab",
                                        events,
                                        self.tab == CaptureTab::Events,
                                        |this, cx| {
                                            this.tab = CaptureTab::Events;
                                            cx.notify();
                                        },
                                        cx,
                                    ))
                                    .child(button(
                                        "api-calls-tab",
                                        api_calls,
                                        self.tab == CaptureTab::ApiCalls,
                                        |this, cx| {
                                            this.tab = CaptureTab::ApiCalls;
                                            cx.notify();
                                        },
                                        cx,
                                    ))
                                    .child(button(
                                        "parameters-tab",
                                        parameters_label,
                                        self.tab == CaptureTab::Parameters,
                                        |this, cx| {
                                            this.tab = CaptureTab::Parameters;
                                            if this.structure.page.is_none() {
                                                this.read_structure(Vec::new(), 0, cx);
                                            }
                                            cx.notify();
                                        },
                                        cx,
                                    ))
                                    .child(button(
                                        "textures-tab",
                                        textures_label,
                                        self.tab == CaptureTab::Textures,
                                        |this, cx| {
                                            this.tab = CaptureTab::Textures;
                                            cx.notify();
                                        },
                                        cx,
                                    ))
                                    .child(button(
                                        "buffers-tab",
                                        buffers_label,
                                        self.tab == CaptureTab::Buffers,
                                        |this, cx| {
                                            this.tab = CaptureTab::Buffers;
                                            cx.notify();
                                        },
                                        cx,
                                    ))
                                    .child(button(
                                        "shaders-tab",
                                        shaders_tab_label,
                                        self.tab == CaptureTab::Shaders,
                                        |this, cx| {
                                            this.tab = CaptureTab::Shaders;
                                            cx.notify();
                                        },
                                        cx,
                                    ))
                                    .child(button(
                                        "mesh-tab",
                                        self.t("Mesh", "网格", "メッシュ", "메시"),
                                        self.tab == CaptureTab::Mesh,
                                        |this, cx| {
                                            this.tab = CaptureTab::Mesh;
                                            if this.mesh.info.is_none() {
                                                this.read_mesh(this.mesh.selection, 0, cx);
                                            }
                                            cx.notify();
                                        },
                                        cx,
                                    ))
                                    .child(button(
                                        "vertex-input-tab",
                                        self.t("Vertex input", "顶点输入", "頂点入力", "정점 입력"),
                                        self.tab == CaptureTab::VertexInput,
                                        |this, cx| {
                                            this.tab = CaptureTab::VertexInput;
                                            if this.input_page.is_none() {
                                                this.read_vertex_input(0, cx);
                                            }
                                            cx.notify();
                                        },
                                        cx,
                                    ))
                                    .child(button(
                                        "resources-tab",
                                        self.t("Resources", "资源", "リソース", "리소스"),
                                        self.tab == CaptureTab::Resources,
                                        |this, cx| {
                                            this.tab = CaptureTab::Resources;
                                            this.refresh_resources();
                                            cx.notify();
                                        },
                                        cx,
                                    ))
                                    .child(
                                        div()
                                            .self_center()
                                            .text_sm()
                                            .text_color(rgb(0x91a7c0))
                                            .child(count.to_string()),
                                    ),
                            )
                            .when(self.tab == CaptureTab::Resources, |view| {
                                let label = self.t(
                                    "Filter resources by name, type or #index",
                                    "按名称、类型或 #序号筛选资源",
                                    "名前・種類・#番号でリソースを絞り込み",
                                    "이름, 유형 또는 #번호로 리소스 필터링",
                                );
                                view.child(
                                    div()
                                        .px_3()
                                        .pb_2()
                                        .child(
                                            div().text_sm().text_color(rgb(0x9eacc0)).child(label),
                                        )
                                        .child(
                                            Input::new(&self.resource_search)
                                                .id("resource-filter")
                                                .aria_label(label)
                                                .w_full(),
                                        ),
                                )
                            })
                            .when(self.tab == CaptureTab::VertexInput, |view| {
                                view.child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .px_3()
                                        .pb_2()
                                        .child(button(
                                            "vertex-input-previous",
                                            previous_label,
                                            false,
                                            |this, cx| {
                                                if let Some(page) = &this.input_page {
                                                    if page.offset > 0 {
                                                        this.read_vertex_input(
                                                            page.offset
                                                                .saturating_sub(INPUT_PAGE_LIMIT),
                                                            cx,
                                                        );
                                                    }
                                                }
                                            },
                                            cx,
                                        ))
                                        .child(button(
                                            "vertex-input-next",
                                            next_label,
                                            false,
                                            |this, cx| {
                                                if let Some(page) = &this.input_page {
                                                    if let Some(next) = page
                                                        .offset
                                                        .checked_add(page.rows.len() as u32)
                                                    {
                                                        if next < page.total {
                                                            this.read_vertex_input(next, cx);
                                                        }
                                                    }
                                                }
                                            },
                                            cx,
                                        )),
                                )
                            })
                            .when(self.tab == CaptureTab::Mesh, |view| {
                                view.child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .gap_2()
                                        .px_3()
                                        .pb_2()
                                        .child(button(
                                            "mesh-stage",
                                            self.t(
                                                "Next stage",
                                                "切换阶段",
                                                "ステージ切替",
                                                "단계 전환",
                                            ),
                                            false,
                                            |this, cx| {
                                                let mut selection = this.mesh.selection;
                                                selection.stage = selection.stage % 4 + 1;
                                                this.read_mesh(selection, 0, cx);
                                            },
                                            cx,
                                        ))
                                        .child(button(
                                            "mesh-previous",
                                            previous_bytes,
                                            false,
                                            |this, cx| {
                                                if this.mesh.offset > 0 {
                                                    let step = this
                                                        .mesh
                                                        .info
                                                        .as_ref()
                                                        .map(mesh_page_size)
                                                        .unwrap_or(BUFFER_VIEW_PAGE);
                                                    this.read_mesh(
                                                        this.mesh.selection,
                                                        this.mesh.offset.saturating_sub(step),
                                                        cx,
                                                    );
                                                }
                                            },
                                            cx,
                                        ))
                                        .child(button(
                                            "mesh-next",
                                            next_bytes,
                                            false,
                                            |this, cx| {
                                                let step = this
                                                    .mesh
                                                    .info
                                                    .as_ref()
                                                    .map(mesh_page_size)
                                                    .unwrap_or(BUFFER_VIEW_PAGE);
                                                if this.mesh.bytes.as_ref().is_some_and(|bytes| {
                                                    bytes.len() == step as usize
                                                }) {
                                                    if let Some(offset) =
                                                        this.mesh.offset.checked_add(step)
                                                    {
                                                        this.read_mesh(
                                                            this.mesh.selection,
                                                            offset,
                                                            cx,
                                                        );
                                                    }
                                                }
                                            },
                                            cx,
                                        )),
                                )
                            })
                            .when(self.tab == CaptureTab::Events, |view| {
                                view.child(
                                    div()
                                        .px_3()
                                        .pb_2()
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(0x9eacc0))
                                                .child(event_filter_label),
                                        )
                                        .child(
                                            Input::new(&self.event_search)
                                                .id("event-filter")
                                                .aria_label(event_filter_label)
                                                .w_full(),
                                        ),
                                )
                                .child(button(
                                    "toggle-api-events",
                                    show_api_events,
                                    self.event_tree.show_api(),
                                    |this, cx| {
                                        this.event_tree.set_show_api(!this.event_tree.show_api());
                                        cx.notify();
                                    },
                                    cx,
                                ))
                            })
                            .when(self.tab == CaptureTab::Parameters, |view| {
                                view.child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .gap_2()
                                        .px_3()
                                        .pb_2()
                                        .child(button(
                                            "structure-parent",
                                            parent_label,
                                            false,
                                            |this, cx| {
                                                let mut path = this.structure.path.clone();
                                                if path.pop().is_some() {
                                                    this.read_structure(path, 0, cx);
                                                }
                                            },
                                            cx,
                                        ))
                                        .child(button(
                                            "structure-previous",
                                            previous_label,
                                            false,
                                            |this, cx| {
                                                if this.structure.offset > 0 {
                                                    this.read_structure(
                                                        this.structure.path.clone(),
                                                        this.structure.offset.saturating_sub(
                                                            u64::from(STRUCTURE_PAGE_LIMIT),
                                                        ),
                                                        cx,
                                                    );
                                                }
                                            },
                                            cx,
                                        ))
                                        .child(button(
                                            "structure-next",
                                            next_label,
                                            false,
                                            |this, cx| {
                                                if let Some(page) = &this.structure.page {
                                                    let next = this
                                                        .structure
                                                        .offset
                                                        .checked_add(page.children.len() as u64);
                                                    let total =
                                                        page.node.child_count.parse::<u64>().ok();
                                                    if let (Some(next), Some(total)) = (next, total)
                                                    {
                                                        if next < total {
                                                            this.read_structure(
                                                                this.structure.path.clone(),
                                                                next,
                                                                cx,
                                                            );
                                                        }
                                                    }
                                                }
                                            },
                                            cx,
                                        )),
                                )
                            })
                            .when(
                                self.tab == CaptureTab::Parameters && is_structured_buffer,
                                |view| {
                                    view.child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .gap_2()
                                            .px_3()
                                            .pb_2()
                                            .child(button(
                                                "structure-load-bytes",
                                                load_structure_bytes,
                                                false,
                                                |this, cx| this.read_structure_buffer(0, cx),
                                                cx,
                                            ))
                                            .when(self.structure.bytes.is_some(), |row| {
                                                row.child(button(
                                                    "structure-bytes-previous",
                                                    previous_bytes,
                                                    false,
                                                    |this, cx| {
                                                        if this.structure.bytes_offset > 0 {
                                                            this.read_structure_buffer(
                                                                this.structure
                                                                    .bytes_offset
                                                                    .saturating_sub(
                                                                        BUFFER_VIEW_PAGE,
                                                                    ),
                                                                cx,
                                                            );
                                                        }
                                                    },
                                                    cx,
                                                ))
                                                .child(button(
                                                    "structure-bytes-next",
                                                    next_bytes,
                                                    false,
                                                    |this, cx| {
                                                        if let (Some(page), Some(bytes)) = (
                                                            &this.structure.page,
                                                            &this.structure.bytes,
                                                        ) {
                                                            let next = this
                                                                .structure
                                                                .bytes_offset
                                                                .checked_add(bytes.len() as u64);
                                                            let total = page
                                                                .node
                                                                .byte_size
                                                                .parse::<u64>()
                                                                .ok();
                                                            if let (Some(next), Some(total)) =
                                                                (next, total)
                                                            {
                                                                if next < total {
                                                                    this.read_structure_buffer(
                                                                        next, cx,
                                                                    );
                                                                }
                                                            }
                                                        }
                                                    },
                                                    cx,
                                                ))
                                            }),
                                    )
                                },
                            )
                            .child(list),
                    )
                    .child(
                        div()
                            .id("capture-details-scroll")
                            .test_support()
                            .w(px(300.))
                            .flex_none()
                            .min_h_0()
                            .overflow_y_scroll()
                            .bg(rgb(0x192332))
                            .rounded_lg()
                            .p_4()
                            .child(detail_view),
                    ),
            )
    }
}

fn button(
    id: &'static str,
    title: &'static str,
    accent: bool,
    callback: impl Fn(&mut Workspace, &mut Context<Workspace>) + 'static,
    cx: &mut Context<Workspace>,
) -> Button {
    let button = Button::new(id)
        .label(title)
        .on_click(cx.listener(move |this, _, _, cx| callback(this, cx)));
    if accent {
        button.primary()
    } else {
        button.secondary()
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.snapshot.is_some() {
            return self.capture_view(cx).into_any_element();
        }
        let title = self.t(
            "A clearer view of every frame.",
            "让每一帧都清晰可见。",
            "すべてのフレームを、より明確に。",
            "모든 프레임을 더 선명하게.",
        );
        let intro = self.t(
            "A native workspace built around RenderDoc.",
            "基于 RenderDoc 的原生图形调试工作台。",
            "RenderDoc を中心にしたネイティブワークスペース。",
            "RenderDoc 기반의 네이티브 작업 공간입니다.",
        );
        let open = self.t(
            "Open capture",
            "打开捕获文件",
            "キャプチャを開く",
            "캡처 열기",
        );
        let launch = self.t(
            "Launch application",
            "启动应用并捕获",
            "アプリを起動してキャプチャ",
            "앱 실행 및 캡처",
        );
        let discover = self.t(
            "Find local targets",
            "查找本机目标",
            "ローカルターゲットを検索",
            "로컬 대상 찾기",
        );
        let launch_config_label = self.t(
            "Capture settings",
            "捕获设置",
            "キャプチャ設定",
            "캡처 설정",
        );
        let arguments_label = self.t("Application arguments", "启动参数", "起動引数", "실행 인수");
        let working_label = self.t(
            "Working directory",
            "工作目录",
            "作業ディレクトリ",
            "작업 디렉터리",
        );
        let working_default = self.t(
            "Application directory",
            "程序所在目录",
            "アプリケーションのディレクトリ",
            "응용 프로그램 디렉터리",
        );
        let choose_directory = self.t(
            "Choose directory",
            "选择目录",
            "ディレクトリを選択",
            "디렉터리 선택",
        );
        let clear_directory = self.t("Use default", "使用默认", "既定を使用", "기본값 사용");
        let delay_label = self.t(
            "Debugger delay (seconds)",
            "等待调试器（秒）",
            "デバッガー待機（秒）",
            "디버거 대기（초）",
        );
        let memory_label = self.t(
            "Soft memory limit (MB)",
            "软内存限制（MB）",
            "ソフトメモリ制限（MB）",
            "소프트 메모리 제한（MB）",
        );
        let environment_label = self.t(
            "Environment modifications",
            "环境变量修改",
            "環境変数の変更",
            "환경 변수 변경",
        );
        let environment_name = self.t("Variable name", "变量名", "変数名", "변수 이름");
        let environment_value = self.t("Variable value", "变量值", "変数値", "변수 값");
        let environment_add = self.t("Add modification", "添加修改", "変更を追加", "변경 추가");
        let environment_remove = self.t("Remove", "移除", "削除", "제거");
        let operation_label = match self.launch_env_operation {
            EnvOperation::Set => self.t("Set", "设置", "設定", "설정"),
            EnvOperation::Append => self.t("Append", "追加", "末尾に追加", "뒤에 추가"),
            EnvOperation::Prepend => self.t("Prepend", "前置", "先頭に追加", "앞에 추가"),
        };
        let separator_label = match self.launch_env_separator {
            EnvSeparator::Platform => self.t(
                "Platform separator",
                "平台分隔符",
                "プラットフォーム区切り",
                "플랫폼 구분자",
            ),
            EnvSeparator::SemiColon => self.t("Semicolon", "分号", "セミコロン", "세미콜론"),
            EnvSeparator::Colon => self.t("Colon", "冒号", "コロン", "콜론"),
            EnvSeparator::NoSep => self.t("No separator", "无分隔符", "区切りなし", "구분자 없음"),
        };
        let environment_rows: Vec<String> = self
            .launch_environment
            .iter()
            .map(|entry| {
                let operation = match entry.operation {
                    EnvOperation::Set => self.t("Set", "设置", "設定", "설정"),
                    EnvOperation::Append => self.t("Append", "追加", "末尾に追加", "뒤에 추가"),
                    EnvOperation::Prepend => self.t("Prepend", "前置", "先頭に追加", "앞에 추가"),
                };
                let separator = match entry.separator {
                    EnvSeparator::Platform => {
                        self.t("Platform", "平台", "プラットフォーム", "플랫폼")
                    }
                    EnvSeparator::SemiColon => ";",
                    EnvSeparator::Colon => ":",
                    EnvSeparator::NoSep => self.t("None", "无", "なし", "없음"),
                };
                format!("{} · {operation} · {separator}", entry.name)
            })
            .collect();
        let opts = &self.capture_options;
        let option_labels = vec![
            (
                self.t(
                    "Allow VSync",
                    "允许垂直同步",
                    "VSync を許可",
                    "수직 동기화 허용",
                ),
                opts.allow_vsync,
            ),
            (
                self.t(
                    "Allow fullscreen",
                    "允许全屏",
                    "全画面を許可",
                    "전체 화면 허용",
                ),
                opts.allow_fullscreen,
            ),
            (
                self.t("API validation", "API 验证", "API 検証", "API 검증"),
                opts.api_validation,
            ),
            (
                self.t(
                    "Capture callstacks",
                    "捕获调用栈",
                    "コールスタックを記録",
                    "호출 스택 캡처",
                ),
                opts.capture_callstacks,
            ),
            (
                self.t(
                    "Callstacks for actions only",
                    "仅捕获操作调用栈",
                    "アクションのみ記録",
                    "작업 호출 스택만 캡처",
                ),
                opts.capture_callstacks_only_actions,
            ),
            (
                self.t(
                    "Verify buffer access",
                    "验证缓冲访问",
                    "バッファアクセスを検証",
                    "버퍼 접근 검증",
                ),
                opts.verify_buffer_access,
            ),
            (
                self.t(
                    "Hook child processes",
                    "接入子进程",
                    "子プロセスをフック",
                    "자식 프로세스 후킹",
                ),
                opts.hook_into_children,
            ),
            (
                self.t(
                    "Include all resources",
                    "包含全部资源",
                    "全リソースを含める",
                    "모든 리소스 포함",
                ),
                opts.ref_all_resources,
            ),
            (
                self.t(
                    "Capture all command lists",
                    "捕获全部命令列表",
                    "全コマンドリストを記録",
                    "모든 명령 목록 캡처",
                ),
                opts.capture_all_cmd_lists,
            ),
            (
                self.t(
                    "Mute debug output",
                    "静音调试输出",
                    "デバッグ出力を抑制",
                    "디버그 출력 음소거",
                ),
                opts.debug_output_mute,
            ),
        ];
        let working_directory = self
            .launch_working_directory
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| working_default.to_owned());
        let connect_target = self.t("Connect", "连接", "接続", "연결");
        let upstream = self.t(
            "Open RenderDoc",
            "打开 RenderDoc",
            "RenderDoc を開く",
            "RenderDoc 열기",
        );
        let configure_ui = self.t(
            "Select qrenderdoc",
            "选择 qrenderdoc",
            "qrenderdoc を選択",
            "qrenderdoc 선택",
        );
        let configure_bridge = self.t(
            "Select replay bridge",
            "选择回放桥接程序",
            "リプレイブリッジを選択",
            "리플레이 브리지 선택",
        );
        let language = self.t("LANGUAGE", "语言", "言語", "언어");
        let integration = self.t(
            "UNITY / TUANJIE",
            "UNITY / 团结引擎",
            "UNITY / TUANJIE",
            "UNITY / TUANJIE",
        );
        let integration_text = self.t("Use the engine's RenderDoc integration or launch a standalone player. Some detailed inspection still requires RenderDoc.", "使用引擎自带的 RenderDoc 集成，或启动独立播放器。部分详细调试仍需要 RenderDoc。", "エンジンの RenderDoc 統合またはスタンドアロンプレイヤーを使用します。詳細な解析の一部には RenderDoc が必要です。", "엔진의 RenderDoc 통합 또는 독립 실행형 플레이어를 사용하세요. 일부 상세 분석에는 RenderDoc이 필요합니다.");
        let live_title = self.t(
            "LIVE CAPTURE",
            "实时捕获",
            "ライブキャプチャ",
            "실시간 캡처",
        );
        let trigger = self.t(
            "Capture next frame",
            "捕获下一帧",
            "次のフレームをキャプチャ",
            "다음 프레임 캡처",
        );
        let disconnect = self.t("Disconnect", "断开连接", "切断", "연결 끊기");
        let frame_label = self.t("Frame", "帧", "フレーム", "프레임");
        let remote_label = self.t(
            "Remote capture",
            "远程捕获",
            "リモートキャプチャ",
            "원격 캡처",
        );
        let active = self.live.is_some();
        let live_status = self.live_status.clone();
        let live_captures = self.live_captures.clone();
        let live_targets = self.live_targets.clone();
        let status = self.status.clone();

        div()
            .id("home-scroll")
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .bg(rgb(0x101722))
            .text_color(rgb(0xe6edf6))
            .flex()
            .flex_col()
            .p_8()
            .gap_6()
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x82aaff))
                    .child("RENDERDOCX  /  WORKSPACE"),
            )
            .child(
                div()
                    .text_3xl()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(title),
            )
            .child(div().text_color(rgb(0x9eacc0)).child(intro))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .child(button(
                        "open",
                        open,
                        true,
                        |this, cx| {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("RenderDoc", &["rdc", "cap"])
                                .pick_file()
                            {
                                this.open(path, cx);
                            }
                        },
                        cx,
                    ))
                    .child(button(
                        "launch",
                        launch,
                        false,
                        |this, cx| {
                            if let Some(path) = rfd::FileDialog::new().pick_file() {
                                this.launch_live(path, cx);
                            }
                        },
                        cx,
                    ))
                    .child(button(
                        "launch-settings",
                        launch_config_label,
                        self.show_launch_config,
                        |this, cx| {
                            this.show_launch_config = !this.show_launch_config;
                            cx.notify();
                        },
                        cx,
                    ))
                    .child(button(
                        "discover-targets",
                        discover,
                        false,
                        |this, cx| this.discover_live(cx),
                        cx,
                    ))
                    .child(button(
                        "upstream",
                        upstream,
                        false,
                        |this, cx| {
                            let result = backend::open_upstream(&this.settings);
                            this.result(result, cx);
                        },
                        cx,
                    ))
                    .child(button(
                        "configure-bridge",
                        configure_bridge,
                        false,
                        |this, cx| {
                            if let Some(path) = rfd::FileDialog::new().pick_file() {
                                this.settings.replay_bridge = Some(path);
                                this.save();
                                cx.notify();
                            }
                        },
                        cx,
                    )),
            )
            .when(self.show_launch_config, |view| {
                view.child(
                    div()
                        .bg(rgb(0x192332))
                        .rounded_lg()
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(0x82aaff))
                                .child(launch_config_label),
                        )
                        .when_some(status.clone(), |view, message| view.child(div().text_color(rgb(0xd4b77c)).child(message)))
                        .child(arguments_label)
                        .child(
                            Input::new(&self.launch_arguments)
                                .id("launch-arguments")
                                .aria_label(arguments_label)
                                .w_full(),
                        )
                        .child(format!("{working_label}: {working_directory}"))
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_2()
                                .child(button(
                                    "launch-working-directory",
                                    choose_directory,
                                    false,
                                    |this, cx| {
                                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                            this.launch_working_directory = Some(path);
                                            cx.notify();
                                        }
                                    },
                                    cx,
                                ))
                                .child(button(
                                    "launch-working-default",
                                    clear_directory,
                                    false,
                                    |this, cx| {
                                        this.launch_working_directory = None;
                                        cx.notify();
                                    },
                                    cx,
                                )),
                        )
                        .child(delay_label)
                        .child(
                            Input::new(&self.launch_delay)
                                .id("launch-delay")
                                .aria_label(delay_label)
                                .w_full(),
                        )
                        .child(memory_label)
                        .child(
                            Input::new(&self.launch_memory)
                                .id("launch-memory")
                                .aria_label(memory_label)
                                .w_full(),
                        )
                        .child(div().flex().flex_wrap().gap_2().children(
                            option_labels.into_iter().enumerate().map(
                                |(index, (label, enabled))| {
                                    div()
                                        .id(SharedString::from(format!("capture-option-{index}")))
                                        .test_support()
                                        .cursor_pointer()
                                        .px_3()
                                        .py_2()
                                        .rounded_md()
                                        .bg(rgb(if enabled { 0x4169e1 } else { 0x252f40 }))
                                        .child(format!(
                                            "{} {label}",
                                            if enabled { "☑" } else { "☐" }
                                        ))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            let opts = &mut this.capture_options;
                                            let field = match index {
                                                0 => &mut opts.allow_vsync,
                                                1 => &mut opts.allow_fullscreen,
                                                2 => &mut opts.api_validation,
                                                3 => &mut opts.capture_callstacks,
                                                4 => &mut opts.capture_callstacks_only_actions,
                                                5 => &mut opts.verify_buffer_access,
                                                6 => &mut opts.hook_into_children,
                                                7 => &mut opts.ref_all_resources,
                                                8 => &mut opts.capture_all_cmd_lists,
                                                _ => &mut opts.debug_output_mute,
                                            };
                                            *field = !*field;
                                            cx.notify();
                                        }))
                                },
                            ),
                        ))
                        .child(environment_label)
                        .child(environment_name)
                        .child(Input::new(&self.launch_env_name).id("launch-env-name").aria_label(environment_name).w_full())
                        .child(environment_value)
                        .child(Input::new(&self.launch_env_value).id("launch-env-value").aria_label(environment_value).w_full())
                        .child(div().flex().flex_wrap().gap_2()
                            .child(button("launch-env-operation", operation_label, false, |this, cx| {
                                this.launch_env_operation = match this.launch_env_operation { EnvOperation::Set => EnvOperation::Append, EnvOperation::Append => EnvOperation::Prepend, EnvOperation::Prepend => EnvOperation::Set }; cx.notify();
                            }, cx))
                            .child(button("launch-env-separator", separator_label, false, |this, cx| {
                                this.launch_env_separator = match this.launch_env_separator { EnvSeparator::Platform => EnvSeparator::SemiColon, EnvSeparator::SemiColon => EnvSeparator::Colon, EnvSeparator::Colon => EnvSeparator::NoSep, EnvSeparator::NoSep => EnvSeparator::Platform }; cx.notify();
                            }, cx))
                            .child(div().id("launch-env-add").test_support().cursor_pointer().px_3().py_2().rounded_md().bg(rgb(0x4169e1)).child(environment_add)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let entry = EnvironmentModification { name: this.launch_env_name.read(cx).value().to_string(), value: this.launch_env_value.read(cx).value().to_string(), operation: this.launch_env_operation, separator: this.launch_env_separator };
                                    let validation = LaunchConfiguration { environment: vec![entry.clone()], ..Default::default() }.validate();
                                    if validation.is_err() {
                                        this.status = Some(this.t("Invalid environment variable name or value.", "环境变量名或值无效。", "環境変数名または値が無効です。", "환경 변수 이름 또는 값이 올바르지 않습니다.").to_owned());
                                    } else {
                                        this.launch_environment.push(entry);
                                        this.launch_env_name.update(cx, |state, cx| state.set_value("", window, cx));
                                        this.launch_env_value.update(cx, |state, cx| state.set_value("", window, cx));
                                        this.status = None;
                                    }
                                    cx.notify();
                                }))))
                        .children(environment_rows.into_iter().enumerate().map(|(index, label)| {
                            div().flex().items_center().gap_3().child(label)
                                .child(div().id(SharedString::from(format!("launch-env-remove-{index}"))).test_support().cursor_pointer().px_3().py_2().rounded_md().bg(rgb(0x252f40)).child(environment_remove)
                                    .on_click(cx.listener(move |this, _, _, cx| { if index < this.launch_environment.len() { this.launch_environment.remove(index); cx.notify(); } })))
                        })),
                )
            })
            .when(!live_targets.is_empty(), |view| {
                view.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .children(live_targets.into_iter().map(|ident| {
                            div()
                                .id(SharedString::from(format!("target-{ident}")))
                                .test_support()
                                .cursor_pointer()
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .bg(rgb(0x252f40))
                                .child(format!("{connect_target} {ident}"))
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.connect_live(ident, cx)),
                                )
                        })),
                )
            })
            .when(
                active || !live_captures.is_empty() || live_status.is_some(),
                |view| {
                    let capture_button = button(
                        "live-trigger",
                        trigger,
                        true,
                        |this, cx| {
                            if let Some(session) = &this.live {
                                this.live_status = Some(match session.trigger(1) {
                                    Ok(()) => this
                                        .t(
                                            "Capture request submitted",
                                            "捕获请求已提交",
                                            "キャプチャ要求を送信しました",
                                            "캡처 요청을 보냈습니다",
                                        )
                                        .to_owned(),
                                    Err(error) => format!(
                                        "{}: {error}",
                                        this.t(
                                            "Target error",
                                            "目标错误",
                                            "ターゲットエラー",
                                            "대상 오류"
                                        )
                                    ),
                                });
                                cx.notify();
                            }
                        },
                        cx,
                    );
                    let disconnect_button = button(
                        "live-disconnect",
                        disconnect,
                        false,
                        |this, cx| {
                            if let Some(session) = this.live.take() {
                                this.live_generation += 1;
                                let generation = this.live_generation;
                                this.live_status = Some(
                                    this.t(
                                        "Disconnecting…",
                                        "正在断开…",
                                        "切断中…",
                                        "연결 해제 중…",
                                    )
                                    .to_owned(),
                                );
                                let task = cx
                                    .background_executor()
                                    .spawn(async move { session.close() });
                                cx.spawn(async move |this, cx| {
                                    let result = task.await;
                                    let _ = this.update(cx, |this, cx| {
                                        if this.live_generation != generation {
                                            return;
                                        }
                                        this.live_status = Some(match result {
                                            Ok(()) => this
                                                .t(
                                                    "Target disconnected",
                                                    "目标已断开",
                                                    "ターゲット切断",
                                                    "대상 연결 끊김",
                                                )
                                                .to_owned(),
                                            Err(error) => format!(
                                                "{}: {error}",
                                                this.t(
                                                    "Disconnect failed",
                                                    "断开失败",
                                                    "切断に失敗しました",
                                                    "연결 해제 실패"
                                                )
                                            ),
                                        });
                                        cx.notify();
                                    });
                                })
                                .detach();
                                cx.notify();
                            }
                        },
                        cx,
                    );
                    view.child(
                        div()
                            .bg(rgb(0x192332))
                            .rounded_lg()
                            .p_5()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(div().text_sm().text_color(rgb(0x82aaff)).child(live_title))
                            .when_some(live_status, |view, text| view.child(text))
                            .when(active, |view| {
                                view.child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .gap_2()
                                        .child(capture_button)
                                        .child(disconnect_button),
                                )
                            })
                            .children(live_captures.into_iter().enumerate().map(
                                |(index, (frame, path, local))| {
                                    let label = format!(
                                        "{frame_label} {frame} · {}{}",
                                        path.display(),
                                        if local { "" } else { remote_label }
                                    );
                                    div()
                                        .id(SharedString::from(format!("live-capture-{index}")))
                                        .test_support()
                                        .cursor_pointer()
                                        .text_sm()
                                        .child(label)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if local {
                                                this.open(path.clone(), cx);
                                            }
                                        }))
                                },
                            )),
                    )
                },
            )
            .child(
                div()
                    .bg(rgb(0x192332))
                    .rounded_lg()
                    .p_5()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(div().text_sm().text_color(rgb(0x82aaff)).child(integration))
                    .child(div().text_color(rgb(0xc3cedd)).child(integration_text)),
            )
            .child(div().flex().flex_wrap().gap_3().child(button(
                "configure-ui",
                configure_ui,
                false,
                |this, cx| {
                    if let Some(path) = rfd::FileDialog::new().pick_file() {
                        this.settings.qrenderdoc = Some(path);
                        this.save();
                        cx.notify();
                    }
                },
                cx,
            )))
            .child(div().text_sm().text_color(rgb(0x9eacc0)).child(language))
            .child(div().flex().flex_wrap().gap_2().children(
                Language::ALL.into_iter().enumerate().map(|(i, lang)| {
                    div()
                        .id(SharedString::from(format!("language-{i}")))
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .cursor_pointer()
                        .bg(rgb(if self.settings.language == lang {
                            0x4169e1
                        } else {
                            0x252f40
                        }))
                        .child(lang.label())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.settings.language = lang;
                            this.save();
                            cx.notify();
                        }))
                }),
            ))
            .when_some(status, |s, status| {
                s.child(div().text_color(rgb(0xd4b77c)).child(status))
            })
            .into_any_element()
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx: &mut App| {
            gpui_kit::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            let bounds = Bounds::centered(None, size(px(1040.), px(760.)), cx);
            let settings = Settings::load();
            let initial_capture = std::env::var_os("RENDERDOCX_OPEN_CAPTURE")
                .or_else(|| std::env::args_os().nth(1))
                .map(std::path::PathBuf::from);
            let target_pid = std::env::var_os("RENDERDOCX_CONNECT_PID");
            let initial_pid = target_pid.as_deref().and_then(parse_target_pid);
            let invalid_pid = target_pid.is_some() && initial_pid.is_none();
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    let launch_arguments = cx.new(|cx| InputState::new(window, cx));
                    let launch_delay = cx.new(|cx| InputState::new(window, cx).default_value("0"));
                    let launch_memory = cx.new(|cx| InputState::new(window, cx).default_value("0"));
                    let launch_env_name = cx.new(|cx| InputState::new(window, cx));
                    let launch_env_value = cx.new(|cx| InputState::new(window, cx).masked(true));
                    let event_search = cx.new(|cx| InputState::new(window, cx));
                    let resource_search = cx.new(|cx| InputState::new(window, cx));
                    let stats_min = cx.new(|cx| InputState::new(window, cx).default_value("0"));
                    let stats_max = cx.new(|cx| InputState::new(window, cx).default_value("1"));
                    let pixel_x = cx.new(|cx| InputState::new(window, cx).default_value("0"));
                    let pixel_y = cx.new(|cx| InputState::new(window, cx).default_value("0"));
                    let entity = cx.new(|cx| Workspace {
                        settings,
                        status: None,
                        capture: None,
                        session: None,
                        snapshot: None,
                        event_tree: EventTree::default(),
                        event_scroll: UniformListScrollHandle::new(),
                        _resource_search_subscription: Workspace::watch_resource_search(
                            &resource_search,
                            cx,
                        ),
                        resource_search,
                        resource_query: String::new(),
                        resource_visible: Vec::new(),
                        _event_search_subscription: Workspace::watch_event_search(
                            &event_search,
                            cx,
                        ),
                        event_search,
                        selected: None,
                        pipeline: None,
                        tab: CaptureTab::Events,
                        selected_texture: None,
                        texture_selection: TextureSelection::default(),
                        pixel_x,
                        pixel_y,
                        pixel: None,
                        pixel_history: None,
                        history_selected: None,
                        stats_min,
                        stats_max,
                        statistics: None,
                        selected_buffer: None,
                        buffer_offset: 0,
                        buffer_data: None,
                        resource_pending: false,
                        selected_shader: None,
                        shader_data: None,
                        structure: StructureView::default(),
                        mesh: MeshView::default(),
                        input_page: None,
                        debug_vertex_selection: None,
                        debug_view: None,
                        selected_resource: None,
                        usage_page: None,
                        open_revision: 0,
                        selection_revision: 0,
                        replay_pending: false,
                        export_pending: false,
                        preview: None,
                        live: None,
                        live_generation: 0,
                        live_starting: false,
                        live_status: None,
                        live_captures: Vec::new(),
                        live_targets: Vec::new(),
                        show_launch_config: false,
                        launch_arguments,
                        launch_delay,
                        launch_memory,
                        launch_working_directory: None,
                        capture_options: CaptureOptions::default(),
                        launch_env_name,
                        launch_env_value,
                        launch_environment: Vec::new(),
                        launch_env_operation: EnvOperation::Set,
                        launch_env_separator: EnvSeparator::Platform,
                    });
                    if let Some(path) = initial_capture {
                        entity.update(cx, |view, cx| view.open(path, cx));
                    }
                    if let Some(pid) = initial_pid {
                        entity.update(cx, |view, cx| view.connect_target(pid, true, cx));
                    } else if invalid_pid {
                        entity.update(cx, |view, cx| {
                            view.status = Some(
                                view.t(
                                    "Invalid Editor process ID.",
                                    "Editor 进程 ID 无效。",
                                    "Editor のプロセス ID が無効です。",
                                    "Editor 프로세스 ID가 올바르지 않습니다.",
                                )
                                .to_owned(),
                            );
                            cx.notify();
                        });
                    }
                    entity
                },
            )
            .expect("Failed to open RenderDocX window");
            cx.activate(true);
        });
}

#[cfg(all(test, feature = "ui-tests"))]
mod ui_tests {
    use super::*;
    use gpui_kit::{ScrollDelta, TestAppContext, base::Root, point, test::TestWindowExt};

    #[gpui_kit::test]
    fn capture_workspace_back_button_returns_to_home(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (handle, view) = cx.update(|cx| {
            let bounds = Bounds::centered(None, size(px(1040.), px(760.)), cx);
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    let launch_arguments = cx.new(|cx| InputState::new(window, cx));
                    let launch_delay = cx.new(|cx| InputState::new(window, cx).default_value("0"));
                    let launch_memory = cx.new(|cx| InputState::new(window, cx).default_value("0"));
                    let launch_env_name = cx.new(|cx| InputState::new(window, cx));
                    let launch_env_value = cx.new(|cx| InputState::new(window, cx).masked(true));
                    let event_search = cx.new(|cx| InputState::new(window, cx));
                    let resource_search = cx.new(|cx| InputState::new(window, cx));
                    let stats_min = cx.new(|cx| InputState::new(window, cx).default_value("0"));
                    let stats_max = cx.new(|cx| InputState::new(window, cx).default_value("1"));
                    let pixel_x = cx.new(|cx| InputState::new(window, cx).default_value("0"));
                    let pixel_y = cx.new(|cx| InputState::new(window, cx).default_value("0"));
                    cx.new(|cx| Workspace {
                        settings: Settings::default(),
                        status: None,
                        capture: Some("capture.rdc".into()),
                        session: None,
                        snapshot: Some(Snapshot {
                            protocol: 1,
                            api: 3,
                            frame: 2,
                            degraded: false,
                            actions: Vec::new(),
                            resource_count: 0,
                            texture_count: 0,
                            buffer_count: 0,
                            resources: Vec::new(),
                            textures: Vec::new(),
                            buffers: Vec::new(),
                        }),
                        event_tree: EventTree::default(),
                        event_scroll: UniformListScrollHandle::new(),
                        _resource_search_subscription: Workspace::watch_resource_search(
                            &resource_search,
                            cx,
                        ),
                        resource_search,
                        resource_query: String::new(),
                        resource_visible: Vec::new(),
                        _event_search_subscription: Workspace::watch_event_search(
                            &event_search,
                            cx,
                        ),
                        event_search,
                        selected: None,
                        pipeline: None,
                        tab: CaptureTab::Events,
                        selected_texture: None,
                        texture_selection: TextureSelection::default(),
                        pixel_x,
                        pixel_y,
                        pixel: None,
                        pixel_history: None,
                        history_selected: None,
                        stats_min,
                        stats_max,
                        statistics: None,
                        selected_buffer: None,
                        buffer_offset: 0,
                        buffer_data: None,
                        resource_pending: false,
                        selected_shader: None,
                        shader_data: None,
                        structure: StructureView::default(),
                        mesh: MeshView::default(),
                        input_page: None,
                        debug_vertex_selection: None,
                        debug_view: None,
                        selected_resource: None,
                        usage_page: None,
                        open_revision: 0,
                        selection_revision: 0,
                        replay_pending: false,
                        export_pending: false,
                        preview: None,
                        live: None,
                        live_generation: 0,
                        live_starting: false,
                        live_status: None,
                        live_captures: Vec::new(),
                        live_targets: Vec::new(),
                        show_launch_config: false,
                        launch_arguments,
                        launch_delay,
                        launch_memory,
                        launch_working_directory: None,
                        capture_options: CaptureOptions::default(),
                        launch_env_name,
                        launch_env_value,
                        launch_environment: Vec::new(),
                        launch_env_operation: EnvOperation::Set,
                        launch_env_separator: EnvSeparator::Platform,
                    })
                },
            )
            .unwrap()
        });
        let handle = handle.downcast::<Root>().unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("capture-back", cx);
        })
        .unwrap();
        cx.update(|cx| {
            assert!(view.read(cx).snapshot.is_none());
            assert!(view.read(cx).capture.is_none());
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("launch-settings", cx);
            window.click("launch-arguments", cx);
            window.input("--fixture 测试", cx);
            window.click("launch-delay", cx);
            window.input("x", cx);
            window.scroll(
                "home-scroll",
                ScrollDelta::Pixels(point(px(0.), px(-500.))),
                cx,
            );
            window.click("capture-option-0", cx);
        })
        .unwrap();
        cx.update(|cx| {
            assert_eq!(
                view.read(cx).launch_arguments.read(cx).value().as_ref(),
                "--fixture 测试"
            );
            assert!(!view.read(cx).capture_options.allow_vsync);
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.scroll(
                "home-scroll",
                ScrollDelta::Pixels(point(px(0.), px(-700.))),
                cx,
            );
            window.click("launch-env-name", cx);
            window.input("RENDERDOCX_TEST", cx);
            window.click("launch-env-value", cx);
            window.input("fixture-value", cx);
            window.click("launch-env-operation", cx);
            window.click("launch-env-separator", cx);
            window.click("launch-env-separator", cx);
            window.click("launch-env-add", cx);
        })
        .unwrap();
        cx.update(|cx| {
            let entry = &view.read(cx).launch_environment[0];
            assert_eq!(entry.name, "RENDERDOCX_TEST");
            assert_eq!(entry.value, "fixture-value");
            assert!(matches!(entry.operation, EnvOperation::Append));
            assert!(matches!(entry.separator, EnvSeparator::Colon));
            assert!(view.read(cx).launch_env_value.read(cx).value().is_empty());
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.scroll(
                "home-scroll",
                ScrollDelta::Pixels(point(px(0.), px(-700.))),
                cx,
            );
            window.click("launch-env-remove-0", cx);
        })
        .unwrap();
        cx.update(|cx| assert!(view.read(cx).launch_environment.is_empty()));
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.launch_live("missing-target.exe".into(), cx)
            })
        });
        cx.update(|cx| {
            assert!(!view.read(cx).live_starting);
            assert!(view.read(cx).status.is_some());
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.scroll(
                "home-scroll",
                ScrollDelta::Pixels(point(px(0.), px(1200.))),
                cx,
            );
            window.click("launch-settings", cx);
        })
        .unwrap();
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.settings.replay_bridge = Some("definitely-missing-replay-bridge".into());
                view.live_status = Some("fixture".into());
                view.live_targets = vec![38920];
                view.live_captures
                    .push((42, "missing-live-capture.rdc".into(), true));
                cx.notify();
            })
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("target-38920").visible());
            assert!(window.find("live-capture-0").visible());
            window.click("live-capture-0", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(view.read(cx).open_revision > 0);
            assert!(view.read(cx).status.is_some());
            assert!(view.read(cx).snapshot.is_none());
        });
    }

    #[gpui_kit::test]
    fn clicking_an_action_replays_its_exact_event(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (session, events, selections) = renderdocx::replay::test_support::recording_session();
        let (handle, view) = cx.update(|cx| {
            let bounds = Bounds::centered(None, size(px(1040.), px(760.)), cx);
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    let launch_arguments = cx.new(|cx| InputState::new(window, cx));
                    let launch_delay = cx.new(|cx| InputState::new(window, cx).default_value("0"));
                    let launch_memory = cx.new(|cx| InputState::new(window, cx).default_value("0"));
                    let launch_env_name = cx.new(|cx| InputState::new(window, cx));
                    let launch_env_value = cx.new(|cx| InputState::new(window, cx).masked(true));
                    let event_search = cx.new(|cx| InputState::new(window, cx));
                    let resource_search = cx.new(|cx| InputState::new(window, cx));
                    let stats_min = cx.new(|cx| InputState::new(window, cx).default_value("0"));
                    let stats_max = cx.new(|cx| InputState::new(window, cx).default_value("1"));
                    let pixel_x = cx.new(|cx| InputState::new(window, cx).default_value("0"));
                    let pixel_y = cx.new(|cx| InputState::new(window, cx).default_value("0"));
                    cx.new(|cx| Workspace {
                        settings: Settings::default(),
                        status: None,
                        capture: Some("capture.rdc".into()),
                        session: Some(session),
                        snapshot: Some(Snapshot {
                            protocol: 1,
                            api: 0,
                            frame: 2,
                            degraded: false,
                            actions: vec![
                                renderdocx::replay::Action {
                                    event_id: 42,
                                    action_id: 3,
                                    depth: 0,
                                    parent: -1,
                                    flags: 2,
                                    indices: 3,
                                    instances: 1,
                                    base_vertex: Some(0),
                                    index_offset: Some(0),
                                    vertex_offset: Some(0),
                                    instance_offset: Some(0),
                                    name: "Draw".into(),
                                    api_events: vec![
                                        renderdocx::replay::ApiEvent {
                                            event_id: 41,
                                            chunk_index: Some(3),
                                            file_offset: "18446744073709551615".into(),
                                            name: "IASetPrimitiveTopology()".into(),
                                        },
                                        renderdocx::replay::ApiEvent {
                                            event_id: 42,
                                            chunk_index: Some(4),
                                            file_offset: "100".into(),
                                            name: "DrawIndexed()".into(),
                                        },
                                    ],
                                },
                                renderdocx::replay::Action {
                                    event_id: 43,
                                    action_id: 4,
                                    depth: 1,
                                    parent: 0,
                                    flags: 2,
                                    indices: 3,
                                    instances: 1,
                                    base_vertex: Some(0),
                                    index_offset: Some(0),
                                    vertex_offset: Some(0),
                                    instance_offset: Some(0),
                                    name: "Child".into(),
                                    api_events: Vec::new(),
                                },
                                renderdocx::replay::Action {
                                    event_id: 77,
                                    action_id: 5,
                                    depth: 0,
                                    parent: -1,
                                    flags: 2,
                                    indices: 3,
                                    instances: 1,
                                    base_vertex: Some(0),
                                    index_offset: Some(0),
                                    vertex_offset: Some(0),
                                    instance_offset: Some(0),
                                    name: "Next".into(),
                                    api_events: Vec::new(),
                                },
                            ],
                            resource_count: 2,
                            texture_count: 1,
                            buffer_count: 1,
                            resources: vec![
                                renderdocx::replay::ResourceInfo {
                                    name: "Fixture".into(),
                                    resource_type: 5,
                                    autogenerated_name: false,
                                    initialisation_chunks: vec![2],
                                    parents: vec![],
                                    derived: vec![Some(1)],
                                },
                                renderdocx::replay::ResourceInfo {
                                    name: "View".into(),
                                    resource_type: 6,
                                    autogenerated_name: false,
                                    initialisation_chunks: vec![],
                                    parents: vec![Some(0)],
                                    derived: vec![],
                                },
                            ],
                            textures: vec![renderdocx::replay::Texture {
                                name: "Color".into(),
                                width: 16,
                                height: 16,
                                depth: 1,
                                dimension: 2,
                                mips: 2,
                                array_size: 2,
                                samples: 1,
                                bytes: "1024".into(),
                                format_name: "R8G8B8A8_UNORM".into(),
                                format_type: 0,
                                component_type: 0,
                                component_count: 4,
                                component_byte_width: 1,
                            }],
                            buffers: vec![renderdocx::replay::Buffer {
                                name: "Vertices".into(),
                                resource_index: Some(0),
                                length: "300".into(),
                                creation_flags: 1,
                            }],
                        }),
                        event_tree: EventTree::default(),
                        event_scroll: UniformListScrollHandle::new(),
                        _resource_search_subscription: Workspace::watch_resource_search(
                            &resource_search,
                            cx,
                        ),
                        resource_search,
                        resource_query: String::new(),
                        resource_visible: Vec::new(),
                        _event_search_subscription: Workspace::watch_event_search(
                            &event_search,
                            cx,
                        ),
                        event_search,
                        selected: None,
                        pipeline: None,
                        tab: CaptureTab::Events,
                        selected_texture: None,
                        texture_selection: TextureSelection::default(),
                        pixel_x,
                        pixel_y,
                        pixel: None,
                        pixel_history: None,
                        history_selected: None,
                        stats_min,
                        stats_max,
                        statistics: None,
                        selected_buffer: None,
                        buffer_offset: 0,
                        buffer_data: None,
                        resource_pending: false,
                        selected_shader: None,
                        shader_data: None,
                        structure: StructureView::default(),
                        mesh: MeshView::default(),
                        input_page: None,
                        debug_vertex_selection: None,
                        debug_view: None,
                        selected_resource: None,
                        usage_page: None,
                        open_revision: 0,
                        selection_revision: 0,
                        replay_pending: false,
                        export_pending: false,
                        preview: None,
                        live: None,
                        live_generation: 0,
                        live_starting: false,
                        live_status: None,
                        live_captures: Vec::new(),
                        live_targets: Vec::new(),
                        show_launch_config: false,
                        launch_arguments,
                        launch_delay,
                        launch_memory,
                        launch_working_directory: None,
                        capture_options: CaptureOptions::default(),
                        launch_env_name,
                        launch_env_value,
                        launch_environment: Vec::new(),
                        launch_env_operation: EnvOperation::Set,
                        launch_env_separator: EnvSeparator::Platform,
                    })
                },
            )
            .unwrap()
        });
        let handle = handle.downcast::<Root>().unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find(1usize).visible());
            window.click("event-toggle-0", cx);
            window.render_frame(cx);
            assert_eq!(view.read(cx).event_tree.visible, [0, 2]);
            window.click("event-toggle-0", cx);
            window.render_frame(cx);
            assert!(window.find(1usize).visible());
            window.click("textures-tab", cx);
            window.click("texture-0", cx);
            window.click("texture-export", cx);
        })
        .unwrap();
        cx.update(|cx| {
            assert!(view.read(cx).status.is_some());
            assert!(!view.read(cx).export_pending);
        });
        assert!(events.lock().unwrap().is_empty());
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("buffers-tab", cx);
            window.click("buffer-0", cx);
        })
        .unwrap();
        cx.update(|cx| {
            assert!(view.read(cx).status.is_some());
            assert!(view.read(cx).buffer_data.is_none());
            assert!(!view.read(cx).resource_pending);
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("events-tab", cx);
            window.click(0usize, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(view.read(cx).selected, Some(0));
            let pipeline = view.read(cx).pipeline.as_ref().expect("selected pipeline");
            assert_eq!(pipeline.event_id, 42);
            assert_eq!(pipeline.topology, 5);
            assert_eq!(pipeline.index_buffer.as_ref().unwrap().byte_offset, "2");
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("capture-details-scroll").visible());
            assert!(!window.find(("pipeline-viewport", 19usize)).visible());
            window.scroll(
                "capture-details-scroll",
                ScrollDelta::Pixels(point(px(0.), px(-2000.))),
                cx,
            );
            assert!(window.find(("pipeline-viewport", 19usize)).visible());
        })
        .unwrap();
        assert_eq!(*events.lock().unwrap(), vec![42]);
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("textures-tab", cx);
            window.click("texture-0", cx);
        })
        .unwrap();
        cx.update(|cx| {
            assert!(view.read(cx).tab == CaptureTab::Textures);
            assert_eq!(view.read(cx).selected_texture, Some(0));
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.scroll(
                "capture-details-scroll",
                ScrollDelta::Pixels(point(px(0.), px(-600.))),
                cx,
            );
            window.click("pixel-x", cx);
            window.press("ctrl-a", cx);
            window.input("3", cx);
            window.click("pixel-y", cx);
            window.press("ctrl-a", cx);
            window.input("4", cx);
            window.click("pixel-pick", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let pixel = view.read(cx).pixel.as_ref().unwrap();
            assert_eq!((pixel.event_id, pixel.x, pixel.y), (42, 3, 4));
            assert_eq!(pixel.float_values, ["0.25", "0.5", "0.75", "1"]);
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("pixel-history", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let history = view.read(cx).pixel_history.as_ref().unwrap();
            assert_eq!((history.event_id, history.x, history.y), (42, 3, 4));
            assert_eq!(history.entries.len(), 2);
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.scroll(
                "capture-details-scroll",
                ScrollDelta::Pixels(point(px(0.), px(-500.))),
                cx,
            );
            window.click(("pixel-history-row", 1usize), cx);
        })
        .unwrap();
        cx.update(|cx| {
            let view = view.read(cx);
            assert_eq!(view.history_selected, Some(1));
            assert!(
                view.pixel_history.as_ref().unwrap().entries[1]
                    .tests
                    .scissor_clipped
            );
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.scroll(
                "capture-details-scroll",
                ScrollDelta::Pixels(point(px(0.), px(-2400.))),
                cx,
            );
            window.click("histogram-analyze", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let stats = view.read(cx).statistics.as_ref().unwrap();
            assert_eq!(stats.event_id, 42);
            assert_eq!(stats.histogram[64], 16);
            assert_eq!(stats.request.channels, [true, true, true, true]);
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.scroll(
                "capture-details-scroll",
                ScrollDelta::Pixels(point(px(0.), px(-1200.))),
                cx,
            );
            assert!(window.find("texture-histogram").visible());
        })
        .unwrap();
        assert_eq!(*events.lock().unwrap(), vec![42]);
        cx.update_window(handle.into(), |_, window, cx| {
            window.scroll(
                "capture-details-scroll",
                ScrollDelta::Pixels(point(px(0.), px(2000.))),
                cx,
            );
            window.click("texture-next-mip", cx);
            window.click("texture-next-slice", cx);
            window.click("texture-next-sample", cx);
            window.click("texture-next-channel", cx);
            window.click("texture-preview", cx)
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(view.read(cx).pixel.is_none());
            assert!(view.read(cx).statistics.is_none());
            assert!(view.read(cx).pixel_history.is_none());
        });
        assert_eq!(
            *selections.lock().unwrap(),
            vec![TextureSelection {
                mip: 1,
                slice: 1,
                sample: Some(0),
                channel: Some(0)
            }]
        );
        cx.update(|cx| {
            let view = view.read(cx);
            assert!(!view.export_pending);
            assert!(
                view.preview
                    .as_ref()
                    .is_some_and(|preview| preview.path.is_file())
            );
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.scroll(
                "capture-details-scroll",
                ScrollDelta::Pixels(point(px(0.), px(-800.))),
                cx,
            );
            assert!(window.find("texture-preview-image").visible());
            assert!(window.find("texture-preview-image").bounds().size.height > px(0.));
            window.scroll(
                "capture-details-scroll",
                ScrollDelta::Pixels(point(px(0.), px(2000.))),
                cx,
            );
            window.click("texture-next-channel", cx);
        })
        .unwrap();
        cx.update(|cx| assert!(view.read(cx).preview.is_none()));
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("buffers-tab", cx);
            window.click("buffer-0", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let view = view.read(cx);
            assert_eq!(view.buffer_offset, 0);
            assert_eq!(view.buffer_data.as_ref().unwrap().len(), 256);
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.scroll(
                "capture-details-scroll",
                ScrollDelta::Pixels(point(px(0.), px(2000.))),
                cx,
            );
            assert!(window.find(("buffer-line", 0usize)).visible());
            window.click("buffer-next", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let view = view.read(cx);
            assert_eq!(view.buffer_offset, 256);
            assert_eq!(view.buffer_data.as_ref().unwrap().len(), 44);
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("shaders-tab", cx);
            window.click("shader-0", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let shader = view.read(cx).shader_data.as_ref().expect("shader report");
            assert_eq!(shader.event_id, 42);
            assert_eq!(shader.disassembly, "fixture disassembly");
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.scroll(
                "capture-details-scroll",
                ScrollDelta::Pixels(point(px(0.), px(2000.))),
                cx,
            );
            assert!(window.find("shader-disassembly").visible());
            window.click("shader-next-target", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).shader_data.as_ref().unwrap().target, "native"));
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("events-tab", cx);
            window.click("event-toggle-0", cx);
            window.render_frame(cx);
            window.click(2usize, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).pipeline.as_ref().unwrap().event_id, 77));
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("event-toggle-0", cx);
            window.render_frame(cx);
            window.click(1usize, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).pipeline.as_ref().unwrap().event_id, 43));
        assert_eq!(*events.lock().unwrap(), vec![42, 77, 43]);
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("event-toggle-0", cx);
            window.render_frame(cx);
        })
        .unwrap();
        cx.update(|cx| {
            assert_eq!(view.read(cx).selected, Some(1));
            assert_eq!(view.read(cx).pipeline.as_ref().unwrap().event_id, 43);
            assert_eq!(view.read(cx).event_tree.visible, [0, 2]);
        });
        assert_eq!(*events.lock().unwrap(), vec![42, 77, 43]);
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("event-filter", cx);
            window.input("cHiLd", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).event_tree.visible, [0]));
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("event-toggle-0", cx);
            window.render_frame(cx);
            assert!(window.find(1usize).visible());
            window.click("event-filter", cx);
            window.press("ctrl-a", cx);
            window.press("backspace", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(view.read(cx).event_tree.visible, [0, 1, 2]);
            assert_eq!(view.read(cx).pipeline.as_ref().unwrap().event_id, 43);
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.input("no matching name", cx);
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert!(view.read(cx).event_tree.visible.is_empty()));
        cx.update_window(handle.into(), |_, window, cx| {
            window.press("ctrl-a", cx);
            window.press("backspace", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).event_tree.visible, [0, 1, 2]));
        assert_eq!(*events.lock().unwrap(), vec![42, 77, 43]);
        cx.update_window(handle.into(), |_, window, cx| window.click(0usize, cx))
            .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("api-calls-tab", cx);
            window.click("api-event-0", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(view.read(cx).selected, Some(0));
            assert_eq!(view.read(cx).pipeline.as_ref().unwrap().event_id, 41);
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("parameters-tab", cx)
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let page = view.read(cx).structure.page.as_ref().unwrap();
            assert_eq!(page.event_id, 41);
            assert_eq!(page.children[0].name, "Topology");
            assert_eq!(page.children.len(), 128);
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("structure-next", cx)
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(view.read(cx).structure.offset, 128);
            assert_eq!(
                view.read(cx)
                    .structure
                    .page
                    .as_ref()
                    .unwrap()
                    .children
                    .len(),
                2
            );
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("structure-previous", cx)
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).structure.offset, 0));
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("structure-next", cx)
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("structure-child-0", cx)
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).structure.path, [128]));
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("structure-parent", cx)
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).structure.offset, 0));
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("structure-child-0", cx)
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(view.read(cx).structure.path, [0]);
            assert_eq!(
                view.read(cx)
                    .structure
                    .page
                    .as_ref()
                    .unwrap()
                    .node
                    .value
                    .as_deref(),
                Some("4")
            );
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("structure-parent", cx)
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert!(view.read(cx).structure.path.is_empty()));
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("structure-child-1", cx)
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("structure-load-bytes", cx)
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(view.read(cx).structure.bytes.as_ref().unwrap().len(), 256);
            assert_eq!(view.read(cx).structure.bytes_offset, 0);
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("structure-bytes-next", cx)
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(view.read(cx).structure.bytes.as_ref().unwrap().len(), 44);
            assert_eq!(view.read(cx).structure.bytes_offset, 256);
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("structure-bytes-previous", cx)
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).structure.bytes_offset, 0));
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("structure-parent", cx)
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("shaders-tab", cx);
            window.click("shader-0", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).shader_data.as_ref().unwrap().event_id, 41));
        assert_eq!(*events.lock().unwrap(), vec![42, 77, 43, 42, 41]);
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("events-tab", cx);
            window.click("toggle-api-events", cx);
            window.render_frame(cx);
            assert!(window.find("event-api-0-0").visible());
            window.click("event-api-0-0", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).pipeline.as_ref().unwrap().event_id, 41));
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("event-filter", cx);
            window.input("iAsEt", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).event_tree.visible, [3]));
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("event-api-0-0").visible());
            window.click("toggle-api-events", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert!(view.read(cx).event_tree.visible.is_empty()));
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("event-filter", cx);
            window.press("ctrl-a", cx);
            window.press("backspace", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).event_tree.visible, [0, 1, 2]));
        assert_eq!(*events.lock().unwrap(), vec![42, 77, 43, 42, 41, 41]);

        cx.update_window(handle.into(), |_, window, cx| {
            window.click("resources-tab", cx);
            window.click("resource-filter", cx);
            window.input("view", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).resource_visible, [1]));
        cx.update_window(handle.into(), |_, window, cx| {
            window.press("ctrl-a", cx);
            window.press("backspace", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(("resource", 0usize), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).usage_page.as_ref().unwrap().entries.len(), 2));
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(("resource-derived", 0usize), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).selected_resource, Some(1)));
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(("resource-parent", 0usize), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).selected_resource, Some(0)));
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(("resource-usage", 1usize), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(view.read(cx).tab == CaptureTab::Events);
            assert_eq!(view.read(cx).pipeline.as_ref().unwrap().event_id, 41);
            assert!(view.read(cx).event_tree.show_api());
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("event-api-0-0").visible());
        })
        .unwrap();

        // When the local D3D11 fixtures are configured, repeat the interaction against
        // a real capture and replay session, not the recording transport above.
        #[cfg(windows)]
        {
            let bridge = std::env::var_os("RENDERDOCX_TEST_BRIDGE");
            let fixture = std::env::var_os("RENDERDOCX_TEST_FIXTURE");
            assert_eq!(
                bridge.is_some(),
                fixture.is_some(),
                "configure both real-capture fixture paths together"
            );
            if let (Some(bridge), Some(fixture)) = (bridge, fixture) {
                use std::os::windows::process::CommandExt;
                let bridge = PathBuf::from(bridge);
                let dir = tempfile::tempdir().unwrap();
                let result = std::process::Command::new(fixture)
                    .arg(bridge.parent().unwrap().join("renderdoc.dll"))
                    .arg(dir.path().join("frame"))
                    .creation_flags(0x08000000)
                    .output()
                    .unwrap();
                assert!(
                    result.status.success(),
                    "fixture: {}",
                    String::from_utf8_lossy(&result.stderr)
                );
                let output = String::from_utf8(result.stdout).unwrap();
                let capture = PathBuf::from(
                    output
                        .lines()
                        .find_map(|line| line.strip_prefix("RENDERDOCX_CAPTURE="))
                        .expect("capture path"),
                );
                let settings = Settings {
                    replay_bridge: Some(bridge),
                    ..Settings::default()
                };
                let (real_session, real_snapshot) = Session::open(&settings, &capture).unwrap();
                let vertex_resource = real_snapshot
                    .buffers
                    .iter()
                    .find(|buffer| buffer.length == "28")
                    .and_then(|buffer| buffer.resource_index)
                    .expect("vertex resource");
                let color_index = real_snapshot
                    .textures
                    .iter()
                    .position(|texture| texture.width == 16 && texture.height == 16)
                    .expect("captured color target");
                let (draw_index, draw_event) = real_snapshot
                    .actions
                    .iter()
                    .enumerate()
                    .find(|(_, action)| action.name.contains("DrawIndexed"))
                    .map(|(index, action)| (index, action.event_id))
                    .expect("captured indexed draw");
                let (history_draw_index, history_draw_event) = real_snapshot
                    .actions
                    .iter()
                    .enumerate()
                    .filter(|(_, action)| action.name.contains("DrawIndexed"))
                    .nth(1)
                    .map(|(index, action)| (index, action.event_id))
                    .expect("history draw");
                let (scissor_draw_index, scissor_draw_event) = real_snapshot
                    .actions
                    .iter()
                    .enumerate()
                    .filter(|(_, action)| action.name.contains("DrawIndexed"))
                    .nth(2)
                    .map(|(index, action)| (index, action.event_id))
                    .expect("scissor draw");
                let (action_index, api_index, event_id) = real_snapshot
                    .actions
                    .iter()
                    .enumerate()
                    .find_map(|(action_index, action)| {
                        action
                            .api_events
                            .iter()
                            .enumerate()
                            .find(|(_, event)| {
                                event.event_id != action.event_id
                                    && event.name.contains("IASetPrimitiveTopology")
                            })
                            .map(|(api_index, event)| (action_index, api_index, event.event_id))
                    })
                    .expect("captured non-action topology event");
                let (update_row, update_event) = real_snapshot
                    .actions
                    .iter()
                    .enumerate()
                    .find_map(|(action_index, action)| {
                        action
                            .api_events
                            .iter()
                            .enumerate()
                            .find(|(_, event)| event.name.contains("UpdateSubresource"))
                            .map(|(api_index, event)| {
                                let row: gpui::ElementId = if event.event_id == action.event_id {
                                    action_index.into()
                                } else {
                                    SharedString::from(format!(
                                        "event-api-{action_index}-{api_index}"
                                    ))
                                    .into()
                                };
                                (row, event.event_id)
                            })
                    })
                    .expect("captured buffer update event");
                cx.update(|cx| {
                    view.update(cx, |view, cx| {
                        view.open_revision += 1;
                        view.event_tree.reset(&real_snapshot.actions);
                        view.event_tree.set_show_api(false);
                        view.snapshot = Some(real_snapshot);
                        view.session = Some(real_session);
                        view.selected = None;
                        view.pipeline = None;
                        view.tab = CaptureTab::Events;
                        view.selected_resource = None;
                        view.usage_page = None;
                        cx.notify();
                    })
                });
                cx.update_window(handle.into(), |_, window, cx| {
                    window.render_frame(cx);
                    window.click("resources-tab", cx);
                    window.click("resource-filter", cx);
                    window.press("ctrl-a", cx);
                    window.input(&format!("#{vertex_resource}"), cx);
                })
                .unwrap();
                cx.run_until_parked();
                cx.update_window(handle.into(), |_, window, cx| {
                    window.render_frame(cx);
                    window.click(("resource", vertex_resource), cx);
                })
                .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                let usage_row = loop {
                    cx.run_until_parked();
                    if let Some(row) = cx.update(|cx| {
                        view.read(cx).usage_page.as_ref().and_then(|page| {
                            page.entries
                                .iter()
                                .position(|entry| entry.event_id == draw_event && entry.usage == 1)
                        })
                    }) {
                        break row;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "real resource usage did not load: {:?}",
                        cx.update(|cx| view.read(cx).status.clone())
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                };
                cx.update_window(handle.into(), |_, window, cx| {
                    window.render_frame(cx);
                    window.click(("resource-usage", usage_row), cx);
                })
                .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                loop {
                    cx.run_until_parked();
                    if cx.update(|cx| {
                        view.read(cx)
                            .pipeline
                            .as_ref()
                            .is_some_and(|state| state.event_id == draw_event)
                    }) {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "real resource usage event was not replayed"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                cx.update_window(handle.into(), |_, window, cx| {
                    window.render_frame(cx);
                    assert!(window.find(draw_index).visible());
                })
                .unwrap();
                let api_row = format!("event-api-{action_index}-{api_index}");
                cx.update_window(handle.into(), |_, window, cx| {
                    window.render_frame(cx);
                    window.click("toggle-api-events", cx);
                    window.click("event-filter", cx);
                    window.input("IASetPrimitiveTopology", cx);
                    window.render_frame(cx);
                    window.click(SharedString::from(api_row), cx);
                })
                .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                loop {
                    cx.run_until_parked();
                    if cx.update(|cx| {
                        view.read(cx)
                            .pipeline
                            .as_ref()
                            .is_some_and(|state| state.event_id == event_id)
                    }) {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "real API event selection did not complete"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                cx.update_window(handle.into(), |_, window, cx| {
                    window.click("events-tab", cx);
                    window.click("event-filter", cx);
                    window.press("ctrl-a", cx);
                    window.input("UpdateSubresource", cx);
                })
                .unwrap();
                cx.run_until_parked();
                cx.update_window(handle.into(), |_, window, cx| {
                    window.render_frame(cx);
                    window.click(update_row, cx);
                })
                .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                loop {
                    cx.run_until_parked();
                    if cx.update(|cx| {
                        view.read(cx)
                            .pipeline
                            .as_ref()
                            .is_some_and(|state| state.event_id == update_event)
                    }) {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "real upload event selection did not complete"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                cx.update_window(handle.into(), |_, window, cx| {
                    window.click("parameters-tab", cx)
                })
                .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                let source_index = loop {
                    cx.run_until_parked();
                    if let Some(index) = cx.update(|cx| {
                        view.read(cx).structure.page.as_ref().and_then(|page| {
                            page.children
                                .iter()
                                .position(|node| node.name == "pSrcData" && node.basic_type == 4)
                        })
                    }) {
                        break index;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "real upload buffer parameter did not load"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                };
                cx.update_window(handle.into(), |_, window, cx| {
                    window.click(
                        SharedString::from(format!("structure-child-{source_index}")),
                        cx,
                    );
                })
                .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                loop {
                    cx.run_until_parked();
                    if cx.update(|cx| view.read(cx).structure.path == [source_index as u64]) {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "real upload buffer node did not load"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                cx.update_window(handle.into(), |_, window, cx| {
                    window.click("structure-load-bytes", cx)
                })
                .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                loop {
                    cx.run_until_parked();
                    if cx.update(|cx| {
                        view.read(cx)
                            .structure
                            .bytes
                            .as_ref()
                            .is_some_and(|bytes| bytes.starts_with(&[0, 0, 0, 0, 0, 0, 0x40, 0x40]))
                    }) {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "real structured buffer bytes did not load"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                cx.update_window(handle.into(), |_, window, cx| {
                    window.click("events-tab", cx);
                    window.click("event-filter", cx);
                    window.press("ctrl-a", cx);
                    window.input("DrawIndexed", cx);
                })
                .unwrap();
                cx.run_until_parked();
                cx.update_window(handle.into(), |_, window, cx| {
                    window.render_frame(cx);
                    window.click(draw_index, cx);
                })
                .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                loop {
                    cx.run_until_parked();
                    if cx.update(|cx| {
                        view.read(cx)
                            .pipeline
                            .as_ref()
                            .is_some_and(|state| state.event_id == draw_event)
                    }) {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "real draw selection did not complete"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                cx.update_window(handle.into(), |_, window, cx| {
                    window.click("textures-tab", cx);
                    window.click(SharedString::from(format!("texture-{color_index}")), cx);
                    window.scroll(
                        "capture-details-scroll",
                        ScrollDelta::Pixels(point(px(0.), px(-800.))),
                        cx,
                    );
                    window.click("pixel-pick", cx);
                })
                .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                loop {
                    cx.run_until_parked();
                    if cx.update(|cx| {
                        view.read(cx)
                            .pixel
                            .as_ref()
                            .is_some_and(|pixel| pixel.event_id == draw_event)
                    }) {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "real pixel did not load: {:?}",
                        cx.update(|cx| view.read(cx).status.clone())
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                cx.update(|cx| {
                    let pixel = view.read(cx).pixel.as_ref().unwrap();
                    let values: Vec<f32> = pixel
                        .float_values
                        .iter()
                        .map(|value| value.parse().unwrap())
                        .collect();
                    for (actual, expected) in values.iter().zip([0.25, 0.5, 0.75, 1.0]) {
                        assert!((actual - expected).abs() <= 1.0 / 255.0);
                    }
                });
                cx.update_window(handle.into(), |_, window, cx| {
                    window.scroll(
                        "capture-details-scroll",
                        ScrollDelta::Pixels(point(px(0.), px(-700.))),
                        cx,
                    );
                    window.click("histogram-analyze", cx);
                })
                .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                loop {
                    cx.run_until_parked();
                    if cx.update(|cx| {
                        view.read(cx)
                            .statistics
                            .as_ref()
                            .is_some_and(|stats| stats.event_id == draw_event)
                    }) {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "real texture statistics did not load: {:?}",
                        cx.update(|cx| view.read(cx).status.clone())
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                cx.update(|cx| {
                    let statistics = view.read(cx).statistics.as_ref().unwrap();
                    assert_eq!(statistics.histogram.len(), 256);
                    assert!(statistics.histogram.iter().any(|count| *count > 0));
                });
                cx.update_window(handle.into(), |_, window, cx| {
                    window.click("vertex-input-tab", cx)
                })
                .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                loop {
                    cx.run_until_parked();
                    if cx.update(|cx| {
                        view.read(cx)
                            .input_page
                            .as_ref()
                            .is_some_and(|page| page.event_id == draw_event)
                    }) {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "real vertex input did not load: {:?}",
                        cx.update(|cx| view.read(cx).status.clone())
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                cx.update(|cx| {
                    let page = view.read(cx).input_page.as_ref().unwrap();
                    let position = page.rows[0]
                        .attributes
                        .iter()
                        .find(|a| a.name == "POSITION")
                        .unwrap();
                    assert_eq!(position.values.as_ref().unwrap(), &["3", "3"]);
                });
                cx.update_window(handle.into(), |_, window, cx| {
                    window.render_frame(cx);
                    assert!(window.find(("vertex-input-row", 0usize)).visible());
                    window.click(("vertex-input-row", 0usize), cx);
                    window.scroll(
                        "capture-details-scroll",
                        ScrollDelta::Pixels(point(px(0.), px(2000.))),
                        cx,
                    );
                    window.render_frame(cx);
                    window.click("vertex-debug-start", cx);
                })
                .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                loop {
                    cx.run_until_parked();
                    if cx.update(|cx| view.read(cx).debug_view.is_some()) {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "real vertex debug did not finish: {:?}",
                        cx.update(|cx| view.read(cx).status.clone())
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                cx.update(|cx| {
                    let debug = view.read(cx).debug_view.as_ref().unwrap();
                    assert!(matches!(
                        debug.context,
                        ShaderDebugContext::Vertex {
                            vertex: 0,
                            instance: 0,
                            index: 0
                        }
                    ));
                    assert_eq!(debug.trace.event_id, draw_event);
                    assert!(debug.states.len() > 1);
                    assert!(debug.disassembly.contains("vs_4_0"));
                    assert!(debug.states.iter().any(|state| !state.changes.is_empty()));
                    assert_eq!(debug.position, 0);
                });
                cx.update_window(handle.into(), |_, window, cx| {
                    window.render_frame(cx);
                    window.click("vertex-debug-next", cx);
                })
                .unwrap();
                cx.update(|cx| assert_eq!(view.read(cx).debug_view.as_ref().unwrap().position, 1));
                cx.update_window(handle.into(), |_, window, cx| {
                    window.click("vertex-debug-previous", cx);
                })
                .unwrap();
                cx.update(|cx| assert_eq!(view.read(cx).debug_view.as_ref().unwrap().position, 0));
                cx.update_window(handle.into(), |_, window, cx| {
                    window.click("mesh-tab", cx);
                })
                .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                loop {
                    cx.run_until_parked();
                    if cx.update(|cx| {
                        view.read(cx)
                            .mesh
                            .info
                            .as_ref()
                            .is_some_and(|info| info.event_id == draw_event)
                    }) {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "real mesh output did not load: {:?}",
                        cx.update(|cx| view.read(cx).status.clone())
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                cx.update(|cx| {
                    let mesh = &view.read(cx).mesh;
                    let positions = mesh_positions(
                        mesh.info.as_ref().unwrap(),
                        mesh.bytes.as_ref().unwrap(),
                        0,
                    )
                    .unwrap();
                    assert_eq!(positions[0], (0, vec![3.0, 3.0, 0.0, 1.0]));
                });
                cx.update_window(handle.into(), |_, window, cx| {
                    window.render_frame(cx);
                    assert!(window.find(("mesh-row", 0usize)).visible());
                })
                .unwrap();
                cx.update_window(handle.into(), |_, window, cx| {
                    window.click("events-tab", cx);
                    window.click("event-filter", cx);
                    window.press("ctrl-a", cx);
                    window.input("DrawIndexed", cx);
                })
                .unwrap();
                cx.run_until_parked();
                cx.update_window(handle.into(), |_, window, cx| {
                    window.render_frame(cx);
                    window.click(scissor_draw_index, cx);
                })
                .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                loop {
                    cx.run_until_parked();
                    if cx.update(|cx| {
                        view.read(cx)
                            .pipeline
                            .as_ref()
                            .is_some_and(|state| state.event_id == scissor_draw_event)
                    }) {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "scissored draw did not replay"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                cx.update_window(handle.into(), |_, window, cx| {
                    window.click("textures-tab", cx);
                    window.click(SharedString::from(format!("texture-{color_index}")), cx);
                    window.scroll(
                        "capture-details-scroll",
                        ScrollDelta::Pixels(point(px(0.), px(2000.))),
                        cx,
                    );
                    window.scroll(
                        "capture-details-scroll",
                        ScrollDelta::Pixels(point(px(0.), px(-550.))),
                        cx,
                    );
                    window.click("pixel-history", cx);
                })
                .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                let (failed_row, passed_row) = loop {
                    cx.run_until_parked();
                    if let Some(rows) = cx.update(|cx| {
                        view.read(cx).pixel_history.as_ref().and_then(|page| {
                            Some((
                                page.entries.iter().position(|entry| {
                                    entry.event_id == scissor_draw_event
                                        && entry.tests.scissor_clipped
                                })?,
                                page.entries.iter().position(|entry| {
                                    entry.event_id == history_draw_event && entry.passed
                                })?,
                            ))
                        })
                    }) {
                        break rows;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "real pixel history did not load: {:?}",
                        cx.update(|cx| view.read(cx).status.clone())
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                };
                cx.update_window(handle.into(), |_, window, cx| {
                    window.render_frame(cx);
                    window.scroll(
                        "capture-details-scroll",
                        ScrollDelta::Pixels(point(px(0.), px(-500.))),
                        cx,
                    );
                    window.click(("pixel-history-row", failed_row), cx);
                })
                .unwrap();
                cx.update(|cx| {
                    assert!(
                        view.read(cx).pixel_history.as_ref().unwrap().entries
                            [view.read(cx).history_selected.unwrap()]
                        .tests
                        .scissor_clipped
                    )
                });
                cx.update_window(handle.into(), |_, window, cx| {
                    window.render_frame(cx);
                    window.click(("pixel-history-row", passed_row), cx);
                    for _ in 0..20 {
                        let target = window.find("pixel-history-jump");
                        if target.visible() {
                            break;
                        }
                        let viewport = window.find("capture-details-scroll").bounds();
                        let delta = if target.bounds().origin.y < viewport.origin.y {
                            100.0
                        } else {
                            -100.0
                        };
                        window.scroll(
                            "capture-details-scroll",
                            ScrollDelta::Pixels(point(px(0.), px(delta))),
                            cx,
                        );
                    }
                    window.click("pixel-history-jump", cx);
                })
                .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                loop {
                    cx.run_until_parked();
                    if cx.update(|cx| {
                        view.read(cx)
                            .pipeline
                            .as_ref()
                            .is_some_and(|state| state.event_id == history_draw_event)
                    }) {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "pixel history navigation did not replay event"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                cx.update_window(handle.into(), |_, window, cx| {
                    window.render_frame(cx);
                    assert!(window.find(history_draw_index).visible());
                    window.click("textures-tab", cx);
                    window.click(SharedString::from(format!("texture-{color_index}")), cx);
                    window.scroll(
                        "capture-details-scroll",
                        ScrollDelta::Pixels(point(px(0.), px(2000.))),
                        cx,
                    );
                    window.render_frame(cx);
                    for _ in 0..20 {
                        let target = window.find("pixel-history");
                        if target.visible() {
                            break;
                        }
                        window.scroll(
                            "capture-details-scroll",
                            ScrollDelta::Pixels(point(px(0.), px(-100.))),
                            cx,
                        );
                    }
                    window.click("pixel-history", cx);
                })
                .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                let (row, primitive, x, y) = loop {
                    cx.run_until_parked();
                    if let Some(found) = cx.update(|cx| {
                        view.read(cx).pixel_history.as_ref().and_then(|page| {
                            let row = page.entries.iter().position(|entry| {
                                entry.event_id == history_draw_event && entry.passed
                            })?;
                            Some((row, page.entries[row].primitive, page.x, page.y))
                        })
                    }) {
                        break found;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "pixel history for debugging did not load: {:?}",
                        cx.update(|cx| view.read(cx).status.clone())
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                };
                cx.update_window(handle.into(), |_, window, cx| {
                    window.render_frame(cx);
                    for _ in 0..30 {
                        let target = window.find(("pixel-history-row", row));
                        if target.visible() {
                            break;
                        }
                        let viewport = window.find("capture-details-scroll").bounds();
                        let delta = if target.bounds().origin.y < viewport.origin.y {
                            100.0
                        } else {
                            -100.0
                        };
                        window.scroll(
                            "capture-details-scroll",
                            ScrollDelta::Pixels(point(px(0.), px(delta))),
                            cx,
                        );
                    }
                    window.click(("pixel-history-row", row), cx);
                    for _ in 0..20 {
                        let target = window.find("pixel-history-debug");
                        if target.visible() {
                            break;
                        }
                        let viewport = window.find("capture-details-scroll").bounds();
                        let delta = if target.bounds().origin.y < viewport.origin.y {
                            100.0
                        } else {
                            -100.0
                        };
                        window.scroll(
                            "capture-details-scroll",
                            ScrollDelta::Pixels(point(px(0.), px(delta))),
                            cx,
                        );
                    }
                    window.click("pixel-history-debug", cx);
                })
                .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                loop {
                    cx.run_until_parked();
                    if cx.update(|cx| view.read(cx).debug_view.is_some()) {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "pixel debug did not finish: {:?}",
                        cx.update(|cx| view.read(cx).status.clone())
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                cx.update(|cx| {
                    let debug = view.read(cx).debug_view.as_ref().unwrap();
                    let ShaderDebugContext::Pixel(selection) = debug.context else {
                        panic!("expected pixel trace")
                    };
                    assert_eq!((selection.x, selection.y), (x, y));
                    assert_eq!(
                        selection.primitive,
                        (primitive != u32::MAX).then_some(primitive)
                    );
                    assert_eq!(debug.trace.event_id, history_draw_event);
                    assert_eq!(debug.trace.stage, 4);
                    assert!(debug.states.len() > 1);
                    assert!(debug.disassembly.contains("ps_4_0"));
                });
                cx.update_window(handle.into(), |_, window, cx| {
                    for _ in 0..30 {
                        let target = window.find("pixel-debug-start");
                        if target.visible() {
                            break;
                        }
                        let viewport = window.find("capture-details-scroll").bounds();
                        let delta = if target.bounds().origin.y < viewport.origin.y {
                            100.0
                        } else {
                            -100.0
                        };
                        window.scroll(
                            "capture-details-scroll",
                            ScrollDelta::Pixels(point(px(0.), px(delta))),
                            cx,
                        );
                    }
                    window.click("pixel-debug-start", cx);
                })
                .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                loop {
                    cx.run_until_parked();
                    if cx.update(|cx| view.read(cx).debug_view.as_ref().is_some_and(|debug| matches!(debug.context, ShaderDebugContext::Pixel(selection) if selection.primitive.is_none()))) { break; }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "default pixel debug did not finish: {:?}",
                        cx.update(|cx| view.read(cx).status.clone())
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                cx.update_window(handle.into(), |_, window, cx| window.click("mesh-tab", cx))
                    .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                loop {
                    cx.run_until_parked();
                    if cx.update(|cx| {
                        view.read(cx)
                            .mesh
                            .info
                            .as_ref()
                            .is_some_and(|mesh| mesh.event_id == history_draw_event)
                    }) {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "replay did not recover after pixel debug: {:?}",
                        cx.update(|cx| view.read(cx).status.clone())
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
            if let Some(fixture) = std::env::var_os("RENDERDOCX_TEST_OPENGL_FIXTURE") {
                use std::os::windows::process::CommandExt;
                let bridge = PathBuf::from(
                    std::env::var_os("RENDERDOCX_TEST_BRIDGE")
                        .expect("OpenGL fixture requires bridge"),
                );
                let dir = tempfile::tempdir().unwrap();
                let output = std::process::Command::new(fixture)
                    .arg(bridge.parent().unwrap().join("renderdoc.dll"))
                    .arg(dir.path().join("frame"))
                    .creation_flags(0x08000000)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "OpenGL fixture: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                let stdout = String::from_utf8(output.stdout).unwrap();
                let capture = PathBuf::from(
                    stdout
                        .lines()
                        .find_map(|line| line.strip_prefix("RENDERDOCX_CAPTURE="))
                        .unwrap(),
                );
                let settings = Settings {
                    replay_bridge: Some(bridge),
                    ..Settings::default()
                };
                let (session, snapshot) = Session::open(&settings, &capture).unwrap();
                let (draw_index, draw_event) = snapshot
                    .actions
                    .iter()
                    .enumerate()
                    .find(|(_, action)| action.name.contains("glDrawArrays"))
                    .map(|(index, action)| (index, action.event_id))
                    .unwrap();
                cx.update(|cx| {
                    view.update(cx, |view, cx| {
                        view.open_revision += 1;
                        view.event_tree.reset(&snapshot.actions);
                        view.snapshot = Some(snapshot);
                        view.session = Some(session);
                        view.selected = None;
                        view.pipeline = None;
                        view.input_page = None;
                        view.tab = CaptureTab::Events;
                        cx.notify();
                    })
                });
                cx.update_window(handle.into(), |_, window, cx| {
                    window.render_frame(cx);
                    window.click("event-filter", cx);
                    window.press("ctrl-a", cx);
                    window.input("glDrawArrays", cx);
                })
                .unwrap();
                cx.run_until_parked();
                cx.update_window(handle.into(), |_, window, cx| {
                    window.render_frame(cx);
                    window.click(draw_index, cx);
                })
                .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                loop {
                    cx.run_until_parked();
                    if cx.update(|cx| {
                        view.read(cx)
                            .pipeline
                            .as_ref()
                            .is_some_and(|state| state.event_id == draw_event)
                    }) {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "OpenGL draw selection did not complete"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                cx.update_window(handle.into(), |_, window, cx| {
                    window.click("vertex-input-tab", cx)
                })
                .unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                loop {
                    cx.run_until_parked();
                    if cx.update(|cx| {
                        view.read(cx)
                            .input_page
                            .as_ref()
                            .is_some_and(|page| page.event_id == draw_event)
                    }) {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "OpenGL inputs did not load: {:?}",
                        cx.update(|cx| view.read(cx).status.clone())
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                cx.update(|cx| {
                    let page = view.read(cx).input_page.as_ref().unwrap();
                    let tint = page.rows[0]
                        .attributes
                        .iter()
                        .find(|a| a.name == "tint")
                        .unwrap();
                    assert_eq!(tint.values.as_ref().unwrap(), &["0.25", "0.5", "0.75", "1"]);
                });
                cx.update_window(handle.into(), |_, window, cx| {
                    window.render_frame(cx);
                    assert!(window.find(("vertex-input-row", 0usize)).visible());
                })
                .unwrap();
            }
        }
    }
}
