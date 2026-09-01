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
    /// Props-param-rooted forms of [`Self::path`], for deps-coverage matching:
    /// `[value]`, `[props.value]` and `[props]` must all cover a seed read as
    /// `value`. Non-empty by construction — a path that does not normalize to
    /// the props param is not a seed and produces no row.
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
        // The effect half stays LEXICAL, and deliberately: the question is
        // "does this effect re-run and write the slot", so a write nested in a
        // `.then` the effect kicks off still counts — the effect re-running is
        // what re-runs it. That is also what the scan this replaced did.
        let effects: Vec<HookLabel> = rows()
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
                path: seed.orig,
                normalized: seed.normalized,
                sync,
                setter_escapes: escapes,
            });
        }
    }

    out.sort_by_key(|a| (a.slot, a.path.to_string()));
    out
}

// ── The fold helpers (moved from `rules/impls/frozen_initial_state.rs`) ───────

/// One prop read by a `useState` initializer, before it becomes a row.
struct SeedPath {
    orig: AccessPath,
    normalized: Vec<AccessPath>,
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

/// Rewrite `path` into props-param-rooted form(s) by chasing single-binding
/// local chains: destructuring preambles (`let value = __p0.value`), aliases,
/// and derived bindings (`const v = props.a ?? d` roots at `props.a`). A
/// multi-write binding is uncertain and not chased. For a complex RHS the
/// outer segments are dropped (wider path — safe for rootedness, and coverage
/// against a wider path only errs toward keeping the finding).
fn normalize_to_prop(
    path: &AccessPath,
    param: &Var,
    bindings: &HashMap<&str, Vec<&Expr>>,
    seen: &mut HashSet<Var>,
) -> Vec<AccessPath> {
    if path.root == *param {
        return vec![path.clone()];
    }
    if !seen.insert(path.root.clone()) {
        return vec![];
    }
    let Some(rhss) = bindings.get(path.root.as_str()) else {
        return vec![];
    };
    let [single] = rhss.as_slice() else {
        return vec![];
    };
    if let Some(base) = as_member_chain(single) {
        let combined = AccessPath {
            root: base.root,
            segments: base
                .segments
                .into_iter()
                .chain(path.segments.iter().cloned())
                .collect(),
        };
        return normalize_to_prop(&combined, param, bindings, seen);
    }
    let mut sub = HashSet::new();
    collect_used_paths(single, &mut sub);
    let mut out = Vec::new();
    for p in sub {
        out.extend(normalize_to_prop(&p, param, bindings, seen));
    }
    out
}

/// Prop paths read by a `useState` initializer, empty when it is prop-free.
fn seed_paths(init: &Expr, param: &Var, bindings: &HashMap<&str, Vec<&Expr>>) -> Vec<SeedPath> {
    let mut used = HashSet::new();
    collect_used_paths(init, &mut used);
    let mut seeds: Vec<SeedPath> = used
        .into_iter()
        .filter_map(|orig| {
            let normalized = normalize_to_prop(&orig, param, bindings, &mut HashSet::new());
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
    let mut declared_norm: Vec<AccessPath> = Vec::new();
    for d in &declared {
        declared_norm.extend(normalize_to_prop(d, param, bindings, &mut HashSet::new()));
    }
    if path_covered(&seed.orig, &declared) {
        return true;
    }
    seed.normalized
        .iter()
        .any(|n| path_covered(n, &declared_norm))
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
