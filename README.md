# rexweb

A single-binary Linux terminal service with an embedded browser client and a Rex-compatible native endpoint. Web, remote Rex and local Rex clients operate on the same sessions, tabs and tiled panes. The service runs as one unprivileged account, in the foreground, under systemd or directly from a terminal.

Based on [rcarmo/rexweb](https://github.com/rcarmo/rexweb), with its webterm components and pinned Ghostty WASM renderer. Independent of Superlogical Rex. No proprietary Rex executable is bundled.

## Build and run

Build dependencies: Go 1.24 or later, Bun, Git and make. Runtime: Linux, `/bin/sh` (or your chosen shell), system tools and unprivileged user namespaces. No Bun, Docker, external sandbox program, frontend directory or Go installation is needed on the target machine.

```sh
make install-dev
make build
bin/rexweb
```

Open <http://127.0.0.1:8080>. The binary works from any working directory. Browser disconnects leave shells running; reconnects replay up to 512 KiB per pane. Stopping or restarting the service ends shells and discards the in-memory layout and output. Workspace files remain on disk. Set `-workspace` to reuse a private directory outside the service account's home; the default is a fresh temporary directory whose path is logged at startup.

Desktop keyboard input and composition use the terminal's real textarea, with
its caret anchored to the focused pane. Same-origin browser tabs and iframes
hand off a pane's single viewer connection. A paused view offers **Resume
terminal in this tab**; this preserves the server session and never replays
keyboard input. Coordination is limited to the same browser profile and origin;
the server's native single-viewer guard still applies to other clients.

## Search and input

Use **Ctrl+F / ⌘F** or the magnifying-glass button to search the focused pane.
Enter moves to an older match; Shift+Enter moves to a newer match. The arrow
buttons do the same, and Escape closes search. Switching panes changes the
search target. Search covers the active screen and retained history loaded in
that renderer, not every session or output already discarded by the server.
Literal matching uses ASCII case folding. The search bar also offers case-sensitive,
whole-word and regex modes. Advanced searches use a linear-time RE2JS matcher,
not JavaScript backtracking regex; unsupported syntax is shown as an error.
Snapshots are built cooperatively, with limits of 500,000 cells/1 MiB of text
and 10,000 matches. Partial counts are marked with '+'. Queries are limited
to 1024 characters.

One shared mobile keyboard/input follows the active pane's cursor, including
horizontal movement, scrolling and resizing. The workspace follows Safari's
visual viewport when its keyboard or input accessory pans/resizes the view; the
whole native caret box stays within the visible area. Pinch zoom is not treated
as a keyboard resize. Native Copy uses the current terminal selection. Text paste retains sanitisation and bracketed-paste handling.
Large output is parsed cooperatively so the page can remain interactive;
reconnect offsets and input readiness wait for parser consumption.

Selections are anchored in the client VT core and copied through its formatter.
Alt-drag selects a rectangle; Shift/Alt selection gestures override application
mouse reporting. Cell pitch and baseline resolve to physical pixels, including
fractional DPR, and owning-window resize/DPR changes trigger a full repaint.

Kitty graphics use core-resolved viewport rectangles, layers, source crops and
content generations. The client keeps bounded owned pixel snapshots and schedules
animation through core deadlines, with reset/suspend/DEC-hold cleanup. The server
VT accepts images with a 16 MiB storage budget and a bounded Go PNG decoder
(8 MiB encoded input, up to 4096 per dimension/4 million pixels), so it can supply
correct protocol acknowledgements while remaining file/network independent.

## Optional rendering modes

The command palette offers **Try WebGPU compositor (reload)** and
**Enable RTL/BiDi (reload)**. The equivalent URL options are
`?renderer=webgpu` and `?bidi=1`; combine them with `&`. Reloading reconnects
to the same server session and does not restart its shells. Both options are off
by default and can be disabled through the palette.

WebGPU requires a supported secure browser context (HTTPS or localhost). It uses
Canvas glyph rasterization with GPU composition/presentation, not GPU-side text
layout. Missing adapters, initialization errors, device loss and oversized GPU
textures fall back to Canvas without discarding VT state or selection. Additional
GPU texture storage is capped at 64 MiB per pane. GPU display behaviour has not
been validated in this development environment.

RTL/BiDi reorders cells within each row, keeping an LTR paragraph base and logical
server/core storage. Cursor/IME anchors, links, search highlighting, selection
and application mouse reports use the same coordinate map. Linear Copy preserves
logical text; rectangular Copy takes a visual box and emits its cells in logical
order per row. The mode uses Unicode 13 BiDi tables, does not add Arabic
joining/shaping or cross-row paragraph layout, and leaves rows containing Kitty
Unicode image placeholders in logical order. Leave it off for applications that
perform their own BiDi layout.

## Images and files

Enable uploads explicitly with a web password:

```sh
rexweb -mode both -uploads -password-file /private/config/web-password
```

Use **Upload files…** in the command palette, paste a clipboard image, or drop
files onto a pane. Each file is stored under a generated name inside the service
workspace. The quoted path is inserted into the initiating pane without Enter;
commands are never run automatically. A file is limited to 16 MiB, a paste/drop
to eight files, and the server to four simultaneous uploads. HTTP uploads require
both authentication and a matching Origin, including on loopback. Interrupted
uploads are removed and completed files are committed without overwriting an
existing file. The selected workspace directory is pinned by a directory handle,
so replacement symlinks cannot redirect writes. Uploaded files persist until you
remove them from the workspace. Uploads have no automatic retry.

## Program status and prompts

Panes show reported working, blocked, done or error states, optional progress,
and terminal titles/cwd. These come from OSC 7501/OSC 0/2/7 reports; the service
does not guess arbitrary applications' activity. Native Rex `program_status`,
`title` and `pwd` read the same server-side state.

The command palette offers **Previous prompt** and **Next prompt**. Supported
default sh/bash/zsh shells emit OSC 133 prompt boundaries. Startup files can
replace PS1; other shells need their own integration. Navigation follows marks
retained in the focused renderer and sends no keyboard input to the shell.

## Capture and export

The service keeps parsed VT state for every pane, even without a browser attached.
Use the command palette to export the focused terminal as text, HTML or VT.
The official Rex CLI can capture the same authoritative screen and history:

```sh
rex -S /run/user/1000/rexweb/session.sock --autostart=false capture -s work --trim
```

The VT engine shares a compilation cache but isolates each terminal's memory,
with a 64 MiB per-module limit, 16 MiB/2000-line scrollback limits and bounded
execution calls. Exports are limited to 4 MiB. Native Rex exports paginate large
content with `max_bytes` and `blob_read`; retained export blobs are bounded and
expire after five minutes. HTML exports are offline documents with unsafe links
removed and scripts blocked. Capture describes the active screen; it does not
keep shell processes alive across a service restart. The service owns terminal query replies; the embedded browser suppresses local
parser replies to prevent duplicate input.

## Choose listeners

```sh
# Browser plus private local Rex socket (default).
rexweb -mode web

# Native Rex TCP plus private local Rex socket; no browser listener.
rexweb -mode rex -rex-listen 127.0.0.1:7297

# Browser and native Rex TCP, sharing sessions.
rexweb -mode both -listen 127.0.0.1:8080 -rex-listen 127.0.0.1:7297
```

The local socket defaults to `$XDG_RUNTIME_DIR/rexweb/session.sock`, falling back to the user's cache directory when XDG_RUNTIME_DIR is unset. Its directory is private and its mode is 0600. A lifetime lock prevents two services from replacing the same socket. Use `-session-socket /absolute/private/path/session.sock` to choose another instance. The socket speaks the native Rex protocol, not the browser REST API.

With the official Rex CLI installed separately:

```sh
rex -S /run/user/1000/rexweb/session.sock --autostart=false ls
rex -S /run/user/1000/rexweb/session.sock --autostart=false new work
rex -S /run/user/1000/rexweb/session.sock --autostart=false split -s work
rex -S /run/user/1000/rexweb/session.sock --autostart=false send -s work 'ls'
rex -S /run/user/1000/rexweb/session.sock --autostart=false send-key -s work enter
```

In Rex.app, add `http://127.0.0.1:7297` as a remote host when native TCP is enabled. The adapter supports Rex protocol revision 12: session management, tabs, tiled splits, terminal input and output. Newer clients must support revision 12.

One live viewer per pane is supported. Close a browser pane/tab viewer before attaching that pane from Rex, or use another pane. Native resume, floating panes, Lua, command-layout options, the full automation/discovery API and multi-user login are unsupported. Unsupported methods fail explicitly. See [SPEC.md](SPEC.md).

## Remote access

Prefer SSH forwarding. It adds authentication and encryption without exposing a shell service to the network:

```sh
# On the server, run -mode both. On your computer:
ssh -N user@server -L 8080:127.0.0.1:8080 -L 7297:127.0.0.1:7297
```

Use <http://127.0.0.1:8080> in the browser or `http://127.0.0.1:7297` in Rex. Alternatively forward the private Unix socket for a native client. Anyone on the server who can reach a loopback TCP listener has access unless web password authentication is configured. Local socket permissions restrict access to its owner.

For direct remote web access, use a private password file and TLS:

```sh
umask 077
openssl rand -base64 32 > /private/config/web-password
rexweb -mode web -listen 0.0.0.0:8443 \
  -public-url https://terminal.example:8443 \
  -password-file /private/config/web-password \
  -tls-cert /private/config/fullchain.pem -tls-key /private/config/key.pem
```

Log in as `rexweb` with the password in the file. Use a valid certificate for the external hostname. Host and Origin checks use the exact `-public-url`. Certificate/password files are read at startup; restart after rotating them. `-allow-insecure-http` permits a remote HTTP listener only when explicitly selected; use it behind a trusted TLS-terminating proxy that preserves Host, Origin and Authorization and proxies WebSocket upgrades. Forwarded identity headers are never trusted.

Native Rex has no password authentication. Non-loopback native TCP requires `-allow-anonymous`, which grants every reachable client full service access. Bind it only on a trusted VPN with access controls; prefer SSH. `-tls-cert` and `-tls-key` enable TLS on both TCP listeners when supplied, but TLS alone does not authenticate native clients. Never expose anonymous native Rex to the public internet.

## Shell permissions

The default `-isolation sandbox` restricts shells to the workspace and read-only system tools, with no network access. It requires unprivileged Linux user namespaces and refuses to start if isolation is unavailable. Some distributions, including recent Ubuntu, require a local policy allowing these namespaces.

For terminals that need the server account's normal files, installed tools and network access, select this explicitly:

```sh
rexweb -mode both -isolation host -workspace /srv/private/rexweb-work
```

**Host mode grants full service-account permissions to every connected client and its programs.** Both modes use an explicit environment and workspace-local `.home` and `.tmp` directories. They do not inherit service secrets or your normal shell startup files. Root execution is refused. Sandbox panes share workspace files and are not mutually untrusted tenants. See [SECURITY.md](SECURITY.md) for the trust boundary and limitations.

## systemd

Examples are in [deploy/](deploy/). For a dedicated system account:

```sh
sudo useradd --system --home-dir /var/lib/rexweb --shell /usr/sbin/nologin rexweb
sudo install -m 0755 bin/rexweb /usr/local/bin/rexweb
sudo install -m 0644 deploy/rexweb.service /etc/systemd/system/rexweb.service
sudo systemctl daemon-reload
sudo systemctl enable --now rexweb
journalctl -u rexweb -f
```

The unit creates private directories `/run/rexweb`, `/var/lib/rexweb` (the account home) and `/var/lib/rexweb-work` (the separate workspace). Do not start a system service as root. To access its private Unix socket, run the client as the service account (for example through `sudo -u rexweb`).

Use `systemctl edit rexweb` to replace ExecStart with your listener/isolation options. Stopping the unit terminates the whole control group. A user-unit example is included for a personal server; install the binary at `~/.local/bin/rexweb` and the unit as `~/.config/systemd/user/rexweb.service`, then `systemctl --user enable --now rexweb`. Its default workspace is the private `/var/tmp/rexweb-UID-work` directory outside your home. Enable lingering with `loginctl enable-linger` if it must remain running while logged out.

## Verify and package

```sh
make check                  # TypeScript, Go vet, unit tests
make race                   # Go race detector
make browser-smoke          # Chromium workspace/input/replay tests
make input-browser          # Native desktop IME and tab/iframe viewer handoff
make cross-browser          # Firefox/WebKit renderer and BiDi checks (install browsers first)
make smoke                  # Linux isolation, PTYs, reconnects, native protocol, auth and modes
REX_REFERENCE_BIN=/path/to/rex make smoke  # Also test the official CLI
make release                # Static Linux ARM64/AMD64 archives, SHA256SUMS, extracted-native smoke
```

`-version` prints build information; `-help` lists all flags. `-web-dir` optionally overrides embedded assets for frontend development. Releases contain the binary, systemd examples, documentation and third-party notices. See [RELEASING.md](RELEASING.md) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). Project code uses the [MIT licence](LICENSE).
