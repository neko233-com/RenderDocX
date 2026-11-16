# Agent CLI (initial contract)

`renderdocx-cli` exposes the same native replay session used by the GPUI event browser. It does not control `qrenderdoc`. Build and configure the matching `renderdocx-replay` bridge first; `--bridge PATH` overrides the user setting for one invocation.

```text
renderdocx-cli capabilities
renderdocx-cli --bridge PATH inspect CAPTURE.rdc
renderdocx-cli --bridge PATH select CAPTURE.rdc EVENT_ID
renderdocx-cli --bridge PATH serve
```

One-shot invocations write one JSON object on stdout with `schema: "renderdocx.cli.v1"` and `ok`. Failures use a nonzero exit code and include `error.code` and `error.message`. `inspect` returns the upstream action tree flattened in depth-first order; each `parent` is the zero-based action array index or `-1` for a root. It also includes texture metadata; `bytes` is a decimal string to preserve the complete 64-bit value in JavaScript. The array ordering is scoped to this replay session and is not a persistent resource identifier. One-shot `select` opens its own replay session and sets the requested action's event.

`serve` keeps a replay session open and accepts one JSON object per stdin line. It handles requests sequentially and emits exactly one JSON response per line on stdout, using `schema: "renderdocx.agent.v1"`, echoing the optional `id`, and returning either `data` or `error.code`/`error.message`. The commands are `capabilities`, `open` (`path` is a nonempty string), `snapshot`, `select` (`event_id` is an unsigned 32-bit integer), `export_texture` (`index` is the zero-based texture index from the current session and `path` is the destination), `close`, and `quit`. For example:

```json
{"id":1,"command":"open","path":"C:/captures/frame.rdc"}
{"id":2,"command":"select","event_id":42}
{"id":3,"command":"export_texture","index":0,"path":"C:/captures/texture.png"}
{"id":4,"command":"close"}
{"id":5,"command":"quit"}
```

An unsuccessful `open` leaves the previous session intact. `export_texture` requires a selected event and writes PNG using RenderDoc's default mapping; it never replaces an existing destination. `close` and `quit` wait for the replay helper to shut down, releasing its capture file before acknowledging. The server opens no network listener and performs no authentication; only processes with access to its stdin can send commands. Pipeline state, raw pixels, shaders, targets, and remote capture are not yet exposed.

The helper and its RenderDoc core must be built from matching source revisions. `select` returning success means the upstream replay call returned without a fatal error, not that the rendered output was visually validated.
