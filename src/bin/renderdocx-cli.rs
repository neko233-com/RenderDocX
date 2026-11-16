use renderdocx::{
    backend::Settings,
    replay::{Session, Snapshot},
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
    ],"global_options":["--bridge PATH"],"limitations":["Additional RenderDoc GUI and agent features are not yet implemented."]})
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
            .select(event_id, revision)
            .map_err(|e| ("replay_select_failed", e))?
        {
            true => {
                session.close().map_err(|e| ("replay_close_failed", e))?;
                Ok(Invocation::Output(
                    json!({"schema":"renderdocx.cli.v1","ok":true,"command":"select","event_id":event_id}),
                ))
            }
            false => Err((
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
                    match session.select(event, revision) {
                        Ok(true) => {
                            selected_event = Some(event);
                            Ok(json!({"event_id":event}))
                        }
                        Ok(false) => Err((
                            "replay_select_superseded",
                            "Selection was superseded".into(),
                        )),
                        Err(message) => Err(("replay_select_failed", message)),
                    }
                }
            },
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
                        if index as usize >= snapshot.textures.len() {
                            Err(("invalid_arguments", "Texture index is out of range".into()))
                        } else {
                            session
                                .export_texture(index, PathBuf::from(path).as_path())
                                .map(|_| json!({"index":index,"event_id":event,"path":path}))
                                .map_err(|error| ("replay_export_failed", error))
                        }
                    }
                }
            }
            "close" | "quit" => {
                selected_event = None;
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
