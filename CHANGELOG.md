# Changelog

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
