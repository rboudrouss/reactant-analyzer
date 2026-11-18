# ADR-047: The call relation is a product of the fixpoint

- **Status**: Accepted
- **Date**: 2026-11-18
- **Follows**: [ADR-042](ADR-042-relations-are-engine-products.md) (relations are engine
  products), design note W1 (`design-walk.md` §2–§3, plan rows P7–P8)
- **Distinct from**: [ADR-036](ADR-036-call-relation.md), the `calls` channel of the setter walk
  (a body's calls by spelling, for packs); this one answers what a call *runs*

## Context

"What may this call run" had two answers. The fixpoint answers with values — heap sites, setter
values, parameters bound to arguments — and that answer writes the store. The writer walk answered
with names and bindings it re-derived (#287, #292, #316, #322), and that answer wrote the rows
every loop claim reads. Each shape the fixpoint already knew had to be taught to the walk again,
with defects of its own: name keying, an unbounded recursion, one slot printed under two
spellings.

## Decision

1. **The fixpoint records it.** At every call it evaluates in a component's passes,
   `exec_callbacks_depth` joins one evaluation into the row of the call's id (a call is an
   allocation site, P5): `Targets { setters, bodies, open, sure, written }`. At every position a
   value is handed to code the walk follows by position — a call's argument, a `new`'s, a JSX
   prop React or a child may invoke, a hook argument the expansion left, an effect's returned
   cleanup — it joins `Escapes { setters, bodies }`: what `escape` reached, or what an in-cycle
   callee ran it as. Stored on `AnalysisResult::calls` (`engine::calls`).
2. **One row per call id, joined over every evaluation.** Passes, fixpoint iterations, inlining
   depths and contexts join: a parent's closure a child calls is evaluated in the child's
   analysis, the owner's run of a closure a child let escape in the owner's, and the program
   analysis joins every kept analysis's relation into one, which every result holds. A body
   spliced or inlined twice has two ids, so two rows. A probe records nothing.
3. **Contract: may, except `sure`.** Every column over-approximates what the evaluations saw.
   `sure` is must: every evaluation that reached the call with a callee saw exactly one target,
   surely — one setter, or one body of a value that is surely that function — and a closed
   callee. `open` (code the analysis does not see, or past the
   inlining budget) adds no target, since what that code may call escaped; it rules `sure` out.
   Every call of a block the fixpoint reached has a row; a call of a body no pass ran has none.
4. **Who may read it.** Engine relations computed at convergence (the writer walk, P8), never
   the rules directly; a rule reads the rows those relations build.
5. **The writer walk reads it and binds nothing** (P8). It keeps the traversal, the phase of
   each position (the timing table, cleanup and listener recognition, the `await` split) and the
   placement. At a call it writes the row's setters and enters the row's bodies, placed only when
   the row is `sure`; at a handed position (an argument, a JSX prop it does not reify as a
   handler region, a hook argument, an effect's cleanup, and at `CALLEE` what an in-cycle
   callee's callbacks returned) it writes every setter and enters every body the value reaches.
   A body of another component hides every name it reads. Past the walk's depth a body still
   runs: `CallRelation::writes_of` gives every slot it may write at any depth, unplaced. A row
   is keyed by (call site, slot, region) and prints the site's own spelling, else the slot's
   setter. The rules layer's own name walks (`setter-in-render`, body scans) are not readers yet.
6. **Every reader of a setter call reads it** (P10, amends §4 and §5). `setter-in-render` reads
   the writer rows; a rule's own body scan asks the engine's walk over that body
   (`setters::body_writes`, `collect_body_calls`, `collect_slot_reads`), the writer walk
   itself. The walk keeps the traversal, the phase classification, placement, provenance and
   the spelling, and nothing else: no name is bound, no table of names resolves a call, and the
   runs of closures the rules layer made for itself (`SiteEnvs::closure_envs`, #246) are gone
   (#314.2: the fixpoint's own runs are the value source). An `Escapes` row is `sure` when every
   evaluation handed an in-cycle callee a value surely its one setter or one function; a
   handed write is then placed as a call's is. A rule reads `CallRelation::body` and
   `slot_name` for a body's CFG and a slot's printed name, nothing else of the relation.

## Consequences

- A reader of "what does this call run" asks the fixpoint's answer, so a shape the fixpoint
  learns is learnt by every reader, and a defect of the answer is fixed once.
- `sure` joins contexts: a helper called with one setter in one context and another elsewhere is
  not `sure`, so an Error through it falls to a Warning — never a finding lost.
- The relation adds one row insert per call evaluation, and a heap reach per handed value, to the
  fixpoint's cost.
