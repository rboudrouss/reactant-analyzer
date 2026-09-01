# ADR-033: the binding chase carries an exactness bit, and its cycle guard is per-branch

- **Status**: Accepted
- **Date**: 2026-09-02
- **Fixes**: #120 (output not run-to-run deterministic)
- **Amends**: [ADR-031](ADR-031-slot-seed-relation.md) — the chase moved into
  `engine/seeds.rs` unchanged, and carried this in with it

## Context

The analyzer was not run-to-run deterministic: on mantine a true-positive
`frozen-initial-state` finding appeared in roughly one run in six. Rust seeds
every `HashMap`/`HashSet` differently per process, so the flap was an
order dependence — but the order dependence was a symptom. The defect
underneath it is that `normalize_to_prop` answered a **different prop** each
run.

`normalize_to_prop` rewrites a local path into props-param-rooted form by
chasing single-binding chains, so that a seed and a dep written differently can
be compared. On mantine it resolved `defaultColorScheme` to
`__p0.forceColorScheme`, `__p0.getRootElement`, `__p0.colorSchemeManager` or
`__p0.defaultColorScheme` depending on the run, and the dep `manager.subscribe`
to another arbitrary member of the same set. When the two happened to collide,
the dep "covered" the seed, the slot read as synced, and the finding vanished.

## Decision

### 1. The cycle guard is cloned per branch, because it keys on roots and the recursion is over paths

When the chase cannot select through a right-hand side it widens to every path
that side reads and recurses on each. Those siblings shared one `seen` set,
which keys on the **root variable** — but the recursion is over **paths**, and
two sibling paths sharing a root are not a cycle. `seed` and `other` both reach
`__p0` through the same destructuring temp, so whichever branch ran first
consumed that temp and every later branch died on it. The result was one
arbitrary survivor.

Cloning `seen` per branch restores all of them. It still terminates: each
branch's own chain cannot revisit a root, which is what stops `a = a.b` from
growing a path forever. Branching only happens where the chase cannot select,
and §2 removes the common reason it could not.

### 2. An object literal is selected through, not widened past

`{ manager: colorSchemeManager }.manager` *is* `colorSchemeManager`. The IR has
carried the per-member map since #88; the chase was not reading it, so a
component that packs props into an object — every custom hook called with an
options object, after inlining — lost the selector and widened to the whole
literal. Selecting consumes the segment and keeps the chase exact.

The spread rule is the interpreter's, and now literally so: `object_member` and
the interpreter's `obj_members` share `members_after_last_spread` in
`ir::expr`. A member a spread may have overwritten is not resolvable, and two
readers of one rule must not each keep their own copy of it.

### 3. A widened path may not support a must-claim

The chase has two consumers that want opposite things from the same widening,
so the answer carries a bit rather than picking a side:

- `seed_paths` asks *may this initializer read a prop* — a widened path answers
  it, and a wrong guess costs a false positive.
- `deps_cover_seed` asks *does this dep cover that seed* — a **must**-claim
  that suppresses a finding. A widened path stands for a set of possibilities,
  and matching one possibility against another suppresses on a coincidence.

So `normalize_to_prop` returns `NormPath { path, exact }`, `deps_cover_seed`
credits only `exact` on both sides, and the row keeps the full may-set.

This is also what makes §1 safe on its own. Restoring the dropped siblings
*widens* both sides of the coverage test, and widening the declared side
suppresses more — the forbidden direction. The bit is not a refinement of the
fix; it is the half that keeps the other half sound.

## Soundness arguments

- **Every part of this change moves suppression in one direction: less of it.**
  §2 replaces a set of guesses with the one right answer, §3 refuses credit to
  a guess. Neither can newly silence a finding that fired before.
- **§1 alone would move it the other way**, which is why §3 ships with it and
  not after it.
- **Termination is unchanged.** The per-branch clone still refuses a root the
  current chain already visited; a unit test pins `a = a.b`.
- **In this chase, determinism is now structural rather than observed**: the
  widened fold sorts its sub-paths before recursing and dedups its output, so no
  `HashSet` order reaches a result. That is a claim about the chase, not about
  the analyzer — whole-program determinism stays an empirical property, and the
  corpus sweep below is how it is checked.

## Consequences

- `frozen-initial-state` gains the findings the collisions were eating, and the
  delta is additive: measured against `3eab6b5` over the fourteen corpora, one
  run each, thirteen are byte-identical and mantine gains exactly one Warning —
  the `use-provider-color-scheme` freeze ADR-031's corrected note traces. Nothing
  is removed anywhere, which is what §3 is for.
- Each half is gated by a test that fails when that half alone is removed —
  six unit tests on the chase in `engine::seeds::chase_tests`, and two
  end-to-end tests in `tests/frozen_initial_state.rs` covering both directions
  (a sibling member is not a sync; the same prop named differently still is).
- `tests/declarative.rs::runs_are_deterministic` covered a flat component with
  no chase at all. `repeated_analyses_of_an_object_literal_chase_agree` covers
  the path it missed, in-process — a fresh process is not needed, because Rust
  reseeds each container within one.
- ADR-031's corpus claim is re-verified in its own §"Soundness arguments" note.
- `collect_component_setter_vars` carries the same class of order dependence —
  first env wins, iterated over a `HashMap` — and is fixed alongside: it now
  walks `CFG::blocks` order, with its allocation-site and capture scans sorted.
  Unlike the chase this one was **latent**: no corpus output was observed to
  vary through it, so it is closed on inspection rather than on a repro. A
  determinism fix only — *which* block should win is #119, unchanged here.
- Two of #120's three suspects were wrong. The registry's `get_by_name` already
  sorts its matches (ADR-013), and `analyze_program`'s phase 1 already iterates
  sorted roots. Only the two sites above were live.
