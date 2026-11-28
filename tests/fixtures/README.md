# Native capture regression data

`capture-chunks.jsonl` contains 225 synthetic cases recorded with Rust Anything
5.0.1 from `internal/capturechunk/chunk.go` at
[`9932286f9f51488de76aad790c9147f6c20d6305`](https://github.com/rcarmo/rexweb/blob/9932286f9f51488de76aad790c9147f6c20d6305/internal/capturechunk/chunk.go).
`capture-chunk-inputs.jsonl` retains the hand-picked inputs used for that recording;
`capture-chunks-origin.json` retains the original recorder metadata.

`make test` replays the recorded outputs against the same `json_prefix` function
that production native capture and blob reads use. Raw malformed UTF-8 is covered
separately in Rust; JSON fixtures can only represent valid Unicode. This is a
capture-budget regression test, not a claim of full backend parity. It does not
run the historical implementation or require Go or Rust Anything.
