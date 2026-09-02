//! The slot-seed relation (#106, ADR-031): which state slots a `useState`
//! initializer seeds from a prop, and whether anything visibly re-syncs them.
//!
//! Computed at convergence beside the slot-writer relation and stored on
//! [`crate::engine::AnalysisResult`], so the two consumers — the native
//! `frozen-initial-state` rule and the Tier-A `seeds` edge — read one relation
//! instead of each running its own scan (ADR-027 §1).
//!
//! **The sync fold is syntactic on purpose** (ADR-020 item 3): it reads no
//! abstract value at any program point. It answers "is there a write that
//! re-runs when this prop moves", not "does that write produce the right
//! value" — the latter is `derived-state`'s question.
//!
//! **The effect half is derived from `slot_writers`**, never from a second
//! write scan. A relation that already says "an effect wrote this slot" is the
//! answer; running a parallel scan beside it is how two readings of one fact
//! drift apart.

use std::collections::{HashMap, HashSet};

use crate::domains::StateValue;
use crate::ir::{
    bindings::local_bindings,
    cfg::CFG,
    expr::Expr,
    free_vars::{AccessPath, collect_used_paths, dep_paths, path_covered},
    hooks::{DepsArg, HookEntry},
    types::{HookLabel, Var},
};

use super::setters::{SlotWriter, WriterPhase, WriterRegion};
use super::{AnalysisResult, EffectInfo};

/// Whether anything visibly re-syncs a prop-seeded slot when the prop moves.
///
/// **MAY-typed in one direction.** `Synced` is a claim the relation makes from
/// a write it saw; `NoneSeen` is the absence of one, which is not a proof that
/// no sync exists — a setter the component handed out could be called from
/// anywhere (see [`SlotSeed::setter_escapes`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedSync {
    /// A render-time write, or an effect write that re-runs when this seed's
    /// prop moves (its deps cover the seed path, or it declares none).
    Synced,
    /// No such write was seen.
    NoneSeen,
}

/// One `(state slot, prop path its initializer reads)` row.
#[derive(Debug, Clone)]
pub struct SlotSeed {
    pub slot: HookLabel,
    /// The path as written at the seed site — what a message shows, and what
    /// an env evaluation is run against. Exact.
    pub path: AccessPath,
    /// Props-param-rooted forms of [`Self::path`]: `[value]`, `[props.value]`
    /// and `[props]` must all cover a seed read as `value`. Non-empty by
    /// construction — a path that does not normalize to the props param is not
    /// a seed and produces no row.
    ///
    /// A **may**-set. Where the chase could not select through a binding it
    /// widened to every path that binding reads, so a form here is one the
    /// seed *may* denote (ADR-033 §3). The deps-coverage test is a must-claim
    /// and therefore runs on the exact forms only, inside the relation — this
    /// column is for consumers that want the possibilities.
    pub normalized: Vec<AccessPath>,
    pub sync: SeedSync,
    /// An alias of this slot's setter is used somewhere other than a direct
    /// call or a pure alias binding — passed as a prop, stored in an object,
    /// handed to an opaque call.
    ///
    /// A **separate column, not folded into [`SeedSync`]**, because it answers
    /// a different question: not "is there a sync" but "could there be one we
    /// cannot see". Folding it into `Synced` would erase the distinction the
    /// native rule's Error tier is built on — a no-sync claim is certain only
    /// when the setter stayed home.
    pub setter_escapes: bool,
}

/// Every prop-seeded slot of the component, with its sync verdict.
///
/// Ordered by `(slot, path)`, so the relation is deterministic.
pub(crate) fn collect_slot_seeds(
    render_cfg: &CFG,
    hooks: &[HookEntry],
    param: &Var,
    setter_labels: &HashMap<Var, HookLabel>,
    slot_writers: &[SlotWriter],
    effect_info: &HashMap<HookLabel, EffectInfo>,
) -> Vec<SlotSeed> {
    let bindings = local_bindings(render_cfg);
    let mut out: Vec<SlotSeed> = Vec::new();

    for hook in hooks {
        let HookEntry::State { label, init, .. } = hook else {
            continue;
        };
        let seeds = seed_paths(init, param, &bindings);
        if seeds.is_empty() {
            continue;
        }

        let aliases: HashSet<Var> = setter_labels
            .iter()
            .filter(|(_, l)| **l == *label)
            .map(|(v, _)| v.clone())
            .collect();

        // The two slot-level kills, read off the writer relation: a
        // render-time write (the adjust-during-render pattern — a sync path
        // exists, its misuse is `setter-in-render`'s business), and an effect
        // with no readable deps list, which re-runs after every render.
        let rows = || slot_writers.iter().filter(|w| w.slot == *label);
        // **Phase, not region.** `region` is lexical, and a callback literal
        // written inline in render lives in the render CFG — `useCallback(() =>
        // setValue(x), …)` would read as an adjust-during-render write and
        // suppress the finding. Only `WriterPhase::Render` says the write
        // actually runs during render; a nested write is ⊤, and suppressing on
        // ⊤ is the false negative this project does not trade.
        let render_write = rows().any(|w| w.phase == WriterPhase::Render);
        // The effect half asks "does this effect re-run and write the slot",
        // so it takes the lexical region — but only for rows whose PHASE says
        // the effect is what runs the write (#121). ADR-031 §2 kept this half
        // purely lexical on the argument that a write nested in a `.then` the
        // effect kicks off still counts, because the effect re-running is what
        // re-runs it. That argument covers `Deferred` and `Cleanup`, both of
        // which the effect schedules. It does not cover a callback the effect
        // merely *hands* to an opaque callee — `manager.subscribe(setColor)`
        // is a ⊤-phase row, nothing establishes that the callee ever calls it,
        // and suppressing on ⊤ is the false negative §3 already refuses for
        // the render half. `Handler` is refused for the same reason a handler
        // does not close a churn cycle: it needs an external event.
        let effects: Vec<HookLabel> = rows()
            .filter(|w| effect_triggered(w.phase))
            .filter_map(|w| match w.region {
                WriterRegion::Effect(l) => Some(l),
                _ => None,
            })
            .collect();
        let unconditional_effect = effects.iter().any(|l| {
            effect_info
                .get(l)
                .is_some_and(|i| matches!(i.deps, DepsArg::Absent))
        });

        let escapes = setter_escapes(render_cfg, hooks, &aliases);

        for seed in seeds {
            // Per-seed: an effect whose declared deps cover THIS seed's path
            // re-runs when this prop moves. A `DepsArg::Opaque` list gates the
            // effect by something the engine cannot read, so it proves no sync
            // and must not suppress one; a flattened spread declares its
            // elements, not its source, so `covering()` is what may be
            // credited.
            let covered = effects.iter().any(|l| {
                effect_info.get(l).is_some_and(|i| match &i.deps {
                    DepsArg::Absent => true,
                    DepsArg::Opaque => false,
                    DepsArg::List(d) => deps_cover_seed(&d.covering(), &seed, param, &bindings),
                })
            });
            let sync = if render_write || unconditional_effect || covered {
                SeedSync::Synced
            } else {
                SeedSync::NoneSeen
            };
            out.push(SlotSeed {
                slot: *label,
                normalized: seed.paths(),
                path: seed.orig,
                sync,
                setter_escapes: escapes,
            });
        }
    }

    out.sort_by_key(|a| (a.slot, a.path.to_string()));
    out
}

/// Does this writer row's phase say the **effect** is what runs the write?
///
/// The three phases an effect schedules: its own body, a continuation it kicks
/// off, and the cleanup it returns. `Handler` needs an external event and
/// `Unknown` is ⊤ — a callback handed to an opaque callee, where nothing says
/// the callee ever calls it. Neither may support the must-claim that a
/// suppression is (#121).
fn effect_triggered(p: WriterPhase) -> bool {
    matches!(
        p,
        WriterPhase::Effect | WriterPhase::Deferred | WriterPhase::Cleanup
    )
}

// ── The fold helpers (moved from `rules/impls/frozen_initial_state.rs`) ───────

/// One prop read by a `useState` initializer, before it becomes a row.
struct SeedPath {
    orig: AccessPath,
    normalized: Vec<NormPath>,
}

impl SeedPath {
    /// Every props-rooted form, exact or widened — the may-set the row keeps.
    fn paths(&self) -> Vec<AccessPath> {
        self.normalized.iter().map(|n| n.path.clone()).collect()
    }
}

/// `props.a.b` / `value` as a plain member chain, `None` for anything else.
fn as_member_chain(e: &Expr) -> Option<AccessPath> {
    match e.peel_ts() {
        Expr::Var(v) => Some(AccessPath {
            root: v.clone(),
            segments: vec![],
        }),
        Expr::FieldAccess { obj, field } => {
            let mut p = as_member_chain(obj)?;
            p.segments.push(field.clone());
            Some(p)
        }
        _ => None,
    }
}

/// One props-param-rooted form of a chased path, and whether the chase that
/// produced it was **exact**.
///
/// The two consumers ask opposite questions of the same chase, so the bit is
/// not optional: "does this initializer read a prop" is a may-query a widened
/// path answers, while "does this dep cover that seed" is a must-claim a
/// widened path cannot support — crediting one suppresses a finding on a
/// coincidence.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NormPath {
    path: AccessPath,
    exact: bool,
}

/// Rewrite `path` into props-param-rooted form(s) by chasing single-binding
/// local chains: destructuring preambles (`let value = __p0.value`), aliases,
/// object-literal members (`__p0 = { manager: colorSchemeManager }` resolves
/// `__p0.manager`), and derived bindings (`const v = props.a ?? d` roots at
/// `props.a`). A multi-write binding is uncertain and not chased.
///
/// A complex RHS the chase cannot select through widens to every path it
/// reads, with the outer segments dropped — those results carry
/// `exact: false`.
///
/// **`seen` is cloned per branch, never shared across siblings.** It keys on
/// the root var while the recursion is over *paths*, so two sibling paths that
/// share a root are not a cycle; threading one set through them cut every
/// branch after the first, and which one survived was `HashSet` iteration
/// order (#120). Branching only happens at a RHS the chase cannot select
/// through, and every branch still shrinks the root set it may revisit.
fn normalize_to_prop(
    path: &AccessPath,
    param: &Var,
    bindings: &HashMap<&str, Vec<&Expr>>,
    seen: &HashSet<Var>,
) -> Vec<NormPath> {
    if path.root == *param {
        return vec![NormPath {
            path: path.clone(),
            exact: true,
        }];
    }
    let mut seen = seen.clone();
    if !seen.insert(path.root.clone()) {
        return vec![];
    }
    let Some(rhss) = bindings.get(path.root.as_str()) else {
        return vec![];
    };
    let [single] = rhss.as_slice() else {
        return vec![];
    };
    // A member chain: splice this path's segments onto it and keep going.
    if let Some(base) = as_member_chain(single) {
        let combined = AccessPath {
            root: base.root,
            segments: base
                .segments
                .into_iter()
                .chain(path.segments.iter().cloned())
                .collect(),
        };
        return normalize_to_prop(&combined, param, bindings, &seen);
    }
    // An object literal the path selects a readable member of: `{ a: x }.a`
    // *is* `x`, so the selector is consumed and the chase stays exact.
    if let (Expr::ObjectLit { fields, .. }, Some(first)) = (single.peel_ts(), path.segments.first())
        && let Some(member) = crate::ir::expr::object_member(fields, first)
        && let Some(base) = as_member_chain(member)
    {
        let combined = AccessPath {
            root: base.root,
            segments: base
                .segments
                .into_iter()
                .chain(path.segments[1..].iter().cloned())
                .collect(),
        };
        return normalize_to_prop(&combined, param, bindings, &seen);
    }
    // Nothing to select through: widen to every path the RHS reads, dropping
    // the outer segments. Inexact by construction.
    let mut set = HashSet::new();
    collect_used_paths(single, &mut set);
    let mut sub: Vec<AccessPath> = set.into_iter().collect();
    sub.sort_by_key(|p| p.to_string());
    let mut out = Vec::new();
    for p in sub {
        out.extend(
            normalize_to_prop(&p, param, bindings, &seen)
                .into_iter()
                .map(|n| NormPath {
                    path: n.path,
                    exact: false,
                }),
        );
    }
    dedup_norm(out)
}

/// Stable order, no duplicates — the relation's output must not depend on the
/// order a `HashSet` happened to yield.
fn dedup_norm(mut v: Vec<NormPath>) -> Vec<NormPath> {
    v.sort_by_key(|n| (n.path.to_string(), !n.exact));
    v.dedup_by(|a, b| a.path == b.path && a.exact == b.exact);
    v
}

/// Prop paths read by a `useState` initializer, empty when it is prop-free.
fn seed_paths(init: &Expr, param: &Var, bindings: &HashMap<&str, Vec<&Expr>>) -> Vec<SeedPath> {
    let mut used = HashSet::new();
    collect_used_paths(init, &mut used);
    let mut seeds: Vec<SeedPath> = used
        .into_iter()
        .filter_map(|orig| {
            let normalized = normalize_to_prop(&orig, param, bindings, &HashSet::new());
            (!normalized.is_empty()).then_some(SeedPath { orig, normalized })
        })
        .collect();
    seeds.sort_by_key(|s| s.orig.to_string());
    seeds
}

/// `true` when some declared dep covers `seed` — matched on the syntactic path
/// AND on the props-rooted normal forms of both sides, so `[value]`,
/// `[props.value]` and `[props]` all cover a seed read as `value`.
fn deps_cover_seed(
    deps: &[Expr],
    seed: &SeedPath,
    param: &Var,
    bindings: &HashMap<&str, Vec<&Expr>>,
) -> bool {
    let declared = dep_paths(deps);
    // **Only exact normalizations may be credited, on either side.** Coverage
    // is a must-claim that suppresses a finding; a widened path stands for a
    // set of possibilities, and matching one of those against a widened seed
    // path suppresses on a coincidence rather than on a proof (#120).
    let mut declared_norm: Vec<AccessPath> = Vec::new();
    for d in &declared {
        declared_norm.extend(
            normalize_to_prop(d, param, bindings, &HashSet::new())
                .into_iter()
                .filter(|n| n.exact)
                .map(|n| n.path),
        );
    }
    if path_covered(&seed.orig, &declared) {
        return true;
    }
    seed.normalized
        .iter()
        .filter(|n| n.exact)
        .any(|n| path_covered(&n.path, &declared_norm))
}

/// `true` when an alias of the slot's setter is used anywhere outside a direct
/// call or a pure alias binding — passed as a prop, stored in an object,
/// handed to an opaque call. An escaped setter means something we cannot see
/// may sync the slot, so the no-sync claim loses certainty.
fn setter_escapes(render_cfg: &CFG, hooks: &[HookEntry], aliases: &HashSet<Var>) -> bool {
    fn in_expr(e: &Expr, aliases: &HashSet<Var>) -> bool {
        match e {
            Expr::Var(v) => aliases.contains(v),
            Expr::Call { fn_, args } => {
                let callee_is_alias = matches!(fn_.peel_ts(), Expr::Var(v) if aliases.contains(v));
                (!callee_is_alias && in_expr(fn_, aliases))
                    || args.iter().any(|a| in_expr(a, aliases))
            }
            Expr::FnLit {
                params, body_cfg, ..
            } => {
                // Params shadow same-named outer bindings inside the body.
                let inner: HashSet<Var> = aliases
                    .iter()
                    .filter(|a| !params.contains(a))
                    .cloned()
                    .collect();
                !inner.is_empty() && in_cfg(body_cfg, &inner)
            }
            other => {
                let mut found = false;
                other.for_each_child(&mut |c| found = found || in_expr(c, aliases));
                found
            }
        }
    }
    fn in_cfg(cfg: &CFG, aliases: &HashSet<Var>) -> bool {
        use crate::ir::{cfg::Terminator, stmt::Stmt};
        cfg.blocks.values().any(|block| {
            block.stmts.iter().any(|stmt| match stmt {
                // `let s2 = s1` where both sides are known aliases is the
                // alias chain itself, not an escape.
                Stmt::Let { var, rhs, .. } | Stmt::Assign { var, rhs, .. } => match rhs.peel_ts() {
                    Expr::Var(v) if aliases.contains(v) => !aliases.contains(var),
                    _ => in_expr(rhs, aliases),
                },
                Stmt::MemberWrite { obj, key, rhs, .. } => {
                    in_expr(obj, aliases)
                        || matches!(key, crate::ir::stmt::MemberKey::Index(i) if in_expr(i, aliases))
                        || in_expr(rhs, aliases)
                }
                Stmt::ExprStmt(e, _) => in_expr(e, aliases),
            }) || match &block.term {
                Terminator::Return(e) | Terminator::Branch { cond: e, .. } => in_expr(e, aliases),
                _ => false,
            }
        })
    }
    let cfgs = std::iter::once(render_cfg).chain(hooks.iter().filter_map(|h| h.body_cfg()));
    for cfg in cfgs {
        if in_cfg(cfg, aliases) {
            return true;
        }
    }
    // Custom-hook args and state/ref initializers can smuggle the setter too.
    hooks.iter().any(|h| match h {
        HookEntry::Custom { args, .. } => args.iter().any(|a| in_expr(a, aliases)),
        HookEntry::State { init, .. } | HookEntry::Ref { init, .. } => {
            !matches!(init.peel_ts(), Expr::StateSetter(_)) && in_expr(init, aliases)
        }
        _ => false,
    })
}

impl AnalysisResult<StateValue> {
    /// The seed rows of one state slot, in relation order.
    pub fn seeds_of(&self, slot: HookLabel) -> impl Iterator<Item = &SlotSeed> {
        self.slot_seeds.iter().filter(move |s| s.slot == slot)
    }
}

#[cfg(test)]
mod chase_tests {
    //! #120 — the binding chase, pinned directly. Both halves of the fix are
    //! invisible end-to-end on most shapes, so they are gated here.

    use super::*;
    use crate::ir::expr::{BinOp, Prim};
    use crate::ir::stmt::Stmt;
    use crate::ir::types::ExprId;
    use crate::test_support::single_block_cfg;

    fn let_(var: &str, rhs: Expr) -> Stmt {
        Stmt::Let {
            var: var.to_string(),
            rhs,
            span: None,
        }
    }

    fn field(obj: &str, f: &str) -> Expr {
        Expr::FieldAccess {
            obj: Box::new(Expr::Var(obj.to_string())),
            field: f.to_string(),
        }
    }

    fn path(root: &str, segs: &[&str]) -> AccessPath {
        AccessPath {
            root: root.to_string(),
            segments: segs.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// `function C({ seed, other }) { const opts = { seed, other }; … }` —
    /// the destructuring preamble plus the literal that re-packs it.
    fn destructured_then_repacked() -> CFG {
        single_block_cfg(vec![
            let_("__obj", Expr::Var("__p0".to_string())),
            let_("seed", field("__obj", "seed")),
            let_("other", field("__obj", "other")),
            let_(
                "opts",
                Expr::ObjectLit {
                    id: ExprId(0),
                    fields: vec![
                        ("seed".to_string(), Expr::Var("seed".to_string())),
                        ("other".to_string(), Expr::Var("other".to_string())),
                    ],
                },
            ),
        ])
    }

    fn chase(cfg: &CFG, p: AccessPath) -> Vec<(String, bool)> {
        let bindings = local_bindings(cfg);
        normalize_to_prop(&p, &"__p0".to_string(), &bindings, &HashSet::new())
            .into_iter()
            .map(|n| (n.path.to_string(), n.exact))
            .collect()
    }

    #[test]
    fn a_literal_member_resolves_to_that_member_alone() {
        // Half 1. `{ seed, other }.seed` IS `seed` — the selector is consumed,
        // so the chase stays exact and the sibling member is not a candidate.
        let cfg = destructured_then_repacked();
        assert_eq!(
            chase(&cfg, path("opts", &["seed"])),
            vec![("__p0.seed".to_string(), true)]
        );
        assert_eq!(
            chase(&cfg, path("opts", &["other"])),
            vec![("__p0.other".to_string(), true)]
        );
    }

    #[test]
    fn a_member_a_spread_may_have_overwritten_is_not_resolved() {
        // `{ ...rest, seed }` is exact for `seed`; `{ seed, ...rest }` is not —
        // the spread may carry its own `seed`. The chase widens instead.
        let cfg = single_block_cfg(vec![
            let_("__obj", Expr::Var("__p0".to_string())),
            let_("seed", field("__obj", "seed")),
            let_("rest", Expr::Var("__obj".to_string())),
            let_(
                "opts",
                Expr::ObjectLit {
                    id: ExprId(0),
                    fields: vec![
                        ("seed".to_string(), Expr::Var("seed".to_string())),
                        (
                            format!("{}0", crate::ir::expr::SPREAD_KEY_PREFIX),
                            Expr::Var("rest".to_string()),
                        ),
                    ],
                },
            ),
        ]);
        let got = chase(&cfg, path("opts", &["seed"]));
        assert!(
            got.iter().all(|(_, exact)| !exact),
            "a member behind a spread cannot be claimed exact: {got:?}"
        );
    }

    #[test]
    fn sibling_branches_of_a_widened_chase_all_survive() {
        // Half 2. `const key = seed + other` cannot be selected through, so the
        // chase widens to both reads. Both go through the SAME `__obj` root:
        // one shared cycle guard let the first branch consume it and killed
        // every branch after, and which one ran first was `HashSet` order.
        let mut cfg = destructured_then_repacked();
        cfg.blocks.values_mut().next().unwrap().stmts.push(let_(
            "key",
            Expr::BinOp {
                op: BinOp::Add,
                lhs: Box::new(Expr::Var("seed".to_string())),
                rhs: Box::new(Expr::Var("other".to_string())),
            },
        ));
        assert_eq!(
            chase(&cfg, path("key", &[])),
            vec![
                ("__p0.other".to_string(), false),
                ("__p0.seed".to_string(), false),
            ],
            "both widened branches must survive, in a stable order"
        );
    }

    #[test]
    fn a_self_referential_binding_still_terminates() {
        // The guard the per-branch clone must keep doing its job: `a = a.b`
        // grows the path forever unless a repeated root stops the chase.
        let cfg = single_block_cfg(vec![let_("a", field("a", "b"))]);
        assert!(chase(&cfg, path("a", &[])).is_empty());
    }

    #[test]
    fn a_plain_alias_chain_stays_exact() {
        let cfg = single_block_cfg(vec![
            let_("__obj", Expr::Var("__p0".to_string())),
            let_("value", field("__obj", "value")),
            let_("alias", Expr::Var("value".to_string())),
        ]);
        assert_eq!(
            chase(&cfg, path("alias", &["deep"])),
            vec![("__p0.value.deep".to_string(), true)]
        );
    }

    #[test]
    fn a_multi_write_binding_is_not_chased() {
        let cfg = single_block_cfg(vec![
            let_("__obj", Expr::Var("__p0".to_string())),
            let_("v", field("__obj", "a")),
            let_("v", Expr::Lit(Prim::Int(1))),
        ]);
        assert!(chase(&cfg, path("v", &[])).is_empty());
    }
}
