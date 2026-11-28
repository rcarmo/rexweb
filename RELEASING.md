# Releasing rexweb

The `rust` branch builds static Linux ARM64 and AMD64 candidates. Each archive contains one Rust executable with embedded JavaScript, WASM and fonts, plus documentation, licences and systemd examples. The published [Go v0.1.1 release](https://github.com/rcarmo/rexweb/releases/tag/v0.1.1) and `main` are unchanged.

1. For a new candidate, update `VERSION`, the Cargo package version and lockfile, and [CHANGELOG.md](CHANGELOG.md). Use a new `-rust.N` version; do not reuse a published tag.
2. On Linux hosts with unprivileged user namespaces, run `make check` and `REX_REFERENCE_BIN=/path/to/rex make smoke`. Run the Chromium workspace, prefix, IME, viewport and upload tests, plus the Firefox and desktop WebKit checks [listed in the contributing guide](CONTRIBUTING.md).
3. On native ARM64 and AMD64, run `REX_REFERENCE_BIN=/path/to/rex make release` from the same commit. It builds and checks the native musl executable, packages it and smoke-tests the extracted executable. Cross-compilation and QEMU startup checks do not replace native sandbox tests.
4. Review the version output, archive contents, third-party notices, native smoke results and `dist/SHA256SUMS-rust`. Candidate archive names are `rexweb-<VERSION>-rust-linux-<arch>.tar.gz`; `<arch>` is `arm64` or `amd64`. These names and checksum file are separate from the Go release. `make release` does not tag, push or publish.

WebGPU stays opt-in. Where a usable GPU is unavailable, verify Canvas fallback and record WebGPU display validation as unavailable. The compositor does not rasterise glyphs or lay out text on the GPU. Before switching the public build, check the floating keyboard, input focus and page stability on a physical iPad. Desktop WebKit and simulated mobile viewports do not exercise these hardware-dependent behaviours.

Rust candidate CI runs on native Linux ARM64 and AMD64 runners. It installs the pinned Rust toolchain and Oak Oil, enables unprivileged user namespaces for the disposable test hosts and tests the static candidates. When authorised to stage a candidate, push a tag of the form `v*-rust.*` matching `v$(cat VERSION)`, for example `v0.1.1-rust.1`. The release workflow collects the native packages and checksums into a draft GitHub prerelease after both jobs pass. Review the draft; do not replace a published tag or archive.
