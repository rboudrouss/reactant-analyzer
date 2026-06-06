# ADR-012: Inter-component analysis architecture

- **Status**: Accepted
- **Date**: 2026-06-04

## Context

The current analyzer (ADR-005, Phase 1) is strictly intra-component: each component is analyzed in isolation, props are `⊤`, setters passed as callbacks are not traced beyond the component boundary. The engine is powerful enough for local alarms; it now needs to be extended to detect cross-component patterns (unstable prop drilling, setter-via-prop, cross-boundary infinite loop, etc.).

The reference concrete semantics (ADR-001, React-tRace) already models inter-component via:
- `Set_clos { label, path }` — setter carrying its origin component's identity
- Tree Memory with per-component state stores
- Global scheduler (StepCheck/StepEffect) iterating until no component is dirty

## Decisions

### 1. Analysis direction: top-down inlining

Parent component analyzed first. When the parent encounters a `CompApp { name, props }`, the child component is inlined inline in the parent's analysis — its CFGs executed in the parent's context with the abstract props as argument. No bottom-up pre-computation of summaries.

Reason: natural extension of the existing fixpoint. Parametric summaries (bottom-up) require an additional abstraction layer (parametric analysis) without justified gain at the current stage.

### 2. Context sensitivity: memoization by abstract equality

Same component instantiated from several call sites: one analysis per unique abstract props value.

Cache key: `(Symbol, AbstractPropsValue)` — hit iff `current_props ⊑ cached_props AND cached_props ⊑ current_props` (lattice equality). No reuse of a less precise result for a more precise input (avoids precision loss if the first call was with `⊤`).

Cache bounded to N entries per component (configurable). Overflow: join of all entries → one degraded entry. Lookup on a degraded entry stays sound (over-approximation).

### 3. Domain extensibility

Cache hit via double leq (§2) — extensible: each domain implements `leq`, the cache mechanism is agnostic. Domains composed statically via generic `ProductDomain` (idiomatic Rust, no `dyn Hash`). Adding a domain = implementing `AbstractDomain` + `leq`, the cache follows.

### 4. Scope: multi-file via ComponentRegistry

Current phase: the user passes the files to analyze (`reactant src/**/*.tsx`). The engine pre-analyzes all files → builds `ComponentRegistry: Symbol → ComponentIR`. Phase 2 of the analysis accesses the registry to inline children. `CompApp` whose name is absent from the registry → props and result `⊤`, optional warning.

Import resolution (tracing `import { Button } from './Button'`) is out of scope for now — left to the user or the plugin system.

### 5. Bidirectional flow: descent + callbacks

The engine propagates abstract values in both directions:
- **Descending** (parent → child): props evaluated in the parent's abstract env, passed as the child's `param`.
- **Ascending** (child → parent): callbacks passed as props can be setters of the parent. When the child calls them, the engine propagates the update to the parent's state store.

### 6. Closure representation: free vars + shared store

```rust
// Extension of HeapValue
HeapValue::FnLit {
    params: Vec<Var>,
    body_cfg: Arc<CFG>,
    captured: HashMap<Symbol, StateValue>,  // free vars captured at creation
}
```

On call in the child: the body runs with `captured` as initial bindings + the call args. State mutations via setters write to the shared store (§7).

### 7. ComponentSetter: new abstract value

New variant in `StateValue` (and `HeapValue`):

```rust
StateValue::ComponentSetter {
    component: Symbol,   // "ParentForm"
    label: HookLabel,    // 0
}
```

Corresponds to `Set_clos { label, path }` of React-tRace. When a parent passes `setCount` as a prop → the child sees `ComponentSetter { component: "ParentForm", label: 0 }` in its abstract props. Calling this value → propagation to the `(ParentForm, 0)` slice of the shared store.

### 8. Global fixpoint: shared store, no new layer

```rust
// Extension of StateStore
pub struct SharedStateStore {
    entries: HashMap<(Symbol, HookLabel), StateValue>,
}
```

Passed as parameter to every component analysis. The parent's fixpoint stays structurally identical to today (render → effects → handlers → check convergence) — cross-component callbacks are mutations of the shared store, indistinguishable from local setters from the fixpoint's perspective. No separate program-level fixpoint layer.

Global convergence: the parent's fixpoint re-iterates if its slice of the shared store has changed following the inlining of a child. Same mechanism as the current local convergence.

### 9. Abstract props: HeapValue::AbstractObject

```rust
HeapValue::AbstractObject {
    fields: HashMap<Symbol, StateValue>,
}
```

When the parent inlines a child: evaluates each props field in its abstract env → creates an `AbstractObject` → allocates a synthetic `ExprId` → binds the child's `param` to this ExprId. `FieldAccess { obj: param, field: "onClick" }` in the child → heap lookup → `StateValue::ComponentSetter { .. }` or another abstract value.

### 10. Root detection: modular RootDetector

`RootDetector` is separated from the engine. The engine receives `roots: Vec<Symbol>`. Three strategies, combinable:

- **Heuristic (default)**: components not appearing in any `CompApp` of the program = roots.
- **`--all-roots` flag**: all components analyzed as roots (props `⊤` if not inlined from a parent). Exhaustive, sound.
- **`--entry Foo,Bar` flag**: explicit roots. Useful for the plugin system (Next.js: `pages/`, TanStack: route components detected by pattern).

### 11. Recursive components: MOPSA-style

Stack of components under analysis maintained in the analysis context. If `CompApp { name: X }` encountered during the analysis of `X` → cycle detected → result `⊤`, assumption `A_ignore_recursion(X)` recorded.

### 12. ProgramAnalysisResult

```rust
pub struct ProgramAnalysisResult {
    pub components: HashMap<Symbol, AnalysisResult>,
    pub shared_state: SharedStateStore,
    pub call_graph: ComponentCallGraph,
    // ComponentCallGraph: Symbol → Vec<CallSite>
    // CallSite { callee: Symbol, props: ExprId, location: SourceRange }
    pub recursive_components: HashSet<Symbol>,
    pub analysis_stats: AnalysisStats,
}
```

Rules receive `&ProgramAnalysisResult`. Intra-component rules access via `result.components[name]`. Cross-component rules traverse `call_graph` + `shared_state`. No backward compatibility maintained — existing rules are updated for the new signature.

## Accepted limits

- **Import resolution out of scope** — component absent from the registry → result `⊤` + `Info` `analysis-limit` emitted (`--info` to see). Next phase.
- **Deep recursion** → `⊤` (depth 1) + `Info` `analysis-limit` emitted on the calling component.
- **Dynamic** (`const Comp = cond ? A : B; <Comp />`) → `CompApp` not generated, not analyzed.
- **Plugin system** (Next.js, TanStack) → future extension of `RootDetector`.

## Consequences

- `src/engine/fixpoint.rs`: receives `SharedStateStore` as a parameter, propagates `ComponentSetter` calls.
- `src/domains/stores/`: new `SharedStateStore`.
- `src/ir/expr.rs`: `StateValue::ComponentSetter` + `HeapValue::AbstractObject` + `HeapValue::FnLit` extended with `captured`.
- `src/engine/`: new `ComponentRegistry`, `RootDetector`, `ProgramAnalysisResult`.
- `src/rules/`: signatures updated to `ProgramAnalysisResult`. Existing rules broken → to be migrated.
- `src/main.rs`: pre-analysis phase to build the registry before analysis.
