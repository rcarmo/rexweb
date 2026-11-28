#!/bin/sh
# Package one native, verified static Rust candidate. This script does not build
# or publish a release; run it separately on native ARM64 and AMD64 hosts.
set -eu

usage() {
    printf 'Usage: %s PATH_TO_STATIC_REXWEB [VERSION] [arm64|amd64]\n' "$0" >&2
    exit 2
}

[ "$#" -ge 1 ] && [ "$#" -le 3 ] || usage
binary=$1
release_version=${2:-$(cat VERSION)}
arch=${3:-$(uname -m)}

[ "$(uname -s)" = Linux ] || { echo 'Release packaging requires Linux.' >&2; exit 1; }
case "$(uname -m):$arch" in
    aarch64:aarch64|aarch64:arm64|x86_64:x86_64|x86_64:amd64) ;;
    *) echo 'Package and smoke-test on the native Linux architecture.' >&2; exit 1 ;;
esac

case "$release_version" in
    ''|*[!a-zA-Z0-9.+-]*)
        printf 'Invalid release version: %s\n' "$release_version" >&2
        exit 1
        ;;
esac
case "$arch" in
    aarch64) arch=arm64 ;;
    x86_64) arch=amd64 ;;
    arm64|amd64) ;;
    *)
        printf 'Unsupported release architecture: %s\n' "$arch" >&2
        exit 1
        ;;
esac

[ -f "$binary" ] && [ -x "$binary" ] || {
    printf 'Rust executable is missing or not executable: %s\n' "$binary" >&2
    exit 1
}
command -v readelf >/dev/null 2>&1 || {
    printf 'readelf is required to verify the release executable.\n' >&2
    exit 1
}

machine=$(LC_ALL=C readelf -h "$binary" 2>/dev/null | awk -F: '/^[[:space:]]*Machine:/ { sub(/^[[:space:]]*/, "", $2); print $2; exit }')
case "$arch:$machine" in
    arm64:*AArch64*) ;;
    amd64:*X86-64*) ;;
    *)
        printf 'Executable architecture %s does not match %s.\n' "${machine:-unknown}" "$arch" >&2
        exit 1
        ;;
esac
if LC_ALL=C readelf -l "$binary" | grep -q '[[:space:]]INTERP[[:space:]]' ||
   LC_ALL=C readelf -d "$binary" | grep -q '(NEEDED)'; then
    printf 'Release candidate is dynamically linked; provide a static executable.\n' >&2
    exit 1
fi

version_output=$("$binary" -version 2>&1) || {
    printf 'Could not run %s -version. Package on a native host for %s.\n' "$binary" "$arch" >&2
    exit 1
}
case "$version_output" in
    "rexweb $release_version ("*) ;;
    *)
        printf 'Executable version %s does not match requested %s.\n' "$version_output" "$release_version" >&2
        exit 1
        ;;
esac

archive="rexweb-$release_version-rust-linux-$arch.tar.gz"
name=${archive%.tar.gz}
scratch=$(mktemp -d /tmp/rexweb-rust-release.XXXXXX)
trap 'rm -rf "$scratch"' EXIT
trap 'exit 1' HUP INT TERM
stage="$scratch/$name"
mkdir -p "$stage/bin" "$stage/docs" dist
cp "$binary" "$stage/bin/rexweb"
chmod 755 "$stage/bin/rexweb"
cp docs/three-pane-workspace.png "$stage/docs/"
cp -R licenses deploy "$stage/"
cp LICENSE README.md CHANGELOG.md SECURITY.md THIRD_PARTY_NOTICES.md SPEC.md CONTRIBUTING.md RELEASING.md "$stage/"
printf '%s\n' "$release_version" > "$stage/VERSION"
tar --owner=0 --group=0 -czf "dist/$archive" -C "$scratch" "$name"

checksums="$scratch/SHA256SUMS-rust"
if [ -f dist/SHA256SUMS-rust ]; then
    awk -v arm="rexweb-$release_version-rust-linux-arm64.tar.gz" \
        -v amd="rexweb-$release_version-rust-linux-amd64.tar.gz" \
        -v archive="$archive" '($2 == arm || $2 == amd) && $2 != archive' \
        dist/SHA256SUMS-rust > "$checksums"
else
    : > "$checksums"
fi
(cd dist && sha256sum "$archive") >> "$checksums"
LC_ALL=C sort -k2,2 "$checksums" > dist/SHA256SUMS-rust
printf 'Rust Linux %s archive is in dist/%s\n' "$arch" "$archive"
printf 'Updated checksum list is in dist/SHA256SUMS-rust.\n'
