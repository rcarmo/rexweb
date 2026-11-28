# Contributing

Keep changes focused and follow [AGENTS.md](AGENTS.md) and [SPEC.md](SPEC.md). Preserve explicit remote-access opt-ins, private sockets, browser Host and Origin checks, and fail-closed sandboxing.

| Path | Contents |
| --- | --- |
| `Cargo.toml`, `Cargo.lock`, `src/` | Rust service, CLI, listeners, Linux sandbox and PTYs, Rex protocol, VT, layouts and uploads |
| `assets/` | Server VT callback trampoline and native blank terminal snapshot |
| `web/` | Bun frontend, webterm sources and Ghostty renderer; `web/dist/` is generated and embedded |
| `tests/` | Rust regression fixtures and Linux, browser and extracted-release tests |
| `deploy/`, `scripts/` | Systemd examples and build and release scripts |

Install the tools in [README.md](README.md) and run `make build`. It builds the frontend before embedding it in the static `bin/rexweb`. Run `make install-dev` if you only want to install frontend dependencies. To cross-compile, install the matching musl target and run `make build ARCH=arm64` or `make build ARCH=amd64`. Either replaces `bin/rexweb`; rebuild for the host before running local smoke tests.

`make check` runs Rust formatting, Clippy and tests, plus frontend checks. `make test` runs the Rust and Bun suites. The Rust suite replays 225 recorded synthetic capture cases against production code; it needs no Go compiler or migration bridge. Oak Oil is required for Rust builds. The build and check paths share the ignored `target/` directory, disable incremental compilation and default to a 512 MiB, seven-day Oak Oil cache under the user cache directory. Avoid additional Cargo profiles, target directories and worktrees for routine builds.

Install OpenSSL for smoke tests. Run `make smoke` on Linux with unprivileged user namespaces enabled. Set `REX_REFERENCE_BIN=/path/to/rex` to include tests against a separately installed official Rex CLI. Do not use host mode as a substitute for sandbox testing. Set `REXWEB_BIN=/absolute/path/to/rexweb` to choose the binary under test. It defaults to `bin/rexweb`. See [RELEASING.md](RELEASING.md) for extracted-package verification.

Install Playwright browsers and their system libraries before running browser tests:

```sh
(cd web && bunx playwright install --with-deps chromium firefox webkit)
make browser-smoke prefix-browser input-browser viewport-browser cross-browser
```

The Chromium suite covers shared mobile input, caret and native Copy, focused-pane search, replay, paste and IME, graphics, Canvas recovery, application mouse input, links and authenticated uploads. Run `make upload-browser` to exercise uploads alone. Set `BROWSER_CDP_URL` to connect the main Chromium suite to an existing development browser. Desktop WebKit does not substitute for physical iPad Safari.

Keep generated output and dependencies out of Git. Frontend or dependency updates must retain the pins, security fixes and licence notices listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
