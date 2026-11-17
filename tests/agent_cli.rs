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

#[test]
fn texture_center_reports_capability_and_requires_capture() {
    let mut agent = Agent::new(None);
    let capabilities = agent.send(r#"{"command":"capabilities"}"#);
    assert!(
        capabilities["data"]["serve_commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|command| command == "texture_center")
    );
    let result = agent.send(r#"{"command":"texture_center","index":0,"x":0,"y":0}"#);
    assert_eq!(result["error"]["code"], "no_capture");
    assert!(
        capabilities["data"]["serve_commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|command| command == "texture_context")
    );
    let result =
        agent.send(r#"{"command":"texture_context","index":0,"x":0,"y":0,"path":"context.png"}"#);
    assert_eq!(result["error"]["code"], "no_capture");
    assert!(
        capabilities["data"]["serve_commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|command| command == "event_format")
    );
    let result = agent.send(r#"{"command":"event_format","event_id":1}"#);
    assert_eq!(result["error"]["code"], "no_capture");
    agent.quit();
}

#[test]
fn saved_event_filters_survive_restart_and_stay_separate_from_formats() {
    let dir = tempfile::tempdir().unwrap();
    let mut agent = Agent::with_config(None, false, Some(dir.path()));
    let saved = agent.send(
        r#"{"command":"event_filter_save","name":"筛选/../例","expression":"+$action() -Copy"}"#,
    );
    assert_eq!(saved["data"]["filter"]["expression"], "+$action() -Copy");
    assert_eq!(
        agent.send(r#"{"command":"event_filter_save","name":"empty","expression":""}"#)["ok"],
        true
    );
    assert_eq!(
        agent.send(r#"{"command":"buffer_formats_list"}"#)["data"]["formats"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    agent.quit();
    let mut restarted = Agent::with_config(None, false, Some(dir.path()));
    assert_eq!(
        restarted.send(r#"{"command":"event_filter_get","name":"筛选/../例"}"#)["data"]["filter"]["expression"],
        "+$action() -Copy"
    );
    assert_eq!(
        restarted.send(r#"{"command":"event_filters_list"}"#)["data"]["filters"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        restarted.send(r#"{"command":"event_filter_delete","name":"筛选/../例"}"#)["ok"],
        true
    );
    assert_eq!(
        restarted.send(r#"{"command":"event_filter_get","name":"筛选/../例"}"#)["error"]["code"],
        "event_filter_store_failed"
    );
    restarted.quit();
}

#[test]
fn custom_shader_library_round_trips_and_requires_explicit_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("upstream");
    std::fs::create_dir(&input).unwrap();
    std::fs::write(input.join("from-renderdoc.frag"), "void main() {}").unwrap();
    let mut agent = Agent::with_config(None, false, Some(dir.path()));
    let request =
        serde_json::json!({"command":"custom_shader_library_import_directory","path":input})
            .to_string();
    assert_eq!(agent.send(&request)["data"]["results"][0]["imported"], true);
    assert_eq!(
        agent.send(&request)["data"]["results"][0]["imported"],
        false
    );
    assert_eq!(
        agent.send(r#"{"command":"custom_shader_library_get","name":"from-renderdoc.frag"}"#)["data"]
            ["shader"]["encoding"],
        2
    );
    assert_eq!(
        agent.send(r#"{"command":"custom_shader_library_delete","name":"from-renderdoc.frag"}"#)["ok"],
        true
    );
    let mut shader = serde_json::json!({"name":"../着色器", "encoding":3, "entry":"main", "source_hex":"00ff12", "flags":[{"name":"opt","value":"1"}], "apply_prefix":false});
    let request = |shader: &Value, replace: bool| {
        serde_json::json!({"command":"custom_shader_library_save","shader":shader,"replace":replace}).to_string()
    };
    assert_eq!(agent.send(&request(&shader, false))["ok"], true);
    shader["entry"] = "changed".into();
    assert_eq!(
        agent.send(&request(&shader, false))["error"]["code"],
        "custom_shader_store_failed"
    );
    assert_eq!(
        agent.send(r#"{"command":"custom_shader_library_get","name":"../着色器"}"#)["data"]["shader"]
            ["entry"],
        "main"
    );
    assert_eq!(agent.send(&request(&shader, true))["ok"], true);
    assert_eq!(
        agent.send(r#"{"command":"custom_shader_library_build","name":"../着色器"}"#)["error"]["code"],
        "no_capture"
    );
    agent.quit();
    let mut agent = Agent::with_config(None, false, Some(dir.path()));
    assert_eq!(
        agent.send(r#"{"command":"custom_shader_library_list"}"#)["data"]["shaders"][0],
        shader
    );
    assert_eq!(
        agent.send(r#"{"command":"custom_shader_library_delete","name":"../着色器"}"#)["ok"],
        true
    );
    assert!(
        agent.send(r#"{"command":"custom_shader_library_list"}"#)["data"]["shaders"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    agent.quit();
}

impl Agent {
    fn new(bridge: Option<&str>) -> Self {
        Self::with_handoff_environment(bridge, false)
    }

    fn with_handoff_environment(bridge: Option<&str>, handoff: bool) -> Self {
        Self::with_config(bridge, handoff, None)
    }

    fn with_config(bridge: Option<&str>, handoff: bool, config: Option<&std::path::Path>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_renderdocx-cli"));
        if let Some(config) = config {
            command.env("RENDERDOCX_CONFIG_DIR", config);
        }
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

#[test]
fn saved_buffer_formats_survive_agent_restart_and_are_isolated() {
    let dir = tempfile::tempdir().unwrap();
    let mut agent = Agent::with_config(None, false, Some(dir.path()));
    let saved = agent.send(
        r#"{"command":"buffer_format_save","name":"纹理/../layout","layout":"float4 value;"}"#,
    );
    assert_eq!(saved["data"]["format"]["name"], "纹理/../layout", "{saved}");
    assert_eq!(
        agent.send(r#"{"command":"buffer_formats_list"}"#)["data"]["formats"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        agent.send(r#"{"command":"buffer_format_save","name":" ","layout":"uint x;"}"#)["ok"],
        false
    );
    agent.quit();
    let mut restarted = Agent::with_config(None, false, Some(dir.path()));
    assert_eq!(
        restarted.send(r#"{"command":"buffer_format_get","name":"纹理/../layout"}"#)["data"]["format"]
            ["layout"],
        "float4 value;"
    );
    assert_eq!(
        restarted
            .send(r#"{"command":"buffer_format_save","name":"纹理/../layout","layout":"uint x;"}"#)
            ["ok"],
        true
    );
    assert_eq!(
        restarted.send(r#"{"command":"buffer_format_delete","name":"纹理/../layout"}"#)["ok"],
        true
    );
    assert_eq!(
        restarted.send(r#"{"command":"buffer_formats_list"}"#)["data"]["formats"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    restarted.quit();
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
#[ignore = "requires the upstream Vulkan mesh demo, a GLSL compiler, and a matching Windows runtime"]
fn real_vulkan_target_launch_uses_packaged_layer() {
    use std::{
        os::windows::process::CommandExt,
        path::PathBuf,
        time::{Duration, Instant},
    };
    struct Stop(u32);
    impl Drop for Stop {
        fn drop(&mut self) {
            let _ = Command::new("taskkill")
                .args(["/PID", &self.0.to_string(), "/T", "/F"])
                .creation_flags(0x08000000)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
    let bridge = PathBuf::from(
        std::env::var_os("RENDERDOCX_TEST_BRIDGE").expect("set RENDERDOCX_TEST_BRIDGE"),
    );
    let demo = PathBuf::from(
        std::env::var_os("RENDERDOCX_TEST_VULKAN_MESH_DEMO")
            .expect("set RENDERDOCX_TEST_VULKAN_MESH_DEMO"),
    );
    assert!(
        bridge
            .parent()
            .unwrap()
            .join("renderdocx-vulkan.json")
            .is_file(),
        "packaged layer manifest missing"
    );
    let dir = tempfile::tempdir().unwrap();
    let mut agent = Agent::new(bridge.to_str());
    let mut arguments = "VK_Mesh_Shader --width 128 --height 128".to_owned();
    if let Ok(gpu) = std::env::var("RENDERDOCX_TEST_VULKAN_GPU") {
        assert!(
            gpu.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        arguments.push_str(&format!(" --gpu {gpu}"));
    }
    let launch = agent.send(&serde_json::json!({"command":"target_launch", "path":demo,
        "capture_directory":dir.path(), "configuration":{"arguments":arguments, "working_directory":demo.parent()}}).to_string());
    assert_eq!(launch["ok"], true, "{launch}");
    let _stop = Stop(launch["data"]["target"]["pid"].as_u64().unwrap() as u32);
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut ready = false;
    let mut observed = Vec::new();
    while Instant::now() < deadline && !ready {
        let reply = agent.send(r#"{"command":"target_events"}"#);
        assert_eq!(reply["data"]["connected"], true, "{reply}");
        observed.extend(reply["data"]["events"].as_array().unwrap().iter().cloned());
        ready = reply["data"]["events"].as_array().unwrap().iter().any(|e| {
            e["type"] == "api"
                && e["name"] == "Vulkan"
                && e["supported"] == true
                && e["presenting"] == true
        });
        if !ready {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    assert!(ready, "Vulkan target did not become ready: {observed:?}");
    assert_eq!(
        agent.send(r#"{"command":"target_trigger","frames":1}"#)["ok"],
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
            .find(|e| e["type"] == "capture")
            .cloned();
        if captured.is_none() {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let captured = captured.expect("target did not produce a capture");
    assert_eq!(captured["api"], "Vulkan");
    let opened =
        agent.send(&serde_json::json!({"command":"open", "path":captured["path"]}).to_string());
    assert_eq!(opened["ok"], true, "{opened}");
    assert!(
        opened["data"]["capture"]["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["flags"].as_u64().is_some_and(|f| f & 0x8 != 0)),
        "{opened}"
    );
    agent.quit();
}

#[test]
#[ignore = "requires an upstream VK_Mesh_Shader capture and a matching RenderDoc replay bridge"]
fn real_vulkan_mesh_shader_debug_trace() {
    let bridge = std::env::var("RENDERDOCX_TEST_BRIDGE").expect("set RENDERDOCX_TEST_BRIDGE");
    let capture = std::env::var("RENDERDOCX_TEST_VULKAN_MESH_CAPTURE")
        .expect("set RENDERDOCX_TEST_VULKAN_MESH_CAPTURE");
    let mut agent = Agent::new(Some(&bridge));
    let opened = agent.send(&serde_json::json!({"command":"open","path":capture}).to_string());
    assert_eq!(opened["ok"], true, "{opened}");
    let actions = opened["data"]["capture"]["actions"]
        .as_array()
        .expect("capture actions");
    let mesh = actions
        .iter()
        .find(|action| {
            action["flags"]
                .as_u64()
                .is_some_and(|flags| flags & 0x8 != 0)
        })
        .expect("capture must contain a mesh dispatch");
    let event = mesh["event_id"].as_u64().unwrap();
    let selected =
        agent.send(&serde_json::json!({"command":"select","event_id":event}).to_string());
    assert_eq!(selected["ok"], true, "{selected}");
    let mesh = agent.send(r#"{"command":"mesh","stage":"mesh_out","instance":0,"view":0}"#);
    assert_eq!(mesh["ok"], true, "{mesh}");
    let draw = agent.send(r#"{"command":"mesh_draw","offset":0,"count":12}"#);
    assert_eq!(draw["ok"], true, "{draw}");
    let rows = draw["data"]["page"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 12, "{draw}");
    assert_eq!(
        (rows[5]["meshlet"].as_u64(), rows[5]["meshlet_row"].as_u64()),
        (Some(0), Some(5))
    );
    assert_eq!(
        (rows[6]["meshlet"].as_u64(), rows[6]["meshlet_row"].as_u64()),
        (Some(1), Some(0))
    );
    assert_eq!(rows[6]["vertex"], 6);
    let color = rows[0]["attributes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "outColor")
        .expect("mesh color output");
    assert_eq!(
        color["values"],
        serde_json::json!(["1", "0", "0", "1"]),
        "{color}"
    );
    let filtered = agent.send(r#"{"command":"mesh_draw","offset":0,"count":64,"meshlet":1}"#);
    assert_eq!(filtered["ok"], true, "{filtered}");
    assert_eq!(filtered["data"]["page"]["total"], 6);
    assert_eq!(filtered["data"]["page"]["rows"][0]["row"], 6);
    assert_eq!(filtered["data"]["page"]["rows"][0]["meshlet"], 1);
    let invalid_filter =
        agent.send(r#"{"command":"mesh_draw","offset":0,"count":1,"meshlet":0,"task":0}"#);
    assert_eq!(invalid_filter["error"]["code"], "invalid_arguments");
    let first = rows[0]["position"].as_array().unwrap();
    assert!((first[0].as_f64().unwrap() + 0.85).abs() < 0.001);
    assert!((first[1].as_f64().unwrap() - 0.45).abs() < 0.001);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mesh.csv");
    let export =
        agent.send(&serde_json::json!({"command":"mesh_draw_export","path":path}).to_string());
    assert_eq!(export["ok"], true, "{export}");
    let csv = std::fs::read_to_string(&path).unwrap();
    assert!(
        csv.lines()
            .next()
            .unwrap()
            .ends_with(",meshlet,meshlet_row")
    );
    assert_eq!(csv.lines().count(), 13);
    let attributes_path = dir.path().join("attributes.csv");
    let attributes_export = agent.send(
        &serde_json::json!({"command":"mesh_attributes_export","path":attributes_path,"meshlet":1})
            .to_string(),
    );
    assert_eq!(attributes_export["ok"], true, "{attributes_export}");
    assert_eq!(attributes_export["data"]["rows"], 6);
    let attributes_csv = std::fs::read_to_string(&attributes_path).unwrap();
    assert!(attributes_csv.lines().next().unwrap().contains("outColor"));
    assert!(attributes_csv.contains("\"1 0 0 1\""));
    assert_eq!(attributes_csv.lines().count(), 7);
    let duplicate = agent.send(
        &serde_json::json!({"command":"mesh_attributes_export","path":attributes_path,"meshlet":1})
            .to_string(),
    );
    assert_eq!(duplicate["error"]["code"], "mesh_export_failed");
    assert_eq!(
        std::fs::read_to_string(&attributes_path).unwrap(),
        attributes_csv
    );
    let shader = agent.send(r#"{"command":"shader","stage":"mesh"}"#);
    assert_eq!(shader["ok"], true, "{shader}");
    let trace = agent.send(r#"{"command":"debug_mesh","group":[0,0,0],"thread":[0,0,0]}"#);
    assert_eq!(trace["ok"], true, "{trace}");
    assert_eq!(trace["data"]["trace"]["event_id"], event);
    assert_eq!(trace["data"]["trace"]["stage"], 7);
    let collected = agent.send(r#"{"command":"debug_collect"}"#);
    assert_eq!(collected["ok"], true, "{collected}");
    assert!(
        collected["data"]["total"].as_u64().is_some_and(|n| n > 1),
        "{collected}"
    );
    let stopped = agent.send(r#"{"command":"debug_stop"}"#);
    assert_eq!(stopped["ok"], true, "{stopped}");
    let task_event = actions
        .iter()
        .filter(|a| a["flags"].as_u64().is_some_and(|f| f & 8 != 0))
        .nth(1)
        .unwrap()["event_id"]
        .as_u64()
        .unwrap();
    assert_eq!(
        agent.send(&serde_json::json!({"command":"select","event_id":task_event}).to_string())["ok"],
        true
    );
    let task_mesh = agent.send(r#"{"command":"mesh","stage":"task_out","instance":0,"view":0}"#);
    assert_eq!(task_mesh["ok"], true, "{task_mesh}");
    let raw = agent.send(r#"{"command":"mesh_read","kind":"vertex","offset":"0","length":112}"#);
    assert!(
        raw["data"]["positions"].is_null(),
        "task payload must not be interpreted as positions: {raw}"
    );
    let payload = agent.send(r#"{"command":"task_payload","offset":0,"count":1}"#);
    assert_eq!(payload["ok"], true, "{payload}");
    let row = &payload["data"]["page"]["rows"][0];
    assert!(row["error"].is_null(), "{row}");
    assert_eq!(row["dispatch"], serde_json::json!([4, 1, 1]));
    let fields = row["fields"].as_array().unwrap();
    for (name, value) in [
        ("padArr[0]", "1000"),
        ("pad", "123"),
        ("inner.a", "500"),
        ("innerArr[3].c", "32"),
        ("tri[3]", "3"),
    ] {
        let field = fields
            .iter()
            .find(|field| field["name"] == name)
            .unwrap_or_else(|| panic!("missing {name}: {row}"));
        assert_eq!(field["values"][0], value, "{field}");
    }
    let mesh = agent.send(r#"{"command":"mesh","stage":"mesh_out","instance":0,"view":0}"#);
    assert_eq!(mesh["ok"], true, "{mesh}");
    let task_filtered = agent.send(r#"{"command":"mesh_draw","offset":0,"count":64,"task":0}"#);
    assert_eq!(task_filtered["ok"], true, "{task_filtered}");
    assert_eq!(task_filtered["data"]["page"]["total"], 12);
    assert_eq!(task_filtered["data"]["page"]["rows"][9]["meshlet"], 3);
    let path = dir.path().join("task-meshlet.csv");
    let export = agent.send(
        &serde_json::json!({"command":"mesh_draw_export","path":path,"meshlet":2}).to_string(),
    );
    assert_eq!(export["ok"], true, "{export}");
    assert_eq!(export["data"]["rows"], 3);
    assert_eq!(std::fs::read_to_string(path).unwrap().lines().count(), 4);
    agent.quit();
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
    let mut agent = Agent::with_config(bridge.to_str(), false, Some(dir.path()));
    let opened = agent.send(&serde_json::json!({"command":"open","path":capture}).to_string());
    assert_eq!(opened["ok"], true, "{opened}");
    let topology_event = opened["data"]["capture"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|action| action["api_events"].as_array().into_iter().flatten())
        .find(|event| {
            event["name"]
                .as_str()
                .is_some_and(|name| name.contains("IASetPrimitiveTopology"))
        })
        .expect("topology event")["event_id"]
        .as_u64()
        .unwrap();
    let structure = agent
        .send(&serde_json::json!({"command":"structure","event_id":topology_event}).to_string());
    let topology = structure["data"]["structure"]["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["name"] == "Topology")
        .unwrap();
    let settings = renderdocx::backend::Settings {
        replay_bridge: Some(bridge.clone()),
        ..Default::default()
    };
    let (session, _) =
        renderdocx::replay::Session::open(&settings, std::path::Path::new(capture)).unwrap();
    let names = vec!["Topology".to_owned(), "missing_parameter".to_owned()];
    let found = session
        .structure_find(topology_event as u32, &names)
        .unwrap()
        .expect("new bridge search must be available");
    assert_eq!(found[0].as_ref().unwrap().node.name, "Topology");
    assert!(found[1].is_none());
    drop(session);
    let value = topology["custom_string"].as_str().unwrap_or("4");
    let parameter_filter = agent.send(
        &serde_json::json!({"command":"filter_events","show_api":true,
        "expression":format!("$param(Topology: {value})")})
        .to_string(),
    );
    assert_eq!(parameter_filter["ok"], true, "{parameter_filter}");
    assert!(
        parameter_filter["data"]["events"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["event_id"] == topology_event && item["direct_match"] == true),
        "{parameter_filter}"
    );
    let combined = agent.send(
        &serde_json::json!({"command":"filter_events","show_api":true,
        "expression":format!("+$param(Topology: {value}) -$param(missing_parameter: x)")})
        .to_string(),
    );
    assert_eq!(combined["ok"], true, "{combined}");
    assert!(
        combined["data"]["events"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["event_id"] == topology_event && item["direct_match"] == true),
        "{combined}"
    );
    let shader_event = opened["data"]["capture"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|action| action["api_events"].as_array().into_iter().flatten())
        .find(|event| {
            event["name"]
                .as_str()
                .is_some_and(|name| name.contains("VSSetShader"))
        })
        .expect("vertex shader event")["event_id"]
        .as_u64()
        .unwrap();
    let shader_page =
        agent.send(&serde_json::json!({"command":"structure","event_id":shader_event}).to_string());
    let shader = shader_page["data"]["structure"]["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["name"] == "pShader")
        .unwrap();
    let shader_index = shader["resource_index"]
        .as_u64()
        .expect("vertex shader resource");
    assert_eq!(shader["resource_edited"], false);
    let rename = agent.send(&serde_json::json!({"command":"resource_rename","index":shader_index,"name":"筛选测试Shader"}).to_string());
    assert_eq!(rename["ok"], true, "{rename}");
    let shader_filter = agent.send(
        r#"{"command":"filter_events","expression":"$param(pShader: 测试shader)","show_api":true}"#,
    );
    assert_eq!(shader_filter["ok"], true, "{shader_filter}");
    assert!(
        shader_filter["data"]["events"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["event_id"] == shader_event && item["direct_match"] == true),
        "{shader_filter}"
    );
    let clear_event = opened["data"]["capture"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|action| action["api_events"].as_array().into_iter().flatten())
        .find(|event| {
            event["name"]
                .as_str()
                .is_some_and(|name| name.contains("ClearRenderTargetView"))
        })
        .expect("clear event")["event_id"]
        .as_u64()
        .unwrap();
    let color_filter = agent.send(
        r#"{"command":"filter_events","expression":"$param(ColorRGBA: 0.25)","show_api":true}"#,
    );
    assert_eq!(color_filter["ok"], true, "{color_filter}");
    assert!(
        color_filter["data"]["events"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["event_id"] == clear_event && item["direct_match"] == true),
        "{color_filter}"
    );
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
    let annotated = agent.send(r#"{"command":"filter_events","expression":"+$annot(renderdocx.pass contains PASS) +$annot(renderdocx.count >= 42)","show_api":true}"#);
    assert_eq!(annotated["ok"], true, "{annotated}");
    let raw_annotations = agent.send(&serde_json::json!({"command":"event_annotation_find","event_id":draw,"keys":["renderdocx.pass","renderdocx.count","renderdocx.missing"]}).to_string());
    assert_eq!(raw_annotations["ok"], true, "{raw_annotations}");
    assert_eq!(raw_annotations["data"]["values"][0]["value"], "First Pass");
    assert_eq!(raw_annotations["data"]["values"][1]["value"], "42");
    assert!(raw_annotations["data"]["values"][2].is_null());
    let column = agent.send(&serde_json::json!({"command":"filter_events","expression":format!("$event(EID={draw})"),"show_api":true,"annotation_key":"renderdocx.pass"}).to_string());
    assert_eq!(column["ok"], true, "{column}");
    assert!(
        column["data"]["events"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["event_id"] == draw && item["annotation"] == "First Pass"),
        "{column}"
    );
    assert!(
        annotated["data"]["events"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["event_id"] == draw && item["direct_match"] == true),
        "{annotated}"
    );
    for expression in [
        "$annot(renderdocx.pass == First Pass)",
        "$annot(renderdocx.pass =~ /Pass/)",
        "$annot(renderdocx.count)",
    ] {
        let result = agent.send(
            &serde_json::json!({"command":"filter_events","expression":expression,"show_api":true})
                .to_string(),
        );
        assert_eq!(result["ok"], true, "{result}");
        assert!(
            result["data"]["events"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["event_id"] == draw && item["direct_match"] == true),
            "{result}"
        );
    }
    let annotated_find = agent.send(&serde_json::json!({"command":"find_event","query":format!("@{draw}"),"filter":"$annot(renderdocx.count = 42)","show_api":true}).to_string());
    assert_eq!(
        annotated_find["data"]["match"]["event_id"], draw,
        "{annotated_find}"
    );
    let absent = agent.send(
        r#"{"command":"filter_events","expression":"$annot(renderdocx.missing)","show_api":true}"#,
    );
    assert_eq!(absent["ok"], true, "{absent}");
    assert!(
        !absent["data"]["events"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["direct_match"] == true)
    );
    let invalid_annotation = agent.send(
        r#"{"command":"filter_events","expression":"$annot(renderdocx.count >=)","show_api":true}"#,
    );
    assert_eq!(invalid_annotation["error"]["code"], "invalid_filter");
    let filtered = agent.send(&serde_json::json!({"command":"filter_events","expression":format!("$event(EID={draw})"),"show_api":true,"offset":"0","limit":128}).to_string());
    assert_eq!(filtered["ok"], true, "{filtered}");
    let visible = agent.send(&serde_json::json!({"command":"filter_events","expression":format!("$event(EID={draw})"),"show_empty_regions":false,"collapsed_events":[]}).to_string());
    assert_eq!(visible["ok"], true, "{visible}");
    assert!(
        visible["data"]["events"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["event_id"] == draw && item["collapsed"] == false),
        "{visible}"
    );
    let unknown = agent
        .send(r#"{"command":"filter_events","expression":"","collapsed_events":[4294967295]}"#);
    assert_eq!(unknown["error"]["code"], "invalid_arguments", "{unknown}");
    assert!(
        filtered["data"]["events"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["event_id"] == draw && row["direct_match"] == true),
        "{filtered}"
    );
    let all = agent.send(
        r#"{"command":"filter_events","expression":"","show_api":true,"offset":"0","limit":1}"#,
    );
    let total = all["data"]["events"]["total"]
        .as_str()
        .unwrap()
        .parse::<usize>()
        .unwrap();
    assert!(total > 1, "{all}");
    let next = agent.send(
        r#"{"command":"filter_events","expression":"","show_api":true,"offset":"1","limit":1}"#,
    );
    assert_eq!(next["data"]["events"]["offset"], "1", "{next}");
    assert_ne!(
        all["data"]["events"]["items"][0]["event_id"],
        next["data"]["events"]["items"][0]["event_id"]
    );
    let invalid_filter = agent.send(r#"{"command":"filter_events","expression":"$unknown()"}"#);
    assert_eq!(invalid_filter["error"]["code"], "invalid_filter");
    let found = agent.send(
        &serde_json::json!({"command":"find_event","query":format!("@{draw}"),"show_api":true})
            .to_string(),
    );
    assert_eq!(found["data"]["match"]["event_id"], draw, "{found}");
    let missing = agent.send(r#"{"command":"find_event","query":"surely no matching event name"}"#);
    assert!(missing["data"]["match"].is_null(), "{missing}");
    let event_path = dir.path().join("事件.txt");
    let request = serde_json::json!({"command":"export_events","path":event_path,"filter":format!("$event(EID={draw})")}).to_string();
    let exported = agent.send(&request);
    assert_eq!(exported["ok"], true, "{exported}");
    let text = std::fs::read_to_string(&event_path).unwrap();
    assert!(text.contains("DrawIndexed"), "{text}");
    let existing = agent.send(&request);
    assert_eq!(existing["error"]["code"], "event_export_failed");
    assert_eq!(std::fs::read_to_string(&event_path).unwrap(), text);
    let invalid_page =
        agent.send(r#"{"command":"filter_events","expression":"","offset":"99999999"}"#);
    assert_eq!(invalid_page["error"]["code"], "invalid_arguments");
    let selected = agent.send(&serde_json::json!({"command":"select","event_id":draw}).to_string());
    assert_eq!(selected["ok"], true, "{selected}");
    let stepped = agent.send(r#"{"command":"step_action","direction":"previous"}"#);
    assert_eq!(stepped["ok"], true, "{stepped}");
    if let Some(event) = stepped["data"]["action"]["event_id"].as_u64() {
        assert_ne!(event, draw);
        assert!(
            opened["data"]["capture"]["actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["event_id"] == event)
        );
    }
    let invalid_step = agent.send(r#"{"command":"step_action","direction":"sideways"}"#);
    assert_eq!(invalid_step["error"]["code"], "invalid_arguments");
    let mesh = agent.send(r#"{"command":"mesh","stage":"vs_out"}"#);
    assert_eq!(mesh["ok"], true, "{mesh}");
    let page = agent.send(r#"{"command":"mesh_draw","offset":0,"count":3}"#);
    assert_eq!(page["ok"], true, "{page}");
    assert_eq!(page["data"]["page"]["rows"][0]["vertex"], 0);
    assert_eq!(
        page["data"]["page"]["rows"][0]["position"],
        serde_json::json!([3.0, 3.0, 0.0, 1.0])
    );
    assert_eq!(
        page["data"]["page"]["rows"][0]["attributes"][0]["values"],
        serde_json::json!(["3", "3", "0", "1"])
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
    let render_path = dir.path().join("native-preview.png");
    let clear_selected =
        agent.send(&serde_json::json!({"command":"select","event_id":clear_event}).to_string());
    assert_eq!(clear_selected["ok"], true, "{clear_selected}");
    let render = agent.send(&serde_json::json!({"command":"render_texture","index":texture_index,"width":16,"height":16,"path":render_path,"display_range":[0.25,0.75]}).to_string());
    assert_eq!(render["ok"], true, "{render}");
    let mut rendered = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(&render_path).unwrap(),
    ))
    .read_info()
    .unwrap();
    assert_eq!((rendered.info().width, rendered.info().height), (16, 16));
    let mut rendered_pixels = vec![0; rendered.output_buffer_size().unwrap()];
    let rendered_info = rendered.next_frame(&mut rendered_pixels).unwrap();
    assert_eq!(rendered_info.color_type, png::ColorType::Rgb);
    let pixel = &rendered_pixels[(15 * 16 + 15) * 3..(15 * 16 + 15) * 3 + 3];
    assert!(
        pixel[0] <= 3 && pixel[1] > 100 && pixel[2] >= 250,
        "rendered pixel: {pixel:?}"
    );
    let duplicate = agent.send(&serde_json::json!({"command":"render_texture","index":texture_index,"width":16,"height":16,"path":render_path}).to_string());
    assert_eq!(duplicate["error"]["code"], "replay_render_failed");
    let invalid_render = agent.send(&serde_json::json!({"command":"render_texture","index":texture_index,"width":0,"height":16,"path":dir.path().join("invalid-render.png")}).to_string());
    assert_eq!(invalid_render["error"]["code"], "invalid_arguments");
    let reselected =
        agent.send(&serde_json::json!({"command":"select","event_id":scissor_draw}).to_string());
    assert_eq!(reselected["ok"], true, "{reselected}");
    let float_index = opened["data"]["capture"]["textures"]
        .as_array()
        .unwrap()
        .iter()
        .position(|texture| texture["format_name"] == "R32G32B32A32_FLOAT")
        .unwrap();
    let layered_index = opened["data"]["capture"]["textures"]
        .as_array()
        .unwrap()
        .iter()
        .position(|texture| {
            texture["width"] == 8
                && texture["height"] == 8
                && texture["array_size"] == 2
                && texture["mips"] == 2
        })
        .expect("layered texture");
    let grid_path = dir.path().join("slice-grid.png");
    let grid = agent.send(&serde_json::json!({"command":"export_texture","index":layered_index,"path":grid_path,"export":{"slices":"grid","grid_width":2}}).to_string());
    assert_eq!(grid["ok"], true, "{grid}");
    let mut grid_reader = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(&grid_path).unwrap(),
    ))
    .read_info()
    .unwrap();
    assert_eq!(
        (grid_reader.info().width, grid_reader.info().height),
        (16, 8)
    );
    let mut grid_data = vec![0; grid_reader.output_buffer_size().unwrap()];
    let grid_info = grid_reader.next_frame(&mut grid_data).unwrap();
    let components = match grid_info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        other => panic!("unexpected grid format {other:?}"),
    };
    assert!(
        grid_data[0] >= 250 && grid_data[1] <= 2 && grid_data[2] <= 2,
        "first grid pixel {:?}",
        &grid_data[..components]
    );
    let second = 8 * components;
    assert!(
        grid_data[second] <= 2 && grid_data[second + 1] <= 2 && grid_data[second + 2] >= 250,
        "second grid pixel {:?}",
        &grid_data[second..second + components]
    );
    let dds_path = dir.path().join("all-mips.dds");
    let dds = agent.send(&serde_json::json!({"command":"export_texture","index":layered_index,"path":dds_path,"format":"dds","export":{"all_mips":true,"slices":"all"}}).to_string());
    assert_eq!(dds["ok"], true, "{dds}");
    let dds_bytes = std::fs::read(&dds_path).unwrap();
    assert_eq!(&dds_bytes[..4], b"DDS ");
    assert_eq!(
        u32::from_le_bytes(dds_bytes[28..32].try_into().unwrap()),
        2,
        "DDS mip count"
    );
    let invalid_export = agent.send(&serde_json::json!({"command":"export_texture","index":layered_index,"path":dir.path().join("invalid-export.png"),"export":{"grid_width":0}}).to_string());
    assert_eq!(
        invalid_export["error"]["code"], "invalid_arguments",
        "{invalid_export}"
    );
    let strip_index = opened["data"]["capture"]["textures"]
        .as_array()
        .unwrap()
        .iter()
        .position(|texture| texture["dimension"] == 1 && texture["width"] == 16)
        .expect("1D strip");
    let bmp_path = dir.path().join("blended.bmp");
    let blended = agent.send(&serde_json::json!({"command":"export_texture","index":strip_index,"path":bmp_path,"format":"bmp","export":{"alpha":"blend_to_color","alpha_color":[0.0,0.0,1.0,1.0]}}).to_string());
    assert_eq!(blended["ok"], true, "{blended}");
    let bmp = std::fs::read(&bmp_path).unwrap();
    assert_eq!(&bmp[..2], b"BM");
    let bmp_offset = u32::from_le_bytes(bmp[10..14].try_into().unwrap()) as usize;
    assert!(
        (125..=128).contains(&bmp[bmp_offset])
            && bmp[bmp_offset + 1] <= 1
            && bmp[bmp_offset + 2] <= 1,
        "BMP blue blended pixel {:?}",
        &bmp[bmp_offset..bmp_offset + 3]
    );
    let low_path = dir.path().join("quality-low.jpg");
    let high_path = dir.path().join("quality-high.jpg");
    for (path, quality) in [(&low_path, 10), (&high_path, 100)] {
        let result = agent.send(&serde_json::json!({"command":"export_texture","index":layered_index,"path":path,"format":"jpg","export":{"jpeg_quality":quality}}).to_string());
        assert_eq!(result["ok"], true, "{result}");
    }
    let low = std::fs::read(&low_path).unwrap();
    let high = std::fs::read(&high_path).unwrap();
    assert!(low.starts_with(&[0xff, 0xd8]) && high.starts_with(&[0xff, 0xd8]));
    assert_ne!(low, high, "JPEG quality must affect encoding");
    let mapped_path = dir.path().join("mapped.png");
    let mapped = agent.send(&serde_json::json!({"command":"export_texture","index":float_index,"path":mapped_path,"display_range":[0.25,0.75]}).to_string());
    assert_eq!(mapped["ok"], true, "{mapped}");
    let mut reader = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(&mapped_path).unwrap(),
    ))
    .read_info()
    .unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    reader.next_frame(&mut pixels).unwrap();
    assert!(
        pixels[0] <= 2 && (126..=129).contains(&pixels[1]) && pixels[2] >= 253,
        "mapped pixel: {:?}",
        &pixels[..3]
    );
    let invalid_range = agent.send(&serde_json::json!({"command":"export_texture","index":float_index,"path":dir.path().join("invalid-range.png"),"display_range":[1.0,1.0]}).to_string());
    assert_eq!(
        invalid_range["error"]["code"], "invalid_arguments",
        "{invalid_range}"
    );
    let render_options = |agent: &mut Agent, path: &std::path::Path, extra: serde_json::Value| {
        let mut request = serde_json::json!({"command":"render_texture","index":float_index,"width":4,"height":4,"path":path,"preview_linear_as_gamma":false});
        request
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let response = agent.send(&request.to_string());
        assert_eq!(response["ok"], true, "{response}");
        let mut reader =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()))
                .read_info()
                .unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut pixels).unwrap();
        pixels
    };
    let normal = render_options(
        &mut agent,
        &dir.path().join("normal-options.png"),
        serde_json::json!({}),
    );
    let background = render_options(
        &mut agent,
        &dir.path().join("red-background.png"),
        serde_json::json!({"preview_scale":1.0,"preview_offset":[32.0,32.0],"preview_background":[1.0,0.0,0.0,1.0]}),
    );
    assert!(
        background[0] >= 250 && background[1] <= 2 && background[2] <= 2,
        "red background: {:?}",
        &background[..3]
    );
    let invalid_background = agent.send(&serde_json::json!({"command":"render_texture","index":float_index,"width":4,"height":4,"path":dir.path().join("invalid-background.png"),"preview_background":[1.1,0.0,0.0,1.0]}).to_string());
    assert_eq!(
        invalid_background["error"]["code"], "invalid_arguments",
        "{invalid_background}"
    );
    let depth_index = opened["data"]["capture"]["textures"]
        .as_array()
        .unwrap()
        .iter()
        .position(|texture| {
            texture["creation_flags"].as_u64().unwrap_or(0) & 4 != 0 && texture["format_type"] == 20
        })
        .expect("D24S8 depth texture");
    let picked_depth = agent.send(
        &serde_json::json!({"command":"pick_pixel","index":depth_index,"x":0,"y":0}).to_string(),
    );
    assert_eq!(picked_depth["ok"], true, "{picked_depth}");
    let value = &picked_depth["data"]["pixel"]["depth_stencil"];
    let depth = value["depth"].as_str().unwrap().parse::<f32>().unwrap();
    assert!((depth - 0.375).abs() < 0.00001, "{picked_depth}");
    assert_eq!(value["stencil"], 0x65, "{picked_depth}");
    let depth_path = dir.path().join("depth-preview.png");
    let depth_result = agent.send(&serde_json::json!({"command":"render_texture","index":depth_index,"width":16,"height":16,"path":depth_path,"preview_channels":[true,false,false,false],"preview_linear_as_gamma":false}).to_string());
    assert_eq!(depth_result["ok"], true, "{depth_result}");
    let stencil_path = dir.path().join("stencil-preview.png");
    let stencil_result = agent.send(&serde_json::json!({"command":"render_texture","index":depth_index,"width":16,"height":16,"path":stencil_path,"preview_channels":[false,true,false,false],"preview_linear_as_gamma":false}).to_string());
    assert_eq!(stencil_result["ok"], true, "{stencil_result}");
    let decode_preview = |path: &std::path::Path| {
        let mut reader =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()))
                .read_info()
                .unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut pixels).unwrap();
        pixels
    };
    let depth_pixels = decode_preview(&depth_path);
    let stencil_pixels = decode_preview(&stencil_path);
    assert_ne!(
        depth_pixels, stencil_pixels,
        "depth and stencil previews must differ"
    );
    let alpha_index = opened["data"]["capture"]["textures"]
        .as_array()
        .unwrap()
        .iter()
        .position(|texture| texture["format_type"] == 28)
        .expect("A8 texture");
    let alpha_pixel = agent.send(
        &serde_json::json!({"command":"pick_pixel","index":alpha_index,"x":0,"y":0}).to_string(),
    );
    assert_eq!(alpha_pixel["ok"], true, "{alpha_pixel}");
    let alpha = alpha_pixel["data"]["pixel"]["alpha"]
        .as_str()
        .unwrap()
        .parse::<f32>()
        .unwrap();
    assert!((alpha - 128.0 / 255.0).abs() < 0.00001, "{alpha_pixel}");
    let alpha_stats = agent.send(&serde_json::json!({"command":"texture_statistics","index":alpha_index,"channels":[false,false,false,true]}).to_string());
    assert_eq!(alpha_stats["ok"], true, "{alpha_stats}");
    let alpha_min = alpha_stats["data"]["statistics"]["min"]["float_values"][3]
        .as_str()
        .unwrap()
        .parse::<f32>()
        .unwrap();
    assert!((alpha_min - 128.0 / 255.0).abs() < 0.00001, "{alpha_stats}");
    let alpha_fit = agent
        .send(&serde_json::json!({"command":"texture_fit_range","index":alpha_index}).to_string());
    assert_eq!(alpha_fit["ok"], true, "{alpha_fit}");
    let alpha_black = alpha_fit["data"]["range"][0].as_f64().unwrap();
    assert!((alpha_black - 128.0 / 255.0).abs() < 0.00001, "{alpha_fit}");
    let alpha_default_stats = agent
        .send(&serde_json::json!({"command":"texture_statistics","index":alpha_index}).to_string());
    assert_eq!(
        alpha_default_stats["data"]["statistics"]["request"]["channels"],
        serde_json::json!([false, false, false, true]),
        "{alpha_default_stats}"
    );
    let alpha_path = dir.path().join("alpha-default.png");
    let alpha_result = agent.send(&serde_json::json!({"command":"render_texture","index":alpha_index,"width":8,"height":8,"path":alpha_path}).to_string());
    assert_eq!(alpha_result["ok"], true, "{alpha_result}");
    assert_eq!(
        alpha_result["data"]["display_selection"]["preview_channels"],
        serde_json::json!([false, false, false, true])
    );
    let alpha_explicit_path = dir.path().join("alpha-explicit.png");
    let alpha_explicit = agent.send(&serde_json::json!({"command":"render_texture","index":alpha_index,"width":8,"height":8,"path":alpha_explicit_path,"preview_channels":[false,false,false,true]}).to_string());
    assert_eq!(alpha_explicit["ok"], true, "{alpha_explicit}");
    assert_eq!(
        decode_preview(&alpha_path),
        decode_preview(&alpha_explicit_path)
    );
    let srgb_index = opened["data"]["capture"]["textures"]
        .as_array()
        .unwrap()
        .iter()
        .position(|texture| texture["component_type"] == 9 && texture["width"] == 8)
        .expect("sRGB texture");
    let srgb_on = dir.path().join("srgb-on.png");
    let srgb_off = dir.path().join("srgb-off.png");
    let srgb_on_result = agent.send(&serde_json::json!({"command":"render_texture","index":srgb_index,"width":8,"height":8,"path":srgb_on,"preview_linear_as_gamma":true}).to_string());
    let srgb_off_result = agent.send(&serde_json::json!({"command":"render_texture","index":srgb_index,"width":8,"height":8,"path":srgb_off,"preview_linear_as_gamma":false}).to_string());
    assert_eq!(srgb_on_result["ok"], true, "{srgb_on_result}");
    assert_eq!(srgb_off_result["ok"], true, "{srgb_off_result}");
    assert_eq!(
        srgb_on_result["data"]["display_selection"]["preview_linear_as_gamma"],
        false
    );
    assert_eq!(decode_preview(&srgb_on), decode_preview(&srgb_off));
    let ordinary_fit = agent
        .send(&serde_json::json!({"command":"texture_fit_range","index":float_index}).to_string());
    assert_eq!(ordinary_fit["ok"], true, "{ordinary_fit}");
    assert_eq!(ordinary_fit["data"]["histogram_computed"], false);
    assert!(
        ordinary_fit["data"]["statistics"]["histogram"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let ordinary_stats = agent
        .send(&serde_json::json!({"command":"texture_statistics","index":float_index}).to_string());
    assert_eq!(ordinary_stats["ok"], true, "{ordinary_stats}");
    assert!(
        !ordinary_stats["data"]["statistics"]["histogram"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let custom_info = agent.send(r#"{"command":"custom_shader_info"}"#);
    assert_eq!(custom_info["ok"], true, "{custom_info}");
    assert!(
        custom_info["data"]["info"]["encodings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == 5)
    );
    let shader = "float4 main(float4 pos : SV_Position, float4 uv : TEXCOORD0) : SV_Target0 { return float4(1, 0, 1, 1); }";
    let import_dir = dir.path().join("upstream-shaders");
    std::fs::create_dir(&import_dir).unwrap();
    std::fs::write(import_dir.join("imported.hlsl"), shader).unwrap();
    let imported = agent.send(
        &serde_json::json!({"command":"custom_shader_library_import_directory","path":import_dir})
            .to_string(),
    );
    assert_eq!(
        imported["data"]["results"][0]["imported"], true,
        "{imported}"
    );
    let imported_build =
        agent.send(r#"{"command":"custom_shader_library_build","name":"imported.hlsl"}"#);
    assert_eq!(
        imported_build["data"]["result"]["compiled"], true,
        "{imported_build}"
    );
    if let Some(executable) = std::env::var_os("RENDERDOCX_TEST_SHADER_PROCESSOR") {
        let processor = serde_json::json!({"executable":executable.to_string_lossy(),"input_encoding":4,"output_encoding":5,"arguments":["{stdin}"]});
        let request = serde_json::json!({"command":"custom_shader_build","encoding":4,"entry":"main","source":shader,"apply_prefix":false,"processor":processor});
        let processed = agent.send(&request.to_string());
        assert_eq!(processed["data"]["result"]["compiled"], true, "{processed}");
        assert!(
            processed["data"]["processor_diagnostics"]
                .as_str()
                .unwrap()
                .contains("fixture compiler diagnostics")
        );
        let record = serde_json::json!({"command":"custom_shader_library_save","shader":{"name":"processor shader","encoding":4,"entry":"main","source_hex":renderdocx::custom_shaders::encode_hex(shader.as_bytes()),"flags":[],"apply_prefix":false}});
        assert_eq!(agent.send(&record.to_string())["ok"], true);
        let library = agent.send(&serde_json::json!({"command":"custom_shader_library_build","name":"processor shader","processor":processor}).to_string());
        assert_eq!(library["data"]["result"]["compiled"], true, "{library}");
        let mut failure = request.clone();
        failure["processor"]["arguments"] = serde_json::json!(["--fail"]);
        let failure = agent.send(&failure.to_string());
        assert_eq!(
            failure["error"]["code"], "custom_shader_failed",
            "{failure}"
        );
        assert_eq!(
            agent.send(r#"{"command":"custom_shader_info"}"#)["data"]["info"]["active"],
            true
        );
        let mut invalid = request.clone();
        invalid["processor"]["output_encoding"] = 999.into();
        assert_eq!(
            agent.send(&invalid.to_string())["error"]["code"],
            "custom_shader_failed"
        );
    }
    let saved = agent.send(&serde_json::json!({"command":"custom_shader_library_save","shader":{"name":"fixture shader","encoding":5,"entry":"main","source_hex":renderdocx::custom_shaders::encode_hex(shader.as_bytes()),"flags":[],"apply_prefix":true}}).to_string());
    assert_eq!(saved["ok"], true, "{saved}");
    let library_build =
        agent.send(r#"{"command":"custom_shader_library_build","name":"fixture shader"}"#);
    assert_eq!(
        library_build["data"]["result"]["compiled"], true,
        "{library_build}"
    );
    let build = agent.send(&serde_json::json!({"command":"custom_shader_build","encoding":5,"entry":"main","source":shader}).to_string());
    assert_eq!(build["ok"], true, "{build}");
    assert_eq!(build["data"]["result"]["compiled"], true, "{build}");
    let custom = render_options(
        &mut agent,
        &dir.path().join("custom-shader.png"),
        serde_json::json!({"preview_custom_shader":true}),
    );
    assert!(
        custom[0] >= 250 && custom[1] <= 2 && custom[2] >= 250,
        "custom pixel {:?}",
        &custom[..3]
    );
    let fitted = agent.send(&serde_json::json!({"command":"texture_fit_range","index":float_index,"channels":[true,false,true,false],"preview_custom_shader":true}).to_string());
    assert_eq!(fitted["ok"], true, "{fitted}");
    assert_eq!(fitted["data"]["histogram_computed"], false);
    assert!(
        fitted["data"]["statistics"]["histogram"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(fitted["data"]["range"][0], 1.0);
    assert!(fitted["data"]["range"][1].as_f64().unwrap() > 1.0);
    let custom_stats = agent.send(&serde_json::json!({"command":"texture_statistics","index":float_index,"min":0.0,"max":2.0,"channels":[true,false,true,false],"preview_custom_shader":true}).to_string());
    assert_eq!(custom_stats["ok"], true, "{custom_stats}");
    assert_eq!(
        custom_stats["data"]["statistics"]["custom_shader"], true,
        "{custom_stats}"
    );
    assert_eq!(
        custom_stats["data"]["statistics"]["min"]["float_values"],
        serde_json::json!(["1", "0", "1", "1"])
    );
    assert!(
        custom_stats["data"]["statistics"]["histogram"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_u64().unwrap_or(0) > 0)
    );
    let single_index = opened["data"]["capture"]["textures"]
        .as_array()
        .unwrap()
        .iter()
        .position(|texture| {
            texture["dimension"] == 1 && texture["width"] == 8 && texture["component_count"] == 1
        })
        .expect("single-channel 1D texture");
    let custom_alpha_path = dir.path().join("custom-alpha.png");
    let custom_alpha = agent.send(&serde_json::json!({"command":"export_texture","index":single_index,"path":custom_alpha_path,"channel":3,"preview_custom_shader":true}).to_string());
    assert_eq!(custom_alpha["ok"], true, "{custom_alpha}");
    let mut alpha_reader = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(&custom_alpha_path).unwrap(),
    ))
    .read_info()
    .unwrap();
    let mut alpha_bytes = vec![0; alpha_reader.output_buffer_size().unwrap()];
    alpha_reader.next_frame(&mut alpha_bytes).unwrap();
    assert!(
        alpha_bytes[..3].iter().all(|value| *value >= 250),
        "custom alpha export pixel {:?}",
        &alpha_bytes[..3]
    );
    let custom_export_path = dir.path().join("custom-output.png");
    let custom_export = agent.send(&serde_json::json!({"command":"export_texture","index":float_index,"path":custom_export_path,"preview_custom_shader":true}).to_string());
    assert_eq!(custom_export["ok"], true, "{custom_export}");
    let mut custom_reader = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(&custom_export_path).unwrap(),
    ))
    .read_info()
    .unwrap();
    let mut custom_bytes = vec![0; custom_reader.output_buffer_size().unwrap()];
    custom_reader.next_frame(&mut custom_bytes).unwrap();
    assert!(
        custom_bytes[0] >= 250 && custom_bytes[1] <= 2 && custom_bytes[2] >= 250,
        "custom export pixel {:?}",
        &custom_bytes[..3]
    );
    let raw_custom_source = agent.send(
        &serde_json::json!({"command":"pick_pixel","index":float_index,"x":0,"y":0}).to_string(),
    );
    let custom_pixel = agent.send(&serde_json::json!({"command":"pick_custom_pixel","index":float_index,"x":0,"y":0,"preview_custom_shader":true}).to_string());
    assert_eq!(custom_pixel["ok"], true, "{custom_pixel}");
    assert_eq!(
        custom_pixel["data"]["pixel"]["bits"],
        raw_custom_source["data"]["pixel"]["bits"]
    );
    assert_eq!(
        custom_pixel["data"]["pixel"]["custom"]["float_values"],
        serde_json::json!(["1", "0", "1", "1"])
    );
    let captured_before = agent.send(
        &serde_json::json!({"command":"pick_pixel","index":texture_index,"x":0,"y":0}).to_string(),
    );
    let combined = agent.send(&serde_json::json!({"command":"pick_custom_pixel","index":texture_index,"x":0,"y":0,"preview_custom_shader":true,"preview_overlay":"quad_overdraw_draw"}).to_string());
    assert_eq!(combined["ok"], true, "{combined}");
    assert_eq!(combined["data"]["pixel"]["overlay"], true, "{combined}");
    assert_eq!(
        combined["data"]["pixel"]["custom"]["float_values"],
        serde_json::json!(["1", "0", "1", "1"])
    );
    let captured_after = agent.send(
        &serde_json::json!({"command":"pick_pixel","index":texture_index,"x":0,"y":0}).to_string(),
    );
    assert_eq!(
        captured_before["data"]["pixel"]["bits"], captured_after["data"]["pixel"]["bits"],
        "combined custom/overlay query must restore replay state"
    );
    let missing_selection = agent.send(
        &serde_json::json!({"command":"pick_custom_pixel","index":float_index,"x":0,"y":0})
            .to_string(),
    );
    assert_eq!(
        missing_selection["error"]["code"], "invalid_arguments",
        "{missing_selection}"
    );
    let failed = agent.send(&serde_json::json!({"command":"custom_shader_build","encoding":5,"entry":"main","source":"not a valid shader"}).to_string());
    assert_eq!(failed["ok"], true, "{failed}");
    assert_eq!(failed["data"]["result"]["compiled"], false, "{failed}");
    assert_eq!(
        failed["data"]["result"]["active"], true,
        "failed compile must retain prior shader: {failed}"
    );
    let retained = render_options(
        &mut agent,
        &dir.path().join("custom-retained.png"),
        serde_json::json!({"preview_custom_shader":true}),
    );
    assert_eq!(&retained[..3], &custom[..3]);
    let cleared = agent.send(r#"{"command":"custom_shader_clear"}"#);
    assert_eq!(cleared["data"]["result"]["active"], false, "{cleared}");
    let cleared_stats = agent.send(&serde_json::json!({"command":"texture_statistics","index":float_index,"preview_custom_shader":true}).to_string());
    assert_eq!(
        cleared_stats["error"]["code"], "replay_statistics_failed",
        "{cleared_stats}"
    );
    let missing_export_path = dir.path().join("custom-missing-export.png");
    let cleared_export = agent.send(&serde_json::json!({"command":"export_texture","index":float_index,"path":missing_export_path,"preview_custom_shader":true}).to_string());
    assert_eq!(
        cleared_export["error"]["code"], "replay_export_failed",
        "{cleared_export}"
    );
    assert!(!missing_export_path.exists());
    let cleared_pixel = agent.send(&serde_json::json!({"command":"pick_custom_pixel","index":float_index,"x":0,"y":0,"preview_custom_shader":true}).to_string());
    assert_eq!(
        cleared_pixel["error"]["code"], "replay_pixel_failed",
        "{cleared_pixel}"
    );
    let missing = agent.send(&serde_json::json!({"command":"render_texture","index":float_index,"width":4,"height":4,"path":dir.path().join("custom-missing.png"),"preview_custom_shader":true}).to_string());
    assert_eq!(
        missing["error"]["code"], "replay_render_failed",
        "{missing}"
    );
    let rgbm = render_options(
        &mut agent,
        &dir.path().join("rgbm-options.png"),
        serde_json::json!({"preview_hdr_multiplier":0.25}),
    );
    assert_ne!(
        &normal[..3],
        &rgbm[..3],
        "RGBM must affect the rendered image"
    );
    let _yuv_request = render_options(
        &mut agent,
        &dir.path().join("yuv-request.png"),
        serde_json::json!({"preview_decode_yuv":true}),
    );
    let invalid_mode = agent.send(&serde_json::json!({"command":"render_texture","index":float_index,"width":4,"height":4,"path":dir.path().join("invalid-mode.png"),"preview_hdr_multiplier":2.0,"preview_decode_yuv":true}).to_string());
    assert_eq!(
        invalid_mode["error"]["code"], "invalid_arguments",
        "{invalid_mode}"
    );
    let invalid_multiplier = agent.send(&serde_json::json!({"command":"render_texture","index":float_index,"width":4,"height":4,"path":dir.path().join("invalid-multiplier.png"),"preview_hdr_multiplier":-2.0}).to_string());
    assert_eq!(
        invalid_multiplier["error"]["code"], "invalid_arguments",
        "{invalid_multiplier}"
    );
    let flipped = render_options(
        &mut agent,
        &dir.path().join("flipped-options.png"),
        serde_json::json!({"preview_flip_y":true}),
    );
    assert_ne!(
        &normal[..3],
        &normal[36..39],
        "fixture must vary vertically"
    );
    assert_eq!(
        &normal[..12],
        &flipped[36..48],
        "vertical flip must reverse the rows"
    );
    let masked = render_options(
        &mut agent,
        &dir.path().join("masked-options.png"),
        serde_json::json!({"preview_channels":[true,false,true,false]}),
    );
    assert!(
        masked[0] > 0 && masked[1] <= 1 && masked[2] > 0,
        "masked pixel {:?}",
        &masked[..3]
    );
    let gamma = render_options(
        &mut agent,
        &dir.path().join("gamma-options.png"),
        serde_json::json!({"preview_linear_as_gamma":true}),
    );
    assert_ne!(gamma[1], normal[1], "gamma display must change mid-grey");
    let before_overlay = agent.send(
        &serde_json::json!({"command":"pick_pixel","index":texture_index,"x":15,"y":15})
            .to_string(),
    );
    assert_eq!(before_overlay["ok"], true, "{before_overlay}");
    let clear_overlay_path = dir.path().join("clear-before-overlay.png");
    let clear_overlay = agent.send(&serde_json::json!({"command":"render_texture","index":texture_index,"width":16,"height":16,"path":clear_overlay_path,"preview_overlay":"clear_before_draw"}).to_string());
    assert_eq!(clear_overlay["ok"], true, "{clear_overlay}");
    let after_overlay = agent.send(
        &serde_json::json!({"command":"pick_pixel","index":texture_index,"x":15,"y":15})
            .to_string(),
    );
    assert_eq!(
        after_overlay["data"]["pixel"]["bits"], before_overlay["data"]["pixel"]["bits"],
        "overlay must restore the selected replay state: {after_overlay}"
    );
    let base_path = dir.path().join("overlay-baseline.png");
    let baseline = agent.send(&serde_json::json!({"command":"render_texture","index":texture_index,"width":16,"height":16,"path":base_path}).to_string());
    assert_eq!(baseline["ok"], true, "{baseline}");
    let zoom_path = dir.path().join("zoom-preview.png");
    let zoom = agent.send(&serde_json::json!({"command":"render_texture","index":texture_index,"width":16,"height":16,"path":zoom_path,"preview_scale":2.0,"preview_offset":[4.0,0.0]}).to_string());
    assert_eq!(zoom["ok"], true, "{zoom}");
    assert_ne!(
        std::fs::read(&base_path).unwrap(),
        std::fs::read(&zoom_path).unwrap(),
        "zoom and pan must affect the rendered image"
    );
    let invalid_zoom = agent.send(&serde_json::json!({"command":"render_texture","index":texture_index,"width":16,"height":16,"path":dir.path().join("invalid-zoom.png"),"preview_scale":0.0}).to_string());
    assert_eq!(
        invalid_zoom["error"]["code"], "invalid_arguments",
        "{invalid_zoom}"
    );
    let draw_path = dir.path().join("draw-overlay.png");
    let draw_overlay = agent.send(&serde_json::json!({"command":"render_texture","index":texture_index,"width":16,"height":16,"path":draw_path,"preview_overlay":"drawcall"}).to_string());
    assert_eq!(draw_overlay["ok"], true, "{draw_overlay}");
    assert_ne!(
        std::fs::read(&base_path).unwrap(),
        std::fs::read(&draw_path).unwrap(),
        "draw overlay must affect the rendered image"
    );
    let raw_before = agent.send(
        &serde_json::json!({"command":"pick_pixel","index":texture_index,"x":0,"y":0}).to_string(),
    );
    assert_eq!(raw_before["ok"], true, "{raw_before}");
    let overlay_pixel = agent.send(&serde_json::json!({"command":"pick_overlay_pixel","index":texture_index,"x":0,"y":0,"preview_overlay":"quad_overdraw_draw"}).to_string());
    assert_eq!(overlay_pixel["ok"], true, "{overlay_pixel}");
    assert_eq!(
        overlay_pixel["data"]["pixel"]["overlay"], true,
        "{overlay_pixel}"
    );
    assert_ne!(
        overlay_pixel["data"]["pixel"]["bits"], raw_before["data"]["pixel"]["bits"],
        "overlay must return distinct pixel values"
    );
    let raw_after = agent.send(
        &serde_json::json!({"command":"pick_pixel","index":texture_index,"x":0,"y":0}).to_string(),
    );
    assert_eq!(
        raw_after["data"]["pixel"]["bits"], raw_before["data"]["pixel"]["bits"],
        "overlay picking must restore replay state"
    );
    let overlay_fallback = agent.send(&serde_json::json!({"command":"pick_overlay_pixel","index":float_index,"x":0,"y":0,"preview_overlay":"quad_overdraw_draw"}).to_string());
    assert_eq!(overlay_fallback["ok"], true, "{overlay_fallback}");
    assert_eq!(
        overlay_fallback["data"]["pixel"]["overlay"], false,
        "{overlay_fallback}"
    );
    let invalid_overlay_pixel = agent.send(&serde_json::json!({"command":"pick_overlay_pixel","index":texture_index,"x":0,"y":0,"preview_overlay":"drawcall"}).to_string());
    assert_eq!(
        invalid_overlay_pixel["error"]["code"], "invalid_arguments",
        "{invalid_overlay_pixel}"
    );
    let unsupported_overlay = agent.send(&serde_json::json!({"command":"render_texture","index":float_index,"width":4,"height":4,"path":dir.path().join("missing-overlay.png"),"preview_overlay":"drawcall"}).to_string());
    assert_eq!(
        unsupported_overlay["error"]["code"], "replay_render_failed",
        "{unsupported_overlay}"
    );
    let invalid_overlay = agent.send(&serde_json::json!({"command":"render_texture","index":texture_index,"width":16,"height":16,"path":dir.path().join("invalid-overlay.png"),"preview_overlay":"unknown"}).to_string());
    assert_eq!(
        invalid_overlay["error"]["code"], "invalid_arguments",
        "{invalid_overlay}"
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
    let buffer_index = opened["data"]["capture"]["buffers"]
        .as_array()
        .unwrap()
        .iter()
        .position(|buffer| buffer["length"] == "12")
        .unwrap();
    let raw = agent.send(&serde_json::json!({"command":"read_buffer", "index":buffer_index, "offset":"0", "length":4}).to_string());
    let hex = raw["data"]["hex"].as_str().unwrap();
    let bytes: Vec<u8> = hex
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    let header = u32::from_le_bytes(bytes.try_into().unwrap()).to_string();
    let later = agent.send(&serde_json::json!({"command":"decode_buffer_layout", "index":buffer_index, "origin":"0", "offset":"4", "length":8, "include_fixed":true, "layout":"uint header; uint2 values[];"}).to_string());
    assert_eq!(
        later["data"]["page"]["fixed"][0]["values"][0][0], header,
        "{later}"
    );
    assert_eq!(later["data"]["page"]["rows"][0]["offset"], "4", "{later}");
    let nested = agent.send(&serde_json::json!({"command":"decode_buffer_layout", "index":buffer_index, "origin":"0", "offset":"4", "length":8, "include_fixed":true, "layout":"struct Payload { uint header; uint2 values[]; }; Payload root;"}).to_string());
    assert_eq!(
        nested["data"]["page"]["fixed"][0]["name"], "root.header",
        "{nested}"
    );
    assert_eq!(
        nested["data"]["page"]["fixed"][0]["values"][0][0], header,
        "{nested}"
    );
    assert_eq!(
        nested["data"]["page"]["rows"][0]["fields"][0]["name"], "root.values",
        "{nested}"
    );
    let fixed_csv = dir.path().join("fixed.csv");
    let exported = agent.send(&serde_json::json!({"command":"export_buffer_csv", "index":buffer_index, "offset":"0", "length":"12", "scope":"fixed", "layout":"struct Payload { uint header; uint2 values[]; }; Payload root;", "path":fixed_csv}).to_string());
    assert_eq!(exported["data"]["summary"]["rows"], "1", "{exported}");
    assert_eq!(
        std::fs::read_to_string(fixed_csv).unwrap(),
        format!("Byte Offset,root.header\n0,{header}\n")
    );
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
    let dds = dir.path().join("gl-texture.dds");
    let saved = agent.send(&serde_json::json!({"command":"export_texture", "index":texture, "path":dds, "format":"dds"}).to_string());
    assert_eq!(saved["ok"], true, "{saved}");
    assert!(std::fs::read(dds).unwrap().starts_with(b"DDS "));
    let raw = dir.path().join("gl.raw");
    let exported = agent.send(
        &serde_json::json!({"command":"export_texture_raw", "index":texture, "path":raw})
            .to_string(),
    );
    assert_eq!(exported["ok"], true, "{exported}");
    assert_eq!(
        std::fs::read(raw).unwrap(),
        [
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255
        ]
    );
    let first = agent.send(&serde_json::json!({"command":"read_texture_raw", "index":texture, "offset":"0", "length":8}).to_string());
    assert_eq!(first["data"]["page"]["hex"], "ff0000ff00ff00ff", "{first}");
    let last = agent.send(&serde_json::json!({"command":"read_texture_raw", "index":texture, "offset":"8", "length":16}).to_string());
    assert_eq!(last["data"]["page"]["hex"], "0000ffffffff00ff", "{last}");
    let found = agent.send(&serde_json::json!({"command":"find_texture_raw", "index":texture, "offset":"0", "hex":"00 ff 00 ff"}).to_string());
    assert_eq!(found["data"]["offset"], "2", "{found}");
    let overlapping = agent.send(&serde_json::json!({"command":"find_texture_raw", "index":texture, "offset":"3", "hex":"00 ff 00 ff"}).to_string());
    assert_eq!(overlapping["data"]["offset"], "4", "{overlapping}");
    let missing = agent.send(&serde_json::json!({"command":"find_texture_raw", "index":texture, "offset":"0", "length":"3", "hex":"00 ff 00 ff"}).to_string());
    assert!(missing["data"]["offset"].is_null(), "{missing}");
    let invalid = agent.send(&serde_json::json!({"command":"find_texture_raw", "index":texture, "offset":"15", "length":"2", "hex":"00"}).to_string());
    assert_eq!(
        invalid["error"]["code"], "raw_texture_search_failed",
        "{invalid}"
    );
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
    assert_eq!(bottom["data"]["pixel"]["coordinates"]["mip_height"], 2);
    assert_eq!(
        bottom["data"]["pixel"]["coordinates"]["normalized"],
        serde_json::json!([0.0, 0.5])
    );
    let centered = agent.send(&serde_json::json!({"command":"texture_center","index":texture,"uv":[0.1,0.75],"width":64,"height":64,"preview_flip_y":true}).to_string());
    assert_eq!(centered["ok"], true, "{centered}");
    assert_eq!(centered["data"]["texel"], serde_json::json!([0, 1]));
    assert_eq!(centered["data"]["viewport"], serde_json::json!([64, 64]));
    let center_path = dir.path().join("gl-centered.png");
    let mut centered_render = centered["data"]["selection"].clone();
    centered_render.as_object_mut().unwrap().extend(serde_json::json!({"command":"render_texture","index":texture,"width":64,"height":64,"path":center_path,"preview_linear_as_gamma":false}).as_object().unwrap().clone());
    let rendered_center = agent.send(&centered_render.to_string());
    assert_eq!(rendered_center["ok"], true, "{rendered_center}");
    let mut reader = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(center_path).unwrap(),
    ))
    .read_info()
    .unwrap();
    let mut center_pixels = vec![0; reader.output_buffer_size().unwrap()];
    reader.next_frame(&mut center_pixels).unwrap();
    assert_eq!(
        &center_pixels[(32 * 64 + 32) * 3..][..3],
        &[255, 0, 0],
        "centered OpenGL texel"
    );
    let context_path = dir.path().join("gl-context.png");
    let request = serde_json::json!({"command":"texture_context","index":texture,"uv":[0.1,0.75],"preview_flip_y":true,"preview_linear_as_gamma":false,"path":context_path});
    let context = agent.send(&request.to_string());
    assert_eq!(context["ok"], true, "{context}");
    assert_eq!(context["data"]["highlight_rendered"], true);
    assert_eq!(
        context["data"]["geometry"]["texel"],
        serde_json::json!([0, 1])
    );
    assert_eq!(
        context["data"]["geometry"]["selection"]["preview_offset"],
        serde_json::json!([80.0, 80.0])
    );
    let mut reader = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(&context_path).unwrap(),
    ))
    .read_info()
    .unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    reader.next_frame(&mut pixels).unwrap();
    assert_eq!(&pixels[(84 * 160 + 84) * 3..][..3], &[255, 0, 0]);
    assert_eq!(&pixels[(80 * 160 + 80) * 3..][..3], &[255, 255, 255]);
    assert_eq!(&pixels[(79 * 160 + 79) * 3..][..3], &[0, 0, 0]);
    let repeat = agent.send(&request.to_string());
    assert_eq!(repeat["error"]["code"], "replay_texture_render_failed");
    let clamped = agent.send(
        &serde_json::json!({"command":"texture_center","index":texture,"x":99,"y":99}).to_string(),
    );
    assert_eq!(
        clamped["data"]["texel"],
        serde_json::json!([1, 1]),
        "{clamped}"
    );
    let invalid = agent.send(
        &serde_json::json!({"command":"texture_center","index":texture,"uv":[1.0,0.0]}).to_string(),
    );
    assert_eq!(invalid["error"]["code"], "invalid_arguments");
    let ambiguous = agent.send(
        &serde_json::json!({"command":"texture_center","index":texture,"x":0,"y":0,"uv":[0.0,0.0]})
            .to_string(),
    );
    assert_eq!(ambiguous["error"]["code"], "invalid_arguments");
    let uv_pick = agent.send(
        &serde_json::json!({"command":"pick_pixel","index":texture,"uv":[0.1,0.75]}).to_string(),
    );
    assert_eq!(uv_pick["ok"], true, "{uv_pick}");
    assert_eq!(uv_pick["data"]["pixel"]["x"], 0);
    assert_eq!(uv_pick["data"]["pixel"]["y"], 1);
    assert_eq!(
        uv_pick["data"]["pixel"]["float_values"],
        serde_json::json!(["1", "0", "0", "1"])
    );
    let invalid_uv = agent.send(
        &serde_json::json!({"command":"pick_pixel","index":texture,"uv":[1.0,0.0]}).to_string(),
    );
    assert_eq!(invalid_uv["error"]["code"], "invalid_arguments");
    let ambiguous = agent.send(
        &serde_json::json!({"command":"pick_pixel","index":texture,"x":0,"y":0,"uv":[0.0,0.0]})
            .to_string(),
    );
    assert_eq!(ambiguous["error"]["code"], "invalid_arguments");
    let preview_path = dir.path().join("gl-preview.png");
    let rendered = agent.send(&serde_json::json!({"command":"render_texture","index":texture,"width":2,"height":2,"path":preview_path,"preview_linear_as_gamma":false}).to_string());
    assert_eq!(rendered["ok"], true, "{rendered}");
    let read_preview = |path: &std::path::Path| {
        let mut reader =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()))
                .read_info()
                .unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut pixels).unwrap();
        pixels
    };
    let preview = read_preview(&preview_path);
    assert_eq!(
        &preview[..3],
        &[0, 0, 255],
        "OpenGL preview top should match top-left PickPixel: {preview:?}"
    );
    assert_eq!(
        &preview[6..9],
        &[255, 0, 0],
        "OpenGL preview bottom should match bottom-left PickPixel: {preview:?}"
    );
    let flip_path = dir.path().join("gl-preview-flipped.png");
    let flipped = agent.send(&serde_json::json!({"command":"render_texture","index":texture,"width":2,"height":2,"path":flip_path,"preview_linear_as_gamma":false,"preview_flip_y":true}).to_string());
    assert_eq!(flipped["ok"], true, "{flipped}");
    let preview = read_preview(&flip_path);
    assert_eq!(
        &preview[..3],
        &[255, 0, 0],
        "OpenGL flip should display bottom at top: {preview:?}"
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
    assert_eq!(value["task_payload_page_limit"], 32);
    for command in [
        "event_filters_list",
        "event_filter_get",
        "event_filter_save",
        "event_filter_delete",
        "custom_shader_library_import_directory",
        "custom_shader_library_list",
        "custom_shader_library_get",
        "custom_shader_library_save",
        "custom_shader_library_delete",
        "custom_shader_library_build",
    ] {
        assert!(
            value["serve_commands"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry == command)
        );
    }
    assert!(
        value["serve_commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "mesh_attributes_export")
    );
    assert!(
        value["serve_commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "task_payload")
    );
    assert!(
        value["serve_commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "debug_mesh")
    );
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
        "debug_mesh",
        "task_payload",
        "mesh_attributes_export",
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
#[ignore = "requires built D3D11 fixture and matching RenderDoc replay bridge"]
fn real_d3d11_nested_markers_resolve_and_replay_effective_events() {
    use std::{os::windows::process::CommandExt, path::PathBuf};
    let bridge = PathBuf::from(
        std::env::var_os("RENDERDOCX_TEST_BRIDGE").expect("set RENDERDOCX_TEST_BRIDGE"),
    );
    let fixture = PathBuf::from(
        std::env::var_os("RENDERDOCX_TEST_FIXTURE").expect("set RENDERDOCX_TEST_FIXTURE"),
    );
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(fixture)
        .arg(bridge.parent().unwrap().join("renderdoc.dll"))
        .arg(dir.path().join("markers"))
        .arg("--markers")
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
    let mut agent = Agent::new(Some(bridge.to_str().unwrap()));
    let opened = agent.send(&serde_json::json!({"command":"open","path":capture}).to_string());
    assert_eq!(opened["ok"], true, "{opened}");
    let actions = opened["data"]["capture"]["actions"].as_array().unwrap();
    let last_draw = actions
        .iter()
        .filter(|a| {
            a["name"]
                .as_str()
                .is_some_and(|n| n.contains("DrawIndexed"))
        })
        .last()
        .unwrap()["event_id"]
        .as_u64()
        .unwrap();
    for (path, expected) in [
        ("RenderDocX Outer", last_draw + 1),
        ("RenderDocX Outer/RenderDocX Inner", last_draw),
    ] {
        let result =
            agent.send(&serde_json::json!({"command":"event_path","path":path}).to_string());
        assert_eq!(result["ok"], true, "{result}");
        let selected = result["data"]["event_id"].as_u64().unwrap();
        let effective = result["data"]["effective_event_id"].as_u64().unwrap();
        assert_ne!(selected, effective);
        assert_eq!(effective, expected, "{result}");
        let replay =
            agent.send(&serde_json::json!({"command":"select","event_id":effective}).to_string());
        assert_eq!(replay["ok"], true, "{replay}");
        assert_eq!(replay["data"]["pipeline"]["event_id"], effective);
    }
    agent.quit();
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
    let mut start_agent = Agent::new(Some(bridge));
    let start_open =
        start_agent.send(&serde_json::json!({"command":"open","path":capture}).to_string());
    assert_eq!(start_open["ok"], true, "{start_open}");
    let start = start_agent.send(r#"{"command":"select","event_id":0}"#);
    assert_eq!(start["ok"], true, "{start}");
    assert_eq!(start["data"]["pipeline"]["event_id"], 0);
    let filtered_start = start_agent.send(
        r#"{"command":"filter_events","expression":"no-such-event","include_capture_start":true}"#,
    );
    assert_eq!(
        filtered_start["data"]["events"]["total"], "1",
        "{filtered_start}"
    );
    assert_eq!(
        filtered_start["data"]["events"]["items"][0]["capture_start"],
        true
    );
    let first_page = start_agent.send(r#"{"command":"filter_events","expression":"","include_capture_start":true,"collapsed_events":[0],"limit":1}"#);
    assert_eq!(first_page["data"]["events"]["items"][0]["event_id"], 0);
    let second_page = start_agent.send(r#"{"command":"filter_events","expression":"","include_capture_start":true,"offset":"1","limit":1}"#);
    assert_ne!(second_page["data"]["events"]["items"][0]["event_id"], 0);
    let forward_start = start_agent.send(
        r#"{"command":"step_action","event_id":0,"direction":"next","include_capture_start":true}"#,
    );
    assert_eq!(forward_start["ok"], true, "{forward_start}");
    let first_event = forward_start["data"]["action"]["event_id"]
        .as_u64()
        .unwrap();
    let action_path = start_agent
        .send(&serde_json::json!({"command":"event_path","event_id":first_event}).to_string());
    assert_eq!(action_path["ok"], true, "{action_path}");
    let path = action_path["data"]["path"].as_str().unwrap();
    let resolved =
        start_agent.send(&serde_json::json!({"command":"event_path","path":path}).to_string());
    assert_eq!(resolved["data"]["event_id"], first_event, "{resolved}");
    let root = start_agent.send(r#"{"command":"event_path","path":"/"}"#);
    assert_eq!(root["data"]["event_id"], 0);
    assert_eq!(root["data"]["effective_event_id"], 0);
    let missing = start_agent.send(r#"{"command":"event_path","path":"not-a-real-action"}"#);
    assert_eq!(missing["ok"], false);
    let previous = start_agent.send(&serde_json::json!({"command":"step_action","event_id":first_event,"direction":"previous","include_capture_start":true}).to_string());
    assert_eq!(
        previous["data"]["action"]["capture_start"], true,
        "{previous}"
    );
    let found_start = start_agent
        .send(r#"{"command":"find_event","query":"Capture Start","include_capture_start":true}"#);
    assert_eq!(
        found_start["data"]["match"]["capture_start"], true,
        "{found_start}"
    );
    let explicit_zero =
        start_agent.send(r#"{"command":"find_event","query":"@0","include_capture_start":true}"#);
    assert!(explicit_zero["data"]["match"].is_null());
    let start_export_path = dir.path().join("capture-start-excluded.txt");
    let start_export = start_agent
        .send(&serde_json::json!({"command":"export_events","path":start_export_path}).to_string());
    assert_eq!(start_export["ok"], true, "{start_export}");
    assert!(
        !std::fs::read_to_string(start_export_path)
            .unwrap()
            .contains("Capture Start")
    );
    start_agent.quit();
    let buffers = result["capture"]["buffers"].as_array().unwrap();
    assert!(!buffers.is_empty());
    assert!(buffers.iter().all(|buffer| {
        buffer["gpu_address"]
            .as_str()
            .is_some_and(|value| value.parse::<u64>().is_ok())
    }));
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
    if counters.iter().any(|c| {
        c["id"] == 1 && c["result_type"] == 1 && c["result_byte_width"] == 8 && c["unit"] == 1
    }) {
        let timing = agent.send(r#"{"command":"event_times"}"#);
        assert_eq!(timing["ok"], true, "{timing}");
        assert!(
            timing["data"]["event_times"]["count"]
                .as_str()
                .unwrap()
                .parse::<usize>()
                .unwrap()
                > 0,
            "{timing}"
        );
        let filtered = agent.send(
            r#"{"command":"filter_events","expression":"$action(duration >= 0ns)","limit":128}"#,
        );
        assert_eq!(filtered["ok"], true, "{filtered}");
        assert!(
            filtered["data"]["events"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["direct_match"] == true),
            "{filtered}"
        );
        for row in filtered["data"]["events"]["items"].as_array().unwrap() {
            if row["direct_match"] == true {
                assert!(
                    row["duration_seconds"]
                        .as_f64()
                        .is_some_and(|value| value >= 0.0),
                    "{row}"
                );
            }
        }
    }
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
    let full_usage = agent.send(
        &serde_json::json!({"command":"timeline_usage", "index":vertex_resource}).to_string(),
    );
    assert_eq!(full_usage["ok"], true, "{full_usage}");
    assert_eq!(full_usage["data"]["usage"]["tracked"], true);
    for (full, page) in full_usage["data"]["usage"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .zip(usage["data"]["entries"].as_array().unwrap())
    {
        assert_eq!(full["event_id"], page["event_id"]);
        assert_eq!(full["usage"], page["usage"]);
    }
    assert!(
        full_usage["data"]["usage"]["entries"]
            .as_array()
            .unwrap()
            .len()
            >= usage["data"]["entries"].as_array().unwrap().len()
    );
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
    let formatted = agent.send(&serde_json::json!({"command":"event_format","event_id":api_event_id,"show_parameter_names":true}).to_string());
    assert_eq!(formatted["ok"], true, "{formatted}");
    let text = formatted["data"]["text"].as_str().unwrap();
    assert!(
        text.contains("IASetPrimitiveTopology(") && text.contains("Topology="),
        "{text}"
    );
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
    let shader_formatted = agent.send(&serde_json::json!({"command":"event_format","event_id":shader_api_id,"show_all_parameters":true,"show_parameter_names":true}).to_string());
    assert_eq!(shader_formatted["ok"], true, "{shader_formatted}");
    assert!(
        shader_formatted["data"]["text"]
            .as_str()
            .unwrap()
            .contains("pShader="),
        "{shader_formatted}"
    );
    let batch = agent.send(&serde_json::json!({"command":"event_format_batch","event_ids":[api_event_id,shader_api_id,u32::MAX],"show_parameter_names":true}).to_string());
    assert_eq!(batch["ok"], true, "{batch}");
    assert_eq!(batch["data"]["events"][0]["event_id"], api_event_id);
    assert_eq!(batch["data"]["events"][0]["ok"], true);
    assert_eq!(batch["data"]["events"][1]["ok"], true);
    assert_eq!(batch["data"]["events"][2]["ok"], false);
    let bad_batch = agent.send(r#"{"command":"event_format_batch","event_ids":[]}"#);
    assert_eq!(bad_batch["error"]["code"], "invalid_arguments");
    let invalid = agent.send(&serde_json::json!({"command":"event_format","event_id":shader_api_id,"show_all_parameters":"yes"}).to_string());
    assert_eq!(invalid["error"]["code"], "invalid_arguments");
    let update_structure = agent
        .send(&serde_json::json!({"command":"structure","event_id":update_api_id}).to_string());
    let found_formatted = agent.send(&serde_json::json!({"command":"find_event","query":"pSrcData","show_api":true,"format_names":true,"show_all_parameters":true,"show_parameter_names":true}).to_string());
    assert_eq!(found_formatted["ok"], true, "{found_formatted}");
    assert!(
        found_formatted["data"]["match"].is_object(),
        "{found_formatted}"
    );
    let invalid_find = agent.send(r#"{"command":"find_event","query":"x","format_names":"yes"}"#);
    assert_eq!(invalid_find["error"]["code"], "invalid_arguments");
    let formatted_filter = agent.send(&serde_json::json!({"command":"filter_events","expression":"pSrcData","show_api":true,"format_names":true,"show_all_parameters":true,"show_parameter_names":true,"use_custom_action_names":false}).to_string());
    assert_eq!(formatted_filter["ok"], true, "{formatted_filter}");
    let invalid_filter_options =
        agent.send(r#"{"command":"filter_events","expression":"","format_names":"yes"}"#);
    assert_eq!(invalid_filter_options["error"]["code"], "invalid_arguments");
    let invalid_custom = agent
        .send(r#"{"command":"filter_events","expression":"","use_custom_action_names":"yes"}"#);
    assert_eq!(invalid_custom["error"]["code"], "invalid_arguments");
    assert_eq!(formatted_filter["data"]["events"]["format_names"], true);
    assert!(
        formatted_filter["data"]["events"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["direct_match"] == true
                && item["name"]
                    .as_str()
                    .is_some_and(|name| name.contains("pSrcData"))),
        "{formatted_filter}"
    );
    let event_export_path = dir.path().join("formatted-events.txt");
    let formatted_export = agent.send(&serde_json::json!({"command":"export_events","path":event_export_path,"filter":"pSrcData","show_api":true,"format_names":true,"show_all_parameters":true,"show_parameter_names":true}).to_string());
    assert_eq!(formatted_export["ok"], true, "{formatted_export}");
    assert!(
        std::fs::read_to_string(&event_export_path)
            .unwrap()
            .contains("pSrcData")
    );
    let repeated_export = agent.send(&serde_json::json!({"command":"export_events","path":event_export_path,"format_names":true}).to_string());
    assert_eq!(repeated_export["error"]["code"], "event_export_failed");
    let invalid_export_path = dir.path().join("invalid-formatted-events.txt");
    let invalid_export = agent.send(&serde_json::json!({"command":"export_events","path":invalid_export_path,"format_names":"yes"}).to_string());
    assert_eq!(invalid_export["error"]["code"], "invalid_arguments");
    assert!(!invalid_export_path.exists());
    let formatted_step = agent.send(&serde_json::json!({"command":"step_action","event_id":api_event_id,"direction":"next","filter":"IndexCount","format_names":true,"show_all_parameters":true,"show_parameter_names":true}).to_string());
    assert_eq!(formatted_step["ok"], true, "{formatted_step}");
    let invalid_step_options = agent.send(&serde_json::json!({"command":"step_action","event_id":api_event_id,"direction":"next","format_names":"yes"}).to_string());
    assert_eq!(invalid_step_options["error"]["code"], "invalid_arguments");
    let stepped_event = formatted_step["data"]["action"]["event_id"]
        .as_u64()
        .expect("next matching draw");
    let stepped_name = agent.send(&serde_json::json!({"command":"event_format","event_id":stepped_event,"show_all_parameters":true,"show_parameter_names":true}).to_string());
    assert!(
        stepped_name["data"]["text"]
            .as_str()
            .is_some_and(|name| name.contains("IndexCount")),
        "{stepped_name}"
    );
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
        agent.send(r#"{"id":12,"command":"select","event_id":4294967295}"#)["error"]["code"],
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
    for (format, signature) in [
        ("dds", &b"DDS "[..]),
        ("jpg", &[0xff, 0xd8][..]),
        ("bmp", &b"BM"[..]),
        ("hdr", &b"#?RADIANCE"[..]),
        ("exr", &[0x76, 0x2f, 0x31, 0x01][..]),
        ("tga", &[][..]),
    ] {
        let path = dir.path().join(format!("texture.{format}"));
        let request = serde_json::json!({"command":"export_texture", "index":texture_index, "path":path, "format":format});
        let result = agent.send(&request.to_string());
        assert_eq!(result["ok"], true, "{format}: {result}");
        assert_eq!(result["data"]["format"], format);
        let data = std::fs::read(&path).unwrap();
        assert!(
            !data.is_empty() && data.starts_with(signature),
            "{format}: {} bytes",
            data.len()
        );
        if format == "tga" {
            assert!(
                data.len() >= 18 && matches!(data[2], 2 | 10),
                "invalid TGA header"
            );
            assert_eq!(u16::from_le_bytes([data[12], data[13]]), 16);
            assert_eq!(u16::from_le_bytes([data[14], data[15]]), 16);
        }
        assert_eq!(
            agent.send(&request.to_string())["error"]["code"],
            "replay_export_failed"
        );
        assert_eq!(std::fs::read(path).unwrap(), data);
    }
    assert_eq!(agent.send(&serde_json::json!({"command":"export_texture","index":texture_index,"path":dir.path().join("invalid.out"),"format":"unsupported"}).to_string())["error"]["code"], "invalid_arguments");
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
    let volume_raw = dir.path().join("volume.raw");
    let exported_volume = agent.send(&serde_json::json!({"command":"export_texture_raw", "index":volume_index, "path":volume_raw}).to_string());
    assert_eq!(exported_volume["ok"], true, "{exported_volume}");
    assert_eq!(exported_volume["data"]["whole_mip_volume"], true);
    let data = std::fs::read(&volume_raw).unwrap();
    assert_eq!(data.len(), 4 * 4 * 2 * 4);
    assert_eq!(&data[..4], &[255, 0, 0, 255]);
    assert_eq!(&data[64..68], &[0, 0, 255, 255]);
    let volume_page = agent.send(&serde_json::json!({"command":"read_texture_raw", "index":volume_index, "offset":"64", "length":4}).to_string());
    assert_eq!(
        volume_page["data"]["whole_mip_volume"], true,
        "{volume_page}"
    );
    assert_eq!(
        volume_page["data"]["page"]["hex"], "0000ffff",
        "{volume_page}"
    );
    let invalid = agent.send(&serde_json::json!({"command":"export_texture_raw", "index":volume_index, "slice":1, "path":dir.path().join("invalid-volume.raw")}).to_string());
    assert_eq!(invalid["error"]["code"], "invalid_arguments", "{invalid}");
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
        let raw_msaa = dir.path().join("sample0.raw");
        let raw_sample = agent.send(&serde_json::json!({"command":"export_texture_raw", "index":msaa, "sample":0, "path":raw_msaa}).to_string());
        assert_eq!(raw_sample["ok"], true, "{raw_sample}");
        let raw_sample_bytes = std::fs::read(raw_msaa).unwrap();
        assert_eq!(raw_sample_bytes.len(), 4 * 4 * 4);
        for (actual, expected) in raw_sample_bytes[..4].iter().zip([64u8, 128, 191, 255]) {
            assert!(actual.abs_diff(expected) <= 1);
        }
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
    let raw_path = dir.path().join("层级 原始.raw");
    let raw_request = serde_json::json!({"command":"export_texture_raw", "index":layered_index, "path":raw_path, "mip":1, "slice":1, "sample":0});
    let raw_result = agent.send(&raw_request.to_string());
    assert_eq!(raw_result["ok"], true, "{raw_result}");
    assert_eq!(raw_result["data"]["bytes"], "64");
    let raw_bytes = std::fs::read(&raw_path).unwrap();
    assert_eq!(&raw_bytes[..4], &[255, 255, 0, 255]);
    let first_page = agent.send(&serde_json::json!({"command":"read_texture_raw", "index":layered_index, "mip":1, "slice":1, "sample":0, "offset":"0", "length":10}).to_string());
    assert_eq!(first_page["data"]["page"]["total"], "64", "{first_page}");
    assert_eq!(
        first_page["data"]["page"]["hex"],
        raw_bytes[..10]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        "{first_page}"
    );
    let last_page = agent.send(&serde_json::json!({"command":"read_texture_raw", "index":layered_index, "mip":1, "slice":1, "sample":0, "offset":"60", "length":20}).to_string());
    assert_eq!(
        last_page["data"]["page"]["hex"],
        raw_bytes[60..]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        "{last_page}"
    );
    let found = agent.send(&serde_json::json!({"command":"find_texture_raw", "index":layered_index, "mip":1, "slice":1, "offset":"4", "hex":"ff ff 00 ff"}).to_string());
    assert_eq!(found["data"]["offset"], "4", "{found}");
    let invalid_page = agent.send(&serde_json::json!({"command":"read_texture_raw", "index":layered_index, "mip":1, "slice":1, "offset":"65", "length":1}).to_string());
    assert_eq!(
        invalid_page["error"]["code"], "raw_texture_read_failed",
        "{invalid_page}"
    );
    assert_eq!(
        agent.send(&raw_request.to_string())["error"]["code"],
        "replay_export_failed"
    );
    assert_eq!(std::fs::read(&raw_path).unwrap(), raw_bytes);
    let invalid_raw = agent.send(&serde_json::json!({"command":"export_texture_raw", "index":layered_index, "path":dir.path().join("bad.raw"), "channel":0}).to_string());
    assert_eq!(
        invalid_raw["error"]["code"], "invalid_arguments",
        "{invalid_raw}"
    );
    let layered_dds = dir.path().join("layered.dds");
    let selected_dds = agent.send(&serde_json::json!({"command":"export_texture", "index":layered_index, "path":layered_dds, "format":"dds", "mip":1, "slice":1, "sample":0}).to_string());
    assert_eq!(selected_dds["ok"], true, "{selected_dds}");
    let dds_data = std::fs::read(&layered_dds).unwrap();
    assert_eq!(&dds_data[..4], b"DDS ");
    assert_eq!(u32::from_le_bytes(dds_data[12..16].try_into().unwrap()), 4);
    assert_eq!(u32::from_le_bytes(dds_data[16..20].try_into().unwrap()), 4);
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
    let invalid_mesh = agent.send(r#"{"command":"debug_mesh","group":[0,0],"thread":[0,0,0]}"#);
    assert_eq!(invalid_mesh["error"]["code"], "invalid_arguments");
    let unsupported_mesh =
        agent.send(r#"{"command":"debug_mesh","group":[0,0,0],"thread":[0,0,0]}"#);
    assert_eq!(unsupported_mesh["error"]["code"], "replay_debug_failed");
    assert!(
        unsupported_mesh["error"]["message"]
            .as_str()
            .unwrap()
            .contains("mesh dispatch")
    );
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
    let bitfields = agent.send(&serde_json::json!({"command":"decode_buffer_layout", "index":vertex_index, "offset":"4", "length":4, "layout":"uint low:10; uint mid:10; uint high:10; uint alpha:2;"}).to_string());
    let fields = bitfields["data"]["page"]["rows"][0]["fields"]
        .as_array()
        .unwrap();
    assert_eq!(
        fields
            .iter()
            .map(|field| field["values"][0][0].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["0", "0", "4", "1"],
        "{bitfields}"
    );
    assert_eq!(
        fields
            .iter()
            .map(|field| field["bit_offset"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [0, 10, 20, 30]
    );
    let enumeration = agent.send(&serde_json::json!({"command":"decode_buffer_layout", "index":vertex_index, "offset":"4", "length":4, "layout":"enum State : uint { Three = 0x40400000 }; State value;"}).to_string());
    let enum_field = &enumeration["data"]["page"]["rows"][0]["fields"][0];
    assert_eq!(enum_field["values"][0][0], "1077936128", "{enumeration}");
    assert_eq!(enum_field["enum_type"], "State", "{enumeration}");
    assert_eq!(
        enum_field["enum_names"][0],
        serde_json::json!(["Three"]),
        "{enumeration}"
    );
    let cbuffer = agent.send(&serde_json::json!({"command":"decode_buffer_layout", "index":vertex_index, "offset":"4", "length":8, "layout":"#pack(cbuffer) float2 position;"}).to_string());
    assert_eq!(cbuffer["data"]["page"]["packing"], "cbuffer", "{cbuffer}");
    assert_eq!(
        cbuffer["data"]["page"]["rows"][0]["fields"][0]["values"][0],
        serde_json::json!(["3", "3"]),
        "{cbuffer}"
    );
    let std430 = agent.send(&serde_json::json!({"command":"decode_buffer_layout", "index":vertex_index, "offset":"4", "length":8, "layout":"#pack(std430) vec2 position;"}).to_string());
    assert_eq!(std430["data"]["page"]["packing"], "std430", "{std430}");
    assert_eq!(
        std430["data"]["page"]["rows"][0]["fields"][0]["values"][0],
        serde_json::json!(["3", "3"]),
        "{std430}"
    );
    let nested = agent.send(&serde_json::json!({"command":"decode_buffer_layout", "index":vertex_index, "offset":"4", "length":8, "layout":"struct V { float2 pos; }; V vertex;"}).to_string());
    assert_eq!(
        nested["data"]["page"]["rows"][0]["fields"][0]["name"], "vertex.pos",
        "{nested}"
    );
    assert_eq!(
        nested["data"]["page"]["rows"][0]["fields"][0]["values"][0],
        serde_json::json!(["3", "3"]),
        "{nested}"
    );
    let packed = agent.send(&serde_json::json!({"command":"decode_buffer_layout", "index":vertex_index, "offset":"4", "length":4, "layout":"[[packed(r10g10b10a2_uint)]] uint4 packed;"}).to_string());
    assert_eq!(
        packed["data"]["page"]["rows"][0]["fields"][0]["packed"], "r10g10b10a2_uint",
        "{packed}"
    );
    assert_eq!(
        packed["data"]["page"]["rows"][0]["fields"][0]["values"][0],
        serde_json::json!(["0", "0", "4", "1"]),
        "{packed}"
    );
    let matrix = agent.send(&serde_json::json!({"command":"decode_buffer_layout", "index":vertex_index, "offset":"4", "length":16, "layout":"[[row_major]] float2x2 m;"}).to_string());
    let field = &matrix["data"]["page"]["rows"][0]["fields"][0];
    assert_eq!(field["matrix_rows"], 2, "{matrix}");
    assert_eq!(field["matrix_columns"], 2, "{matrix}");
    assert_eq!(field["row_major"], true, "{matrix}");
    assert_eq!(field["values"][0][0], "3", "{matrix}");
    assert_eq!(field["values"][0][1], "3", "{matrix}");
    let annotated = agent.send(&serde_json::json!({"command":"decode_buffer_layout", "index":vertex_index, "offset":"0", "length":8, "layout":"[[byte_offset(4)]] float value;"}).to_string());
    assert_eq!(
        annotated["data"]["page"]["rows"][0]["fields"][0]["offset"], "4",
        "{annotated}"
    );
    assert_eq!(
        annotated["data"]["page"]["rows"][0]["fields"][0]["values"][0][0], "3",
        "{annotated}"
    );
    assert_eq!(agent.send(&serde_json::json!({"command":"decode_buffer_layout", "index":vertex_index, "offset":"0", "length":8, "layout":"float a; [[offset(2)]] float b;"}).to_string())["error"]["code"], "invalid_layout");
    let layout_page = agent.send(&serde_json::json!({"command":"decode_buffer_layout", "index":vertex_index, "offset":"4", "length":8, "layout":"#pack(scalar) float2 position;"}).to_string());
    let rejected_prefix = agent.send(&serde_json::json!({"command":"decode_buffer_layout", "index":vertex_index, "offset":"0", "length":12, "layout":"uint header; float2 positions[];"}).to_string());
    assert_eq!(
        rejected_prefix["error"]["code"], "invalid_layout",
        "{rejected_prefix}"
    );
    let repeat = agent.send(&serde_json::json!({"command":"decode_buffer_layout", "index":vertex_index, "offset":"4", "length":8, "layout":"float2 positions[];"}).to_string());
    let page_with_prefix = agent.send(&serde_json::json!({"command":"decode_buffer_layout", "index":vertex_index, "origin":"0", "offset":"4", "length":8, "include_fixed":true, "layout":"[[offset(4)]] float2 positions[];"}).to_string());
    assert_eq!(
        page_with_prefix["data"]["page"]["fixed"],
        serde_json::json!([]),
        "{page_with_prefix}"
    );
    assert_eq!(
        page_with_prefix["data"]["page"]["rows"][0]["fields"][0]["values"][0],
        serde_json::json!(["3", "3"]),
        "{page_with_prefix}"
    );
    assert_eq!(repeat["data"]["page"]["repeat_offset"], "4", "{repeat}");
    assert_eq!(
        repeat["data"]["page"]["rows"][0]["fields"][0]["values"][0],
        serde_json::json!(["3", "3"]),
        "{repeat}"
    );
    let fixed = agent.send(&serde_json::json!({"command":"decode_buffer_layout", "index":vertex_index, "offset":"4", "length":8, "layout":"[[single]] float2 position;"}).to_string());
    assert_eq!(
        fixed["data"]["page"]["fixed"][0]["values"][0],
        serde_json::json!(["3", "3"]),
        "{fixed}"
    );
    assert_eq!(fixed["data"]["page"]["rows"], serde_json::json!([]));
    let sized = agent.send(&serde_json::json!({"command":"decode_buffer_layout", "index":vertex_index, "offset":"4", "length":8, "layout":"[[size(8)]] [[align(4)]] struct S { float x; }; S value;"}).to_string());
    assert_eq!(sized["data"]["page"]["stride"], "8", "{sized}");
    assert_eq!(
        sized["data"]["page"]["rows"][0]["fields"][0]["values"][0][0], "3",
        "{sized}"
    );
    assert_eq!(layout_page["data"]["page"]["stride"], "8", "{layout_page}");
    let rgb = agent.send(&serde_json::json!({"command":"decode_buffer_layout", "index":vertex_index, "offset":"4", "length":8, "layout":"[[rgb]] float2 color;"}).to_string());
    assert_eq!(
        rgb["data"]["page"]["rows"][0]["fields"][0]["rgb_srgb8"][0],
        serde_json::json!([255, 255, 0]),
        "{rgb}"
    );
    let legacy_rgb = agent.send(&serde_json::json!({"command":"decode_buffer_layout", "index":vertex_index, "offset":"4", "length":4, "layout":"unsigned rgb byte4 color;"}).to_string());
    assert_eq!(
        legacy_rgb["data"]["page"]["rows"][0]["fields"][0]["rgb_srgb8"][0],
        serde_json::json!([0, 0, 64]),
        "{legacy_rgb}"
    );
    let norm_display = agent.send(&serde_json::json!({"command":"decode_buffer_layout", "index":vertex_index, "offset":"6", "length":1, "layout":"[[hex]] [[unorm]] ubyte value;"}).to_string());
    let field = &norm_display["data"]["page"]["rows"][0]["fields"][0];
    assert_eq!(
        field["values"][0][0], field["display_values"][0][0],
        "{norm_display}"
    );
    let display = agent.send(&serde_json::json!({"command":"decode_buffer_layout", "index":vertex_index, "offset":"4", "length":8, "layout":"xint h; [[binary]] int b;"}).to_string());
    assert_eq!(
        display["data"]["page"]["rows"][0]["fields"][0]["display_values"][0][0], "40400000",
        "{display}"
    );
    assert_eq!(
        display["data"]["page"]["rows"][0]["fields"][1]["display_values"][0][0],
        "01000000010000000000000000000000",
        "{display}"
    );
    assert_eq!(
        display["data"]["page"]["rows"][0]["fields"][0]["values"][0][0], "1077936128",
        "{display}"
    );
    assert_eq!(
        layout_page["data"]["page"]["rows"][0]["fields"][0]["values"][0],
        serde_json::json!(["3", "3"]),
        "{layout_page}"
    );
    let csv_path = dir.path().join("vertex-format.csv");
    let csv = agent.send(&serde_json::json!({"command":"export_buffer_csv", "index":vertex_index, "offset":"4", "length":"8", "layout":"float2 position;", "path":csv_path}).to_string());
    assert_eq!(csv["data"]["summary"]["rows"], "1", "{csv}");
    assert_eq!(
        std::fs::read_to_string(&csv_path).unwrap(),
        "Byte Offset,position\n4,\"3, 3\"\n"
    );
    let retry = agent.send(&serde_json::json!({"command":"export_buffer_csv", "index":vertex_index, "offset":"4", "length":"8", "layout":"float2 position;", "path":csv_path}).to_string());
    assert_eq!(retry["error"]["code"], "buffer_export_failed", "{retry}");
    assert_eq!(agent.send(&serde_json::json!({"command":"decode_buffer_layout", "index":vertex_index, "offset":"4", "length":8, "layout":"#pack(unknown) float2 pos;"}).to_string())["error"]["code"], "invalid_layout");
    let decoded = agent.send(&serde_json::json!({"command":"decode_buffer", "index":vertex_index, "offset":"4", "length":8, "format":"float"}).to_string());
    assert_eq!(
        decoded["data"]["page"]["values"][0]["offset"], "4",
        "{decoded}"
    );
    assert_eq!(
        decoded["data"]["page"]["values"][0]["value"], "3",
        "{decoded}"
    );
    assert_eq!(
        decoded["data"]["page"]["values"][1]["value"], "3",
        "{decoded}"
    );
    assert_eq!(agent.send(&serde_json::json!({"command":"decode_buffer", "index":vertex_index, "offset":"4", "length":8, "format":"invalid"}).to_string())["error"]["code"], "invalid_arguments");
    let found = agent.send(&serde_json::json!({"command":"find_buffer", "index":vertex_index, "offset":"4", "length":"8", "hex":"00 00 40 40"}).to_string());
    assert_eq!(found["data"]["offset"], "4", "{found}");
    let missing = agent.send(&serde_json::json!({"command":"find_buffer", "index":vertex_index, "offset":"4", "length":"8", "hex":"de ad be ef"}).to_string());
    assert!(missing["data"]["offset"].is_null(), "{missing}");
    assert_eq!(agent.send(&serde_json::json!({"command":"find_buffer", "index":vertex_index, "offset":"4", "length":"8", "hex":"0"}).to_string())["error"]["code"], "invalid_arguments");
    let buffer_export = dir.path().join("vertex-range.bin");
    let export_request = serde_json::json!({"command":"export_buffer", "index":vertex_index, "offset":"4", "length":"8", "path":buffer_export});
    let exported = agent.send(&export_request.to_string());
    assert_eq!(exported["ok"], true, "{exported}");
    assert_eq!(
        std::fs::read(&buffer_export).unwrap(),
        [0, 0, 0x40, 0x40, 0, 0, 0x40, 0x40]
    );
    assert_eq!(
        agent.send(&export_request.to_string())["error"]["code"],
        "buffer_export_failed"
    );
    assert_eq!(agent.send(&serde_json::json!({"command":"export_buffer", "index":vertex_index, "offset":"28", "length":"1", "path":dir.path().join("invalid.bin")}).to_string())["error"]["code"], "invalid_arguments");
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
    let raw_pixel = serde_json::json!({"command":"read_texture_raw", "index":texture_index, "offset":"544", "length":4}).to_string();
    let raw_before = agent.send(&raw_pixel);
    assert_eq!(raw_before["ok"], true, "{raw_before}");
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
    let raw_changed = agent.send(&raw_pixel);
    assert_eq!(raw_changed["ok"], true, "{raw_changed}");
    assert_ne!(
        raw_before["data"]["page"]["hex"], raw_changed["data"]["page"]["hex"],
        "shader replacement must invalidate raw cache"
    );
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
    let raw_restored = agent.send(&raw_pixel);
    assert_eq!(
        raw_restored["data"]["page"]["hex"], raw_before["data"]["page"]["hex"],
        "{raw_restored}"
    );
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
    let history_events = agent.send(r#"{"command":"timeline_history"}"#);
    assert_eq!(history_events["ok"], true, "{history_events}");
    assert_eq!(
        history_events["data"]["history"]["modifications"],
        history["data"]["history"]["total"]
    );
    for entry in history["data"]["history"]["entries"].as_array().unwrap() {
        let category = if entry["passed"] == true {
            "passed_events"
        } else {
            "failed_events"
        };
        assert!(
            history_events["data"]["history"][category]
                .as_array()
                .unwrap()
                .contains(&entry["event_id"])
        );
        assert!(
            history_events["data"]["history"]["events"]
                .as_array()
                .unwrap()
                .contains(&entry["event_id"])
        );
    }
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
    let local_gpus = agent.send(r#"{"command":"gpu_list"}"#);
    assert_eq!(local_gpus["ok"], true, "{local_gpus}");
    let remote_gpus = agent.send(&serde_json::json!({"command":"gpu_list", "url":url}).to_string());
    assert_eq!(remote_gpus["ok"], true, "{remote_gpus}");
    let request =
        serde_json::json!({"command":"open", "path":capture, "remote_url":url, "replay_options":{"optimisation":0}}).to_string();
    let mut opened = Value::Null;
    for _ in 0..20 {
        opened = agent.send(&request);
        if opened["ok"] == true {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(opened["ok"], true, "{opened}");
    assert_eq!(
        opened["data"]["capture"]["replay_options"]["optimisation"],
        0
    );
    let active_gpus = agent.send(&serde_json::json!({"command":"gpu_list", "url":url}).to_string());
    assert_eq!(active_gpus["ok"], true, "{active_gpus}");
    assert_eq!(active_gpus["data"]["gpus"], remote_gpus["data"]["gpus"]);
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
    let timeline = agent.send(
        &serde_json::json!({"command":"timeline", "start":event, "end":event, "limit":128})
            .to_string(),
    );
    assert_eq!(timeline["ok"], true, "{timeline}");
    assert!(
        timeline["data"]["timeline"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["event_id"] == event)
    );
    assert_eq!(
        agent.send(r#"{"command":"timeline","start":2,"end":1}"#)["error"]["code"],
        "invalid_arguments"
    );
    let selected =
        agent.send(&serde_json::json!({"command":"select", "event_id":event}).to_string());
    assert_eq!(selected["ok"], true, "{selected}");
    let vertex_resource = opened["data"]["capture"]["buffers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|buffer| buffer["length"] == "28")
        .unwrap()["resource_index"]
        .as_u64()
        .unwrap();
    let usage = agent.send(
        &serde_json::json!({"command":"timeline_usage", "index":vertex_resource}).to_string(),
    );
    assert_eq!(usage["ok"], true, "{usage}");
    assert!(
        usage["data"]["usage"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["event_id"] == event)
    );
    let texture = opened["data"]["capture"]["textures"]
        .as_array()
        .unwrap()
        .iter()
        .position(|texture| texture["width"] == 16 && texture["height"] == 16)
        .unwrap();
    let history = agent.send(
        &serde_json::json!({"command":"pixel_history", "index":texture, "x":8, "y":8}).to_string(),
    );
    assert_eq!(history["ok"], true, "{history}");
    let history_events = agent.send(r#"{"command":"timeline_history"}"#);
    assert_eq!(history_events["ok"], true, "{history_events}");
    assert_eq!(
        history_events["data"]["history"]["modifications"],
        history["data"]["history"]["total"]
    );
    let highlighted = history_events["data"]["history"]["events"]
        .as_array()
        .unwrap();
    assert!(!highlighted.is_empty(), "{history_events}");
    for entry in history["data"]["history"]["entries"].as_array().unwrap() {
        assert!(highlighted.contains(&entry["event_id"]));
        let category = if entry["passed"] == true {
            "passed_events"
        } else {
            "failed_events"
        };
        assert!(
            history_events["data"]["history"][category]
                .as_array()
                .unwrap()
                .contains(&entry["event_id"])
        );
    }
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

#[test]
fn replay_options_reject_invalid_values_before_open() {
    let mut agent = Agent::new(None);
    for options in [
        serde_json::json!({"optimisation":4}),
        serde_json::json!({"gpu_vendor":11}),
        serde_json::json!({"gpu_driver":"bad\0driver"}),
        serde_json::json!({"unknown":true}),
        serde_json::json!(null),
    ] {
        let request =
            serde_json::json!({"command":"open", "path":"capture.rdc", "replay_options":options})
                .to_string();
        assert_eq!(agent.send(&request)["error"]["code"], "invalid_arguments");
    }
    agent.quit();
}

#[cfg(windows)]
#[test]
#[ignore = "requires matching RenderDoc, dependency and D3D11 fixtures"]
fn real_dependency_exports_preserve_original_locally_and_remotely() {
    use std::{net::TcpListener, os::windows::process::CommandExt, path::PathBuf, time::Duration};
    struct Server(Child);
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let bridge = PathBuf::from(std::env::var_os("RENDERDOCX_TEST_BRIDGE").unwrap());
    let fixture = std::env::var_os("RENDERDOCX_TEST_FIXTURE").unwrap();
    let dependency_fixture = std::env::var_os("RENDERDOCX_TEST_DEPENDENCY_FIXTURE").unwrap();
    let server_exe = std::env::var_os("RENDERDOCX_TEST_REMOTE_SERVER").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(fixture)
        .arg(bridge.parent().unwrap().join("renderdoc.dll"))
        .arg(dir.path().join("frame"))
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let capture = stdout
        .lines()
        .find_map(|line| line.strip_prefix("RENDERDOCX_CAPTURE="))
        .unwrap();
    assert!(
        Command::new(dependency_fixture)
            .arg(capture)
            .creation_flags(0x08000000)
            .status()
            .unwrap()
            .success()
    );
    let original = std::fs::read(capture).unwrap();
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
    let url = format!("127.0.0.1:{port}");
    for remote in [false, true] {
        let mut agent = Agent::new(bridge.to_str());
        let mut request = serde_json::json!({"command":"open", "path":capture});
        if remote {
            request["remote_url"] = serde_json::json!(url);
        }
        let opened = agent.send(&request.to_string());
        assert_eq!(opened["ok"], true, "{opened}");
        let info = agent.send(r#"{"command":"dependencies"}"#);
        assert_eq!(info["ok"], true, "{info}");
        assert_eq!(info["data"]["dependencies"]["embedded"], true, "{info}");
        assert_eq!(info["data"]["dependencies"]["pending"], true, "{info}");
        assert!(
            info["data"]["dependencies"]["nicknames"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n == "renderdocx-fixture")
        );
        let removed = agent.send(&serde_json::json!({"command":"dependencies_save", "action":"remove", "capture_directory":dir.path()}).to_string());
        assert_eq!(removed["ok"], true, "{removed}");
        let embedded = agent.send(&serde_json::json!({"command":"dependencies_save", "action":"embed", "capture_directory":dir.path()}).to_string());
        assert_eq!(embedded["ok"], true, "{embedded}");
        assert_eq!(std::fs::read(capture).unwrap(), original);
        if remote {
            let section = agent.send(r#"{"command":"annotation_read","name":"notes"}"#);
            let saved = agent.send(&serde_json::json!({"command":"annotation_write", "section":section["data"]["section"], "document":{"comments":"changed"}}).to_string());
            assert_eq!(saved["ok"], true, "{saved}");
            let refused = agent.send(&serde_json::json!({"command":"dependencies_save", "action":"embed", "capture_directory":dir.path()}).to_string());
            assert_eq!(
                refused["error"]["code"], "dependencies_save_failed",
                "{refused}"
            );
        }
        agent.quit();
        for (result, expected) in [(&removed, false), (&embedded, true)] {
            let mut check = Agent::new(bridge.to_str());
            let opened = check.send(
                &serde_json::json!({"command":"open", "path":result["data"]["path"]}).to_string(),
            );
            assert_eq!(opened["ok"], true, "{opened}");
            let status = check.send(r#"{"command":"dependencies"}"#);
            assert_eq!(
                status["data"]["dependencies"]["embedded"], expected,
                "{status}"
            );
            check.quit();
        }
    }
}
