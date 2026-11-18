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
the release binaries (≈10 MB). Installing/uninstalling on a target machine has
not been exercised here; that requires an interactive desktop session.

## Visual verification

Two complementary mechanisms are provided:

1. **Element-tree assertions (headless-friendly).** `cargo test --features
   ui-tests` runs 12 GPUI tests that drive the real `Workspace` and assert
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

`agent-desktop` native desktop driving was not available in the session where
this was written, so automated pointer/keyboard driving of the GPUI window is
not claimed. Where a desktop driver is available, the screenshot script plus
the GPUI element-tree tests together provide repeatable visual verification.
