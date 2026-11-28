# Third-party notices

The project's MIT licence applies to its own code. Release archives include this file and the `licenses/` directory. The embedded browser frontend also serves this file and the browser and WASM notices under `/static/`.

| Component | Version or source | Licence and retained notice |
| --- | --- | --- |
| webterm terminal component and styles | [fe3cc0340b5f4e86ec73ae5dcf311d78930134ec](https://github.com/rcarmo/webterm/tree/fe3cc0340b5f4e86ec73ae5dcf311d78930134ec) | MIT; [original notice](licenses/webterm-MIT.txt), copyright 2026 Rui Carmo |
| ghostty-web JavaScript and WASM | [rcarmo fork, 65ea3ddbbd421c5a863c4de72866faf2ae73ad58](https://github.com/rcarmo/ghostty-web/tree/65ea3ddbbd421c5a863c4de72866faf2ae73ad58) | [MIT](licenses/ghostty-web-MIT.txt), copyright 2025 Coder |
| bidi-js bidirectional algorithm | [1.0.3](https://github.com/lojjic/bidi-js), Unicode 13 tables | [MIT](licenses/bidi-js-MIT.txt), copyright 2021 Jason Johnston |
| Adapted BiDi row mapper | [NimbleMarkets/ghostty-web 64200917](https://github.com/NimbleMarkets/ghostty-web/tree/64200917c61d64f0933b2034b0bbb186e3d2b914) | [MIT](licenses/nimble-bidi-MIT.txt), copyright 2025 Coder |
| RE2JS regular expression engine | [2.10.0](https://github.com/le0pard/re2js), linear-time advanced search | [MIT](licenses/re2js-MIT.txt) |
| Ghostty VT core | Fork's submodule [c6174c88f42b48adb14857c9233b16f6947ab732](https://github.com/ghostty-org/ghostty/tree/c6174c88f42b48adb14857c9233b16f6947ab732) | [MIT](licenses/ghostty-MIT.txt), copyright 2024 Mitchell Hashimoto and Ghostty contributors |
| Vendored libghostty-webgl | Version 0.0.1, author 0xBigBoss; included by ghostty-web | MIT as declared in the [retained upstream package metadata](licenses/ghostty-webgl-package.json); upstream has no separate licence file |
| PNG decoding runtime | ghostty-web's vendored `@cf-wasm/png` 0.3.3 runtime, powered by denosaurs/pngs | [MIT](licenses/pngs-MIT.txt), copyright 2021 Denosaurs |
| uucode and generated Unicode tables | 0.2.0, used by Ghostty's VT build | [MIT](licenses/uucode-LICENSE.md), with retained [UTF-8 decoder](licenses/uucode-licenses-LICENSE_Bjoern_Hoehrmann) and [Unicode data](licenses/uucode-licenses-LICENSE_unicode) notices |
| Wuffs image decoding support | [0.4.0-alpha.10, commit 7411f488fe2e2c205c3d3b3d28638b7356522930](https://github.com/google/wuffs/tree/7411f488fe2e2c205c3d3b3d28638b7356522930), linked by the Ghostty VT build | MIT (selected from Apache-2.0 OR MIT); [notice](licenses/wuffs-MIT.txt) |
| Zig runtime | 0.17.0, used to build the bundled VT WASM | [MIT](licenses/zig-MIT.txt) |
| FiraCode Nerd Font | Fira Code 6.002, patched with Nerd Fonts 3.4.0, as recorded in the font's name table | [SIL OFL 1.1](licenses/FiraCode-OFL-1.1.txt), [Nerd Fonts notices](licenses/nerd-fonts-LICENSE.txt), and retained glyph notices below |

The webterm component is adapted for native desktop textarea focus, cursor-anchored composition, same-origin viewer handoff, pane lifecycle, consumed-byte replay offsets, shared cursor-anchored mobile input, clipboard/IME handling and focused-pane search. The CSS is adapted for the workspace. Its original MIT notice is retained. The ghostty-web fork includes MIT-licensed adaptations from [rundel/ghostterm](https://github.com/rundel/ghostterm) for ABI layouts, search integration and cooperative parsing.

## Font glyph notices

The patched font retains its SIL Open Font License. Its glyph sources also carry the following upstream notices from [Nerd Fonts v3.4.0](https://github.com/ryanoasis/nerd-fonts/tree/v3.4.0/src/glyphs):

- [Codicons](licenses/nerd-fonts-codicons-LICENSE.txt): Creative Commons Attribution 4.0.
- [Font Awesome](licenses/nerd-fonts-font-awesome-LICENSE.txt): upstream font, icon and code notices, including Creative Commons Attribution 4.0.
- [Material Design Icons](licenses/nerd-fonts-materialdesign-LICENSE.txt): Apache 2.0; the [full licence](licenses/Apache-2.0.txt) is included.
- [Octicons](licenses/nerd-fonts-octicons-LICENSE.txt): MIT.
- [Pomicons](licenses/nerd-fonts-pomicons-LICENSE.txt): SIL OFL 1.1.
- [Powerline Extra Symbols](licenses/nerd-fonts-powerline-extra-LICENSE.txt): MIT.
- [Powerline Symbols](licenses/nerd-fonts-powerline-symbols-LICENSE.txt): retained upstream permission notice.

The font is redistributed without local modifications. No Rex app executable, artwork or logo is bundled. Native Rex support uses a blank Ghostty snapshot captured from an idle, reset `/bin/cat` block in Rex `0.1.0-preview.14`. The fixture contains terminal state, with no program output or filesystem paths.

## Rust dependency notices

The [crate licence index and retained notices](licenses/rust/RUST-DEPENDENCIES.md) are generated from `Cargo.lock` and included in the source tree and release archives. Run `bun scripts/licenses.ts` after updating Rust dependencies, or add `--check` to verify the retained copies without changing them.

Browser and frontend build dependencies are pinned in `web/bun.lock`. TypeScript and Bun are build tools; their executables are not included in the release archives.
