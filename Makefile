.PHONY: install-dev typecheck build build-web test check smoke browser-smoke \
	prefix-browser input-browser cross-browser upload-browser viewport-browser \
	vt-callbacks release clean
.DEFAULT_GOAL := build

VERSION ?= $(shell cat VERSION)
ARCH ?=
REXWEB_BIN ?=
export REXWEB_BIN
export CARGO_TARGET_DIR := $(CURDIR)/target

frontend_deps := web/node_modules/.rexweb-installed
frontend_inputs := $(shell find web/src web/assets) $(shell find licenses -maxdepth 1 -type f) licenses \
	web/index.html web/tsconfig.json web/bunfig.toml THIRD_PARTY_NOTICES.md
smoke_build := $(if $(strip $(REXWEB_BIN)),,build)

install-dev: $(frontend_deps)

$(frontend_deps): web/package.json web/bun.lock web/bunfig.toml
	cd web && bun install --frozen-lockfile
	touch $@

vt-callbacks: install-dev
	bun scripts/build-vt-callbacks.ts

typecheck: install-dev
	cd web && bun run typecheck

build-web: web/dist/index.html

web/dist/index.html: $(frontend_deps) $(frontend_inputs)
	cd web && bun run build

build: build-web
	sh scripts/static-build.sh "$(ARCH)"

test: build-web
	sh scripts/cargo-oil.sh test --locked
	cd web && bun test

check: build-web
	cargo fmt --check
	cargo clippy --locked --all-targets -- -D warnings
	$(MAKE) test

smoke: $(smoke_build)
	sh tests/rex-smoke.sh

browser-smoke: $(smoke_build)
	bun tests/browser-smoke.ts

prefix-browser: $(smoke_build)
	bun tests/prefix-browser.ts

input-browser: $(smoke_build)
	bun tests/input-browser.ts

upload-browser: $(smoke_build)
	bun tests/upload-browser-checks.ts

viewport-browser: $(smoke_build)
	bun tests/viewport-browser-checks.ts

cross-browser: install-dev
	bun tests/cross-browser-checks.ts

release: build
	sh scripts/release.sh bin/rexweb "$(VERSION)" "$(ARCH)"
	VERSION="$(VERSION)" bun tests/release-smoke.ts

clean:
	rm -rf bin target web/dist
