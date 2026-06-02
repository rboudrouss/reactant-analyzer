use crate::{
    domains::{
        AbstractDomain,
        impls::{
            bool_val::BoolVal, interval::Interval, stability::Stability, state_value::StateValue,
            str_const::StrConst,
        },
        stores::StateStore,
    },
    ir::{
        expr::{Expr, Prim},
        hooks::HookEntry,
        types::HookLabel,
    },
};
use std::collections::HashMap;

// ── StateType ─────────────────────────────────────────────────────────────────

/// Inferred type for a single useState label, derived from its init expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateType {
    Number,
    Boolean,
    Str,
    Reference,
    /// Fallback: null, undefined, or statically unknown type.
    Unknown,
}

/// Infer the domain type of a useState label from its init expression.
pub fn infer_state_type(init: &Expr) -> StateType {
    match init {
        Expr::Lit(Prim::Int(_) | Prim::Float(_)) => StateType::Number,
        Expr::Lit(Prim::Bool(_)) => StateType::Boolean,
        Expr::Lit(Prim::String(_)) => StateType::Str,
        Expr::ObjectLit(_) | Expr::ArrayLit(_) | Expr::FnLit { .. } => StateType::Reference,
        _ => StateType::Unknown, // null, undefined, complex expressions
    }
}

// ── TypedStateStore ───────────────────────────────────────────────────────────

/// Per-label typed state store — ADR-008 Option B.
///
/// Each useState label is associated with a `StateType` inferred from its init.
/// Values for typed labels are stored in specialised sub-stores, enabling
/// per-domain widening (e.g. interval widening for numeric labels).
///
/// The `Transfer` trait stays unchanged: call `to_untyped()` to get a
/// `StateStore<StateValue>` compatible with existing Transfer impls, then
/// reconstruct via `from_untyped()` after each analysis pass.
///
/// ## string | number
///
/// Labels whose init is `null`, `undefined`, or a dynamic expression fall into
/// `Unknown` and use the `unknown_store: StateStore<StateValue>` fallback.
/// This is identical to the pre-Option-B behaviour for those labels; no
/// precision is lost. Typed labels (Number/Boolean/Str/Reference) benefit from
/// per-domain join/widen precision.
pub struct TypedStateStore {
    type_map: HashMap<HookLabel, StateType>,
    number_store: StateStore<Interval>,
    bool_store: StateStore<BoolVal>,
    str_store: StateStore<StrConst>,
    ref_store: StateStore<Stability>,
    unknown_store: StateStore<StateValue>,
}

impl TypedStateStore {
    fn empty_with_types(type_map: HashMap<HookLabel, StateType>) -> Self {
        TypedStateStore {
            type_map,
            number_store: StateStore::bottom(),
            bool_store: StateStore::bottom(),
            str_store: StateStore::bottom(),
            ref_store: StateStore::bottom(),
            unknown_store: StateStore::bottom(),
        }
    }

    /// Build from a component's hook list, inferring types from init expressions.
    pub fn from_component(hooks: &[HookEntry]) -> Self {
        let mut type_map = HashMap::new();
        for hook in hooks {
            if let HookEntry::State { label, init } = hook {
                type_map.insert(*label, infer_state_type(init));
            }
        }
        Self::empty_with_types(type_map)
    }

    /// Read the abstract value for a label as `StateValue`.
    ///
    /// For typed labels, joins the typed sub-store value with the unknown_store
    /// value. This handles type-mismatch cases (e.g., a Number label that
    /// received a Reference value via a cross-type setState call), ensuring the
    /// fallback value is never silently dropped.
    pub fn get(&self, label: HookLabel) -> StateValue {
        // Value from the typed sub-store (Bottom if label not present there).
        let typed_val = match self.type_map.get(&label) {
            Some(StateType::Number) => {
                let i = self.number_store.get(label);
                if i.is_bottom() {
                    StateValue::Bottom
                } else {
                    StateValue::Number(i)
                }
            }
            Some(StateType::Boolean) => {
                let b = self.bool_store.get(label);
                if b.is_bottom() {
                    StateValue::Bottom
                } else {
                    StateValue::Boolean(b)
                }
            }
            Some(StateType::Str) => match self.str_store.get(label) {
                StrConst::Bottom => StateValue::Bottom,
                StrConst::Top => StateValue::Str,
                StrConst::Set(set) => StateValue::StrConst(set),
            },
            Some(StateType::Reference) => {
                let s = self.ref_store.get(label);
                if s.is_bottom() {
                    StateValue::Bottom
                } else {
                    StateValue::Reference(s)
                }
            }
            // Unknown labels: only in unknown_store.
            _ => return self.unknown_store.get(label),
        };
        // Join with unknown_store in case of a type-mismatch update.
        let unknown_val = self.unknown_store.get(label);
        typed_val.join(&unknown_val)
    }

    /// Write an abstract value for a label, dispatching to the right sub-store.
    pub fn update(&mut self, label: HookLabel, val: StateValue) {
        let state_type = self.type_map.get(&label).copied();
        match (state_type, &val) {
            (Some(StateType::Number), StateValue::Number(i)) => {
                self.number_store.update(label, *i);
            }
            (Some(StateType::Boolean), StateValue::Boolean(b)) => {
                self.bool_store.update(label, *b);
            }
            (Some(StateType::Str), StateValue::StrConst(set)) => {
                self.str_store.update(label, StrConst::Set(set.clone()));
            }
            (Some(StateType::Str), StateValue::Str) => {
                self.str_store.update(label, StrConst::Top);
            }
            (Some(StateType::Reference), StateValue::Reference(s)) => {
                self.ref_store.update(label, *s);
            }
            // Type mismatch or Unknown label: use fallback store.
            _ => {
                self.unknown_store.update(label, val);
            }
        }
    }

    /// Convert to a uniform `StateStore<StateValue>` for use with Transfer impls.
    pub fn to_untyped(&self) -> StateStore<StateValue> {
        let mut out = StateStore::bottom();
        for &label in self.type_map.keys() {
            let val = self.get(label);
            if val != StateValue::Bottom {
                out.update(label, val);
            }
        }
        out
    }

    /// Reconstruct a TypedStateStore from a uniform `StateStore<StateValue>`,
    /// preserving the label type assignments from `self`.
    pub fn from_untyped(&self, untyped: &StateStore<StateValue>) -> TypedStateStore {
        let mut result = TypedStateStore::empty_with_types(self.type_map.clone());
        for &label in self.type_map.keys() {
            let val = untyped.get(label);
            if val != StateValue::Bottom {
                result.update(label, val);
            }
        }
        result
    }

    /// `self ⊑ other` per sub-store (more precise than StateValue::leq for typed labels).
    pub fn leq(&self, other: &Self) -> bool {
        self.number_store.leq(&other.number_store)
            && self.bool_store.leq(&other.bool_store)
            && self.str_store.leq(&other.str_store)
            && self.ref_store.leq(&other.ref_store)
            && self.unknown_store.leq(&other.unknown_store)
    }

    /// Pointwise join per sub-store.
    pub fn join(&self, other: &Self) -> Self {
        TypedStateStore {
            type_map: self.type_map.clone(),
            number_store: self.number_store.join(&other.number_store),
            bool_store: self.bool_store.join(&other.bool_store),
            str_store: self.str_store.join(&other.str_store),
            ref_store: self.ref_store.join(&other.ref_store),
            unknown_store: self.unknown_store.join(&other.unknown_store),
        }
    }

    /// Per-sub-store widening (Interval widens bounds, BoolVal/StrConst widen to Top).
    pub fn widen(&self, other: &Self) -> Self {
        TypedStateStore {
            type_map: self.type_map.clone(),
            number_store: self.number_store.widen(&other.number_store),
            bool_store: self.bool_store.widen(&other.bool_store),
            str_store: self.str_store.widen(&other.str_store),
            ref_store: self.ref_store.widen(&other.ref_store),
            unknown_store: self.unknown_store.widen(&other.unknown_store),
        }
    }

    /// Labels whose abstract value differs between `self` and `other`.
    pub fn changed_labels(&self, other: &Self) -> Vec<HookLabel> {
        let mut changed: Vec<HookLabel> = self
            .type_map
            .keys()
            .filter(|&&label| self.get(label) != other.get(label))
            .copied()
            .collect();
        changed.sort_unstable();
        changed
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::expr::Prim;
    use std::sync::Arc;

    fn make_hooks(inits: &[(HookLabel, Expr)]) -> Vec<HookEntry> {
        inits
            .iter()
            .map(|(l, e)| HookEntry::State {
                label: *l,
                init: e.clone(),
            })
            .collect()
    }

    #[test]
    fn infer_int_literal_is_number() {
        assert_eq!(
            infer_state_type(&Expr::Lit(Prim::Int(0))),
            StateType::Number
        );
        assert_eq!(
            infer_state_type(&Expr::Lit(Prim::Float(1.5))),
            StateType::Number
        );
    }

    #[test]
    fn infer_bool_literal_is_boolean() {
        assert_eq!(
            infer_state_type(&Expr::Lit(Prim::Bool(true))),
            StateType::Boolean
        );
    }

    #[test]
    fn infer_string_literal_is_str() {
        assert_eq!(
            infer_state_type(&Expr::Lit(Prim::String("x".into()))),
            StateType::Str
        );
    }

    #[test]
    fn infer_null_is_unknown() {
        assert_eq!(infer_state_type(&Expr::Lit(Prim::Null)), StateType::Unknown);
        assert_eq!(infer_state_type(&Expr::Lit(Prim::Unit)), StateType::Unknown);
    }

    #[test]
    fn infer_object_is_reference() {
        assert_eq!(
            infer_state_type(&Expr::ObjectLit(vec![])),
            StateType::Reference
        );
        assert_eq!(
            infer_state_type(&Expr::ArrayLit(vec![])),
            StateType::Reference
        );
    }

    #[test]
    fn number_update_then_get() {
        let hooks = make_hooks(&[(0, Expr::Lit(Prim::Int(5)))]);
        let mut store = TypedStateStore::from_component(&hooks);
        store.update(0, StateValue::Number(Interval::point(5.0)));
        assert_eq!(store.get(0), StateValue::Number(Interval::point(5.0)));
    }

    #[test]
    fn bool_join_is_precise() {
        // join(True, False) = BoolVal::Top, not StateValue::Top
        let hooks = make_hooks(&[(0, Expr::Lit(Prim::Bool(true)))]);
        let mut a = TypedStateStore::from_component(&hooks);
        a.update(0, StateValue::Boolean(BoolVal::True));
        let mut b = TypedStateStore::from_component(&hooks);
        b.update(0, StateValue::Boolean(BoolVal::False));
        let joined = a.join(&b);
        assert_eq!(joined.get(0), StateValue::Boolean(BoolVal::Top));
    }

    #[test]
    fn number_widen_grows_interval() {
        let hooks = make_hooks(&[(0, Expr::Lit(Prim::Int(0)))]);
        let mut a = TypedStateStore::from_component(&hooks);
        a.update(0, StateValue::Number(Interval::point(0.0)));
        let mut b = TypedStateStore::from_component(&hooks);
        b.update(0, StateValue::Number(Interval::point(1.0)));
        let widened = a.widen(&b);
        match widened.get(0) {
            StateValue::Number(i) => {
                assert_eq!(i.lo, 0.0);
                assert!(i.hi.is_infinite());
            }
            other => panic!("expected Number, got {other:?}"),
        }
    }

    #[test]
    fn leq_number_precise() {
        let hooks = make_hooks(&[(0, Expr::Lit(Prim::Int(0)))]);
        let mut narrow = TypedStateStore::from_component(&hooks);
        narrow.update(0, StateValue::Number(Interval::point(5.0)));
        let mut wide = TypedStateStore::from_component(&hooks);
        wide.update(0, StateValue::Number(Interval { lo: 0.0, hi: 10.0 }));
        assert!(narrow.leq(&wide));
        assert!(!wide.leq(&narrow));
    }

    #[test]
    fn fallback_unknown_label() {
        let hooks = make_hooks(&[(0, Expr::Lit(Prim::Null))]);
        let mut store = TypedStateStore::from_component(&hooks);
        store.update(0, StateValue::Top);
        assert_eq!(store.get(0), StateValue::Top);
    }

    #[test]
    fn round_trip_to_from_untyped() {
        let hooks = make_hooks(&[
            (0, Expr::Lit(Prim::Int(42))),
            (1, Expr::Lit(Prim::Bool(true))),
        ]);
        let mut store = TypedStateStore::from_component(&hooks);
        store.update(0, StateValue::Number(Interval::point(42.0)));
        store.update(1, StateValue::Boolean(BoolVal::True));

        let untyped = store.to_untyped();
        let restored = store.from_untyped(&untyped);

        assert_eq!(restored.get(0), store.get(0));
        assert_eq!(restored.get(1), store.get(1));
    }

    #[test]
    fn str_update_and_get() {
        let hooks = make_hooks(&[(0, Expr::Lit(Prim::String("dark".into())))]);
        let mut store = TypedStateStore::from_component(&hooks);
        store.update(
            0,
            StateValue::StrConst(Arc::new(["dark".to_string()].into_iter().collect())),
        );
        match store.get(0) {
            StateValue::StrConst(set) => assert!(set.contains("dark")),
            other => panic!("expected StrConst, got {other:?}"),
        }
    }
}
