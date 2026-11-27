#!/bin/sh
set -eu

cd "$(dirname "$0")/capturechunk"
command -v cargo-oil >/dev/null || { echo "Install cargo-oil from oakoliver/oak-oil (see rust/README.md)." >&2; exit 1; }
command -v rust-anything >/dev/null || { echo "Install rust-anything (see rust/README.md)." >&2; exit 1; }

# The generated Go bridge reads target/release directly. Share this target
# directory between Oak Oil, Cargo and the parity gate in this checkout.
export CARGO_TARGET_DIR="$PWD/target"
export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
export OAKOIL_HOME="${OAKOIL_HOME:-${XDG_CACHE_HOME:-$HOME/.cache}/rexweb-rust/oak-oil}"
export OAKOIL_GC_MAX_SIZE="${OAKOIL_GC_MAX_SIZE:-512M}"
export OAKOIL_GC_MAX_AGE_DAYS="${OAKOIL_GC_MAX_AGE_DAYS:-7}"

cargo oil build --release --locked
rust-anything gate .

# Keep the production-facing library separate from the generated bridge so the
# capture gate only measures the code it compares. Both use the same target.
cd ../backend
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo oil test --locked
