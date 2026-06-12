pub mod builtins;
pub mod summary;

pub use builtins::register_all;
pub use summary::{HookSummary, SummaryRegistry};

use std::collections::HashMap;

use crate::domains::Stability;

// ── Effect semantics ─────────────────────────────────────────────────────────

/// Describes when a hook with side-effects runs relative to renders.
#[derive(Debug, Clone, PartialEq)]
pub enum EffectSemantics {
    /// Standard useEffect behavior: runs after render based on deps array.
    Standard,
}

// ── Hook analysis result ─────────────────────────────────────────────────────

/// Abstract analysis result for a single hook invocation.
#[derive(Debug, Clone)]
pub struct HookResult {
    /// Abstract stability of the hook's primary return value.
    pub return_stability: Stability,
    /// Whether the hook introduces a piece of mutable state (useState/useReducer).
    pub creates_state: bool,
    /// Side-effect semantics, if any.
    pub effect_semantics: Option<EffectSemantics>,
}

// ── HookModel trait ──────────────────────────────────────────────────────────

/// Describes the abstract behavior of a React hook.
/// Adding a new hook: new struct + `impl HookModel` + `registry.register()`.
pub trait HookModel: Send + Sync {
    fn name(&self) -> &str;

    /// Compute the abstract result of calling this hook.
    ///
    /// - `args`  stability of each positional argument (e.g. initial state).
    /// - `deps`  stability of each dep in the deps array, or `None` if absent.
    fn analyze(&self, args: &[Stability], deps: Option<&[Stability]>) -> HookResult;
}

// ── Registry ─────────────────────────────────────────────────────────────────

pub struct Registry {
    models: HashMap<String, Box<dyn HookModel>>,
}

impl Registry {
    pub fn new() -> Self {
        Registry {
            models: HashMap::new(),
        }
    }

    /// Create a registry pre-populated with all builtin hook models.
    pub fn new_with_builtins() -> Self {
        let mut r = Self::new();
        register_all(&mut r);
        r
    }

    pub fn register(&mut self, model: Box<dyn HookModel>) {
        self.models.insert(model.name().to_string(), model);
    }

    /// Look up a hook by name. Returns `None` for unknown hooks.
    pub fn lookup(&self, name: &str) -> Option<&dyn HookModel> {
        self.models.get(name).map(|m| m.as_ref())
    }

    /// Convenience: analyze a hook by name, or return a conservative fallback.
    pub fn analyze(
        &self,
        name: &str,
        args: &[Stability],
        deps: Option<&[Stability]>,
    ) -> HookResult {
        self.lookup(name)
            .map(|m| m.analyze(args, deps))
            .unwrap_or(HookResult {
                return_stability: Stability::Unknown,
                creates_state: false,
                effect_semantics: None,
            })
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_unknown_returns_none() {
        let r = Registry::new();
        assert!(r.lookup("useUnknownHook").is_none());
    }

    #[test]
    fn analyze_unknown_returns_conservative() {
        let r = Registry::new();
        let res = r.analyze("useUnknown", &[], None);
        assert_eq!(res.return_stability, Stability::Unknown);
        assert!(!res.creates_state);
        assert!(res.effect_semantics.is_none());
    }

    #[test]
    fn register_and_lookup() {
        struct MyHook;
        impl HookModel for MyHook {
            fn name(&self) -> &str {
                "useMyHook"
            }
            fn analyze(&self, _: &[Stability], _: Option<&[Stability]>) -> HookResult {
                HookResult {
                    return_stability: Stability::Stable,
                    creates_state: false,
                    effect_semantics: None,
                }
            }
        }
        let mut r = Registry::new();
        r.register(Box::new(MyHook));
        assert!(r.lookup("useMyHook").is_some());
        assert_eq!(
            r.analyze("useMyHook", &[], None).return_stability,
            Stability::Stable
        );
    }

    #[test]
    fn builtins_registered() {
        let r = Registry::new_with_builtins();
        for name in [
            "useState",
            "useEffect",
            "useMemo",
            "useCallback",
            "useRef",
            "useContext",
            "useReducer",
        ] {
            assert!(r.lookup(name).is_some(), "missing builtin: {name}");
        }
    }
}
