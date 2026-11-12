# Terminal feature priorities

Priorities for rexweb's remote, multi-pane browser workflow. The order balances daily usefulness, integration effort and compatibility risk. These are backlog recommendations, not features already implemented.

The correctness batch covers DEC 2026 synchronized output, canvas recovery, application wheel routing, listener disposal, balanced URL handling and constructor-provided link interception. The shared mobile keyboard and tested paste path remain unchanged.

## Ranked backlog

| Rank | Feature or foundation | Why this order | Effort | Main prerequisite or risk |
| --- | --- | --- | --- | --- |
| 1 | ABI-manifest-driven bindings | Prevents another round of manual struct/enum/packed-bit mistakes when updating Ghostty. High leverage before adding more core APIs. | Medium | Resolve layouts once per loaded module; validate unsupported ABI shapes and bound allocations. |
| 2 | Focused-pane search | Immediate value for long logs, errors and previous commands, using an API the core already provides. | Medium | Bridge literal search first. Treat regex, case-sensitive and whole-word search as separate bounded fallbacks. |
| 3 | Cursor-anchored IME and native clipboard fidelity | Improves everyday mobile/CJK use and context-menu Copy without expanding the service's trust boundary. | Medium | Keep one shared helper. Position it relative to the focused pane and cursor; verify composition, pane switches and clipboard ownership. |
| 4 | Cooperative output parsing | Keeps multiple panes responsive during large logs and reconnect replay. | Medium | Advance replay offsets only after bytes have actually been parsed. Preserve input and callback order. Start with time-budgeted work before introducing workers. |
| 5 | Program-status badges and prompt navigation | Helps monitor several sessions and find command boundaries. | Medium | Requires emitting applications or shell integration. Native Rex needs authoritative server-side status plumbing. |
| 6 | Text, HTML and VT export | Useful for saving logs and browser-side restoration; enables incremental work towards capture. | Medium for browser export | The formatter does not create server-side VT state. Do not equate its output with native Rex's snapshot format. |
| 7 | Physical-pixel font metrics and grid alignment | Improves zoom, fractional-DPR displays and box-drawing continuity without replacing the rendering architecture. | Medium | Compare visual output across fonts, DPRs, browser zoom and split sizes. Separate metrics changes from sprite/core changes. |
| 8 | Image and file paste | Valuable for remote tools that accept screenshots and attachments. | Medium–large | Needs an authenticated upload path, limits, safe names, workspace confinement and an explicit path-insertion policy. Never inject raw Ctrl-V ahead of ordinary text paste by default. |
| 9 | Core-managed and rectangular selection | Can improve selection consistency and add Alt-drag block selection. | Medium | Replaces working selection internals; verify touch gestures, wide cells, scrollback and logical copy order. |
| 10 | Headless VT state and authoritative capture | Unlocks capture when no browser is attached and stronger native reconnect semantics. | Large | Choose a server-side VT strategy compatible with the Go single binary. A browser addon or separate Bun service does not meet that requirement. |
| 11 | Richer Kitty placements and animated images | Useful for image-heavy terminal workflows, but narrower value than search, input and replay improvements. | Medium–large | Validate image generations, timer disposal, resizing and per-pane memory use. |
| 12 | Additional GPU backend and GPU-loss fallback | Potential gains for demanding multi-pane workloads. Existing opt-in WebGL provides a baseline. | Large | Measure bottlenecks first. Preserve a safe Canvas fallback and test resource cleanup/device loss. |
| 13 | Bidirectional/RTL rendering | Valuable for mixed-direction languages, with significant integration and correctness demands. | Large | Reconcile Unicode table versions and visual/logical mapping for cursor, links, mouse reports and copied text. Prefer opt-in initially. |

## Validation work alongside the backlog

Add Firefox and WebKit clipboard, IME, scrolling and rendering cases as the related work lands. Chromium checks alone do not establish Safari parity, and desktop WebKit does not replace testing actual iOS touch and keyboard behaviour.

For output scheduling, test disconnects while parsing a large replay, multiple noisy panes and disposal during queued work. For search and export, cover retained history, wraps, Unicode and alternate-screen transitions.

Keep the current custom WASM build until the official prebuilt module has equivalent graphics support: the tested libghostty-vt 1.0.0 small build reports Kitty graphics disabled. Raw snapshot encoding is unstable and should not become a persisted or Rex wire format without explicit version/compatibility handling.

## Source candidates

Port narrowly and retain the current ABI, renderer lifecycle and security checks. Fork histories contain older memory fixes and API replacements that are not suitable for wholesale merging.

- **ABI layouts, search, serialization, cooperative parsing, selection and browser tests:** [rundel/ghostterm](https://github.com/rundel/ghostterm/tree/0708fc36). Relevant starting points: [manifest layouts](https://github.com/rundel/ghostterm/commit/7a7a9ccfcea609e5a68d6d11a92d66da86ad9892), [search](https://github.com/rundel/ghostterm/commit/9adbb0df918d50960963e78d8d461e832f3a45c8), [serialization](https://github.com/rundel/ghostterm/commit/fb62c9655778c0f7c534e5232883e4ca1b950a66), [bounded parsing](https://github.com/rundel/ghostterm/commit/c653ed12b2d970889d89460148164fab174b8d3f).
- **IME positioning and image paste:** [diegosouzapw/ghostty-web](https://github.com/diegosouzapw/ghostty-web). Starting points: [IME cursor positioning](https://github.com/diegosouzapw/ghostty-web/commit/588dac89c14861f050c40c1d9adb628b86821470), [image paste](https://github.com/diegosouzapw/ghostty-web/commit/21483bde15626226194699b6bf8075d28decdf72).
- **Program status, GPU rendering, BiDi and Kitty images:** [NimbleMarkets/ghostty-web, nm-kitty-built](https://github.com/NimbleMarkets/ghostty-web/tree/64200917c61d64f0933b2034b0bbb186e3d2b914). Starting points: [program status](https://github.com/NimbleMarkets/ghostty-web/commit/0e226b40a2cb318495413ebe41d2eaa298101404), [resolved/animated placements](https://github.com/NimbleMarkets/ghostty-web/commit/5d75d88bc158ff486f1027d42b55f1469cd68072).
- **Physical-pixel rendering:** [Anomaly's grid alignment work](https://github.com/anomalyco/ghostty-web/commit/0dcf95815d9200761bc543f27df09c9e4232032a), plus NimbleMarkets' newer font and box-drawing work.
- **Scheduling ideas:** [ponzS/ghostty-web](https://github.com/ponzS/ghostty-web). Avoid importing its fork-specific Ghostty/WebShell ABI as part of a scheduling change.
