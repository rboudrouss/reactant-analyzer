# ADR-036: the call relation — what a body *does*, beyond its setters

- **Status**: Accepted
- **Date**: 2026-09-02
- **Implements**: #126
- **Follows**: ADR-027 §1 (one central relation, computed once, read by every
  consumer), ADR-034 (the same shape, one relation lower)
- **Amends**: [ADR-035](ADR-035-await-phase-boundary.md) — where the split point
  sits relative to the awaited expression

## Context

The blind wish-list campaign named this the #1 gap in one triage and, in
narrower forms, in three of the four. An effect body was legible to a pack
through exactly three relations — `body_setter_calls`, `deps`, and the
fixed-name-table `registrations` — so everything else a body *does* was
invisible: `el.getBoundingClientRect()`, `fetch(u, {method:"POST"})`,
`socket.join(room)`, `URL.createObjectURL(f)`, `router.push(…)` during render.
Six of the sixty scenarios turn on naming one of those calls.

## Decision

### 1. It is the setter walk's second output, not a second walk

`engine::setters`' walk already answers the hard part: which class a site runs
in, how a directly-called local helper and an IIFE splice into their call site,
where the `await` boundary is, and that an effect's returned function is its
cleanup. A second traversal would re-derive all of it and drift.

So the walk's output becomes two channels — `setters`, unchanged, and `calls`,
filled only when a consumer asks (`collect_calls`). The re-tagging of an
inlined body's rows, previously written twice, becomes `Found::absorb` and now
serves both channels by construction.

The cost gate is the flag. Every setter consumer keeps a walk that pushes
nothing extra; a body with 200 calls does not pay for 200 rows on a walk the
rule pass runs per component per rule.

### 2. A row is `(name, receiver, phase)` — and no argument values

`name` is the function of a bare call or the *method* of a member call;
`receiver` is the root binding the member call was made on. Two fields, because
`socket.join` and any-`.join` are different questions and a rule should say
which it is asking. A callee that resolves to neither — an IIFE, an element of
an array, the result of a call — produces no row: there is nothing a pack could
match.

`phase` is the `WriterPhase` lattice the writer rows already carry, so a call
in a `.then`, past an `await`, or in the returned cleanup is distinguishable
from one in the body, and the words mean the same thing in both relations.

Argument values stay out. That is #67's question, and it should stay one
question; argument *positions* with an identity verdict are the reasonable v2
the observer and resource scenarios actually need.

### 3. A `name` guard is mandatory

This is the first unbounded relation: every other one enumerates something the
engine had a reason to record, and this one enumerates *every call*. A rule
with no `name` guard fires on all of them, which is an attractive nuisance
rather than a rule — so the loader rejects it, and rejects it for a name guard
hidden inside an `any_of`, where one branch could still leave the row unnamed.

### 4. Warning ceiling, structurally

No `must_*` binds the sort. The callee is a resolved binding, never a proof of
which host primitive runs — the same footing as the registrar table, and the
same accepted-FP decision (wontfix #42). Error is unreachable through the
relation.

### 5. The render body gets an anchor, not an edge

`calls` is an edge on the four anchors with a body (effect, memo, callback,
handler), exactly where `body_setter_calls` applies. The render body has no
hook to hang an edge on, so it gets `render_calls`, exactly as its setter calls
already do. Same relation, same mandatory guard, same ceiling.

### 6. The awaited expression is evaluated before the suspension

ADR-035 splits the block at an `await`. The split was placed *after* lowering
the awaited argument but before the enclosing statement was emitted, so in
`const r = await fetch(u)` the `fetch` call travelled into the post-await
block. For a write that made no difference — the write is genuinely on the far
side — but for a call relation it is a false claim: `deferred` means "provably
never inside a React phase", and `fetch` runs synchronously in the effect.

The awaited value is now bound on the near side of the edge, so the call sits
where it runs. A bare name or literal is passed through — there is nothing to
evaluate and no reader wants the binding.

### 7. The negated existential, and why it is admissible

Naming a call is half of what the wish-list asked for; the other half is
*naming its absence* — "acquires a resource and releases none", "has a `value`
prop and no `onChange`", "subscribes and never reads the current value". A
`forEach` is the existential over an edge, and there was no way to write its
negation: `any_of` composes guards, `every` folds over `anchor.deps` only, and
`else: drop` chooses between keeping and dropping a finding, not between rows.

`none` quantifies over any edge of the anchor, typed by the very table the
`forEach` navigation reads (`edge_element_sort`, extracted here so the two
spellings cannot drift).

**The unsound direction is the safe one.** Every relation it can range over may
under-enumerate — a depth-capped walk, a callee it could not resolve — and a
missing row makes `none` pass, so the rule fires where it should not. That is a
false positive; the direction this project never takes is losing a finding. The
mirror-image hazard, a row the relation invented, does not arise: these rows
are call sites, deps entries and write sites the engine saw.

Like `every`, it never mints a proof: a `must_*` inside it, or anywhere in a
rule that uses it, is refused at load time.

### 8. Host elements are an anchor option, not a widening

`jsx_props` gained `elements: component | host | any`, defaulting to
`component` — what the relation has always meant, and the only place a prop is
compared by `Object.is` across a memo boundary. Host elements carry the rest of
the render surface (`<input ref={r} value={v}/>`) and a DOM rule needs them.

It is an option rather than a guard-triggered widening because it changes
*which rows exist*: a shipped pack must keep binding exactly the rows it always
did (ADR-027 §2, the rule #107 followed for foreign setter rows). `kind`
answers `component` / `host` on the row, and `NativeElem` now carries its
opening tag's span, so a host finding points at the element rather than at the
enclosing hook.

### 9. The `prop` guard

`jsx_props` carried `prop` as a renderable field with no guard over it, so a
rule could not skip `children` — fresh on every wrapper — nor scope itself to
`value`, `key` or a handler. It is `text_guard(Field::Prop, …)`: the same
matcher `name` and `source` already are, over a field the relation already had.

### 10. The element becomes an anchor, so an absence can be asked about (#126)

§8 made host elements reachable and §9 gave `prop` a guard, and the corpus
immediately showed what was still missing: `<input value={v}/>` with no
`onChange` — the one shape a `jsx_props` rule cannot state, because `none`
ranges over an *edge*, and `jsx_props` is an edge-less anchor whose rows are
already flattened away from the element that carries them.

So the element becomes the subject: an `elements` anchor (same `elements`
filter, same default) with a `props` edge. `jsx_props` is unchanged — it is now
literally `collect_jsx_elements` plus a flatten, re-sorted by the ordinal each
row already carried, so the two shapes cannot disagree about which elements
exist or what a prop's identity is, and the flat enumeration is bit-identical.

## Soundness arguments

- **The setter relation is byte-identical.** The channel is off for every
  setter consumer, and `absorb` is the two duplicated re-tagging loops verbatim.
  Corpus: 0 added, 0 removed on all fourteen repositories.
- **`phase` is may-typed and named as such.** ⊤ (`unknown`) is a matchable
  name, so a rule that will accept it says so; the guard is positive-only, so a
  ⊤ row can never suppress a finding.
- **§6 moves a claim back to the truth, in the direction that adds findings.**
  A call that read `deferred` now reads `effect`, and `effect` is the phase a
  rule keyed on the body will match.
- **The mandatory guard is a load-time rejection, not a runtime filter**, so a
  pack that would have fired everywhere never loads.
- **§7's absence errs towards firing**, and it is doubly barred from Error: no
  `must_*` may appear inside it, and none may appear in the same rule.
- **§8 changes no shipped pack's rows.** The default is the historical
  enumeration and the corpus is unchanged.
- **§10's two shapes are one relation.** A test asserts the flat and grouped
  enumerations agree row for row, so the grouping cannot drift.

## Consequences

- Vocabulary: 10 anchors, 8 edges, 27 filtering guards, 5 `must_*`.
- `tests/body_calls.rs` pins the lattice, the receiver discrimination, the
  await placement, the mandatory guard (including the `any_of` hole), the two
  type errors, and the Warning ceiling.
- What this does NOT give: argument values (#67) — so an acquire and a release
  can be required to *co-occur* (§7) but not to name the same key; a join
  between two free anchors (#68); and any proof that the named callee is the
  host primitive.
