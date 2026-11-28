# Linux service scope and verification

## Scope

One Linux executable owns PTYs, session layouts and web/native listeners. Local Rex clients use a private Unix socket; network clients share the same backend. The browser renderer and layout controls retain the upstream frontend.

The service uses a single-account trust model. Multi-user login, full Rex API parity and proprietary runtime bundling are out of scope. Client disconnects preserve sessions and byte-offset replay; service restarts end in-memory sessions.

Native network access requires explicit anonymous opt-in. Remote web access requires a private password file and TLS, or explicit HTTP opt-in behind a TLS proxy. Sandbox setup fails closed; host account access is explicit.

## Acceptance criteria

- One static executable serves embedded frontend from an arbitrary working directory.
- Mode web/rex/both selects TCP endpoints; the private local Rex socket remains available.
- Local/remote native and browser clients share layouts and PTYs.
- Linux sandbox denies outside host files, sockets and network; host mode is explicit.
- Shutdown closes WebSockets and shells; systemd controls the whole process group.
- Rust tests, recorded capture regressions, frontend tests, Linux integration and extracted-native-package tests pass.

## Rust migration record

The Rust candidate replaces the service and build tooling on this branch only. The browser keeps the Bun/TypeScript frontend and pinned Ghostty WASM. The service runs that WASM in Wasmi and embeds the browser bundle in the static musl executable. The published Go implementation remains at [v0.1.1](https://github.com/rcarmo/rexweb/tree/v0.1.1).

The [225 synthetic capture cases](../tests/fixtures/capture-chunks.jsonl) were recorded against the Go `internal/capturechunk` unit in [9932286](https://github.com/rcarmo/rexweb/commit/9932286f9f51488de76aad790c9147f6c20d6305). The Rust test runs those fixed inputs and expected results through the production pagination logic. The [recording metadata](../tests/fixtures/capture-chunks-origin.json) preserves the original fingerprint and checksum. No Go code or runtime bridge participates in the check.

The root-layout candidate `0.1.1-rust.1` passed native ARM64 extracted-package, official Rex CLI, Linux sandbox and Chromium workspace, upload, prefix and input checks. Firefox and desktop WebKit passed renderer and BiDi checks, with Canvas fallback. Both musl binaries passed static ELF checks. AMD64 ran version, embedded web and host-mode startup under QEMU; full native AMD64 sandbox testing remains outstanding. Retest each candidate using [RELEASING.md](../RELEASING.md).

The native API implements a revision-12 subset. Process metadata is limited, bootstrap does not return data tickets, and block resolution does not honour window scope. Desktop WebKit and simulated mobile viewport tests do not validate physical iPad Safari; visible WebGPU presentation remains unverified.

## Reference

Upstream clone: 5342396. Official Linux CLI reference: version 0.1.0-preview.16, commit aa2db1b21261b479a550559fb70e2c3a66c07ba2. ARM64 archive SHA-256 f079c5bcc69a2960b400b9c3e80f341b2e3951c205cc9282869cfab0be9ff5eb. Its `/api/help` and `/llms.txt` were used to check target resolution and block descriptions. The reference executable is not redistributed.

## Risks

Namespaces depend on kernel and distribution policy; no independent sandbox audit. The Rex adapter remains a revision-12 subset. GUI app testing and cross-architecture runtime testing must be distinguished from CLI/wire tests and cross-compilation. Service restart intentionally ends in-memory sessions. Host-mode daemonised descendants require systemd control-group termination.
