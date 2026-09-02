# The blind wish-list campaign (2026-09-02)

A measurement of the Tier-A vocabulary against **demand** rather than against
itself. Tracking issue: [#128](https://github.com/rboudrouss/reactant-analyzer/issues/128).

## Why it exists

`tests/catalogue.rs` proves that 21 of 22 catalogue entries are expressible.
That number answers *can the vocabulary express the rules we designed it for* —
a fair question, and a circular one. This campaign asks the other one: **can it
express what someone who has never seen it would ask for?**

## Method

Four agents were briefed as React staff engineers and told only that the target
is a semantic analyzer — data flow, values, reference identity, execution phase,
cross-component prop flow — and that pure AST patterns are out of scope. They
were **forbidden from reading any file in this repository**, so nothing they
proposed was shaped by what reactant happens to support. Each wrote 15 scenarios
in one domain, with a firing fixture, a deliberately hard near-miss that must
stay silent, and a mechanical list of the program facts a checker would need.

Four more agents then triaged those 60 against the shipped vocabulary, wrote
pack rules for what they claimed was expressible, and **ran each rule on its own
fixture pair**. A rule that could not be demonstrated firing was downgraded on
the spot; several were.

## Result

| verdict | count |
|---|---|
| NATIVE — a built-in rule already covers it | 16 |
| EXPRESSIBLE — a pack rule, demonstrated | 1 |
| PARTIAL — a useful proxy, with a named miss | 16 |
| INEXPRESSIBLE | 27 |

Both numbers are real and they measure different things. The content is in the
27, and in the four defects the exercise turned up on the way:
[#122](https://github.com/rboudrouss/reactant-analyzer/issues/122) (a component
returning `null` is not detected at all),
[#123](https://github.com/rboudrouss/reactant-analyzer/issues/123),
[#124](https://github.com/rboudrouss/reactant-analyzer/issues/124),
[#125](https://github.com/rboudrouss/reactant-analyzer/issues/125), plus fresh
evidence on [#4](https://github.com/rboudrouss/reactant-analyzer/issues/4) and
[#117](https://github.com/rboudrouss/reactant-analyzer/issues/117).

## The other half

[`AUDIT.md`](AUDIT.md) is the measurement side of the same exercise: what the
nineteen native rules actually do on 34,730 files, the severity split, and the
finding that 81% of the reported output is one source line repeated once per
consuming component ([#129](https://github.com/rboudrouss/reactant-analyzer/issues/129)).

## What is in here

| file | contents |
|---|---|
| `scenarios-*.md` | the 60 scenarios, verbatim as written blind |
| `triage-*.md` | per-scenario verdict, what was observed when the rule ran, and the gap list |

The pack rules the triage produced live in [`packs/community/`](../../packs/community/).
They are **not** first-party rules: several are proxies with known false
positives, recorded in their own `docs.why`. `tests/community_packs.rs` pins
that they all still load and validate, which is what keeps them honest as the
vocabulary moves.

## The re-triage

[`triage-2026-09-02-wave2.md`](triage-2026-09-02-wave2.md) re-measures the 60
against the vocabulary after #126 and #127. Seven scenarios flipped to
EXPRESSIBLE (1 → 8), so 24 of the 60 are now reachable by a rule somebody can
write. The rules that flipped them are in
[`packs/community/wave2.json`](../../packs/community/wave2.json), each run on
its own scenario's fixture pair.

## What has moved since

The triages are dated evidence and are **not** updated as the vocabulary
changes — a gap list rewritten after the fact stops being a measurement. Two
things they record have since been fixed, and are worth knowing when reading
them:

- The async triage had to drop `setInterval` from one rule's registrar list
  because `clearInterval(id)` could never pair with a listener binding. That is
  [#124](https://github.com/rboudrouss/reactant-analyzer/issues/124), fixed: the
  pairing fact now knows handle-valued and disposer-valued teardowns, and
  `{ once: true }`.
- The effects triage found that `registrations` and `writers` do not see through
  an `async` IIFE. That is [#117](https://github.com/rboudrouss/reactant-analyzer/issues/117),
  fixed: `await` splits the block and the walk descends an IIFE body.

## Reading the triages

A verdict of PARTIAL is the interesting one. It means the vocabulary could
express *something* in the neighbourhood — usually an absence verdict standing
in for a fact the scenario actually named — and the "Notes" line says exactly
what the substitution cost. That gap is the issue text.
