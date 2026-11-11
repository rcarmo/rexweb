# Repository instructions

Do not, _ever_ edit unrelated pieces of code when performing a change.

- Follow [SPEC.md](SPEC.md) for scope and supported behaviour. The Go session daemon owns processes and layout. Keep browser state limited to presentation, selection and the last consumed output offset.
- Use Bun and the existing webterm TypeScript components for all frontend work. Reuse its Ghostty renderer, terminal widget, fonts, styles and input handling. Use `bun install --frozen-lockfile`, `bun run typecheck` and `bun run build`. Keep the Bun lockfile and document the source commit and local adaptations for vendored components.
- Use Go for the gateway, session daemon and WebSockets. Follow the standard library's HTTP and context conventions, format changes with `gofmt`, bound reads and writes, and clean up client connections and goroutines on disconnect. Keep the gateway free of PTY allocation and Docker daemon access.
- Launch each shell with the required OS sandbox, a workspace outside the account home and an explicit environment. Fail closed if isolation cannot be applied.
- Require a private Unix socket for the daemon connection. Preserve origin and Host checks, keep browser operations allowlisted, and keep terminal data out of logs. Changes to disconnect or retry logic must preserve session lifetime and prevent duplicate terminal input or mutations.
- Keep native Rex access opt-in and loopback-only. Check native wire changes against the bundled reference server's versioned `/api/help` and `/llms.txt`. Use disposable workspaces outside the account home for protocol tests.
- Run `make check` before handoff and `make race` for concurrent Go changes. Build the browser bundle with `make build` when changing frontend code. Run `make smoke` when changing the workspace API or terminal protocol.
- Webterm source is pinned to [fe3cc03](https://github.com/rcarmo/webterm/tree/fe3cc0340b5f4e86ec73ae5dcf311d78930134ec). The terminal component under `web/src/webterm` retains the MIT notice. Its local changes cover focus and connection lifecycle for multiple Rex panes. Keep font licences with their assets.
