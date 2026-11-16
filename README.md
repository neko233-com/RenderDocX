# RenderDocX

RenderDocX is an in-progress Rust + GPUI desktop frontend for [RenderDoc](https://github.com/baldurk/renderdoc). Its goal is a clearer, more usable graphics-debugging experience while retaining RenderDoc's capture, replay, and file semantics.

**Status: early prototype, not a replacement for RenderDoc.** The Rust workspace can launch an application, connect to a local RenderDoc target, trigger a capture and open reported local captures. It can open an `.rdc` with its version-matched replay helper, expand or collapse the action hierarchy, browse resource metadata, request event replay, inspect common pipeline state, bound shader disassembly and buffer bytes, select a texture mip, slice, sample and channel for preview, and export it to PNG without overwriting existing files. Preview and export use RenderDoc's 8-bit PNG mapping. Some detailed inspection and `.cap` files still use `qrenderdoc`. A submitted capture request is not proof that a capture was produced.

The event browser can filter visible event names using case-insensitive literal text, retaining matching events' ancestors and existing expansion state. It defaults to action rows; **Show API events** also displays preceding API calls as siblings of their associated actions. RenderDoc's full event-filter expression language is not implemented yet.

Select an action and open **API calls** to inspect its associated API events, or select those events in the main hierarchy after enabling them. Both views can replay the selected event. **Parameters** browses structured capture data on demand, including nested children and pages. Buffer nodes offer a paged byte view; the first read creates an isolated worker and exports the full structured capture, which can require substantial extra memory and time. Event names still use structured chunk names rather than RenderDoc's fully formatted call summaries.

The texture details view can pick a pixel at explicit top-left coordinates for the selected mip, slice and sample. It shows RenderDoc's returned component bits and their float, signed and unsigned interpretations; these are not necessarily the texture's original storage bytes.

Pixel history lists modifications through the selected event, including fragment test failures and before, shader-output and after values. Select an entry to inspect it or replay its event. History can require substantial GPU replay work, and an empty result can also indicate that the upstream backend could not inspect the pixel.

The same view can calculate RenderDoc's per-channel minimum and maximum and display a histogram for a chosen numeric range and channel. RenderDoc's public minimum/maximum call does not report a separate success status, so its returned values cannot by themselves prove the operation succeeded.

**Resources** lists captured resources and their initialization chunk indices, parent and derived relationships, and paged static usage. Filter by name, type or session-scoped `#index`, and select a recorded usage to replay its event. Some resource types do not have usage tracking in RenderDoc; that state is shown separately from an empty tracked list.

**Mesh** queries post-shader output for a selected draw and displays paged positions when RenderDoc reports a supported float position format; otherwise it displays raw bytes. Positions follow buffer order, not draw-index order. The current desktop view uses instance 0 and view 0 and is not yet the complete RenderDoc mesh viewer. The agent CLI also exposes the index buffer and instance selection.

**Vertex input** displays used attributes in draw order, applying the selected draw's indices, base vertex, instance rate and buffer offsets. Common floating-point, integer, normalized and packed formats are decoded; other formats show their raw bytes. OpenGL generic attributes are supported when shader reflection is available. The desktop view shows instance 0; the agent CLI can select another instance.

Select a vertex-input row and choose **Debug vertex** to collect a RenderDoc vertex shader trace, then browse its states, variable changes and disassembly. The selected input index includes the draw offsets. The desktop currently uses instance 0 and view 0.

The texture view can also debug a pixel of the selected draw at the entered top-left coordinates, or debug a selected pixel-history fragment when its event is the current event. The latter passes the recorded primitive and sample to RenderDoc; view remains unspecified. The selected texture is not necessarily bound to the draw, and backend selector support varies. Both views collect the complete trace before displaying it, which can take substantial time and memory. When RenderDoc provides mappings, each step shows its default-disassembly line, source location and source-variable references; embedded source availability depends on shader debug information. The trace can be stepped by instruction or source location and run forward or backward to source or disassembly breakpoints. Source step-over and step-out and mutable-variable reconstruction are implemented, with further real-shader validation needed; other shader stages remain to be implemented. Real D3D11 fixture coverage checks the desktop interactions; other APIs and engines are not yet validated for these workflows.

For a compute dispatch, select the compute shader and enter the workgroup and group-local thread coordinates to inspect its trace. RenderDoc's recorded dispatch dimensions and shader reflection determine the valid range; unavailable dimensions and out-of-range coordinates are rejected. A real D3D11 fixture exercises nonzero group and thread coordinates. Other APIs, indirect dispatches and engine workflows are not yet validated.

The interface detects the system language on startup and supports English, Simplified Chinese, Japanese, and Korean, with a manual language override. Its settings are stored in the operating system's user configuration directory.

## Build and run

Install Rust and the platform prerequisites for [GPUI Kit](https://github.com/longbridge/gpui-kit), then run:

```sh
cargo run --release
```

The UI uses GPUI Kit's pinned GPUI snapshot backend by default. The experimental GPUI Fast backend is available with `--features gpui-fast`; its performance and compatibility must be measured on the intended workload before deployment.

On Windows, build the RenderDoc core and its matching replay helper from a committed checkout with 64-bit PowerShell 7, Visual Studio C++ tools, and CMake:

```powershell
./scripts/build-replay-bridge.ps1
```

Select `target/renderdocx-runtime/renderdocx-replay.exe` in the workspace. The replay and target helpers must stay together with their matching RenderDoc core; both check the loaded core's version and build commit before using the C++ API. This build path currently targets Windows x64; the source-level CMake bridge can also be built against a matching Linux core, but that path has not yet been verified. The application and helpers are not yet packaged together.

The machine-readable [Agent CLI](docs/agent-cli.md) supports one-shot commands and a persistent JSONL replay session. It can be built without the GUI dependencies using `cargo build --no-default-features --bin renderdocx-cli`.

For functionality not yet migrated, RenderDocX can open `qrenderdoc` separately. It searches next to the application, the standard Windows RenderDoc installation directory, or `PATH`; the workspace can also select it explicitly.

The GUI and Agent CLI offer RenderDoc capture options, an optional working directory, application arguments and ordered environment modifications. Each GUI launch writes to a fresh session directory under the user application-data captures directory. Remote workflows and some upstream configuration remain incomplete. Only capture applications you own or are authorized to debug, in accordance with upstream's usage policy.

## Unity and Tuanjie

Unity's built-in RenderDoc integration can load RenderDoc in the Editor and capture from the Game or Scene view on supported platform/API combinations. RenderDocX can discover local targets already running with RenderDoc and attempt a non-forced connection. See the [Unity manual](https://docs.unity3d.com/Manual/RenderDocIntegration.html) and the engine version's documentation. An early [Unity package](integrations/unity/README.md) provides saved capture handoff, an Editor-PID connection command, and an optional wrapper for Unity's experimental external GPU profiler API. Package installation and handoff construction have been checked in Unity 2022.3; the Agent CLI connected to a Direct3D 11 batch Editor, respected a busy client and reconnected. An isolated Windows Development Player has also produced a D3D11 capture through the package and replayed an indexed draw. The Editor menu and Editor frame capture, other engine versions and Tuanjie remain unverified.

## Source and compatibility

This repository includes a RenderDoc source snapshot identifying itself as version 1.48. The supplied snapshot had no Git history. Its files were compared to upstream `v1.x` commit [`5ad608809c9028da9f5c5082397cb6afe9c8dd17`](https://github.com/baldurk/renderdoc/commit/5ad608809c9028da9f5c5082397cb6afe9c8dd17): 5,979 initially tracked files and 50 initially ignored upstream files, including binaries, matched their upstream Git blob IDs. The project README and `.gitignore` differ; upstream GitHub metadata was moved to inactive reference locations. This is file-level evidence, not recovered Git ancestry. The existing capture/replay core and Qt frontend are retained; they have not been translated to Rust. The upstream README is preserved at [docs/README.renderdoc-upstream.md](docs/README.renderdoc-upstream.md), and upstream GitHub workflows under `docs/upstream-github-workflows/`.

The native event browser, texture preview, and event replay are initial implementations. Full texture inspection and mesh views, broader resource and pipeline inspection, target control, remote capture, extensions, accessibility, and full RenderDoc GUI parity remain incomplete. See [docs/renderdocx-roadmap.md](docs/renderdocx-roadmap.md).

## Verification

On Windows x64, `./scripts/test-windows.ps1` builds the matching core and helpers, runs headless protocol and Agent CLI tests, exercises generated D3D11 capture and replay as well as live target discovery, connection and capture, and runs GPUI interaction tests with both a recording transport and a generated D3D11 capture. Add `-IncludeFast` to include the experimental backend, and `-IncludeOpenGL` for OpenGL 3.3 capture and input inspection tests. The real-core tests need a supported D3D11 device or WARP; OpenGL tests require a compatible graphics driver. The headless real-core tests are marked ignored when running `cargo test` alone. Additional graphics APIs, GPU combinations, Unity/Tuanjie, and platforms still need coverage.

## License

The included RenderDoc source is MIT licensed. See [LICENSE.md](LICENSE.md) and [third-party acknowledgements](docs/credits_acknowledgements.rst). Rust dependencies have their own licenses.

The workspace can upload a local capture to a RenderDoc remote replay server and inspect it through the native replay proxy. Enter the replay server address in the remote address field and choose **Replay capture on remote server**. Remote target discovery and replay are separate services and may use different ports. Annotation files remain local; structured-buffer export uses a separate local structured-data processor. Windows D3D11 loopback has been tested; cross-machine and other graphics API support remain unverified.

Remote capture can also launch an application through a RenderDoc remote server: enter its address, the application path and an optional working directory on that server, configure capture options and choose **Launch remote capture**. The remote server must allow execution. Windows loopback D3D11 launch and capture are tested; cross-machine and other graphics API workflows remain unverified.
