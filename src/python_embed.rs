//! Embedded CPython execution via the mature `pyo3` crate, replacing the
//! external-process Python path for interactive scripting and UI-extension
//! lifecycle calls. This is Rust-native orchestration; no C++ bridge involved.

// pyrenderdoc-compatible accessor names intentionally mirror upstream
// (CurEvent, CurAction, GetTextures, ...).
#![allow(non_snake_case)]

use pyo3::{prelude::*, types::PyDict, Python};
use std::{
    ffi::CString,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering},
        Mutex,
    },
};

// debugpy in-process adapter state, mirroring PythonContext::LaunchDebugger.
static DEBUGPY_LISTENING: AtomicBool = AtomicBool::new(false);
static DEBUGPY_ENABLED: AtomicBool = AtomicBool::new(false);
static DEBUGPY_PORT: AtomicU16 = AtomicU16::new(5678);

// Shader save/revert callbacks registered from Python, keyed by
// (stage, event). The GUI invokes them after a successful apply/revert.
type ShaderCallbacks = (
    Option<pyo3::Py<pyo3::PyAny>>,
    Option<pyo3::Py<pyo3::PyAny>>,
);
static SHADER_CALLBACKS: Mutex<Option<std::collections::HashMap<(u32, Option<u32>), ShaderCallbacks>>> =
    Mutex::new(None);

// CPython's interpreter and sys.stdout redirection are process-global; serialize
// all embedded calls so concurrent scripts cannot clobber each other's capture.
static PY_LOCK: Mutex<()> = Mutex::new(());

// Persistent globals for the embedded REPL: variables defined by one console
// entry remain available to the next, like an interactive interpreter session.
static REPL_GLOBALS: Mutex<Option<pyo3::Py<PyDict>>> = Mutex::new(None);

// Replay session exposed to script helpers such as renderdocx.pipeline(event).
static ACTIVE_SESSION: Mutex<Option<crate::replay::Session>> = Mutex::new(None);

// Snapshot pieces needed by event formatting and texture queries. Snapshot
// itself is not Clone, so only the small vectors are copied.
static ACTIVE_RESOURCES: Mutex<Option<Vec<crate::replay::ResourceInfo>>> = Mutex::new(None);
static ACTIVE_BUFFERS: Mutex<Option<Vec<crate::replay::Buffer>>> = Mutex::new(None);
static ACTIVE_TEXTURES: Mutex<Option<Vec<crate::replay::Texture>>> = Mutex::new(None);
static ACTIVE_ACTIONS: Mutex<Option<Vec<crate::replay::Action>>> = Mutex::new(None);
static ACTIVE_LANGUAGE: Mutex<crate::i18n::Language> = Mutex::new(crate::i18n::Language::English);
// Complete snapshot document for renderdocx.snapshot(). Serialized once when a
// capture is bound so scripts can read any field without extra bindings.
static ACTIVE_SNAPSHOT: Mutex<Option<serde_json::Value>> = Mutex::new(None);
// Last event selected or queried by a helper; backs CurEvent()/CurAction().
static ACTIVE_EVENT: AtomicU32 = AtomicU32::new(0);
static ACTIVE_CAPTURE_PATH: Mutex<Option<String>> = Mutex::new(None);
// UI panel requests queued by script Show* calls; the GUI drains them after a
// script finishes (scripts run on a background executor).
static UI_REQUESTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Drain queued `Show*` requests for the GUI to apply.
pub fn take_ui_requests() -> Vec<String> {
    std::mem::take(
        &mut *UI_REQUESTS
            .lock()
            .unwrap_or_else(|error| error.into_inner()),
    )
}

fn queue_ui_request(panel: &str) {
    UI_REQUESTS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push(panel.to_owned());
}

macro_rules! viewer_accessors {
    ($(($show:ident, $has:ident, $panel:literal, $doc:literal)),* $(,)?) => {
        $(
            #[doc = $doc]
            #[pyo3::pyfunction]
            #[allow(non_snake_case)]
            fn $show() {
                queue_ui_request($panel);
            }

            #[doc = concat!("`", stringify!($has), "()` -> whether the panel exists.")]
            #[pyo3::pyfunction]
            #[allow(non_snake_case)]
            fn $has() -> bool {
                true
            }
        )*
    };
}

viewer_accessors!(
    (ShowEventBrowser, HasEventBrowser, "events", "`ShowEventBrowser()` -> open the event browser."),
    (ShowAPIInspector, HasAPIInspector, "parameters", "`ShowAPIInspector()` -> open the API call inspector."),
    (ShowTextureViewer, HasTextureViewer, "textures", "`ShowTextureViewer()` -> open the texture viewer."),
    (ShowPipelineViewer, HasPipelineViewer, "parameters", "`ShowPipelineViewer()` -> open the pipeline state view."),
    (ShowMeshPreview, HasMeshPreview, "mesh", "`ShowMeshPreview()` -> open the mesh preview."),
    (ShowResourceInspector, HasResourceInspector, "resources", "`ShowResourceInspector()` -> open the resource inspector."),
    (ShowDebugMessageView, HasDebugMessageView, "messages", "`ShowDebugMessageView()` -> open the debug messages view."),
    (ShowAnnotationViewer, HasAnnotationViewer, "annotations", "`ShowAnnotationViewer()` -> open notes/bookmarks."),
    (ShowPerformanceCounterViewer, HasPerformanceCounterViewer, "counters", "`ShowPerformanceCounterViewer()` -> open performance counters."),
    (ShowStatisticsViewer, HasStatisticsViewer, "statistics", "`ShowStatisticsViewer()` -> open frame statistics."),
    (ShowTimelineBar, HasTimelineBar, "events", "`ShowTimelineBar()` -> open the event timeline."),
);

/// `ShowDiagnosticLogView()` -> open the renderdoc log panel.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn ShowDiagnosticLogView() {
    queue_ui_request("log");
}

/// `HasDiagnosticLogView()` -> the log panel is always available.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn HasDiagnosticLogView() -> bool {
    true
}

/// `ShowPythonShell()` -> the Python console is part of the workspace.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn ShowPythonShell() {
    queue_ui_request("python");
}

/// `HasPythonShell()` -> the embedded CPython console is available.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn HasPythonShell() -> bool {
    true
}

#[derive(Default)]
struct ViewerState {
    filter: String,
    row: u64,
    column: u64,
    current_resource: Option<usize>,
    mip: u32,
    slice: u32,
    sample: u32,
    overlay: u32,
    zoom: f32,
    channels: u32,
    auto_fit: bool,
    use_custom_action_names: bool,
    show_parameter_names: bool,
    show_all_parameters: bool,
    empty_regions_visible: bool,
    duration_column: bool,
    annotation_column: bool,
    highlighted_annotation: String,
    current_instance: u32,
    current_view: u32,
    preview_stage: u32,
    picked: Option<(u32, u32)>,
    histogram_min: f32,
    histogram_max: f32,
    highlighted_resource: Option<usize>,
    highlighted_history: Option<usize>,
    current_step: u32,
    watches: Vec<String>,
    errors: String,
    script_text: String,
    script_name: String,
    output_filter: Option<String>,
    extension_output_filter: Option<String>,
    show_output: bool,
    show_repl: bool,
    show_help: bool,
    save_callback: Option<pyo3::Py<pyo3::PyAny>>,
    revert_callback: Option<pyo3::Py<pyo3::PyAny>>,
    filter_functions: Vec<String>,
    revealed_parameter: String,
    texture_type_cast: Option<u32>,
}

/// Lightweight handle for a viewer panel, returned by `Get*Viewer()`.
#[pyo3::pyclass]
struct ViewerHandle {
    panel: String,
    state: Mutex<ViewerState>,
}

#[pyo3::pymethods]
impl ViewerHandle {
    /// `Show()` -> open the mapped panel through the GUI request queue.
    fn Show(&self) {
        queue_ui_request(&self.panel);
    }

    /// `IsAvailable()` -> this panel exists in RenderDocX.
    fn IsAvailable(&self) -> bool {
        true
    }

    /// `Name()` -> internal panel key (also useful in tests).
    fn Name(&self) -> String {
        self.panel.clone()
    }

    fn shader_stage(&self) -> Option<u32> {
        self.panel
            .strip_prefix("edit_shader:")
            .or_else(|| self.panel.strip_prefix("shader:"))
            .and_then(|stage| stage.parse::<u32>().ok())
    }

    /// `Widget()` -> no PySide2 widget; the GPUI panel is native.
    #[allow(non_snake_case)]
    fn Widget(&self, py: Python<'_>) -> pyo3::Py<pyo3::PyAny> {
        py.None()
    }

    /// `GetEventName(eventId)` -> formatted event name/parameters.
    #[allow(non_snake_case)]
    fn GetEventName(&self, event: u32) -> pyo3::PyResult<String> {
        let session = active_session()?;
        crate::event_parameters::format_event(
            &session,
            event,
            &active_resources(),
            &active_buffers(),
            active_language(),
            false,
            false,
        )
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)
    }

    /// `GetActionForEID(eventId)` -> containing action, or None.
    #[allow(non_snake_case)]
    fn GetActionForEID(
        &self,
        py: Python<'_>,
        event: u32,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match find_action_for_event(event) {
            Some(action) => to_py(py, &action),
            None => Ok(py.None()),
        }
    }

    /// `GetAPIEventForEID(eventId)` -> API event entry, or None.
    #[allow(non_snake_case)]
    fn GetAPIEventForEID(
        &self,
        py: Python<'_>,
        event: u32,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let api_event = active_actions().into_iter().find_map(|action| {
            action
                .api_events
                .into_iter()
                .find(|api_event| api_event.event_id == event)
        });
        match api_event {
            Some(api_event) => to_py(py, &api_event),
            None => Ok(py.None()),
        }
    }

    /// `IsAPIEventVisible(eventId)` -> whether the event is present in the
    /// bound capture.
    #[allow(non_snake_case)]
    fn IsAPIEventVisible(&self, event: u32) -> bool {
        active_actions().iter().any(|action| {
            action.event_id == event
                || action
                    .api_events
                    .iter()
                    .any(|api_event| api_event.event_id == event)
        })
    }

    /// `Inspect(index)` -> open the resource inspector at a resource index.
    #[allow(non_snake_case)]
    fn Inspect(&self, index: usize) {
        queue_ui_request(&format!("resource:{index}"));
    }

    /// `GetComments()` -> capture notes.
    #[allow(non_snake_case)]
    fn GetComments(&self) -> pyo3::PyResult<String> {
        GetNotes()
    }

    /// `SetComments(text)` -> replace capture notes.
    #[allow(non_snake_case)]
    fn SetComments(&self, text: String) -> pyo3::PyResult<()> {
        SetNotes(text)
    }

    /// `Refresh()` -> re-open the mapped panel.
    #[allow(non_snake_case)]
    fn Refresh(&self) {
        queue_ui_request(&self.panel);
    }

    /// `RevealAnnotation()` -> open the annotation panel.
    #[allow(non_snake_case)]
    fn RevealAnnotation(&self) {
        queue_ui_request("annotations");
    }

    /// `ShowMeshData()` -> open the mesh panel.
    #[allow(non_snake_case)]
    fn ShowMeshData(&self) {
        queue_ui_request("mesh");
    }

    /// `SetCurrentFilterText(text)` -> store the event-browser filter text.
    #[allow(non_snake_case)]
    fn SetCurrentFilterText(&self, text: String) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .filter = text;
    }

    /// `GetCurrentFilterText()` -> stored event-browser filter text.
    #[allow(non_snake_case)]
    fn GetCurrentFilterText(&self) -> String {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .filter
            .clone()
    }

    /// `ScrollToRow(row)` -> store the requested buffer row.
    #[allow(non_snake_case)]
    fn ScrollToRow(&self, row: u64) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .row = row;
    }

    /// `ScrollToColumn(column)` -> store the requested buffer column.
    #[allow(non_snake_case)]
    fn ScrollToColumn(&self, column: u64) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .column = column;
    }

    /// `GetScrollRow()` -> stored buffer row.
    #[allow(non_snake_case)]
    fn GetScrollRow(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .row
    }

    /// `GetScrollColumn()` -> stored buffer column.
    #[allow(non_snake_case)]
    fn GetScrollColumn(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .column
    }

    /// `ViewTexture(index, type_cast=None, focus=True)` -> open the texture
    /// panel and remember the index/optional cast.
    #[allow(non_snake_case)]
    #[pyo3(signature = (index, type_cast = None, focus = true))]
    fn ViewTexture(&self, index: usize, type_cast: Option<u32>, focus: bool) {
        let _ = focus;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.current_resource = Some(index);
        state.texture_type_cast = type_cast;
        drop(state);
        queue_ui_request("textures");
    }

    /// `GetCurrentResource()` -> remembered texture/resource index.
    #[allow(non_snake_case)]
    fn GetCurrentResource(&self) -> Option<usize> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .current_resource
    }

    /// `SetSelectedSubresource(mip, slice, sample)` -> remember the subresource.
    #[allow(non_snake_case)]
    fn SetSelectedSubresource(&self, mip: u32, slice: u32, sample: u32) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.mip = mip;
        state.slice = slice;
        state.sample = sample;
    }

    /// `GetSelectedSubresource()` -> `{mip, slice, sample}`.
    #[allow(non_snake_case)]
    fn GetSelectedSubresource(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        to_py(
            py,
            &serde_json::json!({
                "mip": state.mip,
                "slice": state.slice,
                "sample": state.sample,
            }),
        )
    }

    /// `SetTextureOverlay(overlay)` -> remember the overlay mode.
    #[allow(non_snake_case)]
    fn SetTextureOverlay(&self, overlay: u32) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .overlay = overlay;
    }

    /// `GetTextureOverlay()` -> remembered overlay mode.
    #[allow(non_snake_case)]
    fn GetTextureOverlay(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .overlay
    }

    /// `SetZoomLevel(autofit, zoom)` -> remember auto-fit and the texture zoom.
    #[allow(non_snake_case)]
    fn SetZoomLevel(&self, autofit: bool, zoom: f32) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.auto_fit = autofit;
        state.zoom = zoom;
    }

    /// `GetTextureDisplay()` -> current texture display settings.
    #[allow(non_snake_case)]
    fn GetTextureDisplay(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        to_py(
            py,
            &serde_json::json!({
                "resource": state.current_resource,
                "typeCast": state.texture_type_cast,
                "mip": state.mip,
                "slice": state.slice,
                "sample": state.sample,
                "overlay": state.overlay,
                "zoom": state.zoom,
                "autoFit": state.auto_fit,
                "channels": [
                    state.channels & 1 != 0,
                    state.channels & 2 != 0,
                    state.channels & 4 != 0,
                    state.channels & 8 != 0,
                ],
            }),
        )
    }

    /// `GetZoomLevel()` -> remembered texture zoom.
    #[allow(non_snake_case)]
    fn GetZoomLevel(&self) -> f32 {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .zoom
    }

    /// `IsZoomAutoFit()` -> whether auto-fit is enabled.
    #[allow(non_snake_case)]
    fn IsZoomAutoFit(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .auto_fit
    }

    /// `SetChannelVisibility(red, green, blue, alpha)` -> per-channel visibility,
    /// matching upstream `ITextureViewer`.
    #[allow(non_snake_case)]
    fn SetChannelVisibility(&self, red: bool, green: bool, blue: bool, alpha: bool) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .channels = u32::from(red)
                | (u32::from(green) << 1)
                | (u32::from(blue) << 2)
                | (u32::from(alpha) << 3);
    }

    /// `GetChannelVisibility()` -> `(red, green, blue, alpha)`.
    #[allow(non_snake_case)]
    fn GetChannelVisibility(&self) -> (bool, bool, bool, bool) {
        let channels = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .channels;
        (
            channels & 1 != 0,
            channels & 2 != 0,
            channels & 4 != 0,
            channels & 8 != 0,
        )
    }

    /// `GetChannelVisibilityBits()` -> packed channel mask.
    #[allow(non_snake_case)]
    fn GetChannelVisibilityBits(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .channels
    }

    #[allow(non_snake_case)]
    fn SetUseCustomActionNames(&self, enabled: bool) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .use_custom_action_names = enabled;
    }

    #[allow(non_snake_case)]
    fn SetShowParameterNames(&self, enabled: bool) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .show_parameter_names = enabled;
    }

    #[allow(non_snake_case)]
    fn SetShowAllParameters(&self, enabled: bool) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .show_all_parameters = enabled;
    }

    #[allow(non_snake_case)]
    fn SetEmptyRegionsVisible(&self, enabled: bool) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .empty_regions_visible = enabled;
    }

    #[allow(non_snake_case)]
    fn SetDurationColumnVisible(&self, enabled: bool) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .duration_column = enabled;
    }

    #[allow(non_snake_case)]
    fn SetAnnotationColumnVisible(&self, enabled: bool) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .annotation_column = enabled;
    }

    #[allow(non_snake_case)]
    fn UpdateDurationColumn(&self) {}

    #[allow(non_snake_case)]
    fn SetHighlightedAnnotation(&self, text: String) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .highlighted_annotation = text;
    }

    #[allow(non_snake_case)]
    fn GetHighlightedAnnotation(&self) -> String {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .highlighted_annotation
            .clone()
    }

    #[allow(non_snake_case)]
    fn SetCurrentInstance(&self, instance: u32) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .current_instance = instance;
    }

    #[allow(non_snake_case)]
    fn SetCurrentView(&self, view: u32) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .current_view = view;
    }

    #[allow(non_snake_case)]
    fn SetPreviewStage(&self, stage: u32) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .preview_stage = stage;
    }

    #[allow(non_snake_case)]
    fn GotoLocation(&self, x: u32, y: u32) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .picked = Some((x, y));
    }

    #[allow(non_snake_case)]
    fn GetPickedLocation(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        to_py(
            py,
            &match state.picked {
                Some((x, y)) => serde_json::json!({"x": x, "y": y}),
                None => serde_json::Value::Null,
            },
        )
    }

    #[allow(non_snake_case)]
    fn SetHistogramRange(&self, min: f32, max: f32) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.histogram_min = min;
        state.histogram_max = max;
    }

    #[allow(non_snake_case)]
    fn GetHistogramRange(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        to_py(
            py,
            &serde_json::json!({"min": state.histogram_min, "max": state.histogram_max}),
        )
    }

    #[allow(non_snake_case)]
    #[pyo3(signature = (follow_type = 0, stage = 0, index = 0, array_element = 0))]
    fn ViewFollowedResource(&self, follow_type: u32, stage: u32, index: i32, array_element: u32) {
        let _ = (follow_type, stage, index, array_element);
        queue_ui_request("textures");
    }

    /// `RegisterEventFilterFunction(name, description, filter, parser=None,
    /// completer=None)` -> register a named filter for the event browser.
    #[allow(non_snake_case)]
    #[pyo3(signature = (name, description, filter, parser = None, completer = None))]
    fn RegisterEventFilterFunction(
        &self,
        name: String,
        description: String,
        filter: Bound<'_, pyo3::PyAny>,
        parser: Option<Bound<'_, pyo3::PyAny>>,
        completer: Option<Bound<'_, pyo3::PyAny>>,
    ) -> bool {
        let _ = (description, filter, parser, completer);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if name.is_empty() || state.filter_functions.iter().any(|existing| existing == &name) {
            return false;
        }
        state.filter_functions.push(name);
        true
    }

    /// `UnregisterEventFilterFunction(name)` -> remove a registered filter.
    #[allow(non_snake_case)]
    fn UnregisterEventFilterFunction(&self, name: String) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let length = state.filter_functions.len();
        state.filter_functions.retain(|existing| existing != &name);
        state.filter_functions.len() != length
    }

    /// `RevealParameter(param)` -> reveal a parameter in the API inspector.
    #[allow(non_snake_case)]
    fn RevealParameter(&self, param: String) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .revealed_parameter = param;
        queue_ui_request(&self.panel);
    }

    #[allow(non_snake_case)]
    fn CurrentResource(&self) -> Option<usize> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .current_resource
    }

    #[allow(non_snake_case)]
    fn HighlightResourceUsage(&self, index: usize) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .highlighted_resource = Some(index);
        queue_ui_request("events");
    }

    #[allow(non_snake_case)]
    fn HighlightHistory(&self, index: usize) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .highlighted_history = Some(index);
        queue_ui_request("events");
    }

    #[allow(non_snake_case)]
    fn SetSaveCallback(&self, callback: Option<Bound<'_, pyo3::PyAny>>) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .save_callback = callback.map(Bound::unbind);
    }

    #[allow(non_snake_case)]
    fn SetRevertCallback(&self, callback: Option<Bound<'_, pyo3::PyAny>>) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .revert_callback = callback.map(Bound::unbind);
    }

    #[allow(non_snake_case)]
    fn HasSaveCallback(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .save_callback
            .is_some()
    }

    #[allow(non_snake_case)]
    fn HasRevertCallback(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .revert_callback
            .is_some()
    }

    /// `InvokeSaveCallback(entry, source)` -> call the registered save callback
    /// with `(stage, entry, source)`.
    #[allow(non_snake_case)]
    fn InvokeSaveCallback(
        &self,
        py: Python<'_>,
        entry: String,
        source: String,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let (callback, stage) = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            (state.save_callback.as_ref().map(|callback| callback.clone_ref(py)), self.shader_stage())
        };
        let Some(callback) = callback else {
            return Ok(py.None());
        };
        callback
            .bind(py)
            .call1((stage, entry, source))
            .map(Bound::unbind)
    }

    /// `InvokeRevertCallback()` -> call the registered revert callback with
    /// the shader stage.
    #[allow(non_snake_case)]
    fn InvokeRevertCallback(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let (callback, stage) = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            (
                state
                    .revert_callback
                    .as_ref()
                    .map(|callback| callback.clone_ref(py)),
                self.shader_stage(),
            )
        };
        let Some(callback) = callback else {
            return Ok(py.None());
        };
        callback.bind(py).call1((stage,)).map(Bound::unbind)
    }

    #[allow(non_snake_case)]
    #[pyo3(signature = (extension_name = String::new(), port = 5678))]
    fn AttachDebugger(
        &self,
        py: Python<'_>,
        extension_name: String,
        port: u16,
    ) -> pyo3::PyResult<()> {
        attach_debugger(py, &extension_name, port)
    }

    #[allow(non_snake_case)]
    fn IsDebuggerListening(&self) -> bool {
        DEBUGPY_LISTENING.load(Ordering::SeqCst)
    }

    #[allow(non_snake_case)]
    fn GetDebuggerPort(&self) -> u16 {
        DEBUGPY_PORT.load(Ordering::SeqCst)
    }

    #[allow(non_snake_case)]
    fn IsDebuggerConnected(&self, py: Python<'_>) -> bool {
        is_debugger_connected(py)
    }

    #[allow(non_snake_case)]
    fn CheckUnsavedChanges(&self) -> bool {
        false
    }

    #[allow(non_snake_case)]
    fn CurrentStep(&self) -> u32 {
        if let Some(timeline) = ACTIVE_DEBUG_TIMELINE
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
        {
            return u32::try_from(timeline.position).unwrap_or(u32::MAX);
        }
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .current_step
    }

    #[allow(non_snake_case)]
    fn SetCurrentStep(&self, step: u32) {
        {
            let mut guard = ACTIVE_DEBUG_TIMELINE
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(timeline) = guard.as_mut() {
                timeline.position = usize::try_from(step).unwrap_or(usize::MAX);
            }
        }
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .current_step = step;
    }

    #[allow(non_snake_case)]
    #[pyo3(signature = (line, enabled = true))]
    fn ToggleBreakpointOnDisassemblyLine(
        &self,
        line: u32,
        enabled: bool,
    ) -> pyo3::PyResult<()> {
        toggle_timeline_breakpoint(
            crate::debug_navigation::Breakpoint::Disassembly { line },
            enabled,
        )
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)
    }

    #[allow(non_snake_case)]
    #[pyo3(signature = (instruction, enabled = true))]
    fn ToggleBreakpointOnInstruction(
        &self,
        instruction: u32,
        enabled: bool,
    ) -> pyo3::PyResult<()> {
        let line = {
            let guard = ACTIVE_DEBUG_TIMELINE
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let timeline = guard.as_ref().ok_or_else(|| {
                pyo3::exceptions::PyRuntimeError::new_err(
                    "Collect a shader debug trace first",
                )
            })?;
            timeline
                .trace
                .instructions
                .iter()
                .find(|info| info.instruction == instruction)
                .map(|info| info.disassembly_line)
                .ok_or_else(|| {
                    pyo3::exceptions::PyRuntimeError::new_err(
                        "No source mapping for this instruction",
                    )
                })?
        };
        toggle_timeline_breakpoint(
            crate::debug_navigation::Breakpoint::Disassembly { line },
            enabled,
        )
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)
    }

    #[allow(non_snake_case)]
    fn GetBreakpoints(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let guard = ACTIVE_DEBUG_TIMELINE
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let timeline = guard.as_ref().ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err("Collect a shader debug trace first")
        })?;
        to_py(py, &timeline.breakpoints)
    }

    #[allow(non_snake_case)]
    fn GetCurrentBreakpoint(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let guard = ACTIVE_DEBUG_TIMELINE
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let timeline = guard.as_ref().ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err("Collect a shader debug trace first")
        })?;
        let mut navigator = crate::debug_navigation::DebugNavigator::default();
        navigator.position = timeline.position;
        navigator.breakpoints = timeline.breakpoints.clone();
        match navigator.current_breakpoint(&timeline.trace, &timeline.states, false) {
            Some(breakpoint) => to_py(py, &breakpoint),
            None => Ok(py.None()),
        }
    }

    #[allow(non_snake_case)]
    fn AddWatch(&self, name: String) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .watches
            .push(name);
    }

    #[allow(non_snake_case)]
    fn GetWatches(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .watches
            .clone()
    }

    #[allow(non_snake_case)]
    fn RunForward(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let mut guard = ACTIVE_DEBUG_TIMELINE
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(timeline) = guard.as_mut() {
            let reason = with_navigator(timeline, |navigator, trace, states| {
                navigator.run(trace, states, true, 0)
            });
            let document = serde_json::json!({
                "event_id": timeline.trace.event_id,
                "stage": timeline.trace.stage,
                "position": timeline.position,
                "total": timeline.states.len(),
                "reason": reason,
                "breakpoints": timeline.breakpoints,
            });
            return to_py(py, &document);
        }
        drop(guard);
        debug_next(py)
    }

    #[allow(non_snake_case)]
    fn GetCurrentFileContents(&self) -> pyo3::PyResult<String> {
        let stage = self
            .panel
            .strip_prefix("edit_shader:")
            .or_else(|| self.panel.strip_prefix("shader:"))
            .and_then(|stage| stage.parse::<u32>().ok())
            .ok_or_else(|| {
                pyo3::exceptions::PyRuntimeError::new_err(
                    "This viewer is not a shader viewer",
                )
            })?;
        let session = active_session()?;
        let report = session
            .shader(stage, None)
            .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
        Ok(report
            .source_files
            .first()
            .map(|file| file.contents.clone())
            .unwrap_or_default())
    }

    #[allow(non_snake_case)]
    fn ShowErrors(&self) -> String {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .errors
            .clone()
    }

    #[allow(non_snake_case)]
    fn GetScriptText(&self) -> String {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .script_text
            .clone()
    }

    #[allow(non_snake_case)]
    fn SetScriptText(&self, text: String) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .script_text = text;
    }

    #[allow(non_snake_case)]
    fn LoadScriptFromFilename(&self, path: String) -> bool {
        let Ok(text) = std::fs::read_to_string(&path) else {
            return false;
        };
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.script_text = text;
        state.script_name = path;
        true
    }

    #[allow(non_snake_case)]
    #[pyo3(signature = (name = String::new(), text = String::new()))]
    fn CreateNewScriptEditor(&self, name: String, text: String) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.script_name = name;
        state.script_text = text;
    }

    /// `RunScript(code=None)` -> run the supplied code, or the current editor
    /// contents when no code is passed.
    #[allow(non_snake_case)]
    #[pyo3(signature = (code = None))]
    fn RunScript(&self, py: Python<'_>, code: Option<String>) -> pyo3::PyResult<()> {
        let code = match code {
            Some(code) => code,
            None => self
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .script_text
                .clone(),
        };
        let globals = REPL_GLOBALS
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .map(|globals| globals.bind(py).clone())
            .unwrap_or_else(|| PyDict::new(py));
        py.run(
            &cstring(&code).map_err(pyo3::exceptions::PyValueError::new_err)?,
            Some(&globals),
            None,
        )
        .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(format_py_error(py, &error)))
    }

    #[allow(non_snake_case)]
    fn SetScriptOutputFilter(&self, filter: String) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .output_filter = Some(filter);
    }

    #[allow(non_snake_case)]
    fn SetExtensionOutputFilter(&self, filter: String) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .extension_output_filter = Some(filter);
    }

    #[allow(non_snake_case)]
    fn RemoveOutputFilter(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.output_filter = None;
        state.extension_output_filter = None;
    }

    #[allow(non_snake_case)]
    fn GetOutputFilter(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .output_filter
            .clone()
    }

    #[allow(non_snake_case)]
    fn GetExtensionOutputFilter(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .extension_output_filter
            .clone()
    }

    #[allow(non_snake_case)]
    fn ShowOutput(&self) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .show_output = true;
        queue_ui_request("python");
    }

    #[allow(non_snake_case)]
    fn ShowREPL(&self) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .show_repl = true;
        queue_ui_request("python");
    }

    #[allow(non_snake_case)]
    fn ShowHelp(&self) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .show_help = true;
        queue_ui_request("python");
    }
}

/// Candidate locations for a vendored `debugpy` package. The installed app
/// ships `python-site` next to the executable; development and test builds use
/// the repository's `target/python-site`, and `RENDERDOCX_PYTHON_SITE` always
/// wins.
fn debugpy_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = std::env::var_os("RENDERDOCX_PYTHON_SITE") {
        paths.push(PathBuf::from(path));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            paths.push(dir.join("python-site"));
            if let Some(parent) = dir.parent() {
                paths.push(parent.join("python-site"));
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        paths.push(cwd.join("target/python-site"));
    }
    paths.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/python-site"));
    paths
}

fn debugpy_site() -> Option<PathBuf> {
    debugpy_search_paths()
        .into_iter()
        .find(|path| path.join("debugpy").is_dir())
}

fn debugpy_unavailable() -> pyo3::PyErr {
    pyo3::exceptions::PyRuntimeError::new_err(
        "debugpy is not installed; place it in RENDERDOCX_PYTHON_SITE or <exe>/python-site",
    )
}

/// Make a vendored debugpy importable, without requiring it for normal use.
fn ensure_debugpy(py: Python<'_>) -> pyo3::PyResult<()> {
    if py.import("debugpy").is_ok() {
        return Ok(());
    }
    let Some(site) = debugpy_site() else {
        return Err(debugpy_unavailable());
    };
    let sys_path = py.import("sys")?.getattr("path")?;
    let text = site.to_string_lossy();
    // Insert at the front so a vendored copy wins over unrelated installations.
    sys_path.call_method1("insert", (0, text.as_ref()))?;
    py.import("debugpy").map(|_| ()).map_err(|_| debugpy_unavailable())
}

fn write_debugger_configs(directory: &Path) -> std::io::Result<PathBuf> {
    let vscode = directory.join(".vscode");
    std::fs::create_dir_all(&vscode)?;
    let launch = vscode.join("launch.json");
    if !launch.exists() {
        std::fs::write(
            &launch,
            r#"{
    "version": "0.2.0",
    "configurations": [
        {
            "name": "Python Debugger: Remote Attach",
            "type": "debugpy",
            "request": "attach",
            "connect": { "host": "localhost", "port": 5678 }
        }
    ]
}
"#,
        )?;
    }
    let tasks = vscode.join("tasks.json");
    if !tasks.exists() {
        std::fs::write(
            &tasks,
            r#"{
    "version": "2.0.0",
    "tasks": [
        {
            "label": "attach on startup",
            "command": "${command:workbench.action.debug.start}",
            "runOptions": {
                "runOn": "folderOpen"
            }
        }
    ]
}
"#,
        )?;
    }
    Ok(vscode)
}

fn is_debugger_connected(py: Python<'_>) -> bool {
    if !DEBUGPY_LISTENING.load(Ordering::SeqCst) {
        return false;
    }
    py.import("debugpy")
        .and_then(|debugpy| debugpy.call_method0("is_client_connected"))
        .and_then(|value| value.is_truthy())
        .unwrap_or(false)
}

/// Start the in-process debugpy adapter (once) and optionally write VS Code
/// attach configuration into an extension directory, matching upstream
/// `PythonContext::LaunchDebugger`.
fn attach_debugger(
    py: Python<'_>,
    extension_name: &str,
    port: u16,
) -> pyo3::PyResult<()> {
    ensure_debugpy(py)?;
    DEBUGPY_PORT.store(port, Ordering::SeqCst);
    if !DEBUGPY_LISTENING.load(Ordering::SeqCst) {
        let code = format!(
            r#"import debugpy, threading
debugpy.configure(subProcess=False)
try:
    debugpy.listen(('127.0.0.1', {port}), in_process_debug_adapter=True)
except RuntimeError:
    # Already listening in this interpreter (an earlier attach).
    pass
threading.Thread(target=debugpy.wait_for_client, daemon=True).start()
debugpy.trace_this_thread(True)
"#
        );
        py.run(
            &cstring(&code).map_err(pyo3::exceptions::PyValueError::new_err)?,
            None,
            None,
        )
        .map_err(|error| {
            pyo3::exceptions::PyRuntimeError::new_err(format_py_error(py, &error))
        })?;
        DEBUGPY_LISTENING.store(true, Ordering::SeqCst);
        DEBUGPY_ENABLED.store(true, Ordering::SeqCst);
    }
    if !extension_name.is_empty() {
        let directory = PathBuf::from(extension_name);
        if directory.is_dir() {
            let _ = write_debugger_configs(&directory);
        }
    }
    Ok(())
}

fn register_shader_callbacks(
    stage: u32,
    event: Option<u32>,
    save_callback: Option<pyo3::Py<pyo3::PyAny>>,
    revert_callback: Option<pyo3::Py<pyo3::PyAny>>,
) {
    let mut guard = SHADER_CALLBACKS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let callbacks = guard.get_or_insert_with(std::collections::HashMap::new);
    if save_callback.is_none() && revert_callback.is_none() {
        callbacks.remove(&(stage, event));
    } else {
        callbacks.insert((stage, event), (save_callback, revert_callback));
    }
}

fn registered_callback(
    py: Python<'_>,
    stage: u32,
    event: u32,
    save: bool,
) -> Option<pyo3::Py<pyo3::PyAny>> {
    let guard = SHADER_CALLBACKS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    guard.as_ref().and_then(|callbacks| {
        callbacks
            .get(&(stage, Some(event)))
            .or_else(|| callbacks.get(&(stage, None)))
            .and_then(|(save_cb, revert_cb)| {
                if save {
                    save_cb.as_ref().map(|callback| callback.clone_ref(py))
                } else {
                    revert_cb.as_ref().map(|callback| callback.clone_ref(py))
                }
            })
    })
}

/// Invoke a shader save callback registered by `EditShader`. The GUI calls
/// this after a successful apply; returns false when none is registered.
pub fn invoke_shader_save_callback(stage: u32, event: u32, entry: &str, source: &str) -> bool {
    Python::attach(|py| {
        match registered_callback(py, stage, event, true) {
            Some(callback) => callback
                .bind(py)
                .call1((stage, entry, source))
                .map(|_| true)
                .unwrap_or_else(|error| {
                    eprintln!("shader save callback failed: {error}");
                    false
                }),
            None => false,
        }
    })
}

/// Invoke a shader revert callback registered by `EditShader`.
pub fn invoke_shader_revert_callback(stage: u32, event: u32) -> bool {
    Python::attach(|py| match registered_callback(py, stage, event, false) {
        Some(callback) => callback
            .bind(py)
            .call1((stage,))
            .map(|_| true)
            .unwrap_or_else(|error| {
                eprintln!("shader revert callback failed: {error}");
                false
            }),
        None => false,
    })
}

fn viewer_handle(panel: &str) -> ViewerHandle {
    ViewerHandle {
        panel: panel.to_owned(),
        state: Mutex::new(ViewerState {
            zoom: 1.0,
            auto_fit: true,
            duration_column: true,
            annotation_column: true,
            histogram_max: 1.0,
            ..ViewerState::default()
        }),
    }
}

/// Viewer returned by `ViewShaderMessages`.
#[pyo3::pyclass]
struct ShaderMessageViewerObject {
    value: serde_json::Value,
}

#[pyo3::pymethods]
impl ShaderMessageViewerObject {
    #[allow(non_snake_case)]
    fn GetEvent(&self) -> u32 {
        self.value
            .get("event_id")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0)
    }

    #[allow(non_snake_case)]
    fn GetShaderMessages(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        to_py(
            py,
            &self.value.get("messages").cloned().unwrap_or_default(),
        )
    }

    #[allow(non_snake_case)]
    fn IsOutOfDate(&self) -> bool {
        false
    }

    #[allow(non_snake_case)]
    fn Widget(&self, py: Python<'_>) -> pyo3::Py<pyo3::PyAny> {
        py.None()
    }

    #[allow(non_snake_case)]
    fn Show(&self) {
        queue_ui_request("shader_messages");
    }

    #[pyo3(signature = (key, default = None))]
    fn get(
        &self,
        py: Python<'_>,
        key: &str,
        default: Option<Bound<'_, pyo3::PyAny>>,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Ok(default.map(Bound::unbind).unwrap_or_else(|| py.None())),
        }
    }

    fn __getitem__(
        &self,
        py: Python<'_>,
        key: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Err(pyo3::exceptions::PyKeyError::new_err(key.to_owned())),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }
}

/// Viewer returned by `ViewPixelHistory`.
#[pyo3::pyclass]
struct PixelHistoryViewObject {
    value: serde_json::Value,
}

#[pyo3::pymethods]
impl PixelHistoryViewObject {
    #[allow(non_snake_case)]
    fn GetEvent(&self) -> u32 {
        self.value
            .get("event_id")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0)
    }

    #[allow(non_snake_case)]
    fn GetHistory(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        to_py(
            py,
            &self.value.get("entries").cloned().unwrap_or_default(),
        )
    }

    #[allow(non_snake_case)]
    fn IsOutOfDate(&self) -> bool {
        false
    }

    #[allow(non_snake_case)]
    fn Widget(&self, py: Python<'_>) -> pyo3::Py<pyo3::PyAny> {
        py.None()
    }

    #[allow(non_snake_case)]
    fn Show(&self) {
        queue_ui_request("textures");
    }

    #[allow(non_snake_case)]
    fn SetHistory(&mut self, value: Bound<'_, pyo3::PyAny>) -> pyo3::PyResult<()> {
        self.value = pythonize::depythonize::<serde_json::Value>(&value)
            .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
        Ok(())
    }

    #[allow(non_snake_case)]
    fn SetFailedDebug(&self) {}

    #[pyo3(signature = (key, default = None))]
    fn get(
        &self,
        py: Python<'_>,
        key: &str,
        default: Option<Bound<'_, pyo3::PyAny>>,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Ok(default.map(Bound::unbind).unwrap_or_else(|| py.None())),
        }
    }

    fn __getitem__(
        &self,
        py: Python<'_>,
        key: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Err(pyo3::exceptions::PyKeyError::new_err(key.to_owned())),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }
}

/// Viewer returned by `ViewConstantBuffer`.
#[pyo3::pyclass]
struct ConstantBufferViewObject {
    value: serde_json::Value,
    stage: u32,
}

#[pyo3::pymethods]
impl ConstantBufferViewObject {
    #[allow(non_snake_case)]
    fn GetEvent(&self) -> u32 {
        self.value
            .get("event_id")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0)
    }

    #[allow(non_snake_case)]
    fn GetStage(&self) -> u32 {
        self.stage
    }

    #[allow(non_snake_case)]
    fn GetVariables(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        to_py(
            py,
            &self.value.get("variables").cloned().unwrap_or_default(),
        )
    }

    #[allow(non_snake_case)]
    fn Widget(&self, py: Python<'_>) -> pyo3::Py<pyo3::PyAny> {
        py.None()
    }

    #[allow(non_snake_case)]
    fn Show(&self) {
        queue_ui_request(&format!("shader:{}", self.stage));
    }

    #[pyo3(signature = (key, default = None))]
    fn get(
        &self,
        py: Python<'_>,
        key: &str,
        default: Option<Bound<'_, pyo3::PyAny>>,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Ok(default.map(Bound::unbind).unwrap_or_else(|| py.None())),
        }
    }

    fn __getitem__(
        &self,
        py: Python<'_>,
        key: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Err(pyo3::exceptions::PyKeyError::new_err(key.to_owned())),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }
}

macro_rules! viewer_getters {
    ($(($name:ident, $panel:literal, $doc:literal)),* $(,)?) => {
        $(
            #[doc = $doc]
            #[pyo3::pyfunction]
            #[allow(non_snake_case)]
            fn $name() -> ViewerHandle {
                viewer_handle($panel)
            }
        )*
    };
}

viewer_getters!(
    (GetEventBrowser, "events", "`GetEventBrowser()` -> event browser handle."),
    (GetAPIInspector, "parameters", "`GetAPIInspector()` -> API inspector handle."),
    (GetTextureViewer, "textures", "`GetTextureViewer()` -> texture viewer handle."),
    (GetPipelineViewer, "parameters", "`GetPipelineViewer()` -> pipeline viewer handle."),
    (GetMeshPreview, "mesh", "`GetMeshPreview()` -> mesh preview handle."),
    (GetResourceInspector, "resources", "`GetResourceInspector()` -> resource inspector handle."),
    (GetDebugMessageView, "messages", "`GetDebugMessageView()` -> debug message view handle."),
    (GetAnnotationViewer, "annotations", "`GetAnnotationViewer()` -> annotation viewer handle."),
    (GetPerformanceCounterViewer, "counters", "`GetPerformanceCounterViewer()` -> counter viewer handle."),
    (GetStatisticsViewer, "statistics", "`GetStatisticsViewer()` -> statistics viewer handle."),
    (GetTimelineBar, "events", "`GetTimelineBar()` -> timeline handle."),
    (GetDiagnosticLogView, "log", "`GetDiagnosticLogView()` -> log view handle."),
    (GetPythonShell, "python", "`GetPythonShell()` -> embedded Python console handle."),
);

/// `ViewShader(stage, event=None)` -> open the shader viewer for a bound stage.
/// `stage` uses RenderDoc ShaderStage numbering (0 vertex ... 7 mesh).
#[pyo3::pyfunction]
#[pyo3(signature = (stage, event = None))]
#[allow(non_snake_case)]
fn ViewShader(stage: u32, event: Option<u32>) -> pyo3::PyResult<ViewerHandle> {
    if stage >= 8 {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "Invalid shader stage",
        ));
    }
    if let Some(event) = event {
        let session = active_session()?;
        select_event(&session, event)?;
    }
    queue_ui_request(&format!("shader:{stage}"));
    Ok(viewer_handle(&format!("shader:{stage}")))
}

/// `EditShader(stage, event=None, save_callback=None, revert_callback=None)` ->
/// open the shader viewer in edit-capable mode, optionally registering the
/// upstream save/revert callbacks. The GUI's Edit action remains available for
/// the actual source edit.
#[pyo3::pyfunction]
#[pyo3(signature = (stage, event = None, save_callback = None, revert_callback = None))]
#[allow(non_snake_case)]
fn EditShader(
    py: Python<'_>,
    stage: u32,
    event: Option<u32>,
    save_callback: Option<Bound<'_, pyo3::PyAny>>,
    revert_callback: Option<Bound<'_, pyo3::PyAny>>,
) -> pyo3::PyResult<ViewerHandle> {
    if stage >= 8 {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "Invalid shader stage",
        ));
    }
    if let Some(event) = event {
        let session = active_session()?;
        select_event(&session, event)?;
    }
    queue_ui_request(&format!("edit_shader:{stage}"));
    let handle = viewer_handle(&format!("edit_shader:{stage}"));
    let save_callback = save_callback.map(Bound::unbind);
    let revert_callback = revert_callback.map(Bound::unbind);
    register_shader_callbacks(
        stage,
        event,
        save_callback.as_ref().map(|callback| callback.clone_ref(py)),
        revert_callback.as_ref().map(|callback| callback.clone_ref(py)),
    );
    {
        let mut state = handle
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.save_callback = save_callback;
        state.revert_callback = revert_callback;
    }
    Ok(handle)
}

/// `ViewBuffer(index, offset=0, length=None, format="")` -> open the buffer view
/// at a resource index and offset (upstream signature compatibility).
#[pyo3::pyfunction]
#[pyo3(signature = (index, offset = 0, length = None, format = "".to_owned()))]
#[allow(non_snake_case)]
fn ViewBuffer(
    index: u32,
    offset: u64,
    length: Option<u64>,
    format: String,
) -> ViewerHandle {
    let _ = (length, format);
    queue_ui_request(&format!("buffer:{index}:{offset}"));
    viewer_handle("buffer")
}

/// `ViewConstantBuffer(stage, slot, idx=0, event=None)` -> fetch reflected
/// constant values and open the shader panel.
#[pyo3::pyfunction]
#[pyo3(signature = (stage, slot, idx = 0, event = None))]
#[allow(non_snake_case)]
fn ViewConstantBuffer(
    stage: u32,
    slot: u32,
    idx: u32,
    event: Option<u32>,
) -> pyo3::PyResult<ConstantBufferViewObject> {
    let session = active_session()?;
    let event = match event {
        Some(event) => event,
        None => {
            let current = active_event();
            if current == 0 {
                return Err(pyo3::exceptions::PyRuntimeError::new_err(
                    "No event is selected",
                ));
            }
            current
        }
    };
    session
        .pipeline_for_event(event)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    set_active_event(Some(event));
    queue_ui_request(&format!("shader:{stage}"));
    let value = session
        .shader_constants(event, stage, slot, idx)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    let document = serde_json::to_value(&value)
        .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
    Ok(ConstantBufferViewObject {
        value: document,
        stage,
    })
}

/// `ViewPixelHistory(texture_index=0, x=0, y=0, event=None, offset=0,
/// limit=128)` -> fetch pixel history and open the texture panel.
#[pyo3::pyfunction]
#[pyo3(signature = (texture_index = 0, x = 0, y = 0, event = None, offset = 0, limit = 128))]
#[allow(non_snake_case)]
fn ViewPixelHistory(
    texture_index: usize,
    x: u32,
    y: u32,
    event: Option<u32>,
    offset: u64,
    limit: u32,
) -> pyo3::PyResult<PixelHistoryViewObject> {
    let session = active_session()?;
    let event = match event {
        Some(event) => event,
        None => {
            let current = active_event();
            if current == 0 {
                return Err(pyo3::exceptions::PyRuntimeError::new_err(
                    "No event is selected",
                ));
            }
            current
        }
    };
    let textures = active_textures();
    let texture = textures.get(texture_index).ok_or_else(|| {
        pyo3::exceptions::PyRuntimeError::new_err(
            "Texture index is out of range in the bound snapshot",
        )
    })?;
    select_event(&session, event)?;
    queue_ui_request("textures");
    let value = session
        .pixel_history(
            event,
            texture_index as u32,
            texture,
            crate::replay::TextureSelection::default(),
            x,
            y,
            offset,
            limit,
        )
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    let document = serde_json::to_value(&value)
        .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
    Ok(PixelHistoryViewObject { value: document })
}

/// `ViewShaderMessages(stage_mask=0, event=None)` -> fetch the event's shader
/// messages (optionally filtered by stage bitmask) and open the message panel.
#[pyo3::pyfunction]
#[pyo3(signature = (stage_mask = 0, event = None))]
#[allow(non_snake_case)]
fn ViewShaderMessages(
    stage_mask: u32,
    event: Option<u32>,
) -> pyo3::PyResult<ShaderMessageViewerObject> {
    let session = active_session()?;
    let event = match event {
        Some(event) => event,
        None => {
            let current = active_event();
            if current == 0 {
                return Err(pyo3::exceptions::PyRuntimeError::new_err(
                    "No event is selected",
                ));
            }
            current
        }
    };
    select_event(&session, event)?;
    queue_ui_request("shader_messages");
    let page = session
        .shader_messages(event, 0, 128)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    let mut document = serde_json::to_value(&page)
        .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
    if stage_mask != 0 {
        if let Some(messages) = document
            .get_mut("messages")
            .and_then(serde_json::Value::as_array_mut)
        {
            messages.retain(|message| {
                message
                    .get("stage")
                    .and_then(serde_json::Value::as_u64)
                    .is_some_and(|stage| stage < 32 && stage_mask & (1 << stage) != 0)
            });
        }
    }
    Ok(ShaderMessageViewerObject { value: document })
}

/// `DebugShader(stage, event, vertex=None, x=None, y=None, group=[0,0,0],
/// thread=[0,0,0])` -> start a shader debug trace for vertex (0), fragment (4),
/// compute (5) or mesh (7).
#[pyo3::pyfunction]
#[pyo3(signature = (stage, event, vertex = None, x = None, y = None, group = [0, 0, 0], thread = [0, 0, 0]))]
#[allow(non_snake_case)]
fn DebugShader(
    py: Python<'_>,
    stage: u32,
    event: u32,
    vertex: Option<u32>,
    x: Option<u32>,
    y: Option<u32>,
    group: [u32; 3],
    thread: [u32; 3],
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    match stage {
        0 => debug_vertex(py, event, vertex.unwrap_or(0), 0, None, 0),
        4 => debug_pixel(py, event, x.unwrap_or(0), y.unwrap_or(0), None, None, None),
        5 => debug_compute(py, event, group, thread),
        7 => debug_mesh(py, event, group, thread),
        _ => Err(pyo3::exceptions::PyValueError::new_err(
            "DebugShader supports vertex (0), fragment (4), compute (5) and mesh (7)",
        )),
    }
}

/// `apply_shader_edit(event, stage, encoding, entry, source, flags=None)` ->
/// build a target shader and replace the bound one in replay, mirroring the
/// GUI/CLI shader-apply flow.
#[pyo3::pyfunction]
#[pyo3(signature = (event, stage, encoding, entry, source, flags = None))]
#[allow(non_snake_case)]
fn apply_shader_edit(
    py: Python<'_>,
    event: u32,
    stage: u32,
    encoding: u32,
    entry: String,
    source: String,
    flags: Option<Bound<'_, pyo3::PyAny>>,
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    if entry.is_empty() || entry.contains('\0') || source.is_empty() {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "Shader entry point and source must be nonempty",
        ));
    }
    let session = active_session()?;
    let flags = flags
        .map(|flags| {
            pythonize::depythonize::<Vec<crate::replay::ShaderCompileFlag>>(&flags)
                .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))
        })
        .transpose()?;
    let value = session
        .shader_edit_with_flags(
            event,
            stage,
            Some((encoding, &entry, source.as_bytes())),
            flags.as_deref(),
        )
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    set_active_event(Some(event));
    to_py(py, &value)
}

/// `revert_shader_edit(event, stage)` -> remove the replacement for a shader.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn revert_shader_edit(
    py: Python<'_>,
    event: u32,
    stage: u32,
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let value = session
        .shader_edit(event, stage, None)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    set_active_event(Some(event));
    to_py(py, &value)
}

/// Field-addressable pipeline wrapper mirroring the useful parts of upstream
/// `PipeState` without changing the existing dict-returning `pipeline()` API.
#[pyo3::pyclass]
struct PipelineObject {
    value: serde_json::Value,
}

impl PipelineObject {
    fn field(&self, key: &str) -> Option<&serde_json::Value> {
        self.value.get(key)
    }

    fn field_or_empty(&self, key: &str) -> serde_json::Value {
        self.field(key).cloned().unwrap_or(serde_json::Value::Null)
    }
}

#[pyo3::pymethods]
impl PipelineObject {
    #[getter]
    fn event_id(&self) -> u32 {
        self.field("event_id")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0)
    }

    #[getter]
    fn available(&self) -> bool {
        self.field("available")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    }

    #[allow(non_snake_case)]
    fn GetEventId(&self) -> u32 {
        self.event_id()
    }

    #[allow(non_snake_case)]
    fn GetShader(
        &self,
        py: Python<'_>,
        stage: u32,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let shader = self
            .field("shaders")
            .and_then(|shaders| shaders.get(stage as usize))
            .ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err("Invalid shader stage")
            })?;
        to_py(py, shader)
    }

    #[allow(non_snake_case)]
    fn GetOutputs(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        to_py(py, &self.field_or_empty("outputs"))
    }

    #[allow(non_snake_case)]
    fn GetColorAttachments(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        to_py(py, &self.field_or_empty("color_attachments"))
    }

    #[allow(non_snake_case)]
    fn GetDepthAttachment(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        to_py(py, &self.field_or_empty("depth_attachment"))
    }

    #[allow(non_snake_case)]
    fn GetIBuffer(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        to_py(py, &self.field_or_empty("index_buffer"))
    }

    #[allow(non_snake_case)]
    fn GetVBuffers(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        to_py(py, &self.field_or_empty("vertex_buffers"))
    }

    #[allow(non_snake_case)]
    fn GetVertexInputs(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        to_py(py, &self.field_or_empty("vertex_inputs"))
    }

    #[allow(non_snake_case)]
    fn GetViewports(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        to_py(py, &self.field_or_empty("viewports"))
    }

    #[allow(non_snake_case)]
    fn GetScissors(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        to_py(py, &self.field_or_empty("scissors"))
    }

    #[allow(non_snake_case)]
    fn GetFixedState(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        to_py(py, &self.field_or_empty("fixed_state"))
    }

    #[allow(non_snake_case)]
    fn GetAPISpecific(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        to_py(py, &self.field_or_empty("api_specific"))
    }

    #[allow(non_snake_case)]
    fn GetAPIKind(&self) -> String {
        self.field("api_specific")
            .and_then(|value| value.get("kind"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    #[allow(non_snake_case)]
    fn GetD3D11State(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.field("api_specific").and_then(|value| value.get("d3d11")) {
            Some(value) => to_py(py, value),
            None => Ok(py.None()),
        }
    }

    #[allow(non_snake_case)]
    fn GetD3D12State(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.field("api_specific").and_then(|value| value.get("d3d12")) {
            Some(value) => to_py(py, value),
            None => Ok(py.None()),
        }
    }

    #[allow(non_snake_case)]
    fn GetGLState(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.field("api_specific").and_then(|value| value.get("gl")) {
            Some(value) => to_py(py, value),
            None => Ok(py.None()),
        }
    }

    #[allow(non_snake_case)]
    fn GetOpenGLState(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        self.GetGLState(py)
    }

    #[allow(non_snake_case)]
    fn GetVulkanState(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.field("api_specific").and_then(|value| value.get("vulkan")) {
            Some(value) => to_py(py, value),
            None => Ok(py.None()),
        }
    }

    #[allow(non_snake_case)]
    fn GetD3D12RootParameters(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self
            .field("api_specific")
            .and_then(|value| value.get("d3d12"))
            .and_then(|value| value.get("rootParameters"))
        {
            Some(value) => to_py(py, value),
            None => Ok(py.None()),
        }
    }

    #[allow(non_snake_case)]
    fn GetD3D12StaticSamplers(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self
            .field("api_specific")
            .and_then(|value| value.get("d3d12"))
            .and_then(|value| value.get("staticSamplers"))
        {
            Some(value) => to_py(py, value),
            None => Ok(py.None()),
        }
    }

    #[allow(non_snake_case)]
    fn GetVulkanSampleLocations(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self
            .field("api_specific")
            .and_then(|value| value.get("vulkan"))
            .and_then(|value| value.get("sampleLocations"))
        {
            Some(value) => to_py(py, value),
            None => Ok(py.None()),
        }
    }

    #[allow(non_snake_case)]
    fn GetVulkanRenderPass(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self
            .field("api_specific")
            .and_then(|value| value.get("vulkan"))
            .and_then(|value| value.get("renderPassDetails"))
        {
            Some(value) => to_py(py, value),
            None => Ok(py.None()),
        }
    }

    #[allow(non_snake_case)]
    fn GetVulkanFramebuffer(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self
            .field("api_specific")
            .and_then(|value| value.get("vulkan"))
            .and_then(|value| value.get("framebufferDetails"))
        {
            Some(value) => to_py(py, value),
            None => Ok(py.None()),
        }
    }

    #[allow(non_snake_case)]
    fn GetShaderReflection(
        &self,
        stage: u32,
    ) -> pyo3::PyResult<ShaderReflectionObject> {
        let event = self.event_id();
        if event == 0 {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(
                "No event is selected",
            ));
        }
        let session = active_session()?;
        session
            .pipeline_for_event(event)
            .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
        let report = session
            .shader(stage, None)
            .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
        let value = serde_json::to_value(&report)
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
        Ok(ShaderReflectionObject { value })
    }

    #[pyo3(signature = (key, default = None))]
    fn get(
        &self,
        py: Python<'_>,
        key: &str,
        default: Option<Bound<'_, pyo3::PyAny>>,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Ok(default.map(Bound::unbind).unwrap_or_else(|| py.None())),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn __getitem__(
        &self,
        py: Python<'_>,
        key: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Err(pyo3::exceptions::PyKeyError::new_err(key.to_owned())),
        }
    }
}

/// `PipelineState(event=None)` -> field-addressable pipeline object. Supports
/// `.event_id`, `.available`, `.GetShader(stage)`, `.GetOutputs()`,
/// `.GetColorAttachments()`, `.GetDepthAttachment()`, `.GetIBuffer()`,
/// `.GetVBuffers()`, `.GetVertexInputs()`, `.GetViewports()`, `.GetScissors()`,
/// `.GetFixedState()`, `[...]`, `.get()` and `.keys()`.
#[pyo3::pyfunction]
#[pyo3(signature = (event = None))]
#[allow(non_snake_case)]
fn PipelineState(
    event: Option<u32>,
) -> pyo3::PyResult<PipelineObject> {
    let session = active_session()?;
    let event = match event {
        Some(event) => event,
        None => {
            let current = active_event();
            if current == 0 {
                return Err(pyo3::exceptions::PyRuntimeError::new_err(
                    "No event is selected",
                ));
            }
            current
        }
    };
    let value = session
        .pipeline_for_event(event)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    set_active_event(Some(event));
    let mut document = serde_json::to_value(&value)
        .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
    apply_replacements(&mut document);
    Ok(PipelineObject { value: document })
}

fn json_field_object(
    py: Python<'_>,
    value: &serde_json::Value,
    key: &str,
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    match value.get(key) {
        Some(value) => to_py(py, value),
        None => Err(pyo3::exceptions::PyAttributeError::new_err(key.to_owned())),
    }
}

/// Field-addressable texture description (upstream TextureDescription fields,
/// including camelCase aliases such as `arraySize`).
#[pyo3::pyclass]
struct TextureObject {
    value: serde_json::Value,
}

#[pyo3::pymethods]
impl TextureObject {
    fn __getattr__(
        &self,
        py: Python<'_>,
        name: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let key = match name {
            "arraySize" => "array_size",
            "format" => "format_name",
            _ => name,
        };
        json_field_object(py, &self.value, key)
    }

    fn __getitem__(
        &self,
        py: Python<'_>,
        key: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Err(pyo3::exceptions::PyKeyError::new_err(key.to_owned())),
        }
    }

    #[pyo3(signature = (key, default = None))]
    fn get(
        &self,
        py: Python<'_>,
        key: &str,
        default: Option<Bound<'_, pyo3::PyAny>>,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Ok(default.map(Bound::unbind).unwrap_or_else(|| py.None())),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }
}

/// `Texture(index)` -> field-addressable texture description, or None.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn Texture(index: usize) -> Option<TextureObject> {
    active_textures()
        .get(index)
        .and_then(|texture| serde_json::to_value(texture).ok())
        .map(|value| TextureObject { value })
}

/// Field-addressable resource description (upstream ResourceDescription
/// fields plus camelCase aliases).
#[pyo3::pyclass]
struct ResourceObject {
    value: serde_json::Value,
}

#[pyo3::pymethods]
impl ResourceObject {
    fn __getattr__(
        &self,
        py: Python<'_>,
        name: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let key = match name {
            "resourceId" => "rename_key",
            "autogeneratedName" => "autogenerated_name",
            _ => name,
        };
        json_field_object(py, &self.value, key)
    }

    fn __getitem__(
        &self,
        py: Python<'_>,
        key: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Err(pyo3::exceptions::PyKeyError::new_err(key.to_owned())),
        }
    }

    #[pyo3(signature = (key, default = None))]
    fn get(
        &self,
        py: Python<'_>,
        key: &str,
        default: Option<Bound<'_, pyo3::PyAny>>,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Ok(default.map(Bound::unbind).unwrap_or_else(|| py.None())),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[allow(non_snake_case)]
    fn GetResourceId(&self) -> Option<ResourceIdObject> {
        self.value
            .get("rename_key")
            .and_then(serde_json::Value::as_str)
            .map(|id| ResourceIdObject { id: id.to_owned() })
    }
}

/// `Resource(index)` -> field-addressable resource description, or None.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn Resource(index: usize) -> Option<ResourceObject> {
    active_resources()
        .get(index)
        .and_then(|resource| serde_json::to_value(resource).ok())
        .map(|value| ResourceObject { value })
}

/// Generic field-addressable JSON object used for deep pipeline subobjects
/// (root parameters, static samplers, sample locations, render pass/framebuffer).
#[pyo3::pyclass]
struct FieldObject {
    value: serde_json::Value,
}

#[pyo3::pymethods]
impl FieldObject {
    fn __getattr__(
        &self,
        py: Python<'_>,
        name: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        json_field_object(py, &self.value, name)
    }

    fn __getitem__(
        &self,
        py: Python<'_>,
        key: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Err(pyo3::exceptions::PyKeyError::new_err(key.to_owned())),
        }
    }

    #[pyo3(signature = (key, default = None))]
    fn get(
        &self,
        py: Python<'_>,
        key: &str,
        default: Option<Bound<'_, pyo3::PyAny>>,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Ok(default.map(Bound::unbind).unwrap_or_else(|| py.None())),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }
}

fn field_object(value: Bound<'_, pyo3::PyAny>) -> pyo3::PyResult<FieldObject> {
    let value = pythonize::depythonize::<serde_json::Value>(&value)
        .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
    Ok(FieldObject { value })
}

/// `RootParameter(value)` -> deep D3D12 root parameter object.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn RootParameter(value: Bound<'_, pyo3::PyAny>) -> pyo3::PyResult<FieldObject> {
    field_object(value)
}

/// `StaticSampler(value)` -> deep D3D12 static sampler object.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn StaticSampler(value: Bound<'_, pyo3::PyAny>) -> pyo3::PyResult<FieldObject> {
    field_object(value)
}

/// `SampleLocations(value)` -> Vulkan sample locations object.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn SampleLocations(value: Bound<'_, pyo3::PyAny>) -> pyo3::PyResult<FieldObject> {
    field_object(value)
}

/// `RenderPass(value)` -> Vulkan render-pass detail object.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn RenderPass(value: Bound<'_, pyo3::PyAny>) -> pyo3::PyResult<FieldObject> {
    field_object(value)
}

/// `Framebuffer(value)` -> Vulkan framebuffer detail object.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn Framebuffer(value: Bound<'_, pyo3::PyAny>) -> pyo3::PyResult<FieldObject> {
    field_object(value)
}

/// Field-addressable reflected read-only/read-write resource.
#[pyo3::pyclass]
struct ReflectedResourceObject {
    value: serde_json::Value,
}

#[pyo3::pymethods]
impl ReflectedResourceObject {
    fn __getattr__(
        &self,
        py: Python<'_>,
        name: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let key = match name {
            "arraySize" => "array_size",
            "textureType" => "texture_type",
            "descriptorType" => "descriptor_type",
            "isTexture" => "is_texture",
            "hasSampler" => "has_sampler",
            "inputAttachment" => "input_attachment",
            "readOnly" => "read_only",
            "variableType" => "variable_type",
            _ => name,
        };
        json_field_object(py, &self.value, key)
    }

    fn __getitem__(
        &self,
        py: Python<'_>,
        key: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Err(pyo3::exceptions::PyKeyError::new_err(key.to_owned())),
        }
    }

    #[pyo3(signature = (key, default = None))]
    fn get(
        &self,
        py: Python<'_>,
        key: &str,
        default: Option<Bound<'_, pyo3::PyAny>>,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Ok(default.map(Bound::unbind).unwrap_or_else(|| py.None())),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[allow(non_snake_case)]
    fn GetName(&self) -> String {
        self.value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    #[allow(non_snake_case)]
    fn GetType(&self) -> pyo3::PyResult<ReflectedTypeObject> {
        let value = self.value.get("variable_type").cloned().ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err("Resource has no reflected type")
        })?;
        Ok(ReflectedTypeObject { value })
    }
}

/// `ReflectedResource(value)` -> field-addressable reflected resource.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn ReflectedResource(
    value: Bound<'_, pyo3::PyAny>,
) -> pyo3::PyResult<ReflectedResourceObject> {
    let resource = pythonize::depythonize::<crate::replay::ReflectedResource>(&value)
        .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
    let value = serde_json::to_value(&resource)
        .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
    Ok(ReflectedResourceObject { value })
}

/// Field-addressable shader signature entry.
#[pyo3::pyclass]
struct ShaderSignatureObject {
    value: serde_json::Value,
}

#[pyo3::pymethods]
impl ShaderSignatureObject {
    fn __getattr__(
        &self,
        py: Python<'_>,
        name: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let key = match name {
            "semanticIndex" => "semantic_index",
            "systemValue" => "system_value",
            _ => name,
        };
        json_field_object(py, &self.value, key)
    }

    fn __getitem__(
        &self,
        py: Python<'_>,
        key: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Err(pyo3::exceptions::PyKeyError::new_err(key.to_owned())),
        }
    }

    #[pyo3(signature = (key, default = None))]
    fn get(
        &self,
        py: Python<'_>,
        key: &str,
        default: Option<Bound<'_, pyo3::PyAny>>,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Ok(default.map(Bound::unbind).unwrap_or_else(|| py.None())),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[allow(non_snake_case)]
    fn GetName(&self) -> String {
        self.value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    #[allow(non_snake_case)]
    fn GetSemantic(&self) -> String {
        self.value
            .get("semantic")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    #[allow(non_snake_case)]
    fn GetSemanticIndex(&self) -> u32 {
        self.value
            .get("semantic_index")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0)
    }
}

/// `ShaderSignature(value)` -> field-addressable signature entry.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn ShaderSignature(value: Bound<'_, pyo3::PyAny>) -> pyo3::PyResult<ShaderSignatureObject> {
    let signature = pythonize::depythonize::<crate::replay::ShaderSignature>(&value)
        .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
    let value = serde_json::to_value(&signature)
        .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
    Ok(ShaderSignatureObject { value })
}

/// Field-addressable descriptor entry.
#[pyo3::pyclass]
struct DescriptorObject {
    value: serde_json::Value,
}

#[pyo3::pymethods]
impl DescriptorObject {
    fn __getattr__(
        &self,
        py: Python<'_>,
        name: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let key = match name {
            "resourceId" => "resource",
            "byteOffset" => "byte_offset",
            _ => name,
        };
        json_field_object(py, &self.value, key)
    }

    fn __getitem__(
        &self,
        py: Python<'_>,
        key: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Err(pyo3::exceptions::PyKeyError::new_err(key.to_owned())),
        }
    }

    #[pyo3(signature = (key, default = None))]
    fn get(
        &self,
        py: Python<'_>,
        key: &str,
        default: Option<Bound<'_, pyo3::PyAny>>,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Ok(default.map(Bound::unbind).unwrap_or_else(|| py.None())),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }
}

/// `Descriptor(value)` -> field-addressable descriptor dict.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn Descriptor(value: Bound<'_, pyo3::PyAny>) -> pyo3::PyResult<DescriptorObject> {
    let value = pythonize::depythonize::<serde_json::Value>(&value)
        .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
    Ok(DescriptorObject { value })
}

/// Field-addressable descriptor store.
#[pyo3::pyclass]
struct DescriptorStoreObject {
    value: serde_json::Value,
    index: usize,
}

#[pyo3::pymethods]
impl DescriptorStoreObject {
    fn __getattr__(
        &self,
        py: Python<'_>,
        name: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let key = match name {
            "descriptorByteSize" => "descriptor_byte_size",
            "firstOffset" => "first_offset",
            _ => name,
        };
        json_field_object(py, &self.value, key)
    }

    fn __getitem__(
        &self,
        py: Python<'_>,
        key: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Err(pyo3::exceptions::PyKeyError::new_err(key.to_owned())),
        }
    }

    #[pyo3(signature = (key, default = None))]
    fn get(
        &self,
        py: Python<'_>,
        key: &str,
        default: Option<Bound<'_, pyo3::PyAny>>,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Ok(default.map(Bound::unbind).unwrap_or_else(|| py.None())),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn GetIndex(&self) -> usize {
        self.index
    }
}

/// `DescriptorStore(index)` -> field-addressable descriptor store, or None.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn DescriptorStore(index: usize) -> Option<DescriptorStoreObject> {
    let snapshot = active_snapshot()?;
    let value = snapshot
        .get("descriptor_stores")
        .and_then(serde_json::Value::as_array)
        .and_then(|stores| stores.get(index))
        .cloned()?;
    Some(DescriptorStoreObject { value, index })
}

/// Field-addressable API properties from the capture snapshot.
#[pyo3::pyclass]
struct APIPropertiesObject {
    value: serde_json::Value,
}

#[pyo3::pymethods]
impl APIPropertiesObject {
    fn __getattr__(
        &self,
        py: Python<'_>,
        name: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let key = match name {
            "pipelineType" => "pipelineType",
            "localRenderer" => "localRenderer",
            "vendor" => "vendor",
            "deviceID" => "deviceID",
            "driverName" => "driverName",
            _ => name,
        };
        json_field_object(py, &self.value, key)
    }

    fn __getitem__(
        &self,
        py: Python<'_>,
        key: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Err(pyo3::exceptions::PyKeyError::new_err(key.to_owned())),
        }
    }

    #[pyo3(signature = (key, default = None))]
    fn get(
        &self,
        py: Python<'_>,
        key: &str,
        default: Option<Bound<'_, pyo3::PyAny>>,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Ok(default.map(Bound::unbind).unwrap_or_else(|| py.None())),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[allow(non_snake_case)]
    fn GetPipelineType(&self) -> u32 {
        self.value
            .get("pipelineType")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0)
    }

    #[allow(non_snake_case)]
    fn GetLocalRenderer(&self) -> u32 {
        self.value
            .get("localRenderer")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0)
    }
}

/// `APIProperties()` -> field-addressable API properties, or None.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn APIProperties() -> Option<APIPropertiesObject> {
    let snapshot = active_snapshot()?;
    let mut value = serde_json::json!({
        "pipelineType": snapshot.get("api").cloned().unwrap_or(serde_json::Value::Null),
        "localRenderer": snapshot.get("local_renderer").cloned().unwrap_or(serde_json::Value::Null),
        "vendor": snapshot.get("replay_options").and_then(|options| options.get("gpu_vendor")).cloned().unwrap_or(serde_json::Value::Null),
        "deviceID": snapshot.get("replay_options").and_then(|options| options.get("gpu_device_id")).cloned().unwrap_or(serde_json::Value::Null),
        "driverName": snapshot.get("replay_options").and_then(|options| options.get("gpu_driver")).cloned().unwrap_or(serde_json::Value::Null),
    });
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "api".into(),
            snapshot.get("api").cloned().unwrap_or(serde_json::Value::Null),
        );
        object.insert(
            "local_renderer".into(),
            snapshot
                .get("local_renderer")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        );
    }
    Some(APIPropertiesObject { value })
}

/// Field-addressable debug variable (upstream ShaderVariable shape).
#[pyo3::pyclass]
struct ShaderVariableObject {
    value: serde_json::Value,
}

#[pyo3::pymethods]
impl ShaderVariableObject {
    fn __getattr__(
        &self,
        py: Python<'_>,
        name: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let key = match name {
            "value" => "value_hex",
            _ => name,
        };
        json_field_object(py, &self.value, key)
    }

    fn __getitem__(
        &self,
        py: Python<'_>,
        key: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Err(pyo3::exceptions::PyKeyError::new_err(key.to_owned())),
        }
    }

    #[pyo3(signature = (key, default = None))]
    fn get(
        &self,
        py: Python<'_>,
        key: &str,
        default: Option<Bound<'_, pyo3::PyAny>>,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Ok(default.map(Bound::unbind).unwrap_or_else(|| py.None())),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[allow(non_snake_case)]
    fn GetName(&self) -> String {
        self.value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    #[allow(non_snake_case)]
    fn GetValue(&self) -> String {
        self.value
            .get("value_hex")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    #[allow(non_snake_case)]
    fn GetMembers(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let members = self
            .value
            .get("members")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        field_object_list(py, members, |value| ShaderVariableObject { value })
    }
}

/// `ShaderVariable(value)` -> field-addressable debug variable from a trace
/// input/member/constant value dict.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn ShaderVariable(value: Bound<'_, pyo3::PyAny>) -> pyo3::PyResult<ShaderVariableObject> {
    let variable = pythonize::depythonize::<crate::replay::DebugVariable>(&value)
        .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
    let value = serde_json::to_value(&variable)
        .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
    Ok(ShaderVariableObject { value })
}

/// Field-addressable reflected constant block.
#[pyo3::pyclass]
struct ConstantBlockObject {
    value: serde_json::Value,
}

#[pyo3::pymethods]
impl ConstantBlockObject {
    fn __getattr__(
        &self,
        py: Python<'_>,
        name: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let key = match name {
            "arraySize" => "array_size",
            "byteSize" => "byte_size",
            "bufferBacked" => "buffer_backed",
            "inlineData" => "inline_data",
            "compileConstants" => "compile_constants",
            _ => name,
        };
        json_field_object(py, &self.value, key)
    }

    fn __getitem__(
        &self,
        py: Python<'_>,
        key: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Err(pyo3::exceptions::PyKeyError::new_err(key.to_owned())),
        }
    }

    #[pyo3(signature = (key, default = None))]
    fn get(
        &self,
        py: Python<'_>,
        key: &str,
        default: Option<Bound<'_, pyo3::PyAny>>,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Ok(default.map(Bound::unbind).unwrap_or_else(|| py.None())),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[allow(non_snake_case)]
    fn GetName(&self) -> String {
        self.value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    #[allow(non_snake_case)]
    fn GetVariables(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let variables = self
            .value
            .get("variables")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        field_object_list(py, variables, |value| ShaderConstantObject { value })
    }

    #[allow(non_snake_case)]
    #[pyo3(signature = (index = 0))]
    fn GetVariable(&self, index: usize) -> pyo3::PyResult<ShaderConstantObject> {
        let value = self
            .value
            .get("variables")
            .and_then(|variables| variables.get(index))
            .cloned()
            .ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err(
                    "Constant block variable index is out of range",
                )
            })?;
        Ok(ShaderConstantObject { value })
    }
}

/// `ConstantBlock(value)` -> field-addressable constant block from a
/// `ShaderReflection.GetConstantBlocks()` entry.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn ConstantBlock(value: Bound<'_, pyo3::PyAny>) -> pyo3::PyResult<ConstantBlockObject> {
    let block = pythonize::depythonize::<crate::replay::ReflectedConstantBlock>(&value)
        .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
    let value = serde_json::to_value(&block)
        .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
    Ok(ConstantBlockObject { value })
}

/// Field-addressable reflected sampler.
#[pyo3::pyclass]
struct SamplerDescriptorObject {
    value: serde_json::Value,
}

#[pyo3::pymethods]
impl SamplerDescriptorObject {
    fn __getattr__(
        &self,
        py: Python<'_>,
        name: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let key = match name {
            "arraySize" => "array_size",
            _ => name,
        };
        json_field_object(py, &self.value, key)
    }

    fn __getitem__(
        &self,
        py: Python<'_>,
        key: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Err(pyo3::exceptions::PyKeyError::new_err(key.to_owned())),
        }
    }

    #[pyo3(signature = (key, default = None))]
    fn get(
        &self,
        py: Python<'_>,
        key: &str,
        default: Option<Bound<'_, pyo3::PyAny>>,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Ok(default.map(Bound::unbind).unwrap_or_else(|| py.None())),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[allow(non_snake_case)]
    fn GetName(&self) -> String {
        self.value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }
}

/// `SamplerDescriptor(value)` -> field-addressable reflected sampler.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn SamplerDescriptor(
    value: Bound<'_, pyo3::PyAny>,
) -> pyo3::PyResult<SamplerDescriptorObject> {
    let sampler = pythonize::depythonize::<crate::replay::ReflectedSampler>(&value)
        .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
    let value = serde_json::to_value(&sampler)
        .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
    Ok(SamplerDescriptorObject { value })
}

/// Field-addressable reflected shader constant.
#[pyo3::pyclass]
struct ShaderConstantObject {
    value: serde_json::Value,
}

#[pyo3::pymethods]
impl ShaderConstantObject {
    fn __getattr__(
        &self,
        py: Python<'_>,
        name: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let key = match name {
            "byteOffset" => "byte_offset",
            "bitOffset" => "bit_offset",
            "bitSize" => "bit_size",
            "defaultValue" => "default_value",
            _ => name,
        };
        json_field_object(py, &self.value, key)
    }

    fn __getitem__(
        &self,
        py: Python<'_>,
        key: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Err(pyo3::exceptions::PyKeyError::new_err(key.to_owned())),
        }
    }

    #[pyo3(signature = (key, default = None))]
    fn get(
        &self,
        py: Python<'_>,
        key: &str,
        default: Option<Bound<'_, pyo3::PyAny>>,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Ok(default.map(Bound::unbind).unwrap_or_else(|| py.None())),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[allow(non_snake_case)]
    fn GetName(&self) -> String {
        self.value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    #[allow(non_snake_case)]
    fn GetDefaultValue(&self) -> String {
        self.value
            .get("default_value")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    #[allow(non_snake_case)]
    fn GetType(&self) -> pyo3::PyResult<ReflectedTypeObject> {
        let value = self
            .value
            .get("type")
            .cloned()
            .ok_or_else(|| {
                pyo3::exceptions::PyRuntimeError::new_err("Shader constant has no type")
            })?;
        Ok(ReflectedTypeObject { value })
    }
}

/// `ShaderConstant(value)` -> field-addressable reflected constant.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn ShaderConstant(
    value: Bound<'_, pyo3::PyAny>,
) -> pyo3::PyResult<ShaderConstantObject> {
    let constant = pythonize::depythonize::<crate::replay::ReflectedConstant>(&value)
        .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
    let value = serde_json::to_value(&constant)
        .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
    Ok(ShaderConstantObject { value })
}

/// Field-addressable reflected type description.
#[pyo3::pyclass]
struct ReflectedTypeObject {
    value: serde_json::Value,
}

#[pyo3::pymethods]
impl ReflectedTypeObject {
    fn __getattr__(
        &self,
        py: Python<'_>,
        name: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let key = match name {
            "pointerTypeId" => "pointer_type_id",
            "arrayStride" => "array_stride",
            "baseType" => "base_type",
            "matrixStride" => "matrix_stride",
            _ => name,
        };
        json_field_object(py, &self.value, key)
    }

    fn __getitem__(
        &self,
        py: Python<'_>,
        key: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Err(pyo3::exceptions::PyKeyError::new_err(key.to_owned())),
        }
    }

    #[pyo3(signature = (key, default = None))]
    fn get(
        &self,
        py: Python<'_>,
        key: &str,
        default: Option<Bound<'_, pyo3::PyAny>>,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Ok(default.map(Bound::unbind).unwrap_or_else(|| py.None())),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[allow(non_snake_case)]
    fn GetName(&self) -> String {
        self.value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    #[allow(non_snake_case)]
    fn GetMembers(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let members = self
            .value
            .get("members")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        field_object_list(py, members, |value| ShaderConstantObject { value })
    }

    #[allow(non_snake_case)]
    #[pyo3(signature = (index = 0))]
    fn GetMember(&self, index: usize) -> pyo3::PyResult<ShaderConstantObject> {
        let value = self
            .value
            .get("members")
            .and_then(|members| members.get(index))
            .cloned()
            .ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err("Type member index is out of range")
            })?;
        Ok(ShaderConstantObject { value })
    }
}

/// `ReflectedType(value)` -> field-addressable reflected type.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn ReflectedType(value: Bound<'_, pyo3::PyAny>) -> pyo3::PyResult<ReflectedTypeObject> {
    let reflected = pythonize::depythonize::<crate::replay::ReflectedType>(&value)
        .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
    let value = serde_json::to_value(&reflected)
        .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
    Ok(ReflectedTypeObject { value })
}

/// Opaque resource identifier with string form, matching pyrenderdoc's use of
/// `ResourceId` for rename keys and lookup.
#[pyo3::pyclass]
#[derive(Clone, PartialEq, Eq)]
struct ResourceIdObject {
    id: String,
}

#[pyo3::pymethods]
impl ResourceIdObject {
    #[getter]
    fn id(&self) -> String {
        self.id.clone()
    }

    fn __str__(&self) -> String {
        self.id.clone()
    }

    fn __repr__(&self) -> String {
        format!("ResourceId('{}')", self.id)
    }

    fn __hash__(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.id.hash(&mut hasher);
        hasher.finish()
    }
}

/// `ResourceId(value)` -> opaque resource identifier from a rename key.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn ResourceId(value: String) -> ResourceIdObject {
    ResourceIdObject { id: value }
}

/// Field-addressable frame description (snapshot frame + FrameInfo fields).
#[pyo3::pyclass]
struct FrameDescriptionObject {
    value: serde_json::Value,
}

#[pyo3::pymethods]
impl FrameDescriptionObject {
    fn __getattr__(
        &self,
        py: Python<'_>,
        name: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let key = match name {
            "frameNumber" => "frameNumber",
            "captureTime" => "capture_time",
            "fileOffset" => "file_offset",
            "uncompressedSize" => "uncompressed_size",
            "compressedSize" => "compressed_size",
            "persistentSize" => "persistent_size",
            "initialDataSize" => "initial_data_size",
            "containsAnnotations" => "contains_annotations",
            _ => name,
        };
        json_field_object(py, &self.value, key)
    }

    fn __getitem__(
        &self,
        py: Python<'_>,
        key: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Err(pyo3::exceptions::PyKeyError::new_err(key.to_owned())),
        }
    }

    #[pyo3(signature = (key, default = None))]
    fn get(
        &self,
        py: Python<'_>,
        key: &str,
        default: Option<Bound<'_, pyo3::PyAny>>,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Ok(default.map(Bound::unbind).unwrap_or_else(|| py.None())),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[allow(non_snake_case)]
    fn GetFrameNumber(&self) -> u32 {
        self.value
            .get("frameNumber")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0)
    }

    #[allow(non_snake_case)]
    fn GetCaptureTime(&self) -> String {
        self.value
            .get("capture_time")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }
}

/// `FrameDescription()` -> frame description object, or None without a
/// snapshot.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn FrameDescription() -> Option<FrameDescriptionObject> {
    let snapshot = active_snapshot()?;
    let mut value = snapshot
        .get("frame_info")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "frameNumber".into(),
            snapshot
                .get("frame")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        );
    }
    Some(FrameDescriptionObject { value })
}

/// Field-addressable shader reflection report.
#[pyo3::pyclass]
struct ShaderReflectionObject {
    value: serde_json::Value,
}

impl ShaderReflectionObject {
    fn field(&self, key: &str) -> Option<&serde_json::Value> {
        self.value.get(key)
    }

    fn field_or_empty(&self, key: &str) -> serde_json::Value {
        self.field(key).cloned().unwrap_or(serde_json::Value::Null)
    }
}

#[pyo3::pymethods]
impl ShaderReflectionObject {
    fn __getattr__(
        &self,
        py: Python<'_>,
        name: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let key = match name {
            "entryPoint" => "entry_point",
            "sourceFiles" => "source_files",
            "compileFlags" => "compile_flags",
            "readOnlyResources" => "read_only_resources",
            "readWriteResources" => "read_write_resources",
            "constantBlocks" => "constant_blocks",
            "sourceEncoding" => "source_encoding",
            "sourceEntry" => "source_entry",
            "targetEncodings" => "target_encodings",
            "dispatchThreads" => "dispatch_threads",
            _ => name,
        };
        json_field_object(py, &self.value, key)
    }

    fn __getitem__(
        &self,
        py: Python<'_>,
        key: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Err(pyo3::exceptions::PyKeyError::new_err(key.to_owned())),
        }
    }

    #[pyo3(signature = (key, default = None))]
    fn get(
        &self,
        py: Python<'_>,
        key: &str,
        default: Option<Bound<'_, pyo3::PyAny>>,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Ok(default.map(Bound::unbind).unwrap_or_else(|| py.None())),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[allow(non_snake_case)]
    fn GetEntryPoint(&self) -> String {
        self.field("entry_point")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    #[allow(non_snake_case)]
    fn GetDisassembly(&self) -> String {
        self.field("disassembly")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    #[allow(non_snake_case)]
    fn GetInputSig(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let values = self
            .field("inputs")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        field_object_list(py, values, |value| ShaderSignatureObject { value })
    }

    #[allow(non_snake_case)]
    fn GetOutputSig(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let values = self
            .field("outputs")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        field_object_list(py, values, |value| ShaderSignatureObject { value })
    }

    #[allow(non_snake_case)]
    #[pyo3(signature = (index = 0))]
    fn GetInputSignature(
        &self,
        index: usize,
    ) -> pyo3::PyResult<ShaderSignatureObject> {
        let value = self
            .field("inputs")
            .and_then(|inputs| inputs.get(index))
            .cloned()
            .ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err("Input signature index is out of range")
            })?;
        Ok(ShaderSignatureObject { value })
    }

    #[allow(non_snake_case)]
    #[pyo3(signature = (index = 0))]
    fn GetOutputSignature(
        &self,
        index: usize,
    ) -> pyo3::PyResult<ShaderSignatureObject> {
        let value = self
            .field("outputs")
            .and_then(|outputs| outputs.get(index))
            .cloned()
            .ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err("Output signature index is out of range")
            })?;
        Ok(ShaderSignatureObject { value })
    }

    #[allow(non_snake_case)]
    fn GetConstantBlocks(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let values = self
            .field("constant_block_details")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        field_object_list(py, values, |value| ConstantBlockObject { value })
    }

    #[allow(non_snake_case)]
    fn GetReadOnlyResources(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let values = self
            .field("read_only_details")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        field_object_list(py, values, |value| ReflectedResourceObject { value })
    }

    #[allow(non_snake_case)]
    fn GetReadWriteResources(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let values = self
            .field("read_write_details")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        field_object_list(py, values, |value| ReflectedResourceObject { value })
    }

    #[allow(non_snake_case)]
    fn GetSamplers(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let values = self
            .field("sampler_details")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        field_object_list(py, values, |value| SamplerDescriptorObject { value })
    }

    #[allow(non_snake_case)]
    #[pyo3(signature = (index = 0))]
    fn GetSampler(&self, index: usize) -> pyo3::PyResult<SamplerDescriptorObject> {
        let value = self
            .field("sampler_details")
            .and_then(|samplers| samplers.get(index))
            .cloned()
            .ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err("Sampler index is out of range")
            })?;
        Ok(SamplerDescriptorObject { value })
    }

    #[allow(non_snake_case)]
    fn GetSourceFiles(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        to_py(py, &self.field_or_empty("source_files"))
    }

    #[allow(non_snake_case)]
    fn GetCompileFlags(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        to_py(py, &self.field_or_empty("compile_flags"))
    }

    #[allow(non_snake_case)]
    fn GetTargetEncodings(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        to_py(py, &self.field_or_empty("target_encodings"))
    }

    #[allow(non_snake_case)]
    fn IsDebuggable(&self) -> bool {
        self.field("debuggable")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    }
}

/// `ShaderReflection(stage, event=None, target=None)` -> shader reflection
/// object for the current (or specified) event.
#[pyo3::pyfunction]
#[pyo3(signature = (stage, event = None, target = None))]
#[allow(non_snake_case)]
fn ShaderReflection(
    stage: u32,
    event: Option<u32>,
    target: Option<u32>,
) -> pyo3::PyResult<ShaderReflectionObject> {
    let session = active_session()?;
    let event = match event {
        Some(event) => event,
        None => {
            let current = active_event();
            if current == 0 {
                return Err(pyo3::exceptions::PyRuntimeError::new_err(
                    "No event is selected",
                ));
            }
            current
        }
    };
    session
        .pipeline_for_event(event)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    set_active_event(Some(event));
    let report = session
        .shader(stage, target)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    let value = serde_json::to_value(&report)
        .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
    Ok(ShaderReflectionObject { value })
}

/// Field-addressable action description with upstream camelCase aliases and
/// `GetChildren()`.
#[pyo3::pyclass]
struct ActionObject {
    value: serde_json::Value,
}

#[pyo3::pymethods]
impl ActionObject {
    fn __getattr__(
        &self,
        py: Python<'_>,
        name: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let key = match name {
            "eventId" => "event_id",
            "actionId" => "action_id",
            "numIndices" => "indices",
            "numInstances" => "instances",
            _ => name,
        };
        json_field_object(py, &self.value, key)
    }

    fn __getitem__(
        &self,
        py: Python<'_>,
        key: &str,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Err(pyo3::exceptions::PyKeyError::new_err(key.to_owned())),
        }
    }

    #[pyo3(signature = (key, default = None))]
    fn get(
        &self,
        py: Python<'_>,
        key: &str,
        default: Option<Bound<'_, pyo3::PyAny>>,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        match self.value.get(key) {
            Some(value) => to_py(py, value),
            None => Ok(default.map(Bound::unbind).unwrap_or_else(|| py.None())),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// `GetChildren()` -> direct child actions.
    #[allow(non_snake_case)]
    fn GetChildren(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        let event = self
            .value
            .get("event_id")
            .and_then(serde_json::Value::as_u64);
        let actions = active_actions();
        let index = actions
            .iter()
            .position(|action| Some(u64::from(action.event_id)) == event);
        let children: Vec<crate::replay::Action> = match index {
            Some(index) => actions
                .into_iter()
                .filter(|action| action.parent == index as i64)
                .collect(),
            None => Vec::new(),
        };
        to_py(py, &children)
    }
}

/// `Action(event)` -> field-addressable action description, or None.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn Action(event: u32) -> Option<ActionObject> {
    find_action_for_event(event)
        .and_then(|action| serde_json::to_value(action).ok())
        .map(|value| ActionObject { value })
}

/// `pipeline_shader(event, stage)` -> the bound shader entry for a stage.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn pipeline_shader(
    py: Python<'_>,
    event: u32,
    stage: u32,
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let pipeline = session
        .pipeline_for_event(event)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    set_active_event(Some(event));
    let shader = pipeline.shaders.get(stage as usize).ok_or_else(|| {
        pyo3::exceptions::PyValueError::new_err("Invalid shader stage")
    })?;
    to_py(py, shader)
}

/// `pipeline_output(event, index=0)` -> one bound output resource.
#[pyo3::pyfunction]
#[pyo3(signature = (event, index = 0))]
#[allow(non_snake_case)]
fn pipeline_output(
    py: Python<'_>,
    event: u32,
    index: usize,
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let pipeline = session
        .pipeline_for_event(event)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    set_active_event(Some(event));
    let output = pipeline.outputs.get(index).ok_or_else(|| {
        pyo3::exceptions::PyValueError::new_err("Output index is out of range")
    })?;
    to_py(py, output)
}

/// `pipeline_color_attachment(event, index=0)` -> a color attachment.
#[pyo3::pyfunction]
#[pyo3(signature = (event, index = 0))]
#[allow(non_snake_case)]
fn pipeline_color_attachment(
    py: Python<'_>,
    event: u32,
    index: usize,
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let pipeline = session
        .pipeline_for_event(event)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    set_active_event(Some(event));
    let attachment = pipeline
        .color_attachments
        .as_ref()
        .and_then(|attachments| attachments.get(index))
        .ok_or_else(|| {
            pyo3::exceptions::PyValueError::new_err("Color attachment index is out of range")
        })?;
    to_py(py, attachment)
}

/// `action_info(event)` -> the action containing an event.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn action_info(py: Python<'_>, event: u32) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    match find_action_for_event(event) {
        Some(action) => to_py(py, &action),
        None => Ok(py.None()),
    }
}

/// `action_at(index)` -> one bound action by list index.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn action_at(py: Python<'_>, index: usize) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    match active_actions().get(index) {
        Some(action) => to_py(py, action),
        None => Ok(py.None()),
    }
}

/// `action_children(index)` -> direct child actions of a bound action.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn action_children(py: Python<'_>, index: usize) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let children: Vec<crate::replay::Action> = active_actions()
        .into_iter()
        .enumerate()
        .filter(|(_, action)| action.parent == index as i64)
        .map(|(_, action)| action)
        .collect();
    to_py(py, &children)
}

/// `action_draws()` -> draw and mesh-dispatch actions.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn action_draws(py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let actions: Vec<crate::replay::Action> = active_actions()
        .into_iter()
        .filter(|action| action.flags & DRAW_ACTION_FLAGS != 0)
        .collect();
    to_py(py, &actions)
}

/// `texture_info(index)` -> full texture description, or None.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn texture_info(py: Python<'_>, index: usize) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    match active_textures().get(index) {
        Some(texture) => to_py(py, texture),
        None => Ok(py.None()),
    }
}

/// `buffer_info(index)` -> full buffer description, or None.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn buffer_info(py: Python<'_>, index: usize) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    match active_buffers().get(index) {
        Some(buffer) => to_py(py, buffer),
        None => Ok(py.None()),
    }
}

/// `resource_info(index)` -> full resource description, or None.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn resource_info(py: Python<'_>, index: usize) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    match active_resources().get(index) {
        Some(resource) => to_py(py, resource),
        None => Ok(py.None()),
    }
}

// Collected shader debug timeline for breakpoint/step navigation. The
// navigator is recreated around each operation because its variable cache uses
// RefCell and is not Send.
static ACTIVE_DEBUG_TRACE: Mutex<Option<crate::replay::DebugTrace>> = Mutex::new(None);
static ACTIVE_DEBUG_TIMELINE: Mutex<Option<PythonDebugTimeline>> = Mutex::new(None);

struct PythonDebugTimeline {
    trace: crate::replay::DebugTrace,
    states: Vec<crate::replay::DebugState>,
    position: usize,
    breakpoints: std::collections::BTreeSet<crate::debug_navigation::Breakpoint>,
}

fn with_navigator<T>(
    timeline: &mut PythonDebugTimeline,
    action: impl FnOnce(&mut crate::debug_navigation::DebugNavigator, &crate::replay::DebugTrace, &[crate::replay::DebugState]) -> T,
) -> T {
    let mut navigator = crate::debug_navigation::DebugNavigator::default();
    navigator.position = timeline.position;
    navigator.breakpoints = timeline.breakpoints.clone();
    let result = action(&mut navigator, &timeline.trace, &timeline.states);
    timeline.position = navigator.position;
    timeline.breakpoints = navigator.breakpoints.clone();
    result
}

/// `debug_collect()` -> collect the remaining states of the active debug trace
/// and keep a local timeline for breakpoints and `RunForward`.
#[pyo3::pyfunction]
fn debug_collect(py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let trace = ACTIVE_DEBUG_TRACE
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
        .ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err("Start a shader debug trace first")
        })?;
    let mut states = Vec::new();
    loop {
        let batch = session
            .debug_next()
            .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
        if batch.event_id != trace.event_id {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(
                "Debug states belong to a different event",
            ));
        }
        states.extend(batch.states);
        if batch.finished {
            break;
        }
    }
    session
        .debug_stop()
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    let response = serde_json::json!({
        "event_id": trace.event_id,
        "stage": trace.stage,
        "total": states.len(),
        "breakpoints": [],
    });
    *ACTIVE_DEBUG_TIMELINE
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(PythonDebugTimeline {
        trace,
        states,
        position: 0,
        breakpoints: Default::default(),
    });
    to_py(py, &response)
}

/// `debug_timeline()` -> current debug timeline position/breakpoints.
#[pyo3::pyfunction]
fn debug_timeline(py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let guard = ACTIVE_DEBUG_TIMELINE
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let timeline = guard.as_ref().ok_or_else(|| {
        pyo3::exceptions::PyRuntimeError::new_err("Collect a shader debug trace first")
    })?;
    to_py(
        py,
        &serde_json::json!({
            "event_id": timeline.trace.event_id,
            "stage": timeline.trace.stage,
            "position": timeline.position,
            "total": timeline.states.len(),
            "breakpoints": timeline.breakpoints,
        }),
    )
}

fn toggle_timeline_breakpoint(
    breakpoint: crate::debug_navigation::Breakpoint,
    enabled: bool,
) -> Result<(), String> {
    let mut guard = ACTIVE_DEBUG_TIMELINE
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let timeline = guard
        .as_mut()
        .ok_or_else(|| "Collect a shader debug trace first".to_owned())?;
    with_navigator(timeline, |navigator, trace, _| {
        navigator.set_breakpoint(trace, breakpoint, enabled)
    })
    .map_err(str::to_owned)
}

/// `debug_set_breakpoint(kind, line, enabled=True)` -> set/clear a disassembly
/// or source breakpoint on the collected timeline.
#[pyo3::pyfunction]
#[pyo3(signature = (kind, line, enabled = true, file_index = 0))]
fn debug_set_breakpoint(
    py: Python<'_>,
    kind: String,
    line: u32,
    enabled: bool,
    file_index: i32,
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let breakpoint = match kind.as_str() {
        "disassembly" => crate::debug_navigation::Breakpoint::Disassembly { line },
        "source" => crate::debug_navigation::Breakpoint::Source { file_index, line },
        _ => {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "Breakpoint kind must be disassembly or source",
            ))
        }
    };
    let mut guard = ACTIVE_DEBUG_TIMELINE
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let timeline = guard.as_mut().ok_or_else(|| {
        pyo3::exceptions::PyRuntimeError::new_err("Collect a shader debug trace first")
    })?;
    with_navigator(timeline, |navigator, trace, _| {
        navigator.set_breakpoint(trace, breakpoint, enabled)
    })
    .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
    to_py(
        py,
        &serde_json::json!({
            "position": timeline.position,
            "breakpoints": timeline.breakpoints,
        }),
    )
}

// pyrenderdoc-style resource replacement map (original index -> replacement).
static REPLACEMENTS: Mutex<Option<std::collections::HashMap<u32, u32>>> = Mutex::new(None);

fn replacement_map() -> std::collections::HashMap<u32, u32> {
    REPLACEMENTS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
        .unwrap_or_default()
}

/// Rewrite `resource_index` fields so script-visible pipeline bindings reflect
/// registered replacements, matching qrenderdoc's display substitution.
fn apply_replacements(value: &mut serde_json::Value) {
    let replacements = replacement_map();
    if replacements.is_empty() {
        return;
    }
    match value {
        serde_json::Value::Object(object) => {
            if let Some(index) = object
                .get("resource_index")
                .and_then(serde_json::Value::as_u64)
                .and_then(|index| u32::try_from(index).ok())
            {
                if let Some(replacement) = replacements.get(&index) {
                    object.insert(
                        "resource_index".into(),
                        serde_json::Value::from(*replacement),
                    );
                }
            }
            for value in object.values_mut() {
                apply_replacements(value);
            }
        }
        serde_json::Value::Array(array) => {
            for value in array {
                apply_replacements(value);
            }
        }
        _ => {}
    }
}

/// `RegisterReplacement(original, replacement)` -> map a resource index to a
/// replacement resource for script-visible pipeline bindings.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn RegisterReplacement(original: u32, replacement: u32) -> pyo3::PyResult<()> {
    let mut guard = REPLACEMENTS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    guard
        .get_or_insert_with(std::collections::HashMap::new)
        .insert(original, replacement);
    Ok(())
}

/// `UnregisterReplacement(original)` -> drop a resource replacement.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn UnregisterReplacement(original: u32) -> pyo3::PyResult<()> {
    if let Some(map) = REPLACEMENTS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_mut()
    {
        map.remove(&original);
    }
    Ok(())
}

/// `IsResourceReplaced(original)` -> whether a replacement is registered.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn IsResourceReplaced(original: u32) -> bool {
    replacement_map().contains_key(&original)
}

/// `GetResourceReplacement(original)` -> replacement index, or None.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn GetResourceReplacement(original: u32) -> Option<u32> {
    replacement_map().get(&original).copied()
}

/// Bind the serialized snapshot document for `renderdocx.snapshot()`.
pub fn set_active_snapshot(snapshot: serde_json::Value) {
    *ACTIVE_SNAPSHOT
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(snapshot);
}

fn active_snapshot() -> Option<serde_json::Value> {
    ACTIVE_SNAPSHOT
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

/// Record the event that `CurEvent()` and `CurAction()` should report.
pub fn set_active_event(event: Option<u32>) {
    ACTIVE_EVENT.store(event.unwrap_or(0), Ordering::SeqCst);
}

fn active_event() -> u32 {
    ACTIVE_EVENT.load(Ordering::SeqCst)
}

/// Record the capture filename exposed by `GetCaptureFilename()`.
pub fn set_active_capture(path: Option<String>) {
    *ACTIVE_CAPTURE_PATH
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = path;
}

fn find_action_for_event(event: u32) -> Option<crate::replay::Action> {
    let actions = active_actions();
    actions
        .iter()
        .find(|action| action.event_id == event)
        .or_else(|| {
            actions.iter().find(|action| {
                action
                    .api_events
                    .iter()
                    .any(|api_event| api_event.event_id == event)
            })
        })
        .cloned()
}

// --- pyrenderdoc CaptureContext compatibility surface -------------------
// These use the same names and shapes as the upstream wrapper where the
// underlying data exists in RenderDocX's generic replay model.

/// `CurEvent()` -> last event selected or queried (0 when none).
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn CurEvent() -> u32 {
    active_event()
}

/// `CurSelectedEvent()` -> same as `CurEvent()` in RenderDocX's single-event
/// model.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn CurSelectedEvent() -> u32 {
    active_event()
}

/// `CurAction()` -> action containing the current event, or None.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn CurAction(py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    match find_action_for_event(active_event()) {
        Some(action) => to_py(py, &action),
        None => Ok(py.None()),
    }
}

/// `CurSelectedAction()` -> same as `CurAction()`.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn CurSelectedAction(py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    CurAction(py)
}

/// `CurRootActions()` -> bound root action list.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn CurRootActions(py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    to_py(py, &active_actions())
}

/// `CurPipelineState()` -> pipeline for the current event.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn CurPipelineState() -> pyo3::PyResult<PipelineObject> {
    let event = active_event();
    if event == 0 {
        return Err(pyo3::exceptions::PyRuntimeError::new_err(
            "No event is selected",
        ));
    }
    let session = active_session()?;
    let value = session
        .pipeline_for_event(event)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    let mut document = serde_json::to_value(&value).map_err(|error| {
        pyo3::exceptions::PyRuntimeError::new_err(error.to_string())
    })?;
    apply_replacements(&mut document);
    Ok(PipelineObject { value: document })
}

/// `GetAction(event)` -> action containing the event, or None.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn GetAction(py: Python<'_>, event: u32) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    match find_action_for_event(event) {
        Some(action) => to_py(py, &action),
        None => Ok(py.None()),
    }
}

/// `GetResources()` -> bound resource list.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn GetResources(py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    to_py(py, &active_resources())
}

/// `GetTextures()` -> bound texture list with full descriptions.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn GetTextures(py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    to_py(py, &active_textures())
}

/// `GetBuffers()` -> bound buffer list with full descriptions.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn GetBuffers(py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    to_py(py, &active_buffers())
}

/// `FrameInfo()` -> captured frame info object, or None.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn FrameInfo(py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    match active_snapshot().and_then(|snapshot| snapshot.get("frame_info").cloned()) {
        Some(value) if !value.is_null() => to_py(py, &value),
        _ => Ok(py.None()),
    }
}

/// `APIProps()` -> API and replay properties, or None.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn APIProps(py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let Some(snapshot) = active_snapshot() else {
        return Ok(py.None());
    };
    let value = serde_json::json!({
        "api": snapshot.get("api").cloned().unwrap_or(serde_json::Value::Null),
        "local_renderer": snapshot.get("local_renderer").cloned().unwrap_or(serde_json::Value::Null),
        "replay_options": snapshot.get("replay_options").cloned().unwrap_or(serde_json::Value::Null),
    });
    to_py(py, &value)
}

/// `IsCaptureLoaded()` -> whether a replay session is bound.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn IsCaptureLoaded() -> bool {
    active_session().is_ok()
}

/// `GetCaptureFilename()` -> bound capture path (empty when none).
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn GetCaptureFilename() -> String {
    ACTIVE_CAPTURE_PATH
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
        .unwrap_or_default()
}

/// `DebugMessages()` -> capture debug messages from the replay controller.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn DebugMessages(py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let value = session
        .debug_messages()
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    to_py(py, &value)
}

/// Minimal `pyrenderdoc.Replay()` controller wrapper exposing the upstream
/// method names over RenderDocX's bound session and snapshot.
#[pyo3::pyclass]
struct ReplayController;

#[pyo3::pymethods]
impl ReplayController {
    #[allow(non_snake_case)]
    fn GetPipelineState(&self) -> pyo3::PyResult<PipelineObject> {
        CurPipelineState()
    }

    #[allow(non_snake_case)]
    fn GetTextures(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        GetTextures(py)
    }

    #[allow(non_snake_case)]
    fn GetBuffers(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        GetBuffers(py)
    }

    #[allow(non_snake_case)]
    fn GetResources(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        GetResources(py)
    }

    #[allow(non_snake_case)]
    fn GetFrameInfo(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        FrameInfo(py)
    }

    #[allow(non_snake_case)]
    fn GetAPIProperties(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        APIProps(py)
    }

    #[allow(non_snake_case)]
    fn GetDebugMessages(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        DebugMessages(py)
    }

    #[allow(non_snake_case)]
    fn SetFrameEvent(&self, event: u32) -> pyo3::PyResult<()> {
        let session = active_session()?;
        select_event(&session, event)
    }

    #[allow(non_snake_case)]
    #[pyo3(signature = (event, vertex, instance = 0, index = None, view = 0))]
    fn DebugVertex(
        &self,
        py: Python<'_>,
        event: u32,
        vertex: u32,
        instance: u32,
        index: Option<u32>,
        view: u32,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        debug_vertex(py, event, vertex, instance, index, view)
    }

    #[allow(non_snake_case)]
    #[pyo3(signature = (event, x, y, sample = None, primitive = None, view = None))]
    fn DebugPixel(
        &self,
        py: Python<'_>,
        event: u32,
        x: u32,
        y: u32,
        sample: Option<u32>,
        primitive: Option<u32>,
        view: Option<u32>,
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        debug_pixel(py, event, x, y, sample, primitive, view)
    }

    #[allow(non_snake_case)]
    #[pyo3(signature = (event, group = [0, 0, 0], thread = [0, 0, 0]))]
    fn DebugCompute(
        &self,
        py: Python<'_>,
        event: u32,
        group: [u32; 3],
        thread: [u32; 3],
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        debug_compute(py, event, group, thread)
    }

    #[allow(non_snake_case)]
    #[pyo3(signature = (event, group = [0, 0, 0], thread = [0, 0, 0]))]
    fn DebugMesh(
        &self,
        py: Python<'_>,
        event: u32,
        group: [u32; 3],
        thread: [u32; 3],
    ) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        debug_mesh(py, event, group, thread)
    }

    /// Upstream `ContinueDebug` returns the next batch of states.
    #[allow(non_snake_case)]
    fn ContinueDebug(&self, py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
        debug_next(py)
    }

    /// Upstream `FreeTrace` ends and releases the active trace.
    #[allow(non_snake_case)]
    fn FreeTrace(&self) -> pyo3::PyResult<()> {
        debug_stop()
    }
}

/// `Replay()` -> controller-style wrapper (GetPipelineState, GetTextures,
/// GetBuffers, GetResources, GetFrameInfo, GetAPIProperties, GetDebugMessages,
/// SetFrameEvent, DebugVertex/Pixel/Compute/Mesh, ContinueDebug, FreeTrace).
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn Replay() -> ReplayController {
    ReplayController
}

/// `GetNotes()` -> capture comments string from the Notes section.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn GetNotes() -> pyo3::PyResult<String> {
    let session = active_session()?;
    let document = session
        .annotation_read("notes")
        .and_then(|section| section.document())
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    Ok(document
        .get("comments")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned())
}

/// `SetNotes(text)` -> replace capture comments in the Notes section.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn SetNotes(text: String) -> pyo3::PyResult<()> {
    let session = active_session()?;
    let section = session
        .annotation_read("notes")
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    let document = section
        .document()
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)
        .and_then(|document| {
            crate::annotations::set_comment(&document, &text)
                .map_err(pyo3::exceptions::PyRuntimeError::new_err)
        })?;
    session
        .annotation_write(&section, &document)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    Ok(())
}

/// `GetBookmarks()` -> list of `{eventId, text}` bookmarks.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn GetBookmarks(py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let document = session
        .annotation_read("bookmarks")
        .and_then(|section| section.document())
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    let bookmarks = crate::annotations::bookmarks(&document)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    let list: Vec<serde_json::Value> = bookmarks
        .into_iter()
        .map(|(event, text)| serde_json::json!({"eventId": event, "text": text}))
        .collect();
    to_py(py, &list)
}

/// `SetBookmark(event, text)` -> add or update an event bookmark.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn SetBookmark(event: u32, text: String) -> pyo3::PyResult<()> {
    let session = active_session()?;
    let section = session
        .annotation_read("bookmarks")
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    let document = section
        .document()
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)
        .and_then(|document| {
            crate::annotations::set_bookmark(&document, event, Some(&text))
                .map_err(pyo3::exceptions::PyRuntimeError::new_err)
        })?;
    session
        .annotation_write(&section, &document)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    Ok(())
}

/// `RemoveBookmark(event)` -> remove the bookmark on an event, if present.
#[pyo3::pyfunction]
#[allow(non_snake_case)]
fn RemoveBookmark(event: u32) -> pyo3::PyResult<()> {
    let session = active_session()?;
    let section = session
        .annotation_read("bookmarks")
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    let document = section
        .document()
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)
        .and_then(|document| {
            crate::annotations::set_bookmark(&document, event, None)
                .map_err(pyo3::exceptions::PyRuntimeError::new_err)
        })?;
    session
        .annotation_write(&section, &document)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    Ok(())
}

/// `renderdocx.snapshot()` -> the full capture snapshot as a native Python
/// object (frame info, counters, actions, resources, textures, buffers...).
#[pyo3::pyfunction]
fn snapshot(py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let value = active_snapshot().ok_or_else(|| {
        pyo3::exceptions::PyRuntimeError::new_err("No capture snapshot is bound")
    })?;
    to_py(py, &value)
}

/// `renderdocx.capture_actions()` -> bound action list as native objects,
/// including event ids, flags and api_events for locating draw indices.
#[pyo3::pyfunction]
fn capture_actions(py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    to_py(py, &active_actions())
}

/// Bind formatting context (resources/buffers/textures/language) for scripts.
pub fn set_active_format_context(
    resources: Vec<crate::replay::ResourceInfo>,
    buffers: Vec<crate::replay::Buffer>,
    textures: Vec<crate::replay::Texture>,
    language: crate::i18n::Language,
) {
    *ACTIVE_RESOURCES
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(resources);
    *ACTIVE_BUFFERS
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(buffers);
    *ACTIVE_TEXTURES
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(textures);
    *ACTIVE_LANGUAGE
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = language;
}

fn active_resources() -> Vec<crate::replay::ResourceInfo> {
    ACTIVE_RESOURCES
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
        .unwrap_or_default()
}

fn active_buffers() -> Vec<crate::replay::Buffer> {
    ACTIVE_BUFFERS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
        .unwrap_or_default()
}

fn active_textures() -> Vec<crate::replay::Texture> {
    ACTIVE_TEXTURES
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
        .unwrap_or_default()
}

/// Bind the flattened action list for `vertex_input(action_index)`.
pub fn set_active_actions(actions: Vec<crate::replay::Action>) {
    *ACTIVE_ACTIONS
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(actions);
}

fn active_actions() -> Vec<crate::replay::Action> {
    ACTIVE_ACTIONS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
        .unwrap_or_default()
}

/// `renderdocx.render_texture(destination, index=0, width=64, height=64)` ->
/// render the texture viewer preview (without context highlight) to a PNG.
#[pyo3::pyfunction]
#[pyo3(signature = (destination, index = 0, width = 64, height = 64))]
fn render_texture(
    destination: String,
    index: u32,
    width: u32,
    height: u32,
) -> pyo3::PyResult<String> {
    let session = active_session()?;
    let path = std::path::PathBuf::from(&destination);
    session
        .render_texture_png(
            index,
            crate::replay::TextureSelection::default(),
            width,
            height,
            &path,
        )
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    Ok(destination)
}

/// `renderdocx.mesh_read(event, stage=1, kind="vertex", offset=0, length=256)`
/// -> hex string of mesh vertex or index buffer bytes.
#[pyo3::pyfunction]
#[pyo3(signature = (event, stage = 1, kind = "vertex".to_owned(), offset = 0, length = 256))]
fn mesh_read(
    event: u32,
    stage: u32,
    kind: String,
    offset: u64,
    length: u32,
) -> pyo3::PyResult<String> {
    let session = active_session()?;
    let event = resolve_draw_event(event);
    let selection = crate::replay::MeshSelection {
        instance: 0,
        view: 0,
        stage,
    };
    select_event(&session, event)?;
    // The bridge caches mesh data before its buffers can be read.
    session
        .mesh(event, selection)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    session
        .mesh_read(event, selection, &kind, offset, length)
        .map(|bytes| bytes.iter().map(|byte| format!("{byte:02x}")).collect())
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)
}

/// `renderdocx.export_buffer(destination, index=0, offset=0, length=None)` ->
/// export raw buffer bytes; `length=None` exports to the end of the buffer.
#[pyo3::pyfunction]
#[pyo3(signature = (destination, index = 0, offset = 0, length = None))]
fn export_buffer(
    destination: String,
    index: u32,
    offset: u64,
    length: Option<u64>,
) -> pyo3::PyResult<String> {
    let session = active_session()?;
    let buffers = active_buffers();
    let total = buffers
        .get(index as usize)
        .and_then(|buffer| buffer.length.parse::<u64>().ok())
        .ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(
                "Buffer index is out of range in the bound snapshot",
            )
        })?;
    let length = length.unwrap_or_else(|| total.saturating_sub(offset));
    if offset > total || length > total - offset {
        return Err(pyo3::exceptions::PyRuntimeError::new_err(
            "Buffer range is out of bounds",
        ));
    }
    let path = std::path::PathBuf::from(&destination);
    session
        .export_buffer(index, offset, length, &path)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    Ok(destination)
}

fn remember_debug_trace(trace: &crate::replay::DebugTrace) {
    *ACTIVE_DEBUG_TRACE
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(trace.clone());
    *ACTIVE_DEBUG_TIMELINE
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = None;
}

/// `renderdocx.debug_vertex(event, vertex, instance=0, index=None, view=0)` ->
/// start a vertex-shader debug trace. `index` defaults to `vertex`.
#[pyo3::pyfunction]
#[pyo3(signature = (event, vertex, instance = 0, index = None, view = 0))]
fn debug_vertex(
    py: Python<'_>,
    event: u32,
    vertex: u32,
    instance: u32,
    index: Option<u32>,
    view: u32,
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let event = resolve_draw_event(event);
    select_event(&session, event)?;
    let value = session
        .debug_vertex(vertex, instance, index.unwrap_or(vertex), view)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    remember_debug_trace(&value);
    to_py(py, &value)
}

/// `renderdocx.debug_pixel(event, x, y, sample=None, primitive=None, view=None)`
/// -> start a pixel-shader debug trace.
#[pyo3::pyfunction]
#[pyo3(signature = (event, x, y, sample = None, primitive = None, view = None))]
fn debug_pixel(
    py: Python<'_>,
    event: u32,
    x: u32,
    y: u32,
    sample: Option<u32>,
    primitive: Option<u32>,
    view: Option<u32>,
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let event = resolve_draw_event(event);
    select_event(&session, event)?;
    let value = session
        .debug_pixel(crate::replay::PixelDebugSelection {
            x,
            y,
            sample,
            primitive,
            view,
        })
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    remember_debug_trace(&value);
    to_py(py, &value)
}

/// `renderdocx.debug_compute(event, group=[0,0,0], thread=[0,0,0])` -> start a
/// compute-shader debug trace.
#[pyo3::pyfunction]
#[pyo3(signature = (event, group = [0, 0, 0], thread = [0, 0, 0]))]
fn debug_compute(
    py: Python<'_>,
    event: u32,
    group: [u32; 3],
    thread: [u32; 3],
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    select_event(&session, event)?;
    let value = session
        .debug_compute(crate::replay::ComputeDebugSelection { group, thread })
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    remember_debug_trace(&value);
    to_py(py, &value)
}

/// `renderdocx.debug_mesh(event, group=(0,0,0), thread=(0,0,0))` -> start a
/// mesh-shader debug trace and return its first state.
#[pyo3::pyfunction]
#[pyo3(signature = (event, group = [0, 0, 0], thread = [0, 0, 0]))]
fn debug_mesh(
    py: Python<'_>,
    event: u32,
    group: [u32; 3],
    thread: [u32; 3],
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let event = resolve_draw_event(event);
    select_event(&session, event)?;
    let value = session
        .debug_mesh(crate::replay::ComputeDebugSelection { group, thread })
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    remember_debug_trace(&value);
    to_py(py, &value)
}

/// `renderdocx.debug_next()` -> advance the active debug trace one step.
#[pyo3::pyfunction]
fn debug_next(py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let value = session
        .debug_next()
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    to_py(py, &value)
}

/// `renderdocx.debug_stop()` -> end the active debug trace.
#[pyo3::pyfunction]
fn debug_stop() -> pyo3::PyResult<()> {
    let session = active_session()?;
    session
        .debug_stop()
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    *ACTIVE_DEBUG_TRACE
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = None;
    *ACTIVE_DEBUG_TIMELINE
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = None;
    Ok(())
}

/// `renderdocx.export_texture(destination, index=0)` -> export a PNG of the
/// texture. Existing files are never replaced (matching the GUI exporter).
#[pyo3::pyfunction]
#[pyo3(signature = (destination, index = 0))]
fn export_texture(destination: String, index: u32) -> pyo3::PyResult<String> {
    let session = active_session()?;
    let path = std::path::PathBuf::from(&destination);
    session
        .export_texture(index, &path)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    Ok(destination)
}

/// `renderdocx.texture_context(destination, index=0, width=64, height=64)` ->
/// render the texture context view (including its highlight) to a PNG.
#[pyo3::pyfunction]
#[pyo3(signature = (destination, index = 0, width = 64, height = 64))]
fn texture_context(
    destination: String,
    index: u32,
    width: u32,
    height: u32,
) -> pyo3::PyResult<String> {
    let session = active_session()?;
    let path = std::path::PathBuf::from(&destination);
    session
        .render_texture_context_png(
            index,
            crate::replay::TextureSelection::default(),
            width,
            height,
            &path,
        )
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    Ok(destination)
}

/// `renderdocx.texture_minmax(event, texture_index=0, channels=(True,)*4)` ->
/// native extrema object for a texture in the bound snapshot.
#[pyo3::pyfunction]
#[pyo3(signature = (event, texture_index = 0, channels = None))]
fn texture_minmax(
    py: Python<'_>,
    event: u32,
    texture_index: usize,
    channels: Option<Vec<bool>>,
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let textures = active_textures();
    let texture = textures.get(texture_index).ok_or_else(|| {
        pyo3::exceptions::PyRuntimeError::new_err("Texture index is out of range in the bound snapshot")
    })?;
    let channels = channel_mask(channels);
    select_event(&session, event)?;
    let value = session
        .texture_minmax(
            event,
            texture_index as u32,
            texture,
            crate::replay::TextureSelection::default(),
            channels,
        )
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    to_py(py, &value)
}

/// `renderdocx.texture_fit_range(event, texture_index=0, channels=None)` ->
/// `[min, max]` fitted to the enabled channels, mirroring the texture viewer.
#[pyo3::pyfunction]
#[pyo3(signature = (event, texture_index = 0, channels = None))]
fn texture_fit_range(
    py: Python<'_>,
    event: u32,
    texture_index: usize,
    channels: Option<Vec<bool>>,
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let textures = active_textures();
    let texture = textures.get(texture_index).ok_or_else(|| {
        pyo3::exceptions::PyRuntimeError::new_err("Texture index is out of range in the bound snapshot")
    })?;
    let channels = channel_mask(channels);
    let selection = crate::replay::TextureSelection::default();
    select_event(&session, event)?;
    let stats = session
        .texture_minmax(
            event,
            texture_index as u32,
            texture,
            selection,
            channels,
        )
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    let range = crate::texture_view::fit_range(
        &stats,
        texture.component_type,
        selection.analysis_type_cast,
        texture.component_count,
        channels,
        texture.is_alpha_only(),
    )
    .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    // Fixed-size arrays serialize as tuples; use a Vec so scripts get a list.
    to_py(py, &range.to_vec())
}

fn channel_mask(channels: Option<Vec<bool>>) -> [bool; 4] {
    channels
        .and_then(|channels| <[bool; 4]>::try_from(channels).ok())
        .unwrap_or([true; 4])
}

/// Mirror of upstream `ActionFlags::Drawcall | ActionFlags::MeshDispatch`.
const DRAW_ACTION_FLAGS: u32 = 0x2 | 0x8;
/// Mirror of upstream `ActionFlags::Drawcall` (input assembly only).
const DRAWCALL_FLAG: u32 = 0x2;

/// Resolve an event id to the draw/mesh action that contains it. The replay
/// bridge keys mesh data by the action's own event id (e.g. the
/// `vkCmdDrawMeshTasksEXT` call), while callers may pass any event inside that
/// action (such as a preceding `vkCmdSetViewport`).
fn resolve_draw_event(event: u32) -> u32 {
    let actions = active_actions();
    if let Some(action) = actions
        .iter()
        .find(|action| action.event_id == event && action.flags & DRAW_ACTION_FLAGS != 0)
    {
        return action.event_id;
    }
    actions
        .iter()
        .find(|action| {
            action.flags & DRAW_ACTION_FLAGS != 0
                && action
                    .api_events
                    .iter()
                    .any(|api_event| api_event.event_id == event)
        })
        .map_or(event, |action| action.event_id)
}

/// `renderdocx.mesh(event, stage=1)` -> native Python object for the mesh view
/// (stage 1=vs_out, 2=gs_out, 3=task_out, 4=mesh_out). Any event inside a draw
/// or mesh dispatch resolves to that action's event id.
#[pyo3::pyfunction]
#[pyo3(signature = (event, stage = 1))]
fn mesh(py: Python<'_>, event: u32, stage: u32) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let event = resolve_draw_event(event);
    let selection = crate::replay::MeshSelection {
        instance: 0,
        view: 0,
        stage,
    };
    // Selecting the event first is required by the replay driver's mesh query.
    session
        .pipeline_for_event(event)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    set_active_event(Some(event));
    let value = session
        .mesh(event, selection)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    to_py(py, &value)
}

/// `renderdocx.vertex_input(action_index=None, instance=0, offset=0, limit=64)`
/// -> native Python input page for an action from the bound snapshot. When
/// `action_index` is omitted the first draw action is used.
#[pyo3::pyfunction]
#[pyo3(signature = (action_index = None, instance = 0, offset = 0, limit = 64))]
fn vertex_input(
    py: Python<'_>,
    action_index: Option<usize>,
    instance: u32,
    offset: u32,
    limit: u32,
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let actions = active_actions();
    let index = action_index.unwrap_or_else(|| {
        actions
            .iter()
            .position(|action| action.flags & DRAWCALL_FLAG != 0)
            .unwrap_or(0)
    });
    let action = actions.get(index).ok_or_else(|| {
        pyo3::exceptions::PyRuntimeError::new_err("Action index is out of range in the bound snapshot")
    })?;
    let pipeline = session
        .pipeline_for_event(action.event_id)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    set_active_event(Some(action.event_id));
    let buffers = active_buffers();
    let value = session
        .vertex_input_page(action, &pipeline, &buffers, instance, offset, limit)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    to_py(py, &value)
}

fn active_language() -> crate::i18n::Language {
    *ACTIVE_LANGUAGE
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

/// `renderdocx.event_format(event, show_all=False, show_names=False)` -> the
/// formatted event name/parameter text (uses the bound snapshot context).
#[pyo3::pyfunction]
#[pyo3(signature = (event, show_all = false, show_names = false))]
fn event_format(event: u32, show_all: bool, show_names: bool) -> pyo3::PyResult<String> {
    let session = active_session()?;
    let resources = active_resources();
    let buffers = active_buffers();
    crate::event_parameters::format_event(
        &session,
        event,
        &resources,
        &buffers,
        active_language(),
        show_all,
        show_names,
    )
    .map_err(pyo3::exceptions::PyRuntimeError::new_err)
}

/// `renderdocx.descriptors(event, store=None, offset=0, limit=128)` -> JSON page.
#[pyo3::pyfunction]
#[pyo3(signature = (event, store = None, offset = 0, limit = 128))]
fn descriptors(
    py: Python<'_>,
    event: u32,
    store: Option<usize>,
    offset: u64,
    limit: u32,
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    select_event(&session, event)?;
    let value = session
        .descriptors(event, store, offset, limit)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    let mut document = serde_json::to_value(&value).map_err(|error| {
        pyo3::exceptions::PyRuntimeError::new_err(error.to_string())
    })?;
    apply_replacements(&mut document);
    to_py(py, &document)
}

/// `renderdocx.texture_statistics(event, texture_index=0, min=0.0, max=1.0)`
/// -> JSON statistics/histogram for a texture from the bound snapshot.
#[pyo3::pyfunction]
#[pyo3(signature = (event, texture_index = 0, min = 0.0, max = 1.0))]
fn texture_statistics(
    py: Python<'_>,
    event: u32,
    texture_index: usize,
    min: f32,
    max: f32,
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let textures = active_textures();
    let texture = textures.get(texture_index).ok_or_else(|| {
        pyo3::exceptions::PyRuntimeError::new_err("Texture index is out of range in the bound snapshot")
    })?;
    select_event(&session, event)?;
    let request = crate::replay::HistogramRequest {
        min,
        max,
        channels: [true, true, true, true],
    };
    let value = session
        .texture_statistics(
            event,
            texture_index as u32,
            texture,
            crate::replay::TextureSelection::default(),
            request,
        )
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    to_py(py, &value)
}

/// `renderdocx.pixel_history(event, texture_index=0, x=0, y=0, offset=0, limit=128)`
/// -> JSON page for a texture from the bound snapshot.
#[pyo3::pyfunction]
#[pyo3(signature = (event, texture_index = 0, x = 0, y = 0, offset = 0, limit = 128))]
fn pixel_history(
    py: Python<'_>,
    event: u32,
    texture_index: usize,
    x: u32,
    y: u32,
    offset: u64,
    limit: u32,
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let textures = active_textures();
    let texture = textures.get(texture_index).ok_or_else(|| {
        pyo3::exceptions::PyRuntimeError::new_err("Texture index is out of range in the bound snapshot")
    })?;
    select_event(&session, event)?;
    let value = session
        .pixel_history(
            event,
            texture_index as u32,
            texture,
            crate::replay::TextureSelection::default(),
            x,
            y,
            offset,
            limit,
        )
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    to_py(py, &value)
}

/// Register (or clear) the replay session that Python helpers may query.
/// Clearing the session also drops the bound snapshot context so scripts cannot
/// read stale data after a capture closes.
pub fn set_active_session(session: Option<crate::replay::Session>) {
    let clear = session.is_none();
    let mut guard = ACTIVE_SESSION
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    *guard = session;
    if clear {
        *ACTIVE_RESOURCES
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        *ACTIVE_BUFFERS
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        *ACTIVE_TEXTURES
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        *ACTIVE_ACTIONS
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        *ACTIVE_SNAPSHOT
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        *ACTIVE_CAPTURE_PATH
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        *REPLACEMENTS
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        ACTIVE_EVENT.store(0, Ordering::SeqCst);
    }
}

/// Convert any serializable value into a native Python object (dict/list/scalar)
/// so scripts can use attribute/index access instead of parsing JSON.
/// Build a Python list of field-object pyclasses from JSON values.
fn field_object_list<T, F>(
    py: Python<'_>,
    values: Vec<serde_json::Value>,
    make: F,
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>>
where
    T: pyo3::PyClass<BaseType = pyo3::PyAny>,
    F: Fn(serde_json::Value) -> T,
{
    let mut items = Vec::with_capacity(values.len());
    for value in values {
        items.push(Bound::new(py, make(value))?.into_any().unbind());
    }
    pyo3::types::PyList::new(py, items).map(|list| list.into_any().unbind())
}

fn to_py<T: serde::Serialize>(py: Python<'_>, value: &T) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    pythonize::pythonize(py, value)
        .map(|bound| bound.unbind())
        .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))
}

fn active_session() -> Result<crate::replay::Session, pyo3::PyErr> {
    ACTIVE_SESSION
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
        .ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err("No capture session is open"))
}

/// Select `event` so queued lookups (descriptors, messages and texture queries)
/// observe consistent replay state; the bridge keys several responses by the
/// currently selected event.
fn select_event(session: &crate::replay::Session, event: u32) -> pyo3::PyResult<()> {
    session
        .pipeline_for_event(event)
        .map(|_| set_active_event(Some(event)))
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)
}

/// `renderdocx.pipeline(event)` -> native Python object (dict/list) for the
/// event pipeline, so scripts can use `p["shaders"][0]["stage"]` directly.
#[pyo3::pyfunction]
fn pipeline(py: Python<'_>, event: u32) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let value = session
        .pipeline_for_event(event)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    set_active_event(Some(event));
    let mut document = serde_json::to_value(&value).map_err(|error| {
        pyo3::exceptions::PyRuntimeError::new_err(error.to_string())
    })?;
    apply_replacements(&mut document);
    to_py(py, &document)
}

/// `renderdocx.buffer(index, offset, length)` -> hex string of raw buffer bytes.
#[pyo3::pyfunction]
#[pyo3(signature = (index, offset = 0, length = 256))]
fn buffer(index: u32, offset: u64, length: u32) -> pyo3::PyResult<String> {
    let session = active_session()?;
    session
        .read_buffer(index, offset, length)
        .map(|bytes| bytes.iter().map(|byte| format!("{byte:02x}")).collect())
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)
}

/// `renderdocx.structure(event, offset=0, limit=128)` -> JSON of the event's
/// structured data.
#[pyo3::pyfunction]
#[pyo3(signature = (event, offset = 0, limit = 128))]
fn structure(
    py: Python<'_>,
    event: u32,
    offset: u64,
    limit: u32,
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let value = session
        .structure(event, &[], offset, limit)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    to_py(py, &value)
}

/// `renderdocx.capture_debug_messages()` -> JSON array of capture debug
/// messages (the `debug_messages` name is the pre-fetched list on the module).
#[pyo3::pyfunction]
fn capture_debug_messages(py: Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let value = session
        .debug_messages()
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    to_py(py, &value)
}

/// `renderdocx.shader_messages(event, offset=0, limit=128)` -> JSON page.
#[pyo3::pyfunction]
#[pyo3(signature = (event, offset = 0, limit = 128))]
fn shader_messages(
    py: Python<'_>,
    event: u32,
    offset: u64,
    limit: u32,
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    select_event(&session, event)?;
    let value = session
        .shader_messages(event, offset, limit)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    to_py(py, &value)
}

/// `renderdocx.resource_usage(index, offset=0, limit=128)` -> JSON page.
#[pyo3::pyfunction]
#[pyo3(signature = (index, offset = 0, limit = 128))]
fn resource_usage(
    py: Python<'_>,
    index: u32,
    offset: u64,
    limit: u32,
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let value = session
        .resource_usage(index, offset, limit)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    to_py(py, &value)
}

/// `renderdocx.shader(stage, target=None)` -> JSON shader report.
#[pyo3::pyfunction]
#[pyo3(signature = (stage, target = None))]
fn shader(
    py: Python<'_>,
    stage: u32,
    target: Option<u32>,
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let value = session
        .shader(stage, target)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    to_py(py, &value)
}

/// `renderdocx.callstack(event, resolve=True)` -> JSON callstack.
#[pyo3::pyfunction]
#[pyo3(signature = (event, resolve = true))]
fn callstack(
    py: Python<'_>,
    event: u32,
    resolve: bool,
) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
    let session = active_session()?;
    let value = session
        .callstack(event, resolve)
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)?;
    to_py(py, &value)
}





const CAPTURE_SETUP: &str = "import sys, io\n_stdout = getattr(sys, '_rdx_orig_out', sys.stdout)\n_stderr = getattr(sys, '_rdx_orig_err', sys.stderr)\nsys._rdx_orig_out = _stdout\nsys._rdx_orig_err = _stderr\n_sysbuf_out = io.StringIO()\n_sysbuf_err = io.StringIO()\nsys.stdout = _sysbuf_out\nsys.stderr = _sysbuf_err\n";
const CAPTURE_RESTORE: &str = "sys.stdout = getattr(sys, '_rdx_orig_out', sys.stdout)\nsys.stderr = getattr(sys, '_rdx_orig_err', sys.stderr)\n";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EmbedOutput {
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
}

/// Read-only summary of the open capture exposed to Python scripts as the
/// `renderdocx` module (mirrors the most-used `pyrenderdoc.CaptureContext`
/// properties without embedding the full API binding layer).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaptureView {
    pub frame: u32,
    pub api: u32,
    pub action_count: usize,
    pub resource_count: usize,
    pub texture_count: usize,
    pub buffer_count: usize,
    pub selected_event: Option<u32>,
    pub resource_names: Vec<String>,
    /// (event_id, name, depth, indices) for action queries.
    pub actions: Vec<(u32, String, usize, u32)>,
    /// (name, width, height, depth, format, mips, samples) per texture.
    pub textures: Vec<(String, u32, u32, u32, String, u32, u32)>,
    /// (name, length) per buffer.
    pub buffers: Vec<(String, String)>,
    /// (event_id, source, description) per debug message.
    pub debug_messages: Vec<(u32, String, String)>,
    /// (name, category) per GPU performance counter.
    pub counters: Vec<(String, String)>,
    /// (stage, shader name) for the currently selected event's pipeline.
    pub shaders: Vec<(String, String)>,
    /// (width, height) per viewport of the currently selected event.
    pub viewports: Vec<(f32, f32)>,
}

impl CaptureView {
    /// Return the action with a given event id, if present.
    pub fn action(&self, event_id: u32) -> Option<&(u32, String, usize, u32)> {
        self.actions.iter().find(|action| action.0 == event_id)
    }
}

fn cstring(code: &str) -> Result<CString, String> {
    CString::new(code).map_err(|_| "Python source contains a NUL byte".to_owned())
}

/// Execute Python source in-process, capturing stdout/stderr.
pub fn run_script(code: &str) -> EmbedOutput {
    let _guard = PY_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    Python::attach(|py| {
        let globals = PyDict::new(py);
        let mut result = EmbedOutput::default();
        if let Err(error) = py.run(&cstring(CAPTURE_SETUP).unwrap(), Some(&globals), None) {
            result.error = Some(format!("capture setup failed: {error}"));
            return result;
        }
        match py.run(&cstring(code).unwrap_or_default(), Some(&globals), None) {
            Ok(()) => {}
            Err(error) => result.error = Some(format_py_error(py, &error)),
        }
        result.stdout = get_buffer(&globals, "_sysbuf_out");
        result.stderr = get_buffer(&globals, "_sysbuf_err");
        let _ = py.run(&cstring(CAPTURE_RESTORE).unwrap(), Some(&globals), None);
        result
    })
}

/// Execute Python with a read-only `renderdocx` capture view installed in
/// Builtins, so scripts can inspect the open capture via
/// `renderdocx.frame`, `renderdocx.api_name`, `renderdocx.action_count`, etc.
pub fn run_script_with_capture(code: &str, view: &CaptureView) -> EmbedOutput {
    let _guard = PY_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    Python::attach(|py| {
        // Reuse the REPL globals so variables persist across console entries.
        let globals = {
            let mut guard = REPL_GLOBALS
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match guard.as_ref() {
                Some(existing) => existing.clone_ref(py).into_bound(py),
                None => {
                    let fresh = PyDict::new(py);
                    *guard = Some(fresh.clone().unbind());
                    fresh
                }
            }
        };
        let mut result = EmbedOutput::default();
        // Build and register the renderdocx module (also usable as pyrenderdoc).
        let module = match build_capture_module(py, view) {
            Ok(module) => module,
            Err(error) => {
                result.error = Some(error);
                return result;
            }
        };
        register_capture_module(py, &module).ok();
        if DEBUGPY_ENABLED.load(Ordering::SeqCst) {
            // The adapter traces per-thread; console scripts run on executor
            // threads, so enable tracing on whichever thread executes them.
            let _ = py.run(
                &cstring("import debugpy\ndebugpy.trace_this_thread(True)\n").unwrap(),
                None,
                None,
            );
        }
        if let Err(error) = py.run(&cstring(CAPTURE_SETUP).unwrap(), Some(&globals), None) {
            result.error = Some(format!("capture setup failed: {error}"));
            return result;
        }
        match py.run(&cstring(code).unwrap_or_default(), Some(&globals), None) {
            Ok(()) => {}
            Err(error) => result.error = Some(format_py_error(py, &error)),
        }
        result.stdout = get_buffer(&globals, "_sysbuf_out");
        result.stderr = get_buffer(&globals, "_sysbuf_err");
        let _ = py.run(&cstring(CAPTURE_RESTORE).unwrap(), Some(&globals), None);
        result
    })
}

/// Clear the persistent REPL globals (start a fresh interpreter session).
pub fn reset_repl() {
    let _guard = PY_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut guard = REPL_GLOBALS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    *guard = None;
}

/// Build the `renderdocx` Python module object from a capture view.
fn build_capture_module<'py>(
    py: Python<'py>,
    view: &CaptureView,
) -> Result<Bound<'py, pyo3::types::PyModule>, String> {
    use pyo3::types::{PyList, PyModule};
    let module = PyModule::new(py, "renderdocx").map_err(|error| error.to_string())?;
    let dict = module.dict();
    dict.set_item("frame", view.frame).ok();
    dict.set_item("api", view.api).ok();
    dict.set_item("api_name", crate::api_enums::graphics_api::name(view.api))
        .ok();
    dict.set_item("action_count", view.action_count).ok();
    dict.set_item("resource_count", view.resource_count).ok();
    dict.set_item("texture_count", view.texture_count).ok();
    dict.set_item("buffer_count", view.buffer_count).ok();
    dict.set_item("selected_event", view.selected_event).ok();
    if let Ok(names) = PyList::new(py, &view.resource_names) {
        dict.set_item("resource_names", names).ok();
    }
    // Actions as a list of (event_id, name, depth, indices) tuples.
    let action_tuples: Vec<(u32, &str, usize, u32)> = view
        .actions
        .iter()
        .map(|(id, name, depth, indices)| (*id, name.as_str(), *depth, *indices))
        .collect();
    if let Ok(actions) = PyList::new(py, action_tuples) {
        dict.set_item("actions", actions).ok();
    }
    // Textures as (name, width, height, depth, format, mips, samples).
    if let Ok(textures) = PyList::new(py, view.textures.clone()) {
        dict.set_item("textures", textures).ok();
    }
    // Buffers as (name, length) and counters as (name, category).
    if let Ok(buffers) = PyList::new(py, view.buffers.clone()) {
        dict.set_item("buffers", buffers).ok();
    }
    if let Ok(counters) = PyList::new(py, view.counters.clone()) {
        dict.set_item("counters", counters).ok();
    }
    // Debug messages as (event_id, source, description).
    if let Ok(messages) = PyList::new(py, view.debug_messages.clone()) {
        dict.set_item("debug_messages", messages).ok();
    }
    // Pipeline shaders as (stage, name) and viewports as (width, height).
    if let Ok(shaders) = PyList::new(py, view.shaders.clone()) {
        dict.set_item("shaders", shaders).ok();
    }
    if let Ok(viewports) = PyList::new(py, view.viewports.clone()) {
        dict.set_item("viewports", viewports).ok();
    }
    // Dynamic query helpers that talk to the open replay session.
    if let Ok(function) = pyo3::wrap_pyfunction!(pipeline, py) {
        dict.set_item("pipeline", function).ok();
    }
    if let Ok(function) = pyo3::wrap_pyfunction!(buffer, py) {
        dict.set_item("buffer", function).ok();
    }
    for (name, wrapped) in [
        ("structure", pyo3::wrap_pyfunction!(structure, py)),
        ("capture_debug_messages", pyo3::wrap_pyfunction!(capture_debug_messages, py)),
        ("shader_messages", pyo3::wrap_pyfunction!(shader_messages, py)),
        ("resource_usage", pyo3::wrap_pyfunction!(resource_usage, py)),
        ("shader", pyo3::wrap_pyfunction!(shader, py)),
        ("callstack", pyo3::wrap_pyfunction!(callstack, py)),
        ("event_format", pyo3::wrap_pyfunction!(event_format, py)),
        ("descriptors", pyo3::wrap_pyfunction!(descriptors, py)),
        ("pixel_history", pyo3::wrap_pyfunction!(pixel_history, py)),
        ("texture_statistics", pyo3::wrap_pyfunction!(texture_statistics, py)),
        ("mesh", pyo3::wrap_pyfunction!(mesh, py)),
        ("vertex_input", pyo3::wrap_pyfunction!(vertex_input, py)),
        ("export_texture", pyo3::wrap_pyfunction!(export_texture, py)),
        ("texture_context", pyo3::wrap_pyfunction!(texture_context, py)),
        ("texture_minmax", pyo3::wrap_pyfunction!(texture_minmax, py)),
        ("texture_fit_range", pyo3::wrap_pyfunction!(texture_fit_range, py)),
        ("render_texture", pyo3::wrap_pyfunction!(render_texture, py)),
        ("mesh_read", pyo3::wrap_pyfunction!(mesh_read, py)),
        ("export_buffer", pyo3::wrap_pyfunction!(export_buffer, py)),
        ("snapshot", pyo3::wrap_pyfunction!(snapshot, py)),
        ("capture_actions", pyo3::wrap_pyfunction!(capture_actions, py)),
        ("CurEvent", pyo3::wrap_pyfunction!(CurEvent, py)),
        ("CurSelectedEvent", pyo3::wrap_pyfunction!(CurSelectedEvent, py)),
        ("CurAction", pyo3::wrap_pyfunction!(CurAction, py)),
        ("CurSelectedAction", pyo3::wrap_pyfunction!(CurSelectedAction, py)),
        ("CurRootActions", pyo3::wrap_pyfunction!(CurRootActions, py)),
        ("CurPipelineState", pyo3::wrap_pyfunction!(CurPipelineState, py)),
        ("GetAction", pyo3::wrap_pyfunction!(GetAction, py)),
        ("GetResources", pyo3::wrap_pyfunction!(GetResources, py)),
        ("GetTextures", pyo3::wrap_pyfunction!(GetTextures, py)),
        ("GetBuffers", pyo3::wrap_pyfunction!(GetBuffers, py)),
        ("FrameInfo", pyo3::wrap_pyfunction!(FrameInfo, py)),
        ("APIProps", pyo3::wrap_pyfunction!(APIProps, py)),
        ("IsCaptureLoaded", pyo3::wrap_pyfunction!(IsCaptureLoaded, py)),
        ("GetCaptureFilename", pyo3::wrap_pyfunction!(GetCaptureFilename, py)),
        ("DebugMessages", pyo3::wrap_pyfunction!(DebugMessages, py)),
        ("GetNotes", pyo3::wrap_pyfunction!(GetNotes, py)),
        ("SetNotes", pyo3::wrap_pyfunction!(SetNotes, py)),
        ("GetBookmarks", pyo3::wrap_pyfunction!(GetBookmarks, py)),
        ("SetBookmark", pyo3::wrap_pyfunction!(SetBookmark, py)),
        ("RemoveBookmark", pyo3::wrap_pyfunction!(RemoveBookmark, py)),
        ("RegisterReplacement", pyo3::wrap_pyfunction!(RegisterReplacement, py)),
        ("UnregisterReplacement", pyo3::wrap_pyfunction!(UnregisterReplacement, py)),
        ("IsResourceReplaced", pyo3::wrap_pyfunction!(IsResourceReplaced, py)),
        ("GetResourceReplacement", pyo3::wrap_pyfunction!(GetResourceReplacement, py)),
        ("debug_vertex", pyo3::wrap_pyfunction!(debug_vertex, py)),
        ("debug_pixel", pyo3::wrap_pyfunction!(debug_pixel, py)),
        ("debug_compute", pyo3::wrap_pyfunction!(debug_compute, py)),
        ("debug_mesh", pyo3::wrap_pyfunction!(debug_mesh, py)),
        ("debug_next", pyo3::wrap_pyfunction!(debug_next, py)),
        ("debug_stop", pyo3::wrap_pyfunction!(debug_stop, py)),
        ("debug_collect", pyo3::wrap_pyfunction!(debug_collect, py)),
        ("debug_timeline", pyo3::wrap_pyfunction!(debug_timeline, py)),
        ("debug_set_breakpoint", pyo3::wrap_pyfunction!(debug_set_breakpoint, py)),
        ("Replay", pyo3::wrap_pyfunction!(Replay, py)),
        ("ShowEventBrowser", pyo3::wrap_pyfunction!(ShowEventBrowser, py)),
        ("HasEventBrowser", pyo3::wrap_pyfunction!(HasEventBrowser, py)),
        ("ShowAPIInspector", pyo3::wrap_pyfunction!(ShowAPIInspector, py)),
        ("HasAPIInspector", pyo3::wrap_pyfunction!(HasAPIInspector, py)),
        ("ShowTextureViewer", pyo3::wrap_pyfunction!(ShowTextureViewer, py)),
        ("HasTextureViewer", pyo3::wrap_pyfunction!(HasTextureViewer, py)),
        ("ShowPipelineViewer", pyo3::wrap_pyfunction!(ShowPipelineViewer, py)),
        ("HasPipelineViewer", pyo3::wrap_pyfunction!(HasPipelineViewer, py)),
        ("ShowMeshPreview", pyo3::wrap_pyfunction!(ShowMeshPreview, py)),
        ("HasMeshPreview", pyo3::wrap_pyfunction!(HasMeshPreview, py)),
        ("ShowResourceInspector", pyo3::wrap_pyfunction!(ShowResourceInspector, py)),
        ("HasResourceInspector", pyo3::wrap_pyfunction!(HasResourceInspector, py)),
        ("ShowDebugMessageView", pyo3::wrap_pyfunction!(ShowDebugMessageView, py)),
        ("HasDebugMessageView", pyo3::wrap_pyfunction!(HasDebugMessageView, py)),
        ("ShowAnnotationViewer", pyo3::wrap_pyfunction!(ShowAnnotationViewer, py)),
        ("HasAnnotationViewer", pyo3::wrap_pyfunction!(HasAnnotationViewer, py)),
        ("ShowPerformanceCounterViewer", pyo3::wrap_pyfunction!(ShowPerformanceCounterViewer, py)),
        ("HasPerformanceCounterViewer", pyo3::wrap_pyfunction!(HasPerformanceCounterViewer, py)),
        ("ShowStatisticsViewer", pyo3::wrap_pyfunction!(ShowStatisticsViewer, py)),
        ("HasStatisticsViewer", pyo3::wrap_pyfunction!(HasStatisticsViewer, py)),
        ("ShowTimelineBar", pyo3::wrap_pyfunction!(ShowTimelineBar, py)),
        ("HasTimelineBar", pyo3::wrap_pyfunction!(HasTimelineBar, py)),
        ("ShowDiagnosticLogView", pyo3::wrap_pyfunction!(ShowDiagnosticLogView, py)),
        ("HasDiagnosticLogView", pyo3::wrap_pyfunction!(HasDiagnosticLogView, py)),
        ("ShowPythonShell", pyo3::wrap_pyfunction!(ShowPythonShell, py)),
        ("HasPythonShell", pyo3::wrap_pyfunction!(HasPythonShell, py)),
        ("GetEventBrowser", pyo3::wrap_pyfunction!(GetEventBrowser, py)),
        ("GetAPIInspector", pyo3::wrap_pyfunction!(GetAPIInspector, py)),
        ("GetTextureViewer", pyo3::wrap_pyfunction!(GetTextureViewer, py)),
        ("GetPipelineViewer", pyo3::wrap_pyfunction!(GetPipelineViewer, py)),
        ("GetMeshPreview", pyo3::wrap_pyfunction!(GetMeshPreview, py)),
        ("GetResourceInspector", pyo3::wrap_pyfunction!(GetResourceInspector, py)),
        ("GetDebugMessageView", pyo3::wrap_pyfunction!(GetDebugMessageView, py)),
        ("GetAnnotationViewer", pyo3::wrap_pyfunction!(GetAnnotationViewer, py)),
        ("GetPerformanceCounterViewer", pyo3::wrap_pyfunction!(GetPerformanceCounterViewer, py)),
        ("GetStatisticsViewer", pyo3::wrap_pyfunction!(GetStatisticsViewer, py)),
        ("GetTimelineBar", pyo3::wrap_pyfunction!(GetTimelineBar, py)),
        ("GetDiagnosticLogView", pyo3::wrap_pyfunction!(GetDiagnosticLogView, py)),
        ("GetPythonShell", pyo3::wrap_pyfunction!(GetPythonShell, py)),
        ("ViewShader", pyo3::wrap_pyfunction!(ViewShader, py)),
        ("EditShader", pyo3::wrap_pyfunction!(EditShader, py)),
        ("DebugShader", pyo3::wrap_pyfunction!(DebugShader, py)),
        ("apply_shader_edit", pyo3::wrap_pyfunction!(apply_shader_edit, py)),
        ("revert_shader_edit", pyo3::wrap_pyfunction!(revert_shader_edit, py)),
        ("PipelineState", pyo3::wrap_pyfunction!(PipelineState, py)),
        ("Texture", pyo3::wrap_pyfunction!(Texture, py)),
        ("Action", pyo3::wrap_pyfunction!(Action, py)),
        ("Resource", pyo3::wrap_pyfunction!(Resource, py)),
        ("ShaderReflection", pyo3::wrap_pyfunction!(ShaderReflection, py)),
        ("ShaderVariable", pyo3::wrap_pyfunction!(ShaderVariable, py)),
        ("ShaderSignature", pyo3::wrap_pyfunction!(ShaderSignature, py)),
        ("ConstantBlock", pyo3::wrap_pyfunction!(ConstantBlock, py)),
        ("ShaderConstant", pyo3::wrap_pyfunction!(ShaderConstant, py)),
        ("ReflectedType", pyo3::wrap_pyfunction!(ReflectedType, py)),
        ("ReflectedResource", pyo3::wrap_pyfunction!(ReflectedResource, py)),
        ("RootParameter", pyo3::wrap_pyfunction!(RootParameter, py)),
        ("StaticSampler", pyo3::wrap_pyfunction!(StaticSampler, py)),
        ("SampleLocations", pyo3::wrap_pyfunction!(SampleLocations, py)),
        ("RenderPass", pyo3::wrap_pyfunction!(RenderPass, py)),
        ("Framebuffer", pyo3::wrap_pyfunction!(Framebuffer, py)),
        ("SamplerDescriptor", pyo3::wrap_pyfunction!(SamplerDescriptor, py)),
        ("FrameDescription", pyo3::wrap_pyfunction!(FrameDescription, py)),
        ("Descriptor", pyo3::wrap_pyfunction!(Descriptor, py)),
        ("DescriptorStore", pyo3::wrap_pyfunction!(DescriptorStore, py)),
        ("APIProperties", pyo3::wrap_pyfunction!(APIProperties, py)),
        ("ResourceId", pyo3::wrap_pyfunction!(ResourceId, py)),
        ("ViewBuffer", pyo3::wrap_pyfunction!(ViewBuffer, py)),
        ("ViewConstantBuffer", pyo3::wrap_pyfunction!(ViewConstantBuffer, py)),
        ("ViewPixelHistory", pyo3::wrap_pyfunction!(ViewPixelHistory, py)),
        ("ViewShaderMessages", pyo3::wrap_pyfunction!(ViewShaderMessages, py)),
        ("pipeline_shader", pyo3::wrap_pyfunction!(pipeline_shader, py)),
        ("pipeline_output", pyo3::wrap_pyfunction!(pipeline_output, py)),
        ("pipeline_color_attachment", pyo3::wrap_pyfunction!(pipeline_color_attachment, py)),
        ("action_info", pyo3::wrap_pyfunction!(action_info, py)),
        ("action_at", pyo3::wrap_pyfunction!(action_at, py)),
        ("action_children", pyo3::wrap_pyfunction!(action_children, py)),
        ("action_draws", pyo3::wrap_pyfunction!(action_draws, py)),
        ("texture_info", pyo3::wrap_pyfunction!(texture_info, py)),
        ("buffer_info", pyo3::wrap_pyfunction!(buffer_info, py)),
        ("resource_info", pyo3::wrap_pyfunction!(resource_info, py)),
    ] {
        if let Ok(function) = wrapped {
            dict.set_item(name, function).ok();
        }
    }
    // Built-in help() listing the exposed API: the minimal pure-Rust
    // equivalent of the upstream help browser's discovery role.
    let help_src = "def _rdx_help():\n    print('RenderDocX Python API: ' + ', '.join(sorted(k for k in dir(renderdocx) if not k.startswith('_'))))\n";
    let help_globals = PyDict::new(py);
    help_globals.set_item("renderdocx", &module).ok();
    if py
        .run(&cstring(help_src).unwrap(), Some(&help_globals), None)
        .is_ok()
    {
        if let Ok(function) = help_globals.get_item("_rdx_help") {
            dict.set_item("help", function).ok();
        }
    }
    Ok(module)
}

/// Register the capture module under both `renderdocx` and `pyrenderdoc` in
/// `sys.modules`, so `import renderdocx` / `import pyrenderdoc` both work.
fn register_capture_module(
    py: Python<'_>,
    module: &Bound<'_, pyo3::types::PyModule>,
) -> Result<(), String> {
    let modules = py
        .import("sys")
        .and_then(|sys| sys.getattr("modules"))
        .map_err(|error| error.to_string())?;
    modules
        .set_item("renderdocx", module)
        .map_err(|error| error.to_string())?;
    modules
        .set_item("pyrenderdoc", module)
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn get_buffer(globals: &Bound<'_, PyDict>, name: &str) -> String {
    globals
        .get_item(name)
        .ok()
        .flatten()
        .and_then(|obj| obj.call_method0("getvalue").ok())
        .and_then(|value| value.extract::<String>().ok())
        .unwrap_or_default()
}

fn format_py_error(py: Python<'_>, error: &PyErr) -> String {
    error
        .value(py)
        .getattr("__str__")
        .ok()
        .and_then(|f| f.call0().ok())
        .and_then(|s| s.extract::<String>().ok())
        .unwrap_or_else(|| error.to_string())
}

/// Load an extension package's `__init__.py` and call one of its lifecycle
/// entry points (`register(version, pyrenderdoc)` / `unregister()`), capturing
/// output and exceptions. The extension directory is prepended to `sys.path`.
pub fn call_extension(
    dir: &std::path::Path,
    function: &str,
    version: &str,
    view: Option<&CaptureView>,
) -> EmbedOutput {
    let _guard = PY_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    Python::attach(|py| {
        let globals = PyDict::new(py);
        let mut result = EmbedOutput::default();
        // Expose the open capture as pyrenderdoc/renderdocx for the extension.
        if let Some(view) = view {
            match build_capture_module(py, view) {
                Ok(module) => {
                    register_capture_module(py, &module).ok();
                    globals.set_item("pyrenderdoc", &module).ok();
                }
                Err(error) => {
                    result.error = Some(error);
                    return result;
                }
            }
        }
        // Build a Python string literal for the directory via repr, avoiding
        // manual escaping of Windows separators.
        let dir_repr = py
            .eval(
                &cstring("repr(_dir)").unwrap(),
                Some(&{
                    let d = PyDict::new(py);
                    d.set_item("_dir", dir.to_string_lossy().as_ref()).ok();
                    d
                }),
                None,
            )
            .ok()
            .and_then(|obj| obj.extract::<String>().ok())
            .unwrap_or_else(|| format!("{:?}", dir.to_string_lossy()));
        let setup = format!("{CAPTURE_SETUP}import sys\nsys.path.insert(0, {dir_repr})\n");
        if let Err(error) = py.run(&cstring(&setup).unwrap_or_default(), Some(&globals), None) {
            result.error = Some(format!("setup failed: {error}"));
            return result;
        }
        let load = format!(
            "import importlib.util as _u\n_spec=_u.spec_from_file_location('_rdx_ext', {dir_repr}+'/__init__.py')\n_mod=_u.module_from_spec(_spec)\n_spec.loader.exec_module(_mod)\n"
        );
        if let Err(error) = py.run(&cstring(&load).unwrap_or_default(), Some(&globals), None) {
            result.error = Some(format_py_error(py, &error));
            return result;
        }
        let call = if function == "register" {
            // Pass the capture module as pyrenderdoc when one is available.
            if view.is_some() {
                format!("_mod.register({version:?}, pyrenderdoc)")
            } else {
                format!("_mod.register({version:?}, None)")
            }
        } else {
            "_mod.unregister()".to_owned()
        };
        if let Err(error) = py.run(&cstring(&call).unwrap_or_default(), Some(&globals), None) {
            result.error = Some(format_py_error(py, &error));
        }
        result.stdout = get_buffer(&globals, "_sysbuf_out");
        result.stderr = get_buffer(&globals, "_sysbuf_err");
        let _ = py.run(&cstring(CAPTURE_RESTORE).unwrap(), Some(&globals), None);
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executes_python_in_process_with_captured_output() {
        let result = run_script("print('hello from embedded python')");
        assert!(result.error.is_none(), "{:?}", result.error);
        assert!(result.stdout.contains("hello from embedded python"));
    }

    #[test]
    fn captures_python_exceptions() {
        let result = run_script("raise ValueError('boom')");
        assert!(result.error.is_some());
        assert!(result.error.unwrap().contains("boom"));
    }

    #[test]
    fn calls_extension_register_and_unregister_in_process() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("__init__.py"),
            "def register(version, pyrenderdoc):\n    print('registered', version)\ndef unregister():\n    print('unregistered')\n",
        )
        .unwrap();
        let registered = call_extension(dir.path(), "register", "1.48", None);
        assert!(registered.error.is_none(), "{:?}", registered.error);
        assert!(registered.stdout.contains("registered 1.48"));
        let unregistered = call_extension(dir.path(), "unregister", "1.48", None);
        assert!(unregistered.error.is_none(), "{:?}", unregistered.error);
        assert!(unregistered.stdout.contains("unregistered"));
    }

    #[test]
    fn scripts_can_access_capture_view() {
        let view = CaptureView {
            frame: 7,
            api: crate::api_enums::graphics_api::VULKAN,
            action_count: 42,
            resource_count: 3,
            texture_count: 2,
            buffer_count: 1,
            selected_event: Some(11),
            resource_names: vec!["texA".into(), "bufB".into()],
            shaders: vec![("Vertex".into(), "vertex shader".into())],
            viewports: vec![(1280.0, 720.0)],
            actions: vec![
                (10, "Clear".into(), 0, 0),
                (11, "DrawIndexed".into(), 1, 300),
            ],
            textures: vec![("MainColor".into(), 1920, 1080, 1, "R8G8B8A8".into(), 1, 1)],
            buffers: vec![("VertexBuffer".into(), "4096".into())],
            debug_messages: vec![(11, "API".into(), "sample warning".into())],
            counters: vec![("Draw Calls".into(), "Drawcall".into())],
        };
        assert_eq!(
            view.action(11).map(|action| action.1.as_str()),
            Some("DrawIndexed")
        );
        let result = run_script_with_capture(
            "import renderdocx\nprint(renderdocx.frame, renderdocx.api_name, renderdocx.action_count, renderdocx.resource_names[0])\nprint([a for a in renderdocx.actions if a[0] == 11])\nprint(renderdocx.textures[0][0], renderdocx.textures[0][1], renderdocx.textures[0][2])\nprint(renderdocx.buffers[0][0], renderdocx.buffers[0][1])\nprint(renderdocx.debug_messages[0][0], renderdocx.debug_messages[0][2])\nprint(renderdocx.counters[0][0], renderdocx.counters[0][1])\nprint(renderdocx.shaders[0][0], renderdocx.shaders[0][1])\nprint(renderdocx.viewports[0][0])",
            &view,
        );
        assert!(result.error.is_none(), "{:?}", result.error);
        assert!(result.stdout.contains("7 Vulkan 42 texA"));
        assert!(result.stdout.contains("DrawIndexed"));
        assert!(result.stdout.contains("MainColor 1920 1080"));
        assert!(result.stdout.contains("VertexBuffer 4096"));
        assert!(result.stdout.contains("11 sample warning"));
        assert!(result.stdout.contains("Draw Calls Drawcall"));
        assert!(result.stdout.contains("Vertex vertex shader"));
        assert!(result.stdout.contains("1280"));

        let help = run_script_with_capture("import renderdocx\nrenderdocx.help()", &view);
        assert!(help.error.is_none(), "{:?}", help.error);
        assert!(help.stdout.contains("RenderDocX Python API"));
        assert!(help.stdout.contains("action_count"));
    }

    #[test]
    fn extensions_receive_the_capture_view_as_pyrenderdoc() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("__init__.py"),
            "def register(version, pyrenderdoc):\n    print('frame', pyrenderdoc.frame, 'name', pyrenderdoc.api_name, 'actions', pyrenderdoc.action_count)\ndef unregister():\n    pass\n",
        )
        .unwrap();
        let view = CaptureView {
            frame: 9,
            api: crate::api_enums::graphics_api::D3D11,
            action_count: 12,
            ..Default::default()
        };
        let result = call_extension(dir.path(), "register", "1.48", Some(&view));
        assert!(result.error.is_none(), "{:?}", result.error);
        assert!(result.stdout.contains("frame 9 name D3D11 actions 12"));
    }

    #[test]
    fn repl_state_persists_across_entries_and_resets() {
        reset_repl();
        let view = CaptureView {
            frame: 1,
            ..Default::default()
        };
        let first = run_script_with_capture("_rdx_repl_probe = 41\nprint(_rdx_repl_probe)", &view);
        assert!(first.stdout.contains("41"), "{:?}", first.error);
        let second = run_script_with_capture("print(_rdx_repl_probe + 1)", &view);
        assert!(second.stdout.contains("42"), "{:?}", second.error);
        reset_repl();
        let third = run_script_with_capture("print(_rdx_repl_probe)", &view);
        assert!(third.error.is_some(), "reset must drop prior globals");
    }

    #[test]
    fn dynamic_helpers_require_an_open_session() {
        set_active_session(None);
        let result = run_script_with_capture(
            "import renderdocx\nrenderdocx.pipeline(1)",
            &CaptureView::default(),
        );
        assert!(result.error.is_some(), "pipeline must fail without a session");
        for call in [
            "renderdocx.buffer(0)",
            "renderdocx.structure(1)",
            "renderdocx.capture_debug_messages()",
            "renderdocx.shader_messages(1)",
            "renderdocx.resource_usage(0)",
            "renderdocx.shader(0)",
            "renderdocx.export_texture('out.png')",
            "renderdocx.texture_context('ctx.png')",
            "renderdocx.texture_minmax(1)",
            "renderdocx.texture_fit_range(1)",
            "renderdocx.render_texture('render.png')",
            "renderdocx.mesh_read(1)",
            "renderdocx.export_buffer('buf.bin')",
            "renderdocx.snapshot()",
            "renderdocx.GetNotes()",
            "renderdocx.SetNotes('x')",
            "renderdocx.GetBookmarks()",
            "renderdocx.SetBookmark(1, 'x')",
            "renderdocx.RemoveBookmark(1)",
            "renderdocx.debug_vertex(1, 0)",
            "renderdocx.debug_pixel(1, 0, 0)",
            "renderdocx.debug_compute(1)",
            "renderdocx.debug_mesh(1)",
            "renderdocx.debug_next()",
            "renderdocx.debug_stop()",
            "renderdocx.Replay().GetPipelineState()",
            "renderdocx.Replay().SetFrameEvent(1)",
            "renderdocx.PipelineState()",
            "renderdocx.pipeline_shader(1, 0)",
            "renderdocx.pipeline_output(1)",
            "renderdocx.pipeline_color_attachment(1)",
            "renderdocx.DebugShader(7, 1)",
            "renderdocx.apply_shader_edit(1, 4, 2, 'main', 'x')",
            "renderdocx.revert_shader_edit(1, 4)",
            "renderdocx.ShaderReflection(4)",
            "renderdocx.ViewConstantBuffer(4, 0)",
            "renderdocx.ViewPixelHistory(0, 0, 0)",
            "renderdocx.ViewShaderMessages()",
        ] {
            let result = run_script_with_capture(
                &format!("import renderdocx\n{call}"),
                &CaptureView::default(),
            );
            assert!(result.error.is_some(), "{call} must fail without a session");
        }
        // capture_actions() returns an empty list (not an error) when unbound.
        let result = run_script_with_capture(
            "import renderdocx\nprint(len(renderdocx.capture_actions()))",
            &CaptureView::default(),
        );
        assert!(result.error.is_none(), "{:?}", result.error);
        assert_eq!(result.stdout.trim(), "0");
        let result = run_script_with_capture(
            "import renderdocx\nprint(repr(renderdocx.Texture(0)), repr(renderdocx.Action(1)), repr(renderdocx.Resource(0)), repr(renderdocx.FrameDescription()), repr(renderdocx.APIProperties()), repr(renderdocx.DescriptorStore(0)))",
            &CaptureView::default(),
        );
        assert!(result.error.is_none(), "{:?}", result.error);
        assert_eq!(
            result.stdout.trim(),
            "None None None None None None"
        );
        // pyrenderdoc-compatible accessors report an unloaded capture cleanly.
        let result = run_script_with_capture(
            "import renderdocx\n\
             print(renderdocx.CurEvent(), renderdocx.IsCaptureLoaded(),\n\
                   len(renderdocx.GetTextures()), len(renderdocx.GetResources()),\n\
                   repr(renderdocx.CurAction()), repr(renderdocx.FrameInfo()),\n\
                   repr(renderdocx.APIProps()), renderdocx.GetCaptureFilename())",
            &CaptureView::default(),
        );
        assert!(result.error.is_none(), "{:?}", result.error);
        assert_eq!(result.stdout.trim(), "0 False 0 0 None None None");
        // Resource replacement map mirrors pyrenderdoc's RegisterReplacement.
        let result = run_script_with_capture(
            "import renderdocx\n\
             renderdocx.RegisterReplacement(1, 2)\n\
             print(renderdocx.IsResourceReplaced(1), renderdocx.GetResourceReplacement(1))\n\
             renderdocx.UnregisterReplacement(1)\n\
             print(renderdocx.IsResourceReplaced(1), renderdocx.GetResourceReplacement(1))",
            &CaptureView::default(),
        );
        assert!(result.error.is_none(), "{:?}", result.error);
        assert_eq!(result.stdout.trim(), "True 2\nFalse None");
        RegisterReplacement(1, 2).unwrap();
        let mut pipeline =
            serde_json::json!({"outputs":[{"resource_index":1},{"resource_index":3}]});
        apply_replacements(&mut pipeline);
        assert_eq!(pipeline["outputs"][0]["resource_index"], 2);
        assert_eq!(pipeline["outputs"][1]["resource_index"], 3);
        UnregisterReplacement(1).unwrap();
        assert!(!IsResourceReplaced(1));
        assert_eq!(GetResourceReplacement(1), None);
        // Show* calls queue panel switches for the GUI to drain.
        take_ui_requests();
        let result = run_script_with_capture(
            "import renderdocx\n\
             renderdocx.ShowTextureViewer()\n\
             renderdocx.ShowMeshPreview()\n\
             print(renderdocx.HasTextureViewer(), renderdocx.HasMeshPreview(), renderdocx.HasPythonShell())",
            &CaptureView::default(),
        );
        assert!(result.error.is_none(), "{:?}", result.error);
        assert_eq!(result.stdout.trim(), "True True True");
        assert_eq!(take_ui_requests(), vec!["textures", "mesh"]);
        // Viewer handles and shader view requests.
        let result = run_script_with_capture(
            "import renderdocx\n\
             h = renderdocx.GetTextureViewer()\n\
             print(h.Name(), h.IsAvailable())\n\
             h.Show()\n\
             print(renderdocx.ViewShader(4).Name())\n\
             print(renderdocx.EditShader(4).Name())",
            &CaptureView::default(),
        );
        assert!(result.error.is_none(), "{:?}", result.error);
        assert_eq!(
            result.stdout.trim(),
            "textures True\nshader:4\nedit_shader:4"
        );
        assert_eq!(
            take_ui_requests(),
            vec!["textures", "shader:4", "edit_shader:4"]
        );
        let result = run_script_with_capture(
            "import renderdocx\n\
             cb = renderdocx.ConstantBlock({'name':'Globals','bind':0,'space':0,'array_size':1,'byte_size':16,'buffer_backed':True,'inline_data':False,'compile_constants':False,'variables':[]})\n\
             print(cb.name, cb.byteSize, cb.bufferBacked, isinstance(cb.GetVariables(), list), 'byte_size' in cb.keys())",
            &CaptureView::default(),
        );
        assert!(result.error.is_none(), "{:?}", result.error);
        assert_eq!(result.stdout.trim(), "Globals 16 True True True");
        let result = run_script_with_capture(
            "import renderdocx\n\
             d = renderdocx.Descriptor({'type':0,'resource':7,'byte_offset':16})\n\
             print(d.type, d.resourceId, d.byteOffset, 'resource' in d.keys())",
            &CaptureView::default(),
        );
        assert!(result.error.is_none(), "{:?}", result.error);
        assert_eq!(result.stdout.trim(), "0 7 16 True");
        let result = run_script_with_capture(
            "import renderdocx\n\
             sd = renderdocx.SamplerDescriptor({'name':'samp','bind':0,'space':0,'array_size':1})\n\
             print(sd.name, sd.arraySize, sd.GetName(), 'array_size' in sd.keys())",
            &CaptureView::default(),
        );
        assert!(result.error.is_none(), "{:?}", result.error);
        assert_eq!(result.stdout.trim(), "samp 1 samp True");
        let shell = GetPythonShell();
        assert_eq!(shell.GetDebuggerPort(), 5678);
        shell.SetScriptText("print(7)".to_owned());
        assert_eq!(shell.GetScriptText(), "print(7)");
        shell.SetScriptOutputFilter("alpha".to_owned());
        assert_eq!(shell.GetOutputFilter().as_deref(), Some("alpha"));
        shell.SetExtensionOutputFilter("beta".to_owned());
        assert_eq!(shell.GetExtensionOutputFilter().as_deref(), Some("beta"));
        shell.RemoveOutputFilter();
        assert_eq!(shell.GetOutputFilter(), None);
        let script_dir = tempfile::tempdir().unwrap();
        let script = script_dir.path().join("extension.py");
        std::fs::write(&script, "print(9)\n").unwrap();
        assert!(
            shell.LoadScriptFromFilename(script.display().to_string()),
            "load extension.py"
        );
        assert_eq!(shell.GetScriptText(), "print(9)\n");
        assert!(!shell.LoadScriptFromFilename(
            script_dir.path().join("missing.py").display().to_string()
        ));
        shell.CreateNewScriptEditor("new.py".to_owned(), "print(11)".to_owned());
        assert_eq!(shell.GetScriptText(), "print(11)");
        let result = run_script_with_capture(
            "import renderdocx\n\
             shell = renderdocx.GetPythonShell()\n\
             shell.SetScriptText('print(31415)')\n\
             shell.RunScript(shell.GetScriptText())\n\
             shell.SetScriptText('print(27182)')\n\
             shell.RunScript()",
            &CaptureView::default(),
        );
        assert!(result.error.is_none(), "{:?}", result.error);
        assert!(result.stdout.contains("31415"), "{}", result.stdout);
        assert!(result.stdout.contains("27182"), "{}", result.stdout);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires RENDERDOCX_TEST_BRIDGE and RENDERDOCX_TEST_VULKAN_MESH_CAPTURE"]
    fn python_pipeline_and_buffer_query_a_real_capture() {
        use crate::backend::Settings;
        use std::path::PathBuf;

        let bridge = PathBuf::from(
            std::env::var("RENDERDOCX_TEST_BRIDGE").expect("set RENDERDOCX_TEST_BRIDGE"),
        );
        let capture = PathBuf::from(
            std::env::var("RENDERDOCX_TEST_VULKAN_MESH_CAPTURE")
                .expect("set RENDERDOCX_TEST_VULKAN_MESH_CAPTURE"),
        );
        let mut settings = Settings::default();
        settings.replay_bridge = Some(bridge);
        let (session, snapshot) =
            crate::replay::Session::open(&settings, &capture).expect("open capture");
        set_active_session(Some(session));
        set_active_capture(Some(capture.display().to_string()));
        set_active_format_context(
            snapshot.resources.clone(),
            snapshot.buffers.clone(),
            snapshot.textures.clone(),
            crate::i18n::Language::English,
        );
        set_active_actions(snapshot.actions.clone());
        set_active_snapshot(serde_json::to_value(&snapshot).expect("serialize snapshot"));
        take_ui_requests();
        let result = run_script_with_capture(
            "import renderdocx\n\
             p = renderdocx.pipeline(19)\n\
             print(p.get('event_id'))\n\
             print(len(renderdocx.buffer(0, 0, 16)))\n\
             print(len(renderdocx.structure(19)))\n\
             print(len(renderdocx.capture_debug_messages()))\n\
             print(len(renderdocx.shader_messages(19)))\n\
             print(len(renderdocx.resource_usage(0)))\n\
             s = p.get('shaders') or []\n\
             print('shaders', len(s), s[0].get('stage') if s else 'none')\n\
             print('stack', isinstance(renderdocx.callstack(19), dict))\n\
             print('format', len(renderdocx.event_format(19)) > 0)\n\
             print('descs', isinstance(renderdocx.descriptors(19), dict))\n\
             print('pixels', isinstance(renderdocx.pixel_history(19, 0, 1, 1), dict))\n\
             print('stats', isinstance(renderdocx.texture_statistics(19, 0), dict))\n\
             print('mesh', isinstance(renderdocx.mesh(19, 4), dict))\n\
             import tempfile, os\n\
             d = tempfile.mkdtemp()\n\
             png = os.path.join(d, 'tex.png')\n\
             print('export', renderdocx.export_texture(png, 0) == png and os.path.getsize(png) > 0)\n\
             ctx = os.path.join(d, 'ctx.png')\n\
             print('context', renderdocx.texture_context(ctx, 0, 64, 64) == ctx and os.path.getsize(ctx) > 0)\n\
             print('minmax', isinstance(renderdocx.texture_minmax(19, 0), dict))\n\
             print('fit', isinstance(renderdocx.texture_fit_range(19, 0), list))\n\
             snap = renderdocx.snapshot()\n\
             print('snapshot', isinstance(snap.get('actions'), list) and bool(snap.get('api')))\n\
             print('actions', isinstance(renderdocx.capture_actions(), list))\n\
             print('meshread', isinstance(renderdocx.mesh_read(21, 4, 'vertex', 0, 16), str))\n\
             buf = os.path.join(d, 'buf.bin')\n\
             print('bufexport', renderdocx.export_buffer(buf, 0) == buf and os.path.getsize(buf) > 0)\n\
             rp = os.path.join(d, 'render.png')\n\
             print('render', renderdocx.render_texture(rp, 0, 32, 32) == rp and os.path.getsize(rp) > 0)\n\
             renderdocx.pipeline(19)\n\
             compat = (\n\
                 renderdocx.CurEvent() == 19\n\
                 and renderdocx.CurSelectedEvent() == 19\n\
                 and renderdocx.CurPipelineState().get('event_id') == 19\n\
                 and isinstance(renderdocx.CurAction(), dict)\n\
                 and isinstance(renderdocx.CurSelectedAction(), dict)\n\
                 and isinstance(renderdocx.CurRootActions(), list)\n\
                 and isinstance(renderdocx.GetAction(19), dict)\n\
                 and isinstance(renderdocx.GetTextures(), list)\n\
                 and isinstance(renderdocx.GetBuffers(), list)\n\
                 and isinstance(renderdocx.GetResources(), list)\n\
                 and isinstance(renderdocx.APIProps(), dict)\n\
                 and renderdocx.IsCaptureLoaded()\n\
                 and renderdocx.GetCaptureFilename().endswith('.rdc')\n\
                 and isinstance(renderdocx.DebugMessages(), list)\n\
             )\n\
             print('compat', compat)\n\
             controller = renderdocx.Replay()\n\
             controller.SetFrameEvent(19)\n\
             print('replay', controller.GetPipelineState().get('event_id') == 19 and len(controller.GetTextures()) == 2 and len(controller.GetBuffers()) == 1 and len(controller.GetResources()) > 0 and controller.GetFrameInfo() is not None and isinstance(controller.GetAPIProperties(), dict))\n\
             ps = renderdocx.PipelineState(19)\n\
             print('pipeline-object', ps.event_id == 19 and ps.available and ps.GetEventId() == 19 and ps.GetShader(4).get('bound') and len(ps.GetOutputs()) == 2 and ps['event_id'] == 19 and ps.get('event_id') == 19 and ps.GetColorAttachments()[0].get('resource', {}).get('resource_index') == 7 and 'event_id' in ps.keys())\n\
             apis = ps.GetAPISpecific()\n\
             vk = ps.GetVulkanState()\n\
             print('api-specific', apis.get('kind') == 'Vulkan' and ps.GetAPIKind() == 'Vulkan' and isinstance(vk, dict) and isinstance(vk.get('descriptor_sets'), list) and vk.get('render_pass', {}).get('resource_index') == 9 and vk.get('framebuffer_width') == 128 and vk.get('raster_samples') == 1 and ps.GetD3D11State() is None and renderdocx.pipeline(19).get('api_specific', {}).get('kind') == 'Vulkan')\n\
             sl = renderdocx.SampleLocations(vk['sampleLocations'])\n\
             rpobj = renderdocx.RenderPass(vk['renderPassDetails'])\n\
             fb = renderdocx.Framebuffer(vk['framebufferDetails'])\n\
             print('vulkan-deep', sl.gridWidth == 1 and sl.gridHeight == 1 and isinstance(sl.customLocations, list) and rpobj.resource.get('resource_index') == 9 and rpobj.subpass == 0 and fb.width == 128 and fb.height == 128 and fb.layers == 1 and fb.attachmentCount == 1 and ps.GetVulkanRenderPass().get('subpass') == 0 and ps.GetVulkanFramebuffer().get('width') == 128 and ps.GetVulkanSampleLocations().get('gridWidth') == 1)\n\
             tex = renderdocx.Texture(0)\n\
             print('texture-object', tex.name == 'Swapchain Image 96' and tex.width == 128 and tex.arraySize == 1 and tex.format == 'B8G8R8A8_SRGB' and tex.get('mips') == 1 and tex['height'] == 128 and 'height' in tex.keys())\n\
             act = renderdocx.Action(21)\n\
             print('action-object', act.eventId == 21 and act.actionId == 4 and act.numIndices == 0 and act['name'] == 'vkCmdDrawMeshTasksEXT()' and isinstance(act.GetChildren(), list) and act.get('flags') == 8)\n\
             renderdocx.RegisterReplacement(7, 8)\n\
             replaced = renderdocx.pipeline(19).get('outputs') or [{}]\n\
             print('replace', replaced[0].get('resource_index') == 8)\n\
             renderdocx.UnregisterReplacement(7)\n\
             restored = renderdocx.pipeline(19).get('outputs') or [{}]\n\
             print('unreplace', restored[0].get('resource_index') == 7)\n\
             trace = renderdocx.debug_mesh(21)\n\
             print('debug', trace.get('event_id') == 21 and trace.get('stage') == 7)\n\
             print('debugnext', isinstance(renderdocx.debug_next(), dict))\n\
             renderdocx.debug_stop()\n\
             print('pipeline-fields', renderdocx.pipeline_shader(19, 4).get('bound') and renderdocx.pipeline_shader(19, 7).get('bound') and renderdocx.pipeline_output(19).get('resource_index') == 7 and renderdocx.pipeline_color_attachment(19).get('resource', {}).get('resource_index') == 7)\n\
             print('action-helpers', renderdocx.action_info(19).get('event_id') == 21 and renderdocx.action_at(0).get('event_id') is not None and len(renderdocx.action_draws()) >= 3 and isinstance(renderdocx.action_children(0), list))\n\
             print('resource-info', renderdocx.texture_info(0).get('name') == 'Swapchain Image 96' and renderdocx.buffer_info(0).get('length') == '108' and renderdocx.resource_info(7).get('name') == 'Swapchain Image 96')\n\
             print('viewers', renderdocx.GetTextureViewer().Name() == 'textures' and renderdocx.GetPipelineViewer().IsAvailable())\n\
             sh = renderdocx.ViewShader(4)\n\
             ed = renderdocx.EditShader(4)\n\
             print('viewshader', sh.Name() == 'shader:4' and ed.Name() == 'edit_shader:4')\n\
             sh.SetCurrentStep(3)\n\
             sh.AddWatch('inColor')\n\
             print('shader-viewer', sh.CurrentStep() == 3 and sh.GetWatches() == ['inColor'] and isinstance(sh.GetCurrentFileContents(), str) and sh.GetCurrentFileContents() != '' and sh.ShowErrors() == '')\n\
             res = renderdocx.Resource(7)\n\
             print('resource-object', res.name == 'Swapchain Image 96' and res.type == 8 and res.resourceId == 'ResourceId::96' and res.autogeneratedName is True and 'name' in res.keys())\n\
             rid = res.GetResourceId()\n\
             print('resource-id', str(rid) == 'ResourceId::96' and rid.id == 'ResourceId::96')\n\
             sr = renderdocx.ShaderReflection(4, event=19)\n\
             print('shader-reflection', sr.event_id == 19 and sr.stage == 4 and isinstance(sr.GetEntryPoint(), str) and isinstance(sr.GetDisassembly(), str) and isinstance(sr.GetSourceFiles(), list) and isinstance(sr.GetCompileFlags(), list) and ps.GetShaderReflection(4).stage == 4)\n\
             sig = sr.GetInputSignature(0)\n\
             print('shader-signature', sig.name == 'inColor' and sig.semanticIndex == 0 and sig.GetName() == 'inColor' and isinstance(sig.components, int))\n\
             sigs = sr.GetInputSig()\n\
             print('signature-list', len(sigs) >= 1 and sigs[0].name == 'inColor' and sigs[0].GetSemanticIndex() == 0 and isinstance(sigs[0].components, int) and isinstance(sr.GetConstantBlocks(), list) and isinstance(sr.GetReadWriteResources(), list) and isinstance(sr.GetSamplers(), list))\n\
             vb = renderdocx.ViewBuffer(0, 0)\n\
             print('view-buffer', vb.Name() == 'buffer')\n\
             tv = renderdocx.GetTextureViewer()\n\
             tv.ViewTexture(0, 2, True)\n\
             tv.SetSelectedSubresource(0, 0, 0)\n\
             tv.SetTextureOverlay(1)\n\
             tv.SetZoomLevel(False, 2.5)\n\
             tv.SetChannelVisibility(True, True, True, True)\n\
             print('texture-viewer', tv.GetCurrentResource() == 0 and tv.GetSelectedSubresource().get('sample') == 0 and tv.GetTextureOverlay() == 1 and abs(tv.GetZoomLevel() - 2.5) < 0.001 and tv.GetChannelVisibility() == (True, True, True, True) and tv.GetChannelVisibilityBits() == 15 and not tv.IsZoomAutoFit())\n\
             display = tv.GetTextureDisplay()\n\
             print('texture-display', display.get('resource') == 0 and display.get('typeCast') == 2 and display.get('mip') == 0 and display.get('overlay') == 1 and display.get('autoFit') is False and display.get('channels') == [True, True, True, True])\n\
             tv.SetHistogramRange(0.1, 2.0)\n\
             tv.GotoLocation(3, 4)\n\
             print('texture-viewer-extra', abs(tv.GetHistogramRange().get('min') - 0.1) < 0.001 and tv.GetPickedLocation().get('x') == 3 and tv.GetPickedLocation().get('y') == 4 and tv.CurrentResource() == 0)\n\
             eb = renderdocx.GetEventBrowser()\n\
             eb.SetCurrentFilterText('alpha')\n\
             print('event-browser-state', eb.GetCurrentFilterText() == 'alpha')\n\
             print('event-filters', eb.RegisterEventFilterFunction('custom', 'desc', lambda *args: True) and not eb.RegisterEventFilterFunction('custom', 'desc', lambda *args: True) and eb.UnregisterEventFilterFunction('custom') and not eb.UnregisterEventFilterFunction('custom'))\n\
             renderdocx.GetAPIInspector().RevealParameter('foo')\n\
             renderdocx.GetResourceInspector().RevealParameter('bar')\n\
             eb.SetUseCustomActionNames(True)\n\
             eb.SetShowParameterNames(True)\n\
             eb.SetShowAllParameters(True)\n\
             eb.SetEmptyRegionsVisible(True)\n\
             eb.SetDurationColumnVisible(False)\n\
             eb.SetAnnotationColumnVisible(False)\n\
             eb.UpdateDurationColumn()\n\
             eb.SetHighlightedAnnotation('hi')\n\
             print('event-browser-extra', eb.GetHighlightedAnnotation() == 'hi')\n\
             vb.ScrollToRow(2)\n\
             vb.ScrollToColumn(3)\n\
             print('buffer-viewer', vb.GetScrollRow() == 2 and vb.GetScrollColumn() == 3 and vb.GetEventName(19) == renderdocx.event_format(19))\n\
             ph = renderdocx.ViewPixelHistory(0, 1, 1, event=19)\n\
             ph.SetHistory({'event_id': 19, 'entries': []})\n\
             ph.SetFailedDebug()\n\
             print('view-pixel-history', ph.get('event_id') == 19 and ph.GetEvent() == 19 and isinstance(ph.GetHistory(), list) and ph.Widget() is None)\n\
             vm = renderdocx.ViewShaderMessages(0, event=19)\n\
             print('view-shader-messages', vm.get('event_id') == 19 and vm.GetEvent() == 19 and isinstance(vm.GetShaderMessages(), list) and vm.IsOutOfDate() is False and vm.Widget() is None)\n\
             eb = renderdocx.GetEventBrowser()\n\
             print('event-browser', eb.GetEventName(19) == renderdocx.event_format(19) and eb.GetActionForEID(19).get('event_id') == 21 and eb.IsAPIEventVisible(19) and eb.GetAPIEventForEID(19).get('event_id') == 19 and eb.Widget() is None)\n\
             renderdocx.GetResourceInspector().Inspect(7)\n\
             fd = renderdocx.FrameDescription()\n\
             print('frame-description', fd.frameNumber == snap.get('frame') and fd.GetFrameNumber() == snap.get('frame') and fd.containsAnnotations is False and isinstance(fd.GetCaptureTime(), str) and 'frameNumber' in fd.keys())\n\
             ap = renderdocx.APIProperties()\n\
             print('api-properties', ap.pipelineType == snap.get('api') and ap.localRenderer == snap.get('local_renderer') and ap.GetPipelineType() == snap.get('api') and 'pipelineType' in ap.keys())\n\
             print('descriptor-store', renderdocx.DescriptorStore(0) is None)\n\
             trace = renderdocx.DebugShader(7, 21)\n\
             print('debugshader', trace.get('event_id') == 21)\n\
             var = renderdocx.ShaderVariable((trace.get('inputs') or [{}])[0])\n\
             print('shader-variable', isinstance(var.name, str) and var.GetName() == var.name and isinstance(var.GetMembers(), list) and isinstance(var.GetValue(), str))\n\
             renderdocx.debug_collect()\n\
             sh = renderdocx.ViewShader(7)\n\
             line = next((i.get('disassembly_line') for i in (trace.get('instructions') or []) if i.get('disassembly_line', 0) > 0), None)\n\
             sh.ToggleBreakpointOnDisassemblyLine(line, True)\n\
             print('shader-breakpoint', line is not None and len(sh.GetBreakpoints()) == 1 and isinstance(renderdocx.debug_timeline(), dict))\n\
             run = sh.RunForward()\n\
             print('shader-run', isinstance(run, dict) and run.get('reason') in ('breakpoint', 'boundary', 'step', 'event', 'source') and isinstance(run.get('position'), int))\n\
             cb = sh.GetCurrentBreakpoint()\n\
             print('shader-current-breakpoint', (run.get('reason') != 'breakpoint') or isinstance(cb, dict))\n\
             notes0 = renderdocx.GetNotes()\n\
             bookmarks0 = renderdocx.GetBookmarks()\n\
             renderdocx.SetNotes('renderdocx notes')\n\
             print('notes', renderdocx.GetNotes() == 'renderdocx notes')\n\
             renderdocx.SetBookmark(19, 'script bookmark')\n\
             print('bookmark', any(b.get('eventId') == 19 and b.get('text') == 'script bookmark' for b in renderdocx.GetBookmarks()))\n\
             renderdocx.RemoveBookmark(19)\n\
             print('unbookmark', all(b.get('eventId') != 19 for b in renderdocx.GetBookmarks()))\n\
             renderdocx.SetNotes(notes0)\n\
             [renderdocx.SetBookmark(int(b['eventId']), b['text']) for b in bookmarks0]",
            &CaptureView::default(),
        );
        set_active_session(None);
        assert!(result.error.is_none(), "{:?}", result.error);
        assert!(result.stdout.contains("19"), "{}", result.stdout);
        assert!(result.stdout.contains("32"), "buffer hex length: {}", result.stdout);
        assert!(result.stdout.contains("shaders"), "{}", result.stdout);
        assert!(result.stdout.contains("mesh True"), "{}", result.stdout);
        assert!(result.stdout.contains("export True"), "{}", result.stdout);
        assert!(result.stdout.contains("context True"), "{}", result.stdout);
        assert!(result.stdout.contains("minmax True"), "{}", result.stdout);
        assert!(result.stdout.contains("fit True"), "{}", result.stdout);
        assert!(result.stdout.contains("snapshot True"), "{}", result.stdout);
        assert!(result.stdout.contains("actions True"), "{}", result.stdout);
        assert!(result.stdout.contains("meshread True"), "{}", result.stdout);
        assert!(result.stdout.contains("bufexport True"), "{}", result.stdout);
        assert!(result.stdout.contains("render True"), "{}", result.stdout);
        assert!(result.stdout.contains("compat True"), "{}", result.stdout);
        assert!(result.stdout.contains("replay True"), "{}", result.stdout);
        assert!(result.stdout.contains("pipeline-object True"), "{}", result.stdout);
        assert!(result.stdout.contains("api-specific True"), "{}", result.stdout);
        assert!(result.stdout.contains("vulkan-deep True"), "{}", result.stdout);
        assert!(result.stdout.contains("texture-object True"), "{}", result.stdout);
        assert!(result.stdout.contains("action-object True"), "{}", result.stdout);
        assert!(result.stdout.contains("replace True"), "{}", result.stdout);
        assert!(result.stdout.contains("unreplace True"), "{}", result.stdout);
        assert!(result.stdout.contains("debug True"), "{}", result.stdout);
        assert!(result.stdout.contains("debugnext True"), "{}", result.stdout);
        assert!(result.stdout.contains("pipeline-fields True"), "{}", result.stdout);
        assert!(result.stdout.contains("action-helpers True"), "{}", result.stdout);
        assert!(result.stdout.contains("resource-info True"), "{}", result.stdout);
        assert!(result.stdout.contains("viewers True"), "{}", result.stdout);
        assert!(result.stdout.contains("viewshader True"), "{}", result.stdout);
        assert!(result.stdout.contains("shader-viewer True"), "{}", result.stdout);
        assert!(result.stdout.contains("resource-object True"), "{}", result.stdout);
        assert!(result.stdout.contains("resource-id True"), "{}", result.stdout);
        assert!(result.stdout.contains("shader-reflection True"), "{}", result.stdout);
        assert!(result.stdout.contains("shader-signature True"), "{}", result.stdout);
        assert!(result.stdout.contains("signature-list True"), "{}", result.stdout);
        assert!(result.stdout.contains("view-buffer True"), "{}", result.stdout);
        assert!(result.stdout.contains("texture-viewer True"), "{}", result.stdout);
        assert!(result.stdout.contains("texture-display True"), "{}", result.stdout);
        assert!(result.stdout.contains("event-filters True"), "{}", result.stdout);
        assert!(
            result.stdout.contains("texture-viewer-extra True"),
            "{}",
            result.stdout
        );
        assert!(
            result.stdout.contains("event-browser-state True"),
            "{}",
            result.stdout
        );
        assert!(
            result.stdout.contains("event-browser-extra True"),
            "{}",
            result.stdout
        );
        assert!(result.stdout.contains("buffer-viewer True"), "{}", result.stdout);
        assert!(
            result.stdout.contains("view-pixel-history True"),
            "{}",
            result.stdout
        );
        assert!(
            result.stdout.contains("view-shader-messages True"),
            "{}",
            result.stdout
        );
        assert!(result.stdout.contains("event-browser True"), "{}", result.stdout);
        assert!(
            result.stdout.contains("frame-description True"),
            "{}",
            result.stdout
        );
        assert!(result.stdout.contains("api-properties True"), "{}", result.stdout);
        assert!(
            result.stdout.contains("descriptor-store True"),
            "{}",
            result.stdout
        );
        assert!(result.stdout.contains("shader-variable True"), "{}", result.stdout);
        assert!(
            result.stdout.contains("shader-breakpoint True"),
            "{}",
            result.stdout
        );
        assert!(result.stdout.contains("shader-run True"), "{}", result.stdout);
        assert!(
            result.stdout.contains("shader-current-breakpoint True"),
            "{}",
            result.stdout
        );
        assert!(result.stdout.contains("debugshader True"), "{}", result.stdout);
        assert_eq!(
            take_ui_requests(),
            vec![
                "shader:4",
                "edit_shader:4",
                "buffer:0:0",
                "textures",
                "parameters",
                "resources",
                "textures",
                "shader_messages",
                "resource:7",
                "shader:7"
            ]
        );
        assert!(result.stdout.contains("notes True"), "{}", result.stdout);
        assert!(result.stdout.contains("bookmark True"), "{}", result.stdout);
        assert!(result.stdout.contains("unbookmark True"), "{}", result.stdout);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires RENDERDOCX_TEST_BRIDGE and RENDERDOCX_TEST_VULKAN_VERTEX_CAPTURE"]
    fn python_vertex_input_query_a_real_capture() {
        use crate::backend::Settings;
        use std::path::PathBuf;

        let bridge = PathBuf::from(
            std::env::var("RENDERDOCX_TEST_BRIDGE").expect("set RENDERDOCX_TEST_BRIDGE"),
        );
        let capture = PathBuf::from(
            std::env::var("RENDERDOCX_TEST_VULKAN_VERTEX_CAPTURE")
                .expect("set RENDERDOCX_TEST_VULKAN_VERTEX_CAPTURE"),
        );
        let mut settings = Settings::default();
        settings.replay_bridge = Some(bridge);
        let (session, snapshot) =
            crate::replay::Session::open(&settings, &capture).expect("open capture");
        set_active_session(Some(session));
        set_active_format_context(
            snapshot.resources.clone(),
            snapshot.buffers.clone(),
            snapshot.textures.clone(),
            crate::i18n::Language::English,
        );
        set_active_actions(snapshot.actions.clone());
        set_active_snapshot(serde_json::to_value(&snapshot).expect("serialize snapshot"));
        let result = run_script_with_capture(
            "import renderdocx\n\
             page = renderdocx.vertex_input(limit=4)\n\
             rows = page.get('rows') or []\n\
             print('vertex-event', page.get('event_id'))\n\
             print('vertex-rows', len(rows))\n\
             row = rows[0]\n\
             trace = renderdocx.debug_vertex(page['event_id'], row['row'], 0, row.get('vertex_index') or row['row'], 0)\n\
             print('debug-vertex', trace.get('event_id') == page.get('event_id'))\n\
             renderdocx.debug_stop()\n\
             source = '#version 450 core\\nlayout(location = 0) out vec4 Color;\\nvoid main() { Color = vec4(1.0, 0.0, 0.0, 1.0); }\\n'\n\
             edit = renderdocx.apply_shader_edit(page['event_id'], 4, 2, 'main', source)\n\
             print('shader-edit', edit.get('applied') == True)\n\
             revert = renderdocx.revert_shader_edit(page['event_id'], 4)\n\
             print('shader-revert', revert.get('applied') == False)",
            &CaptureView::default(),
        );
        set_active_session(None);
        assert!(result.error.is_none(), "{:?}", result.error);
        assert!(
            result.stdout.contains("debug-vertex True"),
            "{}",
            result.stdout
        );
        assert!(result.stdout.contains("shader-edit True"), "{}", result.stdout);
        assert!(
            result.stdout.contains("shader-revert True"),
            "{}",
            result.stdout
        );

        let rows: usize = result
            .stdout
            .lines()
            .find_map(|line| line.strip_prefix("vertex-rows "))
            .and_then(|line| line.parse().ok())
            .unwrap_or(0);
        assert!(rows > 0, "expected vertex input rows: {}", result.stdout);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires RENDERDOCX_TEST_BRIDGE and RENDERDOCX_TEST_VULKAN_PRINTF_CAPTURE"]
    fn python_shader_printf_debug_a_real_capture() {
        use crate::backend::Settings;
        use std::path::PathBuf;

        let bridge = PathBuf::from(
            std::env::var("RENDERDOCX_TEST_BRIDGE").expect("set RENDERDOCX_TEST_BRIDGE"),
        );
        let capture = PathBuf::from(
            std::env::var("RENDERDOCX_TEST_VULKAN_PRINTF_CAPTURE")
                .expect("set RENDERDOCX_TEST_VULKAN_PRINTF_CAPTURE"),
        );
        let mut settings = Settings::default();
        settings.replay_bridge = Some(bridge);
        let (session, snapshot) =
            crate::replay::Session::open(&settings, &capture).expect("open capture");
        set_active_session(Some(session));
        set_active_format_context(
            snapshot.resources.clone(),
            snapshot.buffers.clone(),
            snapshot.textures.clone(),
            crate::i18n::Language::English,
        );
        set_active_actions(snapshot.actions.clone());
        set_active_snapshot(serde_json::to_value(&snapshot).expect("serialize snapshot"));
        let result = run_script_with_capture(
            "import renderdocx\n\
             actions = renderdocx.capture_actions()\n\
             dispatch = next(a for a in actions if a.get('flags', 0) & 4)\n\
             draw = next(a for a in actions if a.get('flags', 0) & 2)\n\
             opt = lambda v: None if v in (None, 4294967295) else v\n\
             messages = renderdocx.shader_messages(int(dispatch['event_id']))\n\
             location = messages['messages'][0]['location']\n\
             trace = renderdocx.debug_compute(int(dispatch['event_id']), location['workgroup'], location['thread'])\n\
             print('debug-compute', trace.get('event_id') == dispatch['event_id'])\n\
             renderdocx.debug_stop()\n\
             messages = renderdocx.shader_messages(int(draw['event_id']))\n\
             location = messages['messages'][0]['location']\n\
             trace = renderdocx.debug_pixel(int(draw['event_id']), location['x'], location['y'], opt(location.get('sample')), opt(location.get('primitive')), opt(location.get('view')))\n\
             print('debug-pixel', trace.get('event_id') == draw['event_id'])\n\
             renderdocx.debug_stop()",
            &CaptureView::default(),
        );
        set_active_session(None);
        assert!(result.error.is_none(), "{:?}", result.error);
        assert!(
            result.stdout.contains("debug-compute True"),
            "{}",
            result.stdout
        );
        assert!(
            result.stdout.contains("debug-pixel True"),
            "{}",
            result.stdout
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires RENDERDOCX_TEST_BRIDGE and RENDERDOCX_TEST_D3D11_CONSTANT_CAPTURE"]
    fn python_constant_block_query_a_real_capture() {
        use crate::backend::Settings;
        use std::path::PathBuf;

        let bridge = PathBuf::from(
            std::env::var("RENDERDOCX_TEST_BRIDGE").expect("set RENDERDOCX_TEST_BRIDGE"),
        );
        let capture = PathBuf::from(
            std::env::var("RENDERDOCX_TEST_D3D11_CONSTANT_CAPTURE")
                .expect("set RENDERDOCX_TEST_D3D11_CONSTANT_CAPTURE"),
        );
        let mut settings = Settings::default();
        settings.replay_bridge = Some(bridge);
        let (session, snapshot) =
            crate::replay::Session::open(&settings, &capture).expect("open capture");
        set_active_session(Some(session));
        set_active_format_context(
            snapshot.resources.clone(),
            snapshot.buffers.clone(),
            snapshot.textures.clone(),
            crate::i18n::Language::English,
        );
        set_active_actions(snapshot.actions.clone());
        set_active_snapshot(serde_json::to_value(&snapshot).expect("serialize snapshot"));
        let result = run_script_with_capture(
            "import renderdocx\n\
             actions = renderdocx.capture_actions()\n\
             dispatch = next(a for a in actions if a.get('flags', 0) & 4)\n\
             sr = renderdocx.ShaderReflection(5, event=int(dispatch['event_id']))\n\
             blocks = sr.GetConstantBlocks()\n\
             block = blocks[0]\n\
             var = block.GetVariable(0)\n\
             print('constant-block', len(blocks) >= 1 and block.name == 'Params' and block.byteSize == 16 and block.bufferBacked is True)\n\
             print('shader-constant', var.name == 'seed' and var.byteOffset == 0 and isinstance(var.defaultValue, str) and var.GetName() == 'seed')\n\
             ty = var.GetType()\n\
             print('reflected-type', ty.name == 'uint' and ty.baseType == 4 and ty.rows == 1 and ty.columns == 1 and ty.GetName() == 'uint' and isinstance(ty.GetMembers(), list) and 'base_type' in ty.keys())\n\
             vars = block.GetVariables()\n\
             rw = sr.GetReadWriteResources()\n\
             print('deep-members', len(vars) == 1 and vars[0].GetName() == 'seed' and vars[0].GetType().name == 'uint' and isinstance(vars[0].GetType().GetMembers(), list) and len(rw) == 1 and rw[0].name == 'output' and rw[0].descriptorType == 9 and rw[0].isTexture is False and rw[0].GetType().name == 'uint' and renderdocx.ReflectedResource({'name':'x','bind':0,'space':0,'array_size':1,'texture_type':0,'descriptor_type':9,'is_texture':False,'has_sampler':False,'input_attachment':False,'read_only':True,'variable_type':{'name':'uint','flags':1,'pointer_type_id':4294967295,'elements':1,'array_stride':4,'base_type':4,'rows':1,'columns':1,'matrix_stride':4,'members':[]}}).GetType().name == 'uint')\n\
             p = renderdocx.pipeline(int(dispatch['event_id']))\n\
             d3 = p.get('api_specific', {}).get('d3d11')\n\
             print('d3d11-api-specific', p.get('api_specific', {}).get('kind') == 'D3D11' and isinstance(d3, dict) and d3.get('input_layout', {}).get('bound') is True and d3.get('layout_count', 0) >= 1 and isinstance(d3.get('stream_out'), list) and d3.get('descriptor_count', 0) > 0 and d3.get('descriptor_byte_size', 0) >= 1 and isinstance(d3.get('shader_class_instances'), dict))\n\
             print('sampler-list', len(sr.GetSamplers()) == 0)",
            &CaptureView::default(),
        );
        set_active_session(None);
        assert!(result.error.is_none(), "{:?}", result.error);
        assert!(
            result.stdout.contains("constant-block True"),
            "{}",
            result.stdout
        );
        assert!(
            result.stdout.contains("shader-constant True"),
            "{}",
            result.stdout
        );
        assert!(
            result.stdout.contains("reflected-type True"),
            "{}",
            result.stdout
        );
        assert!(
            result.stdout.contains("deep-members True"),
            "{}",
            result.stdout
        );
        assert!(
            result.stdout.contains("d3d11-api-specific True"),
            "{}",
            result.stdout
        );
        assert!(
            result.stdout.contains("sampler-list True"),
            "{}",
            result.stdout
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires RENDERDOCX_TEST_BRIDGE and RENDERDOCX_TEST_OPENGL_CAPTURE"]
    fn python_opengl_api_specific_state_a_real_capture() {
        use crate::backend::Settings;
        use std::path::PathBuf;

        let bridge = PathBuf::from(
            std::env::var("RENDERDOCX_TEST_BRIDGE").expect("set RENDERDOCX_TEST_BRIDGE"),
        );
        let capture = PathBuf::from(
            std::env::var("RENDERDOCX_TEST_OPENGL_CAPTURE")
                .expect("set RENDERDOCX_TEST_OPENGL_CAPTURE"),
        );
        let mut settings = Settings::default();
        settings.replay_bridge = Some(bridge);
        let (session, snapshot) =
            crate::replay::Session::open(&settings, &capture).expect("open capture");
        set_active_session(Some(session));
        set_active_format_context(
            snapshot.resources.clone(),
            snapshot.buffers.clone(),
            snapshot.textures.clone(),
            crate::i18n::Language::English,
        );
        set_active_actions(snapshot.actions.clone());
        set_active_snapshot(serde_json::to_value(&snapshot).expect("serialize snapshot"));
        let result = run_script_with_capture(
            "import renderdocx\n\
             actions = renderdocx.capture_actions()\n\
             draw = next(a for a in actions if 'glDrawArrays' in a.get('name', ''))\n\
             p = renderdocx.pipeline(int(draw['event_id']))\n\
             gl = p.get('api_specific', {}).get('gl')\n\
             print('opengl-api-specific', p.get('api_specific', {}).get('kind') == 'OpenGL' and isinstance(gl, dict) and gl.get('vertex_array_object', {}).get('bound') is True and gl.get('attribute_count', 0) > 0 and gl.get('vertex_buffer_count', 0) > 0 and gl.get('descriptor_count', 0) > 0 and isinstance(gl.get('feedback', {}).get('buffers'), list) and isinstance(gl.get('framebuffer', {}).get('draw_fbo'), dict) and gl.get('rasterizer', {}).get('sample_mask_value') == 4294967295)",
            &CaptureView::default(),
        );
        set_active_session(None);
        assert!(result.error.is_none(), "{:?}", result.error);
        assert!(
            result.stdout.contains("opengl-api-specific True"),
            "{}",
            result.stdout
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires RENDERDOCX_TEST_BRIDGE and RENDERDOCX_TEST_D3D12_CAPTURE"]
    fn python_d3d12_api_specific_state_a_real_capture() {
        use crate::backend::Settings;
        use std::path::PathBuf;

        let bridge = PathBuf::from(
            std::env::var("RENDERDOCX_TEST_BRIDGE").expect("set RENDERDOCX_TEST_BRIDGE"),
        );
        let capture = PathBuf::from(
            std::env::var("RENDERDOCX_TEST_D3D12_CAPTURE")
                .expect("set RENDERDOCX_TEST_D3D12_CAPTURE"),
        );
        let mut settings = Settings::default();
        settings.replay_bridge = Some(bridge);
        let (session, snapshot) =
            crate::replay::Session::open(&settings, &capture).expect("open capture");
        set_active_session(Some(session));
        set_active_format_context(
            snapshot.resources.clone(),
            snapshot.buffers.clone(),
            snapshot.textures.clone(),
            crate::i18n::Language::English,
        );
        set_active_actions(snapshot.actions.clone());
        set_active_snapshot(serde_json::to_value(&snapshot).expect("serialize snapshot"));
        let result = run_script_with_capture(
            "import renderdocx\n\
             actions = renderdocx.capture_actions()\n\
             draw = next(a for a in actions if 'DrawInstanced' in a.get('name', ''))\n\
             p = renderdocx.pipeline(int(draw['event_id']))\n\
             d3 = p.get('api_specific', {}).get('d3d12')\n\
             print('d3d12-api-specific', p.get('api_specific', {}).get('kind') == 'D3D12' and isinstance(d3, dict) and d3.get('pipeline', {}).get('bound') is True and isinstance(d3.get('descriptor_heaps'), list) and d3.get('root_signature', {}).get('bound') is True and isinstance(d3.get('input_layout_count'), int) and d3.get('topology') == 5 and d3.get('sample_mask') == 4294967295 and d3.get('depth_writes') is True and isinstance(d3.get('line_raster'), int) and isinstance(d3.get('predication'), dict))\n\
             rps = d3.get('rootParameters') or []\n\
             rp = renderdocx.RootParameter(rps[0])\n\
             sss = d3.get('staticSamplers') or []\n\
             ss = renderdocx.StaticSampler(sss[0])\n\
             ps = renderdocx.PipelineState(int(draw['event_id']))\n\
             print('d3d12-deep', len(rps) >= 1 and isinstance(rp.visibility, int) and isinstance(rp.constantsSize, int) and isinstance(rp.descriptor, dict) and 'visibility' in rp.keys() and len(ps.GetD3D12RootParameters()) >= 1 and len(ps.GetD3D12StaticSamplers()) == 2)\n\
             print('static-sampler', len(sss) == 2 and ss.reg == 5 and ss.filterMinify == 1 and ss.addressU == 3 and ss.seamlessCubemaps is True and 'filterMinify' in ss.keys())",
            &CaptureView::default(),
        );
        set_active_session(None);
        assert!(result.error.is_none(), "{:?}", result.error);
        assert!(
            result.stdout.contains("d3d12-api-specific True"),
            "{}",
            result.stdout
        );
        assert!(
            result.stdout.contains("d3d12-deep True"),
            "{}",
            result.stdout
        );
        assert!(
            result.stdout.contains("static-sampler True"),
            "{}",
            result.stdout
        );
    }

    #[test]
    fn debugger_configs_are_created_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let vscode = write_debugger_configs(dir.path()).unwrap();
        let launch = vscode.join("launch.json");
        let tasks = vscode.join("tasks.json");
        assert!(launch.is_file());
        assert!(tasks.is_file());
        let launch_text = std::fs::read_to_string(&launch).unwrap();
        assert!(launch_text.contains("\"request\": \"attach\""));
        assert!(launch_text.contains("\"port\": 5678"));
        std::fs::write(&launch, "custom").unwrap();
        write_debugger_configs(dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&launch).unwrap(), "custom");
    }

    #[test]
    fn debugpy_site_resolution_finds_vendored_copy() {
        for candidate in debugpy_search_paths() {
            assert!(candidate.is_absolute());
        }
        if let Some(site) = debugpy_site() {
            assert!(site.join("debugpy").is_dir());
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires debugpy in target/python-site"]
    fn python_debugpy_attach_speaks_dap() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::time::Duration;

        let port = 5691u16;
        let result = run_script_with_capture(
            &format!(
                "import renderdocx\n\
                 shell = renderdocx.GetPythonShell()\n\
                 shell.AttachDebugger('', {port})\n\
                 print('listening', shell.IsDebuggerListening(), shell.GetDebuggerPort())"
            ),
            &CaptureView::default(),
        );
        assert!(result.error.is_none(), "{:?}", result.error);
        assert!(
            result.stdout.contains(&format!("listening True {port}")),
            "{}",
            result.stdout
        );
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to debugpy");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let body = r#"{"seq":1,"type":"request","command":"initialize","arguments":{"adapterID":"debugpy"}}"#;
        write!(stream, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
        let mut response = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut chunk = [0u8; 8192];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => response.extend_from_slice(&chunk[..count]),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(error) => panic!("read DAP initialize response: {error}"),
            }
            let text = String::from_utf8_lossy(&response);
            if let Some((_, payload)) = text.split_once("\r\n\r\n") {
                if payload.contains("\"command\": \"initialize\"") {
                    break;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no DAP initialize response: {text}"
            );
        }
        let text = String::from_utf8_lossy(&response);
        assert!(text.contains("\"command\": \"initialize\""), "{text}");
        assert!(text.contains("\"success\": true"), "{text}");
        let connected = run_script_with_capture(
            "import renderdocx\nprint(renderdocx.GetPythonShell().IsDebuggerConnected())",
            &CaptureView::default(),
        );
        assert!(connected.error.is_none(), "{:?}", connected.error);
    }

    #[test]
    fn shader_viewer_save_revert_callbacks_are_invoked() {
        let result = run_script_with_capture(
            "import renderdocx\n\
             saved = []\n\
             reverted = []\n\
             h = renderdocx.EditShader(6, save_callback=lambda stage, entry, source: saved.append((stage, entry, source)), revert_callback=lambda stage: reverted.append(stage))\n\
             h.InvokeSaveCallback('main', 'void main() {}')\n\
             h.InvokeRevertCallback()\n\
             print(h.HasSaveCallback(), h.HasRevertCallback(), saved, reverted)",
            &CaptureView::default(),
        );
        assert!(result.error.is_none(), "{:?}", result.error);
        assert!(
            result
                .stdout
                .contains("True True [(6, 'main', 'void main() {}')] [6]"),
            "{}",
            result.stdout
        );
        let result = run_script_with_capture(
            "import renderdocx\n\
             h = renderdocx.ViewShader(6)\n\
             print(h.HasSaveCallback(), repr(h.InvokeSaveCallback('main', 'x')), repr(h.InvokeRevertCallback()))",
            &CaptureView::default(),
        );
        assert!(result.error.is_none(), "{:?}", result.error);
        assert_eq!(result.stdout.trim(), "False None None");
        take_ui_requests();
    }

    #[test]
    fn gui_can_invoke_registered_shader_callbacks() {
        let dir = tempfile::tempdir().unwrap();
        let calls = dir.path().join("calls.txt");
        let calls_text = calls.display().to_string();
        let result = run_script_with_capture(
            &format!(
                "import renderdocx\n\
                 h = renderdocx.EditShader(5, save_callback=lambda stage, entry, source: open(r'{calls_text}', 'a', encoding='utf-8').write(f'save|{{stage}}|{{entry}}|{{source}}\\n'), revert_callback=lambda stage: open(r'{calls_text}', 'a', encoding='utf-8').write(f'revert|{{stage}}\\n'))\n\
                 print('registered', h.HasSaveCallback(), h.HasRevertCallback())"
            ),
            &CaptureView::default(),
        );
        assert!(result.error.is_none(), "{:?}", result.error);
        assert!(
            result.stdout.contains("registered True True"),
            "{}",
            result.stdout
        );
        assert!(invoke_shader_save_callback(5, 99, "main", "void main() {}"));
        assert!(invoke_shader_revert_callback(5, 99));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let written = loop {
            let written = std::fs::read_to_string(&calls).unwrap_or_default();
            if written.contains("revert|5") || std::time::Instant::now() > deadline {
                break written;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(
            written.replace("\r\n", "\n"),
            "save|5|main|void main() {}\nrevert|5\n",
            "{written}"
        );
        assert!(!invoke_shader_save_callback(4, 99, "main", "x"));
        assert!(!invoke_shader_revert_callback(4, 99));
        take_ui_requests();
    }

    #[test]
    fn rejects_nul_bytes() {
        assert!(cstring("a\0b").is_err());
    }
}
