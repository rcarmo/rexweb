# Changelog

## Unreleased

- Keep the workspace aligned with Safari visual-viewport keyboard/accessory panning and resizing without overriding pinch zoom.
- Keep the entire shared input caret box inside the visible area and update its anchor after viewport layout changes.

## 0.1.0-preview.1 — 2026-11-13

- Isolated authenticated upload browser tests with native HTTP-auth credentials, including WebSocket challenges.
- Added opt-in WebGPU composition with Canvas fallback and retained VT/selection on device loss; GPU display validation remains unavailable locally.
- Added opt-in row-local RTL/BiDi with logical copy, visual rectangles, and mapped cursor/IME, search, hyperlinks and mouse reporting.

- Added resolved Kitty image geometry/layers, owned content-generation caching and bounded animation lifecycle support.
- Added bounded server PNG decoding for authoritative image acknowledgements.

- Integrated core-anchored rectangular selection and physical-pixel font/grid metrics with fractional-DPR browser checks.

- Added bounded case-sensitive, whole-word and linear-time regex search with asynchronous results and correct wrapped/Unicode cell mapping.

- Added opt-in authenticated image/file uploads with confined atomic storage, limits, clipboard/drop and file-picker path paste.

- Added server-derived status/title/cwd badges, default-shell prompt marks and focused prompt navigation.
- Made server VT query replies authoritative to avoid duplicate browser replies.

- Added authoritative server-side Ghostty VT state, native Rex capture/pagination, and secure text/HTML/VT exports.

- Added focused-pane search, cursor-anchored shared IME/native Copy, and cooperative replay-safe output parsing.
- Updated the renderer to cached ABI-manifest-driven layouts and compatibility checks.

- Added synchronized output and canvas recovery; fixed application wheel routing, URL parentheses and event-listener disposal.

- Updated ghostty-web and its official Ghostty 1.4.1-dev core; rebuilt with Zig 0.17.0.

- One foreground service with embedded frontend, web/rex/both modes and a private local Rex socket.
- Built-in Linux namespace shell sandbox, explicit full-account host mode and fail-closed shell setup.
- Remote web password/TLS/Origin policy and explicit anonymous native networking.
- Graceful HTTP/WebSocket shutdown, systemd examples and static ARM64/AMD64 releases.
- Linux termios, Rex target resolution/block descriptions and official CLI interoperability tests.
- Linux CI, integration/security smoke and extracted-single-binary release verification.
- One shared mobile keyboard with focused-pane paste/IME handling and bounded Unicode input frames.
- Normalised CRLF in native Rex paste events.
- Removed detached gateway autostart; service restarts now end sessions.


## 0.1.0-preview.1

Initial preview:

- Go session daemon with sandboxed PTYs, private workspaces and explicit shell environments.
- Automatic daemon startup, a default shell, and persistent sessions across gateway restarts.
- Bun frontend using webterm and Ghostty, with sessions, tabs, nested splits and live resizing.
- Rex-inspired controls, a session switcher, a command palette and system light/dark appearance.
- Byte-offset reconnects and a bounded PTY replay buffer.
- An optional loopback endpoint implementing the tested revision-12 Rex client subset.
- MIT project licence, retained third-party notices, macOS CI, and Apple Silicon and Intel release archives.

The preview requires macOS `sandbox-exec`. It retains sessions in memory and supports one live viewer per pane. See [README.md](README.md) for native-client limitations.
