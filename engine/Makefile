# Covalence engine Makefile
#
# These targets wrap cargo commands with the flock-based test serialization
# required to prevent concurrent cargo test runs from polluting the shared
# covalence_test database (tracking#115).
#
# Usage:
#   make test           — run all engine tests (serialized via flock)
#   make test-all       — run full integration suite (serialized via flock)
#   make fmt            — run cargo fmt on the engine crate
#
# Prefer `../scripts/test-locked.sh` for ad-hoc runs with custom arguments.

LOCK := /tmp/covalence-test.lock

.PHONY: test test-all fmt

test:
	flock $(LOCK) cargo test

test-all:
	flock $(LOCK) cargo test --test integration

fmt:
	cargo fmt
