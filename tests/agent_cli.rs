use serde_json::Value;
use std::process::Command;

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

#[test]
fn capabilities_advertises_versioned_machine_readable_contract() {
    let (success, value) = invoke(&["capabilities"]);
    assert!(success);
    assert_eq!(value["schema"], "renderdocx.cli.v1");
    assert_eq!(value["ok"], true);
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
#[ignore = "requires a built D3D11 fixture and matching RenderDoc replay bridge"]
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
        .prefix("capture-")
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
    let (success, result) = invoke(&["--bridge", bridge, "select", capture, &event]);
    assert!(success, "event replay failed: {result}");
    assert_eq!(result["event_id"].as_u64().unwrap().to_string(), event);
}
