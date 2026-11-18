# Real GPU / real-capture verification

This file records the real (non-mocked) verification that was executed on this
machine. Timings are from the local run.

## Environment

| Item | Value |
|---|---|
| GPU | NVIDIA GeForce RTX 5060 (4 GB) + AMD Radeon(TM) Graphics |
| Toolchain | Visual Studio 2026 Community, CMake 3.3x, 64-bit PowerShell 7.6 |
| RenderDoc core | built locally from this checkout, commit `5a3c53f` (Release) |
| Replay bridge | `target/renderdocx-runtime/renderdocx-replay.exe` |
| Fixtures | `target/replay-bridge/Release/renderdocx-*.exe` |

Build:

```powershell
# Must be 64-bit PowerShell (the 32-bit pwsh cannot build the C++ core).
& "C:\Program Files\PowerShell\7\pwsh.exe" -File scripts/build-replay-bridge.ps1 -CoreConfiguration Release -WithFixtures
```

The script now builds `breakpad_common`, `crash_generation_client`,
`exception_handler` and `renderdoc_version` before `renderdoc.vcxproj`, because
those are linked libraries rather than ProjectReferences. A `recompress`
bridge compile error (`std::string` vs `rdcstr` in `ICaptureFile::Convert`) was
found and fixed during this build.

## Passed tests (11)

Run with:

```powershell
pwsh -File scripts/run-real-gpu-tests.ps1
```

| Test | Result | Time |
|---|---|---|
| real_core_reports_missing_capture_as_an_error | ok | 0.83 s |
| native_target_launch_triggers_capture_and_disconnects_without_stopping_target | ok | 16.00 s |
| real_remote_replay_loopback | ok | 6.00 s |
| real_remote_target_launch_and_capture_loopback | ok | 1.11 s |
| real_d3d11_capture_is_inspected_and_selected | ok | 6.31 s |
| real_d3d11_mesh_draw_order | ok | 1.94 s |
| real_d3d11_nested_markers_resolve_and_replay_effective_events | ok | 1.72 s |
| real_d3d11_shader_messages_page_is_empty | ok | 1.06 s |
| real_d3d11_source_variable_values_follow_debug_navigation | ok | 1.07 s |
| real_opengl_vertex_inputs_include_generic_and_normalized_values | ok | 1.46 s |
| real_dependency_exports_preserve_original_locally_and_remotely | ok | 5.81 s |

These cover: loading the real core, missing-capture error handling, a real
D3D11 process capture/trigger/copy, remote replay/launch over loopback, D3D11
capture inspection and event selection, mesh draw order, nested marker
resolution, shader-message paging, source-variable debug navigation, OpenGL
vertex inputs, and dependency export locally and remotely.

A real `.rdc` can also be produced directly:

```powershell
pwsh -File scripts/make-d3d11-capture.ps1
# CAPTURE=…\target\renderdocx-captures\remote-*\capture.rdc
```

## Vulkan toolchain and new tests (added)

No Vulkan SDK was installed, so the Khronos **glslang 16.6.0** Windows release
was downloaded (`target/tools/glslang`) and `glslang.exe` copied to
`glslangValidator.exe`. The demos compile GLSL at runtime through
`glslangValidator` found in `PATH`.

Fixtures built from `util/test/demos` (all succeeded):

| Fixture | Output | Demo |
|---|---|---|
| Mesh | `target/vulkan-mesh-fixture/demos_x64.exe` | `VK_RenderDocX_Mesh_Printf` / `VK_Mesh_Shader` |
| Vertex | `target/vulkan-vertex-fixture/demos_x64.exe` | `VK_RenderDocX_Vertex_Printf` |
| Geometry | `target/vulkan-geometry-fixture/demos_x64.exe` | `VK_RenderDocX_Geometry_Printf` |
| ShaderPrintf | `target/vulkan-shader-printf/demos_x64.exe` | `VK_Shader_Printf` |

Captures were generated with `scripts/make-vulkan-capture.ps1`
(target_launch → wait presenting → trigger 1 frame → target_copy).

`scripts/run-vulkan-tests.ps1` sets all `RENDERDOCX_TEST_VULKAN_*` variables.
Result: **8 of 8 Vulkan tests pass**

| Test | Result |
|---|---|
| real_vulkan_target_launch_uses_packaged_layer | ok |
| real_vulkan_mesh_shader_debug_trace | ok |
| real_vulkan_vertex_shader_messages_resolve_and_debug | ok |
| real_vulkan_geometry_shader_messages_show_approximate_output | ok |
| real_vulkan_task_and_mesh_shader_messages | ok |
| real_vulkan_task_message_sanitizes_older_core | ok |
| real_vulkan_mesh_printf_core_survives_and_initializes_locations | ok |
| real_vulkan_shader_messages_and_debug_handoff | ok |

Note on `VK_Shader_Printf`: its fragment printf only fires at
`gl_FragCoord` x 200–202 / y 150–152, so the capture must use the demo's
default window (400×300) or larger. A 128×128 capture silently yields zero
messages and the test fails; capturing with the default size makes it pass.

A deliberately mismatched bridge is built with
`scripts/build-mismatch-bridge.ps1` (real core library, fake core commit). It
makes `incompatible_core_is_rejected_before_capture_open` pass:

```
test incompatible_core_is_rejected_before_capture_open ... ok
```

## Full ignored-test result: 20/20 pass

With the toolchain, fixtures, captures and mismatch bridge built as above, all
20 `#[ignore]` tests in `tests/agent_cli.rs` pass on this machine.

## Reproduce

```powershell
& "C:\Program Files\PowerShell\7\pwsh.exe" -File scripts/build-replay-bridge.ps1 -CoreConfiguration Release -WithFixtures
pwsh -File scripts/run-real-gpu-tests.ps1
# Vulkan toolchain (glslang is downloaded to target/tools/glslang)
$env:PATH = "$PWD/target/tools/glslang/bin;$env:PATH"
& "C:\Program Files\PowerShell\7\pwsh.exe" -File scripts/build-vulkan-vertex-fixture.ps1 -Fixture Mesh -OutputDirectory target/vulkan-mesh-fixture
& "C:\Program Files\PowerShell\7\pwsh.exe" -File scripts/build-vulkan-vertex-fixture.ps1 -Fixture Vertex -OutputDirectory target/vulkan-vertex-fixture
& "C:\Program Files\PowerShell\7\pwsh.exe" -File scripts/build-vulkan-vertex-fixture.ps1 -Fixture Geometry -OutputDirectory target/vulkan-geometry-fixture
& "C:\Program Files\PowerShell\7\pwsh.exe" -File scripts/build-vulkan-vertex-fixture.ps1 -Fixture ShaderPrintf -OutputDirectory target/vulkan-shader-printf
pwsh -File scripts/make-vulkan-capture.ps1 -Demo target/vulkan-shader-printf/demos_x64.exe -DemoArguments "VK_Shader_Printf"
pwsh -File scripts/run-vulkan-tests.ps1
& "C:\Program Files\PowerShell\7\pwsh.exe" -File scripts/build-mismatch-bridge.ps1
# then set RENDERDOCX_TEST_MISMATCH_BRIDGE and run incompatible_core_is_rejected_before_capture_open
```
