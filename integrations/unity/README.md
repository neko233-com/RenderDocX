# Unity / Tuanjie integration (early)

Add this directory as a local package in Package Manager. In the Editor, set the RenderDocX executable under **Tools > RenderDocX**, then open a saved `.rdc` file or the workspace. The frontend can discover and connect to an already-running local RenderDoc target without taking over a busy client; automatic Editor connection and capture handoff are not implemented and the discovery flow has not yet been verified with Unity or Tuanjie.

`RenderDocX.GpuCapture.TryBegin(out var capture)` optionally wraps Unity's experimental `ExternalGPUProfiler` API. Call on the main thread around appropriate rendering work and dispose the returned scope in `finally` or a `using` statement. `IsAttached` can also indicate a profiler other than RenderDoc. The scope does not load RenderDoc, ensure a frame is captured, choose a destination, or automatically open the result. Use Unity's built-in RenderDoc integration for the Editor's Game/Scene view; its operation is subject to the engine's supported graphics APIs and platforms.

This package targets the documented Unity 2020.3 API surface. Tuanjie compatibility depends on its actual API and engine version and has not been established. No Editor, Player, GPU, or capture-flow validation has been performed.
