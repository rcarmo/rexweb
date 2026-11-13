# Terminal feature coverage

All ten requested areas are incorporated. The options and compatibility limits below describe what is shipped, rather than a future backlog.

| Feature | Incorporated behaviour | Limits |
| --- | --- | --- |
| Program status and prompts | Authoritative title/cwd, OSC 7501 state/progress badges, OSC 133 shell marks and focused Previous/Next prompt actions | Applications must emit reports; custom startup files may replace shell hooks. |
| Export | Command-palette text, HTML and VT export from the server VT; bounded native Rex capture/blob pagination | Not a stable raw Ghostty snapshot format. HTML is sanitised and has a restrictive CSP. |
| Physical-pixel metrics | DPR-aligned cell pitch, baseline and backing dimensions; resize/zoom remeasurement | Font appearance remains browser/font dependent. |
| Advanced search | Case, whole-word and linear-time RE2JS regex modes; cancellable snapshots and wrapped/Unicode cell mapping | Bounded cells/text/matches; unsupported regex syntax is rejected and partial results are marked. |
| Image/file paste | Authenticated picker, clipboard image and drop uploads; confined atomic storage and quoted path insertion without Enter | Explicit `-uploads`, password authentication, Origin validation, per-file/request/concurrency limits; no automatic retry. |
| Core/rectangular selection | Core pins and formatter for linear copy; Alt-drag/programmatic rectangles, backward gestures and history | In BiDi mode rectangle columns are visual and copied cells are emitted in logical order per row. |
| Server VT and capture | Pure-Go wazero execution of the embedded Ghostty core, shared compilation, authoritative replies, browser and native capture | Bounded memory/history/calls/captures; no external runtime process. |
| Resolved/animated Kitty images | Resolved geometry, layers, crops and generations; bounded owned pixels and deadline-driven animation | Server PNG/image budgets and client memory bounds; timers stop on reset/disposal/suspension/held output. |
| WebGPU and fallback | Opt-in GPU compositor, bounded texture storage and device-loss promotion to Canvas without discarding VT/selection | Canvas still rasterises glyphs; no GPU-side glyph layout/instancing. GPU display behaviour is unvalidated in this environment. |
| RTL/BiDi | Opt-in row-local ordering with mapped cursor/IME, linear/rectangle selection, search highlights, links and mouse reports | Unicode 13 BiDi tables, LTR paragraph base; no Arabic joining/shaping or cross-row paragraph layout. Image-placeholder rows retain logical order. |

## Enabling optional rendering

Use the command palette's **Try WebGPU compositor (reload)** and **Enable RTL/BiDi (reload)** actions, or URL parameters `renderer=webgpu` and `bidi=1`. Both are off by default. Reloads reconnect to the same server session; they do not restart its shells. GPU failure retains a Canvas path. The mobile keyboard/input helper remains document-wide and follows the focused pane.

## Verification

- ghostty-web: 532 passing tests and two existing historical scrollback skips; formatting, lint, type checking and rebuilt distributions pass.
- rexweb: Go vet/unit tests, race detector, frontend checks, Linux sandbox/security/native/TLS smoke and official Rex CLI interoperability pass.
- Chromium: full workspace/input/replay/upload checks, shared-helper checks, option reload/session preservation, fractional DPR, image animation and BiDi mapping/copy tests pass.
- Firefox and desktop WebKit: Canvas recovery/DEC guards, wheel/link behaviour, rectangle copy, image animation and BiDi cursor/link/mouse/copy tests pass. Their GPU-unavailable Canvas paths pass. Fractional-DPR overrides remain Chromium-only.
- GPU resource/cleanup tests and earlier software-device readback/device-loss checks passed, but visible GPU presentation is not validated here. This limitation is accepted, not counted as a display-test pass.
- Release archives target Linux ARM64/AMD64. Extracted ARM64 runtime is tested locally; AMD64 runtime and physical iOS remain untested locally. Desktop WebKit does not establish iOS keyboard/touch parity.

## Maintenance contracts

Preserve parser-consumption replay offsets and parser-drain replacement barriers. Received bytes alone must not advance reconnect accounting or enable input. Preserve authentication, Host/Origin checks, fail-closed sandboxing, upload directory-FD confinement and paste sanitisation.

Keep the current custom WASM build until the official prebuilt module has equivalent graphics support: the tested libghostty-vt 1.0.0 small build reports Kitty graphics disabled. Raw snapshot encoding is unstable and should not become a persisted or Rex wire format without explicit version/compatibility handling.

## Source provenance

- Current renderer: [rcarmo/ghostty-web 65ea3dd](https://github.com/rcarmo/ghostty-web/tree/65ea3ddbbd421c5a863c4de72866faf2ae73ad58).
- Official core: [Ghostty c6174c88](https://github.com/ghostty-org/ghostty/tree/c6174c88f42b48adb14857c9233b16f6947ab732), built with Zig 0.17.0.
- ABI/search/scheduling ideas: [rundel/ghostterm](https://github.com/rundel/ghostterm/tree/0708fc36).
- Status, image and BiDi starting points: [NimbleMarkets/ghostty-web](https://github.com/NimbleMarkets/ghostty-web/tree/64200917c61d64f0933b2034b0bbb186e3d2b914). The GPU compositor is a local implementation, not a port of its glyph-instancing pipeline.
- Physical-pixel work: [Anomaly's grid alignment](https://github.com/anomalyco/ghostty-web/commit/0dcf95815d9200761bc543f27df09c9e4232032a).
- Image-paste source survey: [Diego's addon](https://github.com/diegosouzapw/ghostty-web/commit/21483bde15626226194699b6bf8075d28decdf72).

Licences and retained notices are listed in [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md).
