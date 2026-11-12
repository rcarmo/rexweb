# Terminal feature priorities

Priorities for rexweb's remote, multi-pane browser workflow. The remaining order balances daily usefulness, integration effort and compatibility risk. Backlog entries are recommendations, not implemented capabilities.

## Integrated

- **ABI-manifest-driven bindings:** cache compound layouts and packed style flags from the loaded WASM; reject incompatible selectors, field representations and unsupported ABI shapes before binding handles.
- **Focused-pane search:** Ctrl+F / ⌘F or the toolbar button; literal, ASCII-case-insensitive search over the active screen and loaded history, with counts, selected-match highlighting and older/newer navigation. Switching panes changes the target. No regex or whole-word matching yet.
- **Shared cursor-anchored IME and native Copy:** one mobile input/keybar follows the active pane's cursor, including horizontal movement and viewport changes. Desktop and mobile Copy use the selected terminal text without background clipboard reads.
- **Cooperative output parsing:** bounded chunks, time-budgeted work and an 8 MiB parser backlog limit. Parser completion controls resume offsets, input readiness and replacement-connection barriers. Reset/disposal discard unparsed work.

The earlier correctness batch includes DEC 2026 presentation guards, canvas recovery, application wheel routing, listener snapshot dispatch, balanced URLs and constructor link-handler preservation.

## Ranked remainder

| Rank | Feature | Why this order | Effort | Main prerequisite or risk |
| --- | --- | --- | --- | --- |
| 1 | Program-status badges and prompt navigation | Helps monitor several sessions and navigate command boundaries. | Medium | Requires emitting applications or shell integration. Native Rex needs authoritative server-side status plumbing. |
| 2 | Text, HTML and VT export | Useful for saving logs and browser-side restoration; the formatter is already in the core. | Medium for browser export | Export does not create server-side VT state, and its output is not the native Rex snapshot format. |
| 3 | Physical-pixel font metrics and grid alignment | Improves zoom, fractional-DPR displays and box-drawing continuity without replacing the rendering architecture. | Medium | Compare fonts, DPRs, browser zoom and split sizes. Separate metrics changes from sprite/core changes. |
| 4 | Advanced search modes | Extends the delivered literal search with case-sensitive, whole-word, regex and broader Unicode matching. | Medium | The core's native matching is literal with ASCII folding. Bound fallback work and regex cost; preserve match positions across wraps and output. |
| 5 | Image and file paste | Valuable for remote tools accepting screenshots and attachments. | Medium–large | Needs an authenticated upload path, limits, safe names, workspace confinement and an explicit path-insertion policy. Never inject raw Ctrl-V ahead of ordinary text paste by default. |
| 6 | Core-managed and rectangular selection | Adds Alt-drag block selection and may reduce JS/core disagreement. | Medium | Replaces working selection internals; verify touch gestures, wide cells, scrollback and logical copy order. |
| 7 | Headless VT state and authoritative capture | Unlocks capture without a browser and stronger native reconnect semantics. | Large | Choose a server-side VT strategy compatible with the Go single binary. A browser addon or separate Bun service does not meet that requirement. |
| 8 | Richer Kitty placements and animated images | Useful for image-heavy terminal workflows, but narrower value than search, input and replay improvements. | Medium–large | Validate image generations, timer disposal, resizing and per-pane memory use. |
| 9 | Additional GPU backend and GPU-loss fallback | Potential gains for demanding multi-pane workloads. Existing opt-in WebGL provides a baseline. | Large | Measure bottlenecks first. Preserve a safe Canvas fallback and test resource cleanup/device loss. |
| 10 | Bidirectional/RTL rendering | Valuable for mixed-direction languages, with significant integration and correctness demands. | Large | Reconcile Unicode table versions and visual/logical mapping for cursor, links, mouse reports and copied text. Prefer opt-in initially. |

## Validation and maintenance

Firefox and WebKit clipboard, IME, scrolling and rendering cases are the next validation work alongside the backlog. Chromium checks alone do not establish Safari parity; desktop WebKit does not replace actual iOS touch and keyboard testing. Current browser checks cover pane-local search, shared-input geometry, native Copy payloads, paste/IME events, and disconnects during yielded replay parsing.

For scheduling changes, retain the parser-completion barriers and test disconnects while processing a large replay, multiple noisy panes and disposal during queued work. For search and export, cover retained history, wraps, Unicode and alternate-screen transitions.

Keep the current custom WASM build until the official prebuilt module has equivalent graphics support: the tested libghostty-vt 1.0.0 small build reports Kitty graphics disabled. Raw snapshot encoding is unstable and should not become a persisted or Rex wire format without explicit version/compatibility handling. Review PNG-decoder updates separately from fork merges.

## Source candidates

Port narrowly and retain the current ABI, renderer lifecycle and security checks. Fork histories contain older memory fixes and API replacements unsuitable for wholesale merging.

- **Search extensions, serialization, selection and browser tests:** [rundel/ghostterm](https://github.com/rundel/ghostterm/tree/0708fc36). Relevant starting points: [serialization](https://github.com/rundel/ghostterm/commit/fb62c9655778c0f7c534e5232883e4ca1b950a66), [search](https://github.com/rundel/ghostterm/commit/9adbb0df918d50960963e78d8d461e832f3a45c8).
- **Image paste:** [Diego's addon](https://github.com/diegosouzapw/ghostty-web/commit/21483bde15626226194699b6bf8075d28decdf72).
- **Program status, GPU rendering, BiDi and Kitty images:** [NimbleMarkets/ghostty-web, nm-kitty-built](https://github.com/NimbleMarkets/ghostty-web/tree/64200917c61d64f0933b2034b0bbb186e3d2b914). Starting points: [program status](https://github.com/NimbleMarkets/ghostty-web/commit/0e226b40a2cb318495413ebe41d2eaa298101404), [resolved/animated placements](https://github.com/NimbleMarkets/ghostty-web/commit/5d75d88bc158ff486f1027d42b55f1469cd68072).
- **Physical-pixel rendering:** [Anomaly's grid alignment work](https://github.com/anomalyco/ghostty-web/commit/0dcf95815d9200761bc543f27df09c9e4232032a), plus NimbleMarkets' newer font and box-drawing work.
- **Scheduling ideas:** [ponzS/ghostty-web](https://github.com/ponzS/ghostty-web). Avoid importing its fork-specific Ghostty/WebShell ABI as part of a scheduling change.
