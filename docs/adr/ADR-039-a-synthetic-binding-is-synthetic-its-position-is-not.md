# ADR-039: a synthetic binding is synthetic, its position is not

- **Status**: Accepted
- **Date**: 2026-09-02
- **Implements**: #131
- **Follows**: [ADR-035](ADR-035-await-phase-boundary.md) (the `await` hoist
  this gives a span to), [ADR-036](ADR-036-call-relation.md) §10 (the JSX span
  refresh, the first half of this), [ADR-024](ADR-024-inlined-hook-finding-attribution.md)
  (render the origin), [ADR-038](ADR-038-write-position-and-write-certainty.md)
  (the traversal that made the gap measurable)

## Context

**82 of 7,146 corpus findings (1.1%) carried no source location at all.** The
human report printed them with no position and the JSON had `line: null`, so
#129's location grouping could not collapse them either. The named example:
`commerce/components/carousel.tsx` reported a `JSON.stringify` the file does not
contain, at no line.

A row's witness is the span of the statement it sits in. Lowering and the splice
mint statements the source did not write — an `await` hoist, a ternary arm's
temp, a `||` operand's temp, a destructuring default, a spliced parameter
binding, a callee `Return` rewritten into an assignment — and every one of them
was minted with `span: None`. Each of those statements binds a **real source
expression**; only the binding is synthetic.

## Decision

### 1. Every synthetic statement takes the position of the expression it binds

Nine mint sites, one rule. `await fetch(…)` takes the awaited expression's
position (ADR-035 introduced this hoist and this is the half it missed); a
ternary arm and a `||`/`&&` operand take theirs; a binding pattern's every
sub-node — an array element, an object property, a rest element, a computed
key, a default value — takes its own, falling back to the pattern's.

### 2. What the source cannot name, the splice names by its call site

`Terminator::Return` carries no span — `Branch` does, `Return` never has — so a
callee `Return(e)` rewritten into `let bound = e` has no position of its own,
and neither does a `let param = arg` prefix. Both **execute at the call site**;
that is what inlining means, and it is the one position the splice knows. The
callee's own statements keep their own spans, so a finding inside the utility
still names the utility. Adding a span to the `Return` variant would name the
callee's `return` instead — better by a line, at the cost of a hundred
construction sites; not worth it for what it buys.

### 3. A body with no statements of its own inherits the one it was entered from

A concise-body arrow (`(i) => JSON.stringify(i)`) is a `Return` terminator and
nothing else. The setter walk passed `None` as the witness for every terminator
expression and for every nested body it descended into, discarding the position
it had just been standing on. The walk now carries a `witness` — the innermost
position it has already passed — and a statement, a terminator or a nested body
with none of its own inherits it. A `Branch` uses its own span first.

That ordering matters: inheritance is a *fallback*, never an override. §1 exists
because inheritance alone would have answered "the enclosing `useEffect(`" for a
ternary arm — a line, and the wrong one.

### 4. A finding takes the first position its witness chain names

The last residue was not the IR's fault at all: several rules thread a span into
a `Step` and never onto the diagnostic, so the row rendered with no line while
its own trace note carried one (`redundant-set-state` on mantine's `Popover`).
The chain is the finding's evidence and its first located step is where that
evidence starts, so the rule registry fills an empty `range` from it — once, for
every rule, rather than in each rule that happens to notice. A range a rule *did*
set is never overwritten.

## Consequences

Measured over 34,730 files:

| | spanless | total |
|---|---|---|
| native rules only, before | 9 (0.14%) | 6,340 |
| native rules only, after | **0** | 6,340 |
| with `packs/community/wave2.json`, before | 82 (1.15%) | 7,146 |
| with `packs/community/wave2.json`, after | **0** | 7,146 |

The **finding set is unchanged** in both — same rules, same components, same
messages, nothing added, nothing removed. Only positions moved, and only rows
that had none. Runtime is unchanged on every repository.

Two second-order effects:

- `commerce`'s six `JSON.stringify` rows now name `lib/shopify/index.ts:85`, the
  file that actually contains the call, and #129 collapses them to one line
  reading `[in 6 components]`. Attribution and grouping were both waiting on the
  span.
- `--fail-on` and the JSON schema are untouched: a position is not a finding.

## Not decided here

`Terminator::Return` still carries no span (§2). If a future relation needs the
callee's `return` position rather than its call site's, that is the change to
make, and it is mechanical.
