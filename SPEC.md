# Linux service contract

## Process and persistence

One foreground Rust process owns session state, PTYs, the private Unix listener and selected TCP listeners. `-mode web` enables browser TCP, `-mode rex` enables native TCP, and `-mode both` enables both. The private local Rex socket is always present. Each pane owns a separate Ghostty VT instance; the service shares the compiled Wasmi modules. Default addresses are loopback. All selected listeners must bind successfully; a startup error exits nonzero and releases already-opened listeners. A socket lifetime lock prevents concurrent service instances on the same socket. Regular files are never replaced as stale sockets.

SIGINT/SIGTERM stops the listeners and closes WebSocket connections and panes. Shells and layouts survive client disconnects. They do not survive service termination. Workspace files persist, and `-workspace` allows a stable location across service restarts. A fresh service starts one default shell.

## Sessions and streams

The Store owns session labels, ordered window tabs, nested binary horizontal/vertical splits, ratios and focus. Browser state handles presentation, selection and the last consumed stream byte offset. Each pane has a PTY, one live viewer and 512 KiB of retained output. Panes that exit retain output until closed.

Browser attachment sends `stream` metadata, retained binary bytes, `ready`, and live bytes. A valid retained offset resumes without duplication; offsets outside retention reset the renderer. Slow viewers detach when the bounded output queue fills. Browser input is suppressed while replaying and discarded on disconnect. Output parsing is ordered and time-budgeted; consumed offsets advance only in parser completion callbacks. Input readiness and replacement transports wait behind parser barriers, so a disconnect during queued replay cannot claim bytes not yet consumed. Parser failure resets the renderer and resumes from a fresh retained replay. The bundled frontend retains the upstream webterm lifecycle and Ghostty renderer.

## Native Rex

Both the Unix socket and native TCP use `rex.control.v1` at `/api/control`, with API revision 12, and ticketed `rex.data.terminal.v1` at `/api/data`. New streams receive a blank Ghostty snapshot followed by retained bytes and live output. Tickets are single-use, expire after 60 seconds and require a live originating control connection. Native data-stream resume is unsupported; reopen with a fresh ticket.

Supported control methods:

- `server.version`, `server.status`, `server.whoami`, `hosts.list` (empty), `client.keymap`, `client.set_label` (no-op), `block_creator.list`.
- `session.list`, `session.attach`, `session.detach`, `session.attach_all`, `session.bootstrap`, `session.view`, `session.create`, `session.destroy`, `session.set_label`.
- `session.new_window`, `session.close_window`, `session.set_window_label`, `session.focus_window`, `session.focus_block`, `session.new_split`, `session.resize_pane`, `block.close`.
- `session.own_block`, `session.disown_block`, `data.connect`, `client.key`, `block.event`, `block.method` and creator-qualified terminal methods.
- `target.resolve` for sessions, windows and blocks, including exact IDs, labels, window positions, suffixes and ambiguity reporting. Block lookup supports session scope but does not honour window scope.
- `session.describe_block`, `session.list_blocks` with terminal metadata for CLI input commands.

`session.bootstrap` can attach sessions but does not issue data tickets. Use `data.connect` before opening a data stream.

Terminal methods: `write`, `resize`, `size`, `input_mode` (Linux `TCGETS`), `title`, `pwd`, `process`, `program_status`, `set_theme` (no-op), `format`, and `blob_read`. Title, working directory and program status reflect VT reports. Process metadata describes the initial shell. The service does not inspect the live foreground process or its ancestry. Paste, focus events and basic key encoding are supported.

Creation accepts one initial shell tab; custom command layouts and initial input are rejected. Floating panes, Lua, target completion, full discovery/help, downloads and native file-transfer automation, native stream resume and multi-user login are outside this subset. Unknown methods fail explicitly. Revision 13 is not advertised.

## Security and verification

Default shell isolation uses Linux namespaces and fails closed. Host isolation is a deliberate full-account option. Remote web requires password authentication and a fixed public origin. Remote native TCP requires anonymous-access opt-in. See [SECURITY.md](SECURITY.md) for precise boundaries.

Run `make check`, `make smoke` and `make release` on the target Linux architecture. Smoke tests cover embedded assets/WASM, sandbox file/network/socket denial, capability drop, PTY resizing, reconnect offsets, nested splits, native wire streams, shared layouts, mode selection, web authentication and shutdown. Release verification runs the extracted executable from an empty directory. Set `REX_REFERENCE_BIN` to also test the official CLI over both native transports.

## Browser search and shared input

The focused-pane search uses the core's literal search over the active screen and loaded history, with ASCII case folding, a 1024-character query limit, result counts and a selected-match highlight. It owns a separate decoration layer and frees native handles when panes reset or close. Switching panes clears the former target's highlight. Search never supplies terminal input or searches other sessions. Optional case-sensitive, whole-word and regex modes use cooperatively built text/cell snapshots and RE2JS linear-time matching, with generation cancellation and explicit pending/error/truncated result fields. Limits are 500,000 cells/1 MiB text and 10,000 matches. Historical soft wraps and complete combining graphemes map back to terminal cells; unsupported syntax is rejected.

Mobile input remains a single shared textarea/keybar. Its anchor follows the focused pane's cursor and stays within the visible viewport. Native clipboard staging is user-initiated, uses the current selection, and clears staged text after Copy or subsequent input. No background clipboard reads are performed.

## Authoritative capture

The server runs the browser bundle's embedded Ghostty WASM in Wasmi, without an external runtime. Each pane has an independent instance with a 64 MiB memory cap and a bounded instruction budget per call; output is fed before publication. Native `format`/`blob_read` and the browser capture endpoint read this state without requiring a viewer. Browser command-palette exports support text, HTML and VT. Captures cover the active screen and its retained history (2000 lines/16 MiB, page-granular pruning), with a 4 MiB export cap. Native chunks obey encoded JSON limits; at most 4 MiB of temporary blobs is retained per pane. HTML export is sanitised and supplied with an offline CSP.

The server is authoritative for terminal query replies. The embedded browser sets emitTerminalResponses=false so replay and live parser activity cannot generate duplicate replies. Capture does not imply service-restart session persistence or a stable native snapshot wire format.

## Program and prompt effects

The server records bounded OSC 7501 statuses and reported title/pwd using Ghostty callbacks. Session views carry a `terminal_metadata` extension for browser badges. Native `program_status`, `title` and `pwd` read the same state. Records and prompt events are cleared on VT reset. Shell exit is reflected in browser state.

Default supported shells receive OSC 133 PS1 hooks. Browser Previous/Next prompt navigation uses the client's core marks, temporarily reading core scroll positions and restoring the live viewport before updating external scroll state. No shell input is emitted.

## Web image/file uploads

`-uploads` plus password authentication enables multipart POST uploads with a matching Origin. The browser sends one file per request; the server buffers the first multipart field and limits it to 16 MiB. The initiating session and pane must exist. The generated workspace name, detected MIME, size and absolute path are returned as JSON. The server uses a pinned directory descriptor, creates an exclusive no-follow temporary file and publishes it through a no-replace hard link. Browser clipboard, drop and file-picker integrations upload at most eight files sequentially and paste quoted paths without Enter into the initiating live pane. There is no automatic retry or uploaded-file serving. See [SECURITY.md](SECURITY.md) for request limits.

## Resolved image frames

Client image snapshots use the shared render state's resolved rectangles/layers/crops and native content generations. Owned pixel caching is bounded to 64 MiB; borrowed WASM views are not retained. Terminal-owned animation scheduling cancels on reset/disposal/suspension, respects DEC holds and uses bounded deadlines. Canvas clips image layers to repainted rows to avoid stale retransmission and alpha accumulation. Server VT image storage is bounded to 16 MiB with a host PNG decoder limited to 8 MiB input/4096 dimensions/4 million pixels, and remains the source of protocol acknowledgements.
