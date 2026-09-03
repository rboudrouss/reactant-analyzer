# Triage of the two clusters nobody had looked at

Dated 2026-09-03. [`AUDIT.md`](AUDIT.md) cleared the two biggest rules —
`always-unstable-deps` and `lazy-init`, 42 % of the output — and left the small
clusters unexamined. Two of them had never been triaged at all:
`frozen-initial-state` (79 locations) and `unstable-context-value` (53).

Measured against a frozen post-#94 binary on `test-repo/` (14 repos), counted
with [`scripts/corpus-diff.py`](../../scripts/corpus-diff.py). Like the earlier
triage files, this one is dated evidence and is not rewritten later.

## The verdict

| cluster | locations | material FPs | outcome |
|---|---:|---:|---|
| `unstable-context-value` | 53 | **0** | clean |
| `frozen-initial-state` | 79 | **14** (18 %) | one named family → [#136](https://github.com/rboudrouss/reactant-analyzer/issues/136) |

## `unstable-context-value` — 53/53 true positives

47 of the 53 are literally `value={{ … }}` on a `Provider`: a fresh object
every render, the case React's own docs tell you to `useMemo`. True by
construction — there is no reading under which they are wrong.

The 6 that pass a *named* value are where an FP could have hidden. Each was
read to its binding:

| site | binding | verdict |
|---|---|---|
| `activity-log-context.tsx:33` | `const value = { … }` | fresh literal |
| `time-series-chart.tsx:169` | `const chartContext = { … }` | fresh literal |
| `RecordInlineCell.tsx:215` | `const RecordInlineCellContextValue = { … }` | fresh literal |
| `ThemeProvider.tsx:192,201` | `const contextValue = { theme, colorScheme }` | fresh literal |
| `ReactSpreadsheetImportContextProvider.tsx:21` | `value={values}`, a **prop** | see below |

The last one looked like the FP. `values` is a plain prop, and the rule is
correctly silent on a bare prop — a minimal repro confirms it, and so does a
module-level const passed through the same prop. What makes it fire is the real
caller:

```jsx
const mergedProps = { … };                                  // fresh every render
<ReactSpreadsheetImportContextProvider values={mergedProps}>
```

So the cross-component chase traced a fresh object through a prop into a
context value and was **right**. The stories that pass a module const are
irrelevant: one caller allocating is enough, and the may-side is the sound one.

Nothing to fix here. Recorded so the cluster is not re-triaged.

## `frozen-initial-state` — one FP family, 14 of 79

The family: state seeded from a prop, inside a component whose callers *always*
render it with a `key` derived from the same thing the seed is.

```jsx
// callee — flagged
const [formData, setFormData] = useState(() => { … action.settings.input … });

// caller — what the rule cannot see
<WorkflowEditActionCreateCalendarEvent key={stepId} action={…} />
```

The frozen state is the *point*: when `action` changes the key changes, React
discards the component, and the initializer runs again. React's documented
"reset state with a key". Both consumers of twenty's workflow-action components
do this at every call site — `WorkflowStepDetail.tsx` and
`WorkflowRunStepNodeDetail.tsx` render 22 `key={stepId}` each.

The `key` is at the call site and the `useState` is in the callee, usually one
more hop away in a shared hook, so a single-anchor intra-component rule cannot
reach the deciding fact — the structural limit of
[#68](https://github.com/rboudrouss/reactant-analyzer/issues/68). Written up as
[#136](https://github.com/rboudrouss/reactant-analyzer/issues/136).

**The other 65 are not this**, and are not proposed for change. They are the
genuinely ambiguous cases the Warning tier exists for: a settings form
(`useState(hubSpotSettings.leadTriggerEvent)`), a draft input
(`useState(value)`). The rule's message states a true fact about React
semantics and stops short of calling it a bug, which is the right posture.

## Noted in passing, not filed

`unnecessary-rerender` (11 locations) is 9 × the SSR mount-flag idiom. The rule
names the idiom in its own message and offers `useSyncExternalStore`, so it is
not making a false claim — it is style advice at Warning level, and whether it
belongs there is the project's call, not a precision bug. One inaccuracy worth
knowing: the SSR wording is applied to any `false → true` mount flip, including
`isVisible` in `ScrollEntrance.tsx:32`, which is an entrance animation and has
nothing to do with client detection.
