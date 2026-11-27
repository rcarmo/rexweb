# Rust service migration

The `rust` branch uses [Rust Anything](https://github.com/erkamyaman/rust-anything) 5.0.1 to port backend units against the Go implementation. The browser remains TypeScript with embedded Ghostty WASM. The Rust branch now contains a complete standalone service candidate; `main`, the default build and the published Go releases remain unchanged.

| Service area | Candidate and parity coverage |
| --- | --- |
| Native capture pagination | [Rust port and 225 recorded Go cases](capturechunk/README.md); parity retained as part of the migration gate |
| Native paste encoding, split geometry and capture pagination | Rust with 27 service/VT/security/layout tests and 225 recorded Go parity cases |
| Ghostty VT, image decoding and safe HTML export | Rust service implementation, with real-WASM and sanitisation coverage |
| Workspace state, sessions, Rex protocol and uploads | Rust implementation with workspace, native-protocol and upload security coverage |
| PTYs and Linux sandbox | Rust implementation with Linux namespace and process-lifecycle coverage |
| Web/native listeners, authentication and lifecycle | Rust service implementation with embedded `web/dist`; covered by the service and browser parity gates |

These statements describe source and test coverage for the Rust candidate, not a switch in the default product build.

## Verified candidate

- `make rust-check`: Rust Anything compares 225 recorded Go capture cases; 27 Rust unit tests and Clippy pass under Oak Oil. Warm capture and backend plans compile zero units.
- Static `aarch64-unknown-linux-musl` and `x86_64-unknown-linux-musl` release executables are built with Oak Oil and contain no ELF interpreter or dynamic dependencies.
- Extracted static ARM64 package: official Rex CLI, Linux sandbox/security, PTY reconnect/resize, nested layout, auth, TLS, Origin and failed-shell smoke pass.
- Static ARM64 candidate: the complete Chromium workspace/browser suite passes, including viewport, IME, native Copy, replay, search, images, BiDi, uploads and shared input. Prefix and iframe/tab handoff suites pass separately.
- Static AMD64 package is checked as a static ELF and executes under QEMU, including version, embedded web and host-mode session startup. Full AMD64 sandbox/native verification still requires native AMD64 hardware.
- The local static archives include 187 locked Rust dependency entries and retained licence/notice files. A package is eligible for publication only when the final static executable has passed the same release gates used for the Go service. In particular, report each architecture separately: passing on native ARM64 does not establish AMD64, and passing desktop WebKit does not establish physical iPad Safari.

Rust Anything's package scan (`rust-anything analyze .`) flagged cross-package interfaces, pointers and concurrent state in the service. Its Go bridge cannot directly port `internal/vt`'s exported constructor, which takes `context.Context`. The migration therefore keeps small data-only parity boundaries for Rust Anything and implements stateful service modules behind the existing browser, native Rex and Linux integration contracts. Do not ship the generated CGO bridge; the candidate is one self-contained Linux service executable with the web bundle embedded.

## Build and parity verification

Install [Oak Oil 0.3.2](https://github.com/oakoliver/oak-oil/releases/tag/v0.3.2) and Rust Anything 5.0.1 on `PATH`, then run `make rust-check` from the repository root. The target uses `cargo oil build --release --locked` for the capture bridge and `cargo oil test --locked` for the backend crate, alongside Rust Anything's capture parity gate. It does not silently fall back if Oak Oil is missing.

Following [Cargo Plans, We Execute](https://engineering.oakoliver.com/articles/oak-oil-cargo-plans-we-execute-lean-fast-rust-builds), use one checkout and one ignored `rust/capturechunk/target` directory. The generated parity bridge expects that path. The check disables incremental compilation and dev/test debug info. Oak Oil's store lives under the user cache by default, with 512 MiB and seven-day GC settings; keep it on the same filesystem as the target directory where practical. Run `cargo oil clean` without `--apply` to inspect stale outputs before any cleanup.

Build the host candidate with `make rust-build`. The development output is written to `rust/capturechunk/target/release/rexweb` and embeds `web/dist` at compile time. Build static release outputs with `make rust-static-build`; ARM64 and AMD64 musl executables are written under `rust/capturechunk/target/<target>/release/rexweb` and checked to have no interpreter or dynamic dependencies. The generated source bridge is test-only; the service binary does not load a shared library. This ordinary host build is dynamically linked and must not be substituted for a release artifact.

## Release switch policy

Keep the Go build as the reference and do not change the default build, tag, or published release merely because the Rust candidate starts or passes unit tests. Switch only after all of the following pass for the **same candidate commit**:

1. `make rust-check` and the existing `make check`/`make race` gates.
2. Official native Rex, Chromium browser, authentication, upload, TLS/Origin, reconnect and Linux sandbox smoke tests against the static Rust executable. The current candidate passes these natively on ARM64.
3. Static musl package builds on Linux ARM64 and AMD64. The current ARM64 binary passes extracted-binary full smoke. The AMD64 binary runs through QEMU and static ELF checks; full native AMD64 hardware smoke remains outstanding.
4. Firefox and desktop WebKit regression coverage, with opt-in WebGPU limits recorded honestly and Canvas fallback passing where GPU display validation is unavailable.
5. A physical iPad Safari check for the visual viewport/floating-keyboard path, terminal focus, IME and page stability. Desktop WebKit emulation is not a substitute for this check.
6. Archive content, third-party notices, checksums and version output reviewed from the final packages.

Until all release gates are complete, keep `main` and release tags on Go and document Rust as a candidate. Issue a new version rather than replacing any published tag or archive.

## Package a verified static candidate

Do not package the development host binary above as a release. Once a static Linux executable for the target architecture is available, package it from the repository root with:

```sh
sh rust/release.sh PATH_TO_STATIC_REXWEB VERSION ARCH
```

`ARCH` must be `arm64` or `amd64`. The script rejects a foreign architecture, a dynamically linked executable, or a binary whose version differs from the requested version. It includes the standalone executable, the embedded frontend, the screenshot and documentation, deployment examples, source notices and licences in the same archive layout as the Go release. It writes the archive and updates `dist/SHA256SUMS`; it does not publish, tag, or build the binary.

For reproducible static builds, install the two Rust musl targets and run `make rust-static-build` on a Linux host with clang, ar and readelf. The script rejects dynamic output. Oak Oil's warm release plans compile no units when inputs are unchanged. Run the packaging step separately on native ARM64 and AMD64 verification hosts, then aggregate and verify the two archives and checksums. The script is intentionally not invoked by `make rust-build`.
