#!/bin/sh
set -eu

scratch=$(mktemp -d /tmp/rexweb-smoke.XXXXXX)
cleanup() {
  if [ -n "${daemon_pid:-}" ]; then
    kill "$daemon_pid" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
  fi
  rm -rf "$scratch"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

scratch=$(cd "$scratch" && pwd -P)
mkdir -m 700 "$scratch/run" "$scratch/work"
printf 'outside data\n' >"$scratch/canary"
ln -s "$scratch/canary" "$scratch/work/escape"
go build -o "$scratch/rexweb" ./cmd/rexweb
ln -s "$HOME" "$scratch/home-alias"
if "$scratch/rexweb" -serve-sessions -workspace "$scratch/home-alias" \
  -session-socket "$scratch/run/session.sock" >"$scratch/rejection.log" 2>&1; then
  echo 'accepted the real home through a symlink'
  exit 1
fi
if ! grep -q 'outside the account home' "$scratch/rejection.log"; then
  cat "$scratch/rejection.log"
  exit 1
fi
REXWEB_SMOKE_SECRET=private "$scratch/rexweb" -serve-sessions -shell /bin/sh \
  -workspace "$scratch/work" -session-socket "$scratch/run/session.sock" -rex-listen 127.0.0.1:0 >"$scratch/daemon.log" 2>&1 &
daemon_pid=$!
for attempt in 1 2 3 4 5 6 7 8 9 10; do
  [ -S "$scratch/run/session.sock" ] && break
  sleep 0.2
done
if [ ! -S "$scratch/run/session.sock" ]; then
  cat "$scratch/daemon.log"
  exit 1
fi

rex_address=$(sed -n 's/.*Rex protocol at http:\/\///p' "$scratch/daemon.log")
bun tests/rex-smoke.ts "$scratch/rexweb" "$scratch/run/session.sock" "$scratch/work" "$scratch/canary" "$rex_address" || {
  cat "$scratch/daemon.log"
  exit 1
}

mkdir -m 700 "$scratch/autostart" "$scratch/autostart/run"
bun tests/rex-autostart.ts "$scratch/rexweb" "$scratch/autostart/run/session.sock" "$scratch/autostart/work"
