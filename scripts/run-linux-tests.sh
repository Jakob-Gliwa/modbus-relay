#!/bin/sh
# Cargo runner for cross-compiled static Linux tests. Mount at the same absolute
# path so CARGO_BIN_EXE_* in CLI tests resolves correctly inside the container.
set -eu
modbus_test_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
exec docker run --rm --network none \
  --mount "type=bind,source=$modbus_test_root,target=$modbus_test_root,readonly" \
  --workdir "$modbus_test_root" \
  --env "PROPTEST_CASES=${PROPTEST_CASES:-2048}" \
  "${MODBUS_TEST_IMAGE:-alpine:3.23}" "$@"
