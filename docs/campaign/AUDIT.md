# Rule audit — 2026-09-02

What the rule set does on real code, measured at `451ed75` over the fourteen
repositories in `test-repo/`: **34,730 files, 14,249 components, zero parse
errors**, one run per repository.

Companion to [`README.md`](README.md), which covers the blind wish-list half of
the same exercise. Tracking issue: [#128](https://github.com/rboudrouss/reactant-analyzer/issues/128).

## What fires

6,322 findings. Three rules are 92% of them.

| rule | findings |
|---|---:|
| `missing-deps` | 4128 |
| `always-unstable-deps` | 1185 |
| `lazy-init` | 522 |
| `infinite-loop` | 128 |
| `frozen-initial-state` | 86 |
| `setter-in-render` | 72 |
| `unnecessary-rerender` | 56 |
| `unstable-context-value` | 53 |
| `conditional-hook` | 35 |
| `redundant-set-state` | 19 |
| `state-mutation` | 14 |
| `cross-component-infinite-loop` | 13 |
| `missing-cleanup` | 4 |
| `derived-state` | 3 |
| `cross-setter-in-render` | 3 |
| `server-component-hook` | 1 |
| `stale-closure` | **0** |

**The zero is healthy.** The canonical shape — a `setInterval` registered by a
mount-only effect whose callback reads and writes the same slot — fires
`stale-closure` at Error on a six-line fixture. Maintained codebases use
functional updaters and declare the dep, so the shape is genuinely rare.
Recorded so the zero is not mistaken for a defect later.

`cross-setter-in-render` was also at zero before
[#122](https://github.com/rboudrouss/reactant-analyzer/issues/122); its three
findings are all in components the analyzer did not previously know existed.

## Severity

**6,279 warning, 43 error (0.7%).** Seven of the nineteen rules can reach Error;
three actually do on the corpus.

| rule | errors | the proof |
|---|---:|---|
| `conditional-hook` | 35 | the call is provably inside a branch |
| `state-mutation` | 5 | the mutated object provably roots in a state slot |
| `frozen-initial-state` | 3 | the seeding prop provably moves and nothing re-syncs |

## The finding of the audit: 81% of the output is one line repeated

6,322 findings resolve to **1,170 distinct `(rule, file, line, col)`**.

| corpus | reported | distinct |
|---|---:|---:|
| mantine | 3165 | 311 |
| dub | 1796 | 384 |
| twenty | 1140 | 369 |
| chakra-ui | 122 | 21 |
| next-shadcn-dashboard-starter | 21 | 14 |
| excalidraw | 21 | 19 |
| ai-chatbot | 10 | 7 |
| the other seven | 47 | 45 |
| **total** | **6322** | **1170** |

Not a double-counting bug — each component is analysed exactly once. It is a
shared custom hook inlined into many consumers, each producing an honest
per-component row pointing at the hook's line. Grouped, the top of chakra-ui is:

```
87 reports, 87 distinct components — use-chart.ts:123
12 reports, 12 distinct components — use-chart.ts:123   (a second message, same line)
 4 reports,  4 distinct components — use-media-query.ts:45
```

99 of chakra-ui's 120 findings are one line of one shared hook. The repetition
is worst on the codebases that factor their hooks best.

**Every corpus number in this project's ADRs counts consumer attributions, not
defects.** Filed as [#129](https://github.com/rboudrouss/reactant-analyzer/issues/129).

## The two big rules were never FP-triaged. They hold up (2026-09-02)

`always-unstable-deps` (356 locations) and `lazy-init` (212) are 42% of the
output and had no issue against them, so the FP campaign that produced #86–#95
had never looked at them. Sampled and read against source:

**`always-unstable-deps` — accurate.** Every dep traced to its definition is a
genuinely fresh allocation: `useNavigateApp` returns a bare arrow (no
`useCallback`), `useToggleScrollWrapper` returns two, `useMetadataErrorHandler`
returns one, `searchParamsObj` is `Object.fromEntries(searchParams)`, and
excalidraw's `ColorInput` receives `onChange={(color) => …}` inline from its
parent. The rule also **requires proof**: an unknown prop function (⊤) used as
its own dep fires nothing, so the 356 are proven-fresh references, not
unknowns. twenty (217) and dub (72) carry the pattern as real technical debt.

**`lazy-init` — accurate, and concentrated in demos.** 146 of the 212 are
`dayjs().format(…)` or `toDateString(new Date())` in mantine `.story.tsx` /
`.demo.tsx` files. A bare literal initializer (`useState([1,2,3])`,
`useState({x:1})`) correctly fires nothing, and `Math.random()` is correctly
graded Info. The one gap is grading, not correctness: a pure builtin method on
a primitive (`"x".toUpperCase()`, `reactId.replace(…)`, `new Set()`) is a
Warning where the rule's own doc says cheap-and-pure should be Info — about six
locations, not worth a campaign.

**Conclusion.** The remaining false positives are in the small clusters, not the
big two: `infinite-loop` (40 locations, 39 of them the object-churn arm) and
`setter-in-render` (48, of which 32 are the ⊤ "no timing summary" class that
[#94](https://github.com/rboudrouss/reactant-analyzer/issues/94) covers and that
is sound by construction). Recorded so the sample is not re-run.

## What the engine admits it cannot see

Behind `--info`, measured one commit earlier at `0c45de8` (so the two large
figures are if anything understated):

| count | limit |
|---:|---|
| 28230 | hook not found in registry — pass its source file or add a `HookSummary` |
| 24070 | component not found in analysis registry |
| 356 | callback inlining reached the depth cap |
| 98 | `useRef` initialised by a direct call |
| 50 | object state recreated outside its deps, no cycle proven |
| 35 | seeded-and-never-resynced, downgraded by mount coupling (#95) |
| 18 | recursive component reference not followed |
| 10 | the deps argument is not a written array |

The two large ones are the same fact from two sides: `node_modules` is never
lowered (wontfix #51), so anything flowing through third-party code reads ⊤.
That is a deliberate perimeter — and its size is why the `SummaryRegistry` is
the extension point that matters most.
