# ADR-040: component identity is an interned id, and the display name is a rendering

- **Status**: Accepted
- **Date**: 2026-09-05
- **Implements**: #7
- **Supersedes**: [ADR-038](ADR-038-write-position-and-write-certainty.md) §5,
  which chose the **display name** as that one spelling
- **Keeps**: [ADR-013](ADR-013-cross-file-analysis.md) §1's composite
  `(file, name)` registry key, now the thing an id is interned *from*

## Context

[ADR-038](ADR-038-write-position-and-write-certainty.md) §5 already found this
problem and fixed half of it: it made the analysis speak **one** spelling
instead of two, and picked the display name. That closed the self-ownership
failure it was chasing — a component reading as its own parent — and the
comparisons it named do agree today. What it could not fix is the spelling
itself, because the display name is content-dependent, so every table keyed by
it still re-keyed when an unrelated file changed. This ADR replaces the choice,
not the finding.

A component was named three ways at once, and #7 filed the rest of the
consequences.

1. `ComponentKey = (PathBuf, Symbol)`, the registry's key.
2. The **display name** minted by `ComponentRegistry::display_name`: the bare
   name when one file defines it, `Name@<file>` when several do.
3. The bare `Symbol` on `Expr::CompApp`, with no file at all.

After ADR-038 §5 the display name was not merely a third spelling, it was the
one the analysis *ran on*: `ProgramAnalysisResult.components`, `SharedStateStore`,
`Stability::Versioned`, `SetterVal::One`, the call graph, the stats sets and
`RuleCtx` were all keyed by it. And it is **content-dependent** by
construction — adding an unrelated file that happens to define a second
`Widget` renames the first one — so a distant, unrelated edit re-keyed every one
of those tables at once.

Two defects followed, and both ended in `✓ … no issues found`, the one line
[`limitations.md`](../limitations.md) promises is printed only when the run read
everything it was pointed at:

- `eval_comp_app` resolved a JSX callee through `get_by_name`, which answers
  with the first `(file, name)` key **in sort order**. A decoy `a/Widget.tsx`
  displaced the imported `./b/Widget`, and the Error that depended on the real
  body vanished.
- Discovery and `ImportResolver` spelled paths differently (`./b/W.tsx` against
  `b/W.tsx`), so every `(file, name)` lookup built from a resolved import missed
  on any `.`-rooted run — for hooks and contexts as much as components.

`ComponentRegistry::ir_for` is the fossil of the confusion: it handed back the
IR with `name` **overwritten** by the display name, because the analysis stamped
its own name onto everything it recorded and those stamps had to match the
results map.

## Decision

### 1. Identity is `ComponentId`, interned in a `ComponentTable`

Exactly what `FileId`/`FileTable` already are for paths (ADR-019), for the same
reason: identity has to be comparable, cheap, and independent of anything but
the thing it names. 4 bytes and `Copy`, so it sits inside a `BTreeSet` label or
a store key without the allocation a name costs.

The registry mints the table — the only place that knows the whole set of
components — and hands it to `ProgramAnalysisResult`, so rules and renderers
resolve against the same table the analysis was keyed by.

### 2. The display name is minted at render, and nothing compares it

`ComponentTable::display_name` owns the rule, because minting that string needs
the collision counts and nothing else does. It stays content-dependent, which is
the point: the `@file` suffix exists precisely to tell two same-named components
apart *in a report*. That is why no table may be keyed by it.

`RuleCtx` says both halves out loud: `component()` is the identity, a
comparison or a lookup wants it; `component_name()` is for a message and never
for a comparison.

### 3. A JSX callee carries what its own file proved

`Expr::CompApp` gains `origin: Option<Arc<CompOrigin>>` — the defining file plus
the name that file exports it under — the same fact
`HookEntry::Custom::resolved_file` already carried for a custom hook call. Both
halves are load-bearing: the file separates twenty components called `Form`, the
name resolves `import { Widget as Panel }`, which never resolved at all before.

`LowerCtx` is what made the threading affordable: the span table, the
allocation-site counter and the origin map travel as one value, so a nested
`FnLit` body's builder inherits all three and the next per-file fact needs no
pass of its own.

### 4. Resolution lives in one place, and refuses to guess

`ComponentRegistry::resolve_child` answers three ways: the proven origin; else
the name when only one file defines it; else **`Ambiguous`**. Root detection and
`SymbolGraph` read the same fact, so the three consumers can no longer disagree
about who a `<Widget/>` is.

`Ambiguous` makes the child unanalysable — like any callee outside the run — and
the parent carries an `analysis-limit` saying so. The old first-match was not
non-deterministic, it was **wrong**, and a wrong guess inlines a body the
program never renders at that site.

### 5. One spelling for every path

`lower_files_with` normalises every path it lowers, so registry keys and
resolver answers agree. This is upstream of component identity and fixes the
same class of miss for hooks, contexts and utilities.

## Consequences

`ir_for` stops rewriting the component's name: the comparisons agree by
construction now, so the IR keeps the name the source wrote. `Symbol` became an
unused import in ten files — the mechanical proof that the string no longer
circulates as identity.

Every remaining `display_name` call is a `format!`, the renderer, or `--entry`
resolution. None is a comparison and none is a key.

`analyze_component` keeps `ComponentId::SYNTHETIC`: a lone intra analysis has no
registry to intern it and compares its component to nothing.
`analyze_component_as` is for a caller that has already interned one, and
`ComponentTable::register` takes an id that already exists rather than minting
one the result's own labels would not match.

### Rejected: analysing every candidate of an ambiguous name

Sound and strictly more precise, and dropped on measurement.
Ambiguity is **1,347** references across the fourteen corpus repositories run
separately — eight of them have none — against 24,500 unknown-component
references already reported, so it adds 5.5% to a limitation that was there
before. `<Button/>` alone has 1,453 ambiguous sites when the corpus is analysed
as one tree; paying a full child analysis per candidate at each is the shape of
the O(C²) hang [#86](https://github.com/rboudrouss/reactant-analyzer/issues/86)
was. The residual concentrates in two monorepos importing through
`@workspace/*` aliases the resolver cannot map
([#48](https://github.com/rboudrouss/reactant-analyzer/issues/48)), which is the
fix that removes it.

### The measurement

The identity change is behaviour-preserving and was checked as such: **identical
digest** over 35,541 files, 1,348 locations, bit-for-bit. `873s` against `858s`
for the pre-#7 binary — one clean measurement each side, so: no regression
worth reporting, and no claim of a gain either.

The resolution change that precedes it is the one that moved the corpus, from
1,317 to 1,348 (29 removed, 60 added); see
[precision-log](../precision-log.md#7-a-jsx-callee-is-resolved-by-the-file-that-writes-it-2026-09-05).
