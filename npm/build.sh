#!/usr/bin/env bash
# Build the npm package artifacts: the wasm core (+ Node glue) and the JSON
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

wasm-bindgen target/wasm32-unknown-unknown/release/reactant_wasm.wasm \
  --target nodejs --out-dir npm/dist

# Schemas from the native binary — same commit compiles the shipped core and
# the published schemas, so they validate the same types by construction.
cargo run --quiet --release -- schemas --out npm/schemas

# npm auto-includes README.md/LICENSE only from the package dir; LICENSE is
# copied from the repo root so there is a single source.
cp LICENSE npm/LICENSE
