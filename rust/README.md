# Rust service migration

The `rust` branch uses [Rust Anything](https://github.com/erkamyaman/rust-anything) 5.0.1 to port backend units against the Go implementation. The browser remains TypeScript with embedded Ghostty WASM. The shipping build is still Go; this branch does not yet have a standalone Rust service.

| Service area | State |
| --- | --- |
| Native capture pagination | [Rust port and recorded parity suite](capturechunk/README.md); not wired into the service |
| Ghostty VT, image decoding, HTML export | Go only |
| Workspace state, Rex protocol, uploads, PTYs and sandbox | Go only |
| Web and native listeners, authentication, lifecycle and packaging | Go only |

Rust Anything's package scan (`rust-anything analyze .`) flagged cross-package interfaces, pointers and concurrent state in the service. Its Go bridge cannot directly port `internal/vt`'s exported constructor, which takes `context.Context`. Extract small data-only boundaries for parity tests, and port stateful service modules behind the existing browser, native CLI and Linux integration tests. Do not ship the generated CGO bridge; the target is a single self-contained Linux executable with the web bundle embedded.

Switch the default build only after the Rust binary passes the same browser, native CLI, sandbox, authentication, upload and package smoke tests on Linux ARM64 and AMD64. Keep the Go build as the reference until then.
