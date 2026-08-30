# Triage reference

Numbers like #32 below are issues on the analyzer's tracker, at
`https://github.com/rboudrouss/reactant-analyzer/issues/32`. Cite them when you
tell the user a warning is a known limit rather than a bug in their code.

## Urgency

Severity tells you how confident the analyzer is. Urgency is the runtime
consequence multiplied by reachability, meaning whether the component renders
often or sits on a rarely-mounted leaf.

| Runtime consequence | Rules | Urgency |
|---|---|---|
| The app hangs or burns CPU forever | `infinite-loop`, `cross-component-infinite-loop`, `setter-in-render`, `cross-setter-in-render` | now. The user sees a freeze |
| React throws, or hook state corrupts | `conditional-hook`, `server-component-hook` | now. It crashes at runtime |
| The UI shows stale or wrong data | `stale-closure`, `frozen-initial-state`, `state-mutation`, `missing-deps` | soon. Wrong behaviour, nothing in the console |
| Extra renders and wasted work | `derived-state`, `unnecessary-rerender`, `unstable-context-value`, `always-unstable-deps`, `lazy-init`, `redundant-set-state`, `missing-cleanup` | later. Measure before spending time |
| Not a defect | `analysis-limit`, `widening-info` | never a work item |

`missing-cleanup` jumps to "soon" when the effect subscribes, opens a socket or
starts a timer in a component that unmounts often. That is a leak, not
overhead.

## Known false positives

Check here before you conclude that a warning is new. The current list lives at
<https://github.com/rboudrouss/reactant-analyzer/blob/main/docs/limitations.md>,
section *Why reactant may warn wrongly*. A false positive never carries an
`error`.

| Shape in the code | Rule that fires wrongly | Issue |
|---|---|---|
| A callback left out of a mount-only or trigger-keyed effect on purpose | `missing-deps` | #32 |
| A conditionally re-bound closure, `let cb = a ? f : g` | `missing-deps` | #35 |
| The setter slot of a tuple-returning third-party hook such as jotai's `useAtom` | `missing-deps` | #37 |
| A module constant whose initializer is a call, `const X = f()`, so its value reads as unknown | deps-related rules | #34, #36 |
| A DOM-typed prop whose type comes from another file | `state-mutation` | #38 |
| Any 2-arg `on`/`addListener`, or 1-arg `subscribe`, treated as a long-lived registration | `stale-closure` | #42 |
| A dep declared as a field, `[x?.locale]`, with a truthiness test on the whole object | `missing-deps` | #40. Kept on purpose, the warning is sound and matches eslint |
| Two state slots that write each other but do converge | `infinite-loop`, `cross-component-infinite-loop` | #39 |

## What a clean report does not prove

A clean result is a proof only where `--info` printed `verified: ...`. If the
code has one of the shapes below, the absence of a finding tells you nothing.

- `useContext` has no model, so a context value reads as unknown (#28).
- Seven React hooks have no model either: `useActionState`, `useOptimistic`,
  `useTransition`, `useDeferredValue`, `useId`, `useSyncExternalStore`,
  `useFormStatus` (#27).
- Import aliases that live outside tsconfig `paths` stay opaque, which covers
  vite-config-only aliases, `jsconfig.json` and monorepo `@workspace/*`
  specifiers. The imported hook never gets analyzed (#47, #48).
- reactant never analyzes code inside `node_modules`, and re-export chains
  break past one level (#49).
- A utility function is inlined only when the call is a whole statement, so
  `if (util(x))` and `setX(util(y))` stay opaque (#52).
- A setter nested deeper than four closures, or reached through an index, a
  returned function, a spread or a class method (#45, #46, #76, #77).

Say this out loud when you report a clean run over code that uses one of them.

## JSON fields worth knowing (schema v2)

`file` against `component_file`. `file` is where `line` and `col` point, so a
finding inside an inlined custom hook gets reported in the hook's file, not in
the component's.

`component` is the display name reactant gives the component. The `@<file>`
suffix shows up only when two files define the same component name, and that
collision is itself a known defect (#7).

`notes[]` is the witness chain. `notes[].file` can differ from the diagnostic's
`file`. Each `kind` carries its own fields (`var`, `name`/`target`,
`callee`/`effect_class`, `slot`/`value_class`, `what`, `desc`, `event`,
`from`/`to`, `iteration`), and `message` holds the same prose `--trace` prints.

`line` counts from 1, `col` from 0. Both are `null` when the finding has no
source range.

`parse_errors[]` lists the files that never entered the analysis at all.

`summary.exit_code` mirrors the process exit code under the active `--fail-on`.

## Useful flags during triage

| Flag | Use |
|---|---|
| `--rule <name>` | Re-run one rule while you investigate it. It resurrects a rule the config set to `"off"` |
| `--trace` | Witness chains in human format, capped at 8 steps |
| `--show-clean` | List the components with no findings, hidden by default |
| `--all-roots` | Analyze every component as an entry point, with its props unknown. More findings, more noise |
| `--entry Foo,Bar` | Pin the root components. An ambiguous or misspelt name matches nothing and says nothing about it (#8) |
| `--verbose` | Fixpoint statistics on stderr. Worth it only when a finding looks impossible |
