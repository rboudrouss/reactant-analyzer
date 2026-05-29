# IR — reactant-analyzer

IR dédié basé sur CFG, inspiré de React-tRace (Lee, Ahn, Yi — OOPSLA 2025), étendu pour couvrir les hooks supplémentaires, les objets/arrays et le sous-ensemble JS nécessaire.

## Grammaire

```
Programme
  Prog ::= ComponentDef* MainExpr

Définition de composant
  CompDef ::= Component { name: Symbol, param: Var, body: ComponentIR }

Structure d'un composant
  ComponentIR ::= {
    render_cfg: CFG,
    hooks:      Vec<HookEntry>,
  }

Entrées de hooks (ordre = ordre d'appel dans le render, détermine les labels React)
  HookEntry ::=
    | State    { label: ℓ, init: Expr }
    | Effect   { label: ℓ, body_cfg: CFG, deps: Option<Vec<Expr>> }
    | Memo     { label: ℓ, body_cfg: CFG, deps: Vec<Expr> }
    | Callback { label: ℓ, body_cfg: CFG, deps: Vec<Expr> }
    | Ref      { label: ℓ, init: Expr }
    | Custom   { label: ℓ, name: Symbol, args: Vec<Expr>, deps: Option<Vec<Expr>> }

CFG
  CFG ::= {
    entry:  BlockId,
    blocks: Map<BlockId, BasicBlock>,
    edges:  Vec<Edge>,
  }

  Edge ::= {
    from: BlockId,
    to:   BlockId,
    kind: EdgeKind,
  }

  EdgeKind ::= Unconditional | IfTrue | IfFalse | Back

Bloc de base
  BasicBlock ::= {
    id:    BlockId,
    stmts: Vec<Stmt>,
    term:  Terminator,
  }

Terminateurs (pas de branchement dans les Stmt, uniquement en Terminator)
  Terminator ::=
    | Jump(BlockId)
    | Branch { cond: Expr, then_: BlockId, else_: BlockId }
    | Return(Expr)
    | Unreachable

Instructions
  Stmt ::=
    | Let { var: Var, rhs: Expr }
    | Assign { var: Var, rhs: Expr }       -- réassignation (rare en React fonctionnel)
    | ExprStmt(Expr)                        -- appel avec effets de bord

Expressions
  Expr ::=
    -- Valeurs primitives → Stable
    | Lit(Prim)                             -- number | string | bool | null | unit

    -- Valeurs composites → Unstable
    | ObjectLit { fields: Vec<(Symbol, Expr)> }
    | ArrayLit  { elems: Vec<Expr> }
    | FnLit     { params: Vec<Var>, body_cfg: CFG }

    -- Variables
    | Var(Symbol)

    -- Accès
    | FieldAccess { obj: Expr, field: Symbol }   -- Unknown (conservatif)
    | IndexAccess { arr: Expr, idx: Expr }        -- Unknown

    -- Opérations
    | BinOp  { op: BinOp,   lhs: Expr, rhs: Expr }
    | UnaryOp { op: UnaryOp, arg: Expr }

    -- Appels
    | Call    { fn_: Expr, args: Vec<Expr> }     -- Unstable par défaut
    | CompApp { name: Symbol, props: Expr }       -- JSX composant désucré
    | NativeElem { tag: Symbol, props: Expr, children: Vec<Expr> }

    -- Annotation TypeScript optionnelle (hint de stabilité, non obligatoire)
    | TsAnnotated { expr: Box<Expr>, ty: TsType }
```

## Désucrage (AST Oxc → IR)

### JSX

```
<Foo bar={v} baz={w} />
→ CompApp { name: "Foo", props: ObjectLit { bar: v, baz: w } }

<div class="x">{child}</div>
→ NativeElem { tag: "div", props: ObjectLit { class: "x" }, children: [child] }

<>{a}{b}</>
→ NativeElem { tag: "Fragment", props: ObjectLit {}, children: [a, b] }
```

### Hooks — destructuring

```js
const [count, setCount] = useState(0)
```
Le lowering crée une `HookEntry::State { label: ℓ, init: Lit(0) }` dans `hooks`.  
Dans le `render_cfg`, deux bindings sont insérés :
```
Let { var: "count",    rhs: StateVal(ℓ) }    -- référence au StateStore[ℓ].val
Let { var: "setCount", rhs: StateSetter(ℓ) } -- référence stable au setter
```

`StateVal(ℓ)` et `StateSetter(ℓ)` sont des expressions IR spéciales résolubles par les domaines.

```js
const memoVal = useMemo(() => expensive(x), [x])
```
→ `HookEntry::Memo { label: ℓ, body_cfg: CFG(expensive(x)), deps: [Var("x")] }`  
→ `Let { var: "memoVal", rhs: MemoVal(ℓ) }` dans `render_cfg`

### Court-circuits et ternaires

```js
a && b    →  If(a, b, Lit(false))
a || b    →  If(a, Lit(true), b)
a ? b : c →  If(a, b, c)         -- Branch terminator + blocs de join
```

### Destructuring objet/array

```js
const { x, y } = obj
→ Let { var: "x", rhs: FieldAccess(obj, "x") }
   Let { var: "y", rhs: FieldAccess(obj, "y") }

const [head, ...tail] = arr
→ Let { var: "head", rhs: IndexAccess(arr, Lit(0)) }
   Let { var: "tail", rhs: Call(Array.prototype.slice, [arr, Lit(1)]) }
```

### Spread

```js
{ ...obj, extra: val }  →  ObjectLit { __spread: obj, extra: val }
```
Toujours `Unstable` (nouvel objet).

### Early returns

```js
function Comp({ user }) {
  if (!user) return null;
  const [n, setN] = useState(0);
  return <div>{n}</div>;
}
```
→ CFG avec deux blocs :
```
BB0: Branch(cond: !user, then_: BB1, else_: BB2)
BB1: Return(Lit(null))
BB2: [hooks...] Return(CompApp(...))
```
Pas de CPS-transform. `Return` = terminator normal.  
Le hook dans BB2 n'est pas dominé par l'entrée sur tous les chemins → détecté par la règle `ConditionalHook`.

## Stabilité statique des expressions

| Expression IR | Stabilité |
|---|---|
| `Lit(Prim)` | Stable |
| `ObjectLit` | Unstable |
| `ArrayLit` | Unstable |
| `FnLit` | Unstable |
| `StateVal(ℓ)` | join de tous les args passés à `StateSetter(ℓ)` |
| `StateSetter(ℓ)` | Stable |
| `MemoVal(ℓ)` | join(stability(deps de ℓ)) |
| `Var(x)` | stability(x) depuis AbstractEnv courant |
| `FieldAccess` | Unknown |
| `IndexAccess` | Unknown |
| `Call(f, args)` | Unstable (sauf si f dans HookRegistry et spec dit autre chose) |
| `BinOp` sur primitives | Stable |
| `BinOp` avec composites | Unknown |
| `TsAnnotated` avec type primitif | hint Stable |
| `CompApp` | Unstable (nouvel arbre JSX) |

## Labels de hooks

Chaque hook call reçoit un label `ℓ ∈ ℕ` assigné par **position dans l'ordre d'appel** dans le `render_cfg`. Ce label correspond à l'identifiant interne React (linked list position). Le lowering assigne les labels en parcourant le `render_cfg` en ordre de dominance.

Invariant : deux hooks distincts dans un composant ont des labels distincts.

## TypeScript hints

`TsAnnotated { expr, ty }` transporte le type TypeScript optionnel. Les domaines peuvent l'ignorer ou l'utiliser comme hint :

| Type TypeScript | Hint de stabilité |
|---|---|
| `number \| string \| boolean \| null \| undefined` | Stable |
| `readonly T[]` | Stable (référence du tableau) |
| `React.FC` | marque une fonction comme composant |
| `React.MemoExoticComponent<T>` | Stable |
| `React.RefObject<T>` | Stable (objet ref) |
| Générique / inconnu | ignoré |
