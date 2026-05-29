# Guide d'implémentation — reactant-analyzer

Étapes ordonnées pour implémenter l'analyseur depuis zéro. Chaque étape est testable indépendamment. Ne pas passer à l'étape suivante sans que les tests de l'étape courante passent.

Référence : `docs/PRD.md`, `docs/ir.md`, `docs/adr/`.

---

## Étape 1 — Nettoyage du codebase

**Fichiers à supprimer :**
- `src/impl_/cfg_builder.rs`
- `src/impl_/worklist.rs`
- `src/impl_/mod.rs`
- Retirer `mod impl_;` dans `src/lib.rs`

**Fichiers à garder sans modification :**
- `src/core/aval.rs` — valeurs abstraites de base (évoluera à l'étape 4)
- `src/core/abs_env.rs` — environnement abstrait (évoluera à l'étape 4)
- `src/rules/*` — garder comme oracles de régression end-to-end uniquement

**Vérification :** `cargo test` passe toujours après suppression.

---

## Étape 2 — Types IR

Créer `src/ir/`. Seulement des types — pas de logique.

### `src/ir/mod.rs`
Réexporter tous les sous-modules.

### `src/ir/types.rs`
Types de base :
```rust
pub type Symbol = String;  // interning possible plus tard
pub type HookLabel = usize;
pub type BlockId = usize;
pub type Var = String;
```

### `src/ir/expr.rs`
Enum `Expr` selon `docs/ir.md`. Points clés :
- `StateVal(HookLabel)` et `StateSetter(HookLabel)` = expressions spéciales résolues par les domaines à l'analyse
- `MemoVal(HookLabel)` idem
- `FnLit { params, body_cfg }` contient un `CFG` → utiliser `Box<CFG>` pour la récursion
- `TsAnnotated { expr: Box<Expr>, ty: TsType }` = wrapper optionnel

### `src/ir/cfg.rs`
```rust
pub struct BasicBlock {
    pub id:    BlockId,
    pub stmts: Vec<Stmt>,
    pub term:  Terminator,
}

pub enum Terminator {
    Jump(BlockId),
    Branch { cond: Expr, then_: BlockId, else_: BlockId },
    Return(Expr),
    Unreachable,
}

pub enum EdgeKind { Unconditional, IfTrue, IfFalse, Back }

pub struct Edge { pub from: BlockId, pub to: BlockId, pub kind: EdgeKind }

pub struct CFG {
    pub entry:  BlockId,
    pub blocks: HashMap<BlockId, BasicBlock>,
    pub edges:  Vec<Edge>,
}
```

Ajouter sur `CFG` :
- `fn successors(&self, id: BlockId) -> Vec<BlockId>`
- `fn predecessors(&self, id: BlockId) -> Vec<BlockId>`

### `src/ir/stmt.rs`
```rust
pub enum Stmt {
    Let    { var: Var, rhs: Expr },
    Assign { var: Var, rhs: Expr },
    ExprStmt(Expr),
}
```

### `src/ir/hooks.rs`
```rust
pub enum HookEntry {
    State    { label: HookLabel, init: Expr },
    Effect   { label: HookLabel, body_cfg: CFG, deps: Option<Vec<Expr>> },
    Memo     { label: HookLabel, body_cfg: CFG, deps: Vec<Expr> },
    Callback { label: HookLabel, body_cfg: CFG, deps: Vec<Expr> },
    Ref      { label: HookLabel, init: Expr },
    Custom   { label: HookLabel, name: Symbol, args: Vec<Expr>, deps: Option<Vec<Expr>> },
}
```

### `src/ir/component.rs`
```rust
pub struct ComponentIR {
    pub name:       Symbol,
    pub param:      Var,
    pub render_cfg: CFG,
    pub hooks:      Vec<HookEntry>,
}
```

### Tests étape 2
Construire un `ComponentIR` à la main pour un composant minimal :
```rust
// fn Counter() { const [n, setN] = useState(0); return <div>{n}</div>; }
```
Vérifier que ça compile, que `#[derive(Debug)]` affiche quelque chose de lisible.

---

## Étape 3 — Lowering (AST Oxc → IR)

Créer `src/lowering/`. C'est l'étape la plus longue.  
Entrée : `&oxc_ast::ast::Program`, sortie : `Vec<ComponentIR>`.

### `src/lowering/mod.rs`
Point d'entrée :
```rust
pub fn lower_program(ast: &Program) -> Vec<ComponentIR>
```

### `src/lowering/component_detector.rs`
Identifier les composants React parmi toutes les fonctions du fichier.

Règles d'identification (par priorité) :
1. Nom commence par `use` → custom hook, **jamais** un composant
2. Au moins un chemin de retour produit un `JSXElement` dans l'AST → composant
3. Annoté `React.FC` / `React.ReactElement` / `JSX.Element` → composant

Retourner `Vec<&FunctionDeclaration>` (ou équivalent Oxc).

### `src/lowering/cfg_builder.rs`
Construire un `CFG` depuis le body d'une fonction Oxc.

Traiter dans l'ordre de complexité croissante :
1. Séquence de statements → blocs linéaires
2. `if`/ternaire/`&&`/`||` → `Branch` terminator + blocs then/else + bloc de join
3. `return` → `Return` terminator (les statements suivants vont dans un nouveau bloc mort)
4. `while`/`for` → back-edge explicite (`EdgeKind::Back`) vers le loop header

Utiliser un `BlockBuilder` avec état interne :
```rust
struct BlockBuilder {
    blocks:    HashMap<BlockId, BasicBlock>,
    edges:     Vec<Edge>,
    current:   BlockId,
    counter:   usize,
}
impl BlockBuilder {
    fn new_block(&mut self) -> BlockId { ... }
    fn push_stmt(&mut self, stmt: Stmt) { ... }
    fn seal_with(&mut self, term: Terminator) { ... }
}
```

### `src/lowering/expr_lower.rs`
Traduire les expressions Oxc → `Expr` IR.

Cas importants :
- `JSXElement` → `CompApp` ou `NativeElem`
- `a && b` → `If(a, b, Lit(false))`
- `a || b` → `If(a, Lit(true), b)`
- `a ? b : c` → `If(a, b, c)` — mais en CFG : Branch terminator + blocs
- Appels de hooks → déléguer à `hook_extractor`
- Objets `{a: v}` → `ObjectLit`
- Arrow functions → `FnLit { params, body_cfg: build_cfg(body) }`

### `src/lowering/hook_extractor.rs`
Extraire les `HookEntry` dans l'ordre d'appel (= ordre textuel pour commencer).

Pour chaque appel reconnu comme hook dans le CFG :
- Incrémenter le label counter global du composant
- Créer le `HookEntry` correspondant
- Remplacer l'appel dans le CFG par `StateVal(ℓ)` / `MemoVal(ℓ)` / etc.

Hooks natifs à reconnaître : `useState`, `useEffect`, `useMemo`, `useCallback`, `useRef`, `useContext`, `useReducer`.  
Tout autre `use*` → `HookEntry::Custom`.

### Désucrage destructuring

```
const [x, setX] = useState(0)
→ HookEntry::State { label: 1, init: Lit(0) }
→ Let { var: "x",    rhs: StateVal(1) }
→ Let { var: "setX", rhs: StateSetter(1) }

const { a, b } = obj
→ Let { var: "a", rhs: FieldAccess(Var("obj"), "a") }
→ Let { var: "b", rhs: FieldAccess(Var("obj"), "b") }

const memoVal = useMemo(() => compute(x), [x])
→ HookEntry::Memo { label: 2, body_cfg: CFG(compute(x)), deps: [Var("x")] }
→ Let { var: "memoVal", rhs: MemoVal(2) }
```

### Gotchas du lowering

- **Lifetimes Oxc** : les nœuds Oxc sont `'a` lifetime sur l'allocateur. Ne pas stocker de références Oxc dans les types IR — copier les données nécessaires (spans, noms) pendant le lowering.
- **Early returns imbriqués** : `if (cond) { if (other) return null; ... }` — le bloc après le return interne doit être créé dans le scope du if externe. Le `BlockBuilder` doit gérer correctement le `current_block` après chaque `seal_with(Return(...))`.
- **Hooks dans les branches** : extraire quand même le hook même s'il est dans un `if` — c'est un bug, mais on l'extrait pour l'analyser. La règle `ConditionalHook` le signalera plus tard.
- **Fonctions internes** : un composant peut définir des helpers internes (`const format = (x) => ...`). Ce ne sont pas des composants — les exclure si pas de JSX return et pas de hook calls.

### Tests étape 3
- Prendre chaque fichier de `examples/` — vérifier que le lowering ne panique pas.
- Pour `examples/bugs.tsx` : afficher le `ComponentIR` de chaque composant et vérifier à la main que les hooks sont bien extraits avec les bons labels.
- Vérifier les désucrage : `a && b` → `If`, destructuring useState → `StateVal`/`StateSetter`.

---

## Étape 4 — Domaines abstraits

Créer `src/domains/`.

### `src/domains/mod.rs`
Trait central :
```rust
pub trait AbstractDomain: Clone + PartialOrd {
    fn bottom() -> Self;
    fn top() -> Self;
    fn is_bottom(&self) -> bool;
    fn join(&self, other: &Self) -> Self;    // least upper bound
    fn meet(&self, other: &Self) -> Self;    // greatest lower bound
    fn widen(&self, other: &Self) -> Self;   // pour forcer la convergence
}
```

### `src/domains/stability.rs`
Treillis `{Bottom, Stable, Unstable, Unknown}` :
```
       Unknown (⊤)
      /    \
 Stable   Unstable
      \    /
       Bottom (⊥)
```

- `join(Stable, Unstable) = Unknown`
- `join(x, Unknown) = Unknown`
- `join(x, Bottom) = x`
- `widen` = `join` (treillis de hauteur finie 2, pas besoin d'autre chose)
- `meet` = inverse du join

Méthode utilitaire :
```rust
pub fn from_expr_static(expr: &Expr) -> Stability {
    match expr {
        Expr::Lit(_)         => Stability::Stable,
        Expr::ObjectLit(_)   => Stability::Unstable,
        Expr::ArrayLit(_)    => Stability::Unstable,
        Expr::FnLit(_)       => Stability::Unstable,
        Expr::StateSetter(_) => Stability::Stable,
        _                    => Stability::Unknown,
    }
}
```

### `src/domains/abstract_env.rs`
`HashMap<Var, Stability>` avec opérations pointwise :
- `join` = join pointwise sur les clés communes, Unknown pour les clés dans un seul des deux
- `lookup(var) -> Stability` — retourne `Unknown` si absent (conservatif)
- `extend(var, stab)` = met à jour ou insère

### `src/domains/state_store.rs`
`HashMap<HookLabel, Stability>` :
- Représente la stabilité de la valeur de chaque `useState` au fixpoint
- `update(label, stab)` = `self[label] = join(self[label], stab)`
- Sujet au fixpoint — évolue à chaque itération selon les `setState` détectés

### `src/domains/memo_store.rs`
`HashMap<HookLabel, Stability>` :
- Recalculé depuis les deps après chaque update de `abstract_env`
- `recompute(label, deps: &[Expr], env: &AbstractEnv) -> Stability`
  = `join` de `env.lookup(v)` pour chaque `Var(v)` dans `deps`
- Pas de fixpoint propre

### `src/domains/transfer.rs`
Fonctions de transfert — cœur de l'analyse :

```rust
// Évalue une expression → retourne sa stabilité
pub fn eval_expr(
    expr: &Expr,
    env: &AbstractEnv,
    state: &StateStore,
    memo: &MemoStore,
) -> Stability

// Exécute un statement → met à jour env et state
pub fn exec_stmt(
    stmt: &Stmt,
    env: &mut AbstractEnv,
    state: &mut StateStore,
    memo: &mut MemoStore,
)
```

Cas clés dans `eval_expr` :
- `StateVal(ℓ)` → `state.get(ℓ)`
- `StateSetter(ℓ)` → `Stable`
- `MemoVal(ℓ)` → `memo.get(ℓ)`
- `Var(x)` → `env.lookup(x)`
- `TsAnnotated { expr, ty }` → utiliser le hint TypeScript si pertinent, sinon `eval_expr(expr, ...)`
- `BinOp` → `Stable` si les deux opérandes sont `Stable` et le résultat est primitif

Cas clé dans `exec_stmt` :
- `ExprStmt(Call(Var(setter), [arg]))` où `setter` est un `StateSetter(ℓ)` → `state.update(ℓ, eval_expr(arg, ...))`

### Tests étape 4
Tests unitaires sur IR construit à la main :
- `ObjectLit` → `Unstable`
- `useMemo(f, [Var("x")])` avec `x = Stable` dans env → `MemoVal` → `Stable`
- `useMemo(f, [Var("x")])` avec `x = Unstable` dans env → `MemoVal` → `Unstable`
- `StateSetter(ℓ)` → `Stable`
- Après `exec_stmt(setState({}) ...)` → `state_store[ℓ] = Unstable`
- `join(Stable, Unstable)` → `Unknown`

---

## Étape 5 — Engine (fixpoint)

Créer `src/engine/`.

### `src/engine/analysis_result.rs`
```rust
pub struct AnalysisResult {
    pub state_store:    StateStore,
    pub memo_store:     MemoStore,
    pub block_states:   HashMap<BlockId, AbstractEnv>,
    pub hook_calls:     Vec<HookCallInfo>,
    pub effect_info:    HashMap<HookLabel, EffectInfo>,
    pub widened_labels: HashSet<HookLabel>,
    pub render_cfg:     CFG,    // référence ou clone selon besoins
}

pub struct HookCallInfo {
    pub label:    HookLabel,
    pub kind:     HookKind,
    pub block_id: BlockId,
    pub span:     Span,
}

pub struct EffectInfo {
    pub label:         HookLabel,
    pub free_vars:     HashSet<Var>,
    pub declared_deps: Vec<Expr>,
}
```

### `src/engine/dominance.rs`
Calcul de la dominance sur un CFG.

Algorithme de Cooper, Harvey, Kennedy (2001) — itératif, simple :
```
pour chaque bloc b sauf entry :
    dom[b] = tous les blocs

dom[entry] = {entry}

changed = true
tant que changed :
    changed = false
    pour chaque b en ordre RPO (sauf entry) :
        new_dom = intersection(dom[p] pour p prédécesseur de b) ∪ {b}
        si new_dom ≠ dom[b] :
            dom[b] = new_dom
            changed = true
```

Exposer :
```rust
pub fn dominates(cfg: &CFG, a: BlockId, b: BlockId) -> bool
pub fn compute_dominators(cfg: &CFG) -> HashMap<BlockId, HashSet<BlockId>>
```

### `src/engine/cfg_analyzer.rs`
Parcourir un CFG avec worklist et calculer l'`AbstractEnv` à chaque bloc.

```
fn analyze_cfg(
    cfg: &CFG,
    entry_env: AbstractEnv,
    state: &StateStore,
    memo: &MemoStore,
) -> (HashMap<BlockId, AbstractEnv>, StateStore)

Algorithme worklist :
  block_envs[entry] = entry_env
  worklist = {entry}
  
  tant que worklist non vide :
    b = worklist.pop()
    env = block_envs[b]
    
    // exécuter les stmts du bloc
    (env_out, state_updates) = exec_block(cfg.blocks[b], env, state, memo)
    
    // propager aux successeurs
    pour succ dans cfg.successors(b) :
        new_env = join(block_envs.get(succ), env_out)
        si new_env ≠ block_envs[succ] :
            block_envs[succ] = new_env
            worklist.push(succ)
  
  return (block_envs, accumulated_state_updates)
```

Attention aux back-edges : appliquer le widening sur l'env du loop header si seuil atteint.

### `src/engine/fixpoint.rs`
Boucle principale pour un composant :

```
fn analyze_component(comp: &ComponentIR, config: &Config) -> AnalysisResult:
  
  state_store = StateStore::bottom()
  widened_labels = HashSet::new()
  iteration = 0
  
  boucle :
    // Passe render
    (block_states, state_from_render) = analyze_cfg(
        &comp.render_cfg, AbstractEnv::bottom(), &state_store, &memo_store
    )
    
    // Recalculer memo store depuis env final du render
    env_exit_render = block_states[exit_block_of(&comp.render_cfg)]
    memo_store = recompute_memo(&comp.hooks, &env_exit_render)
    
    // Passe effects
    state_from_effects = state_store.clone()
    for hook in &comp.hooks :
        if Effect { label, body_cfg, deps } = hook :
            if should_run(deps, &env_exit_render) :  // deps absentes = toujours run
                (_, state_delta) = analyze_cfg(body_cfg, env_exit_render, &state_store, &memo_store)
                state_from_effects.merge(state_delta)
    
    new_state = state_from_render.join(&state_from_effects)
    
    // Convergence
    if new_state ⊑ state_store :
        break
    
    iteration += 1
    if iteration >= config.widen_threshold :
        for label in new_state.changed_labels(&state_store) :
            widened_labels.insert(label)
        state_store = state_store.widen(&new_state)
    else :
        state_store = new_state
  
  // Collecter hook_calls et effect_info depuis block_states
  ...
  
  AnalysisResult { state_store, block_states, hook_calls, effect_info, widened_labels, ... }
```

**Garde de sécurité pendant le dev :** `assert!(iteration < 100, "fixpoint non convergent")`.

### Tests étape 5
- Composant simple (un `useState`, pas d'effet) → fixpoint en 1 itération
- Composant avec `useEffect` qui `setState` avec valeur constante → fixpoint en 2 itérations, `widened_labels` vide
- Composant avec boucle infinie (`useEffect` qui setState toujours différent) → `widened_labels` non vide

---

## Étape 6 — Règles (post-pass)

Créer `src/rules/` (remplacer les règles actuelles).

### `src/rules/mod.rs`
```rust
pub trait Rule: Send + Sync {
    fn name(&self) -> &str;
    fn check(&self, result: &AnalysisResult) -> Vec<Warning>;
}
```

### `src/rules/conditional_hook.rs`
```
pour chaque hook_call dans result.hook_calls :
    si NON dominates(result.render_cfg, entry, hook_call.block_id) :
        émettre Warning::ConditionalHook { span: hook_call.span, hook: hook_call.label }
```

### `src/rules/missing_deps.rs`
```
pour chaque (label, effect_info) dans result.effect_info :
    env = result.block_states[exit_de_render]
    pour chaque var dans effect_info.free_vars :
        si var n'est pas dans effect_info.declared_deps :
            stab = env.lookup(var)
            si stab != Stable :
                émettre Warning::MissingDep { effect: label, var, stability: stab }
```

Calculer `free_vars` d'un `body_cfg` : variables lues mais non définies localement dans le CFG.  
C'est un calcul de liveness standard : `free_vars = used_vars - defined_vars`.

### `src/rules/redundant_set_state.rs`
```
pour chaque bloc dans result.block_states :
    pour chaque stmt dans bloc :
        si stmt = ExprStmt(Call(StateSetter(ℓ), [arg])) :
            arg_stab = eval_expr(arg, result.block_states[bloc], result.state_store, ...)
            current_state_stab = result.state_store[ℓ]
            si arg_stab == Stable ET current_state_stab == Stable :
                émettre Warning::RedundantSetState { span, label: ℓ }
```

Note : rule conservatrice avec stability seul. Une valeur stable pourrait quand même changer (ex: useState(ref) puis setState(ref) — même référence). Pour éliminer les faux positifs, le constant domain (étape future) aide.

### `src/rules/infinite_loop.rs`
```
pour chaque label dans result.widened_labels :
    // Vérifier que l'effet correspondant appelle setState de ℓ inconditionnellement
    effect = result.effect_info[label]
    si effect.unconditionally_calls_setter(label) :
        émettre Warning::InfiniteLoop { label, span }
```

"Appelle inconditionnellement" = le bloc entry de `body_cfg` contient un `ExprStmt(Call(StateSetter(ℓ), ...))`, ou tous les chemins depuis l'entry passent par un tel appel.

### Tests étape 6
Tester chaque règle avec un `AnalysisResult` fabriqué à la main — pas besoin du lowering ou de l'engine. Cas à couvrir :
- Hook dans branche → `ConditionalHook`
- Hook dans tous les chemins → pas de warning
- Variable libre non stable absente des deps → `MissingDep`
- Variable libre stable absente des deps → pas de warning
- `setState` avec valeur stable → `RedundantSetState`
- Widening déclenché + setState inconditionnel → `InfiniteLoop`

---

## Étape 7 — Hook Registry

### `src/registry/mod.rs`
```rust
pub trait HookModel: Send + Sync {
    fn name(&self) -> &str;
    fn analyze(&self, args: &[Stability], deps: Option<&[Stability]>) -> HookResult;
}

pub struct HookResult {
    pub return_stability: Stability,
    pub creates_state:    Option<HookLabel>,   // None si pas de useState implicite
    pub effect_semantics: Option<EffectSemantics>,
}

pub struct Registry {
    models: HashMap<String, Box<dyn HookModel>>,
}
impl Registry {
    pub fn new_with_builtins() -> Self { ... }
    pub fn register(&mut self, model: Box<dyn HookModel>) { ... }
    pub fn lookup(&self, name: &str) -> Option<&dyn HookModel> { ... }
}
```

### `src/registry/builtins.rs`
Implémenter `HookModel` pour chaque hook natif :

| Hook | return_stability | creates_state | effect_semantics |
|---|---|---|---|
| `useState` | `(Unknown, Stable)` | `Some(label)` | None |
| `useEffect` | `Stable` (unit) | None | `Some(...)` |
| `useMemo` | `join(deps)` | None | None |
| `useCallback` | `join(deps)` | None | None |
| `useRef` | `Stable` | None | None |
| `useContext` | `Unknown` | None | None |
| `useReducer` | `(Unknown, Stable)` | `Some(label)` | None |

### Config utilisateur (optionnel)
`reactant.toml` à la racine du projet analysé :
```toml
[[hooks]]
name = "useMyCustomHook"
return = "stable"  # ou "unstable" ou "unknown"
```

---

## Ordre de priorité global

```
1 → 2 → 3 → 4 → 5 → 6 → 7
    IR   lowering  domaines  engine  règles  registry
```

**Stratégie de dev :** après chaque étape, brancher un `main` debug minimal qui affiche les résultats intermédiaires. Ne pas attendre l'étape 6 pour voir si quelque chose sort de l'analyse.

```
Après étape 3 : afficher le ComponentIR de chaque composant dans examples/
Après étape 4 : afficher la stabilité de chaque variable à la sortie du render_cfg
Après étape 5 : afficher le StateStore au fixpoint
Après étape 6 : afficher tous les warnings sur examples/bugs.tsx
```

---

## Pitfalls à anticiper

**Lifetimes Oxc** : les nœuds Oxc sont `'a` lifetime sur l'allocateur. Ne pas stocker de références Oxc dans les types IR — copier les données (spans, noms) pendant le lowering.

**CFG building et early returns imbriqués** : `if (a) { if (b) return null; ... }` — le bloc après le return interne doit être dans le scope du if externe. Le `BlockBuilder` doit réinitialiser `current_block` correctement après chaque `Return` terminator.

**Hooks dans les branches** : extraire quand même le hook même s'il est dans un `if`. La règle `ConditionalHook` le signalera — le lowering ne doit pas filtrer.

**Labels par position textuelle** : assigner les labels en ordre textuel (position dans le source) pour commencer — correct pour ~95% du code React. L'ordre de dominance est plus correct mais plus complexe à implémenter initialement.

**Fixpoint sans widening** : pendant le dev, mettre `assert!(iteration < 100)` pour détecter les cas non convergents avant d'avoir implémenté le widening correctement.

**Destructuring imbriqué** : `const { a: { b } } = obj` — commencer par le destructuring plat uniquement, rejeter le destructuring imbriqué avec un `todo!()` ou le convertir en `Unknown`.

**Fonctions dans les effets** : `useEffect(() => { const x = ...; setN(x); }, [])` — le `body_cfg` de l'effet a ses propres variables locales. L'analyse de l'effet commence avec l'env du render + les variables locales à `Bottom`.
