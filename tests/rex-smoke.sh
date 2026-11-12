#!/bin/sh
set -eu
exec bun tests/rex-smoke.ts "${1:-bin/rexweb}"
