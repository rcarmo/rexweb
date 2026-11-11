#!/bin/sh
set -eu

if [ "$(uname -s)" != Darwin ]; then
  echo 'Release packaging requires macOS.' >&2
  exit 1
fi

release_version=${1:-$(cat VERSION)}
case "$release_version" in
  ''|*[!a-zA-Z0-9.+-]*) echo 'Invalid release version.' >&2; exit 1 ;;
esac

revision=$(git rev-parse --short=12 HEAD 2>/dev/null || printf 'unknown')
scratch=$(mktemp -d /tmp/rexweb-release.XXXXXX)
trap 'rm -rf "$scratch"' EXIT
trap 'exit 1' HUP INT TERM
mkdir -p dist
export COPYFILE_DISABLE=1

for arch in arm64 amd64; do
  name="rexweb-$release_version-darwin-$arch"
  stage="$scratch/$name"
  mkdir -p "$stage/bin" "$stage/web"
  CGO_ENABLED=0 GOOS=darwin GOARCH="$arch" go build -trimpath \
    -ldflags "-s -w -X main.version=$release_version -X main.commit=$revision" \
    -o "$stage/bin/rexweb" ./cmd/rexweb
  cp -R web/dist "$stage/web/dist"
  cp -R licenses "$stage/licenses"
  cp LICENSE README.md VERSION CHANGELOG.md SECURITY.md THIRD_PARTY_NOTICES.md \
    SPEC.md AGENTS.md CONTRIBUTING.md RELEASING.md "$stage/"
  cp web/src/webterm/LICENSE "$stage/licenses/webterm-MIT.txt"
  cp web/assets/fonts/FiraCode-LICENSE.txt "$stage/licenses/FiraCode-OFL-1.1.txt"
  /usr/bin/tar --uid 0 --gid 0 --uname root --gname root \
    -czf "dist/$name.tar.gz" -C "$scratch" "$name"
done

(
  cd dist
  shasum -a 256 "rexweb-$release_version-darwin-arm64.tar.gz" \
    "rexweb-$release_version-darwin-amd64.tar.gz" > SHA256SUMS
)
printf 'Release archives and SHA256SUMS are in dist/\n'
