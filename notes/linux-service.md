# Linux service work item

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
- Unit, race, Linux integration and extracted-native-package tests pass.

## Reference

Upstream clone: 5342396. Official Linux CLI reference: version 0.1.0-preview.16, commit aa2db1b21261b479a550559fb70e2c3a66c07ba2. ARM64 archive SHA-256 f079c5bcc69a2960b400b9c3e80f341b2e3951c205cc9282869cfab0be9ff5eb. Its `/api/help` and `/llms.txt` were used to check target resolution and block descriptions. The reference executable is not redistributed.

## Risks

Namespaces depend on kernel and distribution policy; no independent sandbox audit. The Rex adapter remains a revision-12 subset. GUI app testing and cross-architecture runtime testing must be distinguished from CLI/wire tests and cross-compilation. Service restart intentionally ends in-memory sessions. Host-mode daemonised descendants require systemd control-group termination.
