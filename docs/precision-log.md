# Precision log

Precision corrections: a rule over-reported a shape, the engine was fixed where
the information was being lost, and the corpus measured it.

**This is not architecture**, so it does not live in [`adr/`](adr/). An ADR
records a decision the rest of the system must respect: a domain, a relation, an
invariant, a rejected alternative. A precision correction records a
*measurement*: the shape, the claim that settles it, the corpus delta. One entry
here, one commit message, and one line in
[`limitations.md`](limitations.md) if a limitation remains.

Every claim stays subject to the project's invariants: false positives
tolerated, **false negatives forbidden**. A precision correction only removes
locations; a *soundness* correction adds some, and those additions are findings
a bug had been silencing. #134 is the only line that is both at once. It removes
26 locations and adds 15, which is exactly what an identity bug looks like:
reads answering from the wrong object, in both directions
([#134](https://github.com/rboudrouss/reactant-analyzer/issues/134)).

## The metric

The comparable column is the number of **distinct locations**
`(file, line, column, message)`. The JSON keeps one row per (finding,
component), since #129 only groups at display time, so the row count depends on
how you count and is not a clean series from one campaign to the next. The first
entry predates this metric and is quoted in raw findings.

Corpus: `test-repo/`, 14 repositories, **40,164 files** since the pinning of
2026-09-04 (34,747 before it; see the series break below, the two halves of the
table are not comparable).

**The analysis is deterministic.** Four runs of a frozen binary on one
repository, and two on the whole corpus, produce *bit-identical* JSON files. A
gap between two measurements is therefore always a real behaviour change or a
counting error, never noise.

**An endpoint is counted, and so are removals.** The 2026-09-03 figures had to
be re-measured, and the error was at both ends of the calculation: removals
counted by re-reading one family by hand rather than by diffing, and an endpoint
deduced from the delta instead of counted. The column being cumulative, a single
wrong line shifts every line after it, and for #134 the sign itself flipped. The
full account is under "Table correction" below.

The rule that comes out of it: [`corpus-diff.py`](../scripts/corpus-diff.py) over
two runs, which always prints `before / after / removed / added` and exits with
an error if the three do not reconcile. What was re-read at the source is not
what changed; the two are stated separately.

The **2026-09-03** lines have been re-measured (see "Table correction" at the end
of this document). The **2026-09-02** lines are not verifiable, since no binary
from that session survived, and they are left as they are.

| date | claim | issue | locations |
|---|---|---|---|
| 2026-09-02 | the longest stable prefix | residue of #88 | −686 findings (6,340 → 5,654) |
| 2026-09-02 | a dynamic index hides what is below it, not the chain above | #89 §3/§4 | 1,423 → 1,417 |
| 2026-09-02 | a dep that *is* the read | #89 §1 | 1,417 → 1,402 |
| 2026-09-02 | a closure reached through a container is still a closure | #89 | 1,402 → 1,394 |
| 2026-09-02 | a rename is not a read | #89 §2 | 1,394 → 1,359 |
| 2026-09-02 | a write that settles its own guard | #91 | 1,359 → 1,343 |
| 2026-09-03 | a member is not the slot | #90 | 1,348 → 1,345 |
| 2026-09-03 | the identity of an allocation site | #134 | 1,345 → 1,334 (26 removed, **15 added**) |
| 2026-09-03 | a library contract is about members | #94 | 1,334 → 1,326 |
| 2026-09-03 | a wrapper does not run its argument | #94 | 1,326 → 1,325 |
| 2026-09-03 | a wrapper is not necessarily stable | #94 | 1,325 → 1,325 (see the entry) |
| 2026-09-03 | a member read needs the converged heap | #135 | 1,325 → 1,325 (see the entry) |
| 2026-09-03 | a slot's writers are read from the relation | #92 | 1,325 → 1,314 |
| 2026-09-03 | a tuple contract is indexed by position | #37 | 1,314 → 1,314 (see the entry) |
| 2026-09-03 | a setter's owner is read at the call site | #119 | 1,314 → 1,314 (see the entry) |
| 2026-09-04 | a clean bill is only worth what was read | #9, #47 | 1,314 → 1,314 |
| 2026-09-04 | a subdirectory is still inside its project | #9 | 1,314 → 1,314 |
| 2026-09-04 | a hook in a terminator is still a hook | #4, #5 | 1,314 → **1,340** (10 removed, 36 added) |
| 2026-09-04 | try/catch/finally is control flow | #2 | 1,340 → **1,344** (0 removed, 4 added) |
| n/a | **corpus pinned: the series restarts** | #15 | same binary, 1,344 → 1,358 |
| 2026-09-04 | a callee's free variable is not the caller's | #141 | 1,358 → **1,317** (41 removed, 0 added) |
| 2026-09-04 | the marker is where the tsconfig search starts, not where it stops | #139 | 1,317 → 1,317 (see the entry) |
| 2026-09-04 | a directory is generated because the repository says so | #137 | 1,317 → 1,317 (0 removed, 0 added; **+88 files read**) |
| 2026-09-04 | following a narrowed run's imports, behind a flag | #138 | 1,317 → 1,317 (default unchanged; see the entry) |

---

## 2026-09-02: the longest stable prefix

*Residue of #88. Frame: [ADR-017](adr/ADR-017-versioned-stability.md), identity
against behaviour.*

`missing-deps` asked whether a capture can go **stale** by looking at two things
and nothing in between: the path's root, and the whole path.

```js
const r = useRef(0);
const bag = { r };
useCallback(() => r.current,     []);   // silent: the root is stable
useCallback(() => bag.r.current, []);   // reported: "`bag` is recreated"
```

**The claim.** A read is stale only if *every* handle it goes through can
change. `bag.r` is the same ref on every render, so the stale copy of `bag`
reaches that ref and reads its current value. A single stable prefix closes the
question. This is not a new exemption: it is the one the rule already had for
the root, which stopped at an arbitrary depth. `Stability::Stable` is a must
claim, since neither ⊤ nor ⊥ is stable, so no prefix can be called stable out of
imprecision.

**Corpus: −686 findings (6,340 → 5,654), none added**, all `missing-deps`, all
one shape (`$values.refValues.current`, a `useRef` reached through a container
that mantine's `useFormValues` rebuilds every render). 11% of the total output.

What keeps firing, rightly: `$values.setValues`, a `useCallback` with a
**non-empty** deps list. Its identity changes when its own dep moves, so no
prefix is stable and the capture really can go stale.

---

## 2026-09-02: a dynamic index hides what is below it, not the chain above

*#89, shapes 3 and 4.*

`extract_path` reduced *any* member chain containing a computed access to its
bare root, so `theme.snackBar[variant].color` was recorded as the whole of
`theme`, which nothing short of a `[theme]` dep can cover.

**The claim.** `x.a[i].b` records `x.a`: the last named handle the read goes
through. This is the stable-prefix claim on the other side of the comparison,
since the read is fresh as soon as `x.a` is, exactly as a `[x.a]` dep already
covers an `x.a.b` read. The segments *below* the index stay lost, so an `[x.a.b]`
dep does not cover `x.a[i].b`: the prefix test falls on the right side by itself.
On the **dep** side nothing changes: `[x.a[i]]` still declares nothing, because a
dep pins the element and not the container.

The second shape, that a `useCallback` is a closure and so the behavioural
question must be asked of it too, is taken up and generalized by the "a closure
reached through a container" entry below.

**Corpus: 1,423 → 1,417 locations, 6 removed, none added**, each one re-read at
the source (memos `PagedMemoList.tsx:163`, next-shadcn `kanban.tsx:720`, which
carries an `eslint-disable` saying precisely this, twenty `SnackBar.tsx:163`,
mantine `use-form-errors.ts:44`).

`extract_path` is shared: `missing-deps`, `stale-closure`, the mount helper and
the seed scan all read a path the same way, and a longer path is more coverable,
never less.

---

## 2026-09-02: a dep that *is* the read

*#89 §1, its sound half.*

Everything a body computes *from* a read is decomposed into the underlying
reads, so a deps array declaring the computation rather than its inputs declared
nothing:

```js
useCallback(() => {
  const sort = searchParams.get("sort");
  queryParams({ del: sort });
}, [queryParams, searchParams.get("sort")]);   // ← reported `searchParams.get`
```

**The claim.** A sub-expression appearing **verbatim** in the deps array is
pinned by it: React compares that expression's value, so the hook is recreated as
soon as it changes and the body's evaluation cannot diverge from the current one.
Verbatim is the whole claim, and it is what draws the line:

- `[searchParams.get(urlParam)]` pins `searchParams.get` **and** `urlParam`;
- `[JSON.stringify(o)]` pins **nothing** for a body reading a bare `o`, because a
  serialization is lossy: `o` can move without the dep moving, and crediting that
  would be a false negative;
- `excludedPayoutIds.length` keeps firing next to a pinned
  `excludedPayoutIds.join(",")`, because a different expression is a different
  read.

**The separation is the load-bearing part.** `EffectInfo` carries both sets.
*This* hook cannot go stale on a pinned read, so `missing-deps` skips it; but a
**consumer** of the produced value still holds a closure over that read, so the
behavioural stability check reasons over the full `free_paths`. Merging them
would make `useCallback(() => log(n), [n])` look like an empty capture and
silence the stale consumer. Two regression tests hold the line on both sides.

**Corpus: 1,417 → 1,402 locations, 15 removed, none added.** Cost: a second
free-path pass per hook whose deps contain at least one keyable expression, which
is inside measurement noise (dub 69.0s → 70.8s).

---

## 2026-09-02: a closure reached through a container is still a closure

*#89, the container half.*

The behavioural question was only asked of a **bare name**:

```js
const bump = useCallback(() => { r.current += 1 }, [n]);
const api  = { bump };
useCallback(() => bump(),     []);   // silent
useCallback(() => api.bump(), []);   // reported
```

A container is how a custom hook returns a closure: mantine's `useFormErrors()`
returns five members, each one a `useCallback`.

**The claim.** The binding chase takes a **path**, not a name. A bare name is the
base case; each segment enters the field of the single `ObjectLit` the prefix is
bound to, following variable aliases (`{ bump }` records the member as
`Var("bump")`). The certainty bar is unchanged and applies to **every hop**: a
name bound zero times or more than once resolves nothing, and neither does a
member behind a spread that could have overwritten it.

Two readers become one: `fn_binding_in` and `callback_binding_in` were the same
chase narrowed to one spelling each; `closure_binding_of` answers both and says
which.

**Corpus: 1,402 → 1,394 locations, 8 removed, none added**, all
`$errors.<member>` in mantine's `use-form.ts`, the four members checked by hand.
Those eight are worth 392 attributions because `useForm` is consumed throughout
mantine.

---

## 2026-09-02: a rename is not a read

*#89 §2, the last of the four shapes. Issue closed.*

The free-path walk recorded every `Expr` it met, so a binding that only *names*
counted as a read of the whole:

```js
useMemo(() => {
  const c = performanceCondition;      // ← recorded: all of `performanceCondition`
  if (!c.attribute) return "attribute";
}, [performanceCondition?.attribute, performanceCondition?.value]);
```

**The shape that pays is not the explicit alias but destructuring**, which all
React code writes: `const { viewport } = ctx` lowers to
`__obj = ctx; viewport = __obj.viewport`, so a read of the whole context preceded
every `[ctx.viewport, ctx.offset]`.

**The claim.** The walk skips the right-hand side of a `let` that binds a name,
exactly once, to a simple member chain, and rewrites paths rooted at that name to
what it renames. Everything else stays a read: a name bound twice is not a
rename, and a right-hand side that *computes* has reads of its own. Nothing is
lost when the alias is used whole, since `JSON.stringify(c)` records a bare `c`,
which the rewrite turns back into a bare `performanceCondition`.

**Companion naming rule.** Refining `settings` into the eight members a body
touches is more exact and less readable: eight lines carrying one instruction.
So **when the deps array names nothing rooted at an object, the finding names the
object**; where it does name members, the uncovered ones are listed one by one.
Same choice one rule further on: several members of an object seeding the same
slot are named by the handle they share (`AccessPath::common_prefix`).

**Corpus: 1,394 → 1,359 locations. Eight hook sites go quiet across the two
changes and no site gains one**; the rest of the movement is the same finding
renamed. Ten `frozen-initial-state` messages stop naming an arbitrary member;
nine of them already were arbitrary before, and resolving renames made it
visible.

**Invariant changed.** The root set of `compute_free_paths` is now a **subset**
of that of `compute_free_vars`, where the two used to coincide.
`compute_free_vars` over-approximates deliberately: `missing-deps` reads it for
the capture set of a function literal, where under-declaring would silence a real
stale closure.

---

## 2026-09-02: a write that settles its own guard

*#91, the compare-then-sync family.*

`converges_once_written` proved that an effect fires only once by *value*: bind
the slot to the written value, narrow the dominating guards, see if one falls to
⊥. That proves the fetch-once shape and nothing else, because the corpus shape is
**relational**:

```js
if (scale < scaleForCurrentValue) { setScale(scaleForCurrentValue); }  // React's own idiom
if (internalDate !== date)        { setInternalDate(date); }
```

No interval bounds either side. `x < y` after `x := y` is false for *all* x and
y, and that is a fact about the **relation** between the two, which no
non-relational domain represents at any precision.

**The claim.** Spellings say what values cannot. A guard is settled when one side
is a path rooted at the written slot and the other is, verbatim, the expression
the write puts there: both denote the same value on the next render, so `<`, `>`,
`!=` and `!==` are false and `==`, `===`, `<=` and `>=` are true. If that
contradicts the polarity of the branch taken, the branch is dead.

Three mechanisms already in place carry it to the real shapes: the member walk
(the closure through a container), the binding chase (the rename), and the
canonical spelling **minus calls**.

**Why calls are excluded.** The claim is that two spellings denote one value. A
call does not guarantee that, not even twice within one render, since
`f(x) !== f(x)` is a possible program. Pinning a dep, on the other hand, can go
through a call: there it is React's `Object.is` doing the comparing. A *name*
bound to a call is still a good spelling, because the name is bound once. `NaN`
is the only value that would break the equalities, and it cannot bite: React
drops an update that is `Object.is`-equal to the current one.

**Corpus: 1,359 → 1,343 locations, 16 removed, none added.** Ten `infinite-loop`
(a quarter of that rule's output) and six `setter-in-render`, including twenty
`CurrencyInput.tsx:139`, the "adjust state during render" pattern React
documents.

What still fires deliberately, and why the arm is written as a *relation* rather
than a heuristic: `setUseAsync(Boolean(groups && !useAsync))` reads the slot
inside the value it writes, so every write re-arms the guard. Four dub
components, and the analyzer is right about all four.

**Not proved** (see [`limitations.md`](limitations.md)): a disjunctive guard
(`if (!prev || prev !== next)`), which would need *every* disjunct to settle and
is a different walk, and arithmetic on the compared value
(`setIndex(Math.max(0, plans.length - 1))`), which is a solver's job.

---

## 2026-09-03: a member is not the slot

*#90. Issue closed.*

`infinite-loop`'s self-churn arm reasoned at slot granularity on both sides: any
fresh write versions the whole object, any read counts as a read. An effect
touching *different members* of one object therefore closed an impossible cycle:

```tsx
// reads `.name`, writes `.slug`
useEffect(() => {
  setData((prev) => ({ ...prev, slug: slugify(prev.name) }));
}, [data.name, oAuthApp]);

// the guard reads `.leadId`, the write puts null there
} else if (!urlLeadId && sheet.leadId) {
  setSheet({ leadId: null, open: false });
}
```

**The claim, deps side.** React passes the current value to a functional updater,
so `prev => ({ ...prev, k: v })` puts `prev`'s value at every member the literal
does not name. A dep reading only those is `Object.is`-equal after the write.
This is the previous entry's move on the other side of the effect: the value
domain cannot say "`data.name` is unchanged", since the slot is a single abstract
value, but the two spellings can, because `prev` names the very value the dep
read.

**The claim, guard side.** A conjunct reading a member of the written slot is
settled by the value the write puts there, restricted truthy or falsy according
to the branch's polarity. The whole slot, by contrast, is a truthy object either
way.

**Four refusals keep it sound**, each because a member the walk does not see
could be the one that matters: the spread must be first and alone, and sourced
from the updater's parameter (`{...prev, slug, ...patch}` proves nothing); every
other key must be one a `FieldAccess` could ask for (a synthetic key *is* "a
member under an unknown name"); the dep must be a member chain, which excludes
the bare slot (`[data]` compares references); and the guard arm reads only a
literal, so the answer never depends on an environment where a local name would
be resolved.

**Corpus: 1,343 → 1,340 locations, 3 removed, none added**, all `infinite-loop`,
all in dub, all re-read at the source. Small for a real defect: the shape needs a
*functional* spread under a *member* dep, and outside dub this corpus writes
whole slots. Both arms also serve `setter-in-render`, which shares
`converges_once_written`.

**Not proved**: the multi-effect graph, where "A's write changes a dep of B" is a
property of a *pair* of edges; and the direct spread (`setData({...data, slug})`),
where `data` is the value captured at that render rather than the current one.

---

## 2026-09-03: a library contract is about members

*#94, the "values" half. The issue stays open for the "timing" half.*

A `SummaryValue` was flat, `Top | StableRef | UnstableRef`, so
`const { setValue } = useForm()` had no answer: the container was ⊤, every
destructured member was too, and each one was reported missing from the deps
array.

**The claim.** What these libraries publish is a *per-member* contract, not a
per-object one: `useForm()` guarantees that `setValue` is the same function on
every render and guarantees **nothing** about `formState`, which is a Proxy that
changes with the form. `SummaryValue::Shape { id, members }` carries exactly
that. The container stays ⊤, and a member absent from the list answers ⊤ as well,
which is what stops a member added to a library after the table was written from
inheriting a stability nobody promised.

The rest is #88's machinery: `bind_rhs` records the member map as a
`HeapValue::Obj`, exactly as it does for an object literal, so
`const { setValue } = useForm()`, which lowers to
`__obj = <marker>; setValue = __obj.setValue`, resolves through the heap like
`const { onClear } = bag`. Six lines in the interpreter, no new resolution path.
The `id` comes from the component's splice cursor (#134), so two `useForm()`
calls in one component are two objects.

Tables shipped: react-hook-form `useForm` and `useFormContext` (14 members),
Next's App Router `useRouter` (6), SWR's `mutate`. Deliberately absent:
`formState`, `data`, `error`.

**Corpus: 1,348 → 1,332 locations, 16 removed, none added**, all `missing-deps`,
all re-read at the source. Two of them, ai-chatbot's `login/page.tsx:26` and
`register/page.tsx:25`, carry the application's own comment:
`biome-ignore … router and updateSession are stable refs`.

The "whole object" removals (`form` in an effect that only calls `form.reset`)
are the same claim read by the longest stable prefix: a stale copy of `form`
holds the same `reset`. `missing-deps` asks about staleness, not about eslint
coverage.

The timing half follows below. It rests entirely on the provenance this change
makes available.

---

## 2026-09-03: a wrapper does not run its argument

*#94, the "timing" half.*

`<form onSubmit={form.handleSubmit(onSubmit)}>` left 34 `setter-in-render`
warnings of class ⊤. The walk saw an opaque call receiving a function that writes
state, and ⊤ includes the render pass: sound, and noisy.

**The claim.** `handleSubmit(cb)` **returns** a handler; it does not call `cb`.
That is a statement about *timing*, not about what a value is worth, so it
travels on its own `SummaryValue::StableWrapper` variant rather than on the
value, which is identical to `StableRef`. (The "a wrapper is not necessarily
stable" entry below has since split that variant into `Wrapper { stable }`:
stability was still welded to timing there, which is exactly what the previous
sentence claimed to avoid.)

**What makes it a contract rather than a guess.** ADR-034 §2 is explicit: moving
a row down from ⊤ to `Handler` is the only direction that can *lose* a finding,
so it is permitted only where timing is a contract. `handleSubmit` as a bare name
is a guess; `handleSubmit` as a member of a value returned by a `useForm()`
imported from `react-hook-form` is a contract. That provenance comes from the
previous entry: without the shapes, there was nothing to attach the claim to.

**The escape check is the other half of soundness.** The contract says the
wrapper will not run the callback; it says nothing about what the component does
with the handler it receives. `const submit = handleSubmit(cb); submit();` really
does run `cb` during render, and the ⊤ warning was right. A spelling is therefore
abandoned when a name bound to its call is itself called in the walked body, or
when the call is invoked in place (`handleSubmit(cb)()`). Restricting the check to
the walked body is not a shortcut: an effect calling `submit()` is a different
phase, which is precisely the question.

A destructured name resolves through the shared binding chase, whose certainty
bar carries the last case: a name bound more than once, two forms inlined into
one render body, resolves nothing, so the walk cannot say which object it is and
keeps ⊤. That is a refusal, not an oversight: an early version filtering `let`
bindings by hand would have claimed it wrongly.

**Corpus: 1,332 → 1,314 locations, 18 removed, none added.** Sixteen
`setter-in-render` and two `cross-setter-in-render`, all re-read at the source,
including `onSubmit={form.handleSubmit(onSubmit)}` in shadcn-admin and twenty's
`const submit = handleSubmit(…)`, where `submit` only goes into JSX.

**Not covered**, and left ⊤: the remaining 16. Nine are `@mantine/form`'s
`form.onSubmit(cb)` and `form.watch(path, cb)`, a different library with no
table; one is a local hook (`useTwoFactorAuthenticationForm`); the rest are the
twice-bound names above.

---

## 2026-09-03: a wrapper is not necessarily stable

*Follow-up to #94. The `@mantine/form` table, and the defect it revealed.*

The nine remaining `setter-in-render` findings from the previous entry were
`<form onSubmit={form.onSubmit(handleSubmit)}>`. The same shape as
react-hook-form's `handleSubmit(cb)`, so on the face of it one table line to add.
But mantine builds `onSubmit` like this:

```js
const onSubmit = (handleSubmit, handleValidationFailure) => (event) => { … };
```

A bare arrow in the hook's body: **a new function on every render.**
react-hook-form's `handleSubmit` is backed by a `useCallback`. Both wrap, only
one is stable.

**The defect.** `SummaryValue::StableWrapper` welded the two statements together,
while its own comment said timing "is a different statement about a different
thing and therefore cannot travel on the value". Writing the mantine entry with
that variant would have credited `form.onSubmit` with a stability nobody
promises, a false negative for any deps list containing it. The variant becomes
`Wrapper { stable: bool }`: the type now says what the comment already said.

**The table.** One line, `("onSubmit", Wrapper { stable: false })`. Nothing else
is listed: mantine's other members are `useCallback`s over deps that are not
stable either (`setValues` over `[onValuesChange]`, a user callback; `getValues`
over `[refValues.current]`), which is not a documented identity guarantee. An
unlisted member stays ⊤. The context hooks `createFormContext()` produces carry a
user-chosen name and therefore cannot be indexed by (package, name) at all.

**The proof that the split is real**: flipping `stable` to `true` turns exactly
the stability test red and no other.

**Corpus: 1,325 → 1,325, no change**, and that is the result rather than a
measurement failure. The table is correct: on an isolated file, and on the
`@docs/demos` subtree analysed alone, it removes the two `form.onSubmit(handleSubmit)`
sites and leaves exactly the three `form.watch(path, cb)` calls, which are
subscriptions and are deliberately not in the table.

What cancels it on the corpus: `test-repo/mantine` contains
`packages/@mantine/form/src/use-form.ts`. The analyzer resolves the import to
that real source and inlines it, and **an inlined source outranks a registry
summary**, rightly. That leaves
[#57](https://github.com/rboudrouss/reactant-analyzer/issues/57): the inlined
`onSubmit` returns a `FnLit` whose call site stays opaque, so the timing becomes
unknown again. A library whose source happens to sit in the analysed tree is thus
*less* well analysed than one consumed from `node_modules`, and no table can work
around that.

---

## 2026-09-03: a member read needs the converged heap

*[#135](https://github.com/rboudrouss/reactant-analyzer/issues/135). Found while
testing the previous entry: a false negative, so the forbidden direction.*

`always-unstable-deps` evaluated each dep against a fresh `Heap::new()`. A member
read only resolves through the heap, since `eval_field_access` follows the `Loc`
to a `HeapValue::Obj`, so with an empty seed it answered ⊤, and the rule reads ⊤
as silence, by construction and rightly.

```jsx
const obj = { f: () => {} };
useEffect(() => {}, [obj.f]);   // silent: `obj.f` is new every render
```

**This was not one rule.** `ConvergedEval::eval_in` took the heap as an argument,
on the theory that an empty seed and a converged seed were two legitimate
choices; four of its six callers took the empty one. `redundant-set-state` and
`unnecessary-rerender` guard on `is_stable()`, which ⊤ also fails: same family,
same missed findings.

**The fix is central, not per rule.** `eval_in` now seeds `self.heap.clone()` and
the parameter disappears: there is no longer a per-site choice to get wrong. A
site that really does evaluate against *empty* stores calls the `eval_in_stores`
primitive, and that bundle has no converged half to be inconsistent with.

**Why this was invisible until now.** The converged heap has only recently been
worth reading: #88 gave object literals a per-member map, and #134 made an
allocation site identify *one* allocation site. Before that, a member read could
answer from the wrong object.

`is_unstable_reference_only()` is a proof predicate: the change can only replace
⊤ with a proven value.

**Corpus: 1,325 → 1,325, no change.** Zero removed, zero added, verified on both
sides and not only on the total. The false negative is real (unit test plus
*gate-by-removal*: putting `Heap::new()` back turns exactly the member-dep test
red), it simply does not occur in these 14 repositories. That is a result, not a
failure: a soundness correction is justified by what it makes impossible, not by
what it moves today.

Runtime unchanged: 827s before, 807s after. The first version seeded the
converged heap *on every call* instead of once per component; the
[`Eval`](../src/rules/helpers/mod.rs) evaluator fixes that shape, but neither
version was measurable (dub: 75s / 74s / 71s).

---

## 2026-09-03: a slot's writers are read from the relation

*[#92](https://github.com/rboudrouss/reactant-analyzer/issues/92).*

`derived-state` and `redundant-set-state` both assert "nothing else writes this
slot", and both answered by walking two places: the render CFG, and the bodies of
the *other* effects. Those are not where the missed writers live: a handler bound
to a JSX prop, a `useCallback` body, a write inside the `.then()` the effect
launched.

**The relation that already knows** is on `AnalysisResult`. `slot_writers`
carries one region per row, `Render | Effect | Memo | Callback | Handler`, so the
three classes the issue names are *already* recorded there; the two rules simply
were not asking. `slot_written_outside` asks.

**The other half**, `setter_escapes`, existed too, but as a private column of
`SlotSeed`, computed only for slots seeded by a prop. Hence only
`frozen-initial-state` had it. Promoted next to the relation, with
`escaping_slots()` answering for any slot: once the setter has left the
component, the assertion is no longer ours to make.

Both facts are *may*-typed, and that is the right direction here: both consumers
use them to **withhold** a finding, so over-approximating costs a warning instead
of inventing one.

**Two mistakes along the way, kept here because they would recur.** Building the
alias set from the render CFG alone makes `const setter = setB` *inside* an
effect read as an escape instead of the alias chain it is. The walk's exemption
is `aliases.contains(var)`, so the set must be closed over every body, exactly as
`collect_slot_writers` does. And querying slot by slot re-walked each CFG once per
slot.

**Corpus: 1,325 → 1,314, 11 removed, none added**, all re-read at the source.
`derived-state` 3 → 0, `redundant-set-state` 12 → 4. These are the shapes the
issue predicted, plus two checked by hand: dub `main-nav.tsx:59`, where `setIsOpen`
is both written by a handler and passed into `SideNavContext.Provider`, and twenty
`use-app-preview-experience.ts:40`, which has four other writers.

---

## 2026-09-03: a tuple contract is indexed by position

*[#37](https://github.com/rboudrouss/reactant-analyzer/issues/37).*

`SummaryRegistry` returned only **one** value per hook, so a hook returning a
tuple could not expose a stable slot. jotai's
`useAtom(a) → [value, setValue]`: `missing-deps` reported `setValue`, which the
library documents as stable. In the corpus, excalidraw's author disabled exactly
that warning by hand (`app-jotai.ts:33`,
`// eslint-disable-next-line react-hooks/exhaustive-deps`).

**The cause was more general than the table.** The engine evaluated *every*
`IndexAccess` to ⊤, unconditionally. Array destructuring reduces to `__arr[0]`
and `__arr[1]`, so no per-position contract was reachable, whatever table was
written.

**The claim.** A **constant** index is a member read, and the heap answers it as
it answers a named member. This is the same per-member map #88 gave object
literals and #94 reuses for summaries. A non-constant index stays ⊤: what `xs[i]`
denotes is [#76](https://github.com/rboudrouss/reactant-analyzer/issues/76)'s
question, not this one.

Once that is done, the table is one line: `("1", StableRef)`. Position 0 is
deliberately absent, since changing is what an atom is for.

**The three negative directions are tested**: position 0 still fires, a position
the table does not name still fires, and an ordinary array element still fires.

**Corpus: 1,314 → 1,314, no change**, the third zero of the day and for a third
reason. The mechanism is proven (unit test, plus a synthetic repro of excalidraw's
exact shape that fires before and goes quiet after). What the corpus does not
reach:

- the only two sites destructuring `useAtom` are in `excalidraw/excalidraw-app`,
  37 files, 18 components analysed, **zero findings in total**, with the "no
  tsconfig `paths` found" warning: excalidraw's `@excalidraw/…` aliases are
  declared in the vite config, which the analyzer does not read
  ([#47](https://github.com/rboudrouss/reactant-analyzer/issues/47));
- novel only uses `useAtomValue` and `useSetAtom`, in components that produce
  nothing anyway.

The general half, the constant index, holds independently of jotai: it is the
only path by which a per-position contract can exist, and React's `useTransition`
and `useOptimistic` need the same mechanics
([#27](https://github.com/rboudrouss/reactant-analyzer/issues/27)). They
deliberately have **no** table here: I could not verify what React documents
about `startTransition`'s identity, and the `use-debounce` precedent says not to
write a claim you have not checked.

---
## Table correction (2026-09-03)

The figures recorded for the 2026-09-03 lines **do not reproduce**. They have
been re-measured and replaced. This section keeps what was recorded, what was
measured, and what remains unexplained: erasing the gap by rewriting the column
would do exactly what this log exists to prevent.

### What is established

**The analysis is deterministic.** Four runs of a frozen binary on one
repository, two on the whole corpus: bit-identical JSON files.

**Every measurement in this correction sees the same corpus**, 34,747 files and
14,016 components, across all eight runs. The chain is therefore consistent with
itself.

**Each step's binary was identified by behavioural probe**, not by timestamp: a
`handleSubmit` case for the timing half, a `const { setValue } = useForm()` case
for the values half. The first labelling, done by timestamp, was wrong, which
cost one useless measurement between two binaries that both already carried the
timing half.

### Recorded against measured

| step | recorded | measured |
|---|---:|---:|
| before #90 | 1,343 | **1,348** |
| #90, a member is not the slot | 1,340 (−3) | **1,345 (−3)**, delta correct |
| #134, the identity of an allocation site | 1,348 (**+8**: 6 removed, 14 added) | **1,334 (−11**: 26 removed, 15 added) |
| #94, values half | 1,332 (−16) | **1,326 (−8)** |
| #94, timing half | 1,314 (−18) | **1,325 (−1)** |

### What remains unexplained

The **deltas** are wrong, not only the endpoints. The natural hypothesis, that
removals were counted in JSON rows while the bounds were counted in locations,
was tested and **does not hold**: rows and locations give the same number (8 and
1). No variant of the counting key reproduces the recorded figures.

One clue without a conclusion: this document's header announced 34,730 files, 17
fewer than `files_analyzed` returns today. But `test-repo/` contains nothing
dated after 2026-09-02, so there is no basis for claiming the runs of that day
saw a different corpus.

### The pattern this draws

**#90's delta is exact** (−3 on both sides): at that point the measurement was
right, and the absolute column already carried a +5 gap inherited from
2026-09-02. What broke afterwards is the counting of **removals**:

- #134: 15 additions measured against 14 recorded, nearly right. But 26 removals
  measured against 6 recorded, and the entry names one family precisely
  (mantine's `$errors.<member>`). The explanation that fits: the family re-read
  by hand was taken for the total. The delta's sign flipped as a result.
- #94: the same shape, without the figures reconstructing themselves. The two
  halves are worth −9 locations together, not −34.

Counting what you re-read is not counting what changed. It is the same mistake as
the subtracted endpoint, at the other end of the calculation.

### What this does not call into question

**The direction of each correction.** Each is held by a *gated* regression test,
where disabling the fix turns exactly that test red, and every removal was
re-read at the source. What was wrong is the announced **size**, not the sign.
The two halves of #94 are worth −9 locations on this corpus, not −34.

### The 2026-09-02 lines

Not verifiable: no binary from that session survived. They are left as they are
and marked as such, rather than presented as checked.

### The rule that prevents a repeat

[`corpus-diff.py`](../scripts/corpus-diff.py) takes two runs, prints
`before / after / removed / added`, and exits with an error if the three do not
reconcile. An endpoint is counted; it is never deduced from a delta.

Since [#15](https://github.com/rboudrouss/reactant-analyzer/issues/15) the rule
is no longer an instruction: the figure lives in
[`docs/corpus-baseline.json`](corpus-baseline.json), produced by
[`corpus-baseline.py`](../scripts/corpus-baseline.py) and never typed, and the
`corpus` workflow replays it on every push to `main`. The file also carries a
digest of the content, since counters alone would let as many removals through as
additions, which is almost the shape the error had, and the corpus identity,
now pinned commit by commit in
[`setup-test-repo.sh`](../scripts/setup-test-repo.sh): a measurement taken over
different sources is not a comparable measurement, and the script refuses rather
than announce a delta.

## #4 and #5: a hook in a terminator, a concise body (2026-09-04)

`1,314 → 1,340` (+26: 10 removed, 36 added), measured with
[`corpus-diff.py`](../scripts/corpus-diff.py) over two complete runs. **The first
entry in this log with a positive balance**: both fixes make previously invisible
code visible, so they add more findings than they remove.

### Why one entry for two issues

#5 alone is a **soundness regression**, and it nearly shipped as one. `Candidate`
lost the arrow's `expression` flag, so a concise body lowered as a statement and
the function returned `unit`. Fixing that routes those bodies to
`Terminator::Return`, which is precisely #4's blind spot, since `extract_hooks`
only walks `block.stmts`. Measured on `const useLocal = (x) => useMystery(x)`:

| | before | after #5 alone |
|---|---|---|
| `analysis-limit` | emitted | **gone** |
| assurances | 4 withheld | **4 issued** |

An honest "I don't know" turned into four unearned guarantees: the forbidden
direction. #4 had to go first.

### The removals are false positives, re-read at the source

The most instructive one, twenty's `useStepBar.ts:45`, chains three fixes:

```ts
export const useAtomState = (state) => { return useAtom(state.atom); };   // #4
const setStep = useCallback(..., [setStepBarInternal]);
useEffect(() => { setStep(initialStep); }, []);   // ← `setStep` missing, we said
```

#4's hoist extracts `useAtom`, the jotai summary added for
[#37](https://github.com/rboudrouss/reactant-analyzer/issues/37) proves element 1
of the tuple is stable, therefore `setStep` is stable, therefore its absence from
the deps is correct. The finding was a genuine false positive.

### What the additions are not

**Untriaged.** 25 `missing-deps`, 9 `always-unstable-deps`, 2
`redundant-set-state`. Two were re-read and are true (commerce's `addCartItem`
really is a fresh arrow every render; react-hook-form's `setValue` really is
stable). The other 34 are not characterized. They lie in the direction the
project's invariant tolerates, not in the forbidden one, and they deserve a
triage pass like the `AUDIT` clusters.

### The known cost: 25 findings with no position

`Terminator::Return` carries no span where `Terminator::Branch` does. Nobody
needed one while no hook came out of a `return`. The `Stmt::Let` the hoist
synthesizes therefore inherits `span: None`, and findings anchored on it have
neither line nor column: 0 before, 25 after, out of 8,941 rows.

This is not a defect created here but a hole in the IR **revealed** here, and the
trade is favourable: those hooks were previously *absent*, not mislocated. Going
from "silently missing" to "reported without a line" moves in the right
direction. The clean fix is a span on `Terminator::Return`, 40 compilation sites,
tracked separately.

## #2: try/catch/finally is control flow (2026-09-04)

`1,340 → 1,344` (+4: **0 removed**, 4 added). No removals: the fix only opens code
that was never lowered, it closes none.

### Two defects in one branch

The lowering chained the three bodies in a straight line, each under
`!builder.is_terminated()`. A `try` whose body returns seals the block, so **the
`catch` and the `finally` were not lowered at all**, even though the branch's own
comment said it walked the `catch` "so hook extraction can find hooks inside catch
blocks". And when the guard did pass, the two bodies were sequenced
*unconditionally* after the `try` body, which made the all-paths reasoning believe
a write present only in the `catch` happens on every path.

A branch on an unknowable condition, since the body may or may not throw, says
both true things at once: the handler is on *a* path and not on all of them, and
both arms converge on the finalizer, which is on all of them.

| shape | before | after |
|---|---|---|
| `useEffect` in a `catch` after `return` | invisible, 1 hook, `✓` | `conditional-hook` (Error) + `infinite-loop` |
| `setN` in a `finally` after `return` | invisible, `✓` | `setter-in-render` |
| `setN` only in the `catch` | **Error**, claimed all-paths | **Warning** |
| `setN` with no `try` (control) | Error | Error |

### An accepted divergence

A `return` in the `try` body seals its block, so that path does not reach the
finalizer where JS would run it first. The finalizer stays reachable through the
throwing arm, so its hooks and writes are found; what is lost is its presence on
the returning path, which costs `must` strength (an Error demoted to a Warning)
and never a finding.

### The 4 additions belong to a pre-existing FP family

All four are `missing-deps` on `t`, twenty's Lingui macro, imported at module
level (`import { t } from '@lingui/core/macro'`) and therefore constant from one
render to the next: it has no business in a deps array.

This is not a family created here. Counted on both sides: **208 `missing-deps`
rows on `t` before the fix, 212 after.** The reads of `t` involved were inside
`catch` blocks that were never lowered; making them visible adds four instances
of a defect that already stood at 208.

Not reduced to a minimal repro. An unresolved import, a tagged call and a
cross-file lowering were each tried in isolation without triggering the finding.
Tracked separately.

## The corpus was pinned (2026-09-04): the column restarts

Until now [`setup-test-repo.sh`](../scripts/setup-test-repo.sh) cloned fourteen
repositories on their default branch, with no fixed commit. The corpus therefore
followed other people's pushes, and two measurements taken on two dates were not
over the same sources. It is now pinned commit by commit
([#15](https://github.com/rboudrouss/reactant-analyzer/issues/15)).

Re-cloning changed the content: **34,747 files → 40,164**. Every line above stays
correct *as measured*, on the corpus of the time; **none of them is comparable to
what follows**. The same binary `3a068ed` gives 1,344 on the old corpus and
**1,358** on the new one.

Do not extend the column across this break: that would be exactly the fault that
opened #15, one figure set next to another that does not measure the same thing.

## #141: a callee's free variable is not the caller's (2026-09-04)

`1,358 → 1,317` (−41: **41 removed, 0 added**), on the pinned corpus. No
additions: the fix only removes assertions, it produces none.

### The defect

The splice alpha-renamed everything the callee *bound*, its parameters and its
`let` bindings, and left its free variables intact, "so they still resolve in the
caller's scope", said the module comment. That is the inverse of lexical scoping:
in JavaScript, a function's free name resolves in **its** module scope, never in
the locals of whoever calls it.

Two witnesses, both real:

```ts
// twenty: an import, constant by construction
import { getFieldMetadataItemByIdOrThrow } from '@/object-metadata/utils/…';
const cb = useCallback(() => { … getFieldMetadataItemByIdOrThrow({…}) }, [store]);

// excalidraw: the same thing without an import, a module const
export const saveCaretPosition = (doc) => { … };          // line 17
const saveCaretPositionToState = useCallback(() => {
  const position = saveCaretPosition(ownerDocument);      // line 78
}, […]);
return { saveCaretPosition: saveCaretPositionToState };   // line 102 ← the trap
```

The second is the more telling: the hook **returns** its result under the name
`saveCaretPosition`, so a consumer writes
`const { saveCaretPosition } = useTextEditorFocus()` and binds that name. Inlining
then made the callee's module function be captured by the consumer's binding.
This is not a question of imports, it is any module binding, which is why the fix
targets free variables in general.

### Why only collisions

Only free names the caller also binds are renamed. A free name the caller does
not bind stays the callee's, and several are recognized *by name* downstream:
`fetch`, `console`, a sibling utility the registry resolves. Renaming all of them
would have traded this false positive for a false negative, which is the
forbidden direction.

### The implementation trap

The first fix changed nothing: at splice time, a callee's `useCallback` is already
a `HookEntry` with its own CFG, and only a marker remains in the body. The
capturable names are therefore not in `body_cfg` at all. The rename map must be
built over the body **and** over its hooks' sub-bodies, since those are what read
`t`.

## #139: the marker is where the tsconfig search starts, not where it stops (2026-09-04)

`1,317 → 1,317`, **identical digest**: not one location moved, and that is
expected. `reactant test-repo` names a tree with no build marker, hence
`ProjectKind::Plain`, hence **no aliases are loaded for any repository**, so the
whole corpus does not exercise this path. That is not a limitation of the fix but
a limitation of the instrument: fourteen projects do not run as one. The
measurement that counts is per project.

### The defect

Since `56ff872` the **marker** is found by walking up from the given path. The
**tsconfig**, however, was loaded from the marker's directory and no higher. A
monorepo keeping `vite.config.mts` in a sub-application and the `paths` map at
the root therefore lost all its aliases.

```
test-repo/excalidraw/
  tsconfig.json          ← "paths": { "@excalidraw/common": [...], … }
  packages/
  excalidraw-app/
    vite.config.mts      ← marker found here, search stopped here
```

### The measurement, per project

| run | before | after |
|---|---|---|
| `excalidraw-app` | `unresolved-aliases` blind spot | 38 unread imports, **named** |
| `excalidraw-app` + `packages` | `unresolved-aliases`, 22 findings | **no blind spot**, 22 findings |

The finding count does not move: the aliases resolve to code the relative imports
already reached. The gain is entirely in the honesty channel #9 opened: 490 files,
248 components, and for the first time a report that withholds nothing.

### The shape of the fix

`locate` and the tsconfig search now share one `nearest_ancestor`: same walk,
different predicate, as the issue asked. An ancestor declaring only a `baseUrl` is
set aside in favour of a further ancestor with real `paths`, which is the
discipline `load_tsconfig_paths` already applies to its `references` hop, but it
remains the answer when nothing better exists, otherwise the fix would remove bare
specifier resolution.

## #137: a directory is generated because the repository says so (2026-09-04)

`1,317 → 1,317` (**0 removed, 0 added**, identical digest): not one location
moved. What the fix moves is **coverage**, and it is the other column, the blind
spots, that says so.

### The defect

`EXCLUDED_DIRS` was four names filtered at any depth. That removes build output,
which is intended, but also build *tooling source* and any business directory
called `build` or `dist`. mantine keeps ten real `.ts` files in `scripts/build/`,
imported from files that were themselves analysed: nobody knew they existed until
#9's blind-spot list named them.

### The claim

A repository already declares what is generated, in the file git reads. One
precedence order, three sources: the configured list (`--exclude-dir` /
`excludeDirs`), otherwise the tree's `.gitignore` files, otherwise the hardcoded
names for a tree with none. `node_modules` and `.git` come before all three.

The explicit list **replaces** the two fallbacks instead of adding to them: that
is what "precedence" means, and a list that had quietly kept the hardcoded names
would have made `dist` unreachable.

### The measurement

| | files before | after | blind spots before | after |
|---|---|---|---|---|
| mantine | 4,784 | 4,798 (+14) | `unread-imports: 3` | **none** |
| chakra-ui | 2,666 | 2,671 (+5) | none | none |
| whole corpus | 35,453 | **35,541** (+88) | `unread-imports: 3` | **none** |

The three unread imports that opened #137 are read. The whole corpus now returns
a report that withholds nothing, which it had never done.

**The +88 are counted, not guessed.** A `find -maxdepth 4` gave 19 and was wrong
by 69: most of the batch is
`twenty/packages/twenty-sdk/src/cli/utilities/build`, **68 files of CLI source**
buried under `src/`. It is the clearest witness that the name says nothing, and a
restatement of the rule that opened #15: an endpoint is counted.

### What the fix also removes

Reading the `.gitignore` cuts both ways: it now excludes generated directories the
name list used to walk (`lib/`, `coverage/`, a codegen'd `src/generated/`). That
is the intended reading, since a directory git does not track is not this
repository's source, and it is never silent: anything an analysed file imports
lands by name in `unread-imports` and withholds the clean bill. `--exclude-dir` is
there to say something different. Nothing is lost on the corpus from that side:
the repositories are freshly cloned and no build output exists.

### The `.gitignore` reader

A separate module, deliberately conservative: anchoring, `!`, `*`, `**`, `?`,
`[…]`, deepest file wins, walking up bounded by the project root as git bounds
itself to its working tree. A pattern it cannot read filters **nothing**, since
over-filtering would be the forbidden direction.

### The wasm host had to follow

Its "superset" walk pre-applied `EXCLUDED_DIRS`, so under wasm the engine could
never see `scripts/build/` whatever the `.gitignore` said, and `MemFileSystem`
does not distinguish a skipped directory from an absent one, so the gap would have
been invisible instead of reported. The host now prunes only `node_modules`,
`.git` and `.next` (served as `prunedDirs`) and loads `.gitignore` and
`package.json` into the map; the engine decides. wasm-to-native parity is green.

## #138: following a narrowed run's imports, behind a flag (2026-09-04)

`1,317 → 1,317` by default (**identical digest**, the gate passes), and
`1,317 → 1,317` *also* with `--follow-imports`: on the whole corpus the flag
follows **0 files**. That is the expected result and worth writing down, since a
run that walks the whole project already contains its own imports. The problem
#138 describes exists only on a **narrowed** run.

### The decision

The issue left the choice open: always follow, behind a flag, or not at all.
Chosen: **behind a flag, off by default**, because naming a directory is a cheap
way to look at one pattern in one place, and that is what the user asked for.
Following imports contradicts precisely the intent of whoever narrowed.

Two questions were conflated in the issue, and separating them is what makes the
flag usable:

1. **Should analysing a named file read the bodies of its imports?** Yes, that is
   what makes the answer correct.
2. **Should findings in unnamed files be reported?** No, that is a question about
   the *report's* scope, not about soundness.

The flag answers yes to the first and no to the second. What the second leaves out
is **counted and named**, like a blind spot in reverse: nothing is unknown, it is
known and filtered on purpose, so it is stated.

### The measurement, on a narrowed run

`reactant check test-repo/excalidraw/excalidraw-app`:

| | files | findings | blind spots | withheld |
|---|---|---|---|---|
| default | 38 | 0 | `unread-imports` | n/a |
| `--follow-imports` | **440** (402 followed) | 0 | **none** | **19** |

excalidraw-app really has no findings, which is now proven rather than dodged, and
the flag announces 19 findings in the code it imports.

Following is more **precise** than naming the parent directory: `excalidraw-app`
plus `packages` gives 490 files and 22 findings, the closure gives 440 and 19. The
difference is what `packages/` contains and nobody imports.

### What it costs

The whole corpus: **826s** without, **822s** with, which is less than the noise
between two fourteen-minute runs. The pre-pass parsing 35,541 files to read their
imports weighs nothing next to the fixpoint.

The real cost is not the pre-pass, it is analysing files that were not being
analysed: 38 → 440 on excalidraw-app, more than ten times as many. **The flag is
not an optimization**: if you want the project, `reactant check src/` is the
better command. The flag buys a *narrow report over a correct analysis*, not
speed. It says so in `usage.md`.

### What it really changes

On the minimal shape (a hook returning a fresh object, a caller putting it in a
dep):

```
default            warn missing-deps          var:setN   ← the guess about an opaque hook
--follow-imports   warn always-unstable-deps  on `bag`   ← the real cause
```

The flag does not only add the true finding, it **removes a false one**: knowing
`useThing`'s body proves `setN` is a stable setter. Both findings are anchored in
the named file, which is the case that justified the work.

### The two hosts

The closure runs in the engine's filesystem view, and under wasm that view is the
map the host loaded. The host only loaded the named paths, and `MemFileSystem`
does not distinguish "never loaded" from "does not exist", so the closure would
have come back empty while announcing `followed 0`: wrong, and silent. The host
now widens its walk to the enclosing project when the flag is set. Output is
bit-identical between native and wasm.

Along the way, `--exclude-dir` (#137) did not work at all under wasm:
`npm/lib/index.js` builds its options object field by field and the field was
missing. That day's check compared *counts*, which matched on both sides.
Comparing the named files shows it immediately: comparing counts is not comparing
behaviour.
