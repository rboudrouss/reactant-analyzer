# ADR-008 : Domaine de valeurs pour le fixpoint SCC — StateValue enum + TypedStateStore

- **Statut** : Implémenté (Option A + Option B actifs)
- **Date** : 2026-06-02
- **Contexte** : [ADR-007](ADR-007-cross-domain-queries.md) (cross-domain), [ADR-002](ADR-002-abstract-domains.md) (Stability)

## Contexte

La détection de boucles infinies repose sur un fixpoint sur le SCC du graphe
`Effect → State → Effect`. Le domaine `Stability` ne suffit pas : il converge
toujours en ≤2 itérations et ne peut pas distinguer `setState(count + 1)`
(boucle infinie) de `setState(42)` (convergent). Il faut un domaine qui track
la valeur concrète du state afin de détecter si le fixpoint **widen ou
converge**.

L'init du useState est déjà présente dans l'IR (`HookEntry::State { init: Expr, .. }`),
ce qui permet d'inférer le type du state sans annotation supplémentaire.

### Le problème des nullable states en JS

Un state JS est souvent `T | null | undefined` :

```js
const [value, setValue] = useState(null);      // int | null
const [open, setOpen] = useState(undefined);   // boolean | undefined
```

React utilise `Object.is` pour comparer. `Object.is(null, null) === true` donc
`setState(null)` quand le state est déjà null → **pas de re-render**. Mais
`setState(42)` quand le state est null → re-render.

---

## Option A (implémentée) — Enum `StateValue` unifiée

`StateValue` est une enum plate représentant toutes les valeurs JS abstraites.
`Copy` retiré (trop large) — Clone seulement.

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum StateValue {
    /// ⊥ — chemin non atteignable (distinct de Null).
    Bottom,
    /// Valeur JS `null` explicite.
    Null,
    /// Valeur JS `undefined` explicite.
    Undefined,
    /// Nombre entier ou flottant dans l'intervalle [lo, hi].
    Number(Interval),
    /// Valeur booléenne.
    Boolean(BoolVal),
    /// Chaîne de caractères — ensemble exact de valeurs possibles.
    StrConst(Arc<BTreeSet<String>>),
    /// Chaîne de caractères — précision perdue (⊤ pour les strings).
    Str,
    /// Référence objet/tableau/fonction — stability de référence.
    Reference(Stability),
    /// ⊤ — any JS value, précision perdue.
    Top,
}
```

Note : `StateValue::StrConst` contient directement un `Arc<BTreeSet<String>>`,
pas le type wrapper `StrConst`. Le widening string est géré par `StrConst`
(voir Option B / `str_const.rs`), mais le résultat final stocké dans
`StateValue` est l'arc brut ou `Str` une fois la précision perdue.

Fichiers extraits pour lisibilité :
- `src/domains/impls/interval.rs` — type `Interval`
- `src/domains/impls/bool_val.rs` — type `BoolVal`

### Lattice et `join`

```
                    Top  (⊤)
                 /   |   \   \
         Number  Boolean  Str  Reference
           |       |      |        |
       Interval  BoolVal StrConst Stability
           \       \          /
            Null  Undefined  Bottom (⊥)
```

Règles de `join` :

| a | b | join(a, b) |
|---|---|---|
| Bottom | x | x |
| Null | Null | Null |
| Undefined | Undefined | Undefined |
| Null | Undefined | Top |
| Number(i) | Number(j) | Number(i.join(j)) |
| Boolean(x) | Boolean(y) | Boolean(x.join(y)) |
| StrConst(a) | StrConst(b) | StrConst(a ∪ b) ou Str si seuil dépassé |
| StrConst(_) | Str | Str |
| Str | Str | Str |
| Reference(s) | Reference(t) | Reference(s.join(t)) |
| **Null \| Undefined** | **Number(i)** | **Top** (¹) |
| **Number(i)** | **Boolean(x)** | **Top** |
| tout autre mélange | | Top |

**(¹) Précision perdue pour `int \| null`.** Voir section "Nullable" ci-dessous.

### `widen`

- `Number` → interval widening standard : si `lo` décroît → `lo = -∞`, si `hi` croît → `hi = +∞`
- `Boolean` → `join` (treillis fini, height 2)
- `StrConst` → `join` puis widen à `Str` si `|set| > 4` (seuil = 4, voir `str_const.rs`)
- `Str`, `Null`, `Undefined`, `Reference` → `join` (finis)
- `Top` → stable

### Narrowing sur les branches

Après widening, les conditions de branche raffinent l'état :

```
// useEffect(() => { if (count < 10) setCount(count + 1) })
// Après widening : count ∈ [0, +∞)
// Branche taken : count < 10 → narrow → count ∈ [0, 9]
// Convergence à [0, 9] → pas de boucle infinie
```

Le narrowing est appliqué dans le `exec_stmt` du CFG analyzer quand le
terminator est un `Branch { cond }`. Le domaine `StateValue::Number(i)` doit
implémenter `narrow_lt`, `narrow_leq`, `narrow_eq`, etc. sur son `Interval`.

### Inférence de type depuis `init`

```rust
impl StateValue {
    pub fn type_from_init(init: &Expr) -> StateType {
        match init {
            Expr::Lit(Prim::Int(_) | Prim::Float(_)) => StateType::Number,
            Expr::Lit(Prim::Bool(_))                 => StateType::Boolean,
            Expr::Lit(Prim::String(_))               => StateType::Str,
            Expr::Lit(Prim::Null)                    => StateType::Nullable(None),
            Expr::Lit(Prim::Unit)                    => StateType::Nullable(None),
            Expr::ObjectLit(_) | Expr::ArrayLit(_)
            | Expr::FnLit { .. }                     => StateType::Reference,
            _                                        => StateType::Unknown,
        }
    }

    /// Valeur initiale abstraite du state.
    pub fn init_value(init: &Expr) -> Self {
        match init {
            Expr::Lit(Prim::Int(n))    => StateValue::Number(Interval::point(*n as f64)),
            Expr::Lit(Prim::Float(f))  => StateValue::Number(Interval::point(*f)),
            Expr::Lit(Prim::Bool(b))   => StateValue::Boolean(BoolVal::from(*b)),
            Expr::Lit(Prim::String(s)) => StateValue::StrConst(Arc::new(BTreeSet::from([s.clone()]))),
            Expr::Lit(Prim::Null)      => StateValue::Null,
            Expr::Lit(Prim::Unit)      => StateValue::Undefined,
            Expr::ObjectLit(_)
            | Expr::ArrayLit(_)
            | Expr::FnLit { .. }       => StateValue::Reference(Stability::Unstable),
            _                          => StateValue::Top,
        }
    }
}
```

### Handling `int | null` et `bool | null`

**Problème** : `join(Null, Number([5,5])) = Top` → précision perdue immédiatement.

Dans un SCC comme :
```js
const [n, setN] = useState(null);
useEffect(() => { if (n === null) setN(0) }, [n]);
```
- Iter 1 : n = Null → condition true → setN(0) → join(Null, Number([0,0])) = **Top**
- Iter 2 : n = Top → condition ? → conservative → **Top** (converge)
- Pas de widening → pas de signal boucle infinie ✓

→ Pattern `null → value` non-cyclique : converge à Top, pas de faux positif.

**Pattern problématique :**
```js
useEffect(() => { if (n !== null) setN(n + 1) }, [n]);
```
- n démarre Null, après premier setter devient Number
- join(Null, Number([1,1])) = Top → on perd la progression [1,2,3,...]
- Pas de signal widening → **faux négatif possible** (boucle non détectée)

**Mitigations** :
1. Si init = Null mais premier setter est `Number` → promouvoir le type à `Number` (flow-sensitive type refinement, complexe)
2. Ou documenter comme limite connue et recommander annotation TypeScript dans un futur

Pour l'instant : **connu et accepté**. L'annotation `useState<number>(null)` en TypeScript donne un hint de type que le lowering pourrait capturer (TODO futur).

---

## Option B (implémentée) — `TypedStateStore` avec sous-stores spécialisés

Chaque `HookLabel` est associé à un `StateType` inféré statiquement.
`TypedStateStore` dispatche vers un sous-store spécialisé selon ce type.

### Structure (`src/domains/stores/typed_state_store.rs`)

```rust
pub struct TypedStateStore {
    type_map:      HashMap<HookLabel, StateType>,
    number_store:  StateStore<Interval>,
    bool_store:    StateStore<BoolVal>,
    str_store:     StateStore<StrConst>,
    ref_store:     StateStore<Stability>,
    unknown_store: StateStore<StateValue>,  // fallback / mélange de types
}
```

### `get()` — join avec `unknown_store`

Pour gérer les labels dont le type change en cours d'itération (type-mismatch),
`get(label)` joint la valeur du sous-store spécialisé avec celle de
`unknown_store` :

```rust
// pseudo-code
fn get(&self, label: &HookLabel) -> StateValue {
    let typed_val = match self.type_map.get(label) {
        Some(StateType::Number)  => self.number_store.get(label).into(),
        Some(StateType::Boolean) => self.bool_store.get(label).into(),
        Some(StateType::Str)     => self.str_store.get(label).into(),
        Some(StateType::Reference) => self.ref_store.get(label).into(),
        _ => StateValue::Bottom,
    };
    typed_val.join(self.unknown_store.get(label))
}
```

Si un setter appelle le label avec un type inattendu, la valeur va dans
`unknown_store` et remonte via le join → pas de perte silencieuse.

### `update()` — dispatch par `(state_type, &val)`

```rust
fn update(&mut self, label: &HookLabel, val: StateValue) {
    match (self.type_map.get(label), &val) {
        (Some(StateType::Number), StateValue::Number(i))    => self.number_store.update(label, *i),
        (Some(StateType::Boolean), StateValue::Boolean(b))  => self.bool_store.update(label, *b),
        (Some(StateType::Str), StateValue::StrConst(_) | StateValue::Str) => self.str_store.update(label, ...),
        (Some(StateType::Reference), StateValue::Reference(s)) => self.ref_store.update(label, *s),
        _ => self.unknown_store.update(label, val),  // fallback
    }
}
```

### Interface Transfer / règles — inchangée

`TypedStateStore` est interne au fixpoint dans `analyze_component`.
Le trait `Transfer` et toutes les règles voient toujours `StateStore<StateValue>`.
Les méthodes `to_untyped()` / `from_untyped()` assurent la conversion :

```rust
impl TypedStateStore {
    pub fn to_untyped(&self) -> StateStore<StateValue> { ... }
    pub fn from_untyped(store: StateStore<StateValue>, type_map: ...) -> Self { ... }
}
```

`AnalysisResult::state_store` retourne toujours `StateStore<StateValue>` — API publique inchangée.

### `StrConst` (`src/domains/impls/str_const.rs`)

```rust
pub enum StrConst {
    Bottom,
    Set(Arc<BTreeSet<String>>),
    Top,
}
```

- Seuil de widening : 4 (`|set| > 4` → widen vers `Top`)
- `str_store` dans `TypedStateStore` utilise `StateStore<StrConst>`
- Lors de `to_untyped()`, `StrConst::Set(s)` → `StateValue::StrConst(s)`, `StrConst::Top` → `StateValue::Str`

---

## Signal boucle infinie depuis ce domaine

Dans le fixpoint SCC :
- Si `StateStore<StateValue>` **widen** sur un label → état de ce label non convergent → **boucle infinie potentielle**
- Si convergence sans widening → **pas de boucle infinie**

Précision par type :

| Type state | Détecte `setState(s + 1)` ? | Détecte `setState({...})` ? | `if (s < 10)` converge ? |
|---|---|---|---|
| Number (Interval) | ✓ widening [0,+∞) | n/a | ✓ avec narrowing |
| Boolean | ✓ oscillation true↔false | n/a | ✓ fini |
| StrConst | ✓ track ensemble exact, widen → Str au seuil=4 | n/a | n/a |
| Reference (Stability) | n/a | ✓ Unstable | n/a |
| Null init → Number | ✗ faux négatif possible | n/a | n/a |
| Top | ✗ converge immédiatement | ✗ | ✗ |

---

## Conséquences

- `src/domains/impls/state_value.rs` — `StateValue` enum (Clone, pas Copy)
- `src/domains/impls/interval.rs` — type `Interval` extrait
- `src/domains/impls/bool_val.rs` — type `BoolVal` extrait
- `src/domains/impls/str_const.rs` — enum `StrConst { Bottom, Set(Arc<BTreeSet<String>>), Top }`, seuil widening = 4
- `src/domains/stores/typed_state_store.rs` — `TypedStateStore`, interne à `analyze_component`
- `HookEntry::State { init }` utilisé pour inférer `StateValue::init_value(init)` au démarrage du fixpoint SCC
- Le fixpoint SCC est **distinct** du fixpoint Stability principal (cf. [ADR-007](ADR-007-cross-domain-queries.md), Option A post-pass)
- `StateStore<StateValue>` utilisé dans l'API publique ; `TypedStateStore` transparent pour Transfer et les règles
- Limite connue : états `T | null` initialisés à `null` peuvent produire des faux négatifs sur certains patterns
