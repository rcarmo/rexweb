#!/bin/sh
set -eu

cd "$(dirname "$0")/.."
[ "$(uname -s)" = Linux ] || { echo 'The service builds on Linux.' >&2; exit 1; }
case "${1:-$(uname -m)}" in
    arm64|aarch64) target=aarch64-unknown-linux-musl ;;
    amd64|x86_64) target=x86_64-unknown-linux-musl ;;
    *) echo 'ARCH must be arm64 or amd64.' >&2; exit 1 ;;
esac
for tool in clang ar readelf; do
    command -v "$tool" >/dev/null || { printf '%s is required.\n' "$tool" >&2; exit 1; }
done
rustup target list --installed | grep -qx "$target" || {
    printf 'Install the musl target: rustup target add %s\n' "$target" >&2
    exit 1
}

# Let Cargo check its existing target outputs when the bounded Oak Oil store
# has evicted cached objects for a different target.
sh scripts/cargo-oil.sh build --replan --release --locked --target "$target"
binary="target/$target/release/rexweb"
test -x "$binary"
if LC_ALL=C readelf -l "$binary" | grep -q '[[:space:]]INTERP[[:space:]]' ||
   LC_ALL=C readelf -d "$binary" | grep -q '(NEEDED)'; then
    printf 'Expected a static executable: %s\n' "$binary" >&2
    exit 1
fi
if [ ! -x bin/rexweb ] || ! cmp -s "$binary" bin/rexweb; then
    install -D -m 755 "$binary" bin/rexweb
fi
printf 'Static %s executable: bin/rexweb\n' "$target"
