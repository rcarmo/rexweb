#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
backend="$root/rust/backend"
target_dir="$root/rust/capturechunk/target"

command -v cargo-oil >/dev/null || { echo "Install cargo-oil from oakoliver/oak-oil." >&2; exit 1; }
command -v rustup >/dev/null || { echo "rustup is required to install the musl target libraries." >&2; exit 1; }
command -v clang >/dev/null || { echo "clang is required to compile target-native C sources." >&2; exit 1; }
command -v ar >/dev/null || { echo "ar is required to archive target-native C objects." >&2; exit 1; }
command -v readelf >/dev/null || { echo "readelf is required to verify static release binaries." >&2; exit 1; }

export CARGO_TARGET_DIR="$target_dir"
export CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_DEBUG=0
export OAKOIL_HOME="${OAKOIL_HOME:-${XDG_CACHE_HOME:-$HOME/.cache}/rexweb-rust/oak-oil}"
export OAKOIL_GC_MAX_SIZE="${OAKOIL_GC_MAX_SIZE:-512M}"
export OAKOIL_GC_MAX_AGE_DAYS="${OAKOIL_GC_MAX_AGE_DAYS:-7}"

installed=$(rustup target list --installed)
for target in aarch64-unknown-linux-musl x86_64-unknown-linux-musl; do
    printf '%s\n' "$installed" | grep -qx "$target" || {
        echo "Missing Rust target $target; run: rustup target add $target" >&2
        exit 1
    }
done

cd "$backend"
for target in aarch64-unknown-linux-musl x86_64-unknown-linux-musl; do
    cargo_key=$(printf '%s' "$target" | tr '[:lower:]-' '[:upper:]_')
    cc_key=$(printf '%s' "$target" | tr '-' '_')
    linker_var=CARGO_TARGET_${cargo_key}_LINKER
    cc_var=CC_${cc_key}
    ar_var=AR_${cc_key}
    cflags_var=CFLAGS_${cc_key}

    env "$linker_var=rust-lld" \
        "$cc_var=clang" \
        "$ar_var=ar" \
        "$cflags_var=--target=$target" \
        cargo oil build --replan --release --locked --target "$target"

    binary="$target_dir/$target/release/rexweb"
    test -x "$binary"
    file "$binary"
    if readelf -l "$binary" | grep -q 'Requesting program interpreter'; then
        echo "release is not static: $binary has an ELF interpreter" >&2
        exit 1
    fi
    if readelf -d "$binary" 2>/dev/null | grep -q '(NEEDED)'; then
        echo "release is not static: $binary has dynamic dependencies" >&2
        exit 1
    fi
done
