# RenderDocX visual verification coverage

This file records how each major GPUI surface is verified, so "visual verification"
can be checked rather than asserted.

## Layers of evidence

| Layer | What it proves | Command |
|---|---|---|
| GPUI element-tree assertions | The real `Workspace` renders the panel and its key elements are visible/clickable | `cargo test --features ui-tests` |
| Pure-Rust unit tests | Format/state/dialog logic behind the panel (parsers, validators, matrix math, injection/hook state, crash/update/analytics) | `cargo test --lib` |
| Native window screenshots | Actual on-screen pixels of the GPUI window | `pwsh -File scripts/visual-verify.ps1` (requires an interactive desktop) |
| MSI package contents | Installer payload is complete | `msiexec /a … /qn TARGETDIR=…` |

## Currently asserted element ids (54)

`analytics-toggle`, `buffer-full-resource`, `buffer-layout-input`, `buffer-next`,
`capture-details-scroll`, `capture-start`, `config-editor`, `config-editor-cancel`,
`config-editor-open`, `crash-clear`, `crash-folder`, `crash-refresh`,
`event-action-42`, `event-api-0-0`, `event-breadcrumbs`, `event-eid-42`,
`event-eid-43`, `event-header-action`, `event-header-eid`, `event-toggle-0`,
`extensions-refresh`, `frame-stats-export`, `frame-stats-raw`,
`geometry-message-approximation`, `help-about`, `help-log`, `info-close`,
`info-panel`, `inspect-rdc`, `launch-env-name`, `launch-env-remove-0`,
`live-capture-0`, `log-close`, `log-panel`, `mesh-message-debug-unavailable`,
`pixel-context`, `pixel-depth-stencil`, `pixel-history`, `pixel-history-debug`,
`pixel-history-jump`, `shader-debug-current-instruction`,
`shader-debug-current-source`, `shader-disassembly`, `shader-message-debug`,
`shader-message-jump-result`, `shader-source-breakpoint`, `shader-source-code`,
`target-38920`, `texture-histogram`, `texture-preview-depth`,
`texture-preview-image`, `timeline-axis`, `timeline-background`, `update-check`.

## Panels without a window assertion (unit-tested logic or desktop-only)

- Matrices/inspector panels behind a loaded capture (`texture-*`, `mesh-*`,
  `pixel-*`) need a replay session; their models are unit-tested and the
  session-level flows are covered by the ignored real-GPU tests.
- Global-hook / injection / RGP extraction binary paths are unit-tested
  (`global_hook`, `inject`, `rdc_file::extract_rgp_profile`) and surfaced by
  the buttons asserted above where reachable without a capture.
- True pixel/pointer verification needs an interactive desktop and is provided
  by `scripts/visual-verify.ps1`.

## Environment limits found while verifying

- This session has no interactive desktop: the GUI process runs but exposes no
  top-level window (`MainWindowHandle=0x0`), so `visual-verify.ps1` cannot
  capture pixels here.
- `gpui-pre` has no Windows headless renderer: `current_headless_renderer()`
  returns `Ok(None)` on Windows, and `gpui-kit`'s screenshot tests are
  `#[cfg(any(target_os = "macos", target_os = "linux"))]`.
- `agent-desktop` (native desktop driver) is not available in this session's
  tool set; only browser automation tools are present.
