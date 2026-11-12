# Linux service contract

## Process and persistence

One foreground process owns the session Store, PTYs, private Unix listener and selected TCP listeners. `-mode web` enables browser TCP, `-mode rex` enables native TCP, and `-mode both` enables both. The private local Rex socket is always present. Default addresses are loopback. All selected listeners must bind successfully; a startup error exits nonzero and releases already-opened listeners. A socket lifetime lock prevents concurrent service instances on the same socket. Regular files are never replaced as stale sockets.

SIGINT/SIGTERM closes HTTP servers, hijacked WebSockets and panes. Shells and layouts survive client disconnects. They do not survive service termination. Workspace files persist, and `-workspace` allows a stable location across service restarts. A fresh service starts one default shell.

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
- `target.resolve` for sessions, windows and blocks, including exact IDs, labels, window positions, suffixes and ambiguity reporting.
- `session.describe_block`, `session.list_blocks` with terminal metadata for CLI input commands.

Terminal methods: write, resize, size, input_mode (Linux TCGETS), title (empty), pwd (initial workspace), process (initial shell metadata), program_status (empty), set_theme (no-op). Paste, focus events and basic key encoding are supported. These metadata methods do not inspect arbitrary foreground processes or reconstruct a terminal screen.

Creation accepts one initial shell tab; custom command layouts and initial input are rejected. Floating panes, Lua, target completion, full discovery/help, capture, downloads/uploads, native stream resume and multi-user login are outside this subset. Unknown methods fail explicitly. Revision 13 is not advertised.

## Security and verification

Default shell isolation uses Linux namespaces and fails closed. Host isolation is a deliberate full-account option. Remote web requires password authentication and a fixed public origin. Remote native TCP requires anonymous-access opt-in. See [SECURITY.md](SECURITY.md) for precise boundaries.

Run `make check`, `make race`, `make smoke` and `make release`. Smoke tests cover execution from an empty directory, embedded assets/WASM, sandbox file/network/socket denial, capability drop, PTY resizing, reconnect offsets, nested splits, native wire streams, shared layouts, mode selection, web authentication and shutdown. Set `REX_REFERENCE_BIN` for optional official CLI tests over both native transports.

## Browser search and shared input

The focused-pane search uses the core's literal search over the active screen and loaded history, with ASCII case folding, a 1024-character query limit, result counts and a selected-match highlight. It owns a separate decoration layer and frees native handles when panes reset or close. Switching panes clears the former target's highlight. Search never supplies terminal input or searches other sessions.

Mobile input remains a single shared textarea/keybar. Its anchor follows the focused pane's cursor and stays within the visible viewport. Native clipboard staging is user-initiated, uses the current selection, and clears staged text after Copy or subsequent input. No background clipboard reads are performed.
