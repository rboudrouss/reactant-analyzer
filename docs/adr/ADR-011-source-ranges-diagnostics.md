# ADR-011: Source ranges + diagnostic notes

- **Status**: Accepted — implemented
- **Date**: 2026-06-03

## Context

Diagnostics displayed a hook label (`[hook:0]`) without indicating the source line or explaining the causal chain. In particular, the InfiniteLoop pattern detected via a handler (ADR-009 §5) produced:

```
⚠  infinite-loop  [hook:0]  — effect 1 unconditionally sets state 0 ...
```

without mentioning the `onClick` handler that had broadened the state range to make the effect's branch reachable.

Two distinct problems:
1. **Missing source location** — Oxc spans were available during parsing but discarded at the `lower_program(&ret.program)` point.
2. **Flat diagnostic** — `Diagnostic` only had a single `hook_label`; impossible to express "multiple participants" in a diagnostic.

## Decision

### 1. `SourceRange` — type and utilities

New module `src/ir/source_range.rs`: `SourceRange { line: u32, col: u32 }` (1-indexed line), `compute_line_starts(source: &str) -> Vec<u32>`, `offset_to_range(starts: &[u32], offset: u32) -> SourceRange`.

The `line_starts` table is precomputed once in `O(n)` in `analyze_file` and passed into the lowering. The offset → (line, col) conversion is a `binary_search` in `O(log n)`.

### 2. Spans in the IR

`Stmt` and `HookEntry` all have a `span: Option<SourceRange>` field. `Option` preserves compatibility with manual IR tests (engine/rules unit tests build the IR with `span: None`).

- **Stmts**: the `BlockBuilder` carries the `line_starts` table and exposes `span_at(offset: u32)`. Each `Statement::ExpressionStatement` and main `VariableDeclarator` gets its span at construction.
- **HookEntry**: `process_stmt` in `hook_extractor.rs` reads the span of the incoming `Stmt` and propagates it via `make_hook_entry(..., span)`. Effect and State as well as the other hooks inherit the span of their declaration statement.
- **Handler**: `lower_jsx_props` collects `prop_spans: HashMap<String, Option<SourceRange>>` for each `onX` prop during the lowering. `collect_handlers_in_expr` consumes this map via `prop_spans.get(name)` — span is `Some` in production, `None` only in tests that pass `&[]` as `line_starts`.

### 3. `Note` + `notes` on `Diagnostic`

```rust
pub struct Note {
    pub message: String,
    pub hook_label: Option<HookLabel>,
    pub range: Option<SourceRange>,
}

pub struct Diagnostic {
    // existing fields...
    pub range: Option<SourceRange>,  // location of the main finding
    pub notes: Vec<Note>,            // causal chain
}
```

Builder: `.with_range(r)`, `.with_note(msg, hook, range)`.

### 4. InfiniteLoop — scan handlers + build notes

The rule now scans `result.hooks` for `HookEntry::Handler` that call the same setter as the offending effect. Each handler found generates a `Note` with its label and capitalized event name.

### 5. CLI — note display

```
⚠  infinite-loop  [hook:0]  (line 54:2)  — effect 1 sets state 0 ...
   → handler `onClick` also calls setter — grows state 0 range  [hook:2]
```

## Consequences

- **Thread line_starts**: `lower_program`, `build_cfg`, `BlockBuilder` have a `line_starts: &[u32]` parameter. Tests that build the IR by hand (fixpoint, rules) pass `&[]` or `None`.
- **Mechanical sites**: `Stmt::ExprStmt(e)` → `ExprStmt(e, _)` in all patterns; `ExprStmt(e)` → `ExprStmt(e, None)` in test constructions. Same for `HookEntry` variants.
- **Handler span**: resolved — `lower_jsx_props` captures `prop_spans: HashMap<String, Option<SourceRange>>` for each `onX` prop before the Oxc AST is freed. `collect_handlers_in_expr` reads `prop_spans.get(name)` when building the `HookEntry::Handler`. Span is `Some` in production and `None` only in unit tests that pass `&[]` as `line_starts`.
- **Rules not updated**: only `InfiniteLoop` generates notes for now. The other rules (`missing-deps`, `stale-closure`, etc.) will benefit from the `range` on Diagnostic when they propagate their Effect.span.
