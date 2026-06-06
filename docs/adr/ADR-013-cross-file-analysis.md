# ADR-013: Cross-file analysis — import resolution + symbol graph

- **Status**: Accepted — Phases 1-4 implemented (2026-06-06)
- **Date**: 2026-06-05

## Context

The current analyzer operates as a **flat-merge**: the user explicitly passes the files to analyze, all extracted components and hooks are merged into a single namespace keyed by `String`. This approach has three concrete limits:

1. **Name collision** — two `Page()` components in two different files (common Next.js pattern) overwrite each other in the registry → incorrect or silently wrong analysis.
2. **Manual discovery** — the user must list all files by hand or use a shell glob. A 50-file project requires a fragile glob.
3. **Cross-file utilities opaque** — `doOrNot(setX(...))` where `doOrNot` comes from `./helper.ts` is modeled as an opaque call → the analyzer over-approximates → possible FP (e.g. false infinite loop if the utility contains a `if (LAUNCH) ...` guard).

ADR-012 (inter-component) explicitly put import resolution out of scope. This ADR makes it the main target.

## Decisions

### 1. Module-scoped keying: `(PathBuf, String)` instead of `String`

All registries move to a composite key:

```rust
// Before
ComponentRegistry: HashMap<String, ComponentIR>
HookRegistry:      HashMap<String, HookIR>

// After
ComponentRegistry: HashMap<(PathBuf, String), ComponentIR>
HookRegistry:      HashMap<(PathBuf, String), HookIR>
FunctionRegistry:  HashMap<(PathBuf, String), FunctionIR>  // new (§5)
```

`ComponentIR` and `HookIR` receive a `file: PathBuf` field. Lookups in the engine move from name alone to `(file_of_caller, resolved_import_path, name)`.

### 2. Two separate traits for resolution

```rust
/// Discovery of the files to analyze from a root.
pub trait FileDiscoverer: Send + Sync {
    fn discover(&self, root: &Path) -> Vec<PathBuf>;
}

/// Resolution of a relative import specifier into an absolute path.
pub trait ImportResolver: Send + Sync {
    fn resolve(&self, from: &Path, specifier: &str) -> Option<PathBuf>;
}
```

Default implementations:

- **`DefaultFileDiscoverer`**: recursive glob `*.ts` / `*.tsx`, excludes `node_modules/`, `*.test.*`, `*.spec.*`, `*.d.ts`.
- **`DefaultImportResolver`**: tries `<specifier>.ts`, `<specifier>.tsx`, `<specifier>/index.ts`, `<specifier>/index.tsx`. Relative imports only (starts with `.`). Non-relative → `None` (handled by `SummaryRegistry` via `import_source`).

Both traits can be overridden by a Rust plugin (external crate implementing the traits). No config file in this phase — if a recurring pattern emerges (e.g. tsconfig `paths` alias), an ADR-014 can add `ImportResolverConfig`.

### 3. CLI: directory input

```
# Before: explicit list of files
reactant src/app/page.tsx src/components/Button.tsx

# After: root directory (B) or explicit files (A)
reactant src/           # automatic discovery via FileDiscoverer
reactant src/app/page.tsx src/components/Button.tsx  # preserved, no discovery
```

When a directory is passed: `FileDiscoverer::discover(root)` → list of files. When files are passed explicitly: used as-is, without additional discovery. `ImportResolver` is always active to resolve the imports in the provided files.

### 4. Symbol graph (not file graph)

The dependency graph is built at the **symbol** level (components, hooks, functions), not at the file level. Reason: circular imports between files are common in TypeScript but circular dependencies between React functions are nearly nonexistent (would violate the rules of hooks).

**Lightweight pre-pass** (without full lowering): for each file, scan `CallExpression` and `Identifier` to extract each function's direct dependencies:

```
SymbolNode = (PathBuf, String, SymbolKind)
SymbolKind = Component | Hook | Utility
SymbolGraph: DAG of SymbolNode → Vec<SymbolNode>
```

**Topo sort** on the symbol graph → analysis order. The leaves (functions with no dependencies in the registries) are analyzed first.

**Cycles**: if detected (e.g. two hooks calling each other), handled by the existing fixpoint (same mechanism as `ComponentCache` for component recursion).

### 5. FunctionIR and utility inlining

Utility functions whose source is available (in the discovered files) are lowered to `FunctionIR`:

```rust
pub struct FunctionIR {
    pub file: PathBuf,
    pub name: String,
    pub params: Vec<String>,
    pub body_cfg: CFG,
}
```

In the fixpoint, `Call { fn_: Var("doOrNot"), args }` is resolved via `FunctionRegistry`: if present, the body CFG is inlined at the call site (same mechanism as `expand_custom_hooks`). If absent (external utility, unresolved), current behavior: opaque call → `Top`.

This fixes the `doOrNot(setX(...))` FP: the `if (LAUNCH) return` guard is visible in the inlined body → the analyzer sees the branching → `setX` on dead path → no state update → correct convergence.

### 6. Eager analysis (not lazy)

The full graph is built before analysis starts:

```
1. FileDiscoverer → Vec<PathBuf>
2. Parse all files (fast — no lowering)
3. Symbol-extractor pre-pass → dependencies per symbol
4. Build SymbolGraph + topo sort
5. Lower + analyze in topo order
```

Reason: current batch model preserved, deterministic order, cycles handled before analysis. Full lowering (CFG + hooks extraction) is only done for files containing detected React components/hooks/utilities.

### 7. Unresolved imports

If `ImportResolver` returns `None` for a specifier:
- Expected symbol → `Top` in the registry
- `Info` `analysis-limit/unresolved-import` emitted (visible with `--info`)
- Analysis continues (FPs possible, FNs forbidden — same policy as the existing)

No fatal error: a real project may have unresolved imports (tsconfig aliases, monorepo links) that don't concern the analyzed components.

## Implementation phases

### Phase 1 — Foundation (~2 wk)
- `src/resolver/`: `FileDiscoverer` + `ImportResolver` traits + default implementations
- CLI: accepts directory, uses `FileDiscoverer`
- Eager multi-file parsing (discovered files parsed in batch)
- Flat-merge preserved — zero regression, unchanged behavior for a single file

### Phase 2 — Module-scoped keying (~2 wk)
- `file: PathBuf` on `ComponentIR` / `HookIR`
- Registries → `(PathBuf, String)` keys
- `ImportResolver` active to resolve the `import { X } from './file'` → `(resolved_path, X)`
- Symbol graph pre-pass + topo sort
- Fix Next.js `Page()` collision

### Phase 3 — Utility inlining (~1-2 wk)
- `FunctionIR` + `FunctionRegistry`
- Pre-pass extended to utilities (non-hook, non-component)
- Inlining in the fixpoint via `FunctionRegistry`

### Phase 4 — Plugin interface (future)
- Public exposure of the `FileDiscoverer` + `ImportResolver` traits
- Next.js plugin example: `FileDiscoverer` that finds every `page.tsx` in `app/`

## Accepted limits

Consolidated list in [docs/TODO.md](../TODO.md#adr-013--cross-file-analysis-limits). Summary:

- **tsconfig `paths` aliases** — not resolved by `DefaultImportResolver` (`@/components/Button` → `None`). Workaround: custom `ImportResolver` passed to `analyze_with_resolvers` (see [docs/plugins.md](../plugins.md)).
- **Chain re-exports** — `export { useMyQuery } from './hooks'` → traced one level if `./hooks` is in the discovered files; deep chains can be missed.
- **`node_modules` utilities/hooks/components** — never lowered (not in discovered files) → fallback `SummaryRegistry` for hooks, `⊤` for the rest.
- **Statement-level inlining only** — calls in expression position stay opaque. Typical cases: `if (util(x))`, `setX(util(y))`, `arr.map(util)`.
- **Utility recursion** — inlining at most once per CFG (recursion guard).
- **Nested closures** — only top-level functions are lowered.
- **`get_by_name` fallback** — when `resolved_file` is `None`, first match by path order kept → non-deterministic result on collision without import.

## Consequences

- `src/resolver/`: new module with traits + default implementations
- `src/engine/symbol_graph.rs`: new — symbol graph + topo sort
- `src/lowering/symbol_extractor.rs`: new — lightweight pre-pass
- `src/ir/component.rs`, `src/ir/hook_ir.rs`: `file: PathBuf` added
- `src/ir/function_ir.rs`: new
- `src/engine/component_registry.rs`, `src/engine/hook_registry.rs`: keys `(PathBuf, String)`
- `src/engine/fixpoint.rs`: utility inlining, module-scoped lookups
- `src/main.rs`: directory input, `FileDiscoverer`, eager pipeline
