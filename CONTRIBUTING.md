# Contributing

Keep changes focused. Follow [AGENTS.md](AGENTS.md) and [SPEC.md](SPEC.md). Preserve explicit remote-access opt-ins, private socket permissions, browser Host/Origin checks and fail-closed sandboxing.

- `cmd/rexweb/`: CLI flags, listeners, HTTP security and service lifecycle.
- `internal/workspace/`: session layouts, PTYs, Linux namespaces and Rex protocol.
- `web/`: embedded Bun frontend, webterm source and Ghostty dependencies.
- `deploy/`: systemd examples.
- `tests/`: Linux integration and extracted-release verification.
- `scripts/`: release packaging.

Run `make install-dev`, then `make build` before Go tests: `web/embed.go` needs generated `web/dist`. Build order is explicit in the Makefile. Bun is only a build/test dependency. Use `make check`, `make race` and `make smoke`; integration tests require unprivileged Linux user namespaces. Never silently skip isolation tests on unsupported hosts. Set `REX_REFERENCE_BIN=/path/to/rex` to test against an independently installed official CLI.

`make release` cross-compiles static Linux ARM64/AMD64 binaries and tests the native extracted executable from an empty directory. Keep generated output and dependencies out of Git. Frontend modifications must retain the pinned security fixes, vendor provenance and licences in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

For browser regressions, install Chromium with `cd web && bunx playwright install chromium`, then run `make browser-smoke`. This checks the shared mobile keyboard with multiple panes. Set `BROWSER_CDP_URL` to use an existing development browser instead.
