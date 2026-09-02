# ADR-034: the registration relation, and one registrar table

- **Status**: Accepted
- **Date**: 2026-09-02
- **Implements**: #111
- **Discharges**: [ADR-027](ADR-027-writer-relation-setter-provenance.md) §2's
  unimplemented registration→Handler summary
- **Follows**: ADR-027 §1 (one central relation, computed once, read by every
  consumer)

## Context

Three readers asked the same question of the same call sites and none of them
shared an answer. `stale-closure` and `missing-cleanup` shared a scan in
`rules::helpers::registrations` with its own `REGISTRARS` list; the slot-writer
walk kept `DEFERRING_GLOBALS` and `DEFERRING_METHODS` in `engine::setters`,
overlapping the first on timers and promise continuations and free to drift from
it. And ADR-027 §2 promised a third thing off that table — a phase summary for
registration-shaped calls — which was never written, so a callback handed to
`addEventListener` fell to ⊤.

## Decision

### 1. One table, with a timing column

`REGISTRARS` and the two `DEFERRING_*` lists become one `&[Registrar]` in
`engine::registrations`. A row carries what each reader needs: `cb_arg` and
`firing` for the two native rules, `teardown` for the pairing fact of §3, and
`timing` for the walk.

### 2. `timing` is what ADR-027 §2 promised, and it is not uniform

- **`Deferred`** — a timer, a microtask, a promise continuation. The callback
  runs on a later turn of the event loop, provably outside every React phase.
  Exactly the former `DEFERRING_*` set, unchanged.
- **`Handler`** — `addEventListener`. The DOM has no synchronous dispatch from
  a registration, so the callback provably does not run during the registering
  call.
- **`Unknown`** — `subscribe`, `on`, `addListener`. An RxJS `BehaviorSubject`
  emits to a new subscriber on the spot, so a synchronous run is not excluded
  and the walk must keep ⊤.

The split is the whole soundness argument. `WriterPhase` is a MAY verdict and ⊤
satisfies every `writer_phases` query, so narrowing a row from `Unknown` to
`Handler` is the one direction that can *lose* a finding — it is allowed only
where the timing is a contract, not a name-table guess. #42's accepted-FP
decision buys over-approximation; it does not buy this.

### 3. Pairing is a three-valued fact, and only `Paired` is a claim

A registration is `Paired` when a cleanup the walk can read calls one of the
row's `teardown` names holding the **same listener binding**. Everything else
is `Unpaired` (a readable cleanup without one, or no cleanup at all) or
`Unknown` (an unreadable cleanup, or a listener that is not a resolvable name).

Matching on the teardown name alone would certify exactly the bug
`subscribe-with-fresh-listener` exists to catch — a cleanup that removes a
*different* listener. The binding is the fact; the name is not.

`Unknown` folds to the may-unpaired side, which is what
[`Pairing::may_be_unpaired`] exposes: a rule that fires on a missing teardown
fires on it, and no rule may read it as a proof of one.

**Amended 2026-09-02 (#124): a teardown comes in three shapes, not one.**
Comparing the listener binding was right for one of them and answered
`none-seen` on correct code for the other two — three measured false positives
on the corpus within hours of shipping:

- **listener-valued** — `removeEventListener(t, h)`, `off(evt, h)`. The rule
  above, unchanged.
- **handle-valued** — `clearInterval(id)`. The teardown takes what the
  registration *returned*, so a listener comparison can never succeed. A
  `teardown_takes` column says which, and the row records the binding the call's
  result was assigned to.
- **disposer-valued** — `const u = store.subscribe(f); return () => u()`. Same
  fact as the handle case; the difference is only whether the cleanup passes the
  handle or invokes it. Available to every registrar that returns something, so
  it is checked whatever the column says.

Plus one registration that takes *itself* back: `addEventListener(t, h, { once:
true })` needs no cleanup and no cleanup could name it, so the row is `Paired`
outright. The boolean third argument is `capture`, not `once`, and does not
count.

`Paired` is the verdict that suppresses, so the disposer form is a **closed set
of method names** (`unsubscribe`, `dispose`, `cancel`, `close`, `destroy`,
`remove`, `off`, `abort`) rather than "any method called on the handle" —
`s.emit('bye')` is not a teardown, and reading it as one would be the false
negative the three-valued design exists to avoid.

The `Unpaired`/`Unknown` split follows: the verdict is a claim only when there
was something to compare. A handle-valued registration whose result is never
bound, and a listener-valued one whose callback is an inline literal, both
answer `Unknown`.

### 4. A handler-class write does not close a churn cycle

`collect_setter_calls_with_extra` computed a `WalkClass` per site and then threw
it away, collapsing to one row per setter variable. `infinite-loop` therefore
could not tell an effect-body write from one that only runs on a keydown, which
is #93. The class is now carried on the row and the rule skips `Handler` —
the churn graph's own documented reasoning ("handlers need a user event, so the
loop is not self-sustaining"), applied where it was missing.

With §2 that predicate covers every `addEventListener` shape, inline literal or
bound name, instead of the reified subset.

### 5. A teardown takes the callback back; it does not call it

The walk descends any function argument of an unknown call, at ⊤, because an
unknown callee may invoke it. `removeEventListener(type, h)` is not unknown any
more — it is the `teardown` column of the row that registered `h` — and it
provably does not call `h`. Descending it gave every registered listener a
*second* writer row at ⊤, produced by the very cleanup that unregisters it, and
a ⊤ row satisfies `writer_phases includes <anything>`: the Handler
classification of §2 would have bought nothing.

Trusting the table here narrows where §2's registrar side widens, so the set is
deliberately only the teardown partners of registrars already in the table —
nothing joins it without a registration to undo.

## Soundness arguments

- **The narrowing of §2 is confined to a contract.** Only `addEventListener`
  moves off ⊤. Every other registration keeps the phase it had.
- **§3 never certifies a teardown it did not read.** The two unreadable cases
  are distinguished from the readable-and-absent one, and both fold to the
  firing side.
- **§4's skip is a narrowing with a semantic argument, not a heuristic.** A
  handler needs an external event; a render loop that requires a user keystroke
  per iteration is not the loop the rule reports.
- **Each of §2, §4 and §5 is pinned by a test that fails when that one is
  removed** (`tests/registrations.rs`), and the corpus is unchanged by the
  relation migration itself.
- **The reified-listener anti-double-count skip survives.** It is a separate
  predicate from §5 — the reification skip stops the *registration* site from
  descending a listener extraction already walked as its own region; §5 stops
  the *teardown* site from descending it a third time — and
  `extracted_subscription_listener_is_handler_not_top` still pins it.

## Consequences

- No Tier-A vocabulary and no catalogue flip here: the public `registrations`
  anchor is #116.
- `setImmediate` joins the table as a `Once`/`Deferred` registrar. It was in
  `DEFERRING_GLOBALS` and not in `REGISTRARS`; `missing-cleanup` fires on
  `Repeating` only, so nothing there changes, and `stale-closure` gains a real
  registration shape it was blind to.
- `rules::helpers::registrations` is deleted.

## Amendment 2026-09-02 — the anchor, and the #42 decision extended (#116)

### 6. The relation becomes public vocabulary, and that extends wontfix #42

The `registrations` anchor exposes the rows to Tier A: `name` (the registrar,
the table key rather than the receiver-qualified display — a pack cannot match
what varies per site), `firing`, and `identity` for the listener. `teardown`
carries the pairing fact, and `registers` puts an existential over an effect
anchor's rows so the `missing-effect-cleanup` entry can finally say *repeating*.

**The decision this records**: wontfix #42's registrar-name heuristic is now
public vocabulary, not just the native rules' interior. The relation is a
may-registration — a name-table match, never a proof the callee is the host
primitive — so the polarity is capped at may/Warning, no `must_*` binds the
sort, and Error is structurally unreachable through the anchor. #42 stays
closed; its accepted-FP decision covers more surface than it did.

### 7. The flip rule's subject is the pairing, not the identity

`subscribe-with-fresh-listener` flips Blocked → Expressible on
`identity is fresh-every-render` ∧ `teardown is none-seen`, over
`firing: repeating` rows only.

Identity alone was refuted, and the counterexample is the React documentation's
own shape: `const h = () => …; el.addEventListener('x', h); return () =>
el.removeEventListener('x', h)` has a listener that IS fresh on every effect
run. A rule keyed on freshness fires on it with a factually false message. The
pairing is what separates the two, which is why §3 exists at all.

`firing: repeating` is the other half. Without it a `.then(() => …)` — a fresh
callback, no teardown possible, `Unpaired` by §3 — would fire, and a promise
continuation accumulates nothing.

### Consequences of the amendment

- `subscribe-with-fresh-listener` flips; the measure moves 20/22 → **21/22**,
  the honest ceiling. The one entry left is `nullable-return-unguarded`,
  excluded by design in #101.
- `missing-effect-cleanup` loses its "no registration fact" weakening.
- Recorded weakening on the flip: effect-body registrations only; a listener
  reached through a prop, an import or a computed receiver reads Unknown and
  never fires; a cleanup the walk cannot read folds to `none-seen` rather than
  being credited; Warning, no must primitive.
- The vocabulary is 23 filtering guards, 5 `must_*`, and 8 anchors.
