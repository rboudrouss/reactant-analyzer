# ADR-019: Typed witness chains (explanatory traces)

- **Status**: Implemented
- **Date**: 2026-07-17
- **Supersedes**: [ADR-011](ADR-011-source-ranges-diagnostics.md) §Note (free-text notes) and its file-identity limitation
- **Context**: [ADR-018](ADR-018-effect-cycle-graph.md) (churn graph provides cycle edges), ADR-013 (cross-file inlining), the `--trace` CLI flag

## Context

`--trace` prints the `notes` a rule chose to attach — free-text strings,
hand-built per rule, only `infinite-loop` emits any. Three structural defects:

1. **No file identity.** `SourceRange { line, col }` (ADR-011) carries no
   file. After cross-file inlining (ADR-013) a note's `line:col` may point
   into the inlined hook's *source* file while the diagnostic names the
   component's file — documented as a JSON caveat, actually a wrong-file bug.
2. **Free text.** Rules format prose inline; wording drifts between rules,
   JSON consumers can't interpret the causal chain, and nothing forces a note
   to point at evidence.
3. **Provenance discarded.** The fixpoint knows *why* it widened (which
   write, which iteration); the inliner knows *what* it spliced from *where*.
   Both throw that knowledge away, so rules re-derive causality after the
   fact — or can't (`lazy-init` cannot say "this init calls `f`, imported
   from ./util, whose body calls `fetch`").

Goal: every diagnostic can carry a structured **witness chain** — typed steps
reconstructing why it fired, each step anchored to a `(file, line, col)`
site — produced from shared infrastructure, not per-rule string building.

## Decision

Four pillars. No free-text escape hatch: a rule needing a new kind of
justification extends the closed `Step` vocabulary.

### 1. File identity: `FileId` interned in a `FileTable`

```rust
pub struct FileId(u32);                         // Copy, 4 bytes
pub struct SourceRange { pub file: FileId, pub line: u32, pub col: u32 }  // stays Copy
pub struct FileTable { paths: Vec<PathBuf> }    // intern + resolve
```

- `offset_to_range(starts, offset, file)` is the single span constructor —
  the compiler drives the migration of every lowering site.
- The `FileTable` is built during lowering (one entry per parsed file),
  travels `LoweredProgram → ProgramAnalysisResult → CheckReport`; renderers
  resolve `FileId → &Path` through it.
- Spans inside spliced CFGs were created when *their* file was lowered, so
  they carry the right `FileId` with **zero** logic in `splice_one_call` —
  the ADR-011/ADR-013 wrong-file caveat disappears rather than being patched.
- Manual-IR tests keep `span: None`; `FileTable::for_tests()` provides a
  single-file table where needed.

### 2. Engine-side provenance: record at the point of knowledge

The engine records small provenance events where it already computes the
fact; rules query instead of re-deriving.

```rust
// AnalysisResult gains:
pub widen_trace: HashMap<HookLabel, WidenEvent>,
    // WidenEvent { iteration: usize, writers: Vec<HookLabel> }
    // (write spans are derived from `effect_info[writer].span` at render time)
pub inline_origins: Vec<InlineOrigin>,
    // InlineOrigin { name: String, from: PathBuf, kind: InlineKind /* Hook | Utility */ }
```

- `widen_trace`: one insert at the existing widening point in the fixpoint —
  replaces the bare `widened_labels` set (kept as a derived view or dropped).
- `inline_origins`: stamped by custom-hook expansion and
  `splice_one_call`; feeds `Step::Resolve` ("inlined from ./hooks/media.ts").
- The churn graph (ADR-018) already carries `write_span` per edge — exposed
  as-is for `Step::CycleEdge`.

Explicitly rejected: a full provenance domain (tagging every abstract value
with its origin, joined at joins). Sound and maximal, but touches every
transfer function and multiplies memory for witness depth no rule needs.
The event log covers the semantic points that matter (writes, widening,
inlining, freshness) and can grow if a future rule needs more.

### 3. Typed steps: `Note` becomes structured

```rust
pub struct Note {
    pub step: Step,
    pub hook_label: Option<HookLabel>,
    pub range: Option<SourceRange>,   // carries FileId → cross-file native
}

pub enum Step {
    /// Value flowed through this binding: `const x = f(props)`.
    Binding { var: Var },
    /// Name resolution: `f` → import / local fn / state setter / unknown.
    Resolve { name: String, target: ResolveTarget },
    /// A call and its effect class (setter / effectful / pure-cheap / unknown).
    Call { callee: String, class: EffectClass },
    /// State write, with the written value's class.
    Write { slot: HookLabel, value: ValueClass },  // Fresh | SameAsCurrent | Unknown
    /// Read of a reactive value (deps rules: undeclared/declared read site).
    Read { what: String },
    /// The site is guarded by this branch (conditional-hook, guards).
    Branch { desc: String },
    /// Escapes into an event handler that re-triggers the cycle.
    Handler { event: String, slot: HookLabel },
    /// Churn-graph edge: the cycle continues through this write.
    CycleEdge { from: String, to: String },
    /// Fixpoint evidence: the slot's abstract value grew until widening.
    Widen { slot: HookLabel, iteration: u32 },
}
```

- **No `Text(String)` variant.** A free-text escape hatch would erode the
  vocabulary back to prose within months. The enum is small and closed on
  purpose; extending it is the sanctioned path.
- **Rendering is centralized**: `render_step(&Step, ctx) -> String` in
  `rules/witness.rs` is the only place witness prose is produced. Rules
  never format notes. `ctx` provides the state-slot naming
  (`state_slot_name`) so messages keep printing `` `count` ``, never a bare
  post-inlining label.
- **JSON** (additive, schema stays v1): each note gains `kind` (the variant,
  kebab-case) plus its structured fields, and `file` becomes correct;
  `message` remains the rendered prose for human-oriented consumers.

### 4. Shared witness library: `rules/witness.rs`

Producers are mutualized; a rule only picks entry points:

```rust
/// Chase a value backwards through bindings and calls; resolve callees via
/// the FunctionRegistry (one resolution level).
pub fn chase_value(cfg: &CFG, expr: &Expr, registry: &FunctionRegistry, file: FileId) -> Vec<Note>;
/// Slot history from engine provenance: writes, then the widen event.
pub fn slot_history(result: &AnalysisResult, slot: HookLabel) -> Vec<Note>;
/// Resolve a callee name → Resolve step + effect scan of the resolved body.
pub fn resolve_and_classify(registry: &FunctionRegistry, file: FileId, name: &str) -> Vec<Note>;
```

`resolve_and_classify` absorbs `lazy-init`'s name-based effect classification
(the `EFFECTFUL` / pure-builtin sets move to `witness.rs`) and adds a scan of
the resolved body. Soundness guard unchanged: the body scan may only *refine*
a classification (Unknown → named Effectful) — it never downgrades severity,
so no new false negatives.

### Bounds

- Callee resolution: **one level** (no transitive `f → g → h` chase) —
  consistent with `max_inline_depth` philosophy.
- Witness length: capped at 8 steps per diagnostic; the renderer summarizes
  the tail (`… n more step(s)`).

## Rule → witness mapping

| Rule | Chain |
|------|-------|
| `infinite-loop` | `Write{Fresh}` → `Handler`/`CycleEdge`* → `Widen` |
| `lazy-init` | `Binding`* → `Resolve` → `Call{class}` |
| `always-unstable-deps` | `Binding` → freshness source (`Call`/`Resolve`) — also carries cross-component blame |
| `derived-state` | `Read` (mirrored slot) → `Write` |
| `missing-deps` | `Read` (undeclared read site) |
| `conditional-hook` | `Branch` (the guard) |
| `unnecessary-rerender` | `Write` (mount effect write site) |
| `redundant-set-state` | `Write{SameAsCurrent}` |
| `setter-in-render` | `Call{Setter}` (render call site) |

\* zero or more.

## Consequences

- `SourceRange` gains a field but stays `Copy`; every
  `SourceRange { line, col }` literal (tests included) is updated by the
  compiler. Renderers and `output_json` need the `FileTable` — carried by
  `CheckReport`.
- ADR-011's `Note { message, … }` is deleted, not deprecated: the 4
  existing `infinite-loop` notes are converted to typed steps in the same
  change. `Diagnostic::with_note(msg, …)` is replaced by
  `with_step(step, …)`.
- The `usage.md` JSON caveat about wrong-file note positions is removed;
  `notes[].file` is documented instead.
- Implementation order: (1) FileId/FileTable, (2) Step model + renderer,
  (3) engine provenance, (4) witness library, (5) per-rule adoption,
  (6) docs/tests. Phases 1–2 are independently shippable.
