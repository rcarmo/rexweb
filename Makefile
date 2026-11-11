.PHONY: install-dev typecheck build build-web build-go test check race smoke release clean
.DEFAULT_GOAL := build

VERSION ?= $(shell cat VERSION)

install-dev:
	go mod download
	cd web && bun install --frozen-lockfile

typecheck:
	cd web && bun run typecheck

build: build-web build-go

build-web:
	cd web && bun run build

build-go:
	go build -o bin/rexweb ./cmd/rexweb

test:
	go test ./...
	cd web && bun test

check: typecheck
	go vet ./...
	$(MAKE) test

race:
	go test -race ./...

smoke: build-web
	sh tests/rex-smoke.sh

release: build-web
	sh scripts/release.sh "$(VERSION)"
	bun tests/release-smoke.ts

clean:
	rm -rf bin dist web/dist
