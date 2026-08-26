#!/usr/bin/env bash
# `reactant packs build` (ADR-023 §5): the JS-authored fixture must compile to
# byte-identical committed JSON, and lib/pack.d.ts must be current with the
# schema. The JSON's own validity is proven Rust-side (tests/declarative.rs
# loads the expected file through load_pack).
set -euo pipefail
cd "$(dirname "$0")/.."

out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT

node bin/reactant.js packs build test/fixtures/team.pack.js --out "$out/team.pack.json" > /dev/null
if ! cmp -s "$out/team.pack.json" test/fixtures/team.pack.expected.json; then
  echo "FAIL: packs build output drifted from test/fixtures/team.pack.expected.json"
  diff test/fixtures/team.pack.expected.json "$out/team.pack.json" | head -20 || true
  exit 1
fi
echo "ok:   packs build (byte-identical)"

node scripts/gen-pack-dts.js --check
