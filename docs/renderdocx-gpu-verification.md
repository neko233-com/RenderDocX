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

## Not executed (environment dependency, not code failure)

| Test(s) | Missing input |
|---|---|
| real_vulkan_* (7 tests) | Vulkan mesh/printf demo and pre-made Vulkan captures. No `VULKAN_SDK`, and `glslc`/`glslangValidator`/`spirv-as` are absent, so the demos cannot be built here. |
| incompatible_core_is_rejected_before_capture_open | A deliberately mismatched bridge/core build (`RENDERDOCX_TEST_MISMATCH_BRIDGE`). |
| interaction tests requiring a pre-made `.rdc` fixture | `RENDERDOCX_TEST_VERTEX_PRINTF_CAPTURE`, `RENDERDOCX_TEST_MESH_PRINTF_CAPTURE`, etc. |

Installing the Vulkan SDK (providing `glslc`) and pre-recording the bundled
demos would enable those tests; they are not blocked by RenderDocX code.
