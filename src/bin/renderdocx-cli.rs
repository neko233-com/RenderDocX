use renderdocx::{
    backend::Settings,
    debug_navigation::{Breakpoint, DebugNavigator, SourceStep, StopReason},
    live::{CaptureOptions, LaunchConfiguration, LiveSession, enumerate},
    replay::{
        BUFFER_PAGE_LIMIT, ComputeDebugSelection, DebugState, DebugTrace, MeshSelection, Pipeline,
        PixelDebugSelection, SHADER_STAGES, STRUCTURE_PAGE_LIMIT, Session, Snapshot,
        TextureSelection,
    },
};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{self, BufRead, Write},
    path::PathBuf,
    process::ExitCode,
};

enum Invocation {
    Output(Value),
    Serve(Settings),
}

struct AgentDebugTimeline {
    trace: DebugTrace,
    states: Vec<DebugState>,
    navigator: DebugNavigator,
}

impl AgentDebugTimeline {
    fn response(&self, reason: Option<StopReason>) -> Value {
        let state = self.navigator.current(&self.states);
        let location = state.and_then(|state| self.trace.instruction_info(state.next_instruction));
        json!({"event_id":self.trace.event_id,"stage":self.trace.stage,"position":self.navigator.position,
            "total":self.states.len(),"state":state,"mutable_variables":self.navigator.variables(&self.states),"location":location,"breakpoints":self.navigator.breakpoints,"reason":reason})
    }
}

fn usage() -> Value {
    json!({"schema":"renderdocx.cli.v1","ok":true,"commands":[
        {"name":"inspect","arguments":["capture.rdc"],"description":"Open a capture and return its action tree and resource counts."},
        {"name":"select","arguments":["capture.rdc","event_id"],"description":"Replay a capture to an event."},
        {"name":"serve","arguments":[],"description":"Serve sequential JSONL requests using a persistent replay session."},
        {"name":"capabilities","arguments":[],"description":"Describe the commands currently implemented."}
    ],"serve_commands":["capabilities","open","snapshot","select","pipeline","shader","shader_export","shader_apply","shader_revert","debug_vertex","debug_pixel","debug_compute","debug_collect","debug_state","debug_breakpoint","debug_navigate","debug_next","debug_stop","vertex_input","mesh","mesh_read","structure","structure_buffer","read_buffer","resource_usage","resource_rename","annotation_read","annotation_write","descriptors","debug_messages","clear_debug_messages","counter_fetch","counter_page","pick_pixel","pixel_history","texture_statistics","export_texture","close","target_list","target_launch","target_connect","target_connect_pid","target_events","target_trigger","target_queue","target_cycle","target_close","quit"],"shader_stages":SHADER_STAGES,"mesh_stages":["vs_out","gs_out","task_out","mesh_out"],"buffer_page_limit":BUFFER_PAGE_LIMIT,"structure_page_limit":STRUCTURE_PAGE_LIMIT,"vertex_input_page_limit":renderdocx::vertex_input::INPUT_PAGE_LIMIT,"resource_usage_page_limit":renderdocx::replay::RESOURCE_USAGE_PAGE_LIMIT,"counter_page_limit":renderdocx::replay::COUNTER_PAGE_LIMIT,"pixel_history_page_limit":renderdocx::replay::PIXEL_HISTORY_PAGE_LIMIT,"capture_options_default":CaptureOptions::default(),"environment_operations":["set","append","prepend"],"environment_separators":["platform","semi_colon","colon","no_sep"],"global_options":["--bridge PATH"],"limitations":["Additional RenderDoc GUI and agent features are not yet implemented."]})
}

fn run(args: Vec<OsString>) -> Result<Invocation, (&'static str, String)> {
    let mut args = args.into_iter();
    let mut settings = Settings::load();
    let mut first = args
        .next()
        .ok_or(("invalid_arguments", "Missing command".into()))?;
    if first == "--bridge" {
        settings.replay_bridge = Some(PathBuf::from(
            args.next()
                .ok_or(("invalid_arguments", "Missing bridge path".into()))?,
        ));
        first = args
            .next()
            .ok_or(("invalid_arguments", "Missing command".into()))?;
    }
    let command = first
        .to_str()
        .ok_or(("invalid_arguments", "Command must be UTF-8".into()))?;
    if command == "capabilities" || command == "--help" || command == "-h" {
        if args.next().is_some() {
            return Err(("invalid_arguments", "Unexpected argument".into()));
        }
        return Ok(Invocation::Output(usage()));
    }
    if command == "serve" {
        if args.next().is_some() {
            return Err(("invalid_arguments", "Unexpected argument".into()));
        }
        return Ok(Invocation::Serve(settings));
    }
    if command != "inspect" && command != "select" {
        return Err(("invalid_arguments", format!("Unknown command: {command}")));
    }
    let capture = PathBuf::from(
        args.next()
            .ok_or(("invalid_arguments", "Missing capture path".into()))?,
    );
    let event = if command == "select" {
        let value = args
            .next()
            .ok_or(("invalid_arguments", "Missing event ID".into()))?;
        Some(value.to_str().and_then(|v| v.parse::<u32>().ok()).ok_or((
            "invalid_arguments",
            "Event ID must be an unsigned 32-bit integer".into(),
        ))?)
    } else {
        None
    };
    if args.next().is_some() {
        return Err(("invalid_arguments", "Unexpected argument".into()));
    }
    let (session, snapshot) =
        Session::open(&settings, &capture).map_err(|e| ("replay_open_failed", e))?;
    if let Some(event_id) = event {
        let revision = session.next_revision();
        match session
            .select_pipeline(event_id, revision)
            .map_err(|e| ("replay_select_failed", e))?
        {
            Some(pipeline) => {
                session.close().map_err(|e| ("replay_close_failed", e))?;
                Ok(Invocation::Output(
                    json!({"schema":"renderdocx.cli.v1","ok":true,"command":"select","event_id":event_id,"pipeline":pipeline}),
                ))
            }
            None => Err((
                "replay_select_superseded",
                "Selection was superseded".into(),
            )),
        }
    } else {
        session.close().map_err(|e| ("replay_close_failed", e))?;
        Ok(Invocation::Output(
            json!({"schema":"renderdocx.cli.v1","ok":true,"command":"inspect","capture":snapshot}),
        ))
    }
}

fn serve(settings: Settings, input: impl BufRead, mut output: impl Write) -> io::Result<()> {
    let mut opened: Option<(Session, Snapshot)> = None;
    let mut selected_event: Option<u32> = None;
    let mut pipeline: Option<Pipeline> = None;
    let mut mesh_selection: Option<(u32, MeshSelection)> = None;
    let mut pending_debug: Option<DebugTrace> = None;
    let mut debug_streamed = false;
    let mut debug_timeline: Option<AgentDebugTimeline> = None;
    let mut target: Option<LiveSession> = None;
    for line in input.lines() {
        let line = line?;
        let parsed: Result<Value, _> = serde_json::from_str(&line);
        let request = match parsed {
            Ok(Value::Object(object)) => object,
            _ => {
                writeln!(
                    output,
                    "{}",
                    json!({"schema":"renderdocx.agent.v1","id":null,"ok":false,"error":{"code":"invalid_request","message":"Expected a JSON object"}})
                )?;
                output.flush()?;
                continue;
            }
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let command = request.get("command").and_then(Value::as_str).unwrap_or("");
        let result: Result<Value, (&str, String)> = match command {
            "capabilities" => Ok(usage()),
            "target_list" => enumerate(&settings)
                .map(|idents| json!({"idents":idents}))
                .map_err(|error| ("target_discovery_failed", error)),
            "target_launch" => {
                let app = request
                    .get("path")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty());
                let template = request
                    .get("capture_directory")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty());
                let configuration = match request.get("configuration") {
                    Some(_) if request.contains_key("arguments") => None,
                    Some(value) => {
                        serde_json::from_value::<LaunchConfiguration>(value.clone()).ok()
                    }
                    None => request.get("arguments").map_or(
                        Some(LaunchConfiguration::default()),
                        |value| {
                            value.as_str().map(|arguments| LaunchConfiguration {
                                arguments: arguments.to_owned(),
                                ..Default::default()
                            })
                        },
                    ),
                };
                match (&target, app, template, configuration) {
                    (Some(_), _, _, _) => {
                        Err(("target_open", "Close the existing target first".into()))
                    }
                    (None, Some(app), Some(template), Some(configuration))
                        if configuration.validate().is_ok() =>
                    {
                        match LiveSession::launch(
                            &settings,
                            PathBuf::from(app).as_path(),
                            PathBuf::from(template).as_path(),
                            &configuration,
                        ) {
                            Ok(session) => {
                                let value = json!({"target":session.connected, "capture_template":session.capture_template});
                                target = Some(session);
                                Ok(value)
                            }
                            Err(error) => Err(("target_launch_failed", error)),
                        }
                    }
                    _ => Err((
                        "invalid_arguments",
                        "target_launch requires path, capture_directory and a valid configuration"
                            .into(),
                    )),
                }
            }
            "target_connect" => {
                let ident = request
                    .get("ident")
                    .and_then(Value::as_u64)
                    .and_then(|v| u32::try_from(v).ok())
                    .filter(|v| *v != 0);
                match (&target, ident) {
                    (Some(_), _) => Err(("target_open", "Close the existing target first".into())),
                    (None, Some(ident)) => match LiveSession::connect(&settings, ident) {
                        Ok(session) => {
                            let value = json!({"target":session.connected});
                            target = Some(session);
                            Ok(value)
                        }
                        Err(error) => Err(("target_connect_failed", error)),
                    },
                    _ => Err((
                        "invalid_arguments",
                        "target_connect requires a nonzero ident".into(),
                    )),
                }
            }
            "target_connect_pid" => {
                let pid = request
                    .get("pid")
                    .and_then(Value::as_u64)
                    .and_then(|v| u32::try_from(v).ok())
                    .filter(|v| *v != 0);
                match (&target, pid) {
                    (Some(_), _) => Err(("target_open", "Close the existing target first".into())),
                    (None, Some(pid)) => match LiveSession::connect_pid(&settings, pid) {
                        Ok(session) => {
                            let value = json!({"target":session.connected});
                            target = Some(session);
                            Ok(value)
                        }
                        Err(error) => Err(("target_connect_failed", error)),
                    },
                    _ => Err((
                        "invalid_arguments",
                        "target_connect_pid requires a nonzero PID".into(),
                    )),
                }
            }
            "target_events" => match &target {
                None => Err(("no_target", "No target session is open".into())),
                Some(session) => session
                    .drain()
                    .map(|events| json!({"connected":session.is_alive(),"events":events}))
                    .map_err(|e| ("target_failed", e)),
            },
            "target_trigger" | "target_queue" | "target_cycle" => match &target {
                None => Err(("no_target", "No target session is open".into())),
                Some(session) => {
                    let count = request
                        .get("frames")
                        .and_then(Value::as_u64)
                        .and_then(|v| u32::try_from(v).ok())
                        .filter(|v| *v > 0);
                    let frame = request
                        .get("frame")
                        .and_then(Value::as_u64)
                        .and_then(|v| u32::try_from(v).ok());
                    let sent = match command {
                        "target_cycle" => Some(session.cycle()),
                        "target_trigger" => count.map(|count| session.trigger(count)),
                        _ => frame
                            .zip(count)
                            .map(|(frame, count)| session.queue(frame, count)),
                    };
                    match sent {
                        Some(result) => result
                            .map(|_| json!({"submitted":true}))
                            .map_err(|e| ("target_failed", e)),
                        None => Err((
                            "invalid_arguments",
                            "A nonzero frames value and, for queue, frame are required".into(),
                        )),
                    }
                }
            },
            "target_close" => match target.take() {
                Some(session) => session
                    .close()
                    .map(|_| json!({}))
                    .map_err(|e| ("target_close_failed", e)),
                None => Ok(json!({})),
            },
            "open" => match request
                .get("path")
                .and_then(Value::as_str)
                .filter(|path| !path.is_empty())
            {
                Some(path) => match Session::open(&settings, PathBuf::from(path).as_path()) {
                    Ok((session, snapshot)) => {
                        let value = json!({"capture": &snapshot});
                        opened = Some((session, snapshot));
                        selected_event = None;
                        pipeline = None;
                        mesh_selection = None;
                        Ok(value)
                    }
                    Err(message) => Err(("replay_open_failed", message)),
                },
                None => Err(("invalid_arguments", "open requires a nonempty path".into())),
            },
            "snapshot" => match &opened {
                Some((_, snapshot)) => Ok(json!({"capture": snapshot})),
                None => Err(("no_capture", "No capture is open".into())),
            },
            "pipeline" => match (&opened, &pipeline) {
                (None, _) => Err(("no_capture", "No capture is open".into())),
                (Some(_), None) => Err((
                    "no_event",
                    "Select an event before inspecting pipeline state".into(),
                )),
                (Some(_), Some(state)) => Ok(json!({"pipeline": state})),
            },
            "structure" => {
                let event = request
                    .get("event_id")
                    .and_then(Value::as_u64)
                    .and_then(|v| u32::try_from(v).ok());
                let offset = match request.get("offset") {
                    None => Some(0),
                    Some(v) => v.as_str().and_then(|s| s.parse::<u64>().ok()),
                };
                let limit = match request.get("limit") {
                    None => Some(STRUCTURE_PAGE_LIMIT),
                    Some(v) => v
                        .as_u64()
                        .and_then(|v| u32::try_from(v).ok())
                        .filter(|v| *v > 0 && *v <= STRUCTURE_PAGE_LIMIT),
                };
                let path = match request.get("path") {
                    None => Some(Vec::new()),
                    Some(v) => v.as_array().and_then(|items| {
                        items
                            .iter()
                            .map(|item| item.as_str()?.parse::<u64>().ok())
                            .collect()
                    }),
                };
                match (&opened, event, path, offset, limit) {
                    (None, _, _, _, _) => Err(("no_capture", "No capture is open".into())),
                    (Some((session, _)), Some(event), Some(path), Some(offset), Some(limit)) => session.structure(event, &path, offset, limit)
                        .map(|page| json!({"structure":page}))
                        .map_err(|error| ("replay_structure_failed", error)),
                    _ => Err(("invalid_arguments", "structure requires event_id, an optional decimal-string path and offset, and a valid page limit".into())),
                }
            }
            "structure_buffer" => {
                let event = request
                    .get("event_id")
                    .and_then(Value::as_u64)
                    .and_then(|v| u32::try_from(v).ok());
                let offset = request
                    .get("offset")
                    .and_then(Value::as_str)
                    .and_then(|v| v.parse::<u64>().ok());
                let length = request
                    .get("length")
                    .and_then(Value::as_u64)
                    .and_then(|v| u32::try_from(v).ok())
                    .filter(|v| *v <= BUFFER_PAGE_LIMIT);
                let path = match request.get("path") {
                    None => Some(Vec::new()),
                    Some(value) => value.as_array().and_then(|items| {
                        items
                            .iter()
                            .map(|item| item.as_str()?.parse::<u64>().ok())
                            .collect()
                    }),
                };
                match (&opened, event, path, offset, length) {
                    (None, _, _, _, _) => Err(("no_capture", "No capture is open".into())),
                    (Some((session, _)), Some(event), Some(path), Some(offset), Some(length)) if offset.checked_add(u64::from(length)).is_some() =>
                        session.structure_buffer(event, &path, offset, length).map(|bytes| {
                            let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
                            json!({"event_id":event,"path":path.iter().map(u64::to_string).collect::<Vec<_>>(),"offset":offset.to_string(),"hex":hex})
                        }).map_err(|error| ("replay_structure_buffer_failed", error)),
                    _ => Err(("invalid_arguments", "structure_buffer requires event_id, decimal-string path and offset, and a valid length".into())),
                }
            }
            "select" => match (
                &opened,
                request
                    .get("event_id")
                    .and_then(Value::as_u64)
                    .and_then(|v| u32::try_from(v).ok()),
            ) {
                (None, _) => Err(("no_capture", "No capture is open".into())),
                (Some(_), None) => Err((
                    "invalid_arguments",
                    "select requires an unsigned 32-bit event_id".into(),
                )),
                (Some((session, _)), Some(event)) => {
                    let revision = session.next_revision();
                    match session.select_pipeline(event, revision) {
                        Ok(Some(state)) => {
                            selected_event = Some(event);
                            mesh_selection = None;
                            let value = json!({"event_id":event, "pipeline": &state});
                            pipeline = Some(state);
                            Ok(value)
                        }
                        Ok(None) => Err((
                            "replay_select_superseded",
                            "Selection was superseded".into(),
                        )),
                        Err(message) => Err(("replay_select_failed", message)),
                    }
                }
            },
            "vertex_input" => {
                let number = |name: &str, default| match request.get(name) {
                    None => Some(default),
                    Some(value) => value.as_u64().and_then(|v| u32::try_from(v).ok()),
                };
                match (
                    &opened,
                    selected_event,
                    &pipeline,
                    number("instance", 0),
                    number("offset", 0),
                    number("limit", renderdocx::vertex_input::INPUT_PAGE_LIMIT),
                ) {
                    (None, _, _, _, _, _) => Err(("no_capture", "No capture is open".into())),
                    (Some(_), None, _, _, _, _) => {
                        Err(("no_event", "Select a draw action first".into()))
                    }
                    (
                        Some((session, snapshot)),
                        Some(event),
                        Some(pipeline),
                        Some(instance),
                        Some(offset),
                        Some(limit),
                    ) => {
                        if let Some(action) = snapshot
                            .actions
                            .iter()
                            .find(|action| action.event_id == event)
                        {
                            session
                                .vertex_input_page(
                                    action,
                                    pipeline,
                                    &snapshot.buffers,
                                    instance,
                                    offset,
                                    limit,
                                )
                                .map(|page| json!({"page":page}))
                                .map_err(|error| ("replay_vertex_input_failed", error))
                        } else {
                            Err(("no_draw", "Select a draw action first".into()))
                        }
                    }
                    _ => Err((
                        "invalid_arguments",
                        "vertex_input requires valid instance, offset and limit".into(),
                    )),
                }
            }
            "mesh" => {
                mesh_selection = None;
                let stage = match request.get("stage").and_then(Value::as_str) {
                    Some("vs_out") => Some(1),
                    Some("gs_out") => Some(2),
                    Some("task_out") => Some(3),
                    Some("mesh_out") => Some(4),
                    _ => None,
                };
                let number = |name: &str| match request.get(name) {
                    None => Some(0),
                    Some(v) => v.as_u64().and_then(|v| u32::try_from(v).ok()),
                };
                match (
                    &opened,
                    selected_event,
                    stage,
                    number("instance"),
                    number("view"),
                ) {
                    (None, _, _, _, _) => Err(("no_capture", "No capture is open".into())),
                    (Some(_), None, _, _, _) => {
                        Err(("no_event", "Select a draw event first".into()))
                    }
                    (Some((session, _)), Some(event), Some(stage), Some(instance), Some(view)) => {
                        let selection = MeshSelection {
                            instance,
                            view,
                            stage,
                        };
                        session
                            .mesh(event, selection)
                            .map(|mesh| {
                                mesh_selection = Some((event, selection));
                                json!({"mesh":mesh})
                            })
                            .map_err(|error| ("replay_mesh_failed", error))
                    }
                    _ => Err((
                        "invalid_arguments",
                        "mesh requires a valid stage, instance and view".into(),
                    )),
                }
            }
            "mesh_read" => {
                let kind = request
                    .get("kind")
                    .and_then(Value::as_str)
                    .filter(|v| matches!(*v, "vertex" | "index"));
                let offset = request
                    .get("offset")
                    .and_then(Value::as_str)
                    .and_then(|v| v.parse::<u64>().ok());
                let length = request
                    .get("length")
                    .and_then(Value::as_u64)
                    .and_then(|v| u32::try_from(v).ok())
                    .filter(|v| *v <= BUFFER_PAGE_LIMIT);
                match (&opened, mesh_selection, kind, offset, length) {
                    (None, _, _, _, _) => Err(("no_capture", "No capture is open".into())),
                    (Some(_), None, _, _, _) => Err(("no_mesh", "Query mesh data first".into())),
                    (Some((session, _)), Some((event, selection)), Some(kind), Some(offset), Some(length)) if offset.checked_add(u64::from(length)).is_some() =>
                        session.mesh_read(event, selection, kind, offset, length).map(|bytes| {
                            let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
                            json!({"event_id":event,"selection":selection,"kind":kind,"offset":offset.to_string(),"length":bytes.len(),"hex":hex})
                        }).map_err(|error| ("replay_mesh_read_failed", error)),
                    _ => Err(("invalid_arguments", "mesh_read requires kind, decimal-string offset and valid length".into())),
                }
            }
            "read_buffer" => {
                let index = request
                    .get("index")
                    .and_then(Value::as_u64)
                    .and_then(|v| u32::try_from(v).ok());
                let offset = request
                    .get("offset")
                    .and_then(Value::as_str)
                    .and_then(|v| v.parse::<u64>().ok());
                let length = request
                    .get("length")
                    .and_then(Value::as_u64)
                    .and_then(|v| u32::try_from(v).ok());
                match (&opened, selected_event, index, offset, length) {
                    (None, _, _, _, _) => Err(("no_capture", "No capture is open".into())),
                    (Some(_), None, _, _, _) => {
                        Err(("no_event", "Select an event before reading buffers".into()))
                    }
                    (
                        Some((session, snapshot)),
                        Some(event),
                        Some(index),
                        Some(offset),
                        Some(length),
                    ) => {
                        let size = snapshot
                            .buffers
                            .get(index as usize)
                            .and_then(|buffer| buffer.length.parse::<u64>().ok());
                        if length > BUFFER_PAGE_LIMIT
                            || size.is_none_or(|size| {
                                offset > size || u64::from(length) > size - offset
                            })
                        {
                            Err((
                                "invalid_arguments",
                                "Buffer range is out of bounds or exceeds the page limit".into(),
                            ))
                        } else {
                            session.read_buffer(index, offset, length).map(|bytes| {
                                let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
                                json!({"event_id":event,"index":index,"offset":offset.to_string(),"hex":hex})
                            }).map_err(|error| ("replay_read_failed", error))
                        }
                    }
                    _ => Err((
                        "invalid_arguments",
                        "read_buffer requires index, decimal-string offset and length".into(),
                    )),
                }
            }
            "debug_vertex" => {
                let number = |name: &str| {
                    request
                        .get(name)
                        .and_then(Value::as_u64)
                        .and_then(|v| u32::try_from(v).ok())
                };
                let view = if request.get("view").is_none() {
                    Some(0)
                } else {
                    number("view")
                };
                match (&opened, selected_event, number("vertex"), number("instance"), number("index"), view) {
                    (None, _, _, _, _, _) => Err(("no_capture", "No capture is open".into())),
                    (Some(_), None, _, _, _, _) => Err(("no_event", "Select a draw before debugging".into())),
                    (Some((session, _)), Some(event), Some(vertex), Some(instance), Some(index), Some(view)) => {
                        mesh_selection = None;
                        session.debug_vertex(vertex, instance, index, view)
                        .and_then(|trace| if trace.event_id == event { Ok(json!({"trace":trace})) } else { Err("Debug trace belongs to a different event".into()) })
                        .map_err(|error| ("replay_debug_failed", error))
                    },
                    _ => Err(("invalid_arguments", "debug_vertex requires vertex, instance and adjusted index as unsigned integers".into())),
                }
            }
            "debug_pixel" => {
                let number = |name: &str| {
                    request
                        .get(name)
                        .and_then(Value::as_u64)
                        .and_then(|v| u32::try_from(v).ok())
                };
                let optional = |name: &str| match request.get(name) {
                    None | Some(Value::Null) => Some(None),
                    Some(_) => number(name).filter(|v| *v != u32::MAX).map(Some),
                };
                match (&opened, selected_event, number("x"), number("y"), optional("sample"), optional("primitive"), optional("view")) {
                    (None, _, _, _, _, _, _) => Err(("no_capture", "No capture is open".into())),
                    (Some(_), None, _, _, _, _, _) => Err(("no_event", "Select a draw before debugging".into())),
                    (Some((session, _)), Some(event), Some(x), Some(y), Some(sample), Some(primitive), Some(view)) => {
                        mesh_selection = None;
                        session.debug_pixel(PixelDebugSelection { x, y, sample, primitive, view })
                            .and_then(|trace| if trace.event_id == event { Ok(json!({"trace":trace})) } else { Err("Debug trace belongs to a different event".into()) })
                            .map_err(|error| ("replay_debug_failed", error))
                    },
                    _ => Err(("invalid_arguments", "debug_pixel requires x and y and optional sample, primitive and view as unsigned integers; use null for no preference".into())),
                }
            }
            "debug_compute" => {
                let dimensions = |key: &str| {
                    request
                        .get(key)
                        .cloned()
                        .and_then(|v| serde_json::from_value::<[u32; 3]>(v).ok())
                };
                match (
                    &opened,
                    selected_event,
                    dimensions("group"),
                    dimensions("thread"),
                ) {
                    (None, _, _, _) => Err(("no_capture", "No capture is open".into())),
                    (Some(_), None, _, _) => Err((
                        "no_event",
                        "Select a compute dispatch before debugging".into(),
                    )),
                    (Some((session, _)), Some(event), Some(group), Some(thread)) => {
                        mesh_selection = None;
                        session
                            .debug_compute(ComputeDebugSelection { group, thread })
                            .and_then(|trace| {
                                if trace.event_id == event {
                                    Ok(json!({"trace":trace}))
                                } else {
                                    Err("Debug trace belongs to a different event".into())
                                }
                            })
                            .map_err(|error| ("replay_debug_failed", error))
                    }
                    _ => Err((
                        "invalid_arguments",
                        "debug_compute requires group and thread arrays of three unsigned integers"
                            .into(),
                    )),
                }
            }
            "debug_collect" => match (&opened, pending_debug.take()) {
                (None, _) => Err(("no_capture", "No capture is open".into())),
                (Some(_), None) => {
                    Err(("no_debug_trace", "Start a shader debug trace first".into()))
                }
                (Some(_), Some(trace)) if debug_streamed => {
                    pending_debug = Some(trace);
                    Err((
                        "debug_already_streamed",
                        "Collect before reading any debug batches".into(),
                    ))
                }
                (Some((session, _)), Some(trace)) => {
                    let collected = (|| {
                        let mut states = Vec::new();
                        loop {
                            let batch = session.debug_next()?;
                            if batch.event_id != trace.event_id {
                                return Err("Debug states belong to a different event".to_owned());
                            }
                            states.extend(batch.states);
                            if batch.finished {
                                break;
                            }
                        }
                        Ok(AgentDebugTimeline {
                            trace,
                            states,
                            navigator: DebugNavigator::default(),
                        })
                    })();
                    let stopped = session.debug_stop();
                    match (collected, stopped) {
                        (Ok(timeline), Ok(())) => {
                            let value = timeline.response(None);
                            debug_timeline = Some(timeline);
                            Ok(value)
                        }
                        (Err(error), _) | (_, Err(error)) => Err(("replay_debug_failed", error)),
                    }
                }
            },
            "debug_state" => match &debug_timeline {
                Some(timeline) => Ok(timeline.response(None)),
                None => Err((
                    "no_debug_timeline",
                    "Collect a shader debug trace first".into(),
                )),
            },
            "debug_breakpoint" => {
                let breakpoint = request
                    .get("breakpoint")
                    .cloned()
                    .and_then(|value| serde_json::from_value::<Breakpoint>(value).ok());
                let enabled = request.get("enabled").and_then(Value::as_bool);
                match (&mut debug_timeline, breakpoint, enabled) {
                    (None, _, _) => Err((
                        "no_debug_timeline",
                        "Collect a shader debug trace first".into(),
                    )),
                    (Some(timeline), Some(breakpoint), Some(enabled)) => timeline
                        .navigator
                        .set_breakpoint(&timeline.trace, breakpoint, enabled)
                        .map(|()| timeline.response(None))
                        .map_err(|error| ("invalid_arguments", error.into())),
                    _ => Err((
                        "invalid_arguments",
                        "debug_breakpoint requires a breakpoint and enabled boolean".into(),
                    )),
                }
            }
            "debug_navigate" => {
                let direction = match request.get("direction").and_then(Value::as_str) {
                    Some("forward") => Some(true),
                    Some("backward") => Some(false),
                    _ => None,
                };
                let mode = request.get("mode").and_then(Value::as_str);
                let mask = match request.get("event_mask") {
                    None => Some(0),
                    Some(value) => value
                        .as_u64()
                        .and_then(|v| u32::try_from(v).ok())
                        .filter(|v| v & !7 == 0),
                };
                match (&mut debug_timeline, direction, mode, mask) {
                    (None, _, _, _) => Err(("no_debug_timeline", "Collect a shader debug trace first".into())),
                    (Some(timeline), Some(forward), Some(mode @ ("instruction" | "source" | "over" | "out" | "run")), Some(mask))
                        if mode == "run" || mask == 0 => {
                        let reason = match mode {
                            "instruction" => timeline.navigator.step(&timeline.states, forward),
                            "source" => timeline.navigator.step_source(&timeline.trace, &timeline.states, forward),
                            "over" => timeline.navigator.step_source_mode(&timeline.trace, &timeline.states, forward, SourceStep::Over),
                            "out" => timeline.navigator.step_source_mode(&timeline.trace, &timeline.states, forward, SourceStep::Out),
                            _ => timeline.navigator.run(&timeline.trace, &timeline.states, forward, mask),
                        };
                        Ok(timeline.response(Some(reason)))
                    }
                    _ => Err(("invalid_arguments", "debug_navigate requires direction and mode; event_mask 0-7 is only valid for run".into())),
                }
            }
            "debug_next" => match (&opened, selected_event) {
                (None, _) => Err(("no_capture", "No capture is open".into())),
                (Some(_), None) => Err(("no_event", "Select a draw before debugging".into())),
                (Some((session, _)), Some(event)) => session
                    .debug_next()
                    .and_then(|batch| {
                        if batch.event_id == event {
                            Ok(json!({"batch":batch}))
                        } else {
                            Err("Debug states belong to a different event".into())
                        }
                    })
                    .map_err(|error| ("replay_debug_failed", error)),
            },
            "debug_stop" => match &opened {
                None => Err(("no_capture", "No capture is open".into())),
                Some((session, _)) => session
                    .debug_stop()
                    .map(|()| json!({}))
                    .map_err(|error| ("replay_debug_failed", error)),
            },
            "shader" => {
                let stage = request
                    .get("stage")
                    .and_then(Value::as_str)
                    .and_then(|stage| SHADER_STAGES.iter().position(|name| *name == stage));
                let target = match request.get("target_index") {
                    None | Some(Value::Null) => Some(None),
                    Some(value) => value.as_u64().and_then(|v| u32::try_from(v).ok()).map(Some),
                };
                match (&opened, selected_event, stage, target) {
                    (None, _, _, _) => Err(("no_capture", "No capture is open".into())),
                    (Some(_), None, _, _) => Err((
                        "no_event",
                        "Select an event before inspecting shaders".into(),
                    )),
                    (Some((session, _)), Some(event), Some(stage), Some(target)) => session
                        .shader(stage as u32, target)
                        .and_then(|report| {
                            if report.event_id != event {
                                return Err("Shader response belongs to a different event".into());
                            }
                            Ok(json!({"stage_name":SHADER_STAGES[stage],"shader":report}))
                        })
                        .map_err(|error| ("replay_shader_failed", error)),
                    _ => Err((
                        "invalid_arguments",
                        "shader requires a valid stage and optional target_index".into(),
                    )),
                }
            }
            "shader_apply" | "shader_revert" => {
                let stage = request
                    .get("stage")
                    .and_then(Value::as_str)
                    .and_then(|stage| SHADER_STAGES.iter().position(|name| *name == stage));
                let encoding = request
                    .get("encoding")
                    .and_then(Value::as_u64)
                    .and_then(|v| u32::try_from(v).ok());
                let entry = request
                    .get("entry")
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty() && !v.contains('\0'));
                let source = match (request.get("source"), request.get("source_hex")) {
                    (Some(Value::String(text)), None) => Some(text.as_bytes().to_vec()),
                    (None, Some(Value::String(hex))) if hex.len() % 2 == 0 && hex.is_ascii() => hex
                        .as_bytes()
                        .chunks_exact(2)
                        .map(|pair| {
                            let digit = |b: u8| (b as char).to_digit(16);
                            Some((digit(pair[0])? * 16 + digit(pair[1])?) as u8)
                        })
                        .collect::<Option<Vec<_>>>(),
                    _ => None,
                };
                match (&opened, selected_event, stage) {
                    (None, _, _) => Err(("no_capture", "No capture is open".into())),
                    (Some(_), None, _) => {
                        Err(("no_event", "Select an event before editing a shader".into()))
                    }
                    (Some((session, _)), Some(event), Some(stage))
                        if command == "shader_revert" =>
                    {
                        session
                            .shader_edit(event, stage as u32, None)
                            .map(|edit| {
                                pipeline = Some(edit.pipeline.clone());
                                mesh_selection = None;
                                json!({"edit":edit})
                            })
                            .map_err(|error| ("shader_edit_failed", error))
                    }
                    (Some((session, _)), Some(event), Some(stage)) => match (
                        encoding, entry, source,
                    ) {
                        (Some(encoding), Some(entry), Some(source)) if !source.is_empty() => {
                            session
                                .shader_edit(event, stage as u32, Some((encoding, entry, &source)))
                                .map(|edit| {
                                    pipeline = Some(edit.pipeline.clone());
                                    mesh_selection = None;
                                    json!({"edit":edit})
                                })
                                .map_err(|error| ("shader_edit_failed", error))
                        }
                        _ => Err((
                            "invalid_arguments",
                            "shader_apply requires encoding, entry and either source or source_hex"
                                .into(),
                        )),
                    },
                    _ => Err((
                        "invalid_arguments",
                        "shader edit requires a valid stage".into(),
                    )),
                }
            }
            "shader_export" => {
                let stage = request
                    .get("stage")
                    .and_then(Value::as_str)
                    .and_then(|stage| SHADER_STAGES.iter().position(|name| *name == stage));
                let target = match request.get("target_index") {
                    None | Some(Value::Null) => Some(None),
                    Some(value) => value.as_u64().and_then(|v| u32::try_from(v).ok()).map(Some),
                };
                let selection = match request.get("kind").and_then(Value::as_str) {
                    Some("disassembly") => Some(renderdocx::shader_export::ShaderText::Disassembly),
                    Some("source") => request
                        .get("source_index")
                        .and_then(Value::as_u64)
                        .and_then(|v| usize::try_from(v).ok())
                        .map(renderdocx::shader_export::ShaderText::Source),
                    _ => None,
                };
                let path = request
                    .get("path")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty());
                match (&opened, selected_event, stage, target, selection, path) {
                    (None, _, _, _, _, _) => Err(("no_capture", "No capture is open".into())),
                    (Some(_), None, _, _, _, _) => Err((
                        "no_event",
                        "Select an event before exporting a shader".into(),
                    )),
                    (
                        Some((session, _)),
                        Some(event),
                        Some(stage),
                        Some(target),
                        Some(selection),
                        Some(path),
                    ) => {
                        let result = (|| {
                            let report = session.shader(stage as u32, target)?;
                            if report.event_id != event {
                                return Err("Shader response belongs to a different event".into());
                            }
                            let text = renderdocx::shader_export::text(&report, selection)?;
                            renderdocx::shader_export::write_new(
                                PathBuf::from(path).as_path(),
                                text,
                            )?;
                            Ok::<_, String>(
                                json!({"event_id":event,"stage":SHADER_STAGES[stage],"bytes":text.len(),"path":path}),
                            )
                        })();
                        result.map_err(|error| ("shader_export_failed", error))
                    }
                    _ => Err((
                        "invalid_arguments",
                        "shader_export requires stage, kind, path and source_index for source"
                            .into(),
                    )),
                }
            }
            "annotation_read" => {
                let name = request.get("name").and_then(Value::as_str);
                match (&opened, name) {
                    (None, _) => Err(("no_capture", "No capture is open".into())),
                    (Some((session, _)), Some(name @ ("notes" | "bookmarks" | "renames"))) => {
                        session
                            .annotation_read(name)
                            .and_then(|section| {
                                section
                                    .document()
                                    .map(|document| json!({"section":section,"document":document}))
                            })
                            .map_err(|error| ("replay_annotation_failed", error))
                    }
                    _ => Err((
                        "invalid_arguments",
                        "annotation_read requires notes, bookmarks or renames".into(),
                    )),
                }
            }
            "resource_rename" => {
                let index = request
                    .get("index")
                    .and_then(Value::as_u64)
                    .and_then(|n| usize::try_from(n).ok());
                let name = request.get("name").and_then(Value::as_str);
                match (&opened, index, name) {
                    (None, _, _) => Err(("no_capture", "No capture is open".into())),
                    (Some((session, snapshot)), Some(index), Some(name)) => {
                        let result = (|| {
                            let key = snapshot
                                .resources
                                .get(index)
                                .and_then(|r| r.rename_key.as_deref())
                                .ok_or("Resource index or stable ID unavailable")?;
                            let section = session.annotation_read("renames")?;
                            let document = renderdocx::annotations::set_resource_name(
                                &section.document()?,
                                key,
                                name,
                            )?;
                            let section = session.annotation_write(&section, &document)?;
                            Ok::<_, String>(json!({"index":index,"name":name,"section":section}))
                        })();
                        result.map_err(|error| ("replay_annotation_failed", error))
                    }
                    _ => Err((
                        "invalid_arguments",
                        "resource_rename requires index and name".into(),
                    )),
                }
            }
            "annotation_write" => {
                let section = request.get("section").cloned().and_then(|v| {
                    serde_json::from_value::<renderdocx::replay::AnnotationSection>(v).ok()
                });
                let document = request.get("document");
                match (&opened, section, document) {
                    (None, _, _) => Err(("no_capture", "No capture is open".into())),
                    (Some((session, _)), Some(section), Some(document)) if document.is_object() => {
                        session
                            .annotation_write(&section, document)
                            .map(|section| json!({"section":section}))
                            .map_err(|error| ("replay_annotation_failed", error))
                    }
                    _ => Err((
                        "invalid_arguments",
                        "annotation_write requires the original section and a JSON object document"
                            .into(),
                    )),
                }
            }
            "descriptors" => {
                let store = match request.get("store_index") {
                    None | Some(Value::Null) => Some(None),
                    Some(v) => v.as_u64().and_then(|v| usize::try_from(v).ok()).map(Some),
                };
                let offset = match request.get("offset") {
                    None => Some(0),
                    Some(v) => v.as_str().and_then(|v| v.parse::<u64>().ok()),
                };
                let limit = match request.get("limit") {
                    None => Some(renderdocx::replay::DESCRIPTOR_PAGE_LIMIT),
                    Some(v) => v.as_u64().and_then(|v| u32::try_from(v).ok()),
                };
                match (&opened, selected_event, store, offset, limit) {
                    (None, _, _, _, _) => Err(("no_capture", "No capture is open".into())),
                    (Some(_), None, _, _, _) => Err(("no_event", "Select an event before inspecting descriptors".into())),
                    (Some((session, snapshot)), Some(event), Some(store), Some(offset), Some(limit))
                        if (1..=renderdocx::replay::DESCRIPTOR_PAGE_LIMIT).contains(&limit) && store.is_none_or(|index| index < snapshot.descriptor_stores.len()) =>
                            session.descriptors(event, store, offset, limit).map(|page| json!({"page":page})).map_err(|error| ("replay_descriptors_failed", error)),
                    _ => Err(("invalid_arguments", "descriptors requires a valid optional store_index, decimal-string offset and page limit".into())),
                }
            }
            "debug_messages" => {
                let offset = match request.get("offset") {
                    None => Some(0),
                    Some(value) => value.as_str().and_then(|v| v.parse::<usize>().ok()),
                };
                let limit = match request.get("limit") {
                    None => Some(128),
                    Some(value) => value.as_u64().and_then(|v| usize::try_from(v).ok()),
                };
                match (&mut opened, offset, limit) {
                    (None, _, _) => Err(("no_capture", "No capture is open".into())),
                    (Some((session, snapshot)), Some(offset), Some(limit)) if (1..=128).contains(&limit) && offset <= snapshot.debug_messages.len() => {
                        match session.debug_messages() {
                            Ok(messages) => {
                                let added = messages.len();
                                snapshot.debug_messages.extend(messages);
                                let end = offset.saturating_add(limit).min(snapshot.debug_messages.len());
                                Ok(json!({"offset":offset.to_string(),"total":snapshot.debug_messages.len().to_string(),"added":added,"messages":&snapshot.debug_messages[offset..end]}))
                            }
                            Err(error) => Err(("replay_messages_failed", error)),
                        }
                    }
                    _ => Err(("invalid_arguments", "debug_messages requires a valid decimal-string offset and page limit 1–128".into())),
                }
            }
            "clear_debug_messages" => match &mut opened {
                None => Err(("no_capture", "No capture is open".into())),
                Some((_, snapshot)) => {
                    snapshot.debug_messages.clear();
                    Ok(json!({}))
                }
            },
            "counter_fetch" => {
                let ids = request
                    .get("counters")
                    .and_then(Value::as_array)
                    .and_then(|values| {
                        values
                            .iter()
                            .map(|v| v.as_u64().and_then(|v| u32::try_from(v).ok()))
                            .collect::<Option<Vec<_>>>()
                    });
                match (&opened, ids) {
                    (None, _) => Err(("no_capture", "No capture is open".into())),
                    (Some((session, snapshot)), Some(ids))
                        if !ids.is_empty()
                            && ids.iter().all(|id| {
                                snapshot.counters.iter().any(|counter| counter.id == *id)
                            }) =>
                    {
                        session
                            .counter_fetch(&ids)
                            .map(|fetch| json!({"fetch":fetch}))
                            .map_err(|error| ("replay_counter_failed", error))
                    }
                    _ => Err((
                        "invalid_arguments",
                        "counter_fetch requires an array of available counter IDs".into(),
                    )),
                }
            }
            "counter_page" => {
                let generation = request
                    .get("generation")
                    .and_then(Value::as_str)
                    .and_then(|v| v.parse::<u64>().ok());
                let offset = match request.get("offset") {
                    None => Some(0),
                    Some(v) => v.as_str().and_then(|v| v.parse::<u64>().ok()),
                };
                let limit = match request.get("limit") {
                    None => Some(renderdocx::replay::COUNTER_PAGE_LIMIT),
                    Some(v) => v.as_u64().and_then(|v| u32::try_from(v).ok()),
                };
                match (&opened, generation, offset, limit) {
                    (None, _, _, _) => Err(("no_capture", "No capture is open".into())),
                    (Some((session, _)), Some(generation), Some(offset), Some(limit)) if limit > 0 && limit <= renderdocx::replay::COUNTER_PAGE_LIMIT =>
                        session.counter_page(generation, offset, limit).map(|page| json!({"page":page})).map_err(|error| ("replay_counter_failed", error)),
                    _ => Err(("invalid_arguments", "counter_page requires a generation, decimal-string offset and valid page limit".into())),
                }
            }
            "resource_usage" => {
                let index = request
                    .get("index")
                    .and_then(Value::as_u64)
                    .and_then(|v| u32::try_from(v).ok());
                let offset = match request.get("offset") {
                    None => Some(0),
                    Some(value) => value.as_str().and_then(|v| v.parse::<u64>().ok()),
                };
                let limit = match request.get("limit") {
                    None => Some(renderdocx::replay::RESOURCE_USAGE_PAGE_LIMIT),
                    Some(value) => value.as_u64().and_then(|v| u32::try_from(v).ok()),
                };
                match (&opened, index, offset, limit) {
                    (None, _, _, _) => Err(("no_capture", "No capture is open".into())),
                    (Some((session, snapshot)), Some(index), Some(offset), Some(limit))
                        if (index as usize) < snapshot.resources.len() && limit > 0 && limit <= renderdocx::replay::RESOURCE_USAGE_PAGE_LIMIT => {
                        session.resource_usage(index, offset, limit).map(|page| {
                            let entries = page.entries.iter().map(|entry| json!({"event_id":entry.event_id,"usage":entry.usage,"usage_name":renderdocx::replay::resource_usage_name(entry.usage)})).collect::<Vec<_>>();
                            json!({"index":page.index,"offset":page.offset,"total":page.total,"tracked":page.tracked,"entries":entries})
                        }).map_err(|error| ("replay_usage_failed", error))
                    }
                    _ => Err(("invalid_arguments", "resource_usage requires a valid resource index, decimal-string offset and page limit".into())),
                }
            }
            "pixel_history" => {
                let number = |key: &str| {
                    request
                        .get(key)
                        .and_then(Value::as_u64)
                        .and_then(|v| u32::try_from(v).ok())
                };
                let offset = match request.get("offset") {
                    None => Some(0),
                    Some(value) => value.as_str().and_then(|v| v.parse::<u64>().ok()),
                };
                let limit = match request.get("limit") {
                    None => Some(renderdocx::replay::PIXEL_HISTORY_PAGE_LIMIT),
                    Some(value) => value.as_u64().and_then(|v| u32::try_from(v).ok()),
                };
                match (&opened, selected_event, number("index"), number("x"), number("y"), offset, limit) {
                    (None, _, _, _, _, _, _) => Err(("no_capture", "No capture is open".into())),
                    (Some(_), None, _, _, _, _, _) => Err(("no_event", "Select an event before inspecting pixel history".into())),
                    (Some((session, snapshot)), Some(event), Some(index), Some(x), Some(y), Some(offset), Some(limit))
                        if limit > 0 && limit <= renderdocx::replay::PIXEL_HISTORY_PAGE_LIMIT => {
                        let selection = serde_json::from_value::<TextureSelection>(Value::Object(request.clone()));
                        match (snapshot.textures.get(index as usize), selection) {
                            (Some(texture), Ok(selection)) => match selection.validate_pixel(texture, x, y) {
                                Ok(()) => {
                                    mesh_selection = None;
                                    session.pixel_history(event, index, texture, selection, x, y, offset, limit)
                                        .map(|page| json!({"history":page,"selection":selection}))
                                        .map_err(|error| ("replay_pixel_history_failed", error))
                                }
                                Err(error) => Err(("invalid_arguments", error)),
                            },
                            _ => Err(("invalid_arguments", "Invalid texture index or selection".into())),
                        }
                    }
                    _ => Err(("invalid_arguments", "pixel_history requires a texture index, x/y, decimal-string offset and valid page limit".into())),
                }
            }
            "texture_statistics" => {
                let index = request
                    .get("index")
                    .and_then(Value::as_u64)
                    .and_then(|v| u32::try_from(v).ok());
                match (&opened, selected_event, index) {
                    (None, _, _) => Err(("no_capture", "No capture is open".into())),
                    (Some(_), None, _) => Err((
                        "no_event",
                        "Select an event before computing texture statistics".into(),
                    )),
                    (Some((session, snapshot)), Some(event), Some(index)) => {
                        let selection = serde_json::from_value::<TextureSelection>(Value::Object(
                            request.clone(),
                        ));
                        match (snapshot.textures.get(index as usize), selection) {
                            (Some(texture), Ok(selection)) => {
                                let number = |key: &str, default: f32| match request.get(key) {
                                    None => Some(default),
                                    Some(value) => value.as_f64().map(|v| v as f32),
                                };
                                let channels = match request.get("channels") {
                                    None if selection
                                        .channel
                                        .is_some_and(|channel| channel >= 4) =>
                                    {
                                        None
                                    }
                                    None => Some(std::array::from_fn(|i| {
                                        selection.channel.map_or(
                                            i < texture.component_count as usize,
                                            |channel| i == channel as usize,
                                        )
                                    })),
                                    Some(value) => {
                                        serde_json::from_value::<[bool; 4]>(value.clone()).ok()
                                    }
                                };
                                match (number("min", 0.0), number("max", 1.0), channels) {
                                    (Some(min), Some(max), Some(channels)) => {
                                        let histogram = renderdocx::replay::HistogramRequest {
                                            min,
                                            max,
                                            channels,
                                        };
                                        match selection.validate_pixel(texture, 0, 0).and_then(|_| histogram.validate()) {
                                            Ok(()) => session.texture_statistics(event, index, texture, selection, histogram)
                                                .map(|stats| json!({"statistics":stats,"selection":selection}))
                                                .map_err(|error| ("replay_statistics_failed", error)),
                                            Err(error) => Err(("invalid_arguments", error)),
                                        }
                                    }
                                    _ => Err((
                                        "invalid_arguments",
                                        "Invalid histogram range or channels".into(),
                                    )),
                                }
                            }
                            _ => Err((
                                "invalid_arguments",
                                "Invalid texture index or selection".into(),
                            )),
                        }
                    }
                    _ => Err((
                        "invalid_arguments",
                        "texture_statistics requires a texture index".into(),
                    )),
                }
            }
            "pick_pixel" => {
                let number = |key: &str| {
                    request
                        .get(key)
                        .and_then(Value::as_u64)
                        .and_then(|v| u32::try_from(v).ok())
                };
                match (
                    &opened,
                    selected_event,
                    number("index"),
                    number("x"),
                    number("y"),
                ) {
                    (None, _, _, _, _) => Err(("no_capture", "No capture is open".into())),
                    (Some(_), None, _, _, _) => {
                        Err(("no_event", "Select an event before picking a pixel".into()))
                    }
                    (Some((session, snapshot)), Some(event), Some(index), Some(x), Some(y)) => {
                        let selection = serde_json::from_value::<TextureSelection>(Value::Object(
                            request.clone(),
                        ));
                        match (snapshot.textures.get(index as usize), selection) {
                            (Some(texture), Ok(selection)) => {
                                match selection.validate_pixel(texture, x, y) {
                                    Ok(()) => session
                                        .pick_pixel(event, index, texture, selection, x, y)
                                        .map(|pixel| json!({"pixel":pixel,"selection":selection}))
                                        .map_err(|error| ("replay_pixel_failed", error)),
                                    Err(error) => Err(("invalid_arguments", error)),
                                }
                            }
                            _ => Err((
                                "invalid_arguments",
                                "Invalid texture index or selection".into(),
                            )),
                        }
                    }
                    _ => Err((
                        "invalid_arguments",
                        "pick_pixel requires a texture index and unsigned x/y".into(),
                    )),
                }
            }
            "export_texture" => {
                let index = request
                    .get("index")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok());
                let path = request
                    .get("path")
                    .and_then(Value::as_str)
                    .filter(|path| !path.is_empty());
                match (&opened, selected_event, index, path) {
                    (None, _, _, _) => Err(("no_capture", "No capture is open".into())),
                    (Some(_), None, _, _) => {
                        Err(("no_event", "Select an event before exporting".into()))
                    }
                    (Some(_), Some(_), None, _) | (Some(_), Some(_), Some(_), None) => Err((
                        "invalid_arguments",
                        "export_texture requires a texture index and output path".into(),
                    )),
                    (Some((session, snapshot)), Some(event), Some(index), Some(path)) => {
                        let selection = serde_json::from_value::<TextureSelection>(Value::Object(
                            request.clone(),
                        ));
                        match (snapshot.textures.get(index as usize), selection) {
                            (Some(texture), Ok(selection)) => match selection.validate(texture) {
                                Ok(()) => session.export_texture_selected(index, selection, PathBuf::from(path).as_path())
                                    .map(|_| json!({"index":index,"event_id":event,"path":path,"selection":selection}))
                                    .map_err(|error| ("replay_export_failed", error)),
                                Err(error) => Err(("invalid_arguments", error)),
                            },
                            _ => Err(("invalid_arguments", "Invalid texture index or selection".into())),
                        }
                    }
                }
            }
            "close" | "quit" => {
                selected_event = None;
                pipeline = None;
                mesh_selection = None;
                match opened.take() {
                    Some((session, _)) => session
                        .close()
                        .map(|_| json!({}))
                        .map_err(|error| ("replay_close_failed", error)),
                    None => Ok(json!({})),
                }
            }
            _ => Err(("invalid_command", "Unknown or missing command".into())),
        };
        if let Ok(data) = &result {
            match command {
                "debug_vertex" | "debug_pixel" | "debug_compute" => {
                    pending_debug = data
                        .get("trace")
                        .cloned()
                        .and_then(|value| serde_json::from_value(value).ok());
                    debug_streamed = false;
                    debug_timeline = None;
                }
                "debug_next" => debug_streamed = true,
                "select" | "open" | "close" | "quit" | "debug_stop" => {
                    pending_debug = None;
                    debug_timeline = None;
                    debug_streamed = false;
                }
                _ => {}
            }
        } else if matches!(command, "debug_vertex" | "debug_pixel" | "debug_compute")
            && matches!(&result, Err(("replay_debug_failed", _)))
        {
            // The bridge may have replaced an earlier trace before a failed start.
            pending_debug = None;
        }
        let response = match result {
            Ok(data) => json!({"schema":"renderdocx.agent.v1","id":id,"ok":true,"data":data}),
            Err((code, message)) => {
                json!({"schema":"renderdocx.agent.v1","id":id,"ok":false,"error":{"code":code,"message":message}})
            }
        };
        writeln!(output, "{response}")?;
        output.flush()?;
        if command == "quit" {
            break;
        }
    }
    if let Some((session, _)) = opened {
        let _ = session.close();
    }
    if let Some(session) = target {
        let _ = session.close();
    }
    Ok(())
}

fn main() -> ExitCode {
    match run(std::env::args_os().skip(1).collect()) {
        Ok(Invocation::Output(value)) => {
            println!("{value}");
            ExitCode::SUCCESS
        }
        Ok(Invocation::Serve(settings)) => {
            match serve(settings, io::stdin().lock(), io::stdout().lock()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("Agent protocol I/O error: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Err((code, message)) => {
            println!(
                "{}",
                json!({"schema":"renderdocx.cli.v1","ok":false,"error":{"code":code,"message":message}})
            );
            ExitCode::FAILURE
        }
    }
}
