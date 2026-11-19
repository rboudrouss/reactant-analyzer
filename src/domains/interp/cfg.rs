//! What the CFG solver needs to know about a CFG that is not about values:
//! the env each edge hands on (narrowed by a branch), and how a finished
//! run's envs are read back.

use std::collections::HashMap;

use crate::{
    domains::{AbstractDomain, stores::AbstractEnv},
    ir::{
        cfg::{CFG, Terminator},
        expr::{BinOp, Expr, Prim, UnaryOp},
        types::BlockId,
    },
};

/// The env each successor of `b` receives when `b` exits with `env_out`: a
/// branch narrows on its condition, a jump passes the env on unchanged.
///
/// A side whose narrowing leaves the tested name no value is not taken: it
/// hands its successor nothing, so the successor's entry is the join of its
/// other edges, ⊥ when it has none. The edge is dead under this `env_out`
/// only — a later run of `b` under a larger env re-reads it, so a fixpoint
/// that grows the name brings the side back.
pub(crate) fn outgoing<D: AbstractDomain>(
    cfg: &CFG,
    b: BlockId,
    env_out: &AbstractEnv<D>,
) -> Vec<(BlockId, AbstractEnv<D>)> {
    if let Some(block) = cfg.blocks.get(&b) {
        match &block.term {
            Terminator::Branch {
                cond, then_, else_, ..
            } => [(*then_, true), (*else_, false)]
                .into_iter()
                .filter_map(|(succ, taken)| {
                    let narrowed = narrow_env_for_branch(env_out, cond, taken);
                    let dead = narrowed_var(cond).is_some_and(|x| {
                        narrowed.lookup(x).is_bottom() && !env_out.lookup(x).is_bottom()
                    });
                    (!dead).then_some((succ, narrowed))
                })
                .collect(),
            Terminator::Jump(succ) => vec![(*succ, env_out.clone())],
            Terminator::Return(..) | Terminator::Unreachable => vec![],
        }
    } else {
        cfg.successors(b)
            .into_iter()
            .map(|s| (s, env_out.clone()))
            .collect()
    }
}

/// The entry env of `block`, read back from a finished pass: `initial` for
/// the entry block, else the join of what each predecessor hands it from that
/// predecessor's exit env (ADR-042 §2).
///
/// Joined, never widened. Every exit env over-approximates its block's
/// concrete exits, so their join over-approximates the concrete entry; the
/// widened entry the pass converged on is at least as large, never smaller.
/// A predecessor with no exit env was unreachable and contributes nothing.
pub(crate) fn entry_env_of<D: AbstractDomain>(
    cfg: &CFG,
    block: BlockId,
    initial: &AbstractEnv<D>,
    exits: &HashMap<BlockId, AbstractEnv<D>>,
) -> AbstractEnv<D> {
    if block == cfg.entry {
        return initial.clone();
    }
    let mut env: Option<AbstractEnv<D>> = None;
    for edge in cfg.edges.iter().filter(|e| e.to == block) {
        let Some(exit) = exits.get(&edge.from) else {
            continue;
        };
        for (succ, out) in outgoing(cfg, edge.from, exit) {
            if succ != block {
                continue;
            }
            env = Some(match env {
                None => out,
                Some(e) => e.join(&out),
            });
        }
    }
    env.unwrap_or_else(AbstractEnv::bottom)
}

/// The join of the exit envs of `cfg`'s `return` blocks: the env a body hands
/// back once it has run. `None` when no return block was reached.
pub(crate) fn exit_env_of<D: AbstractDomain>(
    cfg: &CFG,
    exits: &HashMap<BlockId, AbstractEnv<D>>,
) -> Option<AbstractEnv<D>> {
    cfg.blocks
        .values()
        .filter(|b| matches!(b.term, Terminator::Return(..)))
        .filter_map(|b| exits.get(&b.id))
        .cloned()
        .reduce(|acc, env| acc.join(&env))
}

// ── Branch narrowing ──────────────────────────────────────────────────────────

/// The name [`narrow_env_for_branch`] refines on `cond`, when it refines one:
/// the left operand of a comparison, a tested name, a negated one. A side of
/// the branch under which that name has no value is not taken.
pub(crate) fn narrowed_var(cond: &Expr) -> Option<&str> {
    match cond {
        Expr::Var(x) => Some(x),
        Expr::BinOp { lhs, .. }
        | Expr::UnaryOp {
            op: UnaryOp::Not,
            arg: lhs,
        } => match lhs.as_ref() {
            Expr::Var(x) => Some(x),
            _ => None,
        },
        _ => None,
    }
}

/// Refine `env` by applying the constraint implied by `cond` on the chosen branch.
///
/// Handles:
/// - `BinOp { op, lhs: Var(x), rhs: Lit(Int|Float) }` → numeric interval narrowing
/// - `BinOp { Eq|Neq|LooseEq|LooseNeq, lhs: Var(x), rhs: Lit(Null|Unit) }` →
///   nullability narrowing. The side where the test holds keeps `null` and
///   `undefined` (the strict one could keep one, but needs not). The other
///   side drops what the test proves absent: the loose `x == null` and
///   `x == undefined` hold for both `null` and `undefined`, so neither is
///   left (#303); a strict test drops only the value it names.
/// - `Var(x)` / `!x` truthiness → drops null/undefined on the truthy branch
///
/// `taken = true` → then-branch constraint; `taken = false` → else-branch (negated).
/// Falls through to cloning env unchanged for unrecognised patterns.
pub(crate) fn narrow_env_for_branch<D: AbstractDomain>(
    env: &AbstractEnv<D>,
    cond: &Expr,
    taken: bool,
) -> AbstractEnv<D> {
    let with_refined = |x: &str, refined: D| {
        let mut narrowed = env.clone();
        narrowed.refine(x.to_string(), refined);
        narrowed
    };

    match cond {
        Expr::BinOp { op, lhs, rhs } => {
            let Expr::Var(x) = lhs.as_ref() else {
                return env.clone();
            };
            match rhs.as_ref() {
                Expr::Lit(Prim::Int(_) | Prim::Float(_)) => {
                    let v = match rhs.as_ref() {
                        Expr::Lit(Prim::Int(n)) => *n as f64,
                        Expr::Lit(Prim::Float(f)) => *f,
                        _ => unreachable!(),
                    };
                    let cur = env.lookup(x);
                    // Only the number slot moves, so a loose `==` against a
                    // number (`"" == 0`, `false == 0`) narrows as the strict
                    // one: the other slots are kept either way.
                    let refined = match (op, taken) {
                        (BinOp::Lt, true) => cur.narrow_lt(v),
                        (BinOp::Lt, false) => cur.narrow_geq(v),
                        (BinOp::Leq, true) => cur.narrow_leq(v),
                        (BinOp::Leq, false) => cur.narrow_gt(v),
                        (BinOp::Gt, true) => cur.narrow_gt(v),
                        (BinOp::Gt, false) => cur.narrow_leq(v),
                        (BinOp::Geq, true) => cur.narrow_geq(v),
                        (BinOp::Geq, false) => cur.narrow_lt(v),
                        (BinOp::Eq | BinOp::LooseEq, true) => cur.narrow_eq(v),
                        (BinOp::Eq | BinOp::LooseEq, false) => cur.narrow_neq(v),
                        (BinOp::Neq | BinOp::LooseNeq, true) => cur.narrow_neq(v),
                        (BinOp::Neq | BinOp::LooseNeq, false) => cur.narrow_eq(v),
                        _ => cur,
                    };
                    with_refined(x, refined)
                }
                Expr::Lit(lit @ (Prim::Null | Prim::Unit)) => {
                    let cur = env.lookup(x);
                    // Whether the test holds on this side, and how much its
                    // failing proves: a strict test only the value it names.
                    let (holds, loose) = match op {
                        BinOp::Eq => (taken, false),
                        BinOp::Neq => (!taken, false),
                        BinOp::LooseEq => (taken, true),
                        BinOp::LooseNeq => (!taken, true),
                        _ => return env.clone(),
                    };
                    let refined = match (holds, loose, lit) {
                        (true, _, _) => cur.narrow_keep_nullish_only(),
                        (false, true, _) => cur.narrow_drop_null().narrow_drop_undef(),
                        (false, false, Prim::Null) => cur.narrow_drop_null(),
                        (false, false, _) => cur.narrow_drop_undef(),
                    };
                    with_refined(x, refined)
                }
                _ => env.clone(),
            }
        }
        // Truthiness guard `if (x)`: the taken branch excludes every falsy
        // value (null, undefined, 0, "", false); the else branch keeps only
        // the falsy ones (references are always truthy → ⊥ there).
        Expr::Var(x) => with_refined(
            x,
            if taken {
                env.lookup(x).narrow_truthy()
            } else {
                env.lookup(x).narrow_falsy()
            },
        ),
        // `if (!x)`: branches swap — the ELSE branch is the truthy one.
        Expr::UnaryOp {
            op: UnaryOp::Not,
            arg,
        } => {
            if let Expr::Var(x) = arg.as_ref() {
                with_refined(
                    x,
                    if taken {
                        env.lookup(x).narrow_falsy()
                    } else {
                        env.lookup(x).narrow_truthy()
                    },
                )
            } else {
                env.clone()
            }
        }
        _ => env.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::{BoolVal, Interval, Stability, StateValue};
    use oxc_allocator::Allocator;
    use oxc_ast::ast::Statement;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;

    /// The one branch `f`'s body lowers to: its test, as the lowering wrote it.
    fn the_test(src: &str) -> Expr {
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, src, SourceType::tsx())
            .with_options(ParseOptions::default())
            .parse();
        assert!(ret.diagnostics.is_empty(), "{:?}", ret.diagnostics);
        let cfg = ret
            .program
            .body
            .iter()
            .find_map(|s| match s {
                Statement::FunctionDeclaration(f) => f.body.as_ref().map(|b| {
                    crate::lowering::cfg_builder::build_cfg(b, &crate::lowering::LowerCtx::empty())
                }),
                _ => None,
            })
            .expect("a function");
        let tests: Vec<&Expr> = cfg
            .blocks
            .values()
            .filter_map(|b| match &b.term {
                Terminator::Branch { cond, .. } => Some(cond),
                _ => None,
            })
            .collect();
        let [test] = tests.as_slice() else {
            panic!("{src}: expected one branch, got {tests:?}");
        };
        (*test).clone()
    }

    /// The truth table of the nullish tests (#303). A row is a test as the
    /// source writes it; its `then` side is where the test holds (the
    /// right-hand side of `??`, the skipped rest of `?.`). A column is a value
    /// of the tested name:
    ///
    /// | test            | null | undefined | 0 | "" | false | object |
    /// |-----------------|------|-----------|---|----|-------|--------|
    /// | `x == null`     | T    | T         | F | F  | F     | F      |
    /// | `x != null`     | F    | F         | T | T  | T     | T      |
    /// | `x == undefined`| T    | T         | F | F  | F     | F      |
    /// | `x === null`    | T    | F         | F | F  | F     | F      |
    /// | `x !== null`    | F    | T         | T | T  | T     | T      |
    /// | `x === undefined`| F   | T         | F | F  | F     | F      |
    /// | `x ?? d`        | T    | T         | F | F  | F     | F      |
    /// | `x?.m(1)`       | T    | T         | F | F  | F     | F      |
    ///
    /// On the side JavaScript takes, the narrowed value must still hold the
    /// value: a cell that drops it is a path the analysis no longer sees, a
    /// false negative. On the other side it is dropped, the precision #303
    /// asks for, except in the two cells a strict test does not narrow: the
    /// holding side of `=== null` keeps `undefined` and that of `===
    /// undefined` keeps `null` (both are kept by `narrow_keep_nullish_only`,
    /// an over-approximation).
    #[test]
    fn nullish_tests_narrow_by_their_truth_table() {
        let values: [(&str, StateValue); 6] = [
            ("null", StateValue::null()),
            ("undefined", StateValue::undefined()),
            ("0", StateValue::number(Interval::point(0.0))),
            ("\"\"", StateValue::str_singleton(String::new())),
            ("false", StateValue::boolean(BoolVal::False)),
            ("object", StateValue::reference(Stability::Stable)),
        ];
        // (source, the test's truth per value, the kept cells off-truth)
        let rows: [(&str, [bool; 6], &[&str]); 8] = [
            (
                "function f(x) { if (x == null) a(); else b(); }",
                [true, true, false, false, false, false],
                &[],
            ),
            (
                "function f(x) { if (x != null) a(); else b(); }",
                [false, false, true, true, true, true],
                &[],
            ),
            (
                "function f(x) { if (x == undefined) a(); else b(); }",
                [true, true, false, false, false, false],
                &[],
            ),
            (
                "function f(x) { if (x === null) a(); else b(); }",
                [true, false, false, false, false, false],
                &["undefined"],
            ),
            (
                "function f(x) { if (x !== null) a(); else b(); }",
                [false, true, true, true, true, true],
                &["undefined"],
            ),
            (
                "function f(x) { if (x === undefined) a(); else b(); }",
                [false, true, false, false, false, false],
                &["null"],
            ),
            (
                "function f(x, d) { return x ?? d; }",
                [true, true, false, false, false, false],
                &[],
            ),
            (
                "function f(x) { return x?.m(1); }",
                [true, true, false, false, false, false],
                &[],
            ),
        ];
        for (src, truth, kept_off_truth) in rows {
            let test = the_test(src);
            let x = narrowed_var(&test).expect("the test narrows a name");
            for ((name, value), holds) in values.iter().zip(truth) {
                let mut env = AbstractEnv::<StateValue>::default();
                env.extend(x.to_string(), value.clone());
                let on = |taken: bool| narrow_env_for_branch(&env, &test, taken).lookup(x);
                let (taken, other) = (on(holds), on(!holds));
                assert!(
                    *value <= taken,
                    "{src} on {name}: the side JavaScript takes lost the value ({taken:?})"
                );
                let kept = kept_off_truth.contains(name);
                assert_eq!(
                    !other.is_bottom_value(),
                    kept,
                    "{src} on {name}: the side JavaScript does not take holds {other:?}"
                );
            }
        }
    }

    /// A loose test against a number narrows the number slot only: `"" ==
    /// 0` and `false == 0` hold, so neither is dropped from the side where
    /// `x == 0` holds.
    #[test]
    fn a_loose_numeric_test_keeps_what_coerces() {
        let test = the_test("function f(x) { if (x == 0) a(); else b(); }");
        let x = narrowed_var(&test).unwrap();
        for value in [
            StateValue::str_singleton(String::new()),
            StateValue::boolean(BoolVal::False),
        ] {
            let mut env = AbstractEnv::<StateValue>::default();
            env.extend(x.to_string(), value.clone());
            assert!(value <= narrow_env_for_branch(&env, &test, true).lookup(x));
        }
    }
}
