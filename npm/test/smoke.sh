#!/usr/bin/env bash
# Byte-for-byte smoke test: the wasm wrapper against the native binary, on
# the repo fixtures, same cwd (same relative path display). Run after
# npm/build.sh; needs a release native binary (cargo build --release).
set -euo pipefail
cd "$(dirname "$0")/../.."

NATIVE=target/release/reactant
WASM="node npm/bin/reactant.js"
export NO_COLOR=1

fail=0
compare() {
  local label="$1"; shift
  local n_out n_code w_out w_code
  # Non-zero exits are expected (findings → 1, usage → 2): capture, don't -e.
  n_out=$("$NATIVE" "$@" 2>&1) && n_code=0 || n_code=$?
  w_out=$($WASM "$@" 2>&1) && w_code=0 || w_code=$?
  if [[ "$n_out" != "$w_out" || "$n_code" != "$w_code" ]]; then
    echo "FAIL: $label (native=$n_code wasm=$w_code)"
    diff <(echo "$n_out") <(echo "$w_out") | head -20 || true
    fail=1
  else
    echo "ok:   $label (exit $n_code)"
  fi
}

compare "check human"   check tests/fixtures/vite_project --fail-on never
compare "check json"    check tests/fixtures/vite_project --format json --fail-on never
compare "check trace"   check tests/fixtures/vite_project --trace --info --show-clean --fail-on never
compare "exit code"     check tests/fixtures/vite_project --fail-on warning
compare "next human"    check tests/fixtures/next_project --fail-on never
compare "next json"     check tests/fixtures/next_project --format json --info --fail-on never
compare "pack project"  check tests/fixtures/pack_project --format json --fail-on never
compare "config off"    check tests/fixtures/config_project --config tests/fixtures/config_project/off.json --format json --fail-on never
compare "rules"         rules
compare "explain"       explain infinite-loop
compare "explain pack"  explain team/effect-writes-own-dep --config tests/fixtures/pack_project/reactant.config.json

# The JS→JSON authoring path (byte-identity + d.ts currency).
if ! npm/test/packs.sh; then fail=1; fi

exit $fail
