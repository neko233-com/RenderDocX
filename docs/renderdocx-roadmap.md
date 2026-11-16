# RenderDocX migration boundaries

The supplied upstream snapshot has version macros for RenderDoc 1.48 but no Git metadata. Establish its exact commit before treating it as a reproducible upstream baseline. Keep capture and replay in the matching RenderDoc core; the public replay API includes C++ types, so use a version-matched C ABI bridge with explicit ownership, thread affinity, and error handling rather than guessing the ABI from Rust. The application API in `renderdoc/api/app/renderdoc_app.h` is a separate, versioned in-process capture interface.

The current workspace invokes upstream executables. `renderdoccmd capture` does not give this workspace a target-control session, progress, or a list of saved captures. `qrenderdoc` remains necessary to inspect a capture and for full capture options. There is no native `.rdc` parser, live session, or capture/replay implementation in the Rust frontend.

Remaining work includes:

1. Establish the source provenance and a reproducible RenderDoc core build; implement a small, version-checked bridge and lifecycle management.
2. Integrate target enumeration/control, launch configuration, capture progress, saved captures, remote servers, and errors using the corresponding upstream contracts.
3. Port capture inspection incrementally: event browser, texture and mesh views, API pipeline state, resources, shader debugging, and the remaining upstream windows and workflows.
4. Integrate Unity/Tuanjie Editor and standalone workflows against the applicable documented engine APIs, graphics APIs, and versions, including capture handoff to the new frontend.
5. Extend translation resources, keyboard navigation, accessibility, settings, packaging, and platform support alongside each implemented feature.
6. Establish compatibility with real captures and supported GPU/engine combinations before claiming parity. Compilation alone does not establish these properties.
