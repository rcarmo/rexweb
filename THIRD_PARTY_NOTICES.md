# Third-party notices

The project's MIT licence applies to its own code. The following components retain their upstream licences. Release archives include this file and the `licenses/` directory; the built frontend also serves them under `/static/`.

| Component | Version or source | Licence and retained notice |
| --- | --- | --- |
| webterm terminal component and styles | [fe3cc0340b5f4e86ec73ae5dcf311d78930134ec](https://github.com/rcarmo/webterm/tree/fe3cc0340b5f4e86ec73ae5dcf311d78930134ec) | MIT; [original notice](licenses/webterm-MIT.txt), copyright 2026 Rui Carmo |
| ghostty-web JavaScript and WASM | [rcarmo fork, 7ff18ee7b6b42b09da1f5e0553534319d4057cd4](https://github.com/rcarmo/ghostty-web/tree/7ff18ee7b6b42b09da1f5e0553534319d4057cd4) | [MIT](licenses/ghostty-web-MIT.txt), copyright 2025 Coder |
| Ghostty VT core | Fork's submodule [6590196661f769dd8f2b3e85d6c98262c4ec5b3b](https://github.com/ghostty-org/ghostty/tree/6590196661f769dd8f2b3e85d6c98262c4ec5b3b) | [MIT](licenses/ghostty-MIT.txt), copyright 2024 Mitchell Hashimoto and Ghostty contributors |
| Vendored libghostty-webgl | Version 0.0.1, author 0xBigBoss; included by ghostty-web | MIT as declared in the [retained upstream package metadata](licenses/ghostty-webgl-package.json); upstream has no separate licence file |
| PNG decoding runtime | ghostty-web's vendored `@cf-wasm/png` 0.3.3 runtime, powered by denosaurs/pngs | [MIT](licenses/pngs-MIT.txt), copyright 2021 Denosaurs |
| uucode and generated Unicode tables | 0.2.0, used by Ghostty's VT build | [MIT](licenses/uucode-LICENSE.md), with retained [UTF-8 decoder](licenses/uucode-licenses-LICENSE_Bjoern_Hoehrmann) and [Unicode data](licenses/uucode-licenses-LICENSE_unicode) notices |
| Zig runtime | 0.15.2, used to build the bundled VT WASM | [MIT](licenses/zig-MIT.txt) |
| Go standard library and runtime | Compiler version is recorded by `go version -m` on each release binary | [BSD-3-Clause](licenses/go-BSD-3-Clause.txt) |
| creack/pty | [v1.1.24](https://github.com/creack/pty/tree/v1.1.24) | [MIT](licenses/creack-pty-MIT.txt), copyright 2011 Keith Rarick |
| Gorilla WebSocket | [v1.5.3](https://github.com/gorilla/websocket/tree/v1.5.3) | [BSD-2-Clause](licenses/gorilla-websocket-BSD-2-Clause.txt), copyright 2013 The Gorilla WebSocket Authors |
| FiraCode Nerd Font | Fira Code 6.002, patched with Nerd Fonts 3.4.0, as recorded in the font's name table | [SIL OFL 1.1](licenses/FiraCode-OFL-1.1.txt), [Nerd Fonts notices](licenses/nerd-fonts-LICENSE.txt), and retained glyph notices below |

The webterm component has local changes for multiple-pane lifecycle and focus, byte-offset replay and reconnects, throttled terminal fitting during live resizing, and a shared mobile keyboard helper routed to the focused pane, clipboard/IME handling and bounded Unicode input frames. The CSS is adapted for the workspace. Their original MIT notice is retained.

## Font glyph notices

The patched font retains its SIL Open Font License. Its glyph sources also carry the following upstream notices from [Nerd Fonts v3.4.0](https://github.com/ryanoasis/nerd-fonts/tree/v3.4.0/src/glyphs):

- [Codicons](licenses/nerd-fonts-codicons-LICENSE.txt): Creative Commons Attribution 4.0.
- [Font Awesome](licenses/nerd-fonts-font-awesome-LICENSE.txt): upstream font, icon and code notices, including Creative Commons Attribution 4.0.
- [Material Design Icons](licenses/nerd-fonts-materialdesign-LICENSE.txt): Apache 2.0; the [full licence](licenses/Apache-2.0.txt) is included.
- [Octicons](licenses/nerd-fonts-octicons-LICENSE.txt): MIT.
- [Pomicons](licenses/nerd-fonts-pomicons-LICENSE.txt): SIL OFL 1.1.
- [Powerline Extra Symbols](licenses/nerd-fonts-powerline-extra-LICENSE.txt): MIT.
- [Powerline Symbols](licenses/nerd-fonts-powerline-symbols-LICENSE.txt): retained upstream permission notice.

The font is redistributed without local modifications. No Rex app executable, artwork or logo is bundled. The optional native adapter uses a blank Ghostty snapshot captured from an idle, reset `/bin/cat` block in Rex `0.1.0-preview.14`; the fixture contains terminal state, with no program output or filesystem paths. Its provenance is recorded beside the embedded fixture in `internal/workspace/rex_terminal.go`.

Go modules are pinned in `go.mod` and `go.sum`. Browser and build dependencies are pinned in `web/bun.lock`. TypeScript and Bun are build tools; their executables are not included in the release archives.
