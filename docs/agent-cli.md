# Agent CLI (initial contract)

`renderdocx-cli` exposes the same native replay session used by the GPUI event browser. It does not control `qrenderdoc`. Build and configure the matching `renderdocx-replay` bridge first; `--bridge PATH` overrides the user setting for one invocation.

```text
renderdocx-cli capabilities
renderdocx-cli --bridge PATH inspect CAPTURE.rdc
renderdocx-cli --bridge PATH select CAPTURE.rdc EVENT_ID
```

Each invocation writes one JSON object on stdout with `schema: "renderdocx.cli.v1"` and `ok`. Failures use a nonzero exit code and include `error.code` and `error.message`. `inspect` returns the upstream action tree flattened in depth-first order; each `parent` is the zero-based action array index or `-1` for a root. IDs, parent indices, depths, flags, and counts are exact integers. `select` opens its own replay session and sets the requested action's event. The current CLI does not retain state across invocations and cannot yet inspect pipeline state, pixels, shaders, targets, or remote captures.

The helper and its RenderDoc core must be built from matching source revisions. `select` returning success means the upstream replay call returned without a fatal error, not that the rendered output was visually validated.
