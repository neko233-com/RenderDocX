use renderdocx::{
    backend::Settings,
    replay::{BUFFER_PAGE_LIMIT, Pipeline, SHADER_STAGES, Session, Snapshot, TextureSelection},
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

fn usage() -> Value {
    json!({"schema":"renderdocx.cli.v1","ok":true,"commands":[
        {"name":"inspect","arguments":["capture.rdc"],"description":"Open a capture and return its action tree and resource counts."},
        {"name":"select","arguments":["capture.rdc","event_id"],"description":"Replay a capture to an action event."},
        {"name":"serve","arguments":[],"description":"Serve sequential JSONL requests using a persistent replay session."},
        {"name":"capabilities","arguments":[],"description":"Describe the commands currently implemented."}
    ],"serve_commands":["capabilities","open","snapshot","select","pipeline","shader","read_buffer","export_texture","close","quit"],"shader_stages":SHADER_STAGES,"buffer_page_limit":BUFFER_PAGE_LIMIT,"global_options":["--bridge PATH"],"limitations":["Additional RenderDoc GUI and agent features are not yet implemented."]})
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
