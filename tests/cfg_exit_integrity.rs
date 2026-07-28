//! Every reachable exit of a render CFG must survive inlining.
//!
//! Lowering used to seal a body that falls off the end with
//! `Terminator::Unreachable`. That told the splice control never came back from
//! the callee, so the join block carrying the post-call statements *and the
//! caller's own terminator* was inserted with no predecessor: the caller was
//! severed from its own `Return`, `block_states` never recorded it, and
//! `exit_env()` — which every `stability_verdict` / `may_change` goes through —
//! silently joined over fewer paths. 198 components across the eight corpora
//! were in that state.
//!
//! A JS body that falls off the end returns `undefined`, which is a `Return`.
//! `Unreachable` now means only what it says: a `throw`, a stray `break`.

use reactant::{
    engine::{
        ComponentRegistry, Config, HookRegistry, ProgramAnalysisResult, RootStrategy,
        analyze_program,
    },
    ir::cfg::Terminator,
    lowering::{lower_custom_hooks, lower_program},
    rules::{Diagnostic, RuleCtx, all_rules},
};

fn analyze(src: &str) -> ProgramAnalysisResult {
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;

    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
    let path = std::path::Path::new("test.tsx");
    let components = lower_program(&ret.program, src, path, &mut Default::default());
    let hooks = lower_custom_hooks(&ret.program, src, path, &mut Default::default());
    analyze_program(
        ComponentRegistry::from_components(components),
        HookRegistry::from_hooks(hooks),
        RootStrategy::AllComponents,
        &Config::default(),
    )
}

fn diags(result: &ProgramAnalysisResult, component: &str) -> Vec<Diagnostic> {
    let component = component.to_string();
    let ctx = RuleCtx::new(result, &component);
    all_rules().iter().flat_map(|r| r.check(&ctx)).collect()
}

/// A hook whose body has no `return` at all — the shape that severed callers.
const FALLS_OFF_THE_END: &str = r#"
    function useSetup() {
        useEffect(() => {}, []);
    }
    function C() {
        useSetup();
        const cfg = { a: 1 };
        useEffect(() => { read(cfg); }, [cfg]);
        return <div />;
    }
"#;

#[test]
fn inlining_a_falling_through_hook_keeps_the_caller_exit_reachable() {
    let result = analyze(FALLS_OFF_THE_END);
    let c = &result.components["C"];
    let reachable = c.render_cfg.reachable_blocks();

    let returns: Vec<_> = c
        .render_cfg
        .blocks
        .values()
        .filter(|b| matches!(b.term, Terminator::Return(_)))
        .collect();
    assert!(!returns.is_empty(), "the component must have an exit");

    // The structural claim: at least one exit is reachable AND the fixpoint
    // recorded a state for it, which is exactly what `exit_env()` needs.
    let live_exits: Vec<_> = returns
        .iter()
        .filter(|b| reachable.contains(&b.id))
        .filter(|b| c.block_states.contains_key(&b.id))
        .collect();
    assert!(
        !live_exits.is_empty(),
        "no reachable exit with a recorded state: blocks={:?} reachable={:?} states={:?}",
        c.render_cfg.blocks.keys().collect::<Vec<_>>(),
        reachable,
        c.block_states.keys().collect::<Vec<_>>(),
    );
}

/// The consequence a rule can observe: `always-unstable-deps` evaluates the dep
/// in the render-exit env. With the caller severed, that env was missing the
/// real exit and the fresh object read as "no bound" — silence.
#[test]
fn a_dep_after_an_inlined_call_is_still_evaluated() {
    let d = diags(&analyze(FALLS_OFF_THE_END), "C");
    assert!(
        d.iter().any(|x| x.rule == "always-unstable-deps"),
        "expected always-unstable-deps on the fresh `cfg` dep, got {:?}",
        d.iter().map(|x| &x.rule).collect::<Vec<_>>()
    );
}

/// A body that *does* return keeps behaving as before — the fix must not turn
/// an explicit exit into a second one.
#[test]
fn a_hook_that_returns_is_unaffected() {
    let result = analyze(
        r#"
        function useSetup() {
            useEffect(() => {}, []);
            return 1;
        }
        function C() {
            const n = useSetup();
            const cfg = { a: n };
            useEffect(() => { read(cfg); }, [cfg]);
            return <div />;
        }
        "#,
    );
    let d = diags(&result, "C");
    assert!(d.iter().any(|x| x.rule == "always-unstable-deps"), "{d:?}");
}

/// The guard-throw idiom: a custom hook that validates its context and throws.
/// `throw` leaves `Unreachable`, so it is NOT an exit — the hooks after it
/// still dominate every real exit. Wiring the throw to the caller's join
/// instead invented a path reaching the exit without the hooks, which reported
/// `conditional-hook` at the **Error** tier on conformant code.
#[test]
fn a_hook_after_a_guard_throw_is_not_conditional() {
    let result = analyze(
        r#"
        function useCart() {
            const ctx = useContext(CartContext);
            if (ctx === undefined) {
                throw new Error("useCart must be used within a CartProvider");
            }
            const [items, setItems] = useState(ctx.items);
            return items;
        }
        function CartModal() {
            const items = useCart();
            return <div>{items}</div>;
        }
        "#,
    );
    let d = diags(&result, "CartModal");
    assert!(
        !d.iter().any(|x| x.rule == "conditional-hook"),
        "a hook guarded by an early throw is not conditionally called: {:?}",
        d.iter().map(|x| (&x.rule, &x.message)).collect::<Vec<_>>()
    );
}

/// The genuine violation still fires: an early `return` above a hook really
/// does change hook order between renders.
#[test]
fn a_hook_after_an_early_return_is_still_conditional() {
    let result = analyze(
        r#"
        function Gate({ kind }) {
            if (kind === "none") {
                return null;
            }
            const [n, setN] = useState(0);
            return <div>{n}</div>;
        }
        "#,
    );
    let d = diags(&result, "Gate");
    assert!(
        d.iter().any(|x| x.rule == "conditional-hook"),
        "expected conditional-hook, got {:?}",
        d.iter().map(|x| &x.rule).collect::<Vec<_>>()
    );
}
