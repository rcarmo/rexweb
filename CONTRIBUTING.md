# Contributing

Keep changes focused on the issue being addressed. Follow [AGENTS.md](AGENTS.md) and the behaviour described in [SPEC.md](SPEC.md).

The backend uses Go. The frontend uses Bun and the existing webterm terminal component. Preserve the sandbox, loopback restrictions, private daemon socket and browser origin checks. Use disposable workspaces outside the account home for integration tests.

The repository layout is:

- `cmd/rexweb/`: executable entry point and daemon startup.
- `internal/workspace/`: session, sandbox, terminal and protocol code, with Go tests alongside it.
- `web/`: frontend source, assets, Bun dependencies, lockfile, TypeScript configuration and unit tests.
- `tests/`: integration and release smoke tests.
- `scripts/`: release packaging.
- `docs/`: documentation images.
- `licenses/`: retained third-party notices.

Install dependencies with `make install-dev`. Before submitting a change, run `make check` and `make build`. Run `make race` for concurrent Go changes and `make smoke` for session, sandbox, streaming or protocol changes. Include the checks you ran and any untested behaviour in the pull request.

`make build` writes the executable to `bin/rexweb` and the browser bundle to `web/dist/`. Use `make build-go` or `make build-web` to build either part separately, and `make test` for Go and frontend unit tests. Run Bun commands from `web/`, including `bun install --frozen-lockfile`, `bun run typecheck` and `bun run build`. `make clean` removes `bin/`, `dist/` and `web/dist/` while retaining installed dependencies.

Retain upstream notices when updating vendored code, dependencies or fonts, and update [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). Contributions to project code are provided under the repository's MIT licence.
