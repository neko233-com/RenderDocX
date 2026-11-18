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

## Full ignored-test result: 20/20 + 7 lib tests pass

One command runs every ignored test with all environment variables wired:

```powershell
pwsh -File scripts/verify-all-real-tests.ps1
```

It also runs seven lib-level ignored tests. `python_pipeline_and_buffer_query_a_real_capture`
opens a real Mesh capture through the bridge and asserts pipeline, buffer hex,
structure data, debug/shader messages, resource usage, callstack, event
formatting, descriptors, pixel history, texture statistics/min-max/fit range,
texture preview/context/export PNGs, raw buffer export, mesh output and mesh
vertex reads, plus the full `snapshot()` document and `capture_actions()` list.
It also exercises the pyrenderdoc-compatible CaptureContext surface:
`CurEvent`, `CurSelectedEvent`, `CurAction`, `CurSelectedAction`,
`CurRootActions`, `CurPipelineState`, `GetAction`, `GetTextures`, `GetBuffers`,
`GetResources`, `FrameInfo`, `APIProps`, `IsCaptureLoaded`,
`GetCaptureFilename` and `DebugMessages`. The `Replay()` wrapper is exercised
through `SetFrameEvent(19)` followed by `GetPipelineState`, `GetTextures`,
`GetBuffers`, `GetResources`, `GetFrameInfo` and `GetAPIProperties`.
The same test covers the per-field helpers (`pipeline_shader`,
`pipeline_output`, `pipeline_color_attachment`, `action_info`, `action_at`,
`action_children`, `action_draws`, `texture_info`, `buffer_info`,
`resource_info`), a `PipelineState(19)` object (attributes, `GetShader`,
`GetOutputs`, attachments, `[]`/`get`/`keys`), and field-addressable
`Texture(0)` / `Action(21)` objects (including camelCase aliases and
`GetChildren()`), a `Resource(7)` object, `ShaderReflection(4)` plus
`PipelineState.GetShaderReflection(4)`, `ViewBuffer`,
`ViewPixelHistory`, `ViewShaderMessages` viewer objects, `FrameDescription()`
(frame number/annotations/capture time), `APIProperties()`, the empty
`DescriptorStore(0)` path, `GetEventBrowser()` methods and filter state,
texture-viewer state (`ViewTexture`/subresource/overlay/zoom/channels),
buffer-viewer scroll state, `ShaderSignature` from the fragment reflection, and
a `ShaderVariable` built from the real mesh debug trace, shader-viewer
step/watch/source methods plus `debug_collect`/breakpoint/`GetCurrentBreakpoint`/
`RunForward` navigation, `ReflectedType` from the D3D11 constant's `uint`
type, and API-specific pipeline state (Vulkan graphics pipeline/layouts/render
pass/framebuffer/multisample on the mesh capture; D3D11 input layout,
rasterizer/descriptor-store state and shader class instances on the constant
capture), `Get*Viewer()` handles, `ViewShader`/`EditShader` panel
requests, and a `DebugShader` mesh trace.
Notes and bookmarks are written and
read back through `SetNotes`/`GetNotes`/`SetBookmark`/`GetBookmarks`/
`RemoveBookmark`, then restored to their original values. Resource replacement
is exercised through `RegisterReplacement(7, 8)`: `pipeline(19)` reports the
replacement index, and `UnregisterReplacement(7)` restores the original. Mesh
vertex debugging is exercised through `debug_vertex(...)`/`debug_stop()` on the
Vertex capture, followed by `apply_shader_edit(...)` (GLSL fragment replacement
returns `applied: true`) and `revert_shader_edit(...)` (`applied: false`). A third test uses the upstream VK_Shader_Printf capture and its
shader messages to run `debug_compute(...)` for the dispatch and
`debug_pixel(...)` for the draw, asserting both traces report the selected
event. Mesh debugging is exercised through `debug_mesh(21)`, `debug_next()` and
`debug_stop()`; the returned trace reports event 21 and the mesh stage.
`python_vertex_input_query_a_real_capture` opens a real Vertex capture and
asserts `renderdocx.vertex_input()` returns input rows.
`python_constant_block_query_a_real_capture` opens a D3D11 capture generated by
the local fixture and asserts the compute shader's `Params` constant block,
`seed` `ShaderConstant`, and the (empty) sampler list.
`python_opengl_api_specific_state_a_real_capture` opens an OpenGL capture
generated by the local fixture and asserts `api_specific.kind == "OpenGL"`,
the vertex array object, attribute/vertex-buffer counts, descriptor store and
framebuffer state.
`python_d3d12_api_specific_state_a_real_capture` opens a D3D12 capture produced
by launching the upstream `D3D12_Reflection_Zoo` demo through the target helper
and triggering a capture; it asserts PSO/descriptor heaps/root signature,
topology, sample mask, depth-write state, predication, root parameters
(visibility/constants/descriptor) and the two real static samplers
(registers 5/8, point minification, clamp-U addressing, seamless cubemaps)
through `StaticSampler` objects and `GetD3D12StaticSamplers()`.
The Vulkan mesh test additionally wraps `sampleLocations`, `renderPassDetails`
and `framebufferDetails` with the `SampleLocations`/`RenderPass`/`Framebuffer`
objects and asserts their fields.
`scripts/verify-all-real-tests.ps1` builds the D3D12 fixture/CLI and generates
all three captures before running the tests.

`python_debugpy_attach_speaks_dap` starts the in-process debugpy adapter on
port 5691, connects over TCP, sends a DAP `initialize` request and asserts the
adapter answers `success: true`; it also checks `IsDebuggerListening`,
`GetDebuggerPort` and `IsDebuggerConnected`. The verify script installs
debugpy into `target/python-site` first, and the MSI ships the same
`python-site` beside the executable.

Observed output (20 agent_cli + 7 lib tests):

```
test incompatible_core_is_rejected_before_capture_open ... ok
test native_target_launch_triggers_capture_and_disconnects_without_stopping_target ... ok
test real_core_reports_missing_capture_as_an_error ... ok
test real_d3d11_capture_is_inspected_and_selected ... ok
test real_d3d11_mesh_draw_order ... ok
test real_d3d11_nested_markers_resolve_and_replay_effective_events ... ok
test real_d3d11_shader_messages_page_is_empty ... ok
test real_d3d11_source_variable_values_follow_debug_navigation ... ok
test real_dependency_exports_preserve_original_locally_and_remotely ... ok
test real_opengl_vertex_inputs_include_generic_and_normalized_values ... ok
test real_remote_replay_loopback ... ok
test real_remote_target_launch_and_capture_loopback ... ok
test real_vulkan_geometry_shader_messages_show_approximate_output ... ok
test real_vulkan_mesh_printf_core_survives_and_initializes_locations ... ok
test real_vulkan_mesh_shader_debug_trace ... ok
test real_vulkan_shader_messages_and_debug_handoff ... ok
test real_vulkan_target_launch_uses_packaged_layer ... ok
test real_vulkan_task_and_mesh_shader_messages ... ok
test real_vulkan_task_message_sanitizes_older_core ... ok
test real_vulkan_vertex_shader_messages_resolve_and_debug ... ok
test result: ok. 20 passed; 0 failed; 0 ignored
test python_constant_block_query_a_real_capture ... ok
test python_debugpy_attach_speaks_dap ... ok
test python_d3d12_api_specific_state_a_real_capture ... ok
test python_opengl_api_specific_state_a_real_capture ... ok
test python_pipeline_and_buffer_query_a_real_capture ... ok
test python_shader_printf_debug_a_real_capture ... ok
test python_vertex_input_query_a_real_capture ... ok
test result: ok. 7 passed; 0 failed; 0 ignored
```

## GPU native replay loop

`bridge/replay/main.cpp` command `gpu_replay <texture_index> <seconds>` creates a
real Win32 window and loops `SetFrameEvent(10000000)` +
`IReplayOutput::Display()` on the GPU, pumping window messages so closing the
window stops the loop. Rust exposes `Session::gpu_replay`, the CLI serve command
`gpu_replay {"texture":N,"seconds":S}`, and a GUI "GPU replay (native window)"
button.

Observed with a Vulkan capture, texture 0, 3 seconds:

```json
{"schema":"renderdocx.agent.v1","id":3,"ok":true,"data":{"frames":255}}
```

255 GPU-rendered frames in a native window — this is the upstream ReplayLoop
behaviour (GPU output, not a Rust-side approximation).

## RGP profile creation (`CreateRGPProfile`)

Implemented end-to-end:

- `bridge/replay/main.cpp` command `create_rgp_profile` builds a
  `WindowingData` (hidden Win32 window) and calls
  `IReplayController::CreateRGPProfile`, returning the produced path/size.
- `Session::create_rgp_profile` (Rust), CLI serve command `create_rgp_profile`,
  and a GUI "Create RGP profile (AMD)…" button.

Observed on this machine: `open` with `gpu_vendor=2` replaying an AMD-captured
Vulkan file succeeds, but `create_rgp_profile` returns

```
rgp_profile_failed: RGP profile creation failed. AMD RGP requires an AMD GPU
and a driver that exposes the RGP control interface (GetRGPControl).
```

The AMD Radeon(TM) Graphics iGPU does not expose `GetRGPControl`; RGP generally
needs a discrete Radeon with RGP-capable drivers. This is the same condition the
upstream core reports (`RGP Capture is not supported on this API implementation`),
so the code path is complete and the failure is hardware capability, not a
missing feature. NVIDIA captures cannot be replayed on AMD (the core rejects
cross-vendor replay), so a genuinely RGP-capable AMD GPU is required to produce
a profile.

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
