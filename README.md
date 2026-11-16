# RenderDocX

RenderDocX is an in-progress Rust + GPUI desktop frontend for [RenderDoc](https://github.com/baldurk/renderdoc). Its goal is a clearer, more usable graphics-debugging experience while retaining RenderDoc's capture, replay, and file semantics.

**Status: early prototype, not a replacement for RenderDoc.** The current GPUI workspace can select and open `.rdc`/`.cap` files in the installed `qrenderdoc`, start a target using `renderdoccmd capture`, and open the upstream UI. On launch it uses the target ident reported by `renderdoccmd` to start `qrenderdoc` with `--targetcontrol`. Detailed capture control and inspection remain in the upstream UI. Starting these processes does not establish that the target connection, capture, or replay succeeded.

The interface detects the system language on startup and supports English, Simplified Chinese, Japanese, and Korean, with a manual language override. Its settings are stored in the operating system's user configuration directory.

## Build and run

Install Rust and the platform prerequisites for [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui), then run:

```sh
cargo run --release
```

Install or build a compatible RenderDoc distribution separately. RenderDocX looks for `qrenderdoc` and `renderdoccmd` next to its executable, in the standard Windows RenderDoc installation directory, or on `PATH`. The workspace also offers buttons to select each executable explicitly. Those binaries and their matching RenderDoc libraries must come from the same distribution; this prototype does not bundle them or check their version.

Launching an application uses RenderDoc's default capture options and accepts no application arguments yet. Use the upstream UI for full configuration and capture control. Only capture applications you own or are authorized to debug, in accordance with upstream's usage policy.

## Unity and Tuanjie

Unity's built-in RenderDoc integration can load RenderDoc in the Editor and capture from the Game or Scene view on supported platform/API combinations. Standalone players can be launched through RenderDoc. See the [Unity manual](https://docs.unity3d.com/Manual/RenderDocIntegration.html) and the engine version's documentation. An early [Unity package](integrations/unity/README.md) adds Editor handoff of saved captures and an optional wrapper for Unity's experimental external GPU profiler API. Tuanjie compatibility has not been established. RenderDocX does not yet receive Editor capture events directly or replace Unity's RenderDoc UI integration.

## Source and compatibility

This repository includes a RenderDoc source snapshot identifying itself as version 1.48. Its exact upstream commit is not present in the supplied snapshot. The existing capture/replay core and Qt frontend are retained for reference and compatibility; they have not been translated to Rust. The upstream README is preserved at [docs/README.renderdoc-upstream.md](docs/README.renderdoc-upstream.md). Upstream GitHub workflows are preserved as inactive reference files under `docs/upstream-github-workflows/`.

Native event browsing, resource and pipeline inspection, replay, target control, remote capture, extensions, accessibility, and complete platform parity are not implemented in the GPUI frontend. See [docs/renderdocx-roadmap.md](docs/renderdocx-roadmap.md).

## License

The included RenderDoc source is MIT licensed. See [LICENSE.md](LICENSE.md) and [third-party acknowledgements](docs/credits_acknowledgements.rst). Rust dependencies have their own licenses.
