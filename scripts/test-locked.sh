#!/bin/bash
# Serialize cargo test runs to prevent concurrent database state pollution.
#
# Usage: ./scripts/test-locked.sh [cargo test arguments]
#
# Background: concurrent `cargo test -p covalence-engine` runs share the
# covalence_test database and produce flaky failures through state pollution
# (tracking#115). This wrapper uses flock(1) to ensure only one test run
# executes at a time. A second caller blocks for up to 60 seconds, then
# fails with a clear error if the lock is still held.
#
# Examples:
#   ./scripts/test-locked.sh                            # all tests
#   ./scripts/test-locked.sh -p covalence-engine        # engine only
#   ./scripts/test-locked.sh --test integration         # integration suite
#   ./scripts/test-locked.sh -p covalence-engine -- --nocapture

set -euo pipefail

LOCK=/tmp/covalence-test.lock

exec flock -w 60 "$LOCK" cargo test "$@"
