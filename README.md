# RenderDocX

RenderDocX is an in-progress Rust + GPUI desktop frontend for [RenderDoc](https://github.com/baldurk/renderdoc). Its goal is a clearer, more usable graphics-debugging experience while retaining RenderDoc's capture, replay, and file semantics.

**Status: early prototype, not a replacement for RenderDoc.** The Rust workspace can open an `.rdc` with its version-matched replay helper, browse the action hierarchy, and request event replay. Detailed inspection is still performed in `qrenderdoc`. `.cap` files and application launch use upstream tools; launch passes the target ident from `renderdoccmd` to `qrenderdoc --targetcontrol`. Starting these processes does not establish that the target connection or capture succeeded.

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

Select `target/renderdocx-runtime/renderdocx-replay.exe` in the workspace. The helper checks the loaded core's version and build commit before using the C++ replay API. This build path currently targets Windows x64; the source-level CMake bridge can also be built against a matching Linux core, but that path has not yet been verified. The application and helper are not yet packaged together.

The machine-readable [Agent CLI](docs/agent-cli.md) can be built without the GUI dependencies using `cargo build --no-default-features --bin renderdocx-cli`.

Install or build a compatible RenderDoc distribution separately. RenderDocX looks for `qrenderdoc` and `renderdoccmd` next to its executable, in the standard Windows RenderDoc installation directory, or on `PATH`. The workspace also offers buttons to select each executable explicitly. Those binaries and their matching RenderDoc libraries must come from the same distribution; this prototype does not bundle them or check their version.

Launching an application uses RenderDoc's default capture options and accepts no application arguments yet. Use the upstream UI for full configuration and capture control. Only capture applications you own or are authorized to debug, in accordance with upstream's usage policy.

## Unity and Tuanjie

Unity's built-in RenderDoc integration can load RenderDoc in the Editor and capture from the Game or Scene view on supported platform/API combinations. Standalone players can be launched through RenderDoc. See the [Unity manual](https://docs.unity3d.com/Manual/RenderDocIntegration.html) and the engine version's documentation. An early [Unity package](integrations/unity/README.md) adds Editor handoff of saved captures and an optional wrapper for Unity's experimental external GPU profiler API. Tuanjie compatibility has not been established. RenderDocX does not yet receive Editor capture events directly or replace Unity's RenderDoc UI integration.

## Source and compatibility

This repository includes a RenderDoc source snapshot identifying itself as version 1.48. The supplied snapshot had no Git history. Its files were compared to upstream `v1.x` commit [`5ad608809c9028da9f5c5082397cb6afe9c8dd17`](https://github.com/baldurk/renderdoc/commit/5ad608809c9028da9f5c5082397cb6afe9c8dd17): 5,979 initially tracked files and 50 initially ignored upstream files, including binaries, matched their upstream Git blob IDs. The project README and `.gitignore` differ; upstream GitHub metadata was moved to inactive reference locations. This is file-level evidence, not recovered Git ancestry. The existing capture/replay core and Qt frontend are retained; they have not been translated to Rust. The upstream README is preserved at [docs/README.renderdoc-upstream.md](docs/README.renderdoc-upstream.md), and upstream GitHub workflows under `docs/upstream-github-workflows/`.

The native event browser and event replay are initial implementations. Texture and mesh views, resource and pipeline inspection, target control, remote capture, extensions, accessibility, and full RenderDoc GUI parity remain incomplete. See [docs/renderdocx-roadmap.md](docs/renderdocx-roadmap.md).

## Verification

On Windows x64, `./scripts/test-windows.ps1` builds the matching core and helper, runs headless protocol and Agent CLI tests, exercises a generated D3D11 capture and replay, and runs GPUI interaction tests. Add `-IncludeFast` to include the experimental backend. The real-core tests need a supported D3D11 device or WARP and are marked ignored when running `cargo test` alone. Other graphics APIs, GPU combinations, Unity/Tuanjie, and platforms still need coverage.

## License

The included RenderDoc source is MIT licensed. See [LICENSE.md](LICENSE.md) and [third-party acknowledgements](docs/credits_acknowledgements.rst). Rust dependencies have their own licenses.
