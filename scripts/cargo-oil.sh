#!/bin/sh
set -eu

cd "$(dirname "$0")/.."
command -v cargo-oil >/dev/null || {
    printf '%s\n' 'Install cargo-oil v0.3.2 from https://github.com/oakoliver/oak-oil/releases.' >&2
    exit 1
}
export CARGO_TARGET_DIR="$PWD/target" CARGO_INCREMENTAL=0
export OAKOIL_HOME="${OAKOIL_HOME:-${XDG_CACHE_HOME:-$HOME/.cache}/rexweb-rust/oak-oil}"
export OAKOIL_GC_MAX_SIZE="${OAKOIL_GC_MAX_SIZE:-512M}"
export OAKOIL_GC_MAX_AGE_DAYS="${OAKOIL_GC_MAX_AGE_DAYS:-7}"

cargo oil "$@"
# Oak Oil's automatic sweep runs at most once a day. Enforce the budget
# after each build/test so repeated cross-target builds don't accumulate.
cargo oil gc
