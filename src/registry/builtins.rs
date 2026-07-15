use crate::domains::Stability;

use super::{EffectSemantics, HookModel, HookResult, Registry};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Join all dep stabilities. Empty deps = Stable (runs once). Absent deps = Unknown.
fn join_deps(deps: Option<&[Stability]>) -> Stability {
    match deps {
        None => Stability::Unknown,    // no deps array → runs every render
        Some([]) => Stability::Stable, // [] → runs once → Stable
        Some(ds) => ds.iter().fold(Stability::Bottom, |acc, d| acc.join(d)),
    }
}

fn state_result() -> HookResult {
    HookResult {
        return_stability: Stability::Unknown, // state value can change
        creates_state: true,
        effect_semantics: None,
    }
}

// ── useState ─────────────────────────────────────────────────────────────────

pub struct UseState;

impl HookModel for UseState {
    fn name(&self) -> &str {
        "useState"
    }

    fn analyze(&self, _args: &[Stability], _deps: Option<&[Stability]>) -> HookResult {
        state_result()
    }
}

// ── useReducer ────────────────────────────────────────────────────────────────

pub struct UseReducer;

impl HookModel for UseReducer {
    fn name(&self) -> &str {
        "useReducer"
    }

    fn analyze(&self, _args: &[Stability], _deps: Option<&[Stability]>) -> HookResult {
        state_result()
    }
}

// ── useEffect ─────────────────────────────────────────────────────────────────

pub struct UseEffect;

impl HookModel for UseEffect {
    fn name(&self) -> &str {
        "useEffect"
    }

    fn analyze(&self, _args: &[Stability], _deps: Option<&[Stability]>) -> HookResult {
        HookResult {
            return_stability: Stability::Stable, // returns void (unit)
            creates_state: false,
            effect_semantics: Some(EffectSemantics::Standard),
        }
    }
}

// ── useMemo ───────────────────────────────────────────────────────────────────

pub struct UseMemo;

impl HookModel for UseMemo {
    fn name(&self) -> &str {
        "useMemo"
    }

    fn analyze(&self, _args: &[Stability], deps: Option<&[Stability]>) -> HookResult {
        HookResult {
            return_stability: join_deps(deps),
            creates_state: false,
            effect_semantics: None,
        }
    }
}

// ── useCallback ───────────────────────────────────────────────────────────────

pub struct UseCallback;

impl HookModel for UseCallback {
    fn name(&self) -> &str {
        "useCallback"
    }

    fn analyze(&self, _args: &[Stability], deps: Option<&[Stability]>) -> HookResult {
        HookResult {
            return_stability: join_deps(deps),
            creates_state: false,
            effect_semantics: None,
        }
    }
}

// ── useRef ────────────────────────────────────────────────────────────────────

pub struct UseRef;

impl HookModel for UseRef {
    fn name(&self) -> &str {
        "useRef"
    }

    fn analyze(&self, _args: &[Stability], _deps: Option<&[Stability]>) -> HookResult {
        HookResult {
            return_stability: Stability::Stable, // ref object identity is stable
            creates_state: false,
            effect_semantics: None,
        }
    }
}

// ── useContext ────────────────────────────────────────────────────────────────

pub struct UseContext;

impl HookModel for UseContext {
    fn name(&self) -> &str {
        "useContext"
    }

    fn analyze(&self, _args: &[Stability], _deps: Option<&[Stability]>) -> HookResult {
        HookResult {
            return_stability: Stability::Unknown, // context value is externally controlled
            creates_state: false,
            effect_semantics: None,
        }
    }
}

// ── Registration ─────────────────────────────────────────────────────────────

/// Register all builtin hook models into a `Registry`.
pub fn register_all(registry: &mut Registry) {
    registry.register(Box::new(UseState));
    registry.register(Box::new(UseReducer));
    registry.register(Box::new(UseEffect));
    registry.register(Box::new(UseMemo));
    registry.register(Box::new(UseCallback));
    registry.register(Box::new(UseRef));
    registry.register(Box::new(UseContext));
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;

    fn registry() -> Registry {
        Registry::new_with_builtins()
    }

    // ── useState ──────────────────────────────────────────────────────────────

    #[test]
    fn use_state_creates_state() {
        let r = registry();
        let res = r.analyze("useState", &[Stability::Stable], None);
        assert!(res.creates_state);
        assert!(res.effect_semantics.is_none());
    }

    #[test]
    fn use_state_return_unknown() {
        let r = registry();
        let res = r.analyze("useState", &[Stability::Stable], None);
        assert_eq!(res.return_stability, Stability::Unknown);
    }

    // ── useReducer ────────────────────────────────────────────────────────────

    #[test]
    fn use_reducer_creates_state() {
        let r = registry();
        let res = r.analyze("useReducer", &[Stability::Unknown, Stability::Stable], None);
        assert!(res.creates_state);
        assert_eq!(res.return_stability, Stability::Unknown);
    }

    // ── useEffect ─────────────────────────────────────────────────────────────

    #[test]
    fn use_effect_has_effect_semantics() {
        let r = registry();
        let res = r.analyze("useEffect", &[Stability::PerRender], Some(&[]));
        assert!(!res.creates_state);
        assert_eq!(res.effect_semantics, Some(EffectSemantics::Standard));
        assert_eq!(res.return_stability, Stability::Stable);
    }

    // ── useMemo ───────────────────────────────────────────────────────────────

    #[test]
    fn use_memo_no_deps_array_unknown() {
        let r = registry();
        let res = r.analyze("useMemo", &[Stability::Stable], None);
        assert_eq!(res.return_stability, Stability::Unknown);
    }

    #[test]
    fn use_memo_empty_deps_stable() {
        let r = registry();
        let res = r.analyze("useMemo", &[Stability::Stable], Some(&[]));
        assert_eq!(res.return_stability, Stability::Stable);
    }

    #[test]
    fn use_memo_stable_deps_stable() {
        let r = registry();
        let res = r.analyze(
            "useMemo",
            &[],
            Some(&[Stability::Stable, Stability::Stable]),
        );
        assert_eq!(res.return_stability, Stability::Stable);
    }

    #[test]
    fn use_memo_unstable_dep_unstable() {
        let r = registry();
        let res = r.analyze(
            "useMemo",
            &[],
            Some(&[Stability::Stable, Stability::PerRender]),
        );
        assert_eq!(res.return_stability, Stability::Unknown); // join(Stable, Unstable) = Unknown
    }

    #[test]
    fn use_memo_all_unstable_deps_unstable() {
        let r = registry();
        let res = r.analyze("useMemo", &[], Some(&[Stability::PerRender]));
        assert_eq!(res.return_stability, Stability::PerRender);
    }

    // ── useCallback ───────────────────────────────────────────────────────────

    #[test]
    fn use_callback_empty_deps_stable() {
        let r = registry();
        let res = r.analyze("useCallback", &[Stability::PerRender], Some(&[]));
        assert_eq!(res.return_stability, Stability::Stable);
        assert!(!res.creates_state);
    }

    #[test]
    fn use_callback_unstable_dep_propagates() {
        let r = registry();
        let res = r.analyze("useCallback", &[], Some(&[Stability::PerRender]));
        assert_eq!(res.return_stability, Stability::PerRender);
    }

    // ── useRef ────────────────────────────────────────────────────────────────

    #[test]
    fn use_ref_always_stable() {
        let r = registry();
        for args in [vec![], vec![Stability::Unknown], vec![Stability::PerRender]] {
            let res = r.analyze("useRef", &args, None);
            assert_eq!(
                res.return_stability,
                Stability::Stable,
                "useRef should always be Stable"
            );
            assert!(!res.creates_state);
        }
    }

    // ── useContext ────────────────────────────────────────────────────────────

    #[test]
    fn use_context_always_unknown() {
        let r = registry();
        let res = r.analyze("useContext", &[Stability::Stable], None);
        assert_eq!(res.return_stability, Stability::Unknown);
        assert!(!res.creates_state);
    }

    // ── custom hook via registry.analyze fallback ─────────────────────────────

    #[test]
    fn custom_hook_not_registered_is_conservative() {
        let r = registry();
        let res = r.analyze("useCustomThing", &[Stability::Stable], Some(&[]));
        assert_eq!(res.return_stability, Stability::Unknown);
        assert!(!res.creates_state);
        assert!(res.effect_semantics.is_none());
    }

    // ── manual registration ───────────────────────────────────────────────────

    #[test]
    fn manual_hook_overrides_builtin() {
        struct AlwaysStable;
        impl HookModel for AlwaysStable {
            fn name(&self) -> &str {
                "useMemo"
            }
            fn analyze(&self, _: &[Stability], _: Option<&[Stability]>) -> HookResult {
                HookResult {
                    return_stability: Stability::Stable,
                    creates_state: false,
                    effect_semantics: None,
                }
            }
        }
        let mut r = Registry::new_with_builtins();
        r.register(Box::new(AlwaysStable));
        // Last registered wins (HashMap overwrite).
        let res = r.analyze("useMemo", &[], Some(&[Stability::PerRender]));
        assert_eq!(res.return_stability, Stability::Stable);
    }
}
