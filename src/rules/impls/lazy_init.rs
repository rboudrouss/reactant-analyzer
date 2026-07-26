use crate::rules::RuleCtx;
use std::collections::HashSet;

use crate::ir::{expr::Expr, hooks::HookEntry, types::Var};

use crate::rules::{
    Diagnostic, MustResult, Rule, collect_callees, must_init_calls_setter, setter_var_labels,
};

/// Fires when `useState(...)` is initialised with an expression that contains
/// any function call (e.g. `useState(expensiveCompute())`, `useState(1 + f())`).
/// React evaluates the init argument on every render but only uses the result
/// on mount so the call is wasted work on every render after mount.
/// The fix is the lazy-initialiser form: `useState(() => expensiveCompute())`.
///
/// This goes beyond a syntactic linter on two axes the abstract-interpretation
/// pipeline uniquely enables:
///
/// 1. **Data-flow to the call.** [`arg_is_call_free`] chases the call through
///    local bindings, so a call hidden behind a `const`/temp is still seen:
///    ```js
///    const initial = buildTree(props.data);
///    const [t] = useState(initial);   // ❌  linter sees `useState(initial)` — opaque
///    ```
/// 2. **Effect classification of the call** (see [`InitEffect`]), which grades
///    severity instead of firing one flat warning:
///    - a state-setter call in init runs a state write every render → `Error`;
///    - a side-effecting/async call (`fetch`, `subscribe`, `setTimeout`, …)
///      re-fires the *effect* every render (leaked subscriptions/requests, not
///      just wasted CPU) → `Warning` with a distinct message;
///    - a proven-cheap pure builtin (`Math.*`, `Date.now`, …) → `Info`
///      (advisory; wrapping is optional), which keeps the corpus quiet;
///    - anything else (unknown callee) → `Warning`, as before.
///
/// Patterns matched (any call anywhere in the init expression):
/// ```js
/// useState(expensiveCompute())          // ❌  top-level call
/// useState<number>(Date.now())          // ❌  looks through TSAnnotated
/// useState(1 + expensive())            // ❌  nested call inside BinOp
/// useState({ key: getValue() })        // ❌  call inside ObjectLit
/// ```
///
/// Non-matched:
/// - `useState(0)` literal, call-free.
/// - `useState(a + 1)` BinOp with no call node.
/// - `useState(() => expensiveCompute())` already lazy (FnLit, not Call).
/// - `useState(props.value)` FieldAccess, call-free.
pub struct LazyInit;

/// Nature of the call(s) inside a `useState` initialiser, ordered by how much
/// the finding matters. `Effectful`/`PureCheap` carry the callee name for the
/// message. Classification is a heuristic *refinement* of severity — the
/// trigger stays "a call is present" (sound: no false negative), and only the
/// severity/wording changes.
enum InitEffect {
    /// A state setter is invoked in init — a state write on every render.
    Setter,
    /// A known side-effecting or async call re-fires its effect every render.
    Effectful(String),
    /// Only proven-cheap pure builtins — wrapping is advisory.
    PureCheap(String),
    /// A call whose purity/cost we cannot judge.
    Unknown,
}

impl LazyInit {
    const NAME: &'static str = "lazy-init";
}

impl Rule for LazyInit {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn safe_check(&self, ctx: &RuleCtx) -> Option<crate::rules::SafeCheck> {
        let (result, component) = (ctx.program(), ctx.component());
        use crate::engine::HookKind;
        crate::rules::has_hook_kind(result, component, HookKind::State).then_some(
            crate::rules::SafeCheck {
                rule: Self::NAME,
                message: "no useState initializer re-runs work on every render",
            },
        )
    }

    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic> {
        let (program, component) = (ctx.program(), ctx.component());
        let result = &program.components[component];
        // #1 chases a call through a local binding, but only when that binding is
        // used exactly once (at the init). A `const x = f()` statement runs on
        // every render regardless, so if `x` is read elsewhere the call is not
        // wasted work specific to the init — flagging it would be a false
        // positive (the lazy form cannot defer a value still needed elsewhere).
        let setters: HashSet<Var> = setter_var_labels(&result.render_cfg).into_keys().collect();
        let mut diags = Vec::new();

        for hook in &result.hooks {
            let HookEntry::State {
                label, init, span, ..
            } = hook
            else {
                continue;
            };
            // Fire only on a call that is syntactically part of the init. We do
            // NOT chase calls through local bindings: after custom-hook inlining
            // an already-lazy `useState(() => f())` is flattened to a temp bound
            // to `f()`, which is indistinguishable from an eager
            // `const x = f(); useState(x)` — chasing would flag correct lazy code
            // (corpus FP: `useMediaQuery`). See git history / notes.
            if init.is_call_free() {
                continue;
            }

            // #2: grade by what the call actually does. A setter init is the
            // only Error tier — routed through the must-primitive that mints the
            // proof; the rest stay Warning/Info (safe over-claims, no proof).
            let mut d = match classify_init_effect(init, &setters) {
                InitEffect::Setter => {
                    let msg = "this useState init calls a state setter — it runs a state write on \
                               every render (the result is discarded after mount); move the call \
                               into an effect or event handler";
                    match must_init_calls_setter(init, &setters) {
                        MustResult::All(proof) => Diagnostic::error("lazy-init", proof, msg),
                        // Detection matches classify's `Setter` branch, so this is
                        // unreachable; fall back to Warning (still reported — no FN).
                        _ => Diagnostic::warn("lazy-init", msg),
                    }
                }
                InitEffect::Effectful(name) => Diagnostic::warn(
                    "lazy-init",
                    format!(
                        "this useState init calls `{name}`, which has side effects, on every \
                         render — the result is only used on mount, so every later render \
                         repeats the effect (duplicate subscriptions/requests/timers, not \
                         just wasted work); wrap as `useState(() => …)`"
                    ),
                ),
                InitEffect::PureCheap(name) => Diagnostic::info(
                    "lazy-init",
                    format!(
                        "this useState init calls `{name}` on every render; the call is cheap \
                         and pure, so wrapping as `useState(() => …)` is optional"
                    ),
                ),
                InitEffect::Unknown => Diagnostic::warn(
                    "lazy-init",
                    "this useState is initialised by a direct function call \
                     the call runs on every render but the result is only used on mount; \
                     wrap as `useState(() => …)` to defer it",
                ),
            }
            .with_label(*label);
            if let Some(r) = span {
                d = d.with_range(*r);
            }
            // Witness chain (ADR-019): resolve the init's callee through the
            // function registry — "`f` resolves to ./util.ts" → "its body
            // calls `fetch`". Refinement only: an unresolved callee just
            // yields a Resolve(Unknown) step, never a weaker severity.
            d = d.with_notes(crate::rules::api::witness::chase_value(
                &result.render_cfg,
                init,
                &program.function_registry,
                &result.file,
            ));
            diags.push(d);
        }

        diags
    }
}

/// Classify the call(s) syntactically present in `init`. Precedence:
/// `Setter` > `Effectful` > `Unknown` > `PureCheap` — a single unknown or
/// effectful call is enough to lose the "all cheap and pure" verdict.
fn classify_init_effect(init: &Expr, setters: &HashSet<Var>) -> InitEffect {
    let mut callees: Vec<&Expr> = Vec::new();
    collect_callees(init, &mut callees);

    let mut effectful: Option<String> = None;
    let mut pure_name: Option<String> = None;
    let mut has_unknown = false;

    for callee in callees {
        match classify_callee(callee, setters) {
            Callee::Setter => return InitEffect::Setter,
            Callee::Effectful(n) => effectful.get_or_insert(n),
            Callee::PureCheap(n) => pure_name.get_or_insert(n),
            Callee::Other => {
                has_unknown = true;
                continue;
            }
        };
    }

    match (effectful, pure_name) {
        (Some(n), _) => InitEffect::Effectful(n),
        // A pure-cheap verdict needs an actual pure call AND no unknown call.
        // `has_unknown` also covers the no-`Call`-callee case (e.g. a `CompApp`/
        // `NativeElem` init that is not call-free but has no plain callee).
        (None, Some(n)) if !has_unknown => InitEffect::PureCheap(n),
        _ => InitEffect::Unknown,
    }
}

enum Callee {
    Setter,
    Effectful(String),
    PureCheap(String),
    Other,
}

/// Classify a single call target. Setters are proven (the engine tracks them);
/// the effectful/pure name heuristics are shared witness infrastructure
/// ([`crate::rules::api::witness::classify_callee_name`], ADR-019).
fn classify_callee(fn_: &Expr, setters: &HashSet<Var>) -> Callee {
    let fn_ = match fn_ {
        Expr::TSAnnotated(inner) => inner.as_ref(),
        other => other,
    };
    match fn_ {
        Expr::StateSetter(_) => Callee::Setter,
        Expr::Var(v) if setters.contains(v) => Callee::Setter,
        _ => match crate::rules::api::witness::callee_parts(fn_) {
            Some((method, root)) => {
                match crate::rules::api::witness::classify_callee_name(method, root) {
                    (crate::rules::EffectClass::Effectful, n) => Callee::Effectful(n),
                    (crate::rules::EffectClass::PureCheap, n) => Callee::PureCheap(n),
                    _ => Callee::Other,
                }
            }
            None => Callee::Other,
        },
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domains::StateValueTransfer,
        engine::{Config, ProgramAnalysisResult, analyze_component},
        ir::{
            cfg::{BasicBlock, CFG, Terminator},
            component::ComponentIR,
            expr::{Expr, Prim},
            hooks::HookEntry,
            types::ExprId,
        },
        rules::{Rule, Severity},
    };
    use std::collections::HashMap;
    use std::sync::Arc;

    fn prog(
        r: &crate::engine::AnalysisResult<crate::domains::StateValue>,
    ) -> ProgramAnalysisResult {
        crate::test_support::prog("C", r.clone())
    }

    fn empty_cfg() -> CFG {
        crate::test_support::single_block_cfg(vec![])
    }

    fn component(hooks: Vec<HookEntry>) -> ComponentIR {
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        ComponentIR {
            file: std::path::PathBuf::new(),
            name: "C".to_string(),
            param: "props".to_string(),
            dom_props: Default::default(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
            module_consts: Default::default(),
        }
    }

    #[test]
    fn direct_call_init_warns() {
        // useState(compute())
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::Call {
                fn_: Box::new(Expr::Var("compute".to_string())),
                args: vec![],
            },
            span: None,
        }];
        let result = analyze_component(component(hooks), &StateValueTransfer, &Config::default());
        let diags = LazyInit.check(&RuleCtx::new(&prog(&result), &"C".to_string()));
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "lazy-init");
        assert_eq!(diags[0].hook_label, Some(0));
    }

    #[test]
    fn ts_annotated_call_init_warns() {
        // useState<number>(Date.now()) → TSAnnotated(Call)
        let call = Expr::Call {
            fn_: Box::new(Expr::FieldAccess {
                obj: Box::new(Expr::Var("Date".to_string())),
                field: "now".to_string(),
            }),
            args: vec![],
        };
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::TSAnnotated(Box::new(call)),
            span: None,
        }];
        let result = analyze_component(component(hooks), &StateValueTransfer, &Config::default());
        let diags = LazyInit.check(&RuleCtx::new(&prog(&result), &"C".to_string()));
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn literal_init_no_warning() {
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::Lit(Prim::Int(0)),
            span: None,
        }];
        let result = analyze_component(component(hooks), &StateValueTransfer, &Config::default());
        assert!(
            LazyInit
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
                .is_empty()
        );
    }

    #[test]
    fn fn_lit_init_no_warning() {
        // useState(() => compute()) already lazy, FnLit not Call.
        let lazy = Expr::FnLit {
            id: ExprId(0),
            params: vec![],
            body_cfg: Arc::new(empty_cfg()),
        };
        let hooks = vec![HookEntry::State {
            label: 0,
            init: lazy,
            span: None,
        }];
        let result = analyze_component(component(hooks), &StateValueTransfer, &Config::default());
        assert!(
            LazyInit
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
                .is_empty()
        );
    }

    #[test]
    fn var_init_no_warning() {
        // useState(props.value) no call.
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::FieldAccess {
                obj: Box::new(Expr::Var("props".to_string())),
                field: "value".to_string(),
            },
            span: None,
        }];
        let result = analyze_component(component(hooks), &StateValueTransfer, &Config::default());
        assert!(
            LazyInit
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
                .is_empty()
        );
    }

    #[test]
    fn object_lit_init_no_warning() {
        // useState({}) ObjectLit, not a call.
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::ObjectLit {
                id: ExprId(0),
                fields: vec![],
            },
            span: None,
        }];
        let result = analyze_component(component(hooks), &StateValueTransfer, &Config::default());
        assert!(
            LazyInit
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
                .is_empty()
        );
    }

    fn component_with_render(
        hooks: Vec<HookEntry>,
        stmts: Vec<crate::ir::stmt::Stmt>,
    ) -> ComponentIR {
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts,
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        ComponentIR {
            file: std::path::PathBuf::new(),
            name: "C".to_string(),
            param: "props".to_string(),
            dom_props: Default::default(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
            module_consts: Default::default(),
        }
    }

    #[test]
    fn call_behind_binding_is_not_chased() {
        // const initial = buildTree(props); const [t] = useState(initial);
        // The init is `Var("initial")` — call-free. We deliberately do NOT chase
        // through the binding: after custom-hook inlining an already-lazy
        // `useState(() => f())` flattens to the same shape, so chasing would flag
        // correct lazy code (corpus FP: `useMediaQuery`).
        use crate::ir::stmt::Stmt;
        let stmts = vec![Stmt::Let {
            var: "initial".to_string(),
            rhs: Expr::Call {
                fn_: Box::new(Expr::Var("buildTree".to_string())),
                args: vec![Expr::Var("props".to_string())],
            },
            span: None,
        }];
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::Var("initial".to_string()),
            span: None,
        }];
        let result = analyze_component(
            component_with_render(hooks, stmts),
            &StateValueTransfer,
            &Config::default(),
        );
        assert!(
            LazyInit
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
                .is_empty()
        );
    }

    #[test]
    fn effectful_call_is_warning_with_effect_message() {
        // useState(fetch(url)) — side effect re-fires every render.
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::Call {
                fn_: Box::new(Expr::Var("fetch".to_string())),
                args: vec![Expr::Var("url".to_string())],
            },
            span: None,
        }];
        let result = analyze_component(component(hooks), &StateValueTransfer, &Config::default());
        let diags = LazyInit.check(&RuleCtx::new(&prog(&result), &"C".to_string()));
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity(), Severity::Warning);
        assert!(diags[0].message.contains("side effects"));
    }

    #[test]
    fn pure_cheap_builtin_is_info() {
        // useState(Math.random()) — cheap pure, demoted to Info (advisory).
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::Call {
                fn_: Box::new(Expr::FieldAccess {
                    obj: Box::new(Expr::Var("Math".to_string())),
                    field: "random".to_string(),
                }),
                args: vec![],
            },
            span: None,
        }];
        let result = analyze_component(component(hooks), &StateValueTransfer, &Config::default());
        let diags = LazyInit.check(&RuleCtx::new(&prog(&result), &"C".to_string()));
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity(), Severity::Info);
    }

    #[test]
    fn nested_call_in_binop_fires() {
        // useState(1 + compute()) call nested inside BinOp → must fire.
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::BinOp {
                op: crate::ir::expr::BinOp::Add,
                lhs: Box::new(Expr::Lit(Prim::Int(1))),
                rhs: Box::new(Expr::Call {
                    fn_: Box::new(Expr::Var("compute".to_string())),
                    args: vec![],
                }),
            },
            span: None,
        }];
        let result = analyze_component(component(hooks), &StateValueTransfer, &Config::default());
        assert_eq!(
            LazyInit
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
                .len(),
            1
        );
    }
}
