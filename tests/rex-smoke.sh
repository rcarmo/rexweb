#!/bin/sh
set -eu
exec bun "$(dirname "$0")/rex-smoke.ts" "$@"
