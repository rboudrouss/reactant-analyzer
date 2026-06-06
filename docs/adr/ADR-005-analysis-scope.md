# ADR-005: Intra-procedural scope + modular hook registry

- **Status**: Accepted
- **Date**: 2026-05-29

## Context

Custom hooks (user-defined `use*` functions) and library hooks (TanStack Query, React Router, etc.) call native hooks but are not components. Inter-procedural analysis (inlining custom hooks into the calling components) is costly to implement and not necessary to detect the first target bugs.

## Decision

### Phase 1 (current): intra-procedural

Each component and each custom hook is analyzed independently.
A call to an unrecognized hook → return `Unknown` for all values.
Bugs INSIDE custom hooks are detected when the hook is analyzed directly.

### Inter-component phase (implemented — ADR-012)

Top-down analysis with inlining of child components, `ComponentSetter` as abstract value, `SharedStateStore` for bidirectional propagation. See ADR-012 for the full architecture.

### Phase 2 (future): call-string-1 inlining

At each `useX(args)` call, substitute the hook body in the caller component's CFG with the substituted arguments. Depth = 1 (no recursive inlining).

### Hook Registry

Central mechanism for modeling hooks without inlining:

```rust
trait HookModel: Send + Sync {
    fn name(&self) -> &str;
    fn analyze(
        &self,
        args:  &[AVal],
        deps:  Option<&[AVal]>,
    ) -> HookResult;
}

struct HookResult {
    return_aval:      AVal,
    creates_state:    Option<HookLabel>,
    effect_semantics: Option<EffectSemantics>,
}
```

**Registry layers (decreasing priority):**

1. **Built-in hooks** (always active): `useState`, `useEffect`, `useMemo`, `useCallback`, `useRef`, `useContext`, `useReducer`.
2. **Library modules** (activated if dependency detected in `package.json`): `@tanstack/react-query`, `react-router`, etc.
3. **User config** (`reactant.toml` file at root): custom specs for in-house hooks.
4. **Fallback**: unrecognized hook → `HookResult { return_aval: Unknown, ... }` + optional warning.

**TanStack spec example:**
```
useQuery(queryKey, queryFn, opts) →
  data:      Stable if queryKey Stable, else Unknown
  isLoading: Stable (boolean)
  error:     Stable
  refetch:   Stable (TanStack guarantees identity)
```

## Consequences

- `src/registry/` contains the `HookModel` trait and the built-in implementations.
- `src/registry/tanstack.rs`, `src/registry/react_router.rs` etc. are optional modules.
- `src/registry/user_config.rs` parses `reactant.toml`.
- The lowering produces `HookCall { name, label, args, deps }` for all hooks — the registry is consulted at analysis time only.
- Inlining (phase 2) is added in `src/engine/` without modifying the registry or the domains.
