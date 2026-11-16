# Agent CLI (initial contract)

`renderdocx-cli` exposes the same native replay session used by the GPUI event browser. It does not control `qrenderdoc`. Build and configure the matching `renderdocx-replay` bridge first; `--bridge PATH` overrides the user setting for one invocation.

```text
renderdocx-cli capabilities
renderdocx-cli --bridge PATH inspect CAPTURE.rdc
renderdocx-cli --bridge PATH select CAPTURE.rdc EVENT_ID
renderdocx-cli --bridge PATH serve
```

One-shot invocations write one JSON object on stdout with `schema: "renderdocx.cli.v1"` and `ok`. Failures use a nonzero exit code and include `error.code` and `error.message`. `inspect` returns the upstream action tree flattened in depth-first order; each `parent` is the zero-based action array index or `-1` for a root. It also includes texture metadata; `bytes` is a decimal string to preserve the complete 64-bit value in JavaScript. The array ordering is scoped to this replay session and is not a persistent resource identifier. One-shot `select` opens its own replay session, sets the requested event and returns its common pipeline state.

`serve` keeps replay and target sessions open and accepts one JSON object per stdin line. It handles requests sequentially and emits exactly one JSON response per line on stdout, using `schema: "renderdocx.agent.v1"`, echoing the optional `id`, and returning either `data` or `error.code`/`error.message`. Commands include `capabilities`, `open`, `snapshot`, `select`, `pipeline`, `shader`, `read_buffer`, `export_texture`, `close`, the target commands below, and `quit`. For example:

```json
{"id":1,"command":"open","path":"C:/captures/frame.rdc"}
{"id":2,"command":"select","event_id":42}
{"id":"pipeline","command":"pipeline"}
{"id":"shader","command":"shader","stage":"vertex"}
{"id":"bytes","command":"read_buffer","index":0,"offset":"0","length":256}
{"id":3,"command":"export_texture","index":0,"path":"C:/captures/texture.png"}
{"id":"layer","command":"export_texture","index":0,"path":"C:/captures/layer.png","mip":1,"slice":0,"sample":null,"channel":0}
{"id":4,"command":"close"}
{"id":5,"command":"quit"}
```

Live control uses the matching `renderdocx-target` helper next to the configured replay bridge. `target_list` enumerates local RenderDoc target identifiers; `target_connect` takes a nonzero `ident` and never force-disconnects another client. `target_launch` takes an executable `path`, an existing absolute `capture_directory`, and optional platform-specific raw `arguments`; each launch creates a fresh session subdirectory to avoid overwriting captures from earlier runs, and returns its `capture_template`. `target_trigger` takes nonzero `frames`; `target_queue` additionally takes the target `frame`; `target_cycle` changes the active window. These return `submitted` when the request is written to the helper, not when the target has captured a frame. Poll `target_events` to receive API registration, command-sent, capture, progress, child, window and disconnect notifications. A capture event's `path` is usable locally only when `local` is true. `target_close` disconnects without terminating the target; `quit` also closes the connection. Only one target session is held at a time. Remote target connections, remote capture copying, injection into an already-running process and remote workflows are not implemented.

For `target_launch`, optional `configuration` accepts `arguments`, an absolute existing `working_directory` (null uses the executable directory), `options`, and `environment`. For example:

```json
{"command":"target_launch","path":"C:/game/Player.exe","capture_directory":"C:/captures","configuration":{"arguments":"-screen-width 1280","working_directory":"C:/game","options":{"api_validation":true,"capture_callstacks":true},"environment":[{"name":"EXAMPLE_PATH","value":"C:/example","operation":"prepend","separator":"platform"}]}}
```

`capabilities.capture_options_default` exposes all 12 upstream options and defaults. `delay_for_debugger` is in seconds and `soft_memory_limit` in MB; the latter is an upstream soft target, not a hard limit. Each environment entry requires `name`, `value`, `operation` (`set`, `append`, `prepend`) and `separator` (`platform`, `semi_colon`, `colon`, `no_sep`). Entries are applied in order by RenderDoc. Unknown configuration fields are rejected. The older top-level `arguments` remains accepted when `configuration` is omitted. Environment values are sent to the RenderDocX helper through stdin and are not saved in RenderDocX settings or echoed in responses; the launched target necessarily receives them in its environment. On Windows, upstream's cross-bitness injection path may pass environment values through an additional helper's command line. Application arguments remain visible to the operating system as part of the target command line. The GUI also supports ordered environment modifications and masks the value input.

An unsuccessful `open` leaves the previous session intact. `pipeline` requires a selected event and returns RenderDoc's common topology enum, shader stages, output bindings, index and vertex buffers, vertex input metadata, and viewports. `resource_index` is the index in the upstream resource list for this session only, or null when the resource is absent from that list; it is not the texture index or a persistent identifier. Buffer `byte_offset` and `byte_size` are decimal strings preserving upstream 64-bit values, including sentinel sizes that do not represent the actual buffer length. Non-finite viewport coordinates are represented as null. `available: false` means RenderDoc did not provide a pipeline state. `vertex_inputs` is null for OpenGL because the current upstream common implementation can return uninitialized trailing entries; generic attribute values are not yet exposed. New pipeline fields are null when an older bridge does not provide them. This is a subset: API-specific pipeline details, source editing, shader debugging, texture raw pixels and remote capture are not yet exposed. `export_texture` requires a selected event and writes PNG using RenderDoc's default mapping; it never replaces an existing destination. `close` and `quit` wait for the replay helper to shut down, releasing its capture file before acknowledging. The server opens no network listener and performs no authentication; only processes with access to its stdin can send commands.

`shader` inspects the shader bound at the selected event. `stage` is one of the names in `capabilities.shader_stages`; `target_index` is optional and indexes the `targets` array returned for that stage and event. Omitting it or using null asks RenderDoc for its default. The result includes entry point, encoding, signature metadata, resource counts, supported targets and the disassembly text. RenderDoc may return an error message as the disassembly text; a successful request does not establish that disassembly or decompilation succeeded. Unbound stages and missing reflection return errors.

The capture snapshot lists `buffers` with session-scoped indices, upstream `resource_index`, `creation_flags`, and a decimal-string `length`. `read_buffer` requires a selected event, the buffer array `index`, an unsigned decimal-string `offset`, and an integer `length`. It returns `event_id`, `index`, `offset`, and lowercase hexadecimal bytes in `hex`. The range must lie within the buffer; length zero returns an empty result even though the upstream API uses zero to mean the rest of the buffer. Each request is limited to 65,536 bytes to bound IPC response size; page through larger buffers with successive offsets. A short or failed upstream read is an error. Generic texture raw pixels remain unavailable.

`export_texture` optionally accepts unsigned `mip` and `slice` indices (both default to 0), plus `sample` and `channel` (unsigned indices or null). Null `sample` requests RenderDoc's resolve; null `channel` requests all components. `slice` selects the array layer or, for a 3D texture, the depth slice at the chosen mip. Indices are validated against the capture metadata instead of silently clamped. The response echoes the selection. PNG conversion and channel extraction follow RenderDoc's format-dependent behavior, with its default black/white mapping. The 2D array and channel paths have real D3D11 coverage; multisample and 3D paths have not been validated against real captures. An older bridge can handle the default selection but reports an error for non-default selections.

The helper and its RenderDoc core must be built from matching source revisions. `select` returning success means the upstream replay call returned without a fatal error, not that the rendered output was visually validated.
