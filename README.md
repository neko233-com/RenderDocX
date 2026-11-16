# RenderDocX

RenderDocX is an in-progress Rust + GPUI desktop frontend for [RenderDoc](https://github.com/baldurk/renderdoc). Its goal is a clearer, more usable graphics-debugging experience while retaining RenderDoc's capture, replay, and file semantics.

**Status: early prototype, not a replacement for RenderDoc.** The Rust workspace can launch an application, connect to a local RenderDoc target, trigger a capture and open reported local captures. It can open an `.rdc` with its version-matched replay helper, expand or collapse the action hierarchy, browse resource metadata, request event replay, inspect common pipeline state, bound shader disassembly and buffer bytes, select a texture mip, slice, sample and channel for preview, and export it to PNG without overwriting existing files. Preview and export use RenderDoc's 8-bit PNG mapping. Some detailed inspection and `.cap` files still use `qrenderdoc`. A submitted capture request is not proof that a capture was produced.

The event browser can filter action names using case-insensitive literal text, retaining matching actions' ancestors and existing expansion state. RenderDoc's full event-filter expression language is not implemented yet.

Select an action and open **API calls** to inspect and replay its associated API events. The list currently shows structured chunk names rather than fully formatted call parameters; displaying API events directly in the main hierarchy remains work in progress.

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

Unity's built-in RenderDoc integration can load RenderDoc in the Editor and capture from the Game or Scene view on supported platform/API combinations. RenderDocX can discover local targets already running with RenderDoc and attempt a non-forced connection. See the [Unity manual](https://docs.unity3d.com/Manual/RenderDocIntegration.html) and the engine version's documentation. An early [Unity package](integrations/unity/README.md) provides saved capture handoff, an Editor-PID connection command, and an optional wrapper for Unity's experimental external GPU profiler API. Package installation and handoff construction have been checked in Unity 2022.3; the Agent CLI connected to a Direct3D 11 batch Editor, respected a busy client and reconnected. The Editor menu, frame capture and Tuanjie compatibility remain unverified.

## Source and compatibility

This repository includes a RenderDoc source snapshot identifying itself as version 1.48. The supplied snapshot had no Git history. Its files were compared to upstream `v1.x` commit [`5ad608809c9028da9f5c5082397cb6afe9c8dd17`](https://github.com/baldurk/renderdoc/commit/5ad608809c9028da9f5c5082397cb6afe9c8dd17): 5,979 initially tracked files and 50 initially ignored upstream files, including binaries, matched their upstream Git blob IDs. The project README and `.gitignore` differ; upstream GitHub metadata was moved to inactive reference locations. This is file-level evidence, not recovered Git ancestry. The existing capture/replay core and Qt frontend are retained; they have not been translated to Rust. The upstream README is preserved at [docs/README.renderdoc-upstream.md](docs/README.renderdoc-upstream.md), and upstream GitHub workflows under `docs/upstream-github-workflows/`.

The native event browser, texture preview, and event replay are initial implementations. Full texture inspection and mesh views, broader resource and pipeline inspection, target control, remote capture, extensions, accessibility, and full RenderDoc GUI parity remain incomplete. See [docs/renderdocx-roadmap.md](docs/renderdocx-roadmap.md).

## Verification

On Windows x64, `./scripts/test-windows.ps1` builds the matching core and helpers, runs headless protocol and Agent CLI tests, exercises generated D3D11 capture and replay as well as live target discovery, connection and capture, and runs GPUI interaction tests. Add `-IncludeFast` to include the experimental backend. The real-core tests need a supported D3D11 device or WARP and are marked ignored when running `cargo test` alone. Other graphics APIs, GPU combinations, Unity/Tuanjie, and platforms still need coverage.

## License

The included RenderDoc source is MIT licensed. See [LICENSE.md](LICENSE.md) and [third-party acknowledgements](docs/credits_acknowledgements.rst). Rust dependencies have their own licenses.
