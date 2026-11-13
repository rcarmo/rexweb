# Releasing rexweb

Release archives target Linux ARM64 and AMD64. Each contains one static Go executable with embedded JavaScript, WASM, fonts and licences, plus documentation and systemd examples. Neither archive needs a separate frontend at runtime.

1. Update `VERSION` and `CHANGELOG.md`.
2. Run `make install-dev build check race smoke release` on Linux with unprivileged user namespaces enabled.
3. Set `REX_REFERENCE_BIN` to repeat smoke/release tests against an official Rex CLI when available.
4. With Playwright browsers and their host libraries installed, run `make browser-smoke` and `make cross-browser`. The latter covers Firefox and desktop WebKit; it does not replace physical iOS testing.
5. Review `dist/SHA256SUMS`, archive contents, notices and native extracted-package test results. Run smoke tests on both target architectures.

WebGPU stays opt-in. If the test host has no usable GPU, record display validation as unavailable rather than a pass; the Canvas fallback must still pass. GPU composition does not imply GPU-side glyph rasterization or measured performance gains.

The CI matrix tests Ubuntu ARM64 and AMD64. Its disposable runners explicitly enable unprivileged user namespaces for isolation tests; production hosts need an appropriate local policy. The release workflow packages on Linux after both verification jobs pass, checks the tag against VERSION and creates a draft GitHub release. These workflows need to be enabled in the target repository. `make release` only creates local archives.

Tag a release commit with `v$(cat VERSION)` and push the tag to start the release workflow. GitHub Actions uses its short-lived workflow token, with contents:write only on the packaging job. Review the draft before publishing. Replace neither published tags nor published archives; issue a new version instead.
