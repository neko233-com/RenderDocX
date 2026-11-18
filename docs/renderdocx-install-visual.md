# RenderDocX MSI packaging and visual verification

## MSI installer (Windows x64)

RenderDocX ships its own WiX v4/v5 package at `installer/RenderDocX.wxs`. It is
distinct from the upstream RenderDoc installers in `util/installer/*.wxs`
(those are RenderDoc, manufacturer Baldur Karlsson; they are reference only).

Prerequisites:

- WiX v4/v5 CLI (`wix.exe`), e.g. `dotnet tool install -g wix`
- Rust toolchain with the GUI prerequisites from GPUI Kit

Build the installer with:

```powershell
pwsh -File scripts/build-msi.ps1            # builds release binaries, then the MSI
pwsh -File scripts/build-msi.ps1 -SkipBuild   # reuse binaries already built
```

The script reads the version from `Cargo.toml`, runs `cargo build --release
--bins`, and produces `target/msi/RenderDocX-<version>-x64.msi`. It prints the
SHA-256 of the output. Contents: `renderdocx.exe`, `renderdocx-cli.exe`, the
README and the feature-matrix/roadmap documents, plus Start-menu shortcuts.

Verified in this checkout with WiX 5.0.2: a `0.1.0.0` x64 MSI was produced from
the release binaries (11.18 MB, SHA-256
`14A70EB43D0650704A6B1F0A9C207163582AD2A8C496200E68C30D706BC4BB87`). Its contents were verified
non-interactively with an administrative install
(`msiexec /a RenderDocX-0.1.0-x64.msi /qn TARGETDIR=<dir>`), which exited 0 and
extracted exactly: `PFiles64/RenderDocX/renderdocx.exe` (34.7 MB),
`renderdocx-cli.exe` (5.5 MB), `README.md`, `renderdocx-feature-matrix.md`,
`renderdocx-roadmap.md`.

**Full install → run → screenshot → uninstall** was also executed with
`scripts/verify-msi.ps1` (per-user silent install, no elevation needed):

```
msiexec /i exit code: 0
Installed executable: C:\Users\<user>\AppData\Local\Programs\RenderDocX\renderdocx.exe
Captured installed app 1582x1196 -> target/visual/msi-installed-app.png
msiexec /x exit code: 0
Uninstall removed payload: True
```

The installed binary's real window was captured at 1582×1196 through UI
Automation + BitBlt (the screenshot shows the Chinese workspace UI and the
embedded-CPython extension description), and uninstall removed the payload.
Logs are written to `target/visual/msi-install.log` / `msi-uninstall.log`.

## Visual verification

Two complementary mechanisms are provided:

1. **Element-tree assertions (headless-friendly).** `cargo test --features
   ui-tests` runs 13 GPUI tests that drive the real `Workspace` and assert
   element visibility and interaction (`window.find("...").visible()`),
   covering the log panel, config editor, texture preview, histogram, shader
   source/debug views, event tree and more. These are machine-readable visual
   structure checks and run in any environment.

2. **Native window screenshot capture.** `scripts/visual-verify.ps1` launches
   the app, locates its top-level Win32 window via `MainWindowHandle`, and
   captures real on-screen pixels with `BitBlt` from the screen device context
   (which also captures GPU-composited content) into `target/visual/*.png`:

   ```powershell
   pwsh -File scripts/visual-verify.ps1 -Exe target/release/renderdocx.exe
   ```

   The resulting PNGs can be reviewed with the browser `preview` tool. This
   script requires an interactive Windows desktop session; on a headless/agent
   session the GPUI window cannot be created (the process runs but exposes no
   top-level window), so no pixels can be captured.

For desktop-driving verification, `scripts/desktop-drive.ps1` uses Windows UI
Automation to enumerate the GPUI accessibility tree, scroll controls into view,
invoke them by name/id, and capture before/after PNGs:

```powershell
pwsh -File scripts/desktop-drive.ps1 -Exe target/release/renderdocx.exe -DumpTree
pwsh -File scripts/desktop-drive.ps1 -InvokeName "主题: 深色"
```

It was executed on this checkout: 76 UIA elements, a successful
`Invoked: 主题: 深色` through UI Automation, and before/after screenshots
differing in 45.69% of pixels (385,541 / 843,744). `agent-desktop` was not
present in the authoring session's tool set; this script is the equivalent
native driver.
