use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

struct Agent {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Agent {
    fn new(bridge: Option<&str>) -> Self {
        Self::with_handoff_environment(bridge, false)
    }

    fn with_handoff_environment(bridge: Option<&str>, handoff: bool) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_renderdocx-cli"));
        if handoff {
            command.env("RENDERDOCX_CONNECT_PID", "12345");
            command.env("RENDERDOCX_OPEN_CAPTURE", "stale.rdc");
        }
        if let Some(bridge) = bridge {
            command.args(["--bridge", bridge]);
        }
        let mut child = command
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            input,
            output,
        }
    }

    fn send(&mut self, request: &str) -> Value {
        writeln!(self.input, "{request}").unwrap();
        self.input.flush().unwrap();
        let mut response = String::new();
        assert!(self.output.read_line(&mut response).unwrap() > 0);
        serde_json::from_str(&response).unwrap()
    }

    fn quit(mut self) {
        assert_eq!(self.send(r#"{"id":"done","command":"quit"}"#)["ok"], true);
        assert!(self.child.wait().unwrap().success());
    }
}

fn invoke(args: &[&str]) -> (bool, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_renderdocx-cli"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    (
        output.status.success(),
        serde_json::from_slice(&output.stdout).expect("stdout must be one JSON document"),
    )
}

#[cfg(windows)]
#[test]
#[ignore = "requires the D3D11 fixture and a matching RenderDoc replay bridge"]
fn real_d3d11_mesh_draw_order() {
    use std::{os::windows::process::CommandExt, path::PathBuf};
    let bridge = PathBuf::from(std::env::var_os("RENDERDOCX_TEST_BRIDGE").unwrap());
    let fixture = std::env::var_os("RENDERDOCX_TEST_FIXTURE").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(fixture)
        .arg(bridge.parent().unwrap().join("renderdoc.dll"))
        .arg(dir.path().join("frame"))
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fixture: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let capture = stdout
        .lines()
        .find_map(|line| line.strip_prefix("RENDERDOCX_CAPTURE="))
        .unwrap();
    let mut agent = Agent::new(bridge.to_str());
    let opened = agent.send(&serde_json::json!({"command":"open","path":capture}).to_string());
    assert_eq!(opened["ok"], true, "{opened}");
    let draw = opened["data"]["capture"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| {
            action["name"]
                .as_str()
                .is_some_and(|name| name.contains("DrawIndexed"))
        })
        .expect("indexed draw")["event_id"]
        .as_u64()
        .unwrap();
    let selected = agent.send(&serde_json::json!({"command":"select","event_id":draw}).to_string());
    assert_eq!(selected["ok"], true, "{selected}");
    let mesh = agent.send(r#"{"command":"mesh","stage":"vs_out"}"#);
    assert_eq!(mesh["ok"], true, "{mesh}");
    let page = agent.send(r#"{"command":"mesh_draw","offset":0,"count":3}"#);
    assert_eq!(page["ok"], true, "{page}");
    assert_eq!(page["data"]["page"]["rows"][0]["vertex"], 0);
    assert_eq!(
        page["data"]["page"]["rows"][0]["position"],
        serde_json::json!([3.0, 3.0, 0.0, 1.0])
    );
    assert_eq!(page["data"]["page"]["rows"][1]["vertex"], 1);
    let export_path = dir.path().join("positions.csv");
    let request = serde_json::json!({"command":"mesh_draw_export","path":export_path});
    let exported = agent.send(&request.to_string());
    assert_eq!(exported["ok"], true, "{exported}");
    assert_eq!(exported["data"]["rows"], 3);
    assert!(
        std::fs::read_to_string(&export_path)
            .unwrap()
            .contains("0,0,0,false,3,3,0,1,")
    );
    assert_eq!(
        agent.send(&request.to_string())["error"]["code"],
        "mesh_export_failed"
    );
    let scissor_draw = opened["data"]["capture"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|action| {
            action["name"]
                .as_str()
                .is_some_and(|name| name.contains("DrawIndexed"))
        })
        .last()
        .unwrap()["event_id"]
        .as_u64()
        .unwrap();
    let scissored =
        agent.send(&serde_json::json!({"command":"select","event_id":scissor_draw}).to_string());
    assert_eq!(scissored["ok"], true, "{scissored}");
    let scissor = &scissored["data"]["pipeline"]["scissors"][0];
    assert_eq!(scissor["enabled"], true, "{scissored}");
    assert_eq!(
        (
            scissor["x"].as_i64(),
            scissor["y"].as_i64(),
            scissor["width"].as_i64(),
            scissor["height"].as_i64()
        ),
        (Some(0), Some(0), Some(1), Some(1))
    );
    let fixed = &scissored["data"]["pipeline"]["fixed_state"];
    assert_eq!(fixed["raster"]["fill"], 0, "{fixed}");
    assert_eq!(fixed["raster"]["cull"], 0, "{fixed}");
    assert!(fixed["depth"]["enabled"].is_boolean(), "{fixed}");
    assert!(fixed["stencil_front"]["reference"].is_u64(), "{fixed}");
    assert_eq!(fixed["blend_factor"].as_array().unwrap().len(), 4);
    assert!(!fixed["blends"].as_array().unwrap().is_empty(), "{fixed}");
    let pipeline = &scissored["data"]["pipeline"];
    let color = &pipeline["color_attachments"][0];
    assert_eq!(color["resource"]["bound"], true, "{pipeline}");
    assert_eq!(color["first_mip"], 0);
    assert_eq!(color["first_slice"], 0);
    assert_eq!(color["swizzle"].as_array().unwrap().len(), 4);
    assert_eq!(pipeline["depth_attachment"]["resource"]["bound"], false);
    let texture_index = opened["data"]["capture"]["textures"]
        .as_array()
        .unwrap()
        .iter()
        .position(|texture| texture["width"] == 16 && texture["height"] == 16)
        .unwrap();
    let png_path = dir.path().join("typed.png");
    let png = agent.send(&serde_json::json!({"command":"export_texture","index":texture_index,"path":png_path,"preview_type_cast":2}).to_string());
    assert_eq!(png["ok"], true, "{png}");
    assert_eq!(png["data"]["selection"]["preview_type_cast"], 2);
    assert!(
        std::fs::read(png_path)
            .unwrap()
            .starts_with(b"\x89PNG\r\n\x1a\n")
    );
    for command in ["pick_pixel", "texture_statistics", "pixel_history"] {
        let result = agent.send(&serde_json::json!({"command":command,"index":texture_index,"x":0,"y":0,"analysis_type_cast":2}).to_string());
        assert_eq!(result["ok"], true, "{command}: {result}");
        assert_eq!(result["data"]["selection"]["analysis_type_cast"], 2);
    }
    let dispatch = opened["data"]["capture"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| {
            action["name"]
                .as_str()
                .is_some_and(|name| name.contains("Dispatch"))
        })
        .expect("compute dispatch")["event_id"]
        .as_u64()
        .unwrap();
    let selected =
        agent.send(&serde_json::json!({"command":"select","event_id":dispatch}).to_string());
    assert_eq!(selected["ok"], true, "{selected}");
    let shader = agent.send(r#"{"command":"shader","stage":"compute"}"#);
    assert_eq!(shader["ok"], true, "{shader}");
    let resources = shader["data"]["shader"]["read_write_details"]
        .as_array()
        .unwrap();
    assert!(
        resources
            .iter()
            .any(|resource| resource["name"] == "output" && resource["bind"] == 0),
        "{shader}"
    );
    let blocks = shader["data"]["shader"]["constant_block_details"]
        .as_array()
        .unwrap();
    let block = blocks
        .iter()
        .position(|block| block["name"] == "Params")
        .expect("constant block");
    let constants = agent.send(
        &serde_json::json!({"command":"shader_constants","stage":"compute","block":block})
            .to_string(),
    );
    assert_eq!(constants["ok"], true, "{constants}");
    let seed = constants["data"]["constants"]["variables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["name"] == "seed")
        .unwrap();
    assert!(
        seed["value_hex"].as_str().unwrap().starts_with("2a000000"),
        "{seed}"
    );
    let invalid = agent.send(&serde_json::json!({"command":"shader_constants","stage":"compute","block":block,"element":1}).to_string());
    assert_eq!(invalid["error"]["code"], "replay_shader_constants_failed");
    let bindings = agent
        .send(r#"{"command":"shader_bindings","stage":"compute","kind":"read_write","limit":1}"#);
    assert_eq!(bindings["ok"], true, "{bindings}");
    let entry = &bindings["data"]["page"]["entries"][0];
    assert_eq!(entry["binding"], 0);
    assert_eq!(entry["descriptor"]["resource_present"], true);
    assert!(entry["descriptor"]["resource"].is_u64());
    assert!(entry["descriptor"]["format_component_type"].is_u64());
    let constants =
        agent.send(r#"{"command":"shader_bindings","stage":"compute","kind":"constants"}"#);
    assert_eq!(constants["ok"], true, "{constants}");
    assert_eq!(
        constants["data"]["page"]["entries"][0]["descriptor"]["resource_present"],
        true
    );
    let bytes = agent.send(r#"{"command":"shader_binding_read","stage":"compute","kind":"constants","row":"0","offset":"0","length":4}"#);
    assert_eq!(bytes["ok"], true, "{bytes}");
    assert_eq!(bytes["data"]["hex"], "2a000000");
    let clipped = agent.send(r#"{"command":"shader_binding_read","stage":"compute","kind":"constants","row":"0","offset":"15","length":8}"#);
    assert_eq!(clipped["ok"], true, "{clipped}");
    assert_eq!(clipped["data"]["length"], 1);
    agent.quit();
}

#[cfg(windows)]
#[test]
#[ignore = "requires RENDERDOCX_TEST_BRIDGE and RENDERDOCX_TEST_OPENGL_FIXTURE plus an OpenGL 3.3 driver"]
fn real_opengl_vertex_inputs_include_generic_and_normalized_values() {
    use std::os::windows::process::CommandExt;
    let bridge = std::path::PathBuf::from(std::env::var_os("RENDERDOCX_TEST_BRIDGE").unwrap());
    let fixture = std::env::var_os("RENDERDOCX_TEST_OPENGL_FIXTURE").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(fixture)
        .arg(bridge.parent().unwrap().join("renderdoc.dll"))
        .arg(dir.path().join("frame"))
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fixture: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let capture = stdout
        .lines()
        .find_map(|line| line.strip_prefix("RENDERDOCX_CAPTURE="))
        .unwrap();
    let mut agent = Agent::new(bridge.to_str());
    let opened = agent.send(&serde_json::json!({"command":"open","path":capture}).to_string());
    assert_eq!(opened["ok"], true, "{opened}");
    assert_eq!(opened["data"]["capture"]["api"], 2);
    let draw = opened["data"]["capture"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| {
            action["name"]
                .as_str()
                .is_some_and(|name| name.contains("glDrawArrays"))
        })
        .expect("OpenGL draw")["event_id"]
        .as_u64()
        .unwrap();
    let vertex_resource = opened["data"]["capture"]["buffers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|buffer| buffer["length"] == "12")
        .expect("OpenGL vertex buffer")["resource_index"]
        .as_u64()
        .unwrap();
    let usage = agent
        .send(&serde_json::json!({"command":"resource_usage","index":vertex_resource}).to_string());
    assert_eq!(usage["ok"], true, "{usage}");
    assert!(
        usage["data"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["event_id"] == draw && entry["usage_name"] == "VertexBuffer"),
        "{usage}"
    );
    let selected = agent.send(&serde_json::json!({"command":"select","event_id":draw}).to_string());
    assert_eq!(selected["ok"], true, "{selected}");
    let attributes = selected["data"]["pipeline"]["vertex_inputs"]
        .as_array()
        .unwrap();
    assert_eq!(attributes.len(), 4, "{attributes:?}");
    let page = agent.send(r#"{"command":"vertex_input","limit":3}"#);
    assert_eq!(page["ok"], true, "{page}");
    let row = &page["data"]["page"]["rows"][0];
    let attributes = row["attributes"].as_array().unwrap();
    let position = attributes
        .iter()
        .find(|v| v["name"] == "position")
        .expect("position");
    assert_eq!(position["hex"], "00400080");
    let values = position["values"].as_array().unwrap();
    assert!((values[0].as_str().unwrap().parse::<f32>().unwrap() - 16384.0 / 32767.0).abs() < 1e-7);
    assert_eq!(values[1], "-1");
    let tint = attributes
        .iter()
        .find(|v| v["name"] == "tint")
        .expect("tint");
    assert_eq!(
        tint["values"],
        serde_json::json!(["0.25", "0.5", "0.75", "1"])
    );
    assert!(tint["hex"].is_null());
    let flags = attributes
        .iter()
        .find(|v| v["name"] == "flags")
        .expect("flags");
    assert_eq!(flags["values"], serde_json::json!(["-4", "42"]));
    let counters = attributes
        .iter()
        .find(|v| v["name"] == "counters")
        .expect("counters");
    assert_eq!(counters["values"], serde_json::json!(["9", "123"]));
    let texture = opened["data"]["capture"]["textures"]
        .as_array()
        .unwrap()
        .iter()
        .position(|texture| texture["width"] == 2 && texture["height"] == 2)
        .expect("OpenGL fixture texture");
    let top = agent
        .send(&serde_json::json!({"command":"pick_pixel","index":texture,"x":0,"y":0}).to_string());
    assert_eq!(top["ok"], true, "{top}");
    assert_eq!(
        top["data"]["pixel"]["float_values"],
        serde_json::json!(["0", "0", "1", "1"])
    );
    let bottom = agent
        .send(&serde_json::json!({"command":"pick_pixel","index":texture,"x":0,"y":1}).to_string());
    assert_eq!(
        bottom["data"]["pixel"]["float_values"],
        serde_json::json!(["1", "0", "0", "1"]),
        "{bottom}"
    );
    let stats = agent.send(&serde_json::json!({"command":"texture_statistics","index":texture,"min":0,"max":1,"channels":[true,false,false,false]}).to_string());
    assert_eq!(stats["ok"], true, "{stats}");
    let histogram = stats["data"]["statistics"]["histogram"].as_array().unwrap();
    assert_eq!(histogram.len(), 256);
    assert_eq!(histogram[0], 2);
    assert_eq!(histogram[255], 2);
    let target = opened["data"]["capture"]["textures"]
        .as_array()
        .unwrap()
        .iter()
        .position(|texture| texture["width"] == 16 && texture["height"] == 16)
        .expect("OpenGL render target");
    let history = agent.send(
        &serde_json::json!({"command":"pixel_history","index":target,"x":12,"y":8}).to_string(),
    );
    assert_eq!(history["ok"], true, "{history}");
    let fragment = history["data"]["history"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["event_id"] == draw && entry["passed"] == true)
        .unwrap_or_else(|| panic!("OpenGL draw fragment: {history}"));
    let after = agent
        .send(&serde_json::json!({"command":"pick_pixel","index":target,"x":12,"y":8}).to_string());
    assert_eq!(after["ok"], true, "{after}");
    assert_eq!(
        after["data"]["pixel"]["bits"],
        fragment["post"]["color"]["bits"]
    );
    agent.quit();
}

#[test]
fn capabilities_advertises_versioned_machine_readable_contract() {
    let (success, value) = invoke(&["capabilities"]);
    assert!(success);
    assert_eq!(value["schema"], "renderdocx.cli.v1");
    assert_eq!(value["ok"], true);
    assert_eq!(value["capture_options_default"]["allow_vsync"], true);
    assert_eq!(value["capture_options_default"]["debug_output_mute"], true);
    assert_eq!(value["capture_options_default"]["soft_memory_limit"], 0);
    assert_eq!(value["structure_page_limit"], 128);
    assert_eq!(value["vertex_input_page_limit"], 64);
    assert!(
        value["serve_commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "texture_statistics")
    );
    assert_eq!(value["resource_usage_page_limit"], 128);
    assert_eq!(value["pixel_history_page_limit"], 128);
    assert!(
        value["serve_commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "pixel_history")
    );
    assert!(
        value["serve_commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "resource_usage")
    );
    assert!(
        value["serve_commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "pick_pixel")
    );
    assert!(
        value["serve_commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "vertex_input")
    );
    assert!(
        value["serve_commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "structure")
    );
    assert!(
        value["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|command| command["name"] == "inspect")
    );
}

#[test]
fn malformed_arguments_and_missing_bridge_return_structured_errors() {
    let (success, value) = invoke(&["select", "capture.rdc", "-1"]);
    assert!(!success);
    assert_eq!(value["error"]["code"], "invalid_arguments");
    let (success, value) = invoke(&[
        "--bridge",
        "definitely-missing-renderdocx-bridge",
        "inspect",
        "capture.rdc",
    ]);
    assert!(!success);
    assert_eq!(value["error"]["code"], "replay_open_failed");
}

#[test]
fn agent_jsonl_session_has_correlated_errors_and_clean_shutdown() {
    let mut agent = Agent::new(None);
    assert_eq!(agent.send("not json")["error"]["code"], "invalid_request");
    let reply = agent.send(r#"{"id":17,"command":"snapshot"}"#);
    assert_eq!(reply["schema"], "renderdocx.agent.v1");
    assert_eq!(reply["id"], 17);
    assert_eq!(reply["error"]["code"], "no_capture");
    for command in [
        "debug_vertex",
        "debug_pixel",
        "debug_compute",
        "debug_next",
        "debug_stop",
        "debug_messages",
        "clear_debug_messages",
        "descriptors",
        "annotation_read",
        "annotation_write",
    ] {
        assert_eq!(
            agent.send(&serde_json::json!({"command":command}).to_string())["error"]["code"],
            "no_capture"
        );
    }
    assert_eq!(
        agent.send(r#"{"command":"pixel_history","index":0,"x":0,"y":0}"#)["error"]["code"],
        "no_capture"
    );
    assert_eq!(
        agent.send(r#"{"command":"texture_statistics","index":0}"#)["error"]["code"],
        "no_capture"
    );
    assert_eq!(
        agent.send(r#"{"command":"resource_usage","index":0}"#)["error"]["code"],
        "no_capture"
    );
    assert_eq!(
        agent.send(r#"{"command":"pick_pixel","index":0,"x":0,"y":0}"#)["error"]["code"],
        "no_capture"
    );
    assert_eq!(
        agent.send(r#"{"command":"target_events"}"#)["error"]["code"],
        "no_target"
    );
    assert_eq!(
        agent.send(r#"{"command":"target_connect","ident":0}"#)["error"]["code"],
        "invalid_arguments"
    );
    assert_eq!(
        agent.send(r#"{"command":"target_connect_pid","pid":0}"#)["error"]["code"],
        "invalid_arguments"
    );
    assert_eq!(
        agent.send(r#"{"command":"mesh","stage":"vs_out"}"#)["error"]["code"],
        "no_capture"
    );
    assert_eq!(agent.send(r#"{"id":"c","command":"close"}"#)["ok"], true);
    agent.quit();
}

#[cfg(windows)]
#[test]
#[ignore = "requires a built live D3D11 fixture and matching RenderDoc target helper"]
fn native_target_launch_triggers_capture_and_disconnects_without_stopping_target() {
    use std::{
        path::PathBuf,
        time::{Duration, Instant},
    };
    struct Stop(PathBuf);
    impl Drop for Stop {
        fn drop(&mut self) {
            let _ = std::fs::write(&self.0, b"stop");
            let done = PathBuf::from(format!("{}.done", self.0.display()));
            let deadline = Instant::now() + Duration::from_secs(5);
            while !done.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
    let bridge = PathBuf::from(
        std::env::var_os("RENDERDOCX_TEST_BRIDGE").expect("set RENDERDOCX_TEST_BRIDGE"),
    );
    let fixture = PathBuf::from(
        std::env::var_os("RENDERDOCX_TEST_LIVE_FIXTURE").expect("set RENDERDOCX_TEST_LIVE_FIXTURE"),
    );
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/renderdocx-fixtures");
    std::fs::create_dir_all(&root).unwrap();
    let dir = tempfile::Builder::new()
        .prefix("live-捕获 空格-")
        .tempdir_in(&root)
        .unwrap();
    let stop = dir.path().join("stop.flag");
    let existing = dir.path().join("capture_frame1.rdc");
    std::fs::write(&existing, b"existing capture").unwrap();
    let _stop_guard = Stop(stop.clone());
    let template = dir.path();
    let arguments = format!("\"{}\"", stop.display());
    let mut agent = Agent::with_handoff_environment(Some(bridge.to_str().unwrap()), true);
    let invalid = serde_json::json!({"command":"target_launch","path":fixture,"capture_directory":template,
        "configuration":{"environment":[{"name":"BAD=NAME","value":"fixture-private-value","operation":"set","separator":"no_sep"}]}});
    let rejected = agent.send(&invalid.to_string());
    assert_eq!(rejected["error"]["code"], "invalid_arguments", "{rejected}");
    assert!(!rejected.to_string().contains("fixture-private-value"));
    let request = serde_json::json!({"command":"target_launch","path":fixture,"capture_directory":template,
    "configuration":{"arguments":arguments,"working_directory":dir.path(),
        "options":{"allow_vsync":false,"allow_fullscreen":false,"capture_callstacks":true,"capture_callstacks_only_actions":true,"ref_all_resources":true,"debug_output_mute":false,"soft_memory_limit":64},
        "environment":[
                {"name":"RENDERDOCX_FIXTURE_MARKER","value":"中间","operation":"set","separator":"no_sep"},
                {"name":"RENDERDOCX_FIXTURE_MARKER","value":"开始","operation":"prepend","separator":"colon"},
                {"name":"RENDERDOCX_FIXTURE_MARKER","value":"结束","operation":"append","separator":"colon"}
        ]}});
    let launched = agent.send(&request.to_string());
    assert_eq!(launched["ok"], true, "{launched}");
    let ident = launched["data"]["target"]["ident"].as_u64().unwrap();
    assert!(ident > 0);
    let pid = launched["data"]["target"]["pid"].as_u64().unwrap();
    assert!(pid > 0);
    let info_path = PathBuf::from(format!("{}.info", stop.display()));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !info_path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
    let info = std::fs::read_to_string(&info_path).expect("fixture launch configuration");
    assert_eq!(info, "1 1 0 0 1 1 1 0 64 1");
    let discovered = agent.send(r#"{"command":"target_list"}"#);
    assert_eq!(discovered["ok"], true, "{discovered}");
    assert!(
        discovered["data"]["idents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id.as_u64() == Some(ident)),
        "{discovered}"
    );
    let remote_discovered = agent.send(r#"{"command":"target_list","url":"127.0.0.1"}"#);
    assert_eq!(remote_discovered["ok"], true, "{remote_discovered}");
    assert!(
        remote_discovered["data"]["idents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id.as_u64() == Some(ident)),
        "{remote_discovered}"
    );
    let mut other = Agent::new(Some(bridge.to_str().unwrap()));
    let busy =
        other.send(&serde_json::json!({"command":"target_connect","ident":ident}).to_string());
    assert_eq!(busy["error"]["code"], "target_connect_failed", "{busy}");
    let busy_pid =
        other.send(&serde_json::json!({"command":"target_connect_pid","pid":pid}).to_string());
    assert_eq!(
        busy_pid["error"]["code"], "target_connect_failed",
        "{busy_pid}"
    );
    other.quit();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut ready = false;
    while Instant::now() < deadline && !ready {
        let reply = agent.send(r#"{"command":"target_events"}"#);
        assert_eq!(reply["data"]["connected"], true, "{reply}");
        ready = reply["data"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| {
                event["type"] == "api" && event["supported"] == true && event["presenting"] == true
            });
        if !ready {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    assert!(ready, "target did not report a supported presenting API");
    assert_eq!(
        agent.send(r#"{"command":"target_trigger","frames":0}"#)["error"]["code"],
        "invalid_arguments"
    );
    assert_eq!(
        agent.send(r#"{"command":"target_trigger","frames":1}"#)["data"]["submitted"],
        true
    );
    let mut captured = None;
    while Instant::now() < deadline && captured.is_none() {
        let reply = agent.send(r#"{"command":"target_events"}"#);
        assert_eq!(reply["data"]["connected"], true, "{reply}");
        captured = reply["data"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|event| event["type"] == "capture")
            .cloned();
        if captured.is_none() {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let capture = captured.expect("target must report a completed capture");
    let capture_id = capture["capture_id"].as_u64().unwrap();
    let copy_request = serde_json::json!({"command":"target_copy","capture_id":capture_id,"capture_directory":dir.path()});
    let copy = agent.send(&copy_request.to_string());
    assert_eq!(copy["ok"], true, "{copy}");
    let copy_path = PathBuf::from(copy["data"]["path"].as_str().unwrap());
    assert!(copy_path.starts_with(dir.path()));
    assert_eq!(
        agent.send(&copy_request.to_string())["error"]["code"],
        "target_copy_failed"
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut copied = false;
    while Instant::now() < deadline && !copied {
        let reply = agent.send(r#"{"command":"target_events"}"#);
        assert_eq!(reply["data"]["connected"], true, "{reply}");
        copied = reply["data"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["type"] == "copied" && event["capture_id"] == capture_id);
        if !copied {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    assert!(copied, "capture copy completion was not reported");
    assert_eq!(
        std::fs::read(&copy_path).unwrap(),
        std::fs::read(capture["path"].as_str().unwrap()).unwrap()
    );
    assert_eq!(
        agent.send(r#"{"command":"target_cycle"}"#)["data"]["submitted"],
        true
    );
    let queued_frame = capture["frame"].as_u64().unwrap() + 300;
    let queued = agent.send(
        &serde_json::json!({"command":"target_queue","frame":queued_frame,"frames":1}).to_string(),
    );
    assert_eq!(queued["data"]["submitted"], true, "{queued}");
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut queued_capture = None;
    while Instant::now() < deadline && queued_capture.is_none() {
        let reply = agent.send(r#"{"command":"target_events"}"#);
        assert_eq!(reply["data"]["connected"], true, "{reply}");
        queued_capture = reply["data"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|event| {
                event["type"] == "capture" && event["frame"].as_u64() == Some(queued_frame)
            })
            .cloned();
        if queued_capture.is_none() {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    assert!(
        queued_capture.is_some(),
        "queued frame {queued_frame} was not captured"
    );
    assert_eq!(capture["local"], true, "{capture}");
    let path = PathBuf::from(capture["path"].as_str().unwrap());
    assert!(path.is_file(), "capture path: {capture}");
    assert!(
        path.canonicalize()
            .unwrap()
            .starts_with(dir.path().canonicalize().unwrap())
    );
    assert_ne!(path.parent().unwrap(), dir.path());
    assert_eq!(std::fs::read(&existing).unwrap(), b"existing capture");
    let (success, inspected) = invoke(&[
        "--bridge",
        bridge.to_str().unwrap(),
        "inspect",
        path.to_str().unwrap(),
    ]);
    assert!(success, "{inspected}");
    assert_eq!(agent.send(r#"{"command":"target_close"}"#)["ok"], true);
    assert!(
        !PathBuf::from(format!("{}.done", stop.display())).exists(),
        "closing control must not stop the target"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let reconnected =
            agent.send(&serde_json::json!({"command":"target_connect_pid","pid":pid}).to_string());
        if reconnected["ok"] == true {
            assert_eq!(reconnected["data"]["target"]["pid"], pid);
            assert_eq!(reconnected["data"]["target"]["ident"], ident);
            break;
        }
        assert!(Instant::now() < deadline, "{reconnected}");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(agent.send(r#"{"command":"target_close"}"#)["ok"], true);
    let remote = agent.send(
        &serde_json::json!({"command":"target_connect","url":"127.0.0.1","ident":ident})
            .to_string(),
    );
    assert_eq!(remote["ok"], true, "{remote}");
    assert_eq!(remote["data"]["target"]["ident"], ident);
    let remote_copy = agent.send(&copy_request.to_string());
    assert_eq!(remote_copy["ok"], true, "{remote_copy}");
    let remote_path = PathBuf::from(remote_copy["data"]["path"].as_str().unwrap());
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut completed = false;
    while Instant::now() < deadline && !completed {
        let response = agent.send(r#"{"command":"target_events"}"#);
        completed = response["data"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["type"] == "copied" && event["capture_id"] == capture_id);
        if !completed {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    assert!(completed, "loopback target copy did not complete");
    assert_eq!(
        std::fs::read(remote_path).unwrap(),
        std::fs::read(&copy_path).unwrap()
    );
    assert_eq!(agent.send(r#"{"command":"target_close"}"#)["ok"], true);
    agent.quit();
}

#[test]
#[ignore = "requires RENDERDOCX_TEST_BRIDGE pointing to a matching built RenderDoc core"]
fn real_core_reports_missing_capture_as_an_error() {
    let bridge = std::env::var("RENDERDOCX_TEST_BRIDGE").expect("set RENDERDOCX_TEST_BRIDGE");
    let (success, value) = invoke(&[
        "--bridge",
        &bridge,
        "inspect",
        "missing-renderdocx-capture.rdc",
    ]);
    assert!(!success);
    assert_eq!(value["error"]["code"], "replay_open_failed");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("missing-renderdocx-capture.rdc")
    );
}

#[test]
#[ignore = "requires RENDERDOCX_TEST_MISMATCH_BRIDGE built against a deliberately different commit"]
fn incompatible_core_is_rejected_before_capture_open() {
    let bridge = std::env::var("RENDERDOCX_TEST_MISMATCH_BRIDGE")
        .expect("set RENDERDOCX_TEST_MISMATCH_BRIDGE");
    let (success, value) = invoke(&[
        "--bridge",
        &bridge,
        "inspect",
        "missing-renderdocx-capture.rdc",
    ]);
    assert!(!success);
    assert_eq!(value["error"]["code"], "replay_open_failed");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("does not match")
    );
}

#[cfg(windows)]
#[test]
#[ignore = "requires built D3D11 and shader processor fixtures and matching RenderDoc replay bridge"]
fn real_d3d11_capture_is_inspected_and_selected() {
    use std::{os::windows::process::CommandExt, path::PathBuf};
    let bridge = PathBuf::from(
        std::env::var_os("RENDERDOCX_TEST_BRIDGE").expect("set RENDERDOCX_TEST_BRIDGE"),
    );
    let fixture = PathBuf::from(
        std::env::var_os("RENDERDOCX_TEST_FIXTURE").expect("set RENDERDOCX_TEST_FIXTURE"),
    );
    let core = bridge.parent().unwrap().join("renderdoc.dll");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/renderdocx-fixtures");
    std::fs::create_dir_all(&root).unwrap();
    let dir = tempfile::Builder::new()
        .prefix("capture-图像 空格-")
        .tempdir_in(&root)
        .unwrap();
    assert!(
        dir.path()
            .canonicalize()
            .unwrap()
            .starts_with(root.canonicalize().unwrap())
    );
    let output = Command::new(fixture)
        .arg(core)
        .arg(dir.path().join("frame"))
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fixture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("fixture output must be UTF-8");
    let msaa_samples: u32 = stdout
        .lines()
        .find_map(|line| line.strip_prefix("RENDERDOCX_MSAA_SAMPLES="))
        .expect("MSAA support report")
        .parse()
        .unwrap();
    let capture = stdout
        .lines()
        .find_map(|line| line.strip_prefix("RENDERDOCX_CAPTURE="))
        .expect("fixture must report its capture path");
    let capture_path = PathBuf::from(capture)
        .canonicalize()
        .expect("capture must exist");
    assert!(capture_path.starts_with(dir.path().canonicalize().unwrap()));
    let bridge = bridge.to_str().unwrap();
    let capture = capture_path.to_str().unwrap();
    let (success, result) = invoke(&["--bridge", bridge, "inspect", capture]);
    assert!(success, "inspect failed: {result}");
    assert_eq!(result["capture"]["api"], 0);
    assert!(result["capture"]["texture_count"].as_u64().unwrap() > 0);
    assert!(
        result["capture"]["textures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|texture| texture["width"] == 16 && texture["height"] == 16)
    );
    let clear = result["capture"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| {
            action["name"]
                .as_str()
                .is_some_and(|name| name.contains("ClearRenderTargetView"))
        })
        .expect("capture should contain the D3D11 clear action");
    let event = clear["event_id"].as_u64().unwrap().to_string();
    let draw_event = result["capture"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| {
            action["name"]
                .as_str()
                .is_some_and(|name| name.contains("Draw"))
        })
        .unwrap_or_else(|| {
            panic!(
                "capture should contain D3D11 draw: {}",
                result["capture"]["actions"]
            )
        })["event_id"]
        .as_u64()
        .unwrap();
    let history_draw_event = result["capture"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|action| {
            action["name"]
                .as_str()
                .is_some_and(|name| name.contains("DrawIndexed"))
        })
        .nth(1)
        .expect("history draw")["event_id"]
        .as_u64()
        .unwrap();
    let scissor_draw_event = result["capture"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|action| {
            action["name"]
                .as_str()
                .is_some_and(|name| name.contains("DrawIndexed"))
        })
        .nth(2)
        .expect("scissored draw")["event_id"]
        .as_u64()
        .unwrap();
    let api_event = result["capture"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|action| {
            let action_event = action["event_id"].as_u64().unwrap();
            action["api_events"]
                .as_array()
                .unwrap()
                .iter()
                .filter(move |event| event["event_id"].as_u64() != Some(action_event))
        })
        .find(|event| {
            event["name"]
                .as_str()
                .is_some_and(|name| name.contains("IASetPrimitiveTopology"))
        })
        .expect("capture must expose its non-action D3D11 topology API event");
    assert!(api_event["chunk_index"].as_u64().is_some(), "{api_event}");
    assert!(
        api_event["file_offset"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .is_ok()
    );
    let api_event_id = api_event["event_id"].as_u64().unwrap();
    let shader_api_id = result["capture"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|action| action["api_events"].as_array().unwrap())
        .find(|event| {
            event["name"]
                .as_str()
                .is_some_and(|name| name.contains("PSSetShader"))
        })
        .expect("pixel shader API event")["event_id"]
        .as_u64()
        .unwrap();
    let update_api_id = result["capture"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|action| action["api_events"].as_array().unwrap())
        .find(|event| {
            event["name"]
                .as_str()
                .is_some_and(|name| name.contains("UpdateSubresource"))
        })
        .expect("buffer update API event")["event_id"]
        .as_u64()
        .unwrap();
    let (success, api_selected) = invoke(&[
        "--bridge",
        bridge,
        "select",
        capture,
        &api_event_id.to_string(),
    ]);
    assert!(success, "API event replay failed: {api_selected}");
    assert_eq!(api_selected["event_id"], api_event_id);
    let (success, result) = invoke(&["--bridge", bridge, "select", capture, &event]);
    assert!(success, "event replay failed: {result}");
    assert_eq!(result["event_id"].as_u64().unwrap().to_string(), event);

    // Annotation writes are exercised on a copy, never on the fixture capture.
    let annotation_copy = dir.path().join("annotations-copy.rdc");
    std::fs::copy(capture, &annotation_copy).unwrap();
    let mut annotation_agent = Agent::new(Some(bridge));
    let opened_copy = annotation_agent
        .send(&serde_json::json!({"command":"open","path":annotation_copy}).to_string());
    assert_eq!(opened_copy["ok"], true, "{opened_copy}");
    let resources = opened_copy["data"]["capture"]["resources"]
        .as_array()
        .unwrap();
    for texture in opened_copy["data"]["capture"]["textures"]
        .as_array()
        .unwrap()
    {
        if let Some(index) = texture["resource_index"].as_u64() {
            assert_eq!(texture["name"], resources[index as usize]["name"]);
        }
    }
    let (rename_index, rename_key) = resources
        .iter()
        .enumerate()
        .find_map(|(i, resource)| {
            resource["rename_key"]
                .as_str()
                .filter(|key| *key != "ResourceId::0")
                .map(|key| (i, key.to_owned()))
        })
        .expect("fixture has a stable resource ID");
    let renamed = annotation_agent.send(&serde_json::json!({"command":"resource_rename","index":rename_index,"name":"中文 日本語 한국어"}).to_string());
    assert_eq!(renamed["ok"], true, "{renamed}");
    let edit_read = annotation_agent.send(r#"{"command":"annotation_read","name":"edits"}"#);
    assert_eq!(edit_read["ok"], true, "{edit_read}");
    let edit_doc = serde_json::json!({"editors":[{"id":rename_key,"stage":0,"entryPoint":"main","encoding":5,"commandLine":"","tool":"Builtin","flags":{},"files":[{"name":"main.hlsl","contents":"中文"},{"name":"include.hlsl","contents":"keep"}]}],"extra":true});
    let edit_saved = annotation_agent.send(&serde_json::json!({"command":"annotation_write","section":edit_read["data"]["section"],"document":edit_doc}).to_string());
    assert_eq!(edit_saved["ok"], true, "{edit_saved}");
    let read = annotation_agent.send(r#"{"command":"annotation_read","name":"notes"}"#);
    assert_eq!(read["ok"], true, "{read}");
    let original = read["data"]["section"].clone();
    let mut document = read["data"]["document"].clone();
    document["comments"] = serde_json::json!("你好\nRenderDocX");
    document["test-extra"] = serde_json::json!({"preserved":true});
    let saved = annotation_agent.send(
        &serde_json::json!({"command":"annotation_write","section":original,"document":document})
            .to_string(),
    );
    assert_eq!(saved["ok"], true, "{saved}");
    let stale = annotation_agent.send(
        &serde_json::json!({"command":"annotation_write","section":original,"document":document})
            .to_string(),
    );
    assert_eq!(
        stale["error"]["code"], "replay_annotation_failed",
        "{stale}"
    );
    let bookmark_read =
        annotation_agent.send(r#"{"command":"annotation_read","name":"bookmarks"}"#);
    assert_eq!(bookmark_read["ok"], true, "{bookmark_read}");
    let mut bookmark_doc = bookmark_read["data"]["document"].clone();
    bookmark_doc["Bookmarks"] = serde_json::json!([{"eventId":draw_event,"text":"draw"}]);
    let bookmark_saved = annotation_agent.send(&serde_json::json!({"command":"annotation_write","section":bookmark_read["data"]["section"],"document":bookmark_doc}).to_string());
    assert_eq!(bookmark_saved["ok"], true, "{bookmark_saved}");
    annotation_agent.quit();
    let mut reopened = Agent::new(Some(bridge));
    assert_eq!(
        reopened.send(&serde_json::json!({"command":"open","path":annotation_copy}).to_string())["ok"],
        true
    );
    assert_eq!(
        reopened.send(r#"{"command":"annotation_read","name":"notes"}"#)["data"]["document"],
        document
    );
    assert_eq!(
        reopened.send(r#"{"command":"annotation_read","name":"bookmarks"}"#)["data"]["document"],
        bookmark_doc
    );
    assert_eq!(
        reopened.send(r#"{"command":"annotation_read","name":"edits"}"#)["data"]["document"],
        edit_doc
    );
    let renamed = reopened.send(r#"{"command":"annotation_read","name":"renames"}"#);
    assert_eq!(
        renamed["data"]["document"]["CustomResourceNames"][&rename_key],
        "中文 日本語 한국어"
    );
    let reset = reopened.send(
        &serde_json::json!({"command":"resource_rename","index":rename_index,"name":""})
            .to_string(),
    );
    assert_eq!(reset["ok"], true, "{reset}");
    let reset_names = reopened.send(r#"{"command":"annotation_read","name":"renames"}"#);
    assert!(
        reset_names["data"]["document"]["CustomResourceNames"]
            .get(&rename_key)
            .is_none()
    );
    reopened.quit();

    let mut agent = Agent::new(Some(bridge));
    let request = serde_json::json!({"id":1,"command":"open","path":capture}).to_string();
    let opened = agent.send(&request);
    assert_eq!(opened["ok"], true, "{opened}");
    let capture_data = &opened["data"]["capture"];
    let messages = agent.send(r#"{"command":"debug_messages","offset":"0","limit":1}"#);
    assert_eq!(messages["ok"], true, "{messages}");
    let total = messages["data"]["total"]
        .as_str()
        .unwrap()
        .parse::<usize>()
        .unwrap();
    assert_eq!(
        messages["data"]["messages"].as_array().unwrap().len(),
        total.min(1)
    );
    let invalid = agent.send(r#"{"command":"debug_messages","limit":0}"#);
    assert_eq!(invalid["error"]["code"], "invalid_arguments");
    assert_eq!(
        agent.send(r#"{"command":"clear_debug_messages"}"#)["ok"],
        true
    );
    assert!(
        agent.send(r#"{"command":"snapshot"}"#)["data"]["capture"]["debug_messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let counters = capture_data["counters"].as_array().unwrap();
    if let Some(counter) = counters.first() {
        let id = counter["id"].as_u64().unwrap();
        let fetched =
            agent.send(&serde_json::json!({"command":"counter_fetch","counters":[id]}).to_string());
        assert_eq!(fetched["ok"], true, "{fetched}");
        let generation = fetched["data"]["fetch"]["generation"].as_str().unwrap();
        let total = fetched["data"]["fetch"]["total"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        let page = agent.send(&serde_json::json!({"command":"counter_page","generation":generation,"offset":"0","limit":1}).to_string());
        assert_eq!(page["ok"], true, "{page}");
        assert_eq!(
            page["data"]["page"]["entries"].as_array().unwrap().len(),
            total.min(1) as usize
        );
        if total > 0 {
            assert_eq!(page["data"]["page"]["entries"][0]["counter"], id);
        }
        let next =
            agent.send(&serde_json::json!({"command":"counter_fetch","counters":[id]}).to_string());
        assert_eq!(next["ok"], true, "{next}");
        let stale = agent.send(
            &serde_json::json!({"command":"counter_page","generation":generation}).to_string(),
        );
        assert_eq!(stale["error"]["code"], "replay_counter_failed");
    }
    assert_eq!(
        capture_data["resources"].as_array().unwrap().len(),
        capture_data["resource_count"].as_u64().unwrap() as usize
    );
    let vertex_resource = capture_data["buffers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|buffer| buffer["length"] == "28")
        .unwrap()["resource_index"]
        .as_u64()
        .unwrap();
    let usage = agent
        .send(&serde_json::json!({"command":"resource_usage","index":vertex_resource}).to_string());
    assert_eq!(usage["ok"], true, "{usage}");
    assert_eq!(usage["data"]["tracked"], true);
    assert!(
        usage["data"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["event_id"] == draw_event && entry["usage_name"] == "VertexBuffer"),
        "{usage}"
    );
    let first = agent.send(
        &serde_json::json!({"command":"resource_usage","index":vertex_resource,"limit":1})
            .to_string(),
    );
    assert_eq!(first["data"]["entries"].as_array().unwrap().len(), 1);
    let total = usage["data"]["total"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let end = agent.send(&serde_json::json!({"command":"resource_usage","index":vertex_resource,"offset":total.to_string()}).to_string());
    assert!(end["data"]["entries"].as_array().unwrap().is_empty());
    let invalid = agent.send(&serde_json::json!({"command":"resource_usage","index":vertex_resource,"offset":(total + 1).to_string()}).to_string());
    assert_eq!(invalid["error"]["code"], "replay_usage_failed");
    let shader_index = capture_data["resources"]
        .as_array()
        .unwrap()
        .iter()
        .position(|resource| resource["type"] == 10)
        .expect("shader resource");
    let shader_usage = agent
        .send(&serde_json::json!({"command":"resource_usage","index":shader_index}).to_string());
    assert_eq!(shader_usage["data"]["tracked"], false, "{shader_usage}");
    assert_eq!(shader_usage["data"]["entries"][0]["usage_name"], "Unused");
    let shader_end = agent.send(
        &serde_json::json!({"command":"resource_usage","index":shader_index,"offset":"1"})
            .to_string(),
    );
    assert_eq!(shader_end["data"]["tracked"], false);
    assert!(shader_end["data"]["entries"].as_array().unwrap().is_empty());
    let root_request = serde_json::json!({"command":"structure","event_id":api_event_id,"limit":1});
    let root = agent.send(&root_request.to_string());
    assert_eq!(root["ok"], true, "{root}");
    assert_eq!(root["data"]["structure"]["event_id"], api_event_id);
    let structure = &root["data"]["structure"];
    let children = structure["children"].as_array().unwrap();
    assert_eq!(children.len(), 1, "{root}");
    let child_count: u64 = structure["node"]["child_count"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(child_count > 0);
    let full =
        agent.send(&serde_json::json!({"command":"structure","event_id":api_event_id}).to_string());
    let topology = full["data"]["structure"]["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["name"] == "Topology")
        .expect("topology parameter");
    assert_eq!(topology["basic_type"], 6); // upstream SDBasic::Enum
    assert_eq!(topology["value"], "4"); // D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST
    let shader_structure = agent
        .send(&serde_json::json!({"command":"structure","event_id":shader_api_id}).to_string());
    let shader_resource = shader_structure["data"]["structure"]["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["name"] == "pShader")
        .expect("shader parameter");
    assert_eq!(shader_resource["basic_type"], 12); // upstream SDBasic::Resource
    assert_eq!(shader_resource["resource_non_null"], true);
    let update_structure = agent
        .send(&serde_json::json!({"command":"structure","event_id":update_api_id}).to_string());
    let update_children = update_structure["data"]["structure"]["children"]
        .as_array()
        .unwrap();
    let source_index = update_children
        .iter()
        .position(|node| node["name"] == "pSrcData" && node["basic_type"] == 4)
        .expect("structured upload buffer");
    assert_eq!(update_children[source_index]["byte_size"], "28");
    let source_path = source_index.to_string();
    let invalid_range = agent.send(&serde_json::json!({"command":"structure_buffer","event_id":update_api_id,"path":[source_path],"offset":"27","length":2}).to_string());
    assert_eq!(
        invalid_range["error"]["code"],
        "replay_structure_buffer_failed"
    );
    let first = agent.send(&serde_json::json!({"command":"structure_buffer","event_id":update_api_id,"path":[source_path],"offset":"0","length":8}).to_string());
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(first["data"]["hex"], "0000000000004040");
    let rest = agent.send(&serde_json::json!({"command":"structure_buffer","event_id":update_api_id,"path":[source_path],"offset":"8","length":20}).to_string());
    assert_eq!(
        rest["data"]["hex"],
        "0000404000004040000040400000404000004040"
    );
    let empty = agent.send(&serde_json::json!({"command":"structure_buffer","event_id":update_api_id,"path":[source_path],"offset":"28","length":0}).to_string());
    assert_eq!(empty["data"]["hex"], "");
    let child = agent.send(
        &serde_json::json!({"command":"structure","event_id":api_event_id,"path":["0"]})
            .to_string(),
    );
    assert_eq!(child["ok"], true, "{child}");
    assert_eq!(
        child["data"]["structure"]["node"]["name"],
        children[0]["name"]
    );
    let end = agent.send(&serde_json::json!({"command":"structure","event_id":api_event_id,"offset":child_count.to_string()}).to_string());
    assert!(
        end["data"]["structure"]["children"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let bad_path = agent.send(&serde_json::json!({"command":"structure","event_id":api_event_id,"path":["18446744073709551615"]}).to_string());
    assert_eq!(bad_path["error"]["code"], "replay_structure_failed");
    let bad_limit = agent.send(
        &serde_json::json!({"command":"structure","event_id":api_event_id,"limit":129}).to_string(),
    );
    assert_eq!(bad_limit["error"]["code"], "invalid_arguments");
    assert_eq!(
        agent.send(r#"{"command":"pipeline"}"#)["error"]["code"],
        "no_event"
    );
    assert_eq!(
        agent.send(r#"{"command":"shader","stage":"vertex"}"#)["error"]["code"],
        "no_event"
    );
    assert_eq!(
        agent.send(r#"{"command":"read_buffer","index":0,"offset":"0","length":1}"#)["error"]["code"],
        "no_event"
    );
    let texture_index = opened["data"]["capture"]["textures"]
        .as_array()
        .unwrap()
        .iter()
        .position(|texture| texture["width"] == 16 && texture["height"] == 16)
        .unwrap();
    assert!(
        !opened["data"]["capture"]["textures"][texture_index]["format_name"]
            .as_str()
            .unwrap()
            .is_empty()
    );
    let png_path = dir.path().join("图像 空格.png");
    let export = serde_json::json!({"id":20,"command":"export_texture","index":texture_index,"path":png_path.to_str().unwrap()}).to_string();
    assert_eq!(agent.send(&export)["error"]["code"], "no_event");
    let failed_open =
        agent.send(r#"{"id":10,"command":"open","path":"missing-renderdocx-capture.rdc"}"#);
    assert_eq!(failed_open["error"]["code"], "replay_open_failed");
    assert_eq!(agent.send(r#"{"id":11,"command":"snapshot"}"#)["ok"], true);
    let api_select = agent
        .send(&serde_json::json!({"id":13,"command":"select","event_id":api_event_id}).to_string());
    assert_eq!(api_select["data"]["event_id"], api_event_id, "{api_select}");
    assert_eq!(
        api_select["data"]["pipeline"]["event_id"], api_event_id,
        "{api_select}"
    );
    assert_eq!(
        agent.send(r#"{"id":12,"command":"select","event_id":4294967295u32}"#)["error"]["code"],
        "replay_select_failed"
    );
    let request =
        serde_json::json!({"id":2,"command":"select","event_id":event.parse::<u32>().unwrap()})
            .to_string();
    let selected = agent.send(&request);
    assert_eq!(
        selected["data"]["event_id"].as_u64().unwrap().to_string(),
        event
    );
    let not_draw = agent.send(r#"{"command":"mesh","stage":"vs_out"}"#);
    assert_eq!(
        not_draw["error"]["code"], "replay_mesh_failed",
        "{not_draw}"
    );
    let vertex_before = opened["data"]["capture"]["buffers"]
        .as_array()
        .unwrap()
        .iter()
        .position(|buffer| buffer["length"] == "28")
        .expect("fixture vertex buffer");
    let before = agent.send(
        &serde_json::json!({"command":"read_buffer","index":vertex_before,"offset":"4","length":8})
            .to_string(),
    );
    assert_eq!(before["data"]["hex"], "0000004000000040", "{before}");
    let exported = agent.send(&export);
    assert_eq!(exported["ok"], true, "{exported}");
    let bytes = std::fs::read(&png_path).unwrap();
    let decoder = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(&png_path).unwrap(),
    ));
    let mut reader = decoder.read_info().unwrap();
    assert_eq!((reader.info().width, reader.info().height), (16, 16));
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    assert!(matches!(
        info.color_type,
        png::ColorType::Rgb | png::ColorType::Rgba
    ));
    assert!(
        pixels[0] > 0 && pixels[1] > pixels[0] && pixels[2] > pixels[1],
        "unexpected clear color: {:?}",
        &pixels[..3]
    );
    assert_eq!(agent.send(&export)["error"]["code"], "replay_export_failed");
    assert_eq!(std::fs::read(&png_path).unwrap(), bytes);
    let draw =
        agent.send(&serde_json::json!({"command":"select","event_id":draw_event}).to_string());
    assert_eq!(draw["ok"], true, "{draw}");
    let pipeline = agent.send(r#"{"command":"pipeline"}"#);
    assert_eq!(pipeline["data"]["pipeline"]["multiview_count"], 1);
    let descriptors = agent.send(r#"{"command":"descriptors","offset":"0","limit":1}"#);
    assert_eq!(descriptors["ok"], true, "{descriptors}");
    assert_eq!(descriptors["data"]["page"]["event_id"], draw_event);
    let descriptor_total = descriptors["data"]["page"]["total"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert_eq!(
        descriptors["data"]["page"]["entries"]
            .as_array()
            .unwrap()
            .len(),
        descriptor_total.min(1) as usize
    );
    let last = agent.send(
        &serde_json::json!({"command":"descriptors","offset":descriptor_total.to_string()})
            .to_string(),
    );
    assert_eq!(last["ok"], true, "{last}");
    assert!(
        last["data"]["page"]["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let inputs = agent.send(r#"{"command":"vertex_input","offset":0,"limit":3}"#);
    assert_eq!(inputs["ok"], true, "{inputs}");
    assert_eq!(inputs["data"]["page"]["event_id"], draw_event);
    let rows = inputs["data"]["page"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["raw_index"], 0);
    assert_eq!(rows[1]["vertex_index"], 1);
    let position = rows[0]["attributes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["name"] == "POSITION")
        .unwrap();
    assert_eq!(position["hex"], "0000404000004040");
    assert_eq!(position["values"], serde_json::json!(["3", "3"]));
    let invalid_instance = agent.send(r#"{"command":"mesh","stage":"vs_out","instance":1}"#);
    assert_eq!(
        invalid_instance["error"]["code"], "replay_mesh_failed",
        "{invalid_instance}"
    );
    let invalid_view = agent.send(r#"{"command":"mesh","stage":"vs_out","view":1}"#);
    assert_eq!(
        invalid_view["error"]["code"], "replay_mesh_failed",
        "{invalid_view}"
    );
    assert!(
        invalid_view["error"]["message"]
            .as_str()
            .unwrap()
            .contains("view is out of range")
    );
    let unsupported_stage = agent.send(r#"{"command":"mesh","stage":"mesh_out"}"#);
    assert_eq!(
        unsupported_stage["error"]["code"], "replay_mesh_failed",
        "{unsupported_stage}"
    );
    let mesh = agent.send(r#"{"command":"mesh","stage":"vs_out"}"#);
    assert_eq!(mesh["ok"], true, "{mesh}");
    assert_eq!(mesh["data"]["mesh"]["event_id"], draw_event);
    assert_eq!(mesh["data"]["mesh"]["vertex_bound"], true, "{mesh}");
    assert_eq!(mesh["data"]["mesh"]["component_type"], 1);
    assert_eq!(mesh["data"]["mesh"]["component_count"], 4);
    assert_eq!(mesh["data"]["mesh"]["component_byte_width"], 4);
    assert_eq!(mesh["data"]["mesh"]["num_indices"], 3);
    assert_eq!(mesh["data"]["mesh"]["index_bound"], true, "{mesh}");
    assert_eq!(mesh["data"]["mesh"]["index_stride"], 2);
    let mesh_indices =
        agent.send(r#"{"command":"mesh_read","kind":"index","offset":"0","length":6}"#);
    assert_eq!(mesh_indices["ok"], true, "{mesh_indices}");
    assert_eq!(mesh_indices["data"]["hex"], "000001000200");
    assert_eq!(mesh_indices["data"]["indices"][0]["row"], "0");
    assert_eq!(mesh_indices["data"]["indices"][1]["raw"], 1);
    assert_eq!(mesh_indices["data"]["indices"][1]["vertex"], 1);
    assert_eq!(mesh_indices["data"]["indices"][1]["restart"], false);
    let draw_rows = agent.send(r#"{"command":"mesh_draw","offset":0,"count":3}"#);
    assert_eq!(draw_rows["ok"], true, "{draw_rows}");
    assert_eq!(draw_rows["data"]["page"]["rows"][0]["vertex"], 0);
    assert_eq!(
        draw_rows["data"]["page"]["rows"][0]["position"],
        serde_json::json!([3.0, 3.0, 0.0, 1.0])
    );
    let mesh_bytes =
        agent.send(r#"{"command":"mesh_read","kind":"vertex","offset":"0","length":16}"#);
    assert_eq!(mesh_bytes["ok"], true, "{mesh_bytes}");
    assert_eq!(mesh_bytes["data"]["positions"][0]["row"], "0");
    assert_eq!(
        mesh_bytes["data"]["positions"][0]["values"],
        serde_json::json!([3.0, 3.0, 0.0, 1.0])
    );
    assert_eq!(
        mesh_bytes["data"]["hex"],
        "0000404000004040000000000000803f"
    );
    assert_eq!(
        agent.send(r#"{"command":"mesh_read","kind":"vertex","offset":"0","length":65537}"#)["error"]
            ["code"],
        "invalid_arguments"
    );
    assert_eq!(
        agent.send(r#"{"command":"mesh","stage":"invalid"}"#)["error"]["code"],
        "invalid_arguments"
    );
    assert_eq!(
        agent.send(r#"{"command":"mesh_read","kind":"vertex","offset":"0","length":16}"#)["error"]
            ["code"],
        "no_mesh"
    );
    assert_eq!(
        agent.send(r#"{"command":"mesh","stage":"vs_out"}"#)["ok"],
        true
    );
    assert_eq!(
        agent.send(&serde_json::json!({"command":"select","event_id":draw_event}).to_string())["ok"],
        true
    );
    assert_eq!(
        agent.send(r#"{"command":"mesh_read","kind":"vertex","offset":"0","length":16}"#)["error"]
            ["code"],
        "no_mesh"
    );
    let layered_index = opened["data"]["capture"]["textures"]
        .as_array()
        .unwrap()
        .iter()
        .position(|texture| {
            texture["width"] == 8
                && texture["height"] == 8
                && texture["mips"] == 2
                && texture["array_size"] == 2
        })
        .expect("layered fixture texture");
    let picked = agent.send(&serde_json::json!({"command":"pick_pixel","index":layered_index,"mip":1,"slice":1,"sample":0,"x":3,"y":3}).to_string());
    assert_eq!(picked["ok"], true, "{picked}");
    assert_eq!(picked["data"]["pixel"]["event_id"], draw_event);
    assert_eq!(
        picked["data"]["pixel"]["float_values"],
        serde_json::json!(["1", "1", "0", "1"])
    );
    let statistics = agent.send(&serde_json::json!({"command":"texture_statistics","index":layered_index,"mip":1,"slice":1,"sample":0,"min":0,"max":1,"channels":[true,false,false,false]}).to_string());
    assert_eq!(statistics["ok"], true, "{statistics}");
    assert_eq!(
        statistics["data"]["statistics"]["min"]["float_values"],
        serde_json::json!(["1", "1", "0", "1"])
    );
    assert_eq!(
        statistics["data"]["statistics"]["max"]["float_values"],
        serde_json::json!(["1", "1", "0", "1"])
    );
    let histogram = statistics["data"]["statistics"]["histogram"]
        .as_array()
        .unwrap();
    assert_eq!(histogram.len(), 256);
    assert_eq!(histogram[255], 16);
    assert_eq!(
        histogram.iter().map(|v| v.as_u64().unwrap()).sum::<u64>(),
        16
    );
    let bad_range = agent.send(
        &serde_json::json!({"command":"texture_statistics","index":layered_index,"min":1,"max":1})
            .to_string(),
    );
    assert_eq!(bad_range["error"]["code"], "invalid_arguments");
    let blue_stats = agent.send(&serde_json::json!({"command":"texture_statistics","index":layered_index,"mip":1,"slice":1,"channel":2}).to_string());
    assert_eq!(blue_stats["ok"], true, "{blue_stats}");
    assert_eq!(
        blue_stats["data"]["statistics"]["request"]["channels"],
        serde_json::json!([false, false, true, false])
    );
    assert_eq!(blue_stats["data"]["statistics"]["histogram"][0], 16);
    let bad_channels = agent.send(&serde_json::json!({"command":"texture_statistics","index":layered_index,"channels":[true,false]}).to_string());
    assert_eq!(bad_channels["error"]["code"], "invalid_arguments");
    let out_of_range = agent.send(&serde_json::json!({"command":"pick_pixel","index":layered_index,"mip":1,"slice":1,"x":4,"y":0}).to_string());
    assert_eq!(
        out_of_range["error"]["code"], "invalid_arguments",
        "{out_of_range}"
    );
    let integer_index = opened["data"]["capture"]["textures"]
        .as_array()
        .unwrap()
        .iter()
        .position(|texture| texture["format_name"] == "R32G32B32A32_UINT")
        .expect("integer texture");
    let integer = agent.send(
        &serde_json::json!({"command":"pick_pixel","index":integer_index,"x":0,"y":0}).to_string(),
    );
    assert_eq!(integer["ok"], true, "{integer}");
    assert_eq!(
        integer["data"]["pixel"]["uint_values"],
        serde_json::json!([1, 2, 3, 4])
    );
    let integer_stats = agent.send(&serde_json::json!({"command":"texture_statistics","index":integer_index,"min":0,"max":5,"channels":[true,false,false,false]}).to_string());
    assert_eq!(integer_stats["ok"], true, "{integer_stats}");
    assert_eq!(
        integer_stats["data"]["statistics"]["min"]["uint_values"],
        serde_json::json!([1, 2, 3, 4])
    );
    assert_eq!(
        integer_stats["data"]["statistics"]["max"]["uint_values"],
        serde_json::json!([1, 2, 3, 4])
    );
    let volume_index = opened["data"]["capture"]["textures"]
        .as_array()
        .unwrap()
        .iter()
        .position(|texture| {
            texture["dimension"] == 3 && texture["width"] == 4 && texture["depth"] == 2
        })
        .expect("3D texture");
    let volume = agent.send(
        &serde_json::json!({"command":"pick_pixel","index":volume_index,"slice":1,"x":3,"y":3})
            .to_string(),
    );
    assert_eq!(volume["ok"], true, "{volume}");
    assert_eq!(
        volume["data"]["pixel"]["float_values"],
        serde_json::json!(["0", "0", "1", "1"])
    );
    let volume_stats = agent.send(&serde_json::json!({"command":"texture_statistics","index":volume_index,"slice":1,"channels":[false,false,true,false]}).to_string());
    assert_eq!(volume_stats["ok"], true, "{volume_stats}");
    assert_eq!(
        volume_stats["data"]["statistics"]["min"]["float_values"],
        serde_json::json!(["0", "0", "1", "1"])
    );
    assert_eq!(volume_stats["data"]["statistics"]["histogram"][255], 16);
    if msaa_samples > 1 {
        println!("D3D11 MSAA statistics sample count: {msaa_samples}");
        let msaa = opened["data"]["capture"]["textures"]
            .as_array()
            .unwrap()
            .iter()
            .position(|texture| {
                texture["width"] == 4
                    && texture["height"] == 4
                    && texture["samples"] == msaa_samples
            })
            .expect("multisampled texture");
        let single = agent.send(&serde_json::json!({"command":"texture_statistics","index":msaa,"sample":0,"channels":[true,false,false,false]}).to_string());
        let resolved = agent.send(&serde_json::json!({"command":"texture_statistics","index":msaa,"channels":[true,false,false,false]}).to_string());
        assert_eq!(single["ok"], true, "{single}");
        assert_eq!(resolved["ok"], true, "{resolved}");
        assert_eq!(resolved["data"]["statistics"]["sample"], u32::MAX);
        assert_eq!(
            single["data"]["statistics"]["min"]["bits"],
            resolved["data"]["statistics"]["min"]["bits"]
        );
        assert_eq!(
            single["data"]["statistics"]["max"]["bits"],
            resolved["data"]["statistics"]["max"]["bits"]
        );
        let red = resolved["data"]["statistics"]["min"]["float_values"][0]
            .as_str()
            .unwrap()
            .parse::<f32>()
            .unwrap();
        assert!((red - 0.25).abs() <= 1.0 / 255.0);
        let pixel_sample = agent.send(
            &serde_json::json!({"command":"pick_pixel","index":msaa,"sample":0,"x":0,"y":0})
                .to_string(),
        );
        let pixel_resolved = agent.send(
            &serde_json::json!({"command":"pick_pixel","index":msaa,"x":0,"y":0}).to_string(),
        );
        assert_eq!(pixel_sample["ok"], true, "{pixel_sample}");
        assert_eq!(pixel_resolved["ok"], true, "{pixel_resolved}");
        assert_eq!(
            pixel_sample["data"]["pixel"]["bits"],
            pixel_resolved["data"]["pixel"]["bits"]
        );
        assert_eq!(pixel_resolved["data"]["pixel"]["sample"], u32::MAX);
        let histogram = resolved["data"]["statistics"]["histogram"]
            .as_array()
            .unwrap();
        assert_eq!(
            histogram.iter().map(|v| v.as_u64().unwrap()).sum::<u64>(),
            16
        );
        let invalid = agent.send(
            &serde_json::json!({"command":"texture_statistics","index":msaa,"sample":msaa_samples})
                .to_string(),
        );
        assert_eq!(invalid["error"]["code"], "invalid_arguments");
    } else {
        eprintln!("D3D11 device has no supported 2x/4x sample mode; MSAA statistics not exercised");
    }
    let layered_path = dir.path().join("layered.png");
    let save_layer = serde_json::json!({"command":"export_texture","index":layered_index,"path":layered_path.to_str().unwrap(),"mip":1,"slice":1,"sample":0}).to_string();
    let saved_layer = agent.send(&save_layer);
    assert_eq!(saved_layer["ok"], true, "{saved_layer}");
    assert_eq!(saved_layer["data"]["selection"]["mip"], 1);
    let png = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(&layered_path).unwrap(),
    ));
    let mut layer_reader = png.read_info().unwrap();
    assert_eq!(
        (layer_reader.info().width, layer_reader.info().height),
        (4, 4)
    );
    let mut layer_pixels = vec![0; layer_reader.output_buffer_size().unwrap()];
    layer_reader.next_frame(&mut layer_pixels).unwrap();
    assert_eq!(&layer_pixels[..3], &[255, 255, 0]);
    let saved_bytes = std::fs::read(&layered_path).unwrap();
    assert_eq!(
        agent.send(&save_layer)["error"]["code"],
        "replay_export_failed"
    );
    assert_eq!(std::fs::read(&layered_path).unwrap(), saved_bytes);
    let blue_path = dir.path().join("channel.png");
    let channel_request = serde_json::json!({"command":"export_texture","index":layered_index,"path":blue_path.to_str().unwrap(),"mip":1,"slice":1,"channel":2}).to_string();
    let channel = agent.send(&channel_request);
    assert_eq!(channel["ok"], true, "{channel}");
    let png = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(&blue_path).unwrap(),
    ));
    let mut reader = png.read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    reader.next_frame(&mut pixels).unwrap();
    assert_eq!(&pixels[..3], &[0, 0, 0]);
    for (key, value) in [("mip", 2), ("slice", 2), ("sample", 1), ("channel", 4)] {
        let invalid_path = dir.path().join(format!("invalid-{key}.png"));
        let mut request = serde_json::json!({"command":"export_texture","index":layered_index,"path":invalid_path.to_str().unwrap()});
        request[key] = value.into();
        assert_eq!(
            agent.send(&request.to_string())["error"]["code"],
            "invalid_arguments",
            "{request}"
        );
        assert!(!invalid_path.exists());
    }
    let vertex_shader = agent.send(r#"{"command":"shader","stage":"vertex"}"#);
    assert_eq!(vertex_shader["ok"], true, "{vertex_shader}");
    let shader_path = dir.path().join("vertex-disassembly.txt");
    let export_request = serde_json::json!({"command":"shader_export","stage":"vertex","kind":"disassembly","path":shader_path}).to_string();
    let exported = agent.send(&export_request);
    assert_eq!(exported["ok"], true, "{exported}");
    assert_eq!(
        std::fs::read_to_string(&shader_path).unwrap(),
        vertex_shader["data"]["shader"]["disassembly"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        agent.send(&export_request)["error"]["code"],
        "shader_export_failed"
    );
    let missing_source = dir.path().join("missing-source.txt");
    let result = agent.send(&serde_json::json!({"command":"shader_export","stage":"vertex","kind":"source","source_index":999999,"path":missing_source}).to_string());
    assert_eq!(result["error"]["code"], "shader_export_failed");
    assert!(!missing_source.exists());
    let bad_debug =
        agent.send(r#"{"command":"debug_vertex","vertex":4294967295,"instance":0,"index":0}"#);
    assert_eq!(bad_debug["error"]["code"], "replay_debug_failed");
    let invalid_debug_view =
        agent.send(r#"{"command":"debug_vertex","vertex":0,"instance":0,"index":0,"view":1}"#);
    assert!(
        invalid_debug_view["error"]["message"]
            .as_str()
            .unwrap()
            .contains("view is out of range")
    );
    let debug = agent.send(r#"{"command":"debug_vertex","vertex":0,"instance":0,"index":0}"#);
    assert_eq!(debug["ok"], true, "{debug}");
    assert_eq!(debug["data"]["trace"]["event_id"], draw_event);
    assert_eq!(debug["data"]["trace"]["stage"], 0);
    assert_eq!(
        agent.send(r#"{"command":"shader","stage":"vertex"}"#)["error"]["code"],
        "replay_shader_failed"
    );
    assert_eq!(
        agent.send(r#"{"command":"debug_vertex","vertex":0,"instance":0}"#)["error"]["code"],
        "invalid_arguments"
    );
    assert_eq!(
        agent.send(r#"{"command":"debug_vertex","vertex":0,"instance":0,"index":0,"view":1}"#)["error"]
            ["code"],
        "replay_debug_failed"
    );
    let mut steps = 0;
    loop {
        let batch = agent.send(r#"{"command":"debug_next"}"#);
        assert_eq!(batch["ok"], true, "{batch}");
        assert_eq!(batch["data"]["batch"]["event_id"], draw_event);
        let states = batch["data"]["batch"]["states"].as_array().unwrap();
        for state in states {
            assert_eq!(state["step"], steps);
            steps += 1;
            for change in state["changes"].as_array().unwrap() {
                assert_eq!(change["before"]["value_hex"].as_str().unwrap().len(), 256);
                assert_eq!(change["after"]["value_hex"].as_str().unwrap().len(), 256);
            }
        }
        if batch["data"]["batch"]["finished"] == true {
            break;
        }
        assert!(!states.is_empty());
    }
    assert!(steps > 1);
    assert_eq!(agent.send(r#"{"command":"debug_stop"}"#)["ok"], true);
    assert_eq!(
        agent.send(r#"{"command":"debug_next"}"#)["error"]["code"],
        "replay_debug_failed"
    );
    assert_eq!(
        agent.send(r#"{"command":"debug_vertex","vertex":0,"instance":0,"index":0}"#)["ok"],
        true
    );
    assert_eq!(
        agent.send(&serde_json::json!({"command":"select","event_id":draw_event}).to_string())["ok"],
        true
    );
    assert_eq!(
        agent.send(r#"{"command":"debug_next"}"#)["error"]["code"],
        "replay_debug_failed"
    );
    assert_eq!(vertex_shader["data"]["shader"]["event_id"], draw_event);
    assert!(
        vertex_shader["data"]["shader"]["disassembly"]
            .as_str()
            .unwrap()
            .contains("vs_4_0"),
        "{vertex_shader}"
    );
    assert!(
        vertex_shader["data"]["shader"]["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value["semantic"] == "POSITION"),
        "{vertex_shader}"
    );
    let targets = vertex_shader["data"]["shader"]["targets"]
        .as_array()
        .unwrap();
    assert!(!targets.is_empty(), "{vertex_shader}");
    let alternate = agent.send(r#"{"command":"shader","stage":"vertex","target_index":0}"#);
    assert_eq!(alternate["ok"], true, "{alternate}");
    assert_eq!(alternate["data"]["shader"]["target"], targets[0]);
    let pixel_shader = agent.send(r#"{"command":"shader","stage":"fragment"}"#);
    assert_eq!(pixel_shader["ok"], true, "{pixel_shader}");
    assert!(
        pixel_shader["data"]["shader"]["disassembly"]
            .as_str()
            .unwrap()
            .contains("ps_4_0"),
        "{pixel_shader}"
    );
    assert_eq!(
        agent.send(r#"{"command":"shader","stage":"geometry"}"#)["error"]["code"],
        "replay_shader_failed"
    );
    assert_eq!(
        agent.send(r#"{"command":"shader","stage":"vertex","target_index":4294967295}"#)["error"]["code"],
        "replay_shader_failed"
    );
    assert_eq!(
        agent.send(r#"{"command":"shader","stage":"other"}"#)["error"]["code"],
        "invalid_arguments"
    );
    let state = agent.send(r#"{"command":"pipeline"}"#);
    let pipeline = &state["data"]["pipeline"];
    assert_eq!(pipeline["event_id"], draw_event);
    assert_eq!(pipeline["available"], true);
    assert_eq!(pipeline["topology"], 5);
    assert_eq!(pipeline["index_buffer"]["bound"], true, "{pipeline}");
    assert_eq!(pipeline["index_buffer"]["byte_offset"], "2");
    assert_eq!(pipeline["index_buffer"]["byte_stride"], 2);
    let vb = &pipeline["vertex_buffers"][0];
    assert_eq!(vb["bound"], true, "{pipeline}");
    assert_eq!(vb["byte_offset"], "4");
    assert_eq!(vb["byte_stride"], 8);
    let buffers = opened["data"]["capture"]["buffers"].as_array().unwrap();
    let vertex_index = buffers
        .iter()
        .position(|buffer| buffer["resource_index"] == vb["resource_index"])
        .expect("vertex buffer metadata");
    assert_eq!(vertex_index, vertex_before);
    let read = |offset: &str, length: u32| {
        serde_json::json!({"command":"read_buffer","index":vertex_index,"offset":offset,"length":length}).to_string()
    };
    let content = agent.send(&read("4", 8));
    assert_eq!(content["data"]["hex"], "0000404000004040", "{content}");
    assert_eq!(agent.send(&read("28", 0))["data"]["hex"], "");
    assert_eq!(
        agent.send(&read("28", 1))["error"]["code"],
        "invalid_arguments"
    );
    assert_eq!(
        agent.send(&read("18446744073709551615", 1))["error"]["code"],
        "invalid_arguments"
    );
    assert_eq!(
        agent.send(&read("0", 65537))["error"]["code"],
        "invalid_arguments"
    );
    let inputs = pipeline["vertex_inputs"].as_array().unwrap();
    assert!(
        inputs.iter().any(|input| input["name"] == "POSITION"
            && input["vertex_buffer"] == 0
            && input["byte_offset"] == 0
            && input["used"] == true),
        "{pipeline}"
    );
    assert!(
        pipeline["viewports"]
            .as_array()
            .unwrap()
            .iter()
            .any(|viewport| viewport["enabled"] == true
                && viewport["width"] == 16.0
                && viewport["height"] == 16.0),
        "{pipeline}"
    );
    let shaders = pipeline["shaders"].as_array().unwrap();
    for stage in ["vertex", "fragment"] {
        assert!(
            shaders
                .iter()
                .any(|shader| shader["stage"] == stage && shader["bound"] == true),
            "{pipeline}"
        );
    }
    assert!(
        pipeline["outputs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|output| output["bound"] == true),
        "{pipeline}"
    );
    let selected_history = agent
        .send(&serde_json::json!({"command":"select","event_id":history_draw_event}).to_string());
    assert_eq!(selected_history["ok"], true, "{selected_history}");
    let pick =
        serde_json::json!({"command":"pick_pixel","index":texture_index,"x":8,"y":8}).to_string();
    let original = agent.send(&pick);
    assert_eq!(original["ok"], true, "{original}");
    let fragment_report = agent.send(r#"{"command":"shader","stage":"fragment"}"#);
    assert_eq!(fragment_report["ok"], true, "{fragment_report}");
    let replacement_request = serde_json::json!({"command":"shader_apply","stage":"fragment","encoding":5,"entry":"main","flags":fragment_report["data"]["shader"]["compile_flags"],"files":[{"name":"main.hlsl","contents":"#include \"color.hlsl\"\nfloat4 main() : SV_Target { return color(); }"},{"name":"color.hlsl","contents":"#pragma once\nfloat4 color() { return float4(1,0,0,1); }"}]}).to_string();
    let replacement = agent.send(&replacement_request);
    assert_eq!(
        replacement["data"]["edit"]["applied"], true,
        "{replacement}"
    );
    let changed = agent.send(&pick);
    assert_eq!(changed["ok"], true, "{changed}");
    assert_ne!(
        changed["data"]["pixel"]["bits"],
        original["data"]["pixel"]["bits"]
    );
    let bad_flags = agent.send(&serde_json::json!({"command":"shader_apply","stage":"fragment","encoding":5,"entry":"main","source":"float4 main() : SV_Target { return 1; }","flags":[{"name":"@cmdline","value":"/T invalid_profile"}]}).to_string());
    assert_eq!(bad_flags["data"]["edit"]["applied"], false, "{bad_flags}");
    assert_eq!(
        agent.send(&pick)["data"]["pixel"]["bits"],
        original["data"]["pixel"]["bits"]
    );
    assert_eq!(
        agent.send(&replacement_request)["data"]["edit"]["applied"],
        true
    );
    let external = std::env::var_os("RENDERDOCX_TEST_SHADER_PROCESSOR")
        .expect("set RENDERDOCX_TEST_SHADER_PROCESSOR");
    let processed = agent.send(&serde_json::json!({"command":"shader_apply","stage":"fragment","encoding":5,"entry":"main","source":"float4 main() : SV_Target { return float4(1,0,0,1); }","flags":fragment_report["data"]["shader"]["compile_flags"],"processor":{"executable":std::path::PathBuf::from(external.clone()),"input_encoding":5,"output_encoding":5,"arguments":["{stdin}"]}}).to_string());
    assert_eq!(processed["data"]["edit"]["applied"], true, "{processed}");
    assert!(
        processed["data"]["processor_diagnostics"]
            .as_str()
            .unwrap()
            .contains("fixture compiler diagnostics")
    );
    assert_eq!(
        agent.send(&pick)["data"]["pixel"]["bits"],
        changed["data"]["pixel"]["bits"]
    );
    let processor_failure = agent.send(&serde_json::json!({"command":"shader_apply","stage":"fragment","encoding":5,"entry":"main","source":"float4 main() : SV_Target { return 0; }","processor":{"executable":std::path::PathBuf::from(external.clone()),"input_encoding":5,"output_encoding":5,"arguments":["--fail"]}}).to_string());
    assert_eq!(
        processor_failure["error"]["code"], "shader_processor_failed",
        "{processor_failure}"
    );
    assert_eq!(
        agent.send(&pick)["data"]["pixel"]["bits"],
        changed["data"]["pixel"]["bits"]
    );
    let unsupported_processor = agent.send(&serde_json::json!({"command":"shader_apply","stage":"fragment","encoding":5,"entry":"main","source":"float4 main() : SV_Target { return 0; }","processor":{"executable":"definitely-missing-processor","input_encoding":5,"output_encoding":999,"arguments":[]}}).to_string());
    assert_eq!(
        unsupported_processor["error"]["code"], "shader_processor_failed",
        "{unsupported_processor}"
    );
    assert!(
        unsupported_processor["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unsupported")
    );
    assert_eq!(
        agent.send(&pick)["data"]["pixel"]["bits"],
        changed["data"]["pixel"]["bits"]
    );
    let restored = agent.send(r#"{"command":"shader_revert","stage":"fragment"}"#);
    assert_eq!(restored["ok"], true, "{restored}");
    assert_eq!(
        agent.send(&pick)["data"]["pixel"]["bits"],
        original["data"]["pixel"]["bits"]
    );
    let failed = agent.send(&serde_json::json!({"command":"shader_apply","stage":"fragment","encoding":5,"entry":"main","source":"invalid shader"}).to_string());
    assert_eq!(failed["data"]["edit"]["applied"], false, "{failed}");
    assert!(
        !failed["data"]["edit"]["diagnostics"]
            .as_str()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        agent.send(r#"{"command":"mesh","stage":"vs_out"}"#)["ok"],
        true
    );
    let request = serde_json::json!({"command":"pixel_history","index":texture_index,"x":8,"y":8});
    let history = agent.send(&request.to_string());
    assert_eq!(history["ok"], true, "{history}");
    assert_eq!(
        agent.send(r#"{"command":"mesh_read","kind":"vertex","offset":"0","length":16}"#)["error"]
            ["code"],
        "no_mesh"
    );
    let entries = history["data"]["history"]["entries"].as_array().unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry["event_id"].as_u64().unwrap().to_string() == event),
        "{history}"
    );
    let draw = entries
        .iter()
        .find(|entry| entry["event_id"] == history_draw_event && entry["passed"] == true)
        .expect("history draw fragment");
    let pixel_debug = agent.send(&serde_json::json!({"command":"debug_pixel","x":8,"y":8,"sample":0,"primitive":draw["primitive"]}).to_string());
    assert_eq!(pixel_debug["ok"], true, "{pixel_debug}");
    assert_eq!(pixel_debug["data"]["trace"]["stage"], 4);
    assert_eq!(pixel_debug["data"]["trace"]["event_id"], history_draw_event);
    assert_eq!(
        agent.send(r#"{"command":"debug_pixel","x":8,"y":8,"sample":4294967295}"#)["error"]["code"],
        "invalid_arguments"
    );
    let mut pixel_steps = 0;
    loop {
        let batch = agent.send(r#"{"command":"debug_next"}"#);
        assert_eq!(batch["ok"], true, "{batch}");
        for state in batch["data"]["batch"]["states"].as_array().unwrap() {
            assert_eq!(state["step"], pixel_steps);
            pixel_steps += 1;
        }
        if batch["data"]["batch"]["finished"] == true {
            break;
        }
    }
    assert!(pixel_steps > 1);
    assert_eq!(agent.send(r#"{"command":"debug_stop"}"#)["ok"], true);
    let pre = &draw["pre"]["color"]["float_values"];
    let post = &draw["post"]["color"]["float_values"];
    for (values, expected) in [
        (pre, [0.25, 0.5, 0.75, 1.0]),
        (post, [0.75, 0.25, 0.5, 1.0]),
    ] {
        for (value, expected) in values.as_array().unwrap().iter().zip(expected) {
            assert!(
                (value.as_str().unwrap().parse::<f32>().unwrap() - expected).abs() <= 1.0 / 255.0,
                "{draw}"
            );
        }
    }
    let first = agent.send(
        &serde_json::json!({"command":"pixel_history","index":texture_index,"x":8,"y":8,"limit":1})
            .to_string(),
    );
    assert_eq!(
        first["data"]["history"]["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let total = history["data"]["history"]["total"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    if total > 1 {
        let next = agent.send(&serde_json::json!({"command":"pixel_history","index":texture_index,"x":8,"y":8,"offset":"1","limit":1}).to_string());
        assert_eq!(next["ok"], true, "{next}");
        assert_eq!(
            next["data"]["history"]["entries"][0],
            history["data"]["history"]["entries"][1]
        );
    }
    let end = agent.send(&serde_json::json!({"command":"pixel_history","index":texture_index,"x":8,"y":8,"offset":total.to_string()}).to_string());
    assert!(
        end["data"]["history"]["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let past_end = agent.send(&serde_json::json!({"command":"pixel_history","index":texture_index,"x":8,"y":8,"offset":(total + 1).to_string()}).to_string());
    assert_eq!(past_end["error"]["code"], "replay_pixel_history_failed");
    let invalid_limit = agent.send(&serde_json::json!({"command":"pixel_history","index":texture_index,"x":8,"y":8,"limit":129}).to_string());
    assert_eq!(invalid_limit["error"]["code"], "invalid_arguments");
    let current = agent.send(r#"{"command":"pipeline"}"#);
    assert_eq!(current["data"]["pipeline"]["event_id"], history_draw_event);
    let picked_after = agent.send(
        &serde_json::json!({"command":"pick_pixel","index":texture_index,"x":8,"y":8}).to_string(),
    );
    assert_eq!(
        picked_after["data"]["pixel"]["bits"],
        draw["post"]["color"]["bits"]
    );
    assert_eq!(
        agent.send(&serde_json::json!({"command":"select","event_id":draw_event}).to_string())["ok"],
        true
    );
    let earlier = agent.send(&request.to_string());
    assert!(
        earlier["data"]["history"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["event_id"].as_u64().unwrap() <= draw_event)
    );
    assert_eq!(
        agent.send(
            &serde_json::json!({"command":"select","event_id":scissor_draw_event}).to_string()
        )["ok"],
        true
    );
    let clipped = agent.send(&request.to_string());
    assert_eq!(clipped["ok"], true, "{clipped}");
    assert!(
        clipped["data"]["history"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["event_id"] == scissor_draw_event
                && entry["passed"] == false
                && entry["tests"]["scissor_clipped"] == true),
        "{clipped}"
    );
    let dispatch = capture_data["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| {
            action["name"]
                .as_str()
                .is_some_and(|name| name.contains("Dispatch"))
        })
        .expect("compute dispatch");
    assert_eq!(dispatch["dispatch_groups"], serde_json::json!([2, 1, 1]));
    let dispatch_event = dispatch["event_id"].as_u64().unwrap();
    assert_eq!(
        agent.send(&serde_json::json!({"command":"select","event_id":dispatch_event}).to_string())
            ["ok"],
        true
    );
    let shader = agent.send(r#"{"command":"shader","stage":"compute"}"#);
    assert_eq!(shader["ok"], true, "{shader}");
    assert_eq!(
        shader["data"]["shader"]["dispatch_threads"],
        serde_json::json!([2, 2, 1])
    );
    for (group, thread) in [
        ([2, 0, 0], [0, 0, 0]),
        ([0, 0, 0], [2, 0, 0]),
        ([0, 0, 0], [0, 0, 1]),
    ] {
        let invalid = agent.send(
            &serde_json::json!({"command":"debug_compute","group":group,"thread":thread})
                .to_string(),
        );
        assert_eq!(invalid["error"]["code"], "replay_debug_failed", "{invalid}");
    }
    assert_eq!(
        agent.send(r#"{"command":"debug_compute","group":[0,0],"thread":[0,0,0]}"#)["error"]["code"],
        "invalid_arguments"
    );
    let compute = agent.send(r#"{"command":"debug_compute","group":[1,0,0],"thread":[1,1,0]}"#);
    assert_eq!(compute["ok"], true, "{compute}");
    assert_eq!(shader["data"]["shader"]["source_debug_info"], true);
    let files = shader["data"]["shader"]["source_files"].as_array().unwrap();
    assert!(
        files[0]["filename"]
            .as_str()
            .unwrap()
            .ends_with("compute-fixture.hlsl")
    );
    let source = files[0]["contents"].as_str().unwrap();
    let mappings = compute["data"]["trace"]["instructions"].as_array().unwrap();
    assert!(!mappings.is_empty());
    assert!(
        mappings
            .windows(2)
            .all(|pair| pair[0]["instruction"].as_u64() < pair[1]["instruction"].as_u64())
    );
    let first = &mappings[0];
    assert_eq!(first["file_index"], 0);
    assert_eq!(first["line_start"], 6);
    assert!(source.lines().nth(5).unwrap().contains("uint value ="));
    let disassembly_line = first["disassembly_line"].as_u64().unwrap() as usize;
    assert!(disassembly_line > 0);
    assert!(
        shader["data"]["shader"]["disassembly"]
            .as_str()
            .unwrap()
            .lines()
            .nth(disassembly_line - 1)
            .is_some()
    );
    assert!(
        first["source_variables"]
            .as_array()
            .unwrap()
            .iter()
            .any(|variable| variable["name"] == "group"
                && variable["variables"][0]["name"] == "vThreadGroupID")
    );
    assert!(mappings.iter().any(|mapping| mapping["line_start"] == 7));
    assert_eq!(compute["data"]["trace"]["stage"], 5);
    assert_eq!(compute["data"]["trace"]["event_id"], dispatch_event);
    let inputs = compute["data"]["trace"]["inputs"].as_array().unwrap();
    for (name, expected) in [
        ("vThreadGroupID", "010000000000000000000000"),
        ("vThreadIDInGroup", "010000000100000000000000"),
        ("vThreadID", "030000000100000000000000"),
    ] {
        let value = inputs
            .iter()
            .find(|input| input["name"] == name)
            .expect(name);
        assert!(
            value["value_hex"].as_str().unwrap().starts_with(expected),
            "{value}"
        );
    }
    let mut steps = 0;
    loop {
        let batch = agent.send(r#"{"command":"debug_next"}"#);
        assert_eq!(batch["ok"], true, "{batch}");
        for state in batch["data"]["batch"]["states"].as_array().unwrap() {
            assert_eq!(state["step"], steps);
            steps += 1;
        }
        if batch["data"]["batch"]["finished"] == true {
            break;
        }
    }
    assert!(steps > 1);
    assert_eq!(
        agent.send(r#"{"command":"debug_collect"}"#)["error"]["code"],
        "debug_already_streamed"
    );
    assert_eq!(agent.send(r#"{"command":"debug_stop"}"#)["ok"], true);
    assert_eq!(
        agent.send(r#"{"command":"debug_compute","group":[1,0,0],"thread":[1,1,0]}"#)["ok"],
        true
    );
    let collected = agent.send(r#"{"command":"debug_collect"}"#);
    assert_eq!(collected["ok"], true, "{collected}");
    assert_eq!(collected["data"]["total"], steps);
    assert_eq!(collected["data"]["position"], 0);
    let set = agent.send(r#"{"command":"debug_breakpoint","breakpoint":{"kind":"source","file_index":0,"line":7},"enabled":true}"#);
    assert_eq!(set["ok"], true, "{set}");
    let run = agent.send(r#"{"command":"debug_navigate","direction":"forward","mode":"run"}"#);
    assert_eq!(run["data"]["reason"], "breakpoint", "{run}");
    assert_eq!(run["data"]["location"]["line_start"], 7);
    let set_earlier = agent.send(r#"{"command":"debug_breakpoint","breakpoint":{"kind":"source","file_index":0,"line":6},"enabled":true}"#);
    assert_eq!(set_earlier["ok"], true, "{set_earlier}");
    let reverse = agent.send(r#"{"command":"debug_navigate","direction":"backward","mode":"run"}"#);
    assert_eq!(reverse["data"]["reason"], "breakpoint", "{reverse}");
    assert_eq!(reverse["data"]["location"]["line_start"], 6);
    for mode in ["over", "out"] {
        let step = agent.send(
            &serde_json::json!({"command":"debug_navigate","direction":"forward","mode":mode})
                .to_string(),
        );
        assert_eq!(step["ok"], true, "{step}");
    }
    assert_eq!(
        agent.send(
            r#"{"command":"debug_navigate","direction":"forward","mode":"run","event_mask":8}"#
        )["error"]["code"],
        "invalid_arguments"
    );
    assert_eq!(
        agent.send(
            r#"{"command":"debug_navigate","direction":"forward","mode":"source","event_mask":1}"#
        )["error"]["code"],
        "invalid_arguments"
    );
    assert_eq!(
        agent.send(&serde_json::json!({"command":"select","event_id":draw_event}).to_string())["ok"],
        true
    );
    assert_eq!(
        agent.send(r#"{"command":"debug_state"}"#)["error"]["code"],
        "no_debug_timeline"
    );
    assert_eq!(agent.send(r#"{"id":3,"command":"close"}"#)["ok"], true);
    assert_eq!(
        agent.send(r#"{"id":4,"command":"snapshot"}"#)["error"]["code"],
        "no_capture"
    );
    let reopened =
        agent.send(&serde_json::json!({"id":5,"command":"open","path":capture}).to_string());
    assert_eq!(reopened["ok"], true, "{reopened}");
    agent.quit();
}

#[cfg(windows)]
#[test]
#[ignore = "requires a matching RenderDoc core, D3D11 and remote server fixtures"]
fn real_remote_replay_loopback() {
    use std::{net::TcpListener, os::windows::process::CommandExt, path::PathBuf};
    struct Server(Child);
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let bridge = PathBuf::from(std::env::var_os("RENDERDOCX_TEST_BRIDGE").unwrap());
    let fixture = std::env::var_os("RENDERDOCX_TEST_FIXTURE").unwrap();
    let server_exe = std::env::var_os("RENDERDOCX_TEST_REMOTE_SERVER").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(fixture)
        .arg(bridge.parent().unwrap().join("renderdoc.dll"))
        .arg(dir.path().join("frame"))
        .arg("--callstacks")
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let capture = stdout
        .lines()
        .find_map(|s| s.strip_prefix("RENDERDOCX_CAPTURE="))
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let _server = Server(
        Command::new(server_exe)
            .arg(port.to_string())
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut agent = Agent::new(bridge.to_str());
    let url = format!("127.0.0.1:{port}");
    let listing_request =
        serde_json::json!({"command":"remote_list", "url":url, "path":dir.path()}).to_string();
    let mut listing = Value::Null;
    for _ in 0..20 {
        listing = agent.send(&listing_request);
        if listing["ok"] == true {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(listing["ok"], true, "{listing}");
    assert!(
        !listing["data"]["directory"]["home"]
            .as_str()
            .unwrap()
            .is_empty()
    );
    let filename = PathBuf::from(capture)
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let entry = listing["data"]["directory"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == filename)
        .expect("remote capture entry");
    let size = entry["size"].as_str().unwrap();
    let download = agent.send(
        &serde_json::json!({"command":"remote_download", "url":url, "path":capture,
        "size":size, "capture_directory":dir.path()})
        .to_string(),
    );
    assert_eq!(download["ok"], true, "{download}");
    assert_eq!(
        std::fs::read(download["data"]["path"].as_str().unwrap()).unwrap(),
        std::fs::read(capture).unwrap()
    );
    let mismatch = agent.send(
        &serde_json::json!({"command":"remote_download", "url":url, "path":capture,
        "size":"1", "capture_directory":dir.path()})
        .to_string(),
    );
    assert_eq!(
        mismatch["error"]["code"], "remote_download_failed",
        "{mismatch}"
    );
    let request =
        serde_json::json!({"command":"open", "path":capture, "remote_url":url}).to_string();
    let mut opened = Value::Null;
    for _ in 0..20 {
        opened = agent.send(&request);
        if opened["ok"] == true {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(opened["ok"], true, "{opened}");
    let mut other = Agent::new(bridge.to_str());
    let busy = other.send(&listing_request);
    assert_eq!(busy["error"]["code"], "remote_list_failed", "{busy}");
    other.quit();
    let active_listing = agent.send(&listing_request);
    assert_eq!(active_listing["ok"], true, "{active_listing}");
    let missing = agent.send(&serde_json::json!({"command":"open", "path":dir.path().join("missing.rdc"), "remote_url":url}).to_string());
    assert_eq!(missing["error"]["code"], "replay_open_failed", "{missing}");
    assert_eq!(agent.send(r#"{"command":"snapshot"}"#)["ok"], true);
    let active_download = agent.send(
        &serde_json::json!({"command":"remote_download", "url":url, "path":capture,
        "size":size, "capture_directory":dir.path()})
        .to_string(),
    );
    assert_eq!(active_download["ok"], true, "{active_download}");
    assert_eq!(
        std::fs::read(active_download["data"]["path"].as_str().unwrap()).unwrap(),
        std::fs::read(capture).unwrap()
    );
    let event = opened["data"]["capture"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| {
            a["name"]
                .as_str()
                .is_some_and(|n| n.contains("DrawIndexed"))
        })
        .expect("draw action")["event_id"]
        .as_u64()
        .unwrap();
    let selected =
        agent.send(&serde_json::json!({"command":"select", "event_id":event}).to_string());
    assert_eq!(selected["ok"], true, "{selected}");
    let stack =
        agent.send(&serde_json::json!({"command":"callstack", "event_id":event}).to_string());
    assert_eq!(stack["ok"], true, "{stack}");
    assert_eq!(stack["data"]["callstack"]["available"], true);
    assert!(
        !stack["data"]["callstack"]["frames"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{stack}"
    );
    assert_eq!(stack["data"]["callstack"]["resolved"], false);
    assert_eq!(
        agent.send(
            &serde_json::json!({"command":"callstack", "event_id":4294967295u32}).to_string()
        )["error"]["code"],
        "replay_callstack_failed"
    );

    let update = opened["data"]["capture"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|a| a["api_events"].as_array().unwrap())
        .find(|e| {
            e["name"]
                .as_str()
                .is_some_and(|n| n.contains("UpdateSubresource"))
        })
        .expect("update event")["event_id"]
        .as_u64()
        .unwrap();
    let structure =
        agent.send(&serde_json::json!({"command":"structure", "event_id":update}).to_string());
    assert_eq!(structure["ok"], true, "{structure}");
    let index = structure["data"]["structure"]["children"]
        .as_array()
        .unwrap()
        .iter()
        .position(|n| n["name"] == "pSrcData" && n["basic_type"] == 4)
        .unwrap();
    let data = agent.send(
        &serde_json::json!({"command":"structure_buffer", "event_id":update,
        "path":[index.to_string()], "offset":"0", "length":8})
        .to_string(),
    );
    assert_eq!(data["ok"], true, "{data}");
    assert_eq!(data["data"]["hex"], "0000000000004040");
    let switched = agent.send(&serde_json::json!({"command":"open", "path":active_download["data"]["path"], "remote_url":url}).to_string());
    assert_eq!(switched["ok"], true, "{switched}");
    assert_eq!(
        agent.send(&serde_json::json!({"command":"select", "event_id":event}).to_string())["ok"],
        true
    );
    assert_eq!(agent.send(r#"{"command":"close"}"#)["ok"], true);
    let local_open = agent.send(&serde_json::json!({"command":"open", "path":capture}).to_string());
    assert_eq!(local_open["ok"], true, "{local_open}");
    let local_stack =
        agent.send(&serde_json::json!({"command":"callstack", "event_id":event}).to_string());
    assert_eq!(local_stack["ok"], true, "{local_stack}");
    assert_eq!(
        local_stack["data"]["callstack"]["frames"],
        stack["data"]["callstack"]["frames"]
    );
    agent.quit();
}

#[test]
fn remote_open_rejects_invalid_url() {
    let mut agent = Agent::new(None);
    for value in [
        serde_json::json!(""),
        serde_json::json!("  "),
        serde_json::json!(42),
        serde_json::Value::Null,
    ] {
        let request =
            serde_json::json!({"command":"open", "path":"capture.rdc", "remote_url":value})
                .to_string();
        assert_eq!(agent.send(&request)["error"]["code"], "invalid_arguments");
    }
    agent.quit();
}

#[cfg(windows)]
#[test]
#[ignore = "requires matching RenderDoc server and live D3D11 fixtures"]
fn real_remote_target_launch_and_capture_loopback() {
    use std::{
        net::TcpListener,
        os::windows::process::CommandExt,
        path::PathBuf,
        time::{Duration, Instant},
    };
    struct Server(Child);
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    struct Stop(PathBuf);
    impl Drop for Stop {
        fn drop(&mut self) {
            let _ = std::fs::write(&self.0, b"stop");
        }
    }
    let bridge = PathBuf::from(std::env::var_os("RENDERDOCX_TEST_BRIDGE").unwrap());
    let fixture = PathBuf::from(std::env::var_os("RENDERDOCX_TEST_LIVE_FIXTURE").unwrap());
    let server_exe = std::env::var_os("RENDERDOCX_TEST_REMOTE_SERVER").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let stop = dir.path().join("stop.flag");
    let _stop = Stop(stop.clone());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let _server = Server(
        Command::new(server_exe)
            .arg(port.to_string())
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    std::thread::sleep(Duration::from_millis(500));
    let mut agent = Agent::new(bridge.to_str());
    let request = serde_json::json!({"command":"target_launch", "path":fixture,
        "remote_url":format!("127.0.0.1:{port}"), "configuration": {
        "arguments":format!("\"{}\"",stop.display()), "working_directory":dir.path(),
        "options":{"allow_vsync":false,"soft_memory_limit":64},
        "environment":[{"name":"RENDERDOCX_FIXTURE_MARKER","value":"开始:中间:结束", "operation":"set","separator":"no_sep"}]}});
    let launched = agent.send(&request.to_string());
    assert_eq!(launched["ok"], true, "{launched}");
    let deadline = Instant::now() + Duration::from_secs(20);
    let info_path = PathBuf::from(format!("{}.info", stop.display()));
    while !info_path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let info = std::fs::read_to_string(info_path).expect("remote fixture settings");
    assert!(info.starts_with("1 1 0 "), "{info}");
    let mut ready = false;
    while !ready && Instant::now() < deadline {
        let events = agent.send(r#"{"command":"target_events"}"#);
        ready = events["data"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "api" && e["supported"] == true && e["presenting"] == true);
        if !ready {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    assert!(ready, "remote target did not become ready");
    assert_eq!(
        agent.send(r#"{"command":"target_trigger","frames":1}"#)["ok"],
        true
    );
    let mut captured = false;
    while !captured && Instant::now() < deadline {
        let events = agent.send(r#"{"command":"target_events"}"#);
        captured = events["data"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "capture");
        if !captured {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    assert!(captured, "remote target did not capture");
    assert_eq!(agent.send(r#"{"command":"target_close"}"#)["ok"], true);
    std::fs::write(&stop, b"stop").unwrap();
    let done = PathBuf::from(format!("{}.done", stop.display()));
    while !done.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(done.exists(), "fixture did not exit");
    agent.quit();
}
