# Dossier technique 02 — Lowering : construction du CFG, abaissement des expressions, extraction des hooks

> Matière première pour le manuscrit. Tous les extraits de code sont verbatim et
> référencés `chemin:Ldébut-Lfin` (état du dépôt au commit `e67b10a`,
> 2026-09-27). Les dumps d'IR de la section 6 ont été produits par un
> programme **temporaire hors dépôt** (`/tmp/irdump`, qui appelle
> `reactant::lowering::lower_program` / `lower_custom_hooks` puis imprime l'IR
> dans une notation compacte décrite en 6.0) : le dépôt ne fournit **aucune**
> option CLI pour dumper l'IR (`--verbose` n'imprime sur stderr que des lignes
> `[verbose]` : racine de découverte et alias tsconfig, imports suivis
> (`src/driver/mod.rs:172-230`), graphe de symboles, nombre de composants,
> hits/misses du cache et itérations/widening par composant
> (`src/driver/mod.rs:324-425`)), et aucune
> implémentation `Display` n'existe sur les types de l'IR (seul `Debug` est
> dérivé ; `Expr::describe` produit un *nom* pour les messages, pas un
> pretty-printer). Les sorties d'analyseur citées ont été obtenues avec
> `./target/debug/reactant check <fichier> --info --show-clean --no-color`.

---

## 1. Rôle et position dans le pipeline

### 1.1 Ce que fait le sous-système

Le *lowering* traduit l'AST d'oxc (JS/TS complet, riche en sucre syntaxique)
vers l'IR dédiée du projet (ADR-003) : un **graphe de flot de contrôle** (CFG)
de blocs de base contenant quatre formes d'instructions (`Stmt`) sur un langage
d'expressions réduit (`Expr`), plus une **table des hooks** (`Vec<HookEntry>`)
où chaque appel de hook devient une entrée étiquetée par un `HookLabel`. Toutes
les formes équivalentes de la source (déstructurations, court-circuits,
ternaires, retours anticipés, boucles, `switch`, `try`, `await`…) sont
normalisées ici, pour que les domaines abstraits et le moteur de point fixe ne
voient jamais l'AST (ADR-003, « Consequences »).

Trois fichiers portent le cœur du travail :

- `src/lowering/cfg_builder.rs` — le constructeur de blocs (`BlockBuilder`) et
  l'abaissement des **instructions** (contrôle, déclarations, motifs).
- `src/lowering/expr_lower.rs` — l'abaissement des **expressions**, dont celles
  qui fendent des blocs (`?:`, `&&`, `||`, `??`, `await`).
- `src/lowering/hook_extractor.rs` — une **passe IR→IR** qui reconnaît les
  appels de hooks dans le CFG produit, les remplace par des valeurs marquées
  (`StateVal(ℓ)`, `StateSetter(ℓ)`, `MemoVal(ℓ)`, `CallbackVal(ℓ)`,
  `HookMarker(ℓ, _)`), et fabrique les `HookEntry` (y compris les *handlers*
  d'événements et les *subscriptions* `addEventListener`).

### 1.2 Entrées / sorties

| | Entrée | Sortie |
|---|---|---|
| `build_cfg` / `build_fn_body_cfg` / `build_expr_fn_body_cfg` / `build_stmts_cfg` | un corps oxc (`FunctionBody`, `FormalParameters`, ou `&[Statement]`) + un `LowerCtx` | un `CFG` (et, sauf `build_cfg`/`build_stmts_cfg`, la liste des noms de paramètres `Vec<String>`) |
| `extract_hooks` | `&mut CFG` + `ImportCtx` | le CFG réécrit en place, `(Vec<HookEntry>, Vec<HookProvenance>, HookLabel)` (le dernier = prochain label libre) |
| `extract_handlers` | `&CFG`, `&mut Vec<HookEntry>`, `&mut HookLabel` | ajoute des `HookEntry::Handler` |
| `extract_subscriptions` | `&mut Vec<HookEntry>`, `&mut HookLabel` | ajoute des `HookEntry::Handler` issus des `addEventListener` des corps d'effets |

Le résultat final par fonction candidate est un `ComponentIR`
(`src/ir/component.rs:59-83`), un `HookIR` (`src/ir/hook_ir.rs:11-24`) ou un
`FunctionIR` (utilitaires, sans extraction de hooks).

### 1.3 Qui appelle qui (fonctions d'entrée exactes)

```
cli::check::run (src/cli/check.rs:99)
  └─ driver (src/driver/mod.rs:237)
       let mut lowered = lower_files_with(fs.as_ref(), &files, ctx.resolver.as_ref());
       └─ resolver::lower_files_with (src/resolver/mod.rs:247)      ← pour chaque fichier :
            ├─ oxc_parser::Parser::new(&alloc, &source, source_type_for(path)).parse()
            ├─ lowering::lower_program_with_resolver      (src/lowering/mod.rs:437)  → Vec<ComponentIR>
            ├─ lowering::lower_custom_hooks_with_resolver (src/lowering/mod.rs:373)  → Vec<HookIR>
            ├─ lowering::utility_lowerer::lower_utilities_with_resolver
            │                                             (src/lowering/utility_lowerer.rs:38) → Vec<FunctionIR>
            ├─ lowering::collect_module_facts, scan_context_names, build_resolved_imports
  └─ resolver::analyze_lowered (src/resolver/mod.rs:439)
       └─ engine::analyze_program (src/engine/fixpoint.rs:704)
            ├─ expand_custom_hooks (src/engine/fixpoint.rs:889)   ← greffe les HookIR (splice)
            │    └─ ir::splice_callee_into_cfg (src/ir/splice.rs:64)
            ├─ expand_utility_calls (src/engine/fixpoint.rs:1554) ← greffe les FunctionIR
            ├─ collect_hook_calls (src/engine/fixpoint.rs:1277)   ← relit les labels laissés dans le CFG
            ├─ cfg_analyzer::analyze_cfg (point fixe, narrowing sur Branch)
            └─ relations, puis rules (RuleCtx)
```

Dans `lower_program_with_resolver`, la chaîne par composant est exactement :

```rust
// src/lowering/mod.rs:463-470
    detect_components(program)
        .into_iter()
        .map(|candidate| {
            let (param_names, mut render_cfg) = candidate.build_cfg(&ctx);
            let (mut hooks, hook_provenance, mut next_label) =
                extract_hooks(&mut render_cfg, &imports);
            extract_handlers(&render_cfg, &mut hooks, &mut next_label);
            extract_subscriptions(&mut hooks, &mut next_label);
```

L'ordre est significatif : `extract_handlers` doit passer **après**
`extract_hooks`, parce qu'il résout `onClick={cb}` quand `cb` est lié à un
`CallbackVal(ℓ)` que seul `extract_hooks` a produit (commentaire
`src/lowering/hook_extractor.rs:101-103`) ; `extract_subscriptions` passe en
dernier car il fouille les corps des `HookEntry::Effect`. Les labels sont
contigus : hooks d'abord (0..k), puis handlers, puis subscriptions.

La **même** chaîne de trois passes est appliquée au corps de chaque hook
custom dans `lower_custom_hooks_with_resolver` (`src/lowering/mod.rs:396-404`,
résultat `HookIR { file, name, params, body_cfg, hooks, hook_provenance }`).
Les utilitaires (`lower_utilities_with_resolver`,
`src/lowering/utility_lowerer.rs:50-61`) ne passent **que** par
`Candidate::build_cfg` : aucun `extract_*`, donc un appel `useX()` dans un
utilitaire reste un `Call` ordinaire (par construction, un utilitaire n'est
pas censé appeler de hook).

Le paramètre du composant est le premier nom de paramètre, ou `"props"` s'il
n'y en a pas (`src/lowering/mod.rs:471-474`). Un composant à props
déstructurées reçoit donc le nom synthétique `__p0` (voir 4.10).

La répartition entre les trois détecteurs se fait **avant** le lowering, par
`detector::detect_fns` (`src/lowering/detector.rs:41-71`), qui produit des
`Candidate` (`src/lowering/mod.rs:133-144`). `Candidate::build_cfg` est
l'unique point de dispatch entre corps-bloc et corps-expression :

```rust
// src/lowering/mod.rs:146-160
impl Candidate<'_> {
    /// Lower this candidate's body to a CFG, honouring a concise arrow's
    /// implicit return.
    ///
    /// The single place that dispatch happens, so a new consumer of
    /// [`Candidate`] cannot forget it — which is how the flag came to be
    /// dropped in the first place.
    pub(crate) fn build_cfg(&self, ctx: &LowerCtx) -> (Vec<String>, crate::ir::cfg::CFG) {
        if self.expression {
            cfg_builder::build_expr_fn_body_cfg(self.params, self.body, ctx)
        } else {
            cfg_builder::build_fn_body_cfg(self.params, self.body, ctx)
        }
    }
}
```

Les fonctions imbriquées (flèches, `function` expressions, méthodes de classe,
blocs `static`) ne passent pas par `Candidate` : `lower_expr` les abaisse
récursivement dans leur **propre** `BlockBuilder` et les emballe dans
`Expr::FnLit { id, params, body_cfg: Arc<CFG> }` (4.11.8).

---

## 2. Inventaire des fichiers du périmètre

### 2.1 `src/lowering/cfg_builder.rs` — 1381 lignes (18 tests unitaires)

- **Rôle** : machine à états de construction de blocs + abaissement de toutes
  les instructions JS/TS, des motifs de liaison (`BindingPattern`) et des
  préambules de paramètres.
- **Types publics** : `ExprIds` (`pub struct`, L34-43), `LowerCtx`
  (`pub struct`, L52-74). `BlockBuilder` est `pub(super)` (L78-92), `LoopFrame`
  privé (L95-100).
- **Fonctions d'entrée** : `pub fn build_cfg` (L255), `pub fn build_stmts_cfg`
  (L261), `pub fn build_fn_body_cfg` (L973), `pub fn build_expr_fn_body_cfg`
  (L986) ; `pub(super) fn inject_param_preamble` (L952).
- **Fonctions internes** : `lower_stmts` (L270), `lower_stmt` (L279),
  `lower_try` (L492), `lower_if` (L544), `jump_out` (L591), `lower_while`
  (L603), `lower_for` (L635), `lower_iter_loop` (L713), `lower_switch` (L774),
  `lower_var_declarator` (L845), `lower_binding_pattern` (L855).
- **Dépendances internes** : `crate::ir::{SourceMap, cfg::{BasicBlock, CFG,
  Edge, EdgeKind, Terminator}, expr::{Expr, Prim}, stmt::Stmt, types::{BlockId,
  ExprId}}`, `crate::lowering::JsxOrigins`,
  `crate::lowering::expr_lower::{assign_target_ident, empty_cfg, lower_expr,
  lower_class}` (L8-18). Dépendance mutuelle avec `expr_lower.rs`
  (qui importe `BlockBuilder`, `build_expr_fn_body_cfg`, `build_fn_body_cfg`,
  `build_stmts_cfg`).

### 2.2 `src/lowering/expr_lower.rs` — 1667 lignes (22 tests unitaires)

- **Rôle** : abaissement de chaque `oxc_ast::ast::Expression` vers `Expr`,
  avec émission éventuelle d'instructions dans le bloc courant (écritures,
  lectures « pour effet ») et fente de blocs pour les expressions à
  court-circuit.
- **Types publics** : aucun.
- **Fonctions d'entrée** : `pub(super) fn lower_expr` (L143),
  `pub(super) fn lower_class` (L58), `pub(super) fn assign_target_ident`
  (L972), `pub(super) fn empty_cfg` (L1213).
- **Fonctions internes** : `opaque` (L24), `lower_for_effect` (L32),
  `synthetic_key` (L42), `lower_arguments` (L621), `lower_call` (L636),
  `lower_chain_element` (L654), `lower_ternary` (L683), `lower_logical`
  (L739), `lower_jsx_element` (L799), `lower_jsx_props` (L856),
  `is_event_prop_key` (L908), `jsx_element_name` (L915),
  `jsx_member_obj_name` (L927), `lower_jsx_child` (L937),
  `lower_jsx_fragment` (L952), `assign_target_member` (L981),
  `lower_member_target_expr` (L1005), `lower_assignment_target` (L1030),
  `lower_assignment_maybe_default` (L1140), `faithful_compound_binop`
  (L1161), `lower_binop` (L1181).
- **Dépendances internes** : `crate::ir::{cfg, expr::{BinOp, Expr, Prim,
  SPREAD_KEY_PREFIX, SummaryValue, UnaryOp}, hooks::Arity, stmt::{MemberKey,
  Stmt}, source_range::SourceRange}` (L1-16), `super::cfg_builder`.

### 2.3 `src/lowering/hook_extractor.rs` — 1623 lignes (27 tests unitaires)

- **Rôle** : passe IR→IR de reconnaissance des hooks (par **provenance** des
  imports, ADR-023 « step 1 »), de normalisation des terminateurs (#4),
  d'extraction des handlers et des subscriptions.
- **Types publics** : `ResolvedHookCall` (L466-479), `ImportCtx<'a>`
  (L531-543).
- **Fonctions d'entrée** : `pub fn extract_hooks` (L268),
  `pub fn extract_handlers` (L100), `pub fn extract_subscriptions` (L18),
  `pub(crate) fn is_event_prop` (L243), `pub(crate) fn prop_to_event` (L250),
  `ImportCtx::empty` (L548).
- **Fonctions internes** : `collect_subscriptions_in_cfg` (L28),
  `collect_subscriptions_in_expr` (L43), `handler_body` (L146),
  `collect_handlers_in_expr` (L155), `hoist_terminator_hooks` (L321),
  `contains_hook_call` (L349), `process_stmt` (L360),
  `try_consume_hook_call` (L504), `ImportCtx::classify_callee` (L572),
  `make_hook_entry` (L642), `marker_val` (L766), `hook_result_expr` (L775),
  `unwrap_body` (L794), `hook_body_cfg` (L809), `rewrite_expr` (L842).
- **Dépendances internes** : `super::import_resolution::HookOrigin`,
  `crate::ir::{cfg, expr::{Expr, MarkerVal, Prim}, hooks::{DepsArg, HookEntry,
  HookProvenance}, source_range::SourceRange, stmt::{MemberKey, Stmt},
  types::{BlockId, HookLabel}}` (L1-12), `super::is_hook_name`
  (`src/lowering/mod.rs:52-58`).

### 2.4 Fichiers voisins utiles (hors périmètre, cités)

`src/lowering/mod.rs` (565 l., points d'entrée par fichier, `Candidate`,
`is_hook_name`, `collect_module_consts`), `src/lowering/detector.rs`
(169 l., marcheur commun des détecteurs), `src/lowering/component_detector.rs`
(352 l.), `src/lowering/hook_detector.rs` (162 l.),
`src/lowering/hook_call_detect.rs` (109 l., « le corps appelle-t-il un hook ? »
sur l'AST, pour la règle 4 de détection des composants #122),
`src/lowering/import_resolution.rs` (262 l., `HookOrigin`, `JsxOrigins`),
`src/ir/{cfg,expr,stmt,hooks,types,component,hook_ir,source_range}.rs`.

### 2.5 Tests qui exercent le périmètre

| Fichier | Tests | Ce qu'il fixe |
|---|---|---|
| `cfg_builder.rs` `#[cfg(test)]` (L1003-1380) | 18 | break/continue/label/switch/try-less/classe/fall-through/throw/orphelin |
| `expr_lower.rs` `#[cfg(test)]` (L1232-1667) | 22 | ternaire, `&&`/`||`/`??`, flèche concise, affectations composées, opérateurs, `void`, déstructuration en affectation, spreads |
| `hook_extractor.rs` `#[cfg(test)]` (L914-1623) | 27 | useState/useReducer/useEffect/useMemo/useCallback/useRef/custom, handlers, subscriptions, provenance |
| `tests/cfg_exit_integrity.rs` (185 l.) | 5 | ADR-025 : fall-through = `Return(undefined)`, `throw` = `Unreachable` |
| `tests/hook_in_terminator.rs` (104 l.) + `tests/fixtures/hook_in_terminator/App.tsx` | 4 | #4 : hook dans `return`/condition |
| `tests/destructuring.rs` (259 l.) + `tests/fixtures/nested_destr.tsx` | 7 | déstructurations imbriquées |
| `tests/body_calls.rs` (741 l.) | 22 | relation `calls` ; §#131 positions des liaisons synthétiques (ADR-039) |
| `tests/custom_hook_inlining.rs` (468 l.) | 13 | HookIR + splice |
| `tests/concise_arrow_bodies.rs`, `tests/try_catch_finally.rs` | 4 + 5 | #5, #2 |

Vérifié le 2026-09-28 : `cargo test --lib lowering::` → 106 passés ;
`cargo test --test cfg_exit_integrity --test destructuring --test
hook_in_terminator --test body_calls --test custom_hook_inlining --test
concise_arrow_bodies --test try_catch_finally` → 22 + 5 + 4 + 13 + 7 + 4 + 5
passés, 0 échec (cargo exécute les binaires de test par ordre alphabétique :
`body_calls` 22, `cfg_exit_integrity` 5, `concise_arrow_bodies` 4,
`custom_hook_inlining` 13, `destructuring` 7, `hook_in_terminator` 4,
`try_catch_finally` 5). Revérifié par le relecteur le 2026-09-28 :
`cargo test -q --lib lowering::` → `106 passed` (les 67 tests des trois
fichiers du périmètre — 18 + 22 + 27 — plus ceux des autres modules de
`src/lowering/`).

---

## 3. Types et structures centraux

### 3.1 Le CFG : `BasicBlock`, `Terminator`, `EdgeKind`, `Edge`, `CFG`

```rust
// src/ir/cfg.rs:5-65
#[derive(Debug, Clone)]
pub struct BasicBlock {
    pub id: BlockId,
    pub stmts: Vec<Stmt>,
    pub term: Terminator,
}

#[derive(Debug, Clone)]
pub enum Terminator {
    Jump(BlockId),
    Branch {
        cond: Expr,
        then_: BlockId,
        else_: BlockId,
        /// Where the condition is evaluated in the source (None for
        /// synthetic branches and manual-IR tests).
        span: Option<crate::ir::SourceRange>,
    },
    /// Unlike [`Terminator::Branch`], this carries no span: nothing needed the
    /// position of a `return` until a hook could be extracted from one (#4),
    /// and adding it now is a 40-site IR change tracked separately. The cost is
    /// that a hook reached only through a return yields findings with no line
    /// number — visible but unlocated, which is still strictly better than the
    /// silence it replaced.
    Return(Expr),
    Unreachable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeKind {
    Unconditional,
    IfTrue,
    IfFalse,
    Back,
    /// The split at an `await` (#117, ADR-035). Control is the same — the edge
    /// is unconditional — but the successor runs on a later turn of the event
    /// loop, so `sync_phase`'s "lexis = execution, provably" stops holding
    /// across it. Consumers ask [`CFG::post_await_blocks`] rather than reading
    /// this variant directly.
    Await,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub from: BlockId,
    pub to: BlockId,
    pub kind: EdgeKind,
}

/// The block map is a [`BTreeMap`], not a `HashMap`, on purpose: every walk
/// over `blocks` then visits them in ascending [`BlockId`] — i.e. lowering
/// order — so a pass that picks a *representative* block (the first setter
/// call site of a witness, say) reports the same one on every run. Under a
/// `HashMap` that choice followed the per-process hash seed and diagnostics
/// were not reproducible.
#[derive(Debug, Clone)]
pub struct CFG {
    pub entry: BlockId,
    pub blocks: BTreeMap<BlockId, BasicBlock>,
    pub edges: Vec<Edge>,
}
```

Rôle des champs et invariants :

- `BasicBlock.id` doit égaler sa clé dans `blocks` (vérifié par
  `CFG::validate`, `src/ir/cfg.rs:208-211`).
- `Terminator` : `Jump(b)` inconditionnel ; `Branch{cond, then_, else_, span}`
  (le moteur **narrowe** l'environnement selon `cond` vers `then_` avec
  `taken=true`, vers `else_` avec `taken=false`, `src/engine/cfg_analyzer.rs:135-139`) ;
  `Return(e)` sortie de fonction ; `Unreachable` = « le contrôle ne continue
  pas » (`throw`, `break` orphelin, corps vide) — sens unique fixé par
  ADR-025.
- `edges` est **redondant** avec les terminateurs mais c'est lui que lisent
  `successors`/`predecessors`/`reachable_blocks` (`src/ir/cfg.rs:153-179`).
  Invariant (point 3 de `CFG::validate`, `src/ir/cfg.rs:181-227`) : toute cible
  d'un terminateur a une arête. Le lowering respecte l'invariant par
  construction (chaque `seal_with(Jump/Branch)` est suivi d'`add_edge`), mais
  **`validate` n'est appelé qu'en `debug_assert!` après un splice**
  (`src/ir/splice.rs:252`), jamais directement sur la sortie du lowering.
- `EdgeKind::Back` est le **seul** déclencheur du widening dans le point fixe
  (commentaire `src/lowering/cfg_builder.rs:390-393`) ; `IfTrue`/`IfFalse`
  sont lus par `engine::guards::site_guards` pour la polarité des gardes
  (`src/engine/guards.rs:676-689`) ; `Await` par `CFG::post_await_blocks`.
- `blocks` est un `BTreeMap` : parcours déterministe par id croissant = ordre
  d'allocation du lowering (#86, commit `7c21b90`).
- Méthodes : `for_each_expr` / `for_each_expr_where` (visite les expressions de
  **premier niveau** des instructions et des terminateurs `Return`/`Branch`,
  L72-109), `post_await_blocks` (L121-144), `reachable_blocks` (L153-163),
  `successors`/`predecessors` (scan linéaire de `edges`, O(|E|) par appel),
  `validate` (L196-227). Aucune implémentation `Display`, pas de treillis
  (join/meet) : l'IR est une syntaxe, les treillis sont dans `src/domains`.

### 3.2 Instructions : `Stmt` et `MemberKey`

```rust
// src/ir/stmt.rs:7-29
#[derive(Debug, Clone)]
pub enum Stmt {
    Let {
        var: Var,
        rhs: Expr,
        span: Option<SourceRange>,
    },
    Assign {
        var: Var,
        rhs: Expr,
        span: Option<SourceRange>,
    },
    /// In-place write through a member expression: `obj.f = v`, `arr[i] = v`,
    /// `obj.f++`, `delete obj.f`. The heap identity of `obj` is unchanged —
    /// that is the semantic payload: a mutation, not a rebinding.
    MemberWrite {
        obj: Expr,
        key: MemberKey,
        rhs: Expr,
        span: Option<SourceRange>,
    },
    ExprStmt(Expr, Option<SourceRange>),
}
```

```rust
// src/ir/stmt.rs:47-54
/// Which member of the object a [`Stmt::MemberWrite`] targets. `Index` keeps
/// the index expression alive: it is evaluated at runtime, so its reads count
/// (free variables, callbacks).
#[derive(Debug, Clone)]
pub enum MemberKey {
    Field(Symbol),
    Index(Expr),
}
```

- `Let` : introduction d'une liaison (déclaration `const/let/var`, paramètre
  déstructuré, temporaire de lowering, hoist de terminateur). `Assign` :
  réécriture d'une liaison **existante** (réaffectation, `i++`, opérande droit
  de `&&`/`||`). La distinction est porteuse : pour le splice, « un `Assign`
  sans `Let` est l'orthographe IR d'une écriture sur une liaison extérieure »
  (`src/ir/splice.rs`, commentaire du bras `Return`).
- `ExprStmt(e, span)` : expression évaluée pour ses effets et ses lectures.
  Le lowering en émet beaucoup de **synthétiques** (lectures conservées : clé
  calculée, défaut de motif, opérande de `void`, discriminant de `switch`,
  expression itérée d'un `for…of`, argument spread…).
- `span` : `Option<SourceRange>` ; `Stmt::span()` (`src/ir/stmt.rs:37-44`)
  documente l'invariant ADR-039 : « `None` … is a bug to fix, not a shape to
  route around ».

### 3.3 Expressions : `Prim`, `BinOp`, `UnaryOp`, `Expr`, `MarkerVal`, `SummaryValue`

```rust
// src/ir/expr.rs:10-18
#[derive(Debug, Clone)]
pub enum Prim {
    String(String),
    Int(i32),
    Float(f64),
    Bool(bool),
    Null,
    Unit,
}
```

`Prim::Unit` est **`undefined`** (l'identifiant `undefined` s'abaisse en
`Lit(Unit)`, `src/lowering/expr_lower.rs:187-193`). Les nombres entiers de
magnitude < `i32::MAX` deviennent `Int`, les autres `Float`
(`src/lowering/expr_lower.rs:148-154`).

`BinOp` (`src/ir/expr.rs:20-63`) : `Add, Sub, Mul, Div, And, Or, Eq, Neq, Lt,
Gt, Leq, Geq, Mod, Pow, In, InstanceOf, BitAnd, BitOr, BitXor, Shl, Shr,
UShr`. Il n'y a plus de variante `Unknown` : « every binary operator of the
language has its own variant, and `lower_binop` is exhaustive so a new one
cannot slip in silently » (L52-55). `And`/`Or` existent dans l'énumération
mais **ne sont jamais produits par le lowering** (les `&&`/`||` deviennent des
diamants, 4.12 ; vérifié par lecture de `lower_binop`, L1181-1209).

`UnaryOp` (`src/ir/expr.rs:65-80`) : `Neg, Not, BitNot, TypeOf, Plus,
Unknown` — `Unknown` « Evaluated as ⊤ — never as the identity ».

Le type central :

```rust
// src/ir/expr.rs:174-300
#[derive(Debug, Clone)]
pub enum Expr {
    // Primitive literals
    Lit(Prim),

    // Composites each allocating node carries an ExprId (allocation-site key for the heap).
    ObjectLit {
        id: ExprId,
        fields: Vec<(Symbol, Expr)>,
    },
    ArrayLit {
        id: ExprId,
        elems: Vec<Expr>,
        /// How long the *source* array is, which `elems.len()` stops telling
        /// once lowering flattens a `SpreadElement` into its source (one
        /// element standing for however many it holds) or drops an elision
        /// (no element at all). An elision still leaves the length countable,
        /// so only a spread makes it a lower bound. This is the last point
        /// where any of it is knowable, which is why it is recorded here
        /// rather than derived later.
        arity: Arity,
        /// Positions in `elems` that came from a spread. Such an element is a
        /// *container* of entries, not an entry: a reader that treats it as
        /// one claims the array holds `rows` where it holds `rows[0], …`.
        spread_at: Vec<usize>,
    },
    FnLit {
        id: ExprId,
        params: Vec<Var>,
        body_cfg: Arc<CFG>,
    },

    // Vars
    Var(Symbol),

    // Accesses
    FieldAccess {
        obj: Box<Expr>,
        field: Symbol,
    },
    IndexAccess {
        arr: Box<Expr>,
        idx: Box<Expr>,
    },

    // Ops
    BinOp {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    UnaryOp {
        op: UnaryOp,
        arg: Box<Expr>,
    },

    // Calls
    Call {
        fn_: Box<Expr>,
        args: Vec<Expr>,
    },
    /// `new X(args)`: a call that always allocates. The value domain reads it
    /// as a fresh reference whose members are ⊤, and every syntactic proof
    /// reads it where it reads an object literal — lowered to a plain `Call`
    /// it was a value that "holds still" and that no freshness check could
    /// see (#158). `id` is the allocation site, as for `ObjectLit`.
    New {
        id: ExprId,
        fn_: Box<Expr>,
        args: Vec<Expr>,
    },
    CompApp {
        name: Symbol,
        props: Box<Expr>,
        /// Span of the element's opening tag. A diagnostic about a JSX element
        /// (a context provider's `value`, an identity-keyed prop) has nowhere
        /// else to point: unlike `NativeElem`, whose handler props are reached
        /// through `HookEntry::Handler`, a component element owns no hook.
        span: Option<SourceRange>,
        /// Which component `name` was *proven* to name, when the file's
        /// imports (or its own declarations) settle it — the same fact
        /// `HookEntry::Custom::resolved_file` records for a custom hook call.
        /// `None` is ignorance, never a proven negative.
        origin: Option<Arc<CompOrigin>>,
    },
    NativeElem {
        tag: Symbol,
        props: Box<Expr>,
        children: Vec<Expr>,
        /// Span of the element's opening tag — what a finding about the
        /// element points at (#125), the same fact `CompApp` carries.
        span: Option<SourceRange>,
        /// Spans of JSX event-handler props (`onX={fn}`), by prop name.
        /// Populated during lowering; consumed by `hook_extractor` to set
        /// `HookEntry::Handler.span`. A list, not a map: an element has a
        /// handful of handlers, `Expr` is the IR's hottest type, and a `HashMap`
        /// header costs it twice what the whole payload does.
        prop_spans: Vec<(Symbol, Option<SourceRange>)>,
    },

    // TypeScript annotation marker (`x as T`, `useState<T>(..)`). The declared
    // type itself is not retained: TS types are erased at runtime, so narrowing
    // an abstract value by a type annotation would be unsound (`useState<number>`
    // can hold `undefined`/`any`-cast values). The wrapper is kept only so
    // `peel_ts` can see through it to the underlying expression.
    TSAnnotated(Box<Expr>),

    // React Hooks
    StateVal(HookLabel),
    StateSetter(HookLabel),
    MemoVal(HookLabel),
    CallbackVal(HookLabel),

    /// Marks the call site of a hook whose result carries no tracked value
    /// (`useEffect`, `useRef`, custom hooks, …). Its primary role is to keep
    /// the hook's label anchored in the CFG so call-site blocks survive
    /// inlining and renumbering (`collect_hook_calls`, conditional-hook).
    /// Every extracted hook leaves its label in the CFG — value-bearing kinds
    /// via `StateVal`/`MemoVal`/…, all others via this. The [`MarkerVal`] says
    /// what the *binding* reads as.
    HookMarker(HookLabel, MarkerVal),

    /// Injected by `expand_custom_hooks` for library hooks with a `HookSummary`.
    /// Evaluates directly to the encoded abstract value without going through the
    /// concrete expression language (avoids a circular dep between `ir` and `domains`).
    SummaryVal(SummaryValue),
}
```

Points à retenir pour le manuscrit :

- **Nœuds allouants** : `ObjectLit`, `ArrayLit`, `FnLit`, `New` portent un
  `ExprId` = **site d'allocation** (clé du tas abstrait, ADR-010). `CompApp`
  et `NativeElem` n'en portent pas eux-mêmes, mais leurs `props` sont un
  `ObjectLit` qui en a un.
- **Nœuds de hooks** (`StateVal`, `StateSetter`, `MemoVal`, `CallbackVal`,
  `HookMarker`) : **aucun** n'est produit par `lower_expr` ; ils sont
  introduits par `hook_extractor`. `SummaryVal(SummaryValue::Top)` est en
  revanche produit par le lowering comme **sentinelle opaque ⊤**
  (`src/lowering/expr_lower.rs:18-26`).
- `Expr::for_each_child` (`src/ir/expr.rs:463-511`) est l'énumération
  canonique **exhaustive** des enfants directs (sans bras `_`, par principe :
  commentaire L385-402) ; elle **ne traverse pas** les corps de `FnLit`.
- `Expr::subscription_listener` (`src/ir/expr.rs:409-419`) : prédicat unique
  « `x.addEventListener("evt", <FnLit>)` », partagé par
  `extract_subscriptions` et la marche des écrivains de slots (ADR-027 §1).
- `peel_ts` / `peel_ts_owned` (L531-547) retirent les `TSAnnotated`.

`MarkerVal` fixe ce que **lit** la liaison d'un `HookMarker` :

```rust
// src/ir/expr.rs:311-331
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkerVal {
    /// Reads as `undefined` — React's own value-less hooks (an effect returns
    /// nothing).
    Undefined,
    /// Reads as a reference whose identity is constant across renders —
    /// `useRef`. Reading it as `Undefined` instead was *stable enough* for the
    /// deps rules (`to_stability` joins `Stable` for `undef` too), but it threw
    /// the identity away: the container is a reference, and every rule that
    /// reasons about references rather than about stability saw nothing there.
    StableRef,
    /// Reads as ⊤ — a custom hook the engine could neither inline nor
    /// summarize. Paired with the `analysis-limit/unknown-hook` Info.
    Unknown,
    /// Reads as the library hook's [`HookSummary`](crate::registry::HookSummary).
    /// Retagged onto the marker by `expand_custom_hooks` rather than replacing
    /// it: overwriting the marker with a bare `SummaryVal` erased the label,
    /// and with it the call site every rules-of-hooks check needs — a
    /// conditional `useAtom()` was invisible to `conditional-hook`.
    Summary(SummaryValue),
}
```

`SummaryValue` (`src/ir/expr.rs:335-382`) : `Top`, `StableRef`,
`UnstableRef`, `Wrapper { stable }`, `Shape { id, members }`, `Held`,
`Navigator { stable }` ; seul `Top` est produit par le lowering.

`SPREAD_KEY_PREFIX` (`src/ir/expr.rs:82-87`) vaut `"..."`, et
`members_after_last_spread` / `object_member` (L89-113) sont les deux lecteurs
qui respectent la règle « un membre écrit avant un spread ne répond de rien ».

### 3.4 Hooks : `HookEntry`, `DepsArg`, `DepsList`, `Arity`, `HookProvenance`

```rust
// src/ir/hooks.rs:247-313
#[derive(Debug, Clone)]
pub enum HookEntry {
    State {
        label: HookLabel,
        init: Expr,
        span: Option<SourceRange>,
    },
    Effect {
        label: HookLabel,
        body_cfg: CFG,
        deps: DepsArg,
        span: Option<SourceRange>,
    },
    Memo {
        label: HookLabel,
        body_cfg: CFG,
        /// `None` when the deps argument is absent or unreadable. React makes
        /// the argument mandatory in practice, but the IR must not invent an
        /// empty list for one it could not parse.
        deps: DepsArg,
        span: Option<SourceRange>,
    },
    Callback {
        label: HookLabel,
        body_cfg: CFG,
        /// Parameters of the memoized function. Unlike effect/memo bodies
        /// (zero-arg), a `useCallback` fn takes arguments; its params must be
        /// subtracted from the body's free variables or they read as captures
        /// of any same-named outer binding (`(options) => …` shadowing a
        /// component-scope `options`).
        params: Vec<Var>,
        /// See [`HookEntry::Memo`]'s `deps`.
        deps: DepsArg,
        span: Option<SourceRange>,
    },
    Ref {
        label: HookLabel,
        init: Expr,
        span: Option<SourceRange>,
    },
    Custom {
        label: HookLabel,
        name: Symbol,
        args: Vec<Expr>,
        deps: DepsArg,
        /// Variable in the caller's render CFG that receives the hook's return value.
        binding: Option<Var>,
        /// NPM package the hook was imported from, if determinable at parse
        /// time (`"@tanstack/react-query"`). Retained even when a self-aliasing
        /// tsconfig path also resolves the package to a local file, so
        /// `SummaryRegistry` package scoping survives. `None` when the hook is
        /// defined locally or came through a relative specifier.
        import_source: Option<String>,
        /// File the hook's definition resolved to via `ImportResolver` —
        /// relative or aliased specifier — or the current file for a local
        /// definition. `None` for unresolved (plain npm) imports.
        resolved_file: Option<PathBuf>,
        span: Option<SourceRange>,
    },
    Handler {
        label: HookLabel,
        /// DOM event name without the "on" prefix, lowercased: "click", "change", "submit"…
        event: String,
        body_cfg: CFG,
        span: Option<SourceRange>,
    },
}
```

Remarques : le commentaire de `Memo.deps` parle encore de `None` alors que le
type est `DepsArg` (trace d'une époque `Option<Vec<Expr>>`, ADR-004). Les
corps (`body_cfg`) sont possédés (`CFG`, pas `Arc`). `HookEntry::body_cfg()`
(L349-357) est exhaustif « on purpose » pour qu'une nouvelle variante à corps
soit une erreur de compilation. Le `deps` d'un `Custom` est toujours
`DepsArg::Absent` à l'extraction.

```rust
// src/ir/hooks.rs:52-58
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arity {
    /// The source array holds exactly this many entries.
    Exact(usize),
    /// The source array holds at least this many; a spread supplies the rest.
    AtLeast(usize),
}
```

```rust
// src/ir/hooks.rs:102-107
#[derive(Debug, Clone)]
pub enum DepsArg {
    Absent,
    Opaque,
    List(DepsList),
}
```

`DepsArg::from_expr` (`src/ir/hooks.rs:113-127`) : un `ArrayLit` (après
`peel_ts_owned`, donc `[a] as const` compte) → `List(DepsList{elems, arity,
spread_at})`, toute autre expression → `Opaque`. `DepsList::covering`
(L203-215) exclut les éléments issus d'un spread : « Reading `elems` to make a
rule *fire* is sound … Reading it to make a rule *stop* is not ». Les trois
états sont trois faits distincts (commentaire L88-101, commits `a195bfa`,
`48ffef9`, #104).

```rust
// src/ir/hooks.rs:17-42
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookProvenance {
    pub label: HookLabel,
    /// Name the origin defines the hook under (`useLayoutEffect`, `useData`) —
    /// the *imported* name for an aliased import, never the local alias.
    pub origin_hook: Symbol,
    /// `true` iff the call was classified as React's own hook.
    pub react: bool,
    /// Raw import specifier at the call's import site (`"zustand"` even when a
    /// self-aliasing tsconfig path resolves it to a local file). `None` for a
    /// local definition or an unimported name.
    pub specifier: Option<String>,
    /// File the hook's definition resolved to; the current file for a local
    /// definition.
    pub file: Option<PathBuf>,
    /// `false` = written in the component itself; `true` = reached through an
    /// inlined custom hook.
    pub inlined: bool,
    /// Call-site span, pointing into the file the row was lowered from (for
    /// an inlined row, the custom hook's own file — ADR-024 renders the
    /// origin). Provenance-anchored findings need it because the row's label
    /// can dangle: `expand_custom_hooks` keeps the wrapper call's direct row
    /// but splices its `HookEntry` away, so there is no `hook_calls` row left
    /// to join back to for a `SourceRange` (ADR-027 §7).
    pub span: Option<SourceRange>,
}
```

Une ligne de provenance est émise **par appel de hook extrait**, jamais pour
un handler (`src/lowering/hook_extractor.rs:265-266`). Observé : un
`useLayoutEffect` devient `HookEntry::Effect` mais sa provenance garde
`origin_hook = "useLayoutEffect"` — c'est la raison d'être de la table.

### 3.5 Identifiants, positions, contextes

```rust
// src/ir/types.rs:1-15
pub type Symbol = String;
pub type HookLabel = usize;
pub type BlockId = usize;
pub type Var = String;

/// A state slot qualified by the component that owns it. `HookLabel` is
/// per-component, so every cross-component fact — a `Versioned` label set, a
/// churn node, a foreign writer row — names a slot this way (ADR-042 §2).
pub type QualifiedSlot = (super::component_id::ComponentId, HookLabel);

/// Allocation-site key. `Ord` so that a walk over a set of sites has one
/// stable order — the first match over a `HashSet<ExprId>` used to depend on
/// the process hash seed (#120).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExprId(pub usize);
```

`ExprId::fresh()` (L17-25) tire d'un compteur global partant de
`1_000_000_000` : réservé aux valeurs synthétiques du moteur, jamais au
lowering.

```rust
// src/lowering/cfg_builder.rs:34-58
#[derive(Debug, Clone, Default)]
pub struct ExprIds(Rc<Cell<usize>>);

impl ExprIds {
    fn next(&self) -> ExprId {
        let id = self.0.get();
        self.0.set(id + 1);
        ExprId(id)
    }
}

/// Everything one file's lowering shares with every body it builds: the span
/// table, the allocation-site counter, and the JSX callee origins.
///
/// One carrier rather than three parameters. Nested bodies are lowered through
/// their own `BlockBuilder`, so a per-file fact only reaches a `FnLit` body if
/// it travels with the builder that makes it; bundling is what keeps the next
/// such fact from needing its own threading pass.
#[derive(Debug, Clone)]
pub struct LowerCtx {
    pub smap: SourceMap,
    /// Shared with every other body of this file — see [`ExprIds`].
    pub ids: ExprIds,
    pub jsx: JsxOrigins,
}
```

- `ExprIds` : compteur **partagé** (`Rc<Cell<_>>`) entre tous les corps
  (render + corps imbriqués) construits avec le même `LowerCtx`. Cloner le
  `LowerCtx` partage le compteur. Motif : #134 (deux objets sans rapport qui
  partageaient une entrée de tas → faux négatif). Précision observée : chaque
  point d'entrée (`lower_program_with_resolver`, `lower_custom_hooks_with_resolver`,
  `lower_utilities_with_resolver`) crée **son propre** `LowerCtx`
  (`src/lowering/mod.rs:382-385`, `446-449`, `utility_lowerer.rs:46-49`).
  Précisément : **tous les composants** d'un fichier partagent un compteur
  (celui de `lower_program_with_resolver`, qui ne crée le `LowerCtx` qu'une
  fois avant d'itérer sur `detect_components`), **tous les hooks custom** du
  même fichier en partagent un second, les utilitaires un troisième ; chacune
  de ces trois familles est donc numérotée depuis 0 (observé : `useToggle` de
  l'exemple 6.5 a `fn#0`, comme le composant a `#0` ; dans le fichier
  `/tmp/verif02/ex/v1_ts_target.tsx`, le deuxième composant commence à `#4`
  parce que le premier a consommé `#0..#3`). Ce n'est pas une collision parce que le splice décale les ids du
  callé (`Offsets.ids`, `src/ir/remap.rs:20-25` ; « One counter per file; the
  splice gives an inlined callee its own range », `cfg_builder.rs:32-33`).
- `LowerCtx::new(smap, jsx)` (L61-67) : l'unique constructeur réel — crée un
  `ExprIds::default()` neuf (compteur à 0) ; appelé trois fois en production
  (`mod.rs:382`, `mod.rs:446`, `utility_lowerer.rs:46`) et une fois dans un
  test (`hook_extractor.rs:1376`).
- `LowerCtx::empty()` (L69-73) : `Self::new(SourceMap::empty(),
  JsxOrigins::default())` — pas de spans, pas d'origines JSX — pour les tests
  unitaires (« manual-IR tests, whose callees then resolve by name exactly as
  before »).
- `SourceMap` (`src/ir/source_range.rs:53-81`) : table des débuts de ligne +
  `FileId` ; `span_at(offset)` rend `None` si la table est vide.
  `SourceRange { file: FileId, line (1-indexée), col (0-indexée) }`
  (L36-42). Les spans sont des **points** (début de nœud), pas des intervalles.

### 3.6 Le constructeur : `BlockBuilder` et `LoopFrame`

```rust
// src/lowering/cfg_builder.rs:78-100
pub(super) struct BlockBuilder {
    blocks: BTreeMap<BlockId, BasicBlock>,
    edges: Vec<Edge>,
    current: BlockId,
    counter: usize,
    current_stmts: Vec<Stmt>,
    terminated: bool,
    temp_counter: usize,
    /// Enclosing breakables, innermost last. A `switch` is breakable but not
    /// continuable, so `continue_to` is `None` there.
    loop_stack: Vec<LoopFrame>,
    /// Label of the statement being lowered, when it labels a loop directly.
    pending_label: Option<String>,
    pub(super) ctx: LowerCtx,
}

/// One enclosing `break`/`continue` target.
struct LoopFrame {
    label: Option<String>,
    break_to: BlockId,
    /// `None` for a `switch`: `continue` skips past it to the enclosing loop.
    continue_to: Option<BlockId>,
}
```

| Champ | Rôle |
|---|---|
| `blocks`, `edges` | le CFG en cours ; un bloc n'entre dans `blocks` qu'au moment où il est **scellé** |
| `current` | id du bloc ouvert |
| `counter` | prochain id libre ; initialisé à 1 car « block 0 is entry » (L108) |
| `current_stmts` | instructions du bloc ouvert |
| `terminated` | le bloc courant est scellé ; `push_stmt` devient alors un no-op (L166-170) |
| `temp_counter` | suffixe des temporaires `__tN` (`fresh_temp`, L242-246), propre à chaque builder |
| `loop_stack` | pile des cibles `break`/`continue` |
| `pending_label` | label posé par `LabeledStatement`, consommé par le prochain `push_loop` |
| `ctx` | le `LowerCtx` (cloné) |

Invariants : `seal_with` a `debug_assert!(!self.terminated, "sealing
already-terminated block")` (L174) ; `into_cfg` scelle le dernier bloc ouvert en
`Return(undefined)` (L213-240, ADR-025). Les ids de blocs sont alloués par
`new_block` **avant** d'abaisser les sous-structures (then/else/join d'un `if`
sont réservés d'abord), donc les blocs imbriqués d'une branche ont des ids
**supérieurs** au bloc de jonction qui suit (conséquence en 8.1.8).

### 3.7 Contexte d'import et classification : `ImportCtx`, `ResolvedHookCall`, `HookOrigin`

```rust
// src/lowering/hook_extractor.rs:531-543
pub struct ImportCtx<'a> {
    /// Hook-relevant imported bindings → proven origin.
    pub origins: &'a HashMap<String, HookOrigin>,
    /// Local names bound to the `react` module itself
    /// (`import React from "react"`, `import * as R from "react"`).
    pub react_ns: &'a HashSet<String>,
    /// `use*` functions defined in this file (JS scoping: a local
    /// definition shadows any same-named import or global).
    pub local_hooks: &'a HashSet<String>,
    /// File being lowered — provenance of locally-defined hooks, so the
    /// `(file, name)` registry lookup stays precise. `None` for hand-built IR.
    pub current_file: Option<&'a Path>,
}
```

```rust
// src/lowering/hook_extractor.rs:466-479
#[derive(Debug, Clone)]
pub struct ResolvedHookCall {
    /// Name the origin defines the hook under — the *imported* name for an
    /// aliased import (`useMemo` for `import { useMemo as useM }`), never
    /// the local alias.
    pub origin_name: String,
    /// `true` iff the call is React's own hook (modeled semantics apply).
    pub is_react: bool,
    /// Raw import specifier, retained even when it also resolved to a file
    /// (a self-aliased package keeps its `SummaryRegistry` scope).
    pub specifier: Option<String>,
    /// File the definition resolved to; the current file for a local decl.
    pub resolved_file: Option<PathBuf>,
}
```

`HookOrigin` (`src/lowering/import_resolution.rs:41-55`) : `React { imported }`
(spécificateur littéral `"react"`, décidé **avant** le résolveur),
`File { file, specifier, imported }`, `Package { specifier, imported }`.

### 3.8 Nommage des temporaires (convention porteuse)

| Nom | Créé par | Portée / unicité |
|---|---|---|
| `__tN` | `fresh_temp` (ternaire, logique, hoist d'`await`) | compteur par builder (chaque corps imbriqué repart de 0) |
| `__arr_{offset}` | motif tableau (`lower_binding_pattern`) | offset source du motif, unique par fichier |
| `__obj_{offset}` | motif objet | idem |
| `__dstr_{offset}` | cible de déstructuration en **affectation** | idem |
| `__p{i}` | paramètre déstructuré (`inject_param_preamble`) | index du paramètre, par corps |
| `__term_{blockid}` | hoist de terminateur (`hoist_terminator_hooks`) | id du bloc |

ADR-020 §11 fige ce choix : « Keep offset-based destructuring temp names … the
`__arr_`/`__obj_` prefix is load-bearing (`hook_extractor` matches
`starts_with("__arr_")` to resolve `useState` destructuring) ».

Deux conséquences de ce schéma de nommage à retenir : (1) les noms
`__arr_{offset}`/`__obj_{offset}`/`__dstr_{offset}` sont uniques **par
fichier** (offset d'octet du motif), donc stables entre composants et entre
exécutions ; (2) `__tN` ne l'est que **par corps** : le render et chaque
`FnLit` imbriqué ont leur propre `__t0` (exemple 6.6), ce qui ne crée pas de
collision parce que chaque corps est un CFG séparé — mais le splice doit
renommer (hors périmètre, `src/ir/splice.rs`).

### 3.9 Surface publique du périmètre (inventaire exhaustif)

Obtenue par `grep -n "pub fn\|pub struct\|pub enum\|pub trait\|pub type\|pub
const\|pub(crate)\|pub(super)"` sur les trois fichiers (aucun `pub trait`,
`pub type`, `pub const` ni `pub enum` dans le périmètre ; les énumérations de
l'IR vivent dans `src/ir/`). Les trois modules sont `pub mod`
(`src/lowering/mod.rs:1-12`) et `mod.rs` réexporte
`pub use cfg_builder::{LowerCtx, build_cfg, build_fn_body_cfg};` et
`pub use hook_extractor::{extract_handlers, extract_hooks,
extract_subscriptions};` (`src/lowering/mod.rs:15`, `:18`).

| Item | Visibilité, ligne | Utilisateurs réels (hors définition) |
|---|---|---|
| `ExprIds` | `pub struct`, `cfg_builder.rs:35` | champ `LowerCtx::ids` ; méthode privée `next` (L38) appelée par `BlockBuilder::next_expr_id` (L162) |
| `LowerCtx` (+ champs `smap`, `ids`, `jsx`) | `pub struct`, L53 | tous les points d'entrée par fichier |
| `LowerCtx::new` | `pub fn`, L61 | `mod.rs:382`, `mod.rs:446`, `utility_lowerer.rs:46`, test `hook_extractor.rs:1376` |
| `LowerCtx::empty` | `pub fn`, L71 | tests unitaires des trois fichiers |
| `BlockBuilder` | `pub(super) struct`, L78 ; seul `ctx` est `pub(super)` (L91) | `expr_lower.rs` |
| `BlockBuilder::{new, push_loop, pop_loop, break_target, continue_target, set_pending_label, new_block, next_expr_id, push_stmt, seal_with, start_block, split_at_await, add_edge, is_terminated, into_cfg, fresh_temp, span_at}` | `pub(super) fn`, L103-248 ; `frame` (L144) est privé | `cfg_builder.rs`, `expr_lower.rs` |
| `build_cfg(body, ctx) -> CFG` | `pub fn`, L255 | **uniquement des tests** (`cfg_builder.rs:1026`, `expr_lower.rs:1257`, `hook_extractor.rs:942`, `:1228`, `:1374`, `:1414`, `:1584`) ; ignore les paramètres (pas de préambule) |
| `build_stmts_cfg(stmts, ctx) -> CFG` | `pub fn`, L261 | `build_cfg` et le bloc `static { … }` de `lower_class` (`expr_lower.rs:119`) — « a class `static { … }` block, which has no `FunctionBody` wrapper » (L259-260) |
| `inject_param_preamble` | `pub(super) fn`, L952 | `build_fn_body_cfg` (L980), `build_expr_fn_body_cfg` (L993) |
| `build_fn_body_cfg` | `pub fn`, L973 | `Candidate::build_cfg` (`mod.rs:157`), déclarations de fonctions (`cfg_builder.rs:403`), flèches à bloc, `function` expressions, méthodes de classe (`expr_lower.rs:79`, `:450`, `:462`) |
| `build_expr_fn_body_cfg` | `pub fn`, L986 | `Candidate::build_cfg` (`mod.rs:155`), flèches concises (`expr_lower.rs:448`) |
| `lower_class` | `pub(super) fn`, `expr_lower.rs:58` | `ClassDeclaration` (`cfg_builder.rs:424`) et `ClassExpression` (`expr_lower.rs:567`) — « Shared by the declaration and the expression form so neither can drift » |
| `lower_expr` | `pub(super) fn`, `expr_lower.rs:143` | partout dans `cfg_builder.rs` |
| `assign_target_ident` | `pub(super) fn`, `expr_lower.rs:972` | affectations (`expr_lower.rs:495`) et cible pré-déclarée de `for…of/in` (`cfg_builder.rs:753`) |
| `empty_cfg` | `pub(super) fn`, `expr_lower.rs:1213` | fonctions sans corps (déclaration ambiante, surcharge TS) : `cfg_builder.rs:405`, `expr_lower.rs:464` |
| `extract_subscriptions` | `pub fn`, `hook_extractor.rs:18` | `mod.rs:403`, `mod.rs:470` |
| `extract_handlers` | `pub fn`, L100 | `mod.rs:402`, `mod.rs:469` |
| `is_event_prop` | `pub(crate) fn`, L243 | `extract_handlers` ; **aussi** `src/rules/helpers/render_tree.rs:438`, `:497` et `src/engine/render_deps.rs:1051`, `:1057` |
| `prop_to_event` | `pub(crate) fn`, L250 | idem (`render_tree.rs:439`, `:500` ; `render_deps.rs:1059`) |
| `extract_hooks` | `pub fn`, L268 | `mod.rs:401`, `mod.rs:468`, tests |
| `ResolvedHookCall` (+ 4 champs `pub`) | `pub struct`, L467 ; méthodes **privées** `provenance` (L482) et `import_source` (L496) | `classify_callee`, `try_consume_hook_call`, `make_hook_entry`, `hook_result_expr` |
| `ImportCtx<'a>` (+ 4 champs `pub`) | `pub struct`, L531 | construit en littéral par `mod.rs:390` et `mod.rs:457` ; méthode privée `classify_callee` (L572) |
| `ImportCtx::empty` | `pub fn`, L548 (sur `ImportCtx<'static>`) | tests : « every `use*` call classifies as React's » ; les deux tables vides sont des `static LazyLock` (L550-551) |

Deux doublons à signaler (principe 3 de CLAUDE.md) :
`expr_lower::is_event_prop_key` (`expr_lower.rs:908-913`) et
`hook_extractor::is_event_prop` (`hook_extractor.rs:243-248`) ont un corps
**identique** (`o`, `n`, majuscule ASCII) ; le premier décide quelles props
reçoivent un span dans `prop_spans`, le second lesquelles deviennent des
handlers. S'ils divergeaient, un handler perdrait sa position. Par ailleurs
`build_cfg` n'a plus d'appelant de production : c'est l'API historique
(sans préambule de paramètres) conservée pour les tests et réexportée par
`mod.rs`.

---

## 4. Algorithmes clefs

### 4.1 La machine à blocs

Protocole invariant, utilisé par toutes les constructions :

1. réserver les ids des blocs cibles (`new_block`) ;
2. sceller le bloc courant avec un terminateur (`seal_with`) et **ajouter les
   arêtes** correspondantes (`add_edge`) ;
3. ouvrir chaque cible (`start_block`), y abaisser le sous-arbre ;
4. si le sous-arbre n'a pas terminé le bloc, le sceller par un `Jump` vers la
   jonction (garde `if !builder.is_terminated()`) ;
5. ouvrir la jonction et continuer.

```rust
// src/lowering/cfg_builder.rs:172-203
    /// Seal current block with terminator. Returns sealed block id.
    pub(super) fn seal_with(&mut self, term: Terminator) -> BlockId {
        debug_assert!(!self.terminated, "sealing already-terminated block");
        let id = self.current;
        let stmts = std::mem::take(&mut self.current_stmts);
        self.blocks.insert(id, BasicBlock { id, stmts, term });
        self.terminated = true;
        id
    }

    /// Switch active block. Clears terminated flag.
    pub(super) fn start_block(&mut self, id: BlockId) {
        self.current = id;
        self.terminated = false;
    }

    /// Seal the current block and continue in a fresh one across an `Await`
    /// edge (#117). Control is unconditional — dominance and reachability are
    /// unchanged — but the successor is marked as running after a suspension.
    ///
    /// A no-op on an already-terminated block: a `return await f()` has no
    /// successor to defer, and sealing twice would trip the builder's own
    /// invariant.
    pub(super) fn split_at_await(&mut self) {
        if self.terminated {
            return;
        }
        let next = self.new_block();
        let from = self.seal_with(Terminator::Jump(next));
        self.add_edge(from, next, EdgeKind::Await);
        self.start_block(next);
    }
```

La boucle d'instructions **s'arrête** dès que le bloc est terminé :

```rust
// src/lowering/cfg_builder.rs:270-277
fn lower_stmts(stmts: &[Statement], builder: &mut BlockBuilder) {
    for stmt in stmts {
        if builder.is_terminated() {
            break;
        }
        lower_stmt(stmt, builder);
    }
}
```

Conséquence : le **code mort** qui suit un `return`/`throw`/`break`/`continue`
dans la même liste n'est pas abaissé du tout — y compris une déclaration de
fonction hissée (8.1.4).

La sortie :

```rust
// src/lowering/cfg_builder.rs:213-240
    pub(super) fn into_cfg(mut self, entry: BlockId) -> CFG {
        if !self.terminated {
            let id = self.current;
            let stmts = std::mem::take(&mut self.current_stmts);
            self.blocks.insert(
                id,
                BasicBlock {
                    id,
                    stmts,
                    // A body that falls off the end returns `undefined` — that
                    // is a `Return`, not `Unreachable`. Sealing it `Unreachable`
                    // told the splice that control never came back, so the join
                    // block carrying the post-call statements *and the caller's
                    // own terminator* was left with no predecessor: 198 corpus
                    // components were severed from their own `Return`, and every
                    // `stability_verdict` on them read an exit env missing a
                    // real path (a false negative). `Unreachable` now means only
                    // what it says — a `throw`, a stray `break`.
                    term: Terminator::Return(Expr::Lit(Prim::Unit)),
                },
            );
        }
        CFG {
            entry,
            blocks: self.blocks,
            edges: self.edges,
        }
    }
```

**Blocs orphelins** : un bloc de jonction dont toutes les branches ont
terminé (`if (c) return 1; else return 2;`) est tout de même ouvert, puis
scellé (par `into_cfg` en `Return(undefined)`, ou par les instructions qui
suivent). Il existe dans `blocks` sans prédécesseur. Test
`reachable_blocks_excludes_an_orphaned_join` (`cfg_builder.rs:1348-1369`) ; les
consommateurs qui quantifient sur les sorties doivent passer par
`CFG::reachable_blocks` (ADR-025 §3).

### 4.2 Table de traitement des instructions (`lower_stmt`, `cfg_builder.rs:279-466`)

| Instruction | IR produite |
|---|---|
| `const/let/var` | pour chaque déclarateur : `lower_var_declarator` → rhs (ou `Lit(Unit)` sans init) → `lower_binding_pattern` (4.9) |
| expression `e;` | `ExprStmt(lower_expr(e), span)` ; l'expression peut avoir déjà émis ses propres `Stmt` et fendu des blocs |
| `return e` / `return` | `seal_with(Return(lower_expr(e)))` / `Return(Lit(Unit))` |
| `if` | `lower_if` (4.3) |
| `{ … }` | `lower_stmts` à plat (pas de portée lexicale modélisée) |
| `while`, `for`, `do…while`, `for…in`, `for…of` | 4.4 |
| `throw e` | `ExprStmt(e)` puis `seal_with(Unreachable)` |
| `try` | `lower_try` (4.6) |
| `switch` | `lower_switch` (4.5) |
| `label: loop` | `set_pending_label` puis la boucle ; sur tout autre corps (bloc, `switch`, `if`, label imbriqué `a: b: while…` pour le label externe) le label est **ignoré** mais le corps est abaissé normalement (`lower_stmt(&l.body)`, L379) |
| `break [l]` / `continue [l]` | `jump_out` vers la cible de la pile (`Unconditional` / `Back`) ; cible absente → `Unreachable` |
| `function f(){}` | `Let f = FnLit{…}` **à sa position** (pas de hissage) |
| `class X {…}` | `Let X = lower_class(…)` (ou `ExprStmt` si anonyme) |
| `with (o) body` | `ExprStmt(o)` puis `body` (noms traités comme liaisons ordinaires) |
| vide, `debugger`, déclarations TS, `import`/`export` | rien (liste explicite, sans bras `_`) |

```rust
// src/lowering/cfg_builder.rs:286-297
        Statement::ExpressionStatement(es) => {
            let expr = lower_expr(&es.expression, builder);
            builder.push_stmt(Stmt::ExprStmt(expr, builder.span_at(es.span.start)));
        }
        Statement::ReturnStatement(ret) => {
            let expr = ret
                .argument
                .as_ref()
                .map(|e| lower_expr(e, builder))
                .unwrap_or(Expr::Lit(Prim::Unit));
            builder.seal_with(Terminator::Return(expr));
        }
```

Effet de bord à connaître : une instruction-expression qui est une
affectation émet l'écriture (`Assign`/`MemberWrite`, 4.11.5) **puis** un
`ExprStmt` de sa valeur (observé : `total += 1;` → `total := (total Add 1)`
puis `expr total`, exemple 6.2).

La liste des instructions « sans code » est écrite en extension et commentée :

```rust
// src/lowering/cfg_builder.rs:445-464
        // Nothing executable — and spelled out, with no `_` arm, so a statement
        // kind that *does* carry code cannot join this list by accident. That is
        // how class bodies were lost in the first place.
        Statement::EmptyStatement(_)
        | Statement::DebuggerStatement(_)
        // Types erase before anything runs.
        | Statement::TSTypeAliasDeclaration(_)
        | Statement::TSInterfaceDeclaration(_)
        | Statement::TSEnumDeclaration(_)
        | Statement::TSModuleDeclaration(_)
        | Statement::TSGlobalDeclaration(_)
        | Statement::TSImportEqualsDeclaration(_)
        // `import`/`export` are module-level only: a syntax error inside the
        // function bodies this lowers, so they never reach here.
        | Statement::ImportDeclaration(_)
        | Statement::ExportAllDeclaration(_)
        | Statement::ExportDefaultDeclaration(_)
        | Statement::ExportNamedDeclaration(_)
        | Statement::TSExportAssignment(_)
        | Statement::TSNamespaceExportDeclaration(_) => {}
```

(Note : `TSEnumDeclaration` a du code à l'exécution en TS — un objet enum — ;
le lowering le traite comme effacé. Sans conséquence connue ; à vérifier si un
chapitre le mentionne.)

### 4.3 `if / else`

```rust
// src/lowering/cfg_builder.rs:544-585
fn lower_if(
    test: &Expression,
    consequent: &Statement,
    alternate: Option<&Statement>,
    builder: &mut BlockBuilder,
) {
    let cond = lower_expr(test, builder);
    let span = builder.span_at(test.span().start);
    let then_block = builder.new_block();
    let else_block = builder.new_block();
    let join_block = builder.new_block();

    let branch_id = builder.seal_with(Terminator::Branch {
        cond,
        then_: then_block,
        else_: else_block,
        span,
    });
    builder.add_edge(branch_id, then_block, EdgeKind::IfTrue);
    builder.add_edge(branch_id, else_block, EdgeKind::IfFalse);

    // Then branch
    builder.start_block(then_block);
    lower_stmt(consequent, builder);
    if !builder.is_terminated() {
        let b = builder.seal_with(Terminator::Jump(join_block));
        builder.add_edge(b, join_block, EdgeKind::Unconditional);
    }

    // Else branch
    builder.start_block(else_block);
    if let Some(alt) = alternate {
        lower_stmt(alt, builder);
    }
    if !builder.is_terminated() {
        let b = builder.seal_with(Terminator::Jump(join_block));
        builder.add_edge(b, join_block, EdgeKind::Unconditional);
    }

    // Continue at join block (may have no predecessors if both branches terminated)
    builder.start_block(join_block);
}
```

Toujours **trois** blocs, même sans `else` (le bloc `else` vide contient juste
`Jump(join)`, cf. `B2: => jump B3` de l'exemple 6.2). La condition est abaissée
**avant** la réservation des blocs : si elle contient un `&&`, son diamant
est construit d'abord et le `Branch` porte `Var(__tN)`.

**Retour anticipé** (`if (x) return null;`) : la branche `then` scelle en
`Return`, pas d'arête vers la jonction ; les instructions suivantes vont dans
la jonction, que seule la branche `else` atteint. C'est ce qui rend un hook
situé après non-dominant de toutes les sorties → `conditional-hook`
(exemple 6.2).

### 4.4 Boucles, `break`, `continue`, labels

`while` (`cfg_builder.rs:603-633`) : `pre →U header ; header: Branch(test,
body, exit) ; body … →Back header ; exit`. Le frame poussé est
`push_loop(exit_block, Some(header))`.

`for (init; test; update)` (`cfg_builder.rs:635-702`) : `init` abaissé dans le
bloc courant (ses `ExprStmt` sans span), puis quatre blocs `header`, `body`,
`update`, `exit` ; test absent → `Lit(Bool(true))`. `continue` vise
**`update_block`** (« `continue` in a `for` runs the update before the next
test », L684). Le retour `update → header` est une arête `Back`.

Détails vérifiés sur le code (`cfg_builder.rs:603-702`) :

- La condition d'un `while` est abaissée **dans** le bloc `header` (après
  `start_block(header)`, L610-611) : si elle contient un `&&`/`?:`, le
  diamant naît dans l'en-tête et c'est le bloc de jonction du diamant qui
  porte le `Branch` de la boucle ; l'arête `Back` du corps vise toujours le
  premier bloc de l'en-tête, donc le test est bien réévalué à chaque tour.
  Même chose pour `for` (condition dans `header`) et `do…while` (condition
  dans `test_block`).
- Dans un `for`, un `continue` produit une arête `Back` vers `update_block`
  (via `jump_out`), puis `update_block → header` est une **seconde** arête
  `Back` : un chemin `continue` traverse deux arêtes arrière. Le corps qui
  termine normalement va à `update_block` par une arête `Unconditional`.
- `init` et `update` d'un `for` qui sont des expressions sont émis en
  `ExprStmt(expr, None)` (sans span, L653 et L696) ; une déclaration
  `let i = 0` en `init` passe par `lower_var_declarator` et garde son span.
- Aucune portée lexicale : `for (let i…)` lie `i` par un `Let` dans le bloc
  qui précède l'en-tête, visible après la boucle pour l'analyse.

`do…while` :

```rust
// src/lowering/cfg_builder.rs:322-354
        Statement::DoWhileStatement(dw) => {
            // The test gets its own block: `continue` in a `do…while` runs it,
            // so it needs to be a jump target, and the exit then has a real
            // predecessor even when the body always leaves early.
            let body_block = builder.new_block();
            let test_block = builder.new_block();
            let exit_block = builder.new_block();
            let pre = builder.seal_with(Terminator::Jump(body_block));
            builder.add_edge(pre, body_block, EdgeKind::Unconditional);

            builder.start_block(body_block);
            builder.push_loop(exit_block, Some(test_block));
            lower_stmt(&dw.body, builder);
            builder.pop_loop();
            if !builder.is_terminated() {
                let b = builder.seal_with(Terminator::Jump(test_block));
                builder.add_edge(b, test_block, EdgeKind::Unconditional);
            }

            builder.start_block(test_block);
            let cond = lower_expr(&dw.test, builder);
            let span = builder.span_at(dw.test.span().start);
            let t = builder.seal_with(Terminator::Branch {
                cond,
                then_: body_block,
                else_: exit_block,
                span,
            });
            builder.add_edge(t, body_block, EdgeKind::Back);
            builder.add_edge(t, exit_block, EdgeKind::IfFalse);

            builder.start_block(exit_block);
        }
```

Particularité : l'arête « vraie » d'un `do…while` est de genre `Back`, pas
`IfTrue` (c'est l'arête de retour).

`for…in` / `for…of` (`lower_iter_loop`) : boucle à borne inconnue ; la
source itérée est évaluée une fois dans le pré-en-tête (lecture conservée pour
les deps), l'en-tête est un `Branch(Lit(true))` **sans narrowing** (le
narrowing du moteur ne traite que `BinOp`, `Var`, `Not(Var)` ;
`Lit(true)` → env inchangé des deux côtés, `src/engine/cfg_analyzer.rs:212-292`),
et la variable de boucle est liée à ⊤ en tête de corps :

```rust
// src/lowering/cfg_builder.rs:719-763
    // Preheader: the iterated expression is read once at loop entry.
    let iterated = lower_expr(right, builder);
    let iter_span = builder.span_at(right.span().start);
    builder.push_stmt(Stmt::ExprStmt(iterated, iter_span));

    let header = builder.new_block();
    let body_block = builder.new_block();
    let exit_block = builder.new_block();

    let pre = builder.seal_with(Terminator::Jump(header));
    builder.add_edge(pre, header, EdgeKind::Unconditional);

    builder.start_block(header);
    let h = builder.seal_with(Terminator::Branch {
        cond: Expr::Lit(Prim::Bool(true)),
        then_: body_block,
        else_: exit_block,
        span: None,
    });
    builder.add_edge(h, body_block, EdgeKind::IfTrue);
    builder.add_edge(h, exit_block, EdgeKind::IfFalse);

    builder.start_block(body_block);
    let top = Expr::SummaryVal(crate::ir::expr::SummaryValue::Top);
    match left {
        ForStatementLeft::VariableDeclaration(decl) => {
            for d in &decl.declarations {
                let span = builder.span_at(d.span.start);
                lower_binding_pattern(&d.id, top.clone(), span, builder);
            }
        }
        // Pre-declared target (`for (x of arr)`): re-assign the outer var.
        // Member/pattern targets are not tracked as a single cell — skipped.
        other => {
            if let Some(var) = other.as_assignment_target().and_then(assign_target_ident) {
                let span = builder.span_at(other.span().start);
                builder.push_stmt(Stmt::Assign {
                    var,
                    rhs: top,
                    span,
                });
            }
        }
    }
    builder.push_loop(exit_block, Some(header));
```

(Le cas « cible membre/motif pré-déclarée » `for (o.x of arr)` /
`for ([a,b] of arr)` n'émet **aucune** écriture : `a`/`b` gardent leur valeur
précédente — c'est exactement la forme que `lower_assignment_target` refuse
ailleurs (« a stale binding is an assertion »). Vérifié sur
`/tmp/ex/e21_forof_pattern.tsx` : `let a = 0; let b = 0; for ([a, b] of pairs)
{…}` ne produit que `expr pairs` en pré-en-tête et aucun `Assign` dans le
corps ; `a` et `b` restent exactement `0` pour l'analyse. Voir 8.1.12.)

`break` / `continue` :

```rust
// src/lowering/cfg_builder.rs:381-397
        // A `break`/`continue` is a real edge to the loop's exit or header.
        // Sealing `Unreachable` instead dropped the state the jump carries out
        // of the loop, and left an exit the CFG says is unreachable while the
        // real one has no edge — all-paths reasoning then reads a phantom exit
        // set (`ExitDominance`, `must_setter_on_all_paths`).
        Statement::BreakStatement(b) => {
            let target = builder.break_target(b.label.as_ref().map(|l| l.name.as_str()));
            jump_out(builder, target, EdgeKind::Unconditional);
        }
        // A `continue` is a back edge of the loop it continues: the fixpoint
        // widens only on `Back` edges, and a loop whose counter advances on
        // the `continue` path alone (`left += 1; continue;`) never converged
        // through an `Unconditional` one.
        Statement::ContinueStatement(c) => {
            let target = builder.continue_target(c.label.as_ref().map(|l| l.name.as_str()));
            jump_out(builder, target, EdgeKind::Back);
        }
```

Résolution des cibles (`cfg_builder.rs:132-149`) : on remonte la pile ; avec un
label, le frame de ce label ; sans label, le plus proche frame pour `break`,
le plus proche frame **continuable** (`continue_to.is_some()`, donc on saute
les `switch`) pour `continue`. Les labels ne sont honorés que sur une boucle
(`cfg_builder.rs:364-380`) ; un label de bloc (`l: { … break l; }`) donne un
`break` sans cible → `Unreachable` (sous-approximation assumée par le
commentaire : « vanishingly rare »).

### 4.5 `switch` (chaîne de dispatch opaque)

```rust
// src/lowering/cfg_builder.rs:786-838
    // One block per case, fronted by a chain of opaque dispatches: dispatch `i`
    // either enters case `i` or moves on to dispatch `i + 1`, and the last one
    // falls to the exit. Which case matches is not a truthiness test on the
    // discriminant, so every dispatch is opaque, and no case has to match — the
    // exit is reachable without entering any of them (a `default` clause is
    // over-approximated as optional).
    //
    // Chaining the consequents into one straight line instead was an
    // under-approximation twice over: entering `case 2` without running `case 1`
    // was unrepresentable, and the first `break` sealed the chain, so every
    // later case was dropped from the CFG entirely.
    let case_blocks: Vec<BlockId> = sw.cases.iter().map(|_| builder.new_block()).collect();
    // The first dispatch is the block the discriminant was evaluated in; the
    // rest need blocks of their own.
    let later_dispatch: Vec<BlockId> = (1..case_blocks.len())
        .map(|_| builder.new_block())
        .collect();

    for (i, &case_block) in case_blocks.iter().enumerate() {
        let skip_to = later_dispatch.get(i).copied().unwrap_or(exit_block);
        let d = builder.seal_with(Terminator::Branch {
            cond: Expr::Lit(Prim::Bool(true)),
            then_: case_block,
            else_: skip_to,
            span: None,
        });
        builder.add_edge(d, case_block, EdgeKind::IfTrue);
        builder.add_edge(d, skip_to, EdgeKind::IfFalse);
        if let Some(&next) = later_dispatch.get(i) {
            builder.start_block(next);
        }
    }

    // Lower each case's body into its own block. `break` is the generic
    // statement arm now that the switch pushes a frame, so a guarded
    // `if (x) break;` reaches the exit too. A case that runs off its end falls
    // through to the next one, as JavaScript does.
    builder.push_loop(exit_block, None);
    for (i, case) in sw.cases.iter().enumerate() {
        builder.start_block(case_blocks[i]);
        for stmt in &case.consequent {
            if builder.is_terminated() {
                break;
            }
            lower_stmt(stmt, builder);
        }
        if !builder.is_terminated() {
            let fallthrough = case_blocks.get(i + 1).copied().unwrap_or(exit_block);
            let b = builder.seal_with(Terminator::Jump(fallthrough));
            builder.add_edge(b, fallthrough, EdgeKind::Unconditional);
        }
    }
    builder.pop_loop();
```

- Le discriminant est évalué une fois en `ExprStmt(disc, None)` (L776-777) ;
  les expressions de `case k:` (les tests) **ne sont pas abaissées** (leurs
  lectures sont perdues ; en pratique des constantes).
- Les dispatchs portent la condition littérale `true` avec `span: None` ; elle
  est « opaque » **parce que** le moteur ne narrowe pas sur un littéral
  (les deux successeurs reçoivent l'environnement) — le commentaire dit
  « opaque », le code écrit `true` ; un lecteur qui évaluerait la condition
  concrètement élaguerait à tort la branche `else`. `lower_try` utilise, lui,
  `SummaryVal(Top)`.
- `default` est sur-approximé comme optionnel ; sa position dans la liste
  n'est pas traitée spécialement (un `default` au milieu est dispatché dans
  l'ordre textuel).
- Fall-through : une case qui finit sans `break` saute dans la suivante
  (`Unconditional`) — exactement la sémantique JS.
- `break` dans un case → frame `push_loop(exit_block, None)` ; `continue` saute
  le frame du switch jusqu'à la boucle englobante.
- Ordre d'allocation (`cfg_builder.rs:774-784`) : `exit_block` est réservé
  **avant** l'abaissement du discriminant, puis les blocs de case, puis les
  dispatchs suivants — d'où `B1` = sortie dans l'exemple 6.4. Un `switch`
  sans aucune case scelle simplement `Jump(exit)` (L779-784).
- Le frame du `switch` ne porte jamais de label : `LabeledStatement` n'honore
  un label que sur une boucle (L366-377), donc `sw: switch (k) { … break sw; }`
  donne un `break` sans cible → `Unreachable`.
- Chaque case a son propre bloc, mais la boucle de `lower_stmt` sur
  `case.consequent` est **recopiée** (L826-831) au lieu d'appeler
  `lower_stmts` : même arrêt au premier terminateur.

### 4.6 `try / catch / finally`

Commentaire de conception complet : `cfg_builder.rs:470-491`. Le cœur :

```rust
// src/lowering/cfg_builder.rs:492-542
fn lower_try(tr: &TryStatement, builder: &mut BlockBuilder) {
    let span = builder.span_at(tr.span.start);
    let try_block = builder.new_block();
    // With no handler, a throw runs the finalizer and keeps propagating, so the
    // throwing arm goes straight there.
    let catch_block = builder.new_block();
    let after = builder.new_block();

    let branch = builder.seal_with(Terminator::Branch {
        cond: Expr::SummaryVal(crate::ir::expr::SummaryValue::Top),
        then_: try_block,
        else_: catch_block,
        span,
    });
    builder.add_edge(branch, try_block, EdgeKind::IfTrue);
    builder.add_edge(branch, catch_block, EdgeKind::IfFalse);

    builder.start_block(try_block);
    lower_stmts(&tr.block.body, builder);
    if !builder.is_terminated() {
        let b = builder.seal_with(Terminator::Jump(after));
        builder.add_edge(b, after, EdgeKind::Unconditional);
    }

    builder.start_block(catch_block);
    if let Some(handler) = &tr.handler {
        // Bind the catch param (`catch (e)`) so `compute_free_vars` doesn't see
        // it as a component-scope capture. The thrown value is unknowable → Top.
        if let Some(param) = &handler.param {
            let pspan = builder.span_at(param.span.start);
            lower_binding_pattern(
                &param.pattern,
                Expr::SummaryVal(crate::ir::expr::SummaryValue::Top),
                pspan,
                builder,
            );
        }
        lower_stmts(&handler.body.body, builder);
    }
    if !builder.is_terminated() {
        let b = builder.seal_with(Terminator::Jump(after));
        builder.add_edge(b, after, EdgeKind::Unconditional);
    }

    // The finalizer sits on the join, which both arms reach: in JS a `finally`
    // always runs.
    builder.start_block(after);
    if let Some(finalizer) = &tr.finalizer {
        lower_stmts(&finalizer.body, builder);
    }
}
```

Schéma (repris du commentaire de l'issue #2) :

```
        ┌── Branch(⊤) ──┐
        ▼               ▼
    [try body]     [catch body]
        └──────┬────────┘
               ▼
        [finally body]
```

Divergence documentée (L486-491) : un `return` dans le `try` scelle son bloc,
donc ce chemin ne passe pas par le `finally` (en JS, il y passerait d'abord) ;
coût = force `must` seulement (Error → Warning). Divergence **non documentée**,
observée : la branche `catch` part de l'état **d'avant** le `try` ; les effets
partiels du `try` avant l'exception n'atteignent pas le `catch` (8.1.3).

### 4.7 Retours, fall-through, `throw` (ADR-025)

- `return e` → `Return(e)` ; `return;` → `Return(Lit(Unit))`.
- Corps qui « tombe du bout » → `into_cfg` scelle `Return(Lit(Unit))`
  (4.1). Test : `fall_through_tail_returns_undefined` (`cfg_builder.rs:1320-1331`).
- `throw e` → `ExprStmt(e)` + `Unreachable` (`cfg_builder.rs:355-359`). Test :
  `throw_stays_unreachable` (L1336-1343).
- `break` orphelin → `Unreachable` (`jump_out`, L591-601 ; test L1373-1380).
- Corps de fonction absent (déclaration ambiante) → `empty_cfg()`
  (`expr_lower.rs:1213-1228`), un bloc `Unreachable` sans instruction.
- Le splice réécrit chaque `Return(e)` du callé en `[let bound = e;]
  Jump(join)` et **laisse `Unreachable` tel quel** (`src/ir/splice.rs:158-184`).

### 4.8 Déclarations de fonctions et de classes

```rust
// src/lowering/cfg_builder.rs:398-418
        // Hoisted declarations: bind name but emit no CFG node
        Statement::FunctionDeclaration(func) => {
            if let Some(id) = &func.id {
                let ctx = builder.ctx.clone();
                let (params, body_cfg) = if let Some(body) = func.body.as_deref() {
                    build_fn_body_cfg(&func.params, body, &ctx)
                } else {
                    (vec![], empty_cfg())
                };
                let expr_id = builder.next_expr_id();
                builder.push_stmt(Stmt::Let {
                    var: id.name.to_string(),
                    rhs: Expr::FnLit {
                        id: expr_id,
                        params,
                        body_cfg: std::sync::Arc::new(body_cfg),
                    },
                    span: builder.span_at(func.span.start),
                });
            }
        }
```

Le commentaire dit « Hoisted declarations: bind name but emit no CFG node »,
mais le code émet un `Let` **à la position textuelle** : le hissage JS n'est
pas modélisé (8.1.4). Noter aussi l'ordre des ids : le corps imbriqué est
abaissé **avant** `next_expr_id()`, donc ses propres sites d'allocation ont
des ids inférieurs à celui du `FnLit` englobant (idem pour `lower_class`).

Classes (`lower_class`, `expr_lower.rs:58-135`) : une classe devient un
`ObjectLit` dont les membres sont rangés sous des **clés synthétiques**
(`[method]N`, `[field]N`, `[accessor]N`, `[static]N`, `[extends]N`) qu'aucun
`FieldAccess` réel ne peut atteindre ; méthodes et blocs `static` deviennent des
`FnLit`, initialiseurs de champs abaissés en place, clés calculées lues « pour
effet ». Motif : #77 (`class X { m() { setC(1) } }` se lisait comme un
composant qui n'écrit jamais `c`).

### 4.9 Motifs de liaison (`lower_binding_pattern`) et cibles d'affectation

Principe : un **temporaire** reçoit la source, puis chaque sous-motif est lié à
une projection de ce temporaire, récursivement.

```rust
// src/lowering/cfg_builder.rs:869-894
        BindingPattern::ArrayPattern(arr) => {
            let temp = format!("__arr_{}", arr.span.start);
            builder.push_stmt(Stmt::Let {
                var: temp.clone(),
                rhs,
                span,
            });
            for (i, elem) in arr.elements.iter().enumerate() {
                let Some(elem) = elem else { continue };
                let elem_rhs = Expr::IndexAccess {
                    arr: Box::new(Expr::Var(temp.clone())),
                    idx: Box::new(Expr::Lit(Prim::Int(i as i32))),
                };
                // Each sub-pattern sits somewhere; passing `None` here made a
                // default's side effects report no line at all (#131).
                let elem_span = builder.span_at(elem.span().start).or(span);
                lower_binding_pattern(elem, elem_rhs, elem_span, builder);
            }
            // `const [a, ...rest] = xs`: bind `rest` to the source — the same
            // sound over-approximation the object pattern uses for its rest.
            // Leaving it unbound loses forwarded setters.
            if let Some(rest) = &arr.rest {
                let rest_span = builder.span_at(rest.span.start).or(span);
                lower_binding_pattern(&rest.argument, Expr::Var(temp.clone()), rest_span, builder);
            }
        }
```

Motif objet (`cfg_builder.rs:895-937`) : `__obj_{offset}` ; clé statique ou
chaîne → `FieldAccess(temp, clé)` ; clé calculée `[k]` → `k` abaissé en
`ExprStmt` et la valeur liée à ⊤ ; reste `...rest` → lié au temporaire
lui-même (« rest has a subset of the fields, each with the same value »).
Une clé **numérique** (`const { 0: first } = o`) n'est ni
`StaticIdentifier` ni `StringLiteral` : elle passe par le bras « calculé »
(`other.as_expression()` rend le littéral), est émise en `ExprStmt` et la
liaison reçoit ⊤ (sound, imprécis). Chaque sous-liaison prend le span de sa
propriété (`prop.span`, repli sur celui du motif) ; le `Let` du temporaire
prend le span passé par l'appelant (celui du déclarateur, ou `None` pour un
paramètre, 4.10).

Défaut `x = d` :

```rust
// src/lowering/cfg_builder.rs:938-946
        BindingPattern::AssignmentPattern(ap) => {
            // The default value is not modeled (the binding keeps `rhs`), but
            // it *is* evaluated when the source is undefined — emit it so its
            // reads and side effects survive (`{ cb = () => setX(1) }`).
            let default_span = builder.span_at(ap.right.span().start).or(span);
            let default = lower_expr(&ap.right, builder);
            builder.push_stmt(Stmt::ExprStmt(default, default_span));
            lower_binding_pattern(&ap.left, rhs, span.or(default_span), builder);
        }
```

Le défaut est émis **inconditionnellement** (sur-approximation : il s'exécute
en vrai seulement si la source vaut `undefined`) et la liaison garde la
projection (la valeur du défaut ne rejoint pas celle de la liaison — voir
8.2).

Cibles d'**affectation** (`[a, b] = xs`, `({a, ...r} = o)`) :
`lower_assignment_target` (`expr_lower.rs:1030-1135`) est le miroir de
`lower_binding_pattern` mais émet des `Assign` (rebinding) au lieu de `Let`,
avec un temporaire `__dstr_{offset}` ; les cibles membres imbriquées
(`[obj.f] = xs`) deviennent des `MemberWrite` ; une cible non reconnue émet au
moins le RHS en `ExprStmt`. Invariant écrit (L1027-1029) : « Every leaf
identifier is written on every path … leaving a variable at its previous
abstract value falsifies it. » Les sous-instructions générées par cette voie
ont souvent `span: None` (observé : `i := __dstr_204[0]` sans position,
exemple 6.7).

Les quatre fonctions de la voie « affectation » (`expr_lower.rs:972-1159`) :

| Fonction | Rôle | Cas non couverts |
|---|---|---|
| `assign_target_ident` (L972-977) | `Some(nom)` ssi la cible est un `AssignmentTargetIdentifier` | tout le reste → `None`, **y compris** une cible TS-enveloppée (`(x as any) = …`, `x! = …`, `(<T>x) = …`, `x satisfies T = …`) |
| `assign_target_member` (L981-1001) | cible membre → `(obj, MemberKey)` : `o.f` → `Field("f")`, `o[i]` → `Index(i)` (objet puis index abaissés), `o.#p` → `Field("#p")` | autre → `None` |
| `lower_member_target_expr` (L1005-1021) | même chose pour une **expression** en position d'écriture — seul appelant : `delete o.f` / `delete o[k]` (L205-216) | `delete o.#p` et `delete x` → pas de `MemberWrite`, l'opérateur tombe dans `UnaryOp::Unknown` |
| `lower_assignment_target` (L1030-1135) | miroir de `lower_binding_pattern` ; identifiant → `Assign` ; motifs → `Let __dstr_{offset}` puis récursion ; propriété raccourcie `({ a = d } = o)` → `ExprStmt(d)` puis `a := __dstr.a` ; reste → `Var(__dstr)` ; cible membre imbriquée → `MemberWrite` | cible non reconnue (TS-enveloppée) → `ExprStmt(rhs)` **sans écriture** (L1130-1132) |
| `lower_assignment_maybe_default` (L1140-1156) | élément/propriété avec défaut (`[a = 1] = xs`) : `ExprStmt(défaut, None)` puis `lower_assignment_target(binding, rhs, None)` | — |

Cible TS-enveloppée : **écart vérifié**. `(x as any) = { a: 1 }` passe par
`assign_target_ident` (`None`), `assign_target_member` (`None`), puis
`lower_assignment_target` dont le bras de repli émet seulement
`ExprStmt(rhs)` : `x` garde sa valeur précédente, exactement la « stale
binding » que l'invariant L1027-1029 interdit. Le commentaire du repli
(« A TS-wrapped or otherwise unrecognised target. Emitting the RHS keeps its
reads; the cell it writes is untracked either way », L1130-1131) est faux
pour une cible TS-enveloppée dont l'intérieur est un identifiant : cette
cellule-là **est** suivie par l'environnement. Dump
(`/tmp/verif02/ex/v1_ts_target.tsx`, composant `TsTarget`) :

```
      let x = 1  @4:6
      expr {#0 a: 1}  @5:2
      expr {#0 a: 1}  @5:2
      expr HookMarker(0, Undefined)  @6:2
```

(le même `ObjectLit #0` apparaît deux fois : une fois émis par le repli, une
fois comme valeur de l'instruction-expression). Analyseur : `TsTarget ✓ …
verified always-unstable-deps` alors que le témoin `PlainTarget`
(`x = { a: 1 }` sans `as`) donne `warn always-unstable-deps [hook:0] (line
13:2)`. Même comportement pour `x! = { a: 1 }` (vérifié,
`/tmp/verif02/ex/v2_memo_handler.tsx`, composant `NonNull`) et, par le même
chemin, pour `(x as any)++` (bras `_ => opaque()` de l'`UpdateExpression`,
4.11.3). Voir 8.1.13.

### 4.10 Préambule des paramètres et flèches concises

```rust
// src/lowering/cfg_builder.rs:952-1001
pub(super) fn inject_param_preamble(
    params: &FormalParameters,
    builder: &mut BlockBuilder,
) -> Vec<String> {
    let mut names = Vec::new();
    for (i, p) in params.items.iter().enumerate() {
        match &p.pattern {
            BindingPattern::BindingIdentifier(id) => {
                names.push(id.name.to_string());
            }
            other => {
                let temp = format!("__p{}", i);
                names.push(temp.clone());
                lower_binding_pattern(other, Expr::Var(temp), None, builder);
            }
        }
    }
    names
}

/// Build a function body CFG with param destructuring preamble.
pub fn build_fn_body_cfg(
    params: &FormalParameters,
    body: &FunctionBody,
    ctx: &LowerCtx,
) -> (Vec<String>, CFG) {
    let mut builder = BlockBuilder::new(ctx);
    builder.start_block(0);
    let param_names = inject_param_preamble(params, &mut builder);
    lower_stmts(&body.statements, &mut builder);
    (param_names, builder.into_cfg(0))
}

/// Like [`build_fn_body_cfg`] for concise arrow bodies (`x => expr`).
pub fn build_expr_fn_body_cfg(
    params: &FormalParameters,
    body: &FunctionBody,
    ctx: &LowerCtx,
) -> (Vec<String>, CFG) {
    let mut builder = BlockBuilder::new(ctx);
    builder.start_block(0);
    let param_names = inject_param_preamble(params, &mut builder);
    if let Some(Statement::ExpressionStatement(es)) = body.statements.first() {
        let expr = lower_expr(&es.expression, &mut builder);
        if !builder.is_terminated() {
            builder.seal_with(Terminator::Return(expr));
        }
    }
    (param_names, builder.into_cfg(0))
}
```

Observations :

- Paramètre simple `x` : **aucun** `Let` (le paramètre est lié par
  l'appelant : splice `let renamed_param = arg`, ou environnement initial).
- Paramètre déstructuré : nom `__p{i}` + motif ; le `Let __obj_N = __p0` de
  tête est émis **sans span** (argument `None`, L965) — observé dans tous les
  exemples (`let __obj_56 = __p0` sans `@`). Les sous-liaisons, elles, ont un
  span (celui de leur sous-nœud).
- Paramètre de reste `(...args)` : `params.rest` n'est **pas** lu par
  `inject_param_preamble` (seul `params.items` est parcouru) ; `args` n'est
  donc pas un paramètre déclaré. Vérifié (`/tmp/ex/e20_rest_param.tsx`) : la
  flèche `(...args) => args.length` devient `fn#1()` sans paramètre, et un
  `args` de portée composant est alors lu comme capture — l'analyseur émet
  `warn missing-deps var:args` sur un effet qui ne lit que son paramètre
  (8.1.11).
- Un paramètre avec défaut `(x = 1)` est un `AssignmentPattern` → nom `__p{i}`.
- Flèche concise : l'expression devient le `Return`. oxc stocke `x => expr`
  comme un `FunctionBody` d'un seul `ExpressionStatement`, indiscernable de
  `x => { expr; }` sauf par le drapeau `expression` (#5, `tests/concise_arrow_bodies.rs:1-10`).

### 4.11 Abaissement des expressions (`lower_expr`, `expr_lower.rs:143-615`)

Contrat : « Branching expressions (ternary, `&&`, `||`, `??`) split `builder`
into new blocks and return a `Var` temp. All other expressions lower
structurally. » (L139-142). Ordre d'évaluation : gauche → droite, callee avant
arguments, objet avant clé, **enfants JSX avant props** (L801-806).

Deux outils transverses :

```rust
// src/lowering/expr_lower.rs:18-36
/// An opaque value of unknown kind (⊤). Used for expressions the analysis does
/// not model (exotic operators, `this`/`super`, unhandled syntax). A typed
/// sentinel rather than a magic `Expr::Var("__opaque")`: it evaluates directly
/// to `StateValue::top()` instead of relying on a name lookup missing in the
/// env, so a real user variable can never collide with it and it never shows up
/// as a spurious free-variable capture.
fn opaque() -> Expr {
    Expr::SummaryVal(SummaryValue::Top)
}

/// Lower an expression the enclosing composite cannot represent, keeping it as
/// a statement so its reads and side effects stay visible. Dropping a
/// sub-expression outright is not an over-approximation: the deps rules consume
/// a missing read as "this variable is not used" — a claim, not ignorance.
fn lower_for_effect(expr: &Expression, builder: &mut BlockBuilder) {
    let span = builder.span_at(expr.span().start);
    let lowered = lower_expr(expr, builder);
    builder.push_stmt(Stmt::ExprStmt(lowered, span));
}
```

#### 4.11.1 Littéraux, identifiants, `this`

Booléens, `null`, nombres, chaînes → `Lit` ; gabarit `` `q0${e0}q1` `` →
chaîne d'additions gauche-associées `"q0" + e0 + "q1"` (L156-184, observé :
`` `n=${i}` `` → `(("n=" Add i) Add "")`) ; `undefined` → `Lit(Unit)` ;
identifiant → `Var` ; `this`/`super` → ⊤.

#### 4.11.2 Opérateurs

`BinaryExpression` → `BinOp` via `lower_binop` exhaustif (L1181-1209, « No
wildcard ») ; `==`/`===` confondus en `Eq`, `!=`/`!==` en `Neq`.
`UnaryExpression` : `-`→`Neg`, `!`→`Not`, `~`→`BitNot`, `typeof`→`TypeOf`,
`+`→`Plus`, `void e` → `ExprStmt(e)` + `Lit(Unit)`, `delete o.f` →
`MemberWrite{obj:o, key, rhs: Lit(Unit)}` + `Lit(Bool(true))` (L204-216), autre
→ `Unknown` (le seul opérateur restant est `delete` sur une non-membre :
`delete x`, `delete o.#p`, `delete o?.f` → `UnaryOp{Unknown, arg}` sans
`MemberWrite`). Accès membres (L336-349) : `o.f` → `FieldAccess`, `o[k]` →
`IndexAccess`, champ privé `o.#p` → `FieldAccess{field: "#p"}` (le `#` est
gardé dans le nom, donc aucune collision avec une propriété publique `p`).

#### 4.11.3 `++` / `--`

```rust
// src/lowering/expr_lower.rs:256-277
        // `i++` / `--i`: emit the write (`i = i ± 1`), then yield the variable.
        // Prefix/postfix differ only in the *value* expression (new vs old); we
        // return the post-write `Var` for both — a sound approximation for the
        // numeric domain (the rare `a = i++` over-counts by one, never under).
        Expression::UpdateExpression(upd) => match &upd.argument {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
                let name = id.name.to_string();
                let op = match upd.operator {
                    UpdateOperator::Increment => IrBinOp::Add,
                    UpdateOperator::Decrement => IrBinOp::Sub,
                };
                builder.push_stmt(Stmt::Assign {
                    var: name.clone(),
                    rhs: Expr::BinOp {
                        op,
                        lhs: Box::new(Expr::Var(name.clone())),
                        rhs: Box::new(Expr::Lit(Prim::Int(1))),
                    },
                    span: builder.span_at(upd.span.start),
                });
                Expr::Var(name)
            }
```

(Remarque critique pour le manuscrit : « over-counts by one, never under » est
une affirmation sur la valeur **concrète** ; dans le domaine abstrait, la
valeur de `a = i++` est l'intervalle de `i+1` et non celui de `i`. La
sur-approximation n'est vraie que si l'on raisonne en borne supérieure ; pour
un test d'égalité exacte, c'est une valeur différente. À vérifier dans le
chapitre domaines.) Sur un membre (`o.f++`, `a[i]--`) : `MemberWrite` de ⊤ et
valeur ⊤ (L278-299). Toute autre cible (`(x as any)++`, `x!++`, `o.#p++`) tombe
dans le bras `_ => opaque()` (L301) : valeur ⊤ et **aucune écriture** —
pour `x!++`, `x` garde sa valeur antérieure (même famille que 8.1.13 ;
vérifié sur `/tmp/verif02/ex/v3_update.tsx` : `let x = 1; x!++; (x as any)++;`
donne `let x = 1`, `expr ⊤`, `expr ⊤` et le rendu lit toujours `x = 1`).

#### 4.11.4 Appels, `new`, arguments spread

```rust
// src/lowering/expr_lower.rs:617-647
/// Lower a call/`new` argument list. A `...spread` argument cannot keep a
/// position — parameters bind positionally when a callee is inlined — so it is
/// emitted as a statement instead of guessed into a slot, which preserves its
/// reads without claiming which parameter receives it.
fn lower_arguments(arguments: &[Argument], builder: &mut BlockBuilder) -> Vec<Expr> {
    let mut args = vec![];
    for a in arguments {
        match a {
            Argument::SpreadElement(sp) => lower_for_effect(&sp.argument, builder),
            other => {
                if let Some(e) = other.as_expression() {
                    args.push(lower_expr(e, builder));
                }
            }
        }
    }
    args
}

fn lower_call(call: &CallExpression, builder: &mut BlockBuilder) -> Expr {
    let fn_ = lower_expr(&call.callee, builder);
    let args = lower_arguments(&call.arguments, builder);
    let call_expr = Expr::Call {
        fn_: Box::new(fn_),
        args,
    };
    match &call.type_arguments {
        Some(params) if !params.params.is_empty() => Expr::TSAnnotated(Box::new(call_expr)),
        _ => call_expr,
    }
}
```

`useState<T>(x)` produit donc `TSAnnotated(Call …)`, que
`try_consume_hook_call` regarde à travers. `new X(a)` → `Expr::New { id, fn_,
args }` (L313-322 ; nœud allouant introduit par `e67b10a` pour #158, issue
encore marquée OPEN sur le tracker au 2026-09-28). Gabarit étiqueté
`` tag`…${e}…` `` → `Call(tag, [e…])` (les chaînes cuites ne sont pas passées).

#### 4.11.5 Affectations

```rust
// src/lowering/expr_lower.rs:493-518
        Expression::AssignmentExpression(assign) => {
            let rhs_val = lower_expr(&assign.right, builder);
            match assign_target_ident(&assign.left) {
                Some(name) => {
                    // Reconstruct `x op= e` → `x = x op e`, but only for operators
                    // `lower_binop` maps faithfully (Add/Sub/Mul/Div). Other
                    // compounds (%=, **=, bitwise, logical) would alias onto the
                    // wrong IR op → unsound; havoc the target to Top instead.
                    let rhs = if assign.operator.is_assign() {
                        rhs_val
                    } else if let Some(op) = faithful_compound_binop(assign.operator) {
                        Expr::BinOp {
                            op,
                            lhs: Box::new(Expr::Var(name.clone())),
                            rhs: Box::new(rhs_val),
                        }
                    } else {
                        opaque()
                    };
                    builder.push_stmt(Stmt::Assign {
                        var: name.clone(),
                        rhs,
                        span: builder.span_at(assign.span.start),
                    });
                    Expr::Var(name)
                }
```

Le commentaire interne est **périmé** : `faithful_compound_binop`
(L1161-1179) couvre maintenant `+ - * / % ** & | ^ << >> >>>` ; seuls les
composés logiques `&&=`, `||=`, `??=` retombent sur ⊤ (tests
`exotic_compound_havocs_target`, `mod_and_pow_compound_assignments_are_faithful`,
`bitwise_compound_assignment_is_faithful`). Noter aussi que le RHS est
abaissé **avant** la lecture de la cible (pour `x op= e`, JS lit `x` d'abord ;
sans effet ici car la lecture est reconstruite symboliquement). Cible membre
→ `MemberWrite` (composé → rhs ⊤) ; cible motif → `lower_assignment_target`
(4.9).

#### 4.11.6 Séquence, `?.`, TS

- `(a, b, c)` : `a`, `b` en `ExprStmt`, valeur `c` (L550-566).
- Chaînage optionnel :

```rust
// src/lowering/expr_lower.rs:649-657
/// `a?.b` / `a?.[i]` / `f?.(x)` — optional chaining lowers like its
/// non-optional counterpart. The nullish short-circuit needs no separate
/// branch: a `Loc`-bound receiver is an object literal on every path (never
/// nullish), and any other receiver already evaluates the access to ⊤,
/// which covers the `undefined` outcome.
fn lower_chain_element(elem: &ChainElement, builder: &mut BlockBuilder) -> Expr {
    match elem {
        ChainElement::CallExpression(call) => lower_call(call, builder),
        ChainElement::TSNonNullExpression(ts) => lower_expr(&ts.expression, builder),
```

  L'argument vaut pour la **valeur** ; pour les **effets**, `f?.(x)` est
  abaissé en appel inconditionnel : si `f` est nul, l'appel (et ses
  arguments, éventuellement effectifs) ne s'exécute pas en vrai. Cela peut
  seulement ajouter des chemins (faux positifs possibles), jamais en retirer.
- `x as T` → `TSAnnotated(x)` ; `x!`, `x satisfies T`, `<T>x`, parenthèses →
  transparents.

#### 4.11.7 Littéraux objet et tableau (spreads, clés calculées, accesseurs)

```rust
// src/lowering/expr_lower.rs:352-394
        Expression::ObjectExpression(obj) => {
            let id = builder.next_expr_id();
            let mut fields: Vec<(String, Expr)> = vec![];
            for prop in &obj.properties {
                match prop {
                    ObjectPropertyKind::ObjectProperty(p) => {
                        let key = match &p.key {
                            // A getter/setter runs code on every read: `o.x`
                            // is whatever the body returns, not the function
                            // literal sitting in the property. Kept under a
                            // synthetic key, like a spread — the body stays
                            // visible, the name resolves to nothing.
                            _ if p.kind != PropertyKind::Init => {
                                synthetic_key(builder, "[accessor]")
                            }
                            PropertyKey::StaticIdentifier(ident) => ident.name.to_string(),
                            PropertyKey::StringLiteral(s) => s.value.to_string(),
                            // Computed key (`{ [k]: v }`): the key expression
                            // runs, and `v` is still in the object — under a
                            // synthetic name, since the real one is unknown.
                            other => {
                                if let Some(e) = other.as_expression() {
                                    lower_for_effect(e, builder);
                                }
                                synthetic_key(builder, "[computed]")
                            }
                        };
                        let value = lower_expr(&p.value, builder);
                        fields.push((key, value));
                    }
                    // `{ ...opts }` forwards every one of `opts`' fields. Keep
                    // it under a synthetic key exactly as JSX spread does —
                    // dropping it lost the read of `opts` and any setter it
                    // carries.
                    ObjectPropertyKind::SpreadProperty(sp) => {
                        let key = synthetic_key(builder, SPREAD_KEY_PREFIX);
                        let value = lower_expr(&sp.argument, builder);
                        fields.push((key, value));
                    }
                }
            }
            Expr::ObjectLit { id, fields }
        }
```

`synthetic_key` (L42-44) consomme un `ExprId` pour suffixer la clé
(`"...4"`, `"[computed]7"`) — les clés synthétiques « mangent » des ids du
compteur d'allocation. Raccourci `{ a }` → `("a", Var(a))` (clé statique).
Clé numérique `{ 1: x }` → passe par le bras « calculé » (seuls
`StaticIdentifier` et `StringLiteral` sont nommés) : la valeur est rangée
sous `[computed]N`.

Tableaux (L395-439) : `elems` = éléments visibles, `arity` = `Exact(n)` sauf
spread (`AtLeast(n)`), `spread_at` = positions des spreads. Une élision
compte dans `entries` mais n'a pas d'élément (#104, #118).

#### 4.11.8 Fonctions

```rust
// src/lowering/expr_lower.rs:442-457
        Expression::ArrowFunctionExpression(arrow) => {
            let id = builder.next_expr_id();
            let ctx = builder.ctx.clone();
            // Concise body (`x => expr`) carries an implicit return; block body
            // (`x => { ... }`) lowers like any function body.
            let (params, body_cfg) = if arrow.expression {
                build_expr_fn_body_cfg(&arrow.params, &arrow.body, &ctx)
            } else {
                build_fn_body_cfg(&arrow.params, &arrow.body, &ctx)
            };
            Expr::FnLit {
                id,
                params,
                body_cfg: Arc::new(body_cfg),
            }
        }
```

Ici l'id est pris **avant** le corps (contrairement aux déclarations, 4.8).
Aucune notion de fermeture n'est stockée : les captures sont retrouvées plus
tard par `ir::free_vars::compute_free_vars` (`src/ir/free_vars.rs:92`) sur le
corps.

#### 4.11.9 JSX

```rust
// src/lowering/expr_lower.rs:808-851
    if name.chars().next().is_some_and(|c| c.is_uppercase()) || name.contains('.') {
        // React semantics: nested JSX children ARE `props.children`.
        // Dropping them here would erase the whole subtree from the CFG
        // (`<Dialog><Select onValueChange={setX}/></Dialog>` — the Select
        // would never be visited, its escaping setter never havocked).
        let mut props = props;
        if !children.is_empty()
            && let Expr::ObjectLit { fields, .. } = &mut props
        {
            let id = builder.next_expr_id();
            fields.push((
                "children".to_string(),
                Expr::ArrayLit {
                    id,
                    // Synthetic container, not a written array literal: it
                    // stands for the children the walk kept, so it claims only
                    // a lower bound on anything in the source.
                    arity: Arity::AtLeast(children.len()),
                    elems: children,
                    spread_at: vec![],
                },
            ));
        }
        let span = builder.span_at(jsx.opening_element.span.start);
        // Who `<Name/>` refers to is settled by this file's imports and its own
        // declarations, and nowhere else — resolving it later, from the bare
        // name, is how a same-named component in an unrelated file came to be
        // inlined in its place (#7).
        let origin = builder.ctx.jsx.get(&name).cloned();
        Expr::CompApp {
            name,
            props: Box::new(props),
            span,
            origin,
        }
    } else {
        Expr::NativeElem {
            tag: name,
            props: Box::new(props),
            children,
            span: builder.span_at(jsx.opening_element.span.start),
            prop_spans,
        }
    }
```

- Majuscule initiale **ou** nom pointé (`<X.Provider>`, `<motion.div>`) →
  `CompApp` (les enfants deviennent `props.children`) ; sinon `NativeElem`
  (enfants à part). Nota : `<motion.div>` est donc un `CompApp`.
- Props : `ObjectLit` ; attribut sans valeur → `Lit(true)` ; spread
  `{...p}` → clé `"...N"` (L892-902) ; nom namespacé `a:b` conservé.
- `prop_spans` n'enregistre que les props `onX` (`is_event_prop_key`, `o`,
  `n`, puis une majuscule ASCII).
- Enfants : texte ignoré, `{e}` abaissé, `{...items}` → `items` (L937-950),
  `{/* */}` (expression vide) → rien.
- Fragment `<>…</>` → `NativeElem{tag:"Fragment", props: ObjectLit vide}`
  (`lower_jsx_fragment`, L952-966 : l'`ExprId` du props vide est pris
  **avant** les enfants, `prop_spans` vide, span = début du fragment). Un
  `<React.Fragment>` explicite, lui, a un nom pointé → `CompApp`.
- Nom d'élément (`jsx_element_name`, L915-925 ; `jsx_member_obj_name`,
  L927-935) : identifiant → son nom ; membre → chemin pointé récursif
  (`a.b.C`) ; nom namespacé → `ns:nom` (minuscule → `NativeElem`, p. ex.
  `svg:rect`) ; `this` → `"this"` et `<this.X>` → `"this.X"` (pointé →
  `CompApp`). Le test de majuscule est `char::is_uppercase` (Unicode), pas
  seulement ASCII.
- Valeur d'attribut (`lower_jsx_props`, L856-906) : chaîne → `Lit(String)` ;
  `{e}` → `lower_expr(e)` ; `{}` / `{/* */}` (expression vide) → `Lit(Unit)` ;
  élément ou fragment JSX en valeur (`<X icon=<Y/> />`) → abaissé
  récursivement. L'`ExprId` de l'objet de props est pris **avant** les
  valeurs ; chaque spread d'attribut consomme en plus un id pour sa clé
  `...N`.
- **Ordre d'évaluation** : `lower_jsx_element` abaisse les **enfants avant
  les props** (L801-806), alors que JavaScript (`jsx(type, props)` /
  `createElement`) évalue les attributs d'abord. Sans effet sur les valeurs
  (pas d'effet de bord ordonné modélisé), mais visible dans l'ordre des
  `ExprId` et des instructions émises par les enfants (p. ex. un ternaire
  dans un enfant fend les blocs **avant** celui d'une prop).

#### 4.11.10 `await`, `yield`, repli

```rust
// src/lowering/expr_lower.rs:577-606
        Expression::AwaitExpression(aw) => {
            let value = lower_expr(&aw.argument, builder);
            // The awaited expression is evaluated BEFORE the suspension:
            // `await fetch(u)` calls `fetch` synchronously and only then
            // yields. Returning it here would carry it into the enclosing
            // statement, which the split moves into the post-await block — so
            // the call would read as `deferred`, and `deferred` is a claim
            // ("never inside a React phase"), not an over-approximation. Bind
            // it on this side of the edge instead. A bare name or literal has
            // nothing to evaluate, so it is passed through unchanged rather
            // than given a binding no reader wants.
            let value = match value {
                v @ (Expr::Var(_) | Expr::Lit(_)) => v,
                v => {
                    let tmp = builder.fresh_temp();
                    // The binding is synthetic, its position is not: everything
                    // the walk finds under `await fetch(…)` takes its witness
                    // from the statement it sits in, and a spanless hoist made
                    // every such row report no line at all (#131).
                    builder.push_stmt(Stmt::Let {
                        var: tmp.clone(),
                        rhs: v,
                        span: builder.span_at(aw.argument.span().start),
                    });
                    Expr::Var(tmp)
                }
            };
            builder.split_at_await();
            value
        }
```

`yield e` → valeur de `e` (générateurs non modélisés, pas de coupure).
Repli `_ => opaque()` (L613) : d'après l'énumération `Expression` d'oxc 0.138
(`~/.cargo/registry/.../oxc_ast-0.138.0/src/ast/js.rs:81-165`), les variantes
qui y tombent sont `BigIntLiteral`, `RegExpLiteral`, `MetaProperty`
(`import.meta`, `new.target`), `ImportExpression` (`import(x)`),
`PrivateInExpression` (`#x in o`), `TSInstantiationExpression` (`f<T>`),
`V8IntrinsicExpression`. Pour les quatre dernières, les **lectures** de
sous-expressions sont perdues (8.1.6).

### 4.12 Diamants à court-circuit : `?:`, `&&`, `||`, `??`

Ternaire :

```rust
// src/lowering/expr_lower.rs:675-724
/// `a ? b : c` splits into three blocks:
///
///   current:  Branch(a, then, else)
///   then:     Let __tN = b; Jump(join)
///   else:     Let __tN = c; Jump(join)
///   join:     Var(__tN)   ← returned
///
/// Analysis correctly joins stability(b) ⊔ stability(c) at the join block.
fn lower_ternary(cond: &ConditionalExpression, builder: &mut BlockBuilder) -> Expr {
    let test = lower_expr(&cond.test, builder);
    let then_id = builder.new_block();
    let else_id = builder.new_block();
    let join_id = builder.new_block();
    let tmp = builder.fresh_temp();

    let span = builder.span_at(cond.test.span().start);
    let bid = builder.seal_with(Terminator::Branch {
        cond: test,
        then_: then_id,
        else_: else_id,
        span,
    });
    builder.add_edge(bid, then_id, EdgeKind::IfTrue);
    builder.add_edge(bid, else_id, EdgeKind::IfFalse);

    builder.start_block(then_id);
    let cons_span = builder.span_at(cond.consequent.span().start);
    let cons = lower_expr(&cond.consequent, builder);
    builder.push_stmt(Stmt::Let {
        var: tmp.clone(),
        rhs: cons,
        span: cons_span,
    });
    let t = builder.seal_with(Terminator::Jump(join_id));
    builder.add_edge(t, join_id, EdgeKind::Unconditional);

    builder.start_block(else_id);
    let alt_span = builder.span_at(cond.alternate.span().start);
    let alt = lower_expr(&cond.alternate, builder);
    builder.push_stmt(Stmt::Let {
        var: tmp.clone(),
        rhs: alt,
        span: alt_span,
    });
    let e = builder.seal_with(Terminator::Jump(join_id));
    builder.add_edge(e, join_id, EdgeKind::Unconditional);

    builder.start_block(join_id);
    Expr::Var(tmp)
}
```

Le ternaire lie le temporaire par **deux `Let`** (un par bras) ; les
court-circuits par un `Let` de l'opérande gauche **puis un `Assign`** dans le
bloc droit :

```rust
// src/lowering/expr_lower.rs:739-795
fn lower_logical(log: &LogicalExpression, builder: &mut BlockBuilder) -> Expr {
    let tmp = builder.fresh_temp();
    let span = builder.span_at(log.left.span().start);
    let left = lower_expr(&log.left, builder);
    builder.push_stmt(Stmt::Let {
        var: tmp.clone(),
        rhs: left,
        span,
    });

    let rhs_id = builder.new_block();
    let join_id = builder.new_block();

    let (then_, else_) = match log.operator {
        LogicalOperator::And => (rhs_id, join_id), // truthy → rhs; falsy → join (keep a)
        LogicalOperator::Or | LogicalOperator::Coalesce => (join_id, rhs_id), // truthy → join (keep a); falsy → rhs
    };

    let bid = builder.seal_with(Terminator::Branch {
        cond: Expr::Var(tmp.clone()),
        then_,
        else_,
        span,
    });
    builder.add_edge(
        bid,
        then_,
        if then_ == rhs_id {
            EdgeKind::IfTrue
        } else {
            EdgeKind::IfFalse
        },
    );
    builder.add_edge(
        bid,
        else_,
        if else_ == rhs_id {
            EdgeKind::IfFalse
        } else {
            EdgeKind::IfTrue
        },
    );

    builder.start_block(rhs_id);
    let right_span = builder.span_at(log.right.span().start);
    let right = lower_expr(&log.right, builder);
    builder.push_stmt(Stmt::Assign {
        var: tmp.clone(),
        rhs: right,
        span: right_span,
    });
    let r = builder.seal_with(Terminator::Jump(join_id));
    builder.add_edge(r, join_id, EdgeKind::Unconditional);

    builder.start_block(join_id);
    Expr::Var(tmp)
}
```

Pseudo-code des trois formes :

```
a && b :  Let t = a ; Branch(t, R, J)   R: t := b ; Jump J    J: … Var(t)
a || b :  Let t = a ; Branch(t, J, R)   R: t := b ; Jump J    J: … Var(t)
a ?? b :  identique à ||  (la nullité est approximée par la fausseté)
```

Points décisifs :

- `??` est traité **exactement comme `||`** : le CFG fait comme si `b` était
  évalué quand `a` est *falsy*, alors que JS ne l'évalue que quand `a` est
  *nullish*. Le problème n'est pas le graphe mais le **narrowing** que le
  moteur applique ensuite sur `Branch(Var(t))` : la branche `then_` (= la
  jonction pour `??`) reçoit `narrow_truthy(t)`, qui retire `0`, `""`,
  `false`, `null`, `undefined` (`src/domains/impls/state_value.rs:565-582`) ;
  or `0 ?? b` vaut `0` en JS et arrive à la jonction **sans** passer par le
  bloc `rhs`. La valeur `0` disparaît donc de `t`. **Sous-approximation
  vérifiée** (`/tmp/ex/e19_coalesce.tsx`) : `const z = 0 ?? 5; if (z === 0) {
  setX(1); }` est certifié `✓ … verified setter-in-render`, alors que le
  témoin `const z = 0;` donne `warn setter-in-render (line 16:4)`. Voir
  8.1.10.
- **Polarité des arêtes de jonction inversée** (vérifié sur dumps, 6.3) : pour
  `&&` les deux arêtes sortantes sont `IfTrue`, pour `||`/`??` les deux sont
  `IfFalse` (l'arête vers `rhs` est correcte ; celle vers `join` reçoit la même
  polarité). Latent aujourd'hui : `site_guards` ne lit la polarité que sur des
  chaînes à prédécesseur unique, et la jonction d'un diamant a toujours deux
  prédécesseurs ; `expand_guard` ne lit que l'arête vers `rhs`
  (`src/engine/guards.rs:1018-1029`). Voir 8.1.1.
- ADR-020 §1 fige le diamant (« do not introduce a flat `LogicalOp` node ») :
  il modélise l'exécution **conditionnelle des effets** de `b`.
- La valeur lue après coup est `Var(__tN)` : une instruction `x && f()` se
  termine par `ExprStmt(Var(__tN))` dans la jonction (test
  `logical_and_splits_blocks`).

### 4.13 Extraction des hooks (`extract_hooks`)

```rust
// src/lowering/hook_extractor.rs:264-304
/// Walk `cfg` in block-id order, extract top-level hook calls, and rewrite
/// affected statements in-place. Returns `(hooks, provenance, next_label)`;
/// `provenance` holds one row per extracted hook call (never for handlers).
/// Destructuring is resolved: `__arr_N[0]` → `StateVal(L)`, `__arr_N[1]` → `StateSetter(L)`.
pub fn extract_hooks(
    cfg: &mut CFG,
    imports: &ImportCtx<'_>,
) -> (Vec<HookEntry>, Vec<HookProvenance>, HookLabel) {
    let mut label: HookLabel = 0;
    let mut hooks: Vec<HookEntry> = Vec::new();
    let mut provenance: Vec<HookProvenance> = Vec::new();
    // Maps array-destructuring temps (e.g. "__arr_42") → hook label, for useState/useReducer.
    let mut state_temps: HashMap<String, HookLabel> = HashMap::new();

    // Collected up front because the loop takes `&mut cfg.blocks` — the ids
    // already come out in order (`CFG::blocks` is a `BTreeMap`).
    hoist_terminator_hooks(cfg, imports);

    let ids: Vec<BlockId> = cfg.blocks.keys().copied().collect();

    for id in ids {
        let old = std::mem::take(&mut cfg.blocks.get_mut(&id).unwrap().stmts);
        let mut new: Vec<Stmt> = Vec::with_capacity(old.len());

        for stmt in old {
            process_stmt(
                stmt,
                &mut new,
                &mut hooks,
                &mut provenance,
                &mut label,
                &mut state_temps,
                imports,
            );
        }

        cfg.blocks.get_mut(&id).unwrap().stmts = new;
    }

    (hooks, provenance, label)
}
```

Étapes :

1. **Normalisation des terminateurs** (#4) : tout `Return(e)`/`Branch{cond}`
   dont l'expression **contient** un appel de hook est remplacé par
   `Var(__term_{id})` et `Let __term_{id} = e` est ajouté en fin de bloc :

```rust
// src/lowering/hook_extractor.rs:321-358
fn hoist_terminator_hooks(cfg: &mut CFG, imports: &ImportCtx<'_>) {
    let ids: Vec<BlockId> = cfg.blocks.keys().copied().collect();
    for id in ids {
        let block = cfg.blocks.get_mut(&id).unwrap();
        let (expr, span) = match &mut block.term {
            Terminator::Return(e) => (e, None),
            Terminator::Branch { cond, span, .. } => (cond, *span),
            Terminator::Jump(_) | Terminator::Unreachable => continue,
        };
        if !contains_hook_call(expr, imports) {
            continue;
        }
        let var = format!("__term_{id}");
        let hoisted = std::mem::replace(expr, Expr::Var(var.clone()));
        block.stmts.push(Stmt::Let {
            var,
            rhs: hoisted,
            span,
        });
    }
}

/// Does this expression call a hook anywhere inside it?
///
/// Deliberately a *containment* test rather than "is a hook call": the whole
/// expression is what gets hoisted, so `return cond ? useA() : useB()` moves as
/// one piece and keeps its meaning. Conditional hook calls are the point of
/// `conditional-hook`, and it cannot fire on a hook nobody extracted.
fn contains_hook_call(expr: &Expr, imports: &ImportCtx<'_>) -> bool {
    if let Expr::Call { fn_, .. } = expr
        && imports.classify_callee(fn_).is_some()
    {
        return true;
    }
    let mut found = false;
    expr.for_each_child(&mut |child| found |= contains_hook_call(child, imports));
    found
}
```

   (Le commentaire de `contains_hook_call` évoque `return cond ? useA() :
   useB()` ; en réalité un ternaire dans un `return` a déjà été fendu en blocs
   par `lower_ternary`, le terminateur ne lit que `Var(__tN)` et les appels
   sont déjà dans des `Let` d'arms. Le hoist ne sert effectivement qu'aux
   appels **directs** ou **imbriqués** dans le terminateur ; et si l'appel est
   imbriqué, le `Let` hissé n'est pas reconnu par `try_consume_hook_call` —
   voir 8.1.2.)

2. **Parcours des blocs par id croissant**, et par ordre des instructions dans
   chaque bloc ; chaque `Stmt` passe par `process_stmt` :

```rust
// src/lowering/hook_extractor.rs:369-417
    match stmt {
        Stmt::Let {
            var,
            rhs,
            span: stmt_span,
        } => match try_consume_hook_call(rhs, imports) {
            Ok((call, args)) => {
                let lbl = *label;
                *label += 1;
                let is_state_like =
                    call.is_react && matches!(call.origin_name.as_str(), "useState" | "useReducer");
                let is_arr_temp = var.starts_with("__arr_");

                let entry = make_hook_entry(&call, lbl, args, stmt_span);
                let marker = hook_result_expr(&call, lbl, entry.as_ref());
                provenance.push(call.provenance(lbl, stmt_span));
                if let Some(entry) = entry {
                    hooks.push(entry);
                }

                // Record the binding variable on Custom hooks (their import
                // source and resolved file come from the classification).
                if let Some(HookEntry::Custom { binding, .. }) = hooks.last_mut() {
                    // `__arr_N` is a lowering temp, never a source name.
                    if !is_arr_temp {
                        *binding = Some(var.clone());
                    }
                }

                if is_state_like && is_arr_temp {
                    // Array-destructured useState/useReducer: drop the temp Let,
                    // subsequent IndexAccess stmts will be rewritten by rewrite_expr.
                    state_temps.insert(var, lbl);
                } else {
                    out.push(Stmt::Let {
                        var,
                        rhs: marker,
                        span: stmt_span,
                    });
                }
            }
            Err(rhs) => {
                out.push(Stmt::Let {
                    var,
                    rhs: rewrite_expr(rhs, state_temps),
                    span: stmt_span,
                });
            }
        },
```

   - `Let v = <hook>(…)` → entrée + `Let v = marqueur` (sauf `useState`/
     `useReducer` destructuré en tableau : le `Let` du temporaire disparaît).
   - `ExprStmt(<hook>(…))` → entrée + `ExprStmt(HookMarker(ℓ, marker_val))`
     (`hook_extractor.rs:418-435`).
   - `Assign` et `MemberWrite` : **jamais** reconnus comme appels de hook, seulement
     réécrits par `rewrite_expr` (8.1.2).
   - `rewrite_expr` (L842-910) remplace `Var(t)[0]` → `StateVal(ℓ)`,
     `Var(t)[1]` → `StateSetter(ℓ)`, `Var(t)[k≥2]` → `Lit(Unit)` pour tout `t`
     de `state_temps`, en descendant dans `FieldAccess`, `IndexAccess`,
     `BinOp`, `UnaryOp`, `Call`, `New`, `ArrayLit`, `ObjectLit`,
     `TSAnnotated` — et **pas** dans `CompApp`/`NativeElem`/`FnLit` (bras
     `other => other`). Sans conséquence observée : le motif tableau émet les
     projections `__arr_N[i]` juste après le `Let` du temporaire, dans le même
     bloc.
   - Le `span` de l'entrée est celui de l'**instruction** (le déclarateur, p.
     ex. `@4:8` pour `const [n, setN] = useState(0)` en colonne 8).
   - Formes d'appel de `useState` **vérifiées** (`/tmp/verif02/ex/v1_ts_target.tsx`,
     composant `RestState`) :
     - `const t = useState(1)` (pas de destructuration) : `is_arr_temp` est
       faux, donc `Let t = StateVal(1)` — la liaison lit le **tuple entier**
       comme la valeur d'état ; `t[0]`/`t[1]` resteront des `IndexAccess`
       sur `StateVal` (pas de `StateSetter`). Imprécis, et le setter passé
       par `t[1]` n'est plus reconnu comme tel (à vérifier côté moteur).
     - `useState(2);` en instruction : chemin `ExprStmt`, marqueur
       `marker_val(State) = Unknown` → `expr HookMarker(2, Unknown)` alors
       qu'une entrée `State` existe ; l'analyseur émet alors `info
       analysis-limit [hook:2] (line 20:2) hook \`<hook:2>\` was not found in
       the registry` et suspend les assurances (conservateur, message
       trompeur).
     - `const [s, ...rest] = useState(0)` : le `Let __arr_369` est supprimé
       (temporaire d'état), mais le motif de reste lie `rest` à
       `Var(__arr_369)` (4.9), que `rewrite_expr` ne réécrit pas (ce n'est pas
       un `IndexAccess`) : `let rest = __arr_369` lit une variable **qui
       n'est plus liée** nulle part (observé dans le dump). Cas d'école.
   - `useRef()` sans argument : `init = Lit(Null)` (L711-714) alors que React
     initialise `ref.current` à `undefined` ; seule la valeur initiale
     modélisée du conteneur en dépend (à vérifier dans le chapitre domaines).
   - `useReducer(reducer, init, initFn)` : seul le 2ᵉ argument est gardé ; le
     réducteur **et** un éventuel 3ᵉ argument `init` paresseux sont jetés
     (leurs lectures disparaissent de l'IR).

3. **Classification du callee** par provenance (fail-closed) :

```rust
// src/lowering/hook_extractor.rs:572-583  (début de la méthode ; suite L612-636 ci-dessous)
    fn classify_callee(&self, fn_: &Expr) -> Option<ResolvedHookCall> {
        match fn_ {
            Expr::Var(name) => {
                if self.local_hooks.contains(name) {
                    return Some(ResolvedHookCall {
                        origin_name: name.clone(),
                        is_react: false,
                        specifier: None,
                        resolved_file: self.current_file.map(Path::to_path_buf),
                    });
                }
                if let Some(origin) = self.origins.get(name) {
```

```rust
// src/lowering/hook_extractor.rs:612-636
                // Unimported bare name: hook-shaped is presumed React's.
                // (Shared predicate `super::is_hook_name` — the looser
                // `starts_with("use")` misclassified `userId()` as a hook.)
                super::is_hook_name(name).then(|| ResolvedHookCall {
                    origin_name: name.clone(),
                    is_react: true,
                    specifier: None,
                    resolved_file: None,
                })
            }
            // React.useState / R.useMemo / store.useThing.
            Expr::FieldAccess { obj, field } if super::is_hook_name(field) => {
                let is_react = match obj.as_ref() {
                    Expr::Var(ns) => self.react_ns.contains(ns) || ns == "React",
                    _ => false,
                };
                Some(ResolvedHookCall {
                    origin_name: field.clone(),
                    is_react,
                    specifier: None,
                    resolved_file: None,
                })
            }
            _ => None,
        }
```

   Ordre de priorité pour un nom nu : (1) hook défini localement dans le
   fichier (masque tout, portée JS) → custom ; (2) origine d'import prouvée
   (`React` / `File` / `Package`) ; (3) nom de forme hook non importé →
   présumé React (sources de tests, globales). Pour `ns.useX` : React ssi `ns`
   est lié au module `react` (ou vaut `React`), sinon custom de provenance
   inconnue. `is_hook_name` = `use` + majuscule ou chiffre
   (`src/lowering/mod.rs:52-58`).

4. **Construction de l'entrée** (`make_hook_entry`, L642-747) :

| Appel | `HookEntry` | Marqueur de la liaison (`hook_result_expr`) |
|---|---|---|
| non-React (local, fichier, paquet) | `Custom{name: origin_name, args, deps: Absent, binding, import_source, resolved_file}` | `HookMarker(ℓ, Unknown)` |
| `useState(init)` | `State{init}` (init absent → `Lit(Unit)`) | `StateVal(ℓ)` (ou `[0]`/`[1]` → `StateVal`/`StateSetter`) |
| `useReducer(r, init)` | `State{init}` (réducteur **ignoré**) | idem |
| `useEffect`, `useLayoutEffect`, `useInsertionEffect` | `Effect{body_cfg, deps}` | `HookMarker(ℓ, Undefined)` |
| `useMemo(f, deps)` | `Memo{body_cfg, deps}` | `MemoVal(ℓ)` |
| `useCallback(f, deps)` | `Callback{body_cfg, params, deps}` | `CallbackVal(ℓ)` |
| `useRef(init)` | `Ref{init}` (absent → `Lit(Null)`) | `HookMarker(ℓ, StableRef)` |
| autre `use*` de React (`useContext`, `useId`, …) | `Custom{…, resolved_file: None}` | `HookMarker(ℓ, Unknown)` |

   (Un `React.useX` hors liste mais dont le nom ne commence pas par `use`
   n'existe pas : `classify_callee` exige `is_hook_name`. Le bras `_ => None`
   de `make_hook_entry` est donc inatteignable en pratique ; il laisserait une
   provenance sans entrée.)

   Les corps de hooks :

```rust
// src/lowering/hook_extractor.rs:798-836
/// The body a hook's callback argument runs.
///
/// A literal `() => {…}` contributes its own CFG. Anything else — a variable, a
/// member access, a call returning a function — is *not* unanalysable: the hook
/// invokes it, so the body is exactly that invocation, and the engine resolves
/// the callee from the env like any other call.
///
/// The previous fallback handed back an `Unreachable` CFG, which claimed the
/// callback did nothing at all: ⊥ where ⊤ was required. `useEffect(handler)`
/// came out clean *and certified* `verified infinite-loop`, while the same body
/// written inline was reported.
fn hook_body_cfg(arg: Option<Expr>) -> CFG {
    let stmts = match arg {
        Some(Expr::FnLit { body_cfg, .. }) => return unwrap_body(body_cfg),
        Some(callee) => vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(callee),
                args: vec![],
            },
            None,
        )],
        // No callback argument at all — not valid React; nothing runs.
        None => vec![],
    };
    let mut blocks = std::collections::BTreeMap::new();
    blocks.insert(
        0,
        BasicBlock {
            id: 0,
            stmts,
            term: Terminator::Return(Expr::Lit(Prim::Unit)),
        },
    );
    CFG {
        entry: 0,
        blocks,
        edges: vec![],
    }
}
```

   Remarques : le corps d'un `useEffect(() => …)` est le `body_cfg` du `FnLit`
   **déplacé** (`Arc::try_unwrap`) ; les **paramètres** d'un `FnLit` passé à
   `useEffect`/`useMemo` sont jetés (corps zéro-argument, c'est la sémantique
   React) ; pour `useCallback`, ils sont gardés dans `params`. Le `FnLit`
   lui-même disparaît de l'IR : la valeur du render ne voit plus que
   `MemoVal(ℓ)`/`CallbackVal(ℓ)`/`HookMarker`. `useMemo(fn, deps)` où `fn` est
   une variable → corps `fn()` (un appel du callee, résolu par le moteur).

5. **Marqueur** (`marker_val`, L766-772) : `Effect → Undefined`,
   `Ref → StableRef`, tout le reste → `Unknown` ; la justification
   (L749-765) : répondre `Undefined` pour `useContext` & co rendait leur retour
   *prouvablement stable* et taisait les règles de stabilité (FN).

Complexité : `hoist_terminator_hooks` O(Σ taille des terminateurs),
`process_stmt` O(taille de chaque instruction) ; donc linéaire. Les labels sont
attribués dans **l'ordre des ids de blocs**, pas dans l'ordre source
(8.1.8).

### 4.14 Handlers (`extract_handlers`)

Doc (`hook_extractor.rs:84-99`) : la joignabilité se décide **par fuite**, pas
par nom de prop — élément natif : `on*` et `ref` ; composant : **toute** prop
dont la valeur résout en corps de fonction (`onToggle`, `ref`, render props,
`action={cb}`). Algorithme :

```rust
// src/lowering/hook_extractor.rs:113-141
    let mut var_bodies: HashMap<&str, CFG> = HashMap::new();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            if let Stmt::Let { var, rhs, .. } | Stmt::Assign { var, rhs, .. } = stmt {
                match rhs {
                    Expr::FnLit { body_cfg, .. } => {
                        var_bodies.insert(var.as_str(), (**body_cfg).clone());
                    }
                    Expr::CallbackVal(l) => {
                        if let Some(body) = callback_bodies.get(l) {
                            var_bodies.insert(var.as_str(), (*body).clone());
                        }
                    }
                    // Bare setters (`onOpenChange={setOpen}`) are NOT handled
                    // here: whether the receiver may call them with arbitrary
                    // args depends on whether the child is analyzable, which
                    // only the engine knows (see eval_comp_app's unknown-child
                    // havoc). Synthesizing a ⊤-write at lowering time would
                    // clobber the precise inter-component analysis of known
                    // children (e.g. `onChange={setN}` between two analyzed
                    // components).
                    _ => {}
                }
            }
        }
    }

    let mut found: Vec<HookEntry> = Vec::new();
    cfg.for_each_expr(&mut |e| collect_handlers_in_expr(e, &var_bodies, &mut found, next_label));
```

1. pré-passe : nom de variable → corps (`FnLit` lié, ou `CallbackVal(ℓ)` →
   corps du `HookEntry::Callback` ℓ) ; **un seul corps par nom, le dernier
   rencontré gagne** (8.1.5) ; seuls les blocs du CFG de premier niveau sont
   parcourus ;
2. descente dans toutes les expressions de premier niveau (instructions +
   `Return`/`Branch`) ; dans `NativeElem`, une prop `on*`/`ref` dont la valeur
   se résout via `handler_body` (FnLit, `Var` connu, `TSAnnotated`) devient un
   `Handler{event: prop_to_event(nom)}` (`onClick` → `click`, `onMouseEnter` →
   `mouseEnter`, `ref` → `ref`) avec le span de la prop ; les autres props et
   les enfants sont descendus récursivement ; dans `CompApp`, **toute** prop
   résolue devient un handler (span = balise ouvrante) ; dans un `FnLit`
   (render helpers), on descend dans son corps ; sinon `for_each_child`.

Un setter nu passé en prop (`onChange={setN}`) n'est **pas** un handler (le
moteur s'en charge par le *havoc* d'enfant inconnu).

Subtilités vérifiées sur le code (`hook_extractor.rs:113-240`) :

- La pré-passe `var_bodies` enregistre **tous** les `Let`/`Assign` dont le RHS
  est un `FnLit` ou un `CallbackVal`, temporaires compris : les deux `Let
  __t0 = FnLit` d'un ternaire sont vus, le second écrase le premier
  (d'où 8.1.5).
- Dans un `NativeElem`, une prop `on*`/`ref` dont la valeur **ne** se résout
  **pas** (`onClick={handlers.click}`, `onClick={makeHandler(id)}`) ne donne
  aucun handler **et n'est pas redescendue** (le `else` récursif ne concerne
  que les props non-événement) ; dans un `CompApp`, une prop non résolue est
  toujours redescendue.
- Seuls le CFG de render et les corps de `FnLit` **encore présents** dans ce
  CFG sont parcourus. Les corps déjà déplacés dans une `HookEntry`
  (`useMemo`, `useCallback`, `useEffect`) ne le sont pas : un
  `<button onClick={…}>` construit **dans** un `useMemo` ne produit aucun
  `HookEntry::Handler`. Vérifié (`/tmp/verif02/ex/v2_memo_handler.tsx`,
  `InMemo` : 2 hooks, aucun handler ; le témoin `Direct` a son handler).
  Les règles qui marchent elles-mêmes les corps ne sont pas forcément
  aveugles : `state-mutation` trouve quand même `items.push(1);
  setItems(items)` dans `InMemo` (`error state-mutation … (line 6:35)`). Ce
  qui manque est l'entrée `Handler` (donc ses écritures dans le point fixe
  des handlers) ; l'impact exact par règle est **à vérifier**.
- Un handler n'est pas redescendu : une prop `onClick` dont le corps contient
  lui-même du JSX avec des `on*` n'en extrait pas les handlers imbriqués.
- `prop_to_event` ne met en minuscule que la **première** lettre après `on`
  (`onMouseEnter` → `mouseEnter`, `onDoubleClick` → `doubleClick`) ; une prop
  non `onX` (`ref`, `renderItem`, `action`) garde son nom. Complexité : les corps
sont **clonés** (une fois dans `var_bodies`, une fois par handler trouvé) —
coût mémoire proportionnel à (nombre de liaisons de fonctions × taille des
corps).

### 4.15 Subscriptions (`extract_subscriptions`)

Pour chaque `HookEntry::Effect`, parcours de son `body_cfg` (instructions
seulement ; « Terminator::Return not scanned addEventListener is never a
return expr », L39 — **faux** pour un effet concis `useEffect(() =>
el.addEventListener("x", f))`, vérifié ci-dessous et en 8.1.14), recherche du motif
`recv.addEventListener("<littéral>", <FnLit>)` et création d'un
`Handler{event: littéral, body_cfg: corps du listener, span: span de
l'instruction}`. Le `FnLit` d'arg 1 n'est pas redescendu ; les autres
arguments et le receveur le sont (`collect_subscriptions_in_expr`,
L43-80). Les listeners imbriqués dans un `FnLit` (p. ex. un `setTimeout`) ne
sont pas trouvés (`for_each_child` ne traverse pas les `FnLit`) — test
`nested_addeventlistener_in_callback_not_extracted`. Événement non littéral ou
listener par variable : pas de handler (« acceptable FN », tests
L1465-1499).

Vérification de l'écart « terminateur non scanné »
(`/tmp/verif02/ex/v4_concise_sub.tsx`) :

```tsx
// /tmp/verif02/ex/v4_concise_sub.tsx (extrait, source complète en 6.9)
export function ConciseSub() {
  const [n, setN] = useState(0);
  useEffect(() => window.addEventListener("resize", () => setN(1)), []);
  return <div>{n}</div>;
}
```

donne `ConciseSub (2 hooks) ✓` — aucun `Handler event=resize` et
`verified missing-cleanup` — alors que la même ligne écrite avec un corps en
bloc (`useEffect(() => { window.addEventListener(…); }, [])`, composant
`BlockSub`) donne `#2 Handler event=resize @11:20` et `warn missing-cleanup
[hook:1] (line 11:20)`. La flèche concise met l'appel dans
`Terminator::Return` (4.10), que `collect_subscriptions_in_cfg` ne lit pas ;
`CFG::for_each_expr` (qui visite aussi `Return`/`Branch`) aurait couvert le
cas.

Portée de la recherche : seules les `HookEntry::Effect` sont fouillées — un
`addEventListener` dans un corps `useMemo`/`useCallback`, dans un handler ou
dans le render ne crée pas de subscription. Le span de la subscription est
celui de l'**instruction** qui la contient (`stmt_span`), pas celui de
l'appel.

### 4.16 Où la soundness est garantie (récapitulatif)

| Mécanisme | Direction | Source |
|---|---|---|
| fall-through = `Return(undefined)` | évite de couper l'appelant de sa sortie | ADR-025 |
| `throw` = `Unreachable` (pas d'arête inventée) | évite un FP Error `conditional-hook` | ADR-025 §2 |
| `break`/`continue` = vraies arêtes, `continue` = `Back` | pas de sortie fantôme ; widening | `cfg_builder.rs:381-397` |
| `switch` : dispatch opaque, fall-through | pas de case perdue | #1 |
| `try` : `Branch(⊤)`, finally sur la jonction | catch non « sur tous les chemins » | #2 |
| lectures gardées (`lower_for_effect`, défauts, clés calculées, spreads) | pas de « variable non lue » inventée | #76, `expr_lower.rs:28-31` |
| écriture de chaque feuille de déstructuration | pas de liaison périmée | `expr_lower.rs:1023-1029` |
| composés non fidèles → ⊤ | pas d'aliasing d'opérateur faux | #3 |
| `hook_body_cfg` = appel du callee, pas `Unreachable` | pas de ⊥ pour un corps inconnu | commentaire L805-808 |
| hoist des terminateurs | hooks de `return`/condition extraits | #4 |
| `MarkerVal::Unknown` pour React non modélisé | pas de stabilité prouvée par erreur | L749-765 |
| `await` : argument évalué avant la coupure | pas de `deferred` abusif | ADR-035 |

---

## 5. Décisions de conception

### 5.1 ADR du périmètre

**ADR-003 — Dedicated CFG-based IR** (Accepted, 2026-05-29). Décide une IR
dédiée inspirée de React-tRace mais **en CFG** plutôt qu'en arbre ;
alternative refusée : IR arborescente à la React-tRace, qui exige une
transformation CPS pour les retours anticipés et ne représente pas les boucles
sans arcs arrière. Arguments : retours anticipés, boucles, `switch` natifs ;
en-têtes de boucles structurels (widening) ; dominance standard (hooks
conditionnels). **Écarts entre l'ADR et le code actuel** (à signaler dans le
manuscrit) : la table de désucrage annonce `a && b → If(a, b, Lit(false))` et
`a || b → If(a, Lit(true), b)`, alors que le code produit des diamants de
blocs avec temporaire (ADR-020 §1) ; les « nœuds de hooks » `UseState`,
`UseEffect`… n'existent pas sous ces noms (ce sont des `HookEntry` + marqueurs
`StateVal` etc.) ; `TsAnnotated { expr, ty }` a perdu `ty` (ADR-020 §10) ;
« Early `return null` … following blocks in a separate CFG » : les blocs
suivants sont dans **le même** CFG (bloc de jonction) ; `docs/ir.md` (cité
comme grammaire complète) **n'existe pas** dans le dépôt ; la règle de
détection « Priority 1/2 » a gagné une règle 4 (appel de hook, #122,
`component_detector.rs:66-72`). Autres écarts relevés à la relecture de
l'ADR (`docs/adr/ADR-003-ir-design.md:12-59`) : `a ? b : c → If(a, b, c)`
(aujourd'hui un diamant de trois blocs, 4.12) ; `const { x, y } = obj →
Let(x, FieldAccess(obj, "x")); …` (aujourd'hui un temporaire `__obj_{offset}`
d'abord, puis `FieldAccess(__obj_N, "x")`, 4.9) ; « `Terminator` (Jump |
Branch | Return) » (il existe un quatrième terminateur, `Unreachable`,
ADR-025) ; « `src/lowering/` contains the Oxc AST → IR lowering (single
pass) » (le lowering est suivi d'une passe IR→IR distincte,
`hook_extractor`, 4.13). Statut : non superseded formellement.

**ADR-004 — Component structure — separate render_cfg + effect_cfg**
(Accepted, 2026-05-29). Décide qu'un composant = `render_cfg` + table
`hooks` avec des corps d'effets séparés, pas de méta-CFG unique
render→effect→check ; alternative refusée : méta-CFG (gros, maintenance, ordre
React implicite). La structure décrite (`deps: Option<Vec<Expr>>`,
`Memo.deps: Vec<Expr>`) est **périmée** : aujourd'hui `DepsArg`, champs
`span`, `Custom.binding/import_source/resolved_file`, variante `Handler`,
`ComponentIR.file/dom_props/hook_provenance/module_consts`. Le cycle
d'analyse (render → effets → join → check → widening) reste la référence
(correspondance StepInit → StepEffect → StepCheck de React-tRace).

**ADR-010 — Heap model — allocation-site abstraction** (Accepted —
implemented). Décide `ExprId` sur chaque nœud allouant (à l'époque
`FnLit`/`ObjectLit`/`ArrayLit`, aujourd'hui + `New`), `Arc<CFG>` dans
`FnLit`, un tas `HashMap<ExprId, HeapValue>` monotone et un environnement à
deux cartes (`stabs` + `locs`) ; alternative refusée : une seule carte `EnvVal
= Val | Loc` (l'`extend` écrasait la `Loc`). Côté lowering : le compteur était
« dans `BlockBuilder` » ; il est devenu `ExprIds` partagé par fichier (#134,
commit `0c0bb70`) car un compteur par builder numérotait chaque corps depuis 0
et faisait partager une entrée de tas à deux objets sans rapport (FN). Limite
connue : `IndexAccess` reste ⊤ ; membres avant spread et accesseurs absents de
la carte par membres (clés synthétiques).

**ADR-025 — A body that falls off the end returns `undefined` —
`Unreachable` means only "control stops"** (Accepted, 2026-07-29). Décisions :
(1) fall-through scellé `Return(Lit(Unit))` ; (2) `throw` garde
`Unreachable` et le splice le laisse ; alternative **mesurée et refusée** :
câbler tout `Unreachable` du callé vers la jonction (« juste » une
sur-approximation) — elle inventait un chemin vers la sortie qui évite les
hooks du callé et produisait un `conditional-hook` **Error** sur l'idiome
garde-`throw` (`commerce cart-context.tsx:214`) ; (3) un `Return` inatteignable
n'est pas une sortie : `ExitDominance` est seul propriétaire de l'ensemble des
sorties via `CFG::reachable_blocks`. Mesure : composants coupés 208 → 3 ; 12
findings révélés, 0 perdu. Tests : `tests/cfg_exit_integrity.rs`.

**ADR-039 — a synthetic binding is synthetic, its position is not**
(Accepted, 2026-09-02, implémente #131). Décisions : (1) toute instruction
synthétique prend la position de l'expression qu'elle lie (hoist d'`await`,
temporaires de ternaire et de `||`/`&&`, sous-nœuds de motifs : élément,
propriété, reste, clé calculée, défaut — repli sur la position du motif) ; (2)
ce que la source ne nomme pas, le splice le nomme par son site d'appel
(`Return` n'a pas de span) ; (3) un corps sans instruction hérite du *witness*
d'où on y est entré ; (4) un finding prend la première position de sa chaîne
de témoins. Alternative refusée : ajouter un span à `Terminator::Return`
(« better by a line, at the cost of a hundred construction sites ») — reste
ouvert en #140. Mesure : 82 → 0 findings sans position, ensemble de findings
inchangé.

### 5.2 ADR connexes qui contraignent le lowering

- **ADR-020** (campagne dette technique close) : §1 garder le diamant
  `&&`/`||` ; §10 ne pas brancher `TSType` dans le domaine (types effacés et non
  garantis) ; §11 garder les noms de temporaires à offset et le préfixe
  `__arr_`. Non-changements « à ne pas re-tenter ».
- **ADR-035** (await phase boundary, #117) : l'`await` fend le bloc par une
  arête `EdgeKind::Await` plutôt qu'un champ de bloc (« `BasicBlock` and `CFG`
  are constructed at over two hundred sites ») ; la post-await est calculée par
  le consommateur (`post_await_blocks`) ; nested bodies answer for themselves.
- **ADR-023** (« step 1 », cité par le code) : identité d'un hook par
  **provenance** d'import, fail-closed (`HookOrigin`) ; le texte de l'ADR ne
  contient pas de section « step 1 » explicite (à vérifier : la mention vient
  des commentaires `hook_extractor.rs:465`, `527-530`, `import_resolution.rs`).
- **ADR-019** (witness chain) : `SourceRange` porte un `FileId` pour les
  spans de CFG inlinés inter-fichiers.
- **ADR-024** : un finding dans un hook inliné est attribué à l'origine ; la
  provenance (`HookProvenance.span`) sert à cela.

### 5.3 Issues pertinentes

Fermées `wontfix` touchant le lowering (`gh issue list --state closed --label
wontfix`) :

- **#63** « Out of scope — dynamic components (`const C = cond ? A : B`) » :
  `<C/>` ne génère pas de `CompApp` résolu (le nom `C` n'est lié à aucune
  origine) ; « Reopen with a design for resolving a component reference
  through a join ».
- **#65** « Out of scope — anonymous default exports get a generic name » :
  `export default () => <div/>` → `DefaultExport`
  (`component_detector.rs:22-44`).

(#101, #51, #42, #40 sont `wontfix` mais hors lowering.)

Fermées, historiques du périmètre : #1 (`switch` perdait les cases après le
premier `break` → dispatch opaque), #2 (`try` qui retourne faisait disparaître
`catch`/`finally`), #3 (dix opérateurs abaissés en `Add`), #4 (hook dans
`return`/condition sans `HookEntry` → hoist ; résidu : hook imbriqué dans du
JSX retourné, « Left in `docs/limitations.md` » selon le commentaire de
clôture, **mais absent de ce fichier au 2026-09-28**), #5 (flèches concises),
#13 (splice tronquant sans rollback), #73/#74/#75 (opérateurs modélisés),
#77 (corps de classes perdus), #104 (arity des deps), #117 (await), #122
(composant qui retourne `null` partout).

Ouvertes : **#140** (`Terminator::Return` sans span), **#76** (spreads et clés
calculées gardés pour leurs lectures mais non modélisés ; `f(...handlers)`
invisible à `collect_escaping_setters`), **#64** (`React.memo`/`forwardRef`),
**#158** (`new` en appel simple — code corrigé par `e67b10a`, issue encore
ouverte), **#52** (inlining d'utilitaires en position d'instruction seulement).

### 5.4 Principes de CLAUDE.md qui s'appliquent

- *Pas de workarounds / corriger la cause racine* : c'est ce qui a donné la
  normalisation des terminateurs (#4) au lieu d'un second parcours dans
  l'extracteur (« Teaching the extractor to also walk terminators would mean
  duplicating … so the CFG is normalised instead », L306-316), ou
  `Candidate::build_cfg` comme point unique de dispatch (#5).
- *Modulaire et général d'abord* : `lower_for_effect` et `synthetic_key`
  partagés par objet, tableau, classe, JSX ; `Expr::for_each_child` et
  `subscription_listener` comme prédicats uniques ; `is_hook_name` partagé par
  les détecteurs et l'extracteur.
- *Soundness : faux négatifs interdits* : chaque commentaire de ce code
  justifie un choix par « ne pas perdre une lecture / un chemin / une
  écriture » (tableau 4.16).
- *Niveaux de diagnostic* : le lowering conditionne l'Error : un chemin en
  trop coûte une démotion Error → Warning (catch-only, dispatch de `switch`,
  exemple 6.4) ; un chemin inventé vers la sortie coûterait un FP Error
  (ADR-025 §2).

### 5.5 Historique utile (`git log --oneline -- <fichier>`)

`cfg_builder.rs` (29 commits) : `898621b feat: add cfg builder` →
`a96cfaa` flèches à corps-expression → `8a49f25` heap model → `dcb4d0e` param
de catch → `3e54d70` tête de for-of/for-in → `342c5bf` break/continue/switch
comme vraies arêtes → `34ce48b` fall-through = `undefined` (ADR-025) →
`fb8a7b1` « stop dropping callback bodies, writes and reads » → `04ebf2a`
await (#117) → `c01afe4` positions des liaisons synthétiques (#131) →
`0c0bb70` sites d'allocation (#134) → `3a068ed` try/catch/finally (#2) →
`806d114` origine des callees JSX (#7) → `e257bcc` (#147).

`expr_lower.rs` (40 commits) : `62f3f7c feat: expression lowering` →
`14fc8d9` réaffectation → `50e435c` sentinelle opaque typée → `769a0f5`
opérateurs non supportés → `a195bfa`/`48ffef9` arity des deps → `548f922`
`%`, `**`, `in`, `instanceof` → `e67b10a` `Expr::New` (#158).

`hook_extractor.rs` (48 commits) : `17a837d feat: hook IR` → `4f61e6d`
`HookMarker` → `b7bc459` custom inconnu = ⊤ → `fca0937` `is_hook_name` →
`c44a0d0` provenance du hook destructuré → `27538cd` identité par
provenance (ADR-023 step 1) → `da2fe5d` React non modélisé = ⊤ → `30a00c5`
hook dans un terminateur (#4, #5) → `6f25bd2` setters suivis jusqu'au handler
→ `e67b10a`.

---

## 6. Exemples concrets (vérifiés)

### 6.0 Notation des dumps

Produits par `/tmp/irdump` (hors dépôt). `Bk:` bloc (« (orphan) » si
inatteignable), `let v = e` / `v := e` / `o.f <- e` / `expr e` pour
`Let`/`Assign`/`MemberWrite`/`ExprStmt`, `@l:c` = span, `=> …` terminateur,
`edges: a->b[T|F|U|Back|Await]`. `{#k …}` = `ObjectLit` d'`ExprId` k,
`[#k …]<Exact(n)>` = `ArrayLit`, `fn#k(p) {…}` = `FnLit`, `⊤` =
`SummaryVal(Top)`, `Native<t>(props)[children]`, `CompApp<N>(props)`,
`TS(e)` = `TSAnnotated`.

### 6.1 Compteur (le plus simple)

```tsx
// /tmp/ex/e1_counter.tsx
import { useState } from "react";

export function Counter() {
  const [n, setN] = useState(0);
  return <button onClick={() => setN(n + 1)}>{n}</button>;
}
```

IR observée :

```
=== component Counter (param props) ===
render_cfg:
  B0:
      let n = StateVal(0)  @4:9
      let setN = StateSetter(0)  @4:12
      => return Native<button>({#0 onClick: fn#1() {
          B0:
              => return setN((n Add 1))
          edges: 
      }})[n]
  edges: 
hooks:
  #0 State init=0  @4:8
  #1 Handler event=click  @5:17
    B0:
        => return setN((n Add 1))
    edges: 
provenance:
  #0 origin=useState react=true specifier=Some("react") inlined=false
```

Lecture : `param props` (aucun paramètre) ; le `Let __arr_N = useState(0)` a
disparu (temporaire d'état) ; `__arr_N[0]`/`[1]` réécrits ; l'enfant `{n}` est
abaissé avant les props (le `FnLit` a l'id 1, l'objet de props l'id 0 car
`lower_jsx_props` prend son id avant d'abaisser les valeurs) ; le handler est
le **corps** du `FnLit` (flèche concise → `Return`), label 1 à la suite du
hook 0 ; aucune provenance pour le handler. Analyseur : `Counter (2 hooks) ✓`
avec `verified conditional-hook`, `lazy-init`, `setter-in-render`,
`state-mutation`.

### 6.2 Retour anticipé, `for…of`, `continue`, ternaire

```tsx
// /tmp/ex/e2_control.tsx
import { useState } from "react";

export function Gate({ kind, items }: { kind: string; items: string[] }) {
  if (kind === "none") {
    return null;
  }
  const [n, setN] = useState(0);
  let total = 0;
  for (const it of items) {
    if (!it) continue;
    total += 1;
  }
  const label = n > 0 ? "some" : "none";
  return <div title={label}>{total}</div>;
}
```

```
=== component Gate (param __p0) ===
render_cfg:
  B0:
      let __obj_56 = __p0
      let kind = __obj_56.kind  @3:23
      let items = __obj_56.items  @3:29
      => branch (kind Eq "none") ? B1 : B2
  B1:
      => return null
  B2:
      => jump B3
  B3:
      let n = StateVal(0)  @7:9
      let setN = StateSetter(0)  @7:12
      let total = 0  @8:6
      expr items  @9:19
      => jump B4
  B4:
      => branch true ? B5 : B6
  B5:
      let it = ⊤  @9:13
      => branch Not(it) ? B7 : B8
  B6:
      => branch (n Gt 0) ? B10 : B11
  B7:
      => jump B4
  B8:
      => jump B9
  B9:
      total := (total Add 1)  @11:4
      expr total  @11:4
      => jump B4
  B10:
      let __t0 = "some"  @13:24
      => jump B12
  B11:
      let __t0 = "none"  @13:33
      => jump B12
  B12:
      let label = __t0  @13:8
      => return Native<div>({#0 title: label})[total]
  edges: 0->1[T] 0->2[F] 2->3[U] 3->4[U] 4->5[T] 4->6[F] 5->7[T] 5->8[F] 7->4[Back] 8->9[U] 9->4[Back] 6->10[T] 6->11[F] 10->12[U] 11->12[U]
```

Lecture : préambule de props sans span (`let __obj_56 = __p0`) ; `if` sans
`else` = trois blocs (B1/B2/B3) ; `useState` dans la jonction B3, que la
branche `return null` évite ; `for…of` : lecture de `items` dans le
pré-en-tête, en-tête `branch true` sans span, variable `it = ⊤` ;
`continue` → `7->4[Back]` ; `total += 1` → `Assign` + `ExprStmt` ; le
ternaire (B10/B11/B12) : deux `Let __t0`, la déclaration `label` atterrit
dans la jonction. Note sur la **numérotation** : B6 (sortie de boucle) a été
réservé avant B7..B9 (corps). Analyseur :

```
  Gate  (1 hooks)  /tmp/ex/e2_control.tsx
    error  conditional-hook  [hook:0]  (line 7:8)  this hook is called conditionally (not on every render path)
```

### 6.3 `?.`, `??`, `&&` en instruction, `useEffect`

```tsx
// /tmp/ex/e3_logical.tsx
import { useState, useEffect } from "react";

export function Profile({ user, fallback }: { user?: { name?: string }; fallback: string }) {
  const [open, setOpen] = useState(false);
  const name = user?.name ?? fallback;
  open && console.log(name);
  useEffect(() => {
    setOpen(true);
  }, []);
  return <span>{name}</span>;
}
```

```
render_cfg:
  B0:
      let __obj_70 = __p0
      let user = __obj_70.user  @3:26
      let fallback = __obj_70.fallback  @3:32
      let open = StateVal(0)  @4:9
      let setOpen = StateSetter(0)  @4:15
      let __t0 = user.name  @5:15
      => branch __t0 ? B2 : B1
  B1:
      __t0 := fallback  @5:29
      => jump B2
  B2:
      let name = __t0  @5:8
      let __t1 = open  @6:2
      => branch __t1 ? B3 : B4
  B3:
      __t1 := console.log(name)  @6:10
      => jump B4
  B4:
      expr __t1  @6:2
      expr HookMarker(1, Undefined)  @7:2
      => return Native<span>({#2 })[name]
  edges: 0->2[F] 0->1[F] 1->2[U] 2->3[T] 2->4[T] 3->4[U]
hooks:
  #0 State init=false  @4:8
  #1 Effect deps=List[]<Exact(0)>  @7:2
    B0:
        expr setOpen(true)  @8:4
        => return undefined
    edges: 
```

Lecture : `user?.name` est un simple `FieldAccess` ; `??` = diamant `||` ;
`open && …` laisse `expr __t1` dans la jonction ; `useEffect` en instruction →
`expr HookMarker(1, Undefined)` + `Effect` dont le corps est le `FnLit`
déplacé (fall-through → `return undefined`) ; `[]` → `List[]<Exact(0)>`.
**Polarité** : `0->2[F] 0->1[F]` (les deux arêtes du `??` sont `IfFalse`) et
`2->3[T] 2->4[T]` (les deux arêtes du `&&` sont `IfTrue`), cf. 4.12 et 8.1.1.
Analyseur : `warn unnecessary-rerender [hook:0] (line 7:2) mount-only effect
flips state open from false to true…`, reste vérifié.

### 6.4 `switch` avec fall-through, `try/catch/finally`

```tsx
// /tmp/ex/e4_switch_try.tsx
import { useState } from "react";

export function Status({ code }: { code: number }) {
  const [msg, setMsg] = useState("");
  switch (code) {
    case 1:
      setMsg("one");
      break;
    case 2:
      setMsg("two");
    default:
      setMsg("other");
  }
  try {
    risky();
  } catch (e) {
    report(e);
  } finally {
    done();
  }
  return <p>{msg}</p>;
}
```

```
  B0:
      let __obj_58 = __p0
      let code = __obj_58.code  @3:25
      let msg = StateVal(0)  @4:9
      let setMsg = StateSetter(0)  @4:14
      expr code
      => branch true ? B2 : B5
  B1:
      => branch ⊤ ? B7 : B8
  B2:
      expr setMsg("one")  @7:6
      => jump B1
  B3:
      expr setMsg("two")  @10:6
      => jump B4
  B4:
      expr setMsg("other")  @12:6
      => jump B1
  B5:
      => branch true ? B3 : B6
  B6:
      => branch true ? B4 : B1
  B7:
      expr risky()  @15:4
      => jump B9
  B8:
      let e = ⊤  @16:11
      expr report(e)  @17:4
      => jump B9
  B9:
      expr done()  @19:4
      => return Native<p>({#0 })[msg]
  edges: 0->2[T] 0->5[F] 5->3[T] 5->6[F] 6->4[T] 6->1[F] 2->1[U] 3->4[U] 4->1[U] 1->7[T] 1->8[F] 7->9[U] 8->9[U]
```

Lecture : B1 = sortie du `switch` (réservée en premier) ; dispatchs B0 → B5 →
B6 (`branch true`, sans span) ; `case 2` tombe dans `default`
(`3->4[U]`) ; B1 porte ensuite le `Branch(⊤)` du `try` ; `finally` dans la
jonction B9. Analyseur : `warn setter-in-render [hook:0] (line 7:6) setter
setMsg called directly in the render body…` — **Warning** et non Error, car
aucun `setMsg` n'est sur tous les chemins (la sortie du switch est atteignable
sans entrer dans une case).

### 6.5 Hooks custom, `useRef`, `useMemo` avec spread, handler de composant, subscription

```tsx
// /tmp/ex/e5_hooks.tsx
import { useState, useEffect, useMemo, useRef } from "react";
import { useQuery } from "@tanstack/react-query";

function useToggle(init: boolean) {
  const [on, setOn] = useState(init);
  const toggle = () => setOn(!on);
  return [on, toggle] as const;
}

export function Panel({ id, opts }: { id: string; opts: object }) {
  const { data } = useQuery({ queryKey: [id] });
  const [on, toggle] = useToggle(false);
  const ref = useRef(null);
  const merged = useMemo(() => ({ ...opts, id }), [opts, id]);
  useEffect(() => {
    window.addEventListener("resize", () => toggle());
  }, [merged]);
  return <Child ref={ref} data={data} onPick={(x: number) => toggle()} {...merged} />;
}
```

```
=== component Panel (param __p0) ===
render_cfg:
  B0:
      let __obj_279 = __p0
      let id = __obj_279.id  @10:24
      let opts = __obj_279.opts  @10:28
      let __obj_333 = HookMarker(0, Unknown)  @11:8
      let data = __obj_333.data  @11:10
      let __arr_382 = HookMarker(1, Unknown)  @12:8
      let on = __arr_382[0]  @12:9
      let toggle = __arr_382[1]  @12:13
      let ref = HookMarker(2, StableRef)  @13:8
      let merged = MemoVal(3)  @14:8
      expr HookMarker(4, Undefined)  @15:2
      => return CompApp<Child>({#9 ref: ref, data: data, onPick: fn#10(x) {
          B0:
              => return toggle()
          edges: 
      }, ...11: merged})
  edges: 
hooks:
  #0 Custom useQuery({#0 queryKey: [#1 id]<Exact(1)>}) binding=Some("__obj_333") import_source=Some("@tanstack/react-query") resolved_file=None  @11:8
  #1 Custom useToggle(false) binding=None import_source=None resolved_file=Some("e5_hooks.tsx")  @12:8
  #2 Ref init=null  @13:8
  #3 Memo deps=List[opts, id]<Exact(2)>  @14:8
    B0:
        => return {#3 ...4: opts, id: id}
    edges: 
  #4 Effect deps=List[merged]<Exact(1)>  @15:2
    B0:
        expr window.addEventListener("resize", fn#7() {
            B0:
                => return toggle()
            edges: 
        })  @16:4
        => return undefined
    edges: 
  #5 Handler event=pick  @18:9
    B0:
        => return toggle()
    edges: 
  #6 Handler event=resize  @16:4
    B0:
        => return toggle()
    edges: 
provenance:
  #0 origin=useQuery react=false specifier=Some("@tanstack/react-query") inlined=false
  #1 origin=useToggle react=false specifier=None inlined=false
  #2 origin=useRef react=true specifier=Some("react") inlined=false
  #3 origin=useMemo react=true specifier=Some("react") inlined=false
  #4 origin=useEffect react=true specifier=Some("react") inlined=false
=== custom hook useToggle (params ["init"]) ===
  B0:
      let on = StateVal(0)  @5:9
      let setOn = StateSetter(0)  @5:13
      let toggle = fn#0() {
          B0:
              => return setOn(Not(on))
          edges: 
      }  @6:8
      => return TS([#1 on, toggle]<Exact(2)>)
  edges: 
hooks:
  #0 State init=init  @5:8
```

Lecture :

- `useQuery` (paquet) et `useToggle` (local) → `Custom` + `HookMarker(ℓ,
  Unknown)` ; un custom destructuré en **objet** garde le temporaire
  `__obj_333` comme `binding` (seul le préfixe `__arr_` est exclu) ; destructuré
  en tableau, `binding=None` et les projections `__arr_382[i]` restent des
  `IndexAccess` (pas de réécriture : ce n'est pas `useState`) ;
  `resolved_file` du hook local = le fichier courant.
- `useRef` → `StableRef` ; `useMemo` → `MemoVal(3)`, corps = flèche concise
  dont l'objet a une clé spread `...4` ; deps `List[opts, id]`.
- Handler `#5 event=pick` (prop de composant `onPick`, span de la balise
  `@18:9`) ; `ref={ref}` ne donne rien (valeur = marqueur, pas un corps) ;
  subscription `#6 event=resize` (span de l'instruction dans l'effet).
- Le hook `useToggle` a son propre compteur d'ids (`fn#0`, `#1`) : autre
  `LowerCtx` (3.5). `as const` → `TS(…)`.

Analyseur :

```
  Panel  (7 hooks)  /tmp/ex/e5_hooks.tsx
    info   analysis-limit  component `Child` was not found in the analysis registry. Pass its file on the command line to analyse it (FN possible)
    warn   missing-cleanup  [hook:4]  (line 16:4)  this effect calls `window.addEventListener` but returns no cleanup. …
    warn   missing-deps  [hook:4]  var:toggle  (line 15:2)  `toggle` is used in this effect but not in its deps array, and its value may change between renders
    suspended  analysis-limit  9 passing check(s) withheld: the analysis was truncated in this component, so they are not guaranteed
```

### 6.6 `async`/`await` dans un effet : arêtes `Await`

```tsx
// /tmp/ex/e6_async.tsx
import { useState, useEffect } from "react";

export function Loader({ url }: { url: string }) {
  const [data, setData] = useState(null);
  useEffect(() => {
    async function load() {
      const res = await fetch(url);
      setData(await res.json());
    }
    load();
  }, [url]);
  const useless = () => (url ? 1 : 2);
  return <div>{String(data)}</div>;
}
```

Corps de l'effet (extrait du dump) :

```
  #1 Effect deps=List[url]<Exact(1)>  @5:2
    B0:
        let load = fn#1() {
            B0:
                let __t0 = fetch(url)  @7:24
                => jump B1
            B1:
                let res = __t0  @7:12
                let __t1 = res.json()  @8:20
                => jump B2
            B2:
                expr setData(__t1)  @8:6
                => return undefined
            edges: 0->1[Await] 1->2[Await]
        }  @6:4
        expr load()  @10:4
        => return undefined
```

Lecture : chaque `await` : l'argument est lié **avant** la coupure (`let __t0 =
fetch(url)` dans B0, avec le span de l'argument), puis `Jump` + arête `Await` ;
la déclaration `res` et l'appel `setData(…)` atterrissent après ; la fonction
déclarée `load` devient un `Let load = FnLit` à sa position. Le ternaire du
render helper `useless` a ses propres `__t0` et blocs (autre builder). Analyseur
: `Loader (2 hooks) ✓`, dix vérifications passées.

### 6.7 Déstructurations, affectations composées, `delete`, gabarit, `void`, spread JSX

```tsx
// /tmp/ex/e11_destr.tsx
import { useState } from "react";

export function Destr({ a = 1, ...rest }: any) {
  const [[min, max], setRange] = useState([0, 100]);
  const { [rest.k]: v, w = min, ...others } = rest;
  let i = 0;
  [i, max] = [max, i];
  i += 2;
  i **= 2;
  i ||= 3;
  delete others.x;
  const s = `n=${i}`;
  void track(s);
  return <X {...others} title={s}><span>{v}</span></X>;
}
```

```
  B0:
      let __obj_57 = __p0
      expr 1  @3:28
      let a = __obj_57.a  @3:24
      let rest = __obj_57  @3:31
      let __arr_93 = StateVal(0)  @4:9
      let min = __arr_93[0]  @4:10
      let max = __arr_93[1]  @4:15
      let setRange = StateSetter(0)  @4:21
      let __obj_145 = rest  @5:8
      expr rest.k  @5:11
      let v = ⊤  @5:10
      expr min  @5:27
      let w = __obj_145.w  @5:23
      let others = __obj_145  @5:32
      let i = 0  @6:6
      let __dstr_204 = [#1 max, i]<Exact(2)>  @7:2
      i := __dstr_204[0]
      max := __dstr_204[1]
      expr [#1 max, i]<Exact(2)>  @7:2
      i := (i Add 2)  @8:2
      expr i  @8:2
      i := (i Pow 2)  @9:2
      expr i  @9:2
      i := ⊤  @10:2
      expr i  @10:2
      others.x <- undefined  @11:2
      expr true  @11:2
      let s = (("n=" Add i) Add "")  @12:8
      expr track(s)  @13:2
      expr undefined  @13:2
      => return CompApp<X>({#3 ...4: others, title: s, children: [#5 Native<span>({#2 })[v]]<AtLeast(1)>})
hooks:
  #0 State init=[#0 0, 100]<Exact(2)>  @4:8
```

Lecture : défaut `a = 1` émis en `expr 1` sans que `a` le reçoive ; reste →
source ; motif imbriqué sur `useState` : le temporaire externe disparaît,
l'interne (`__arr_93`) reçoit `StateVal(0)` et ses projections restent des
`IndexAccess` ; clé calculée → lecture `rest.k` + `v = ⊤` ; déstructuration en
**affectation** : `Assign` sans span sur les feuilles, et l'`ExprStmt` final
réutilise la même valeur (le même `ArrayLit` `#1` apparaît **deux fois** dans
le CFG, même `ExprId`) ; `**=` fidèle, `||=` → ⊤ ; `delete` → `MemberWrite`
de `undefined` + `true` ; gabarit en additions ; `void` → `expr track(s)` puis
`expr undefined` ; `<X {...others}>` : spread `...4`, enfants en
`children: [#5 …]<AtLeast(1)>`.

### 6.8 Flèche concise en hook custom, hoist de terminateur, hooks conditionnels

```tsx
// /tmp/ex/e7_edge.tsx
import { useState, useEffect, useContext } from "react";

const useCount = () => useState(0);

export function Edge({ flag, Ctx }: { flag: boolean; Ctx: any }) {
  const [c, setC] = useCount();
  const theme = useContext(Ctx).theme;
  flag && useEffect(() => {}, []);
  const x = flag ? useState(1) : null;
  return <div>{theme}{c}{x}</div>;
}
```

```
=== component Edge (param __p0) ===
render_cfg:
  B0:
      let __obj_116 = __p0
      let flag = __obj_116.flag  @5:23
      let Ctx = __obj_116.Ctx  @5:29
      let __arr_170 = HookMarker(0, Unknown)  @6:8
      let c = __arr_170[0]  @6:9
      let setC = __arr_170[1]  @6:12
      let theme = useContext(Ctx).theme  @7:8
      let __t0 = flag  @8:2
      => branch __t0 ? B1 : B2
  B1:
      __t0 := useEffect(fn#0() {
          B0:
              => return undefined
          edges: 
      }, [#1 ]<Exact(0)>)  @8:10
      => jump B2
  B2:
      expr __t0  @8:2
      => branch flag ? B3 : B4
  B3:
      let __t1 = StateVal(1)  @9:19
      => jump B5
  B4:
      let __t1 = null  @9:33
      => jump B5
  B5:
      let x = __t1  @9:8
      => return Native<div>({#2 })[theme, c, x]
hooks:
  #0 Custom useCount() binding=None import_source=None resolved_file=Some("e7_edge.tsx")  @6:8
  #1 State init=1  @9:19
=== custom hook useCount (params []) ===
  B0:
      let __term_0 = StateVal(0)
      => return __term_0
hooks:
  #0 State init=0
```

Lecture :

- `useCount` (flèche concise) : `Return(useState(0))` normalisé en
  `let __term_0 = …` **sans span** (#140) puis extrait ; l'entrée `State` du
  hook n'a pas de position.
- `useState(1)` dans un bras de ternaire (un `Let`) : extrait, et l'analyseur
  rapporte `error conditional-hook [hook:1] (line 9:19)`.
- `useContext(Ctx).theme` (hook **imbriqué** dans un `FieldAccess`) et
  `flag && useEffect(…)` (hook dans un `Assign`) : **non extraits** — ils
  restent des `Call` ordinaires ; le composant annonce « 2 hooks ». Voir
  8.1.2.

### 6.9 Contre-exemples vérifiés (limites du lowering)

Chaque cas a été exécuté avec l'analyseur ; ils servent la section 8.

| Fichier `/tmp/ex/…` | Source (extrait) | Observé |
|---|---|---|
| `e8_nested_hook.tsx` `AndHook` | `flag && useEffect(() => { setN(1); }, []);` | `AndHook (1 hooks)` … `verified conditional-hook all hooks run unconditionally, in a stable order` |
| `e8_nested_hook.tsx` `MemberHook`, `JsxHook` | `useContext(Ctx).theme` ; `return <div>{useLabel()}</div>` | aucune entrée de hook, aucun `analysis-limit` (`let __term_0 = Native<div>(…)[useLabel()]`) |
| `e9_try_partial.tsx` `Partial` | `try { obj = {a:1}; risky(); } catch (e) { cfg = obj; }` puis `useEffect(…, [cfg])` | `Partial ✓ verified always-unstable-deps` ; le témoin `Direct` (`cfg = {a:1}` sans `try`) → `warn always-unstable-deps` |
| `e13_after_return.tsx` | `return <button onClick={handle}/>; function handle() { setN(n + 1); }` | aucun `Handler` ; `✓` ; la même fonction déclarée avant le `return` (`e14`) donne `#1 Handler event=click` |
| `e15_cond_handler.tsx` | `onClick={flag ? () => setA(1) : () => setB(2)}` | un seul `Handler` (corps `setB(2)`) |
| `e16_empty_pattern.tsx` | `if (c) { const [] = useState(0); }` | `#0 State` mais aucun label dans le CFG → `verified conditional-hook` |
| `e17_dynamic_import.tsx` `Lazy` | `useEffect(() => { import(path); }, [])` | corps `expr ⊤` ; `verified missing-deps` ; le témoin `load(path)` → `warn missing-deps var:path` |
| `e18_label_order.tsx` | `if (a) { if (b) {…} const [x] = useState("x"); } const [y] = useState("y");` | `#0 State init="y"`, `#1 State init="x"` |
| `e10_hoist.tsx` `Hoisted` | `init(); function init() { setX(1); }` | `error setter-in-render (line 7:4)` — compensé par le pré-scan syntaxique (ADR-010 B6) |
| `e19_coalesce.tsx` `Coalesce` | `const z = 0 ?? 5; if (z === 0) { setX(1); }` | `✓ verified setter-in-render` ; le témoin `const z = 0;` → `warn setter-in-render (line 16:4)` |
| `e20_rest_param.tsx` | `useEffect(() => { const f = (...args) => args.length; f(1, 2); }, [])` avec une prop `args` | `fn#1()` sans paramètre ; `warn missing-deps var:args` |
| `e21_forof_pattern.tsx` | `let a = 0, b = 0; for ([a, b] of pairs) {…}` | aucune écriture de `a`/`b` dans le corps |

Contre-exemples ajoutés par la relecture (fichiers sous `/tmp/verif02/ex/`,
rejoués avec `irdump` et `reactant check --info --show-clean --no-color`) :

| Fichier `/tmp/verif02/ex/…` | Source (extrait) | Observé |
|---|---|---|
| `v1_ts_target.tsx` `TsTarget` | `let x: any = 1; (x as any) = { a: 1 }; useEffect(() => { read(x); }, [x]);` | aucun `Assign` de `x` (`expr {#0 a: 1}` ×2) ; `TsTarget ✓ … verified always-unstable-deps` ; témoin `PlainTarget` (`x = { a: 1 }`) → `warn always-unstable-deps [hook:0] (line 13:2)` |
| `v1_ts_target.tsx` `RestState` | `const [s, ...rest] = useState(0); const t = useState(1); useState(2);` | `let rest = __arr_369` (temporaire supprimé) ; `let t = StateVal(1)` ; `expr HookMarker(2, Unknown)` + `info analysis-limit [hook:2] (line 20:2)` |
| `v1_ts_target.tsx` `MemoJsx` / `v2_memo_handler.tsx` `InMemo` | `useMemo(() => <button onClick={() => …}>…</button>, [n])` | aucun `Handler` (2 hooks) ; le témoin `Direct` a son `Handler event=click` |
| `v2_memo_handler.tsx` `NonNull` | `let x: any = 1; x! = { a: 1 };` | aucune écriture de `x` |
| `v3_update.tsx` | `x!++; (x as any)++;` | `expr ⊤` ×2, aucune écriture |
| `v4_concise_sub.tsx` | `useEffect(() => window.addEventListener("resize", () => setN(1)), [])` | aucun `Handler event=resize`, `verified missing-cleanup` ; forme bloc → `#2 Handler event=resize @11:20` et `warn missing-cleanup (line 11:20)` |

Sources complètes (les numéros de ligne cités ci-dessus s'y rapportent) :

```tsx
// /tmp/verif02/ex/v1_ts_target.tsx
import { useState, useEffect, useMemo } from "react";

export function TsTarget({ p }: any) {
  let x: any = 1;
  (x as any) = { a: 1 };
  useEffect(() => { read(x); }, [x]);
  return <div />;
}

export function PlainTarget({ p }: any) {
  let x: any = 1;
  x = { a: 1 };
  useEffect(() => { read(x); }, [x]);
  return <div />;
}

export function RestState() {
  const [s, ...rest] = useState(0);
  const t = useState(1);
  useState(2);
  return <div>{s}{rest}{t}</div>;
}

export function MemoJsx() {
  const [n, setN] = useState(0);
  const el = useMemo(() => <button onClick={() => setN(n + 1)}>{n}</button>, [n]);
  return <div>{el}</div>;
}
```

```tsx
// /tmp/verif02/ex/v2_memo_handler.tsx
import { useState, useMemo } from "react";

export function InMemo() {
  const [items, setItems] = useState<number[]>([]);
  const el = useMemo(
    () => <button onClick={() => { items.push(1); setItems(items); }}>add</button>,
    [items],
  );
  return <div>{el}</div>;
}

export function Direct() {
  const [items, setItems] = useState<number[]>([]);
  return <button onClick={() => { items.push(1); setItems(items); }}>add</button>;
}

export function NonNull() {
  let x: any = 1;
  x! = { a: 1 };
  return <div>{x}</div>;
}
```

```tsx
// /tmp/verif02/ex/v3_update.tsx
export function Upd() {
  let x: any = 1;
  x!++;
  (x as any)++;
  return <div>{x}</div>;
}
```

```tsx
// /tmp/verif02/ex/v4_concise_sub.tsx
import { useEffect, useState } from "react";

export function ConciseSub() {
  const [n, setN] = useState(0);
  useEffect(() => window.addEventListener("resize", () => setN(1)), []);
  return <div>{n}</div>;
}

export function BlockSub() {
  const [n, setN] = useState(0);
  useEffect(() => { window.addEventListener("resize", () => setN(1)); }, []);
  return <div>{n}</div>;
}
```

Extrait du dump de `RestState` (`irdump`) :

```
=== component RestState (param props) ===
render_cfg:
  B0:
      let s = StateVal(0)  @18:9
      let rest = __arr_369  @18:12
      let t = StateVal(1)  @19:8
      expr HookMarker(2, Unknown)  @20:2
      => return Native<div>({#8 })[s, rest, t]
  edges: 
hooks:
  #0 State init=0  @18:8
  #1 State init=1  @19:8
  #2 State init=2  @20:2
```

### 6.10 Tests unitaires/intégration comme exemples « officiels »

- `cfg_builder.rs:1243-1248` `a_break_does_not_drop_the_cases_after_it` (#1) ;
  `1252-1267` `a_later_case_is_reachable_without_the_earlier_ones` ;
  `1299-1315` `labeled_break_targets_the_named_loop`.
- `expr_lower.rs:1637-1656` `spreads_and_computed_keys_keep_their_reads` (huit
  sources où `opts` doit rester lue).
- `hook_extractor.rs:963-992` `indirect_callback_body_is_the_call_not_unreachable`.
- `tests/cfg_exit_integrity.rs:138-162` `a_hook_after_a_guard_throw_is_not_conditional`
  (idiome garde-`throw` d'ADR-025) et `:166-185`
  `a_hook_after_an_early_return_is_still_conditional`.
- `tests/hook_in_terminator.rs:69-84` `every_position_agrees_the_analysis_was_truncated` :
  les trois orthographes (`ViaReturn`, `InCondition`, `InStatement`, fixture
  `tests/fixtures/hook_in_terminator/App.tsx:10-33`) doivent toutes
  déclencher un `analysis-limit` ; `:86-104`
  `a_truncated_component_is_not_credited_with_assurances` exige que
  `ViaReturn` affiche `suspended` et aucun `verified`.
- `tests/body_calls.rs:674-692` : `parse@5:29` et `stringify@6:34` — la
  position d'un appel dans un bras de ternaire et dans un opérande de `||`
  (ADR-039 §1) ; `:717-741` flèche concise héritant du site `items.forEach(`.

### 6.11 Boucles imbriquées : `for` étiqueté, `do…while`, `continue outer` (ajouté par la relecture)

Fichier déjà présent dans `/tmp/ex/` mais non exploité dans la première
version du dossier ; dump rejoué le 2026-09-28 avec `/tmp/irdump`.

```tsx
// /tmp/ex/e12_loops.tsx
export function Loops({ rows }: any) {
  let n = 0;
  outer: for (let i = 0; i < 3; i++) {
    do {
      if (rows[i]) continue outer;
      n++;
    } while (n < 10);
  }
  return <div>{n}</div>;
}
```

```
=== component Loops (param __p0) ===
render_cfg:
  B0:
      let __obj_22 = __p0
      let rows = __obj_22.rows  @1:24
      let n = 0  @2:6
      let i = 0  @3:18
      => jump B1
  B1:
      => branch (i Lt 3) ? B2 : B4
  B2:
      => jump B5
  B3:
      i := (i Add 1)  @3:32
      expr i
      => jump B1
  B4:
      => return Native<div>({#0 })[n]
  B5:
      => branch rows[i] ? B8 : B9
  B6:
      => branch (n Lt 10) ? B5 : B7
  B7:
      => jump B3
  B8:
      => jump B3
  B9:
      => jump B10
  B10:
      n := (n Add 1)  @6:6
      expr n  @6:6
      => jump B6
  edges: 0->1[U] 1->2[T] 1->4[F] 2->5[U] 5->8[T] 5->9[F] 8->3[Back] 9->10[U] 10->6[U] 6->5[Back] 6->7[F] 7->3[U] 3->1[Back]
```

Lecture :

- `for` : `init` (`let i = 0`, avec span car c'est un déclarateur) dans B0 ;
  B1 = en-tête, B2 = corps, B3 = `update`, B4 = sortie (réservés dans cet
  ordre, `cfg_builder.rs:659-662`). B2 (corps) ne contient qu'un `jump B5` :
le corps du `for` est un bloc `{ do … }`, et `DoWhileStatement` scelle
d'abord le bloc courant vers son propre bloc de corps (L329-330). L'`update` `i++` donne `i := (i Add 1)`
  **puis** `expr i` **sans span** (l'`ExprStmt` d'`update` est émis avec
  `None`, L696, alors que l'`Assign` interne garde le span de
  l'`UpdateExpression`).
- `do…while` : B5 = corps, B6 = test, B7 = sortie ; `6->5[Back]` est
  l'arête « vraie » du test (genre `Back`, pas `IfTrue`) et `6->7[F]` la
  sortie.
- `continue outer` : le frame `outer` a été poussé par `lower_for` avec le
  label en attente (`set_pending_label` puis `push_loop(exit, Some(update))`),
  donc `continue outer` vise B3 (`update` du `for`), pas le test du
  `do…while` : arête `8->3[Back]`. Un `continue` **non** étiqueté aurait visé
  B6.
- `n++` → `n := (n Add 1)` avec span + `expr n` avec span (instruction
  source).
- Trois arêtes `Back` (`8->3`, `6->5`, `3->1`) : trois points de widening
  potentiels pour le moteur.
- Numérotation : B5..B10 (boucle interne) ont des ids supérieurs à B3/B4,
  réservés avant d'abaisser le corps du `for` (3.6).

---

## 7. Contexte React nécessaire

- **Phase render vs phase commit.** Le corps d'un composant (le `render_cfg`)
  s'exécute pendant le rendu et doit être pur ; les effets
  (`useEffect`/`useLayoutEffect`/`useInsertionEffect`) s'exécutent **après**
  le commit. D'où deux familles de CFG : `render_cfg` et les `body_cfg` des
  `HookEntry::Effect` (ADR-004). Le lowering confond les trois variantes
  d'effets en `Effect` (la différence de timing n'est gardée que dans la
  provenance).
- **Règles des hooks.** Un hook doit être appelé au premier niveau d'un
  composant ou d'un hook custom, dans le même ordre à chaque rendu. Le
  lowering prépare `conditional-hook` en laissant **le label de chaque hook
  dans le CFG** (marqueurs) pour qu'on sache dans quel bloc il est appelé
  (`collect_hook_calls`, `src/engine/fixpoint.rs:1277-1340` : « first
  occurrence in block order wins »), et en représentant fidèlement les
  retours anticipés, `try`, `switch`, court-circuits. Convention de nommage
  d'un hook : `use` + majuscule/chiffre (`is_hook_name`).
- **`useState` / `useReducer`.** Retournent `[valeur, setter]` ; le setter est
  stable entre rendus ; une mise à jour est asynchrone et batchée ; le setter
  accepte un *updater* `prev => next`. Le lowering expose `StateVal(ℓ)` /
  `StateSetter(ℓ)` et ignore le réducteur de `useReducer` (le `dispatch`
  devient un `StateSetter`).
- **Deps et `Object.is`.** `useEffect`, `useMemo`, `useCallback` comparent
  chaque dep avec `Object.is` ; un objet/tableau/fonction recréé à chaque
  rendu défait la comparaison (stabilité référentielle). D'où l'importance des
  sites d'allocation (`ExprId`) et de `DepsArg` (absent = re-exécution à
  chaque rendu ; opaque ≠ liste vide), et de `DepsList::covering` pour les
  spreads.
- **`useRef`.** Un conteneur d'identité constante (`MarkerVal::StableRef`).
- **`useMemo`/`useCallback`.** Valeurs mémoïsées (`MemoVal`, `CallbackVal`) ;
  `useCallback(f)` mémoïse `f` elle-même (paramètres gardés).
- **Handlers.** Les props d'événements (`onClick`…) et `ref` callbacks sont
  appelés par React 0..N fois, hors rendu ; une fonction passée à un composant
  enfant peut être appelée par lui (render props, `children` en fonction) —
  d'où `HookEntry::Handler` et la règle « par fuite ».
- **JSX.** Majuscule = composant (`CompApp`), minuscule = élément hôte
  (`NativeElem`) ; les enfants d'un composant sont `props.children` ;
  `<Ctx.Provider value>` (nom pointé → `CompApp`).
- **Asynchronisme.** Ce qui suit un `await` s'exécute à un tour ultérieur de
  la boucle d'événements, hors de toute phase React (ADR-035) : arêtes
  `Await`.
- **Strict Mode.** En développement, React monte/démonte/remonte et double
  certains appels ; cité par `missing-cleanup` (« twice under StrictMode »),
  sans effet sur le lowering.
- **Context.** `useContext` n'est pas modélisé : `Custom` + `MarkerVal::Unknown`
  (lit ⊤) ; `createContext` au niveau module est reconnu par
  `collect_module_consts` (`ModuleConstInit::Context`, `src/lowering/mod.rs:326-338`).
- **Server Components.** Hors lowering (ADR-026 `"use client"`, règle
  `server-component-hook`).
- **Initialiseur paresseux.** `useState(() => calc())` n'appelle `calc` qu'au
  premier rendu ; `useState(calc())` l'appelle à chaque rendu. Le lowering
  garde l'argument tel quel dans `State.init` (un `FnLit` dans le premier
  cas, un `Call` dans le second) ; c'est la règle `lazy-init` qui fait la
  différence (assurance « no useState/useRef initializer re-runs work on
  every render » dans les sorties de la section 6).
- **Timing des trois effets.** `useInsertionEffect` avant les mutations du
  DOM, `useLayoutEffect` après les mutations mais avant la peinture,
  `useEffect` après la peinture. Tous trois deviennent `HookEntry::Effect` ;
  seule `HookProvenance.origin_hook` garde le nom (3.4).
- **Nettoyage des effets.** La fonction **retournée** par un effet est son
  *cleanup*, exécutée avant la ré-exécution et au démontage. Le lowering la
  conserve naturellement : c'est le `Return(e)` du `body_cfg` de l'effet
  (d'où l'importance de lire le `Return` d'un effet concis, 8.1.14).
- **Handlers natifs.** React DOM n'invoque, sur un élément hôte, que les
  props `on[A-Z]…` (événements synthétiques) et `ref` (callback appelé au
  montage/démontage) ; toute autre prop est une donnée DOM. C'est exactement
  le filtre `is_event_prop(name) || name == "ref"` de
  `collect_handlers_in_expr`. Sur un composant, en revanche, n'importe quelle
  prop fonction peut être appelée par l'enfant (render prop, `children` en
  fonction, `action={cb}` des formulaires React 19).
- **Namespaces React.** `React.useState(…)` et `import * as R from "react"; R.useMemo(…)`
  sont des hooks React ; `store.useThing()` (un receveur non-React) est un
  hook custom de provenance inconnue (`classify_callee`, 4.13).
- **Sémantique concrète de référence** : ADR-001 adopte **React-tRace** (Lee,
  Ahn, Yi — OOPSLA 2025), sémantique opérationnelle formelle de
  `useState`/`useEffect` (Tree Memory, boucle StepInit → StepEffect →
  StepCheck) ; les extensions (deps, `useMemo`, `useCallback`, `useRef`,
  objets) sont dites spécifiées dans `docs/semantics.md` — **fichier absent du
  dépôt au 2026-09-28** (à vérifier / signaler).

---

## 8. Subtilités, pièges, limites

### 8.1 Écarts constatés (vérifiés par exécution, non documentés ou mal documentés)

1. **Polarité des arêtes des diamants `&&`/`||`/`??`.** Les deux arêtes
   sortantes d'un court-circuit portent la même polarité (`&&` : `IfTrue` ×2 ;
   `||`/`??` : `IfFalse` ×2 ; `expr_lower.rs:763-780`, dumps 6.3). L'arête vers
   la jonction est mal étiquetée. Latent aujourd'hui (voir 4.12), mais tout
   futur consommateur de `EdgeKind` sur une jonction à prédécesseur unique
   lirait une garde de mauvaise polarité.
2. **Hooks imbriqués dans une expression non extraits.** `process_stmt` ne
   reconnaît un hook que si l'appel est la **racine** du RHS d'un `Let` ou d'un
   `ExprStmt` (à `TSAnnotated` près). Échappent : `a && useX()` / `a || useX()`
   / `a ?? useX()` (le RHS devient un `Assign`), `useContext(C).x`,
   `useRouter().push`, `f(useX())`, `[useState(0)]`, `<div>{useX()}</div>`
   (même après hoist de terminateur, le `Let` a un RHS JSX). Conséquence
   observée : `AndHook` reçoit `verified conditional-hook` (assurance
   fausse), `MemberHook`/`JsxHook` n'ont aucun hook ni `analysis-limit`. Le
   résidu JSX est reconnu dans le commentaire de clôture de #4 comme « Left in
   `docs/limitations.md` », mais n'y figure pas. À noter :
   `hook_call_detect.rs` (détection AST des composants, #122) **voit** ces
   formes (membres, JSX, logiques), donc le composant est détecté mais ses
   hooks non.
3. **`catch` part de l'état d'avant le `try`.** `lower_try` branche sur ⊤
   **avant** le corps du `try` : le chemin « le `try` a partiellement
   exécuté puis a levé » n'existe pas. Observé : `Partial` (6.9) est certifié
   `verified always-unstable-deps` alors que `cfg` vaut un objet frais quand
   `risky()` lève. La divergence documentée (L486-491) ne couvre que
   `return`-dans-`try`/`finally`.
4. **Déclarations de fonctions non hissées.** Le `Let f = FnLit` est émis à la
   position textuelle ; après un `return`, il n'est pas émis du tout
   (`lower_stmts` s'arrête). Observé : `e13` perd le handler `onClick={handle}`
   (`✓` au lieu d'un `Handler`). Avant l'usage mais déclarée plus bas
   (`init(); function init(){…}`), la règle `setter-in-render` reste correcte
   grâce au pré-scan syntaxique `let X = FnLit` (ADR-010 §B6, test `e10`),
   mais le point fixe, lui, voit `init` non lié au moment de l'appel.
5. **Handlers : un corps par nom.** `var_bodies` (L113-138) garde le dernier
   `FnLit` lié à un nom ; `onClick={flag ? f : g}` (deux `Let __t0` dans deux
   blocs) n'extrait que `g` (6.9). Le moteur, lui, sait joindre plusieurs sites
   (`locs` multi-sites, ADR-010 « Multi-site join »). Même effet pour une
   variable réaffectée (`let cb = f; if (x) cb = g;`) — non testé.
6. **Expressions de repli qui perdent des lectures.** `_ => opaque()` couvre
   `import(x)`, `#p in o`, `f<T>` (instanciation TS), intrinsèques V8 : leurs
   sous-expressions ne sont pas abaissées. Observé : `Lazy` (6.9) reçoit
   `verified missing-deps` alors que `path` est lu. Contredit le principe écrit
   à L28-31 (« Dropping a sub-expression outright is not an
   over-approximation »). Idem : les tests de `case k:` d'un `switch` ne sont
   pas abaissés (lectures perdues si `k` est une variable).
7. **Motif tableau vide sur `useState`.** `const [] = useState(0)` : le `Let`
   du temporaire est supprimé (`is_state_like && is_arr_temp`) et aucune
   projection ne réintroduit le label → le hook n'a pas de bloc d'appel, et
   `conditional-hook` le déclare inconditionnel (6.9). Viole l'invariant écrit
   sur `HookMarker` (« Every extracted hook leaves its label in the CFG »).
   Cas d'école, rare en pratique.
8. **Labels ≠ ordre d'appel.** Les labels suivent l'ordre des ids de blocs ;
   comme les jonctions sont réservées avant les blocs imbriqués d'une branche,
   un hook placé après un `if` imbriqué peut recevoir un label **inférieur** à
   un hook textuellement antérieur (6.9, `e18`). Sans effet connu sur les
   verdicts (les labels sont des identifiants, pas des positions React) ; à
   garder en tête pour tout raisonnement « ordre des hooks ».
9. **Commentaires périmés.** `expr_lower.rs:497-500` (composés « Add/Sub/Mul/Div »
   seulement) ; `cfg_builder.rs:398` (« Hoisted declarations: bind name but emit
   no CFG node ») ; `hook_extractor.rs:39` (un effet concis peut avoir
   `addEventListener` en `Return`) ; `src/ir/hooks.rs:263` (`None` pour
   `DepsArg`) ; test `logical_and_splits_blocks` (« join: … + Unreachable »,
   aujourd'hui `Return(undefined)`, `expr_lower.rs:1293`) ;
   `expr_lower.rs:1130-1131` (« the cell it writes is untracked either way »,
   faux pour `(x as any) = …`, 8.1.13) ; le doc de `lower_class`
   (`expr_lower.rs:49`, « `new X()` and `X.m()` are both ⊤ ») précède
   `Expr::New` (#158), qui fait désormais de `new X()` une référence
   **fraîche** (membres ⊤) ; trois renvois à une méthode
   `ImportCtx::callee_is_react` **qui n'existe plus** (aujourd'hui
   `classify_callee`) : `src/lowering/hook_detector.rs:44`,
   `src/lowering/mod.rs:164`, `src/lowering/mod.rs:279`. Hors périmètre mais
   à connaître : dans `src/ir/expr.rs`, le commentaire de `for_each_child`
   (L385-401) est placé **au-dessus** du doc de `subscription_listener`
   (L403-408), si bien que rustdoc l'attache à `subscription_listener` et que
   `for_each_child` (L463) n'a pas de doc.
10. **`??` perd les valeurs falsy non nullish.** Conséquence du partage de
    forme avec `||` combinée au narrowing `truthy` de la branche `then_`
    (4.12). Vérifié : `0 ?? 5` est lu `5` ; un `setX` gardé par `z === 0`
    disparaît (`e19`, 6.9). Seul court-circuit concerné (`&&` et `||` ont la
    bonne polarité côté jonction pour le narrowing, qui lit les cibles du
    `Branch`, pas les `EdgeKind`).
11. **Paramètre de reste ignoré.** `(...args) => …` : `params.rest` n'est pas
    lu (4.10). Le nom devient une capture libre (FP `missing-deps` vérifié,
    `e20`) ; si la fonction est greffée, `args` lit une liaison extérieure
    au lieu de ⊤ (direction FN possible, non testée).
12. **`for (motif of …)` / `for (o.x of …)` sans écriture.** Les cibles
    pré-déclarées non identifiantes ne sont pas écrites (4.4) ; les liaisons
    restent à leur valeur antérieure (vérifié, `e21`).
13. **Cible d'affectation TS-enveloppée non écrite.** `(x as any) = v`,
    `x! = v`, `x!++`, `(x as any)++` : aucune écriture de `x` (4.9, 4.11.3).
    Vérifié : `TsTarget` certifié `verified always-unstable-deps`, le témoin
    sans `as` donne `warn always-unstable-deps (line 13:2)`
    (`/tmp/verif02/ex/v1_ts_target.tsx`). C'est une sous-approximation (la
    liaison périmée est une affirmation) ; une correction « à la racine »
    consisterait à peler les enveloppes TS de la cible avant
    `assign_target_ident`/`assign_target_member`, comme `lower_expr` le fait
    pour les expressions (suggestion du relecteur, non évaluée).
14. **Subscription d'un effet concis non extraite.** `useEffect(() =>
    target.addEventListener("evt", fn), deps)` : l'appel est dans le
    `Return` du corps, que `collect_subscriptions_in_cfg` ne parcourt pas
    (L39). Vérifié (`/tmp/verif02/ex/v4_concise_sub.tsx`) : `ConciseSub ✓`
    avec `verified missing-cleanup`, alors que la forme bloc donne `warn
    missing-cleanup (line 11:20)` — une assurance fausse.
15. **Handlers construits dans un corps de `useMemo`/`useCallback`.**
    `extract_handlers` ne voit que le CFG de render (et les `FnLit` qui y
    restent) : aucun `HookEntry::Handler` pour un `onClick` d'un élément
    mémoïsé (vérifié, `InMemo`, 4.14). Impact par règle à vérifier
    (`state-mutation` le trouve par ailleurs).
16. **Formes de `useState` hors `const [a, b] = useState()`.** Tuple non
    destructuré lu comme `StateVal` ; appel en instruction marqué `Unknown`
    (déclenche un `analysis-limit` trompeur) ; reste `...rest` lié à un
    temporaire supprimé (4.13, `RestState`).
17. **`??` et `expand_guard`** (hors périmètre, conséquence du partage de
    forme avec `||`). `expand_guard` (`src/engine/guards.rs:1018-1034`)
    décompose une garde « `__tN` falsy » d'un diamant dont l'arête vers `rhs`
    est `IfFalse` en « `a` falsy ∧ `b` falsy ». Pour `a || b` c'est exact ;
    pour `a ?? b`, si `a` vaut `0`/`""`/`false`, `b` n'est pas évalué et
    rien n'est prouvé sur lui. Non reproduit par un exemple ; **à vérifier**
    avec l'auteur du chapitre moteur.

### 8.2 Précision vs soundness (choix assumés)

- **`?.`** abaissé comme `.` : valeur sur-approximée, effets d'un appel
  optionnel supposés toujours exécutés (seulement des FP possibles).
- **`??` traité comme `||`** : même CFG ; ce n'est **pas** un simple choix de
  précision, car combiné au narrowing il devient une sous-approximation
  (8.1.10).
- **Défauts de motifs** : expression émise inconditionnellement, valeur non
  jointe à la liaison. Pour `const { cb = () => setX(1) } = props`, la
  liaison `cb` vaut `props.cb` et **pas** `props.cb ⊔ FnLit` ; la lecture et
  les effets du défaut (création du `FnLit`) sont visibles comme instruction,
  mais un appel ultérieur `cb()` ne résout pas le corps du défaut (à vérifier :
  le pré-scan des setters par `FnLit` ne voit pas cette liaison).
- **Spreads, clés calculées, accesseurs** : gardés pour leurs lectures sous
  clés synthétiques, non modélisés (#76) ; un membre écrit avant un spread ne
  répond de rien (`members_after_last_spread`).
- **`IndexAccess`** : toujours ⊤ côté domaine, donc un spread gardé comme
  élément de tableau ne ment pas.
- **Argument spread d'appel** : sorti de la liste d'arguments (émis pour
  effet) pour ne pas décaler les paramètres au splice.
- **`for…in/of`** : borne inconnue, variable ⊤.
- **`switch`** : `default` optionnel, test des cases non modélisé.
- **`try`** : force `must` perdue sur le chemin `return`-dans-`try` ; voir aussi
  8.1.3.
- **Classes** : corps visibles, `new X()`/`X.m()` ⊤ ; `this` ⊤.
- **`with`** : noms traités comme liaisons ordinaires (« can only invent a
  read, never lose one »).
- **`i++` en valeur** : post-écriture pour préfixe et postfixe.
- **Types TS** effacés, jamais utilisés pour narrower (ADR-020 §10).

### 8.3 Limites de `docs/limitations.md` concernant le lowering

- Seul défaut confirmé listé : « A hook reached only through a `return` —
  Reported, but with no line or column, since `Terminator::Return` carries no
  span » (#140). « It costs a position, never a finding. »
- FN : spreads/clés calculées non modélisés (#76) ; inlining d'utilitaires en
  position d'instruction seulement (#52) ; `useContext` non modélisé (#28,
  363 sites) ; sept hooks React non modélisés (#27) ; `memo`/`forwardRef`
  (#64).
- Section « Every finding carries a position » : décrit ADR-039.

### 8.4 Dette connue

- `docs/TODO.md` n'est plus qu'une redirection vers le tracker (depuis
  2026-08-27).
- Issues ouvertes en `area/lowering` : #140, #158 (corrigée dans le code, à
  fermer ?), #76, #64.
- `CFG::validate` jamais appelé sur la sortie brute du lowering (seuls
  appels : `debug_assert!` après splice, `src/ir/splice.rs:251-255`, et un
  test, `splice.rs:898`).
- Doublon `is_event_prop_key` / `is_event_prop` (3.9) ; `build_cfg` public
  sans appelant de production ; renvois à `ImportCtx::callee_is_react`
  (méthode disparue, 8.1.9).
- Aucun des écarts 8.1.1-8.1.17 n'a d'issue sur le tracker au 2026-09-28
  (recherche `gh issue list --state all` sur « coalesce », « nullish »,
  « try », « catch », « hoist », « handler », « rest »… : rien de
  correspondant).
- `docs/ir.md` et `docs/semantics.md`, cités par ADR-003 et ADR-001, absents.

### 8.5 Pièges pour qui modifie ce code

- Ne jamais ajouter de bras `_ => {}` dans `lower_stmt`, `for_each_child`,
  `HookEntry::body_cfg`, `lower_binop` : l'exhaustivité est la garantie
  (#77).
- Toute nouvelle instruction synthétique doit porter un span (ADR-039).
- Toute expression non représentable doit passer par `lower_for_effect` plutôt
  qu'être jetée.
- Toujours `add_edge` après `seal_with(Jump|Branch)` ; toujours tester
  `is_terminated()` avant de sceller une fin de branche (sinon
  `debug_assert`).
- Le préfixe `__arr_` est lu par `hook_extractor` ; `__term_`, `__tN`,
  `__obj_`, `__dstr_`, `__pN` sont des noms réservés implicites.
- Un `Unreachable` ne doit être produit que quand le contrôle **ne continue
  pas** (ADR-025).
- `lower_expr` ne doit jamais sceller un bloc en `Return`/`Unreachable` : les
  seules fentes qu'il fait (diamants, `split_at_await`) laissent un bloc
  **ouvert**. C'est ce qui garantit qu'une jonction de diamant a toujours deux
  prédécesseurs (et rend latente la polarité fausse de 8.1.1).
- Toute nouvelle forme de cible d'affectation doit **écrire** la liaison
  (au pire à ⊤) : le repli `ExprStmt(rhs)` n'est acceptable que pour une
  cellule réellement non suivie (8.1.13).
- Modifier `is_event_prop` sans `is_event_prop_key` (ou l'inverse) désaligne
  handlers et spans (3.9).
- Toute nouvelle passe qui lit les corps de hooks doit se souvenir que
  `extract_hooks` **déplace** les `FnLit` d'effets/memos/callbacks hors du
  CFG de render (4.13) : ce qui les cherche dans le render ne les trouvera
  plus (8.1.15).

---

## 9. Glossaire

| Terme | Définition | Où |
|---|---|---|
| lowering (abaissement) | traduction AST oxc → IR (CFG + table des hooks) | `src/lowering/` |
| CFG | graphe de blocs de base + arêtes typées, entrée `entry` | `src/ir/cfg.rs:60-65` |
| bloc de base (`BasicBlock`) | suite linéaire de `Stmt` terminée par un `Terminator` | `src/ir/cfg.rs:5-10` |
| terminateur | `Jump`, `Branch`, `Return`, `Unreachable` | `src/ir/cfg.rs:12-31` |
| sceller (*seal*) | fermer le bloc courant avec un terminateur et l'insérer dans `blocks` | `BlockBuilder::seal_with`, `cfg_builder.rs:173` |
| bloc de jonction (*join*) | bloc où convergent les branches d'un `if`/ternaire/diamant/`try` | `lower_if`, `lower_ternary`… |
| bloc orphelin | bloc présent dans `blocks` sans prédécesseur (jonction de deux `return`) | test `cfg_builder.rs:1348` ; `CFG::reachable_blocks` |
| diamant | motif `Let t = a; Branch(t); rhs: t := b; join` des court-circuits | `lower_logical`, `expr_lower.rs:739` |
| arête `Back` | arête de retour de boucle ; seule à déclencher le widening | `EdgeKind`, `cfg.rs:33-45` |
| arête `Await` | coupure à un `await` ; successeurs « post-await » | ADR-035 ; `CFG::post_await_blocks` |
| post-await | clôture des successeurs des arêtes `Await` | `src/ir/cfg.rs:121-144` |
| frame de boucle | cible `break`/`continue` (+ label) sur la pile | `LoopFrame`, `cfg_builder.rs:95-100` |
| label en attente | label d'un `LabeledStatement` consommé par la boucle suivante | `pending_label`, `set_pending_label` |
| dispatch opaque | chaîne de `Branch(true)` choisissant une case de `switch` | `lower_switch`, `cfg_builder.rs:786-817` |
| fall-through | (1) corps qui finit sans `return` → `Return(undefined)` (ADR-025) ; (2) case de `switch` sans `break` → case suivante | `into_cfg` ; `lower_switch` |
| opaque / ⊤ | valeur inconnue, `Expr::SummaryVal(SummaryValue::Top)` | `expr_lower.rs:24` |
| lecture pour effet | sous-expression gardée en `ExprStmt` pour ses lectures/effets | `lower_for_effect`, `expr_lower.rs:32` |
| clé synthétique | clé de champ inatteignable par un vrai `FieldAccess` (`...N`, `[computed]N`, `[accessor]N`, `[method]N`, `[field]N`, `[static]N`, `[extends]N`) | `synthetic_key`, `expr_lower.rs:42` ; `SPREAD_KEY_PREFIX` |
| temporaire | variable de lowering (`__tN`, `__arr_N`, `__obj_N`, `__dstr_N`, `__pN`, `__term_N`) | 3.8 |
| site (d'allocation) | `ExprId` d'un nœud allouant (`ObjectLit`, `ArrayLit`, `FnLit`, `New`) ; clé du tas abstrait | `src/ir/types.rs:11-15`, ADR-010 |
| site (d'écriture, churn) | ligne d'écriture non-handler d'un corps d'effet/render/memo dans la preuve de convergence | `src/engine/churn.rs:39-43` (#162) |
| `ExprIds` | compteur de sites partagé par les corps d'un `LowerCtx` | `cfg_builder.rs:34-43` |
| `LowerCtx` | contexte par fichier : `SourceMap`, `ExprIds`, `JsxOrigins` | `cfg_builder.rs:52-74` |
| `Candidate` | fonction de premier niveau retenue par un détecteur (nom, params, corps, drapeau `expression`) | `src/lowering/mod.rs:133-160` |
| flèche concise | `x => expr` ; drapeau `expression` ; corps = `Return(expr)` | `build_expr_fn_body_cfg` |
| label (`HookLabel`) | numéro d'un hook dans un composant (0, 1, …), aussi id de slot | `src/ir/types.rs:2` |
| slot | cellule d'état d'un `useState`/`useReducer`, identifiée par son label (qualifiée par composant : `QualifiedSlot`) | `src/ir/types.rs:6-9` |
| marqueur | `HookMarker(ℓ, MarkerVal)` ou `StateVal`/`MemoVal`/… : trace du label au site d'appel | `src/ir/expr.rs:281-294` |
| `MarkerVal` | ce que lit la liaison d'un marqueur : `Undefined`, `StableRef`, `Unknown`, `Summary` | `src/ir/expr.rs:311-331` |
| `HookEntry` | description d'un hook (état, effet, memo, callback, ref, custom, handler) | `src/ir/hooks.rs:247-313` |
| handler | corps qu'un tiers (React, un enfant) peut invoquer 0..N fois ; `HookEntry::Handler` | `extract_handlers` |
| subscription | handler issu d'un `addEventListener("evt", fn)` dans un effet | `extract_subscriptions` ; `Expr::subscription_listener` |
| fuite (*escape*) | critère de joignabilité d'un handler : passé à un élément/composant | doc `hook_extractor.rs:84-99` |
| provenance | ligne `label → (hook d'origine, spécificateur, fichier, inliné?)` | `HookProvenance`, `hooks.rs:17-42` |
| `HookOrigin` | origine prouvée d'un binding importé : `React`, `File`, `Package` | `import_resolution.rs:41-55` |
| fail-closed | un binding importé n'est classé que par ce que prouve son import | doc `ImportCtx`, `hook_extractor.rs:527-530` |
| hoist de terminateur | déplacement d'un terminateur contenant un hook dans un `Let __term_N` | `hoist_terminator_hooks`, `hook_extractor.rs:321` |
| `DepsArg` | argument de deps : `Absent`, `Opaque`, `List` | `src/ir/hooks.rs:102-107` |
| arity | longueur de la source d'un tableau : `Exact(n)` ou `AtLeast(n)` | `src/ir/hooks.rs:52-58` |
| covering | éléments d'une liste de deps qui couvrent une lecture (hors spreads) | `DepsList::covering`, `hooks.rs:203-215` |
| splice (greffe) | insertion du CFG d'un hook/utilitaire au site d'appel ; `Return` → `Let bound = e; Jump(join)` | `src/ir/splice.rs:64` |
| span / `SourceRange` | position (fichier, ligne 1-indexée, colonne 0-indexée) d'un nœud | `src/ir/source_range.rs:36-42` |
| witness (témoin) | position la plus intérieure déjà traversée par la marche, héritée par ce qui n'en a pas | ADR-039 §3 ; `src/engine/setters.rs:1660-1674` ; `rules/api/witness.rs:86` (`Step`) |
| must / may | faits certains sur tous les chemins (seuls à ouvrir l'Error) vs possibles | `MustResult`, `May`, `src/rules/api/query.rs:110-135` |
| guard (garde) | contrainte de branche sous laquelle un site s'exécute | `src/engine/guards.rs` (`site_guards`, `expand_guard`) |
| churn | relation/graph de ré-exécutions en boucle entre écritures et deps | `src/engine/churn.rs` |
| reviver | site d'écriture qui ranime une autre écriture dans la boucle ; exclu s'il est prouvé tirer au plus une fois (point fixe minimal) | `src/engine/churn.rs:44-49`, `guards.rs:36-45` |
| seed | relation « slot initialisé depuis une prop » (`useState(prop)`) | `src/engine/seeds.rs` (ADR-031) |
| anchor | ancre d'une règle déclarative Tier-A (relation sur laquelle elle itère) | `src/rules/declarative/schema.rs:111` |
| pré-en-tête (*preheader*) | bloc qui précède l'en-tête d'une boucle ; y sont émis `init` d'un `for` et l'expression itérée d'un `for…of/in` | `lower_for`, `lower_iter_loop` |
| en-tête (*header*) | bloc cible des arêtes `Back`, qui porte le `Branch` du test | `lower_while`/`lower_for`/`lower_iter_loop` |
| préambule de paramètres | liaisons émises en tête de corps pour les paramètres déstructurés (`__p{i}` + motif) | `inject_param_preamble`, `cfg_builder.rs:952` |
| cible d'affectation | côté gauche d'un `=`/`op=` : identifiant (`Assign`), membre (`MemberWrite`), motif (`__dstr_N`) | `assign_target_ident`, `assign_target_member`, `lower_assignment_target` |
| `ComponentIR` / `HookIR` / `FunctionIR` | produit du lowering pour un composant, un hook custom, un utilitaire | `src/ir/component.rs:59`, `src/ir/hook_ir.rs:11`, `utility_lowerer.rs` |
| `FnLit` | littéral de fonction : `ExprId` + paramètres + `Arc<CFG>` du corps, abaissé par un `BlockBuilder` propre | `src/ir/expr.rs`, `expr_lower.rs:442-471` |
| `CompApp` / `NativeElem` | élément JSX de composant (nom en majuscule ou pointé) / élément hôte | `lower_jsx_element`, `expr_lower.rs:799-852` |
| `prop_spans` | spans des props `onX` d'un `NativeElem`, lus pour `Handler.span` | `lower_jsx_props`, `expr_lower.rs:856` |
| `ImportCtx` | contexte d'imports de l'extracteur (origines, espaces de noms React, hooks locaux, fichier) | `hook_extractor.rs:531` |
| `ResolvedHookCall` | identité résolue d'un appel de hook (nom d'origine, React ?, spécificateur, fichier) | `hook_extractor.rs:467` |
| `state_temps` | table temporaire `__arr_N` → label, pour réécrire `__arr_N[0]`/`[1]` | `extract_hooks`, `rewrite_expr` |
| `var_bodies` | table nom → corps de fonction de la pré-passe des handlers | `extract_handlers`, `hook_extractor.rs:113` |
| coupure d'`await` | scellement du bloc courant par `Jump` + arête `Await` | `BlockBuilder::split_at_await`, `cfg_builder.rs:195` |
| liaison périmée (*stale binding*) | variable laissée à sa valeur antérieure alors que le programme l'a écrite — une affirmation fausse, donc une sous-approximation | `expr_lower.rs:1027-1029`, 8.1.12-13 |
| assurance | ligne `verified …` / `suspended …` sous `--info` | sortie CLI |
| FN / FP | faux négatif (interdit) / faux positif (toléré) | CLAUDE.md |

---

## 10. Plan pédagogique suggéré

### 10.1 Prérequis

- Chapitre « IR » (types `Expr`, `Stmt`, `CFG`, `HookEntry`) — ce dossier les
  couvre en 3.
- Notions d'AST et de CFG ; dominance (pour comprendre ce que le lowering doit
  préserver).
- Chapitre « React » (section 7).
- Pour les renvois : chapitres « domaines » (narrowing, ⊤, stabilité),
  « moteur » (point fixe, widening sur `Back`, splice), « règles »
  (`conditional-hook`, `missing-deps`, assurances).

### 10.2 Ordre d'exposition (du plus simple au plus difficile)

1. Pourquoi une IR en CFG (ADR-003, alternatives) ; la chaîne
   parse → détecteurs → `Candidate::build_cfg` → `extract_*`.
2. La machine à blocs (`BlockBuilder`, sceller/ouvrir, arêtes) sur un corps
   linéaire ; `into_cfg` et le fall-through (ADR-025).
3. `if`/`else`, retour anticipé, bloc orphelin (exemple 6.2).
4. Expressions structurelles (littéraux, opérateurs, appels, membres, JSX) ;
   principe « garder les lectures » (`lower_for_effect`).
5. Diamants : ternaire, `&&`, `||`, `??` ; décision ADR-020 §1 (exemple 6.3).
6. Boucles, `break`/`continue`/labels, arêtes `Back` et widening.
7. `switch` et `try` : histoire de deux sous-approximations corrigées (#1, #2).
8. Motifs et affectations ; nommage des temporaires (ADR-020 §11).
9. Fonctions imbriquées, classes, sites d'allocation (ADR-010, #134).
10. `await` et arêtes `Await` (ADR-035) ; positions synthétiques (ADR-039).
11. Extraction des hooks : classification par provenance, marqueurs,
    destructuration d'état, corps de hooks (exemple 6.5).
12. Handlers et subscriptions (règle « par fuite »).
13. Hoist de terminateur (#4) et flèches concises (#5) — une paire de
    corrections inséparables.
14. Limites et écarts (section 8) : comment on les trouve (exemples 6.9).

### 10.3 Schémas à dessiner

- Pipeline global (1.3), avec la frontière « jamais d'AST après le lowering ».
- Gabarits de CFG par construction : `if`, `while`, `for` (avec bloc
  `update`), `do…while` (arête `Back` depuis le test), `for…of` (pré-en-tête +
  `Branch(true)`), `switch` (chaîne de dispatchs + fall-through), `try`
  (`Branch(⊤)` + finally sur la jonction), diamants `&&`/`||`, ternaire,
  `await` (chaîne `Await`).
- Graphe réel de l'exemple 6.2 (15 arêtes) avec coloration des arêtes par
  `EdgeKind`, et mise en évidence du bloc qui porte `useState` hors de la
  dominance de la sortie.
- Avant/après `extract_hooks` sur l'exemple 6.1 (disparition de `__arr_N`,
  apparition des marqueurs).
- Frise de numérotation des blocs (pourquoi B6 précède B7..B9 ; pourquoi les
  labels ne suivent pas l'ordre source).
- Splice d'un hook concis (`useCount`) dans son appelant : `Return` →
  `Let`/`Jump(join)`, et le cas `Unreachable` laissé seul (ADR-025).

### 10.4 Exercices

1. Dessiner à la main le CFG de `for (let i = 0; i < n; i++) { if (a[i])
   continue; f(); }` puis le vérifier contre le test
   `continue_reaches_the_loop_header` (`cfg_builder.rs:1144-1169`).
2. Expliquer pourquoi `continue` doit être une arête `Back` (commentaire
   L390-393) en construisant une boucle qui ne converge pas sinon.
3. Donner le CFG d'un `switch` à trois cases dont la deuxième n'a pas de
   `break`, et justifier que `setMsg` de la case 1 n'est qu'un Warning
   (exemple 6.4).
4. Montrer que câbler `throw` à la jonction du splice produit un FP Error sur
   l'idiome garde-`throw` (ADR-025 §2, test `cfg_exit_integrity.rs:138-162`).
5. Pour `const { a = f() } = props`, lister les instructions émises et dire
   quelles lectures sont préservées.
6. Montrer sur `flag && useEffect(…)` que le hook n'est pas extrait, et
   proposer une correction « à la racine » compatible avec CLAUDE.md (piste :
   généraliser la normalisation du #4 — lier tout sous-appel de hook dans un
   `Let` au point de son évaluation — plutôt que d'ajouter un cas dans
   `process_stmt`).
7. Construire un exemple où la polarité erronée des arêtes de jonction de
   `lower_logical` changerait un verdict si `site_guards` la lisait.
8. Reproduire l'écart du `catch` (6.9, `e9`) et proposer une modélisation
   sound (piste : arête vers le `catch` depuis chaque point du `try` où une
   exception peut survenir, ou depuis la fin de chaque bloc du `try`).
9. Sur `useEffect(() => el.addEventListener("x", f), [])`, montrer que la
   subscription n'est pas extraite (8.1.14) et proposer la correction qui
   réutilise un parcours existant (`CFG::for_each_expr` visite déjà les
   `Return`).
10. Écrire le CFG de `/tmp/ex/e12_loops.tsx` (6.11) à la main et prédire la
    cible de `continue outer` puis celle d'un `continue` non étiqueté.

---

## Vérification

Relecture-vérification du 2026-09-28, dépôt au commit `e67b10a` (arbre de
travail propre hors `docs/manuscrit/`), binaires `target/debug/reactant` et
`/tmp/irdump/target/debug/irdump` à jour (`cargo build` sans recompilation).

### Méthode

- **Extraits de code** : les 50 blocs portant un en-tête `// chemin:A-B`
  ont été comparés ligne à ligne à `sed -n 'A,Bp' chemin` par un script
  temporaire (`/tmp/verif02/check.py`, lecture seule) : **50/50 verbatim**,
  aucune correction nécessaire. Les 13 sources `.tsx` citées (dont les cinq
  ajoutées) ont été comparées aux fichiers `/tmp/…` : identiques.
- **Note** : pendant la relecture, un processus tiers a vidé
  `/tmp/irdump/target` (le tmpfs était plein) ; tous les dumps du dossier
  ont été rejoués **avant** cette suppression. Pour les régénérer, il faut
  recompiler `/tmp/irdump` (`cargo build` dans ce répertoire, dépendance
  `reactant` par chemin) — ou le réécrire si `/tmp/irdump/src` a disparu.
- **Dumps d'IR** (section 6) : les huit fichiers `/tmp/ex/e*.tsx` cités ont
  été comparés au texte du dossier (sources identiques) et chaque ligne des
  huit dumps a été retrouvée, dans l'ordre, dans la sortie actuelle
  d'`irdump` (`/tmp/verif02/ex.py`). Seule différence : `resolved_file` est
  imprimé relatif (`"e5_hooks.tsx"`) quand `irdump` est lancé depuis
  `/tmp/ex`, absolu sinon — sans conséquence.
- **Sorties d'analyseur** : les contre-exemples de 6.9 (`e8`, `e9`, `e10`,
  `e13`–`e21`) ont été rejoués avec `reactant check --info --show-clean
  --no-color` ; tous les verdicts cités sont reproduits à l'identique
  (notamment `Coalesce ✓ verified setter-in-render` contre `Control warn …
  (line 16:4)`, `Partial ✓` contre `Direct warn always-unstable-deps`,
  `AndHook … verified conditional-hook`, `Order error conditional-hook
  [hook:1] (line 8:10)` avec `#0 State init="y"`).
- **Références `chemin:ligne` en prose** : vérifiées une à une par `sed`
  pour les sections 1 à 5, 6.10, 8 et 9 (≈ 150 références).
- **Inventaire public** : `grep -n "pub fn\|pub struct\|pub enum\|pub
  trait\|pub type\|pub const\|pub(crate)\|pub(super)"` sur les trois
  fichiers, puis recherche de chaque nom dans le dossier.
- **Tests** : `cargo test -q --lib lowering::` → 106 passés.
- **Couverture des variantes oxc** : les variantes de `Expression`
  (`oxc_ast-0.138.0/src/ast/js.rs:81-165`) absentes des bras de `lower_expr`
  sont exactement `BigIntLiteral`, `RegExpLiteral`, `MetaProperty`,
  `ImportExpression`, `PrivateInExpression`, `TSInstantiationExpression`,
  `V8IntrinsicExpression` — la liste de 4.11.10 est confirmée.

### Corrections apportées

1. En-tête : `--verbose` n'imprime pas « que le graphe de symboles et les
   statistiques de point fixe » — il imprime aussi la racine de découverte,
   les alias tsconfig et les imports suivis (`src/driver/mod.rs:172-230`).
2. 2.5 : les nombres de tests intégration étaient donnés dans l'ordre
   alphabétique d'exécution de cargo, pas dans l'ordre des `--test` cités ;
   correspondance explicitée.
3. 3.5 : « les composants et les hooks d'un même fichier sont numérotés
   depuis 0 chacun » était ambigu — tous les composants d'un fichier
   partagent **un** compteur (un seul `LowerCtx` par point d'entrée), les
   hooks custom un autre, les utilitaires un troisième.
4. 6.10 : `tests/hook_in_terminator.rs:256-266` n'existait pas (fichier de
   104 lignes) → `tests/hook_in_terminator.rs:69-84` (et `:86-104`).
5. 4.15 : l'écart « effet concis avec `addEventListener` », noté « à
   vérifier », est désormais **vérifié** (8.1.14).

### Ajouts

- 1.3 : même chaîne `extract_*` pour les hooks custom ; aucune pour les
  utilitaires.
- 3.8 : unicité des temporaires (par fichier vs par corps).
- **3.9 (nouveau)** : inventaire exhaustif des items publics / `pub(crate)`
  / `pub(super)` du périmètre avec leurs utilisateurs réels (dont
  `LowerCtx::new`, jusque-là non mentionné ; `build_cfg` sans appelant de
  production ; `is_event_prop`/`prop_to_event` utilisés par
  `rules/helpers/render_tree.rs` et `engine/render_deps.rs` ; doublon
  `is_event_prop_key`).
- 4.2 : sort d'un label sur un non-boucle ; 4.4 : condition de boucle
  abaissée dans l'en-tête, double arête `Back` d'un `continue` de `for`,
  `ExprStmt` sans span d'`init`/`update`, absence de portée lexicale ;
  4.5 : ordre d'allocation du `switch`, `switch` vide, label de `switch`
  ignoré.
- 4.9 : clé numérique de motif ; tableau des quatre fonctions de la voie
  « affectation » (`assign_target_member`, `lower_member_target_expr`,
  `lower_assignment_maybe_default` n'étaient que dans l'inventaire) ;
  **écart vérifié** des cibles TS-enveloppées.
- 4.11.2 / 4.11.3 : `delete` sur non-membre, champs privés `#p`, mises à
  jour `x!++` sans écriture (vérifié).
- 4.11.9 : `jsx_element_name`, `jsx_member_obj_name`,
  `lower_jsx_fragment`, valeurs d'attributs, ordre enfants-avant-props
  contraire à JavaScript.
- 4.13 : formes de `useState` hors destructuration tableau (vérifiées),
  `useRef()` → `Null`, arguments jetés de `useReducer`.
- 4.14 : pré-passe incluant les temporaires, props `on*` non résolues non
  redescendues, handlers absents des corps de `useMemo` (vérifié),
  `prop_to_event`.
- 4.15 : exemple vérifié `ConciseSub`/`BlockSub` ; portée de la recherche.
- 5.1 : quatre écarts supplémentaires ADR-003 ↔ code.
- 6.9 : six contre-exemples nouveaux (`/tmp/verif02/ex/v1`–`v4`) ;
  **6.11 (nouveau)** : boucles imbriquées étiquetées (`e12_loops.tsx`, déjà
  présent dans `/tmp/ex` mais inexploité).
- 7 : initialiseur paresseux, timing des trois effets, cleanup, filtre des
  handlers natifs, espaces de noms React.
- 8.1.9 : commentaires périmés supplémentaires (`callee_is_react`,
  `lower_class`/`new`, repli de `lower_assignment_target`, doc mal placée de
  `for_each_child`) ; **8.1.13 à 8.1.17 (nouveaux)**.
- 8.4 / 8.5 : dette et pièges supplémentaires.
- 9 : quinze entrées de glossaire (pré-en-tête, en-tête, préambule, cible
  d'affectation, `ComponentIR`/`HookIR`/`FunctionIR`, `FnLit`,
  `CompApp`/`NativeElem`, `prop_spans`, `ImportCtx`, `ResolvedHookCall`,
  `state_temps`, `var_bodies`, coupure d'`await`, liaison périmée).
- 10.4 : exercices 9 et 10.

### Ce qui reste incertain

- **8.1.17** (`??` dans `expand_guard`) : déduit de la lecture de
  `src/engine/guards.rs:1018-1034`, non reproduit par un exemple.
- **8.1.15** : impact exact, règle par règle, de l'absence de `Handler` pour
  un JSX construit dans un `useMemo` (au moins `state-mutation` n'est pas
  aveugle).
- **4.13**, `const t = useState(1)` : que le moteur ne reconnaisse pas
  `t[1]` comme setter est déduit du dump (`IndexAccess` sur `StateVal`), pas
  vérifié par une règle.
- **4.11.3** : la remarque critique sur « over-counts by one, never under »
  (valeur de `a = i++`) reste à confronter au chapitre domaines.
- **`TSEnumDeclaration`** traité comme effacé (4.2) : sans conséquence
  connue, non testé.
- **`useRef()` → `Lit(Null)`** au lieu de `undefined` : conséquence sur le
  domaine non évaluée.
- Les correctifs suggérés (peler les enveloppes TS d'une cible, lire le
  `Return` des effets concis, généraliser le hoist du #4) sont des pistes du
  relecteur, **pas** des décisions du projet ; aucune issue n'existe pour
  les écarts 8.1.1-8.1.17 (recherche sur le tracker au 2026-09-28).
- Les fichiers d'exemple `/tmp/ex/` et `/tmp/verif02/` sont temporaires ; le
  tmpfs `/tmp` était plein à 99 % pendant la relecture (5,8 Go), ils peuvent
  disparaître : les sources utiles sont recopiées dans ce dossier.
