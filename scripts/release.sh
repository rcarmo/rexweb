#!/bin/sh
set -eu
[ "$(uname -s)" = Linux ] || { echo 'Release packaging requires Linux.' >&2; exit 1; }
release_version=${1:-$(cat VERSION)}
case "$release_version" in ''|*[!a-zA-Z0-9.+-]*) echo 'Invalid release version.' >&2; exit 1;; esac
revision=$(git rev-parse --short=12 HEAD 2>/dev/null || printf unknown)
scratch=$(mktemp -d /tmp/rexweb-release.XXXXXX)
trap 'rm -rf "$scratch"' EXIT
trap 'exit 1' HUP INT TERM
mkdir -p dist
for arch in arm64 amd64; do
 name="rexweb-$release_version-linux-$arch"
 stage="$scratch/$name"
 mkdir -p "$stage/bin"
 CGO_ENABLED=0 GOOS=linux GOARCH="$arch" go build -trimpath \
  -ldflags "-s -w -X main.version=$release_version -X main.commit=$revision" \
  -o "$stage/bin/rexweb" ./cmd/rexweb
 cp -R licenses deploy "$stage/"
 cp LICENSE README.md VERSION CHANGELOG.md SECURITY.md THIRD_PARTY_NOTICES.md SPEC.md CONTRIBUTING.md RELEASING.md "$stage/"
 tar --owner=0 --group=0 -czf "dist/$name.tar.gz" -C "$scratch" "$name"
done
(cd dist && sha256sum "rexweb-$release_version-linux-arm64.tar.gz" "rexweb-$release_version-linux-amd64.tar.gz" > SHA256SUMS)
printf 'Static Linux ARM64/AMD64 archives and SHA256SUMS are in dist/\n'
