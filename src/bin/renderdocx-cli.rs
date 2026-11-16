use renderdocx::{backend::Settings, replay::Session};
use serde_json::{Value, json};
use std::{ffi::OsString, path::PathBuf, process::ExitCode};

fn usage() -> Value {
    json!({"schema":"renderdocx.cli.v1","ok":true,"commands":[
        {"name":"inspect","arguments":["capture.rdc"],"description":"Open a capture and return its action tree and resource counts."},
        {"name":"select","arguments":["capture.rdc","event_id"],"description":"Replay a capture to an action event."},
        {"name":"capabilities","arguments":[],"description":"Describe the commands currently implemented."}
    ],"global_options":["--bridge PATH"],"limitations":["Additional RenderDoc GUI and agent features are not yet implemented."]})
}

fn run(args: Vec<OsString>) -> Result<Value, (&'static str, String)> {
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
        return Ok(usage());
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
            true => Ok(
                json!({"schema":"renderdocx.cli.v1","ok":true,"command":"select","event_id":event_id}),
            ),
            false => Err((
                "replay_select_superseded",
                "Selection was superseded".into(),
            )),
        }
    } else {
        Ok(json!({"schema":"renderdocx.cli.v1","ok":true,"command":"inspect","capture":snapshot}))
    }
}

fn main() -> ExitCode {
    match run(std::env::args_os().skip(1).collect()) {
        Ok(value) => {
            println!("{value}");
            ExitCode::SUCCESS
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
