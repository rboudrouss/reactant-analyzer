# ADR-045: The abstract environment is a persistent map

- **Status**: Accepted
- **Date**: 2026-10-27

## Context

Both solvers keep one `AbstractEnv` per block, and an `AnalysisResult` keeps
them all (`block_states`, the effect and handler block states) for the rules.
With `std::collections::HashMap` fields, each of those envs is a full copy of
every name in scope, so a body's retained size is blocks × names. Splicing
custom hooks grows both factors at once: resolving tsconfig aliases per
project took dub's spliced hooks from 196 to 1,159 on `apps/web/{ui,lib}`,
and the retained entries from 0.67 M to 3.58 M there, 10.9 M on the whole
repository (≈4 GB, the run killed at 7 GB on `twenty`). Every clone, join and
equality test on those envs was linear in the names in scope as well.

## Decision

The five maps of `AbstractEnv` are `imbl::OrdMap`s: a B+ tree with
structural sharing, O(1) clone, and a `diff` that skips the subtrees two maps
share. A block's env shares every node its statements did not rewrite with the
env it started from. `join`, `leq` and equality walk only the entries that
differ (`diff`); a name both sides bind to the same value keeps it, since the
join is idempotent. Widening still visits every name, writing only the ones
it changes. The API of `AbstractEnv` is unchanged.

The render-dependence analysis (`render_deps`) keeps its own per-block env
during its fixpoint, transient but dense in the same way: one 1,447-block
component of twenty grew the live heap by 6 GB there. It takes the same map,
with its values behind an `Arc` (a `DVal` holds several sets, so a node copy
would otherwise deep-copy each of them).

`OrdMap` rather than `imbl::HashMap`: its iteration order is the key order,
deterministic with no hasher to choose, its equality skips shared nodes
(`imbl::HashMap::eq` rebuilds a key set on each call), and a `Var` compare is
cheap next to hashing one.

## Consequences

- One dependency in the core store: `imbl` 7 (MPL-2.0, file-level copyleft,
  compatible with distributing an MIT crate that uses it unmodified; pure Rust,
  builds for `wasm32-unknown-unknown`).
- No semantic change: same lattice operations, same results. The analyzer's
  output is byte-identical on dub (`apps/web/{ui,lib}` and the whole
  repository), and the seed-1 oracle fuzz is unchanged.
- Measured: dub `apps/web/{ui,lib}` 37.0 s / 1.83 GB → 23.5 s / 0.99 GB; whole
  dub (MemoryHigh 5 GB) 392 s / ≥5.1 GB (capped) → 94 s / 3.8 GB.
- twenty `object-record` + `ui` + `views` (MemoryHigh 6 GB): 246 s / 6.1 GB
  (swapping) → 77 s / 0.76 GB once `render_deps` shares too.
- An env that rebinds a name in a shared leaf still copies that leaf (≤16
  entries) and the path above it; a body whose blocks each rebind many names
  keeps a correspondingly larger share of its own.
