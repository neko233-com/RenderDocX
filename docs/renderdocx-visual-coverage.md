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

## Executed desktop-driving verification (real window)

`scripts/desktop-drive.ps1` was run against the release binary and produced:

- `target/visual/uia-tree.json`: **76 UI Automation elements** enumerated from the
  real GPUI window (AccessKit tree), e.g. `日志`, `关于`, `主题: 深色`,
  `检查 .rdc…`, `Python 脚本`.
- `target/visual/drive-01-initial.png` (dark theme) and
  `drive-02-after-invoke.png` (light theme), both 1056×799.
- `target/visual/msi-installed-app.png`: the latest app launched from its
  **installed** per-user location after a real `msiexec /i` (exit 0), captured at
  1582×1196; `msiexec /x` then exited 0 and removed the payload.
- Invoking the `主题: 深色` control through UI Automation reached the real
  Workspace (`Invoked: 主题: 深色`). The latest run re-enumerated 76 elements and
  reported 0 new nodes because the toggle replaces its label in place; the
  before/after pixels are the definitive evidence.
- Pixel comparison of the two screenshots: **385,541 / 843,744 pixels differ
  (45.69%)** — the theme change is visible in captured pixels.

Commands:

```powershell
pwsh -File scripts/desktop-drive.ps1 -Exe target/release/renderdocx.exe -DumpTree
pwsh -File scripts/desktop-drive.ps1 -InvokeName "主题: 深色"
```

Notes learned empirically: UIA bounding rectangles are client-area coordinates,
and scrolling updates them (a control moved from y=1360 to y=580). The script
pins the window at (0,0) with `SetWindowPos` so client coordinates equal screen
coordinates, scrolls controls into view, and prefers `InvokePattern` (falling
back to a real mouse click at the element centre).

## Environment limits found while verifying

- `.NET`'s `Process.MainWindowHandle` returns `0x0` for the GPUI window even
  when it is on screen, so `visual-verify.ps1`'s handle lookup fails. The UI
  Automation path in `desktop-drive.ps1` works and is the supported method; the
  earlier "no interactive desktop" conclusion was wrong for this session.
- `gpui-pre` still has no Windows headless renderer: `current_headless_renderer()`
  returns `Ok(None)` on Windows, and `gpui-kit`'s screenshot tests are
  `#[cfg(any(target_os = "macos", target_os = "linux"))]`. Screenshots therefore
  require the real window, which `desktop-drive.ps1` uses.
- `agent-desktop` (a native desktop driver MCP service) is not present in this
  session's tool set. `desktop-drive.ps1` provides equivalent driving with
  Windows UI Automation + Win32 input/BitBlt, and was executed successfully
  (see the section above).
