# Native capture chunking

The Rust implementation chooses a byte boundary for a Rex capture reply under a JSON string-size budget. It matches Go's escaping of ASCII controls, HTML-sensitive characters, and Unicode line separators. It also handles malformed UTF-8; the JSON bridge itself can only supply valid Unicode strings, so Rust unit tests cover malformed bytes separately.

The Go service still calls the original implementation in `internal/capturechunk`. The generated Go and Rust bindings are parity-test tools, not production dependencies. No shared library or C toolchain is required to run rexweb.

## Verify the port

From the repository root, with Rust Anything 5.0.1, Rust 1.93+, and the Go toolchain installed:

```sh
rust-anything record rust/capturechunk --count 200 --inputs rust/capturechunk/inputs.jsonl
rust-anything gate rust/capturechunk
```

`inputs.jsonl` contains targeted boundary cases; `cases.jsonl` records the Go results for replay. The gate runs formatting, Clippy, unit tests, build and behavior comparison.
