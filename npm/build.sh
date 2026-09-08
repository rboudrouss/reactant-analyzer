#!/usr/bin/env bash
# Build the npm package artifacts: the wasm core (+ its JS glue) and the JSON
# schemas. Plain cargo + wasm-bindgen-cli — wasm-pack's generated package.json
# would be overwritten anyway. wasm-bindgen-cli must match the crate's
# wasm-bindgen version (`cargo install wasm-bindgen-cli --version <x>`).
set -euo pipefail
cd "$(dirname "$0")/.."

# `cargo install` drops binaries in $CARGO_HOME/bin, which distro-packaged
# cargo setups don't put on PATH.
PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"

rustup target add wasm32-unknown-unknown

# 8 MiB stack: the analysis is recursive (bounded inline depth, oxc's
# recursive descent) and the wasm default of 1 MiB is tighter than native.
RUSTFLAGS='-C link-arg=-zstack-size=8388608' \
  cargo build -p reactant-wasm --release --target wasm32-unknown-unknown

# `--target web`, not `nodejs`, for BOTH hosts: the emitted `.wasm` is
# byte-identical across wasm-bindgen targets, and the web glue runs under Node
# as well (the bytes come from readFileSync + initSync instead of fetch — see
# npm/lib/core-node.js). One artifact, one glue, both hosts; a second target
# would ship a second copy of the same 3.5 MB module.
wasm-bindgen target/wasm32-unknown-unknown/release/reactant_wasm.wasm \
  --target web --out-dir npm/dist

# Schemas from the native binary — same commit compiles the shipped core and
# the published schemas, so they validate the same types by construction.
cargo run --quiet --release -- schemas --out npm/schemas

# lib/pack.d.ts from the freshly-written pack.schema.json (ADR-023 §5): the
# authoring types compile from the same schemars output as the validator.
node npm/scripts/gen-pack-dts.js

# npm auto-includes README.md/LICENSE only from the package dir; LICENSE is
# copied from the repo root so there is a single source.
cp LICENSE npm/LICENSE
