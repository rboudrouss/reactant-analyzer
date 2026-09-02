# ADR-041: what a dynamic index hides, and the two spellings of a closure

- **Status**: Accepted
- **Date**: 2026-09-02
- **Follows**: [ADR-040](ADR-040-the-longest-stable-prefix.md) (the prefix reading
  of a read path, which this extends to the collection side),
  [ADR-017](ADR-017-versioned-stability.md) (identity vs behavior)
- **Issue**: #89 (shapes 3 and 4)

## Context

`missing-deps` compares what a hook body reads against what its deps array
declares. Two of the four shapes #89 catalogued are not coverage bugs at all —
they are places where the rule never got to ask the question, because the
information was thrown away one layer earlier.

## §1 — the chain above a dynamic index

`extract_path` collapsed *any* member chain containing a computed access to its
bare root:

```js
const icon = useMemo(
  () => theme.snackBar[variant].color,
  [variant, theme.snackBar],          // ← names exactly the handle read
);
```

The read was recorded as whole `theme`, which nothing short of a `[theme]` dep
can cover, so the memo was reported against a deps array that already declares
everything it reads (twenty `SnackBar.tsx:163`).

**Decision.** A dynamic index hides the segments *below* it, not the chain
*above* it. `x.a[i].b` records `x.a` — the last named handle the read passes
through. This is ADR-040's claim on the other side of the comparison: the read
is fresh whenever `x.a` is fresh, exactly as a `[x.a]` dep already covers a
`x.a.b` read, so it costs nothing new in soundness. Segments below the index are
still lost, so a `[x.a.b]` dep does *not* cover `x.a[i].b` — the prefix match
falls the right way on its own.

On the **dep** side nothing changes: `[x.a[i]]` still declares nothing. A dep
pins the element, not the container, and crediting `x.a` there would be a false
negative.

## §2 — a `useCallback` is a closure

The rule's behavioral-stability check (ADR-017: what matters for a *stale*
capture is whether the values the function closes over can change, not whether
its identity does) resolved its subject through `fn_binding_in`, which only
recognises a bare `FnLit`. Hook extraction rewrites `useCallback(fn, deps)` to
`CallbackVal(label)` and lifts `fn` into the hook table, so every
callback-valued binding failed to resolve and was assumed stale-able whatever it
captured:

```js
const clearFieldError = useCallback((path) => { … }, [errorsState]);
const setFieldError   = useCallback((path, e) => { … clearFieldError(path) … }, [errorsState]);
//                                                  ^ reported
```

`clearFieldError` closes over a ref and a `[]`-deps setter — nothing that can
change — so the copy `setFieldError` holds behaves identically to the current
one (mantine `use-form-errors.ts:44`).

**Decision.** `useCallback` freezes its captures at deps-change time, but a
frozen copy of a value that cannot change *is* that value: the two spellings ask
the same behavioral question, and both must be answerable. `bindings.rs` grows
`callback_binding_in` beside `fn_binding_in`, both on one `sole_binding_in`
primitive (one binding, or the name's meaning is not syntactically certain), and
the capture set of a `useCallback` is its `EffectInfo::free_paths` — already
params-subtracted at construction.

## Consequences

- Corpus: 1,423 → 1,417 distinct locations (5,654 → 5,600 attributions), **6
  removed, 0 added**, every removal a verified false positive — four §1 (memos
  `PagedMemoList.tsx:163`, next-shadcn `kanban.tsx:720` — which carries an
  `eslint-disable` comment saying precisely this — `chat-area.tsx:49`, twenty
  `HalftoneStudio.tsx:256`), one §1 at `SnackBar.tsx:163`, one §2 at
  `use-form-errors.ts:44`.
- Both changes are shared-layer: `extract_path` feeds `missing-deps`,
  `stale-closure`, the mount helper and the seed scan alike, and all four read a
  used path the same way (a longer path is more coverable, never less).
- §2 does **not** reach a callback read through a container
  (`$errors.clearFieldError`): the path's root is an object, and resolving a
  member to the closure it names is a separate mechanism. 24 corpus locations
  still wait on it.
