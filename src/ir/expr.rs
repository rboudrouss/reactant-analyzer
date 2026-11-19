use std::sync::Arc;

use crate::ir::{
    cfg::CFG,
    hooks::Arity,
    source_range::SourceRange,
    types::{ExprId, HookLabel, Symbol, Var},
};

/// Where a file's call ids start ([`Expr::Call`]): the upper half of the
/// file's id space, which its literals — numbered up from zero — do not
/// reach, and below a component's hook sites, numbered down from the top of
/// its range ([`crate::ir::ComponentId::hook_site`]).
pub const CALL_SITES: u64 = 1 << 31;

/// A call that is no source call — one the analysis builds to ask a
/// question (a callee applied to a value, a rule's probe): the first call
/// site, which no lowered call of a component's range is.
pub const SYNTHETIC_CALL: ExprId = ExprId(CALL_SITES);

#[derive(Debug, Clone)]
pub enum Prim {
    String(String),
    Int(i32),
    Float(f64),
    Bool(bool),
    Null,
    Unit,
}

#[derive(Debug, Clone)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    And,
    Or,
    /// `===`, and every other strict comparison the lowering emits (a
    /// `switch` case, a default's `=== undefined`).
    Eq,
    /// `!==`.
    Neq,
    /// `==`: `x == null` and `x == undefined` hold for both `null` and
    /// `undefined`, which is the test `??`, `??=` and an optional link make.
    LooseEq,
    /// `!=`.
    LooseNeq,
    Lt,
    Gt,
    Leq,
    Geq,
    /// `%` — the result takes the dividend's sign and is smaller in magnitude
    /// than the divisor; `NaN` when the divisor may be zero.
    Mod,
    /// `**` — exact from the corners of the operand box on the non-negative
    /// quadrant, where the function is monotone in each argument.
    Pow,
    /// `in` — always a boolean.
    In,
    /// `instanceof` — always a boolean.
    InstanceOf,
    /// Bitwise and shift: `&`, `|`, `^`, `<<`, `>>`, `>>>`.
    ///
    /// Real variants rather than one opaque `Unknown`, because they carry
    /// information `Unknown` could not express: JS coerces both operands to
    /// int32 (uint32 for `>>>`), so the result is *always* a number in a known
    /// range. Folding them into `Unknown` erased which operator it was, and
    /// with it every one of those guarantees.
    ///
    /// This is the shared decision for every operator: give it a variant that
    /// says what is known about it. There is no `Unknown` any more — every
    /// binary operator of the language has its own variant, and `lower_binop`
    /// is exhaustive so a new one cannot slip in silently.
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    /// `>>>` — unsigned, so the result is a *uint32*, not an int32.
    UShr,
}

#[derive(Debug, Clone)]
pub enum UnaryOp {
    Neg,
    Not,
    /// `~x` — int32 bitwise NOT, i.e. `-(x + 1)`.
    BitNot,
    /// `typeof x` — always a string, and an exact one when the operand has a
    /// single inhabited kind.
    TypeOf,
    /// Unary `+x` — numeric coercion, not the identity (`+"5"` is the number 5).
    Plus,
    /// An operator whose concrete semantics are not modeled by the abstract
    /// domain. Evaluated as ⊤ — never as the identity, which would falsify the
    /// operand's value.
    Unknown,
}

/// Key prefix lowering gives the synthetic `ObjectLit` member that holds an
/// object spread (`{ ...opts }`). A spread's own members are invisible to the
/// per-member map, and it overwrites every member written before it, so a
/// reader that resolves members by name must stop at one. Source keys can
/// never collide — `...` is not a valid property name.
pub const SPREAD_KEY_PREFIX: &str = "...";

/// Key prefix of a member under a computed key no constant names (`{ [k]: v
/// }`): the value is kept for its reads, the name is unknown.
pub const COMPUTED_KEY_PREFIX: &str = "[computed]";

/// Key prefix of a getter or setter (`{ get x() {…} }`): reading the member
/// runs the body, so the function literal is not the member's value. The key
/// says which, and the member it defines ([`accessor_key`], #321).
pub const ACCESSOR_KEY_PREFIX: &str = "[accessor]";

/// The two halves of an accessor: a getter runs at every read of its member,
/// a setter at every write, with the value written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accessor {
    Get,
    Set,
}

/// The key of an accessor of `member` (`None`: a computed name, which may be
/// any member), unique by `n`: `[accessor]<n>:get:<member>`.
pub fn accessor_key(n: impl std::fmt::Display, kind: Accessor, member: Option<&str>) -> Symbol {
    let kind = match kind {
        Accessor::Get => "get",
        Accessor::Set => "set",
    };
    match member {
        Some(m) => format!("{ACCESSOR_KEY_PREFIX}{n}:{kind}:{m}"),
        None => format!("{ACCESSOR_KEY_PREFIX}{n}:{kind}"),
    }
}

/// What an [`accessor_key`] defines: its half, and the member (`None`: any).
pub fn accessor_of(key: &str) -> Option<(Accessor, Option<&str>)> {
    let mut parts = key.strip_prefix(ACCESSOR_KEY_PREFIX)?.splitn(3, ':');
    parts.next()?;
    let kind = match parts.next()? {
        "get" => Accessor::Get,
        "set" => Accessor::Set,
        _ => return None,
    };
    Some((kind, parts.next()))
}

/// The getter an object literal's `key` is read through, as written: the
/// last entry that defines `key` is a getter of it, with no entry after it
/// that may define any member (a spread, a computed key, an accessor of a
/// computed name). A setter of `key` alone defines no getter, so the search
/// goes on past it; a member written as a value ends it with `None`.
pub fn object_getter<'e>(fields: &'e [(Symbol, Expr)], key: &str) -> Option<&'e Expr> {
    for (k, v) in fields.iter().rev() {
        if k == key {
            return None;
        }
        match accessor_of(k) {
            Some((Accessor::Get, Some(m))) if m == key => return Some(v),
            Some((_, Some(_))) => {}
            _ if is_synthetic_key(k) => return None,
            _ => {}
        }
    }
    None
}

/// Is `key` one lowering made up — a spread, a computed key, an accessor —
/// rather than a member's name? Such an entry may write any member.
pub fn is_synthetic_key(key: &str) -> bool {
    [SPREAD_KEY_PREFIX, COMPUTED_KEY_PREFIX, ACCESSOR_KEY_PREFIX]
        .iter()
        .any(|p| key.starts_with(p))
}

/// The suffix of `fields` a name lookup may read: everything after the last
/// entry that may write any member — a spread, a computed key or an
/// accessor. Members written before one may have been overwritten by
/// something the expression does not spell, so they answer nothing.
///
/// Shared so the syntactic readers that resolve members by name cannot drift
/// apart on the rule — the binding chase in [`crate::engine::seeds`], the
/// splice's returned members, the guard proofs. The abstract interpreter
/// reads the heap's [`Members`](crate::domains::stores::Members) instead,
/// which copies what a spread's source holds.
pub fn members_after_last_spread(fields: &[(Symbol, Expr)]) -> &[(Symbol, Expr)] {
    let start = fields
        .iter()
        .rposition(|(k, _)| is_synthetic_key(k))
        .map_or(0, |i| i + 1);
    &fields[start..]
}

/// The expression bound to `key` in an object literal, exactly — `None` when
/// no readable member has that name, which includes every member a spread may
/// have overwritten.
pub fn object_member<'e>(fields: &'e [(Symbol, Expr)], key: &str) -> Option<&'e Expr> {
    members_after_last_spread(fields)
        .iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
}

/// Methods that mutate their receiver in place (Array, Map/Set, typed arrays).
/// The receiver's reference identity is unchanged — that is the bug when the
/// receiver is state: React compares with `Object.is` and bails out.
///
/// The one list every reader of "is this a mutation site" shares (ADR-028
/// §2): `state-mutation`, the Tier-A purity guard and render dependence.
pub const MUTATING_METHODS: &[&str] = &[
    "push",
    "pop",
    "shift",
    "unshift",
    "splice",
    "sort",
    "reverse",
    "fill",
    "copyWithin",
    "add",
    "delete",
    "clear",
    "set",
];

/// The receiver `expr` mutates in place, when `expr` is a mutation site in
/// call position.
///
/// The statement form is [`crate::ir::stmt::Stmt::MemberWrite`], whose `obj`
/// is its receiver; it has no expression to match, which is why it is not
/// folded in here.
pub fn mutation_receiver(expr: &Expr) -> Option<&Expr> {
    let Expr::Call { fn_, args, .. } = expr else {
        return None;
    };
    match fn_.as_ref() {
        // `items.push(x)` — the receiver is mutated.
        Expr::FieldAccess { obj, field } if MUTATING_METHODS.contains(&field.as_str()) => Some(obj),
        // `Object.assign(target, …)` mutates its first argument, not `Object`.
        Expr::FieldAccess { obj, field }
            if field == "assign" && matches!(obj.as_ref(), Expr::Var(v) if v == "Object") =>
        {
            args.first()
        }
        _ => None,
    }
}

/// The component a JSX callee was proven to name: the file that defines it and
/// the name that file knows it by.
///
/// Both halves are needed, and they are exactly [`crate::engine::ComponentKey`]
/// spelled without the engine dependency. The file settles which of several
/// same-named components is meant; the name settles an alias, since
/// `import { Widget as W }` writes `<W/>` here while the origin file registers
/// `Widget`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CompOrigin {
    pub file: std::path::PathBuf,
    pub name: Symbol,
}

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
    /// A new array of the arguments the running call handed its body, from
    /// position `from` on: the value of a rest parameter (`(a, ...fs)`,
    /// `from` = 1) and of a function's `arguments` (`from` = 0), #324. The
    /// lowering emits it only as the right-hand side of a body's first
    /// statements, where the call's arguments are what the interpreter bound
    /// with the parameters, and an inlined body's are the call's own
    /// (`ir::splice`); it evaluates as the array literal of those arguments,
    /// whose length is unknown when a spread or a caller the analysis does
    /// not see hands them, an entry from a spread on at any position. `id` is
    /// the allocation site, as for `ArrayLit`.
    Arguments {
        id: ExprId,
        from: usize,
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
    /// `fn_(args)`. `id` is the call's site, as `ObjectLit`'s `id` is the
    /// literal's: a call is an allocation site (what it returns lives
    /// there). The lowering mints it from the file's call counter, from
    /// [`CALL_SITES`] up, so a call id never meets a literal's and minting
    /// one moves no other id; placement in a component's range and every
    /// graft shift it by the [`crate::ir::Offsets`] they apply to sites.
    ///
    /// Invariant: one id per call the program performs at one place of one
    /// component. A body inlined twice (a custom hook, a utility, each
    /// salted copy) is two places, so its calls get two ids: each graft
    /// draws its own range from the component's call cursor. A subtree
    /// copied without a graft — an argument substituted for a parameter,
    /// a body shared by its `Arc` — denotes the same evaluation and keeps
    /// the id. Ids are deterministic: they follow the source order of one
    /// lowering and the order of the grafts.
    Call {
        id: ExprId,
        fn_: Box<Expr>,
        args: Vec<Expr>,
    },
    /// `...xs` as an argument of a call or a `new`: the iterable whose
    /// entries take the positions from here on, however many it holds. It
    /// occurs only in `args`, and from the first one on no argument has a
    /// known position ([`Expr::positional_args`]). Evaluated, it is no value
    /// of its own: what it hands the callee is `xs`'s entries.
    Spread(Box<Expr>),
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

/// What a [`Expr::HookMarker`] binding reads as.
///
/// The distinction is decided at lowering, where "is this React's own hook?"
/// is still known: a React hook with no tracked result really does return
/// `undefined`, while an unresolved custom hook returns something the engine
/// has no information about. Collapsing the two — as a single `undefined`
/// once did — makes an opaque hook's return *provably stable*
/// (`to_stability` joins `Stable` for `undef`), which silences every
/// stability-gated rule on it: a false negative, and the forbidden direction.
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

/// Coarse abstract return-value hint for library hooks.
/// Lives in `ir` to avoid a circular dependency between `ir` and `domains`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SummaryValue {
    /// ⊤ completely unknown; default for hooks without a precise summary.
    Top,
    /// Hook returns a reference-stable value (safe as a `useEffect` dep).
    StableRef,
    /// Hook returns a reference-unstable value (new object/array every render).
    UnstableRef,
    /// A function that **wraps** its own function argument rather than running
    /// it: `form.handleSubmit(onSubmit)` returns an event handler and calls
    /// `onSubmit` only when that handler fires.
    ///
    /// `stable` is the wrapper's *own* identity across renders, which is a
    /// separate claim about a separate thing: react-hook-form's `handleSubmit`
    /// is `useCallback`-backed, `@mantine/form`'s `onSubmit` is a fresh arrow
    /// every render, and both wrap. Riding the timing on the value would make
    /// the mantine entry claim a stability nobody promised.
    Wrapper { stable: bool },
    /// Hook returns an object whose *named* members carry their own summaries.
    ///
    /// The contract libraries actually publish is per member, not per object:
    /// `useForm()` guarantees `setValue` and `reset` are the same function at
    /// every render while `formState` is a Proxy that is not. So the container
    /// stays ⊤ — a member the list does not name answers ⊤ too, which is what
    /// keeps an unlisted member from being credited with a stability nobody
    /// promised.
    ///
    /// `id` is the allocation site the members are recorded at, minted per call
    /// site when the marker is retagged, so two calls of one hook are two
    /// objects (#134).
    Shape {
        id: ExprId,
        members: Arc<Vec<(Symbol, SummaryValue)>>,
    },
    /// ⊤ that moves only on an event the automatic re-render loop cannot
    /// raise: navigation, for a router hook's `searchParams`, `params`,
    /// `pathname` or `location` (#161). The convergence proof reads the
    /// value as holding still across the loop.
    Held,
    /// A function whose call navigates — `useNavigate()`'s result, the
    /// `push`/`replace` of a router object, the setter of react-router's
    /// `useSearchParams` tuple (#161). A call through it in a body the loop
    /// can run moves every [`Self::Held`] value. `stable` is its own
    /// identity across renders, a separate claim: Next documents the router
    /// object's methods stable, react-router's `navigate` and
    /// `setSearchParams` change with the location outside a data router.
    Navigator { stable: bool },
}

impl Expr {
    /// Apply `f` to every DIRECT child expression of `self`.
    ///
    /// This is the canonical child enumeration: walkers write only the arms
    /// they treat specially and delegate the default case here, so a new
    /// `Expr` variant is descended into everywhere without each walker
    /// spelling it out. The match is deliberately exhaustive — do NOT add a
    /// `_` arm.
    ///
    /// What that buys, precisely: adding a variant does *not* break compilation
    /// in one place — measured, it breaks about twenty exhaustive matches
    /// across the crate. But every one of them is a compiler error you are led
    /// to, and none of them silently mistreats the new variant. The arm below
    /// is what keeps that true, so a walker that clones this enumeration with a
    /// `_` arm instead of delegating here defeats the whole scheme: it will
    /// absorb the new variant as "no children" and lose everything under it.
    ///
    /// `FnLit` bodies are CFGs, not child expressions: crossing the function
    /// boundary is a per-walker decision (see [`crate::ir::cfg::CFG::for_each_expr`]).
    /// The `target.addEventListener("event", <FnLit>)` registration shape.
    /// Returns the event name, the listener's parameters and its body. Shared between
    /// `extract_subscriptions` (which reifies it as a `HookEntry::Handler`)
    /// and the slot-writer walk (which skips the listener it would otherwise
    /// double-count as a ⊤-phase nested write — ADR-027 §1). One predicate,
    /// so the two sides cannot drift.
    pub fn subscription_listener(&self) -> Option<(&str, &[Var], &Arc<CFG>)> {
        if let Expr::Call { fn_, args, .. } = self
            && let Expr::FieldAccess { field, .. } = fn_.as_ref()
            && field == "addEventListener"
            && let (
                Some(Expr::Lit(Prim::String(event))),
                Some(Expr::FnLit {
                    params, body_cfg, ..
                }),
            ) = (args.first(), args.get(1))
        {
            return Some((event, params, body_cfg));
        }
        None
    }

    /// A short, source-shaped description for a diagnostic message: what the
    /// reader can look for in their own file.
    ///
    /// Not a printer and not a round-trip — it names the expression, it does
    /// not reproduce it. The point is that a message can say *which* dependency
    /// it means without quoting a position, because a position in `elems` stops
    /// being a position in the source array the moment lowering drops an
    /// elision or flattens a spread (#118).
    ///
    /// Roots read through [`crate::ir::source_name`], so a spliced capture's
    /// internal `#salt` never reaches the user, and a name lowering minted
    /// reads `(…)`: only [`crate::ir::SourceNames::describe`] knows what it
    /// stands for.
    pub fn describe(&self) -> String {
        match self.peel_ts() {
            Expr::Var(v) if crate::ir::is_lowering_temp(v) => "(…)".to_string(),
            Expr::Var(v) => crate::ir::source_name(v).to_string(),
            Expr::FieldAccess { obj, field } => format!("{}.{field}", obj.describe()),
            Expr::IndexAccess { arr, .. } => format!("{}[…]", arr.describe()),
            Expr::Call { fn_, .. } => format!("{}(…)", fn_.describe()),
            Expr::New { fn_, .. } => format!("new {}(…)", fn_.describe()),
            Expr::ObjectLit { .. } => "{…}".to_string(),
            Expr::ArrayLit { .. } => "[…]".to_string(),
            Expr::Arguments { .. } => "the arguments".to_string(),
            Expr::FnLit { .. } => "() => …".to_string(),
            Expr::Lit(Prim::String(v)) => format!("\"{v}\""),
            Expr::Lit(Prim::Int(v)) => v.to_string(),
            Expr::Lit(Prim::Float(v)) => v.to_string(),
            Expr::Lit(Prim::Bool(v)) => v.to_string(),
            Expr::Lit(Prim::Null) => "null".to_string(),
            Expr::Lit(Prim::Unit) => "undefined".to_string(),
            // Naming the operands would be longer than the source. What the
            // reader needs is that the dependency is computed, not which
            // operator computed it.
            Expr::BinOp { .. } | Expr::UnaryOp { .. } => "a computed value".to_string(),
            Expr::CompApp { name, .. } => format!("<{name}/>"),
            Expr::NativeElem { tag, .. } => format!("<{tag}/>"),
            // A hook value has a binding name the caller knows and this type
            // does not; the label keeps the message unambiguous meanwhile.
            Expr::StateVal(l) | Expr::MemoVal(l) | Expr::CallbackVal(l) => format!("hook #{l}"),
            Expr::StateSetter(l) => format!("the setter of hook #{l}"),
            Expr::HookMarker(l, _) => format!("hook #{l}"),
            Expr::SummaryVal(_) | Expr::TSAnnotated(_) => "an unmodelled value".to_string(),
            Expr::Spread(inner) => format!("...{}", inner.describe()),
        }
    }

    /// The arguments of a call whose position is known: those before the
    /// first [`Expr::Spread`]. Every parameter from there on may receive any
    /// argument from there on, or an entry of a spread.
    pub fn positional_args(args: &[Expr]) -> &[Expr] {
        let end = args
            .iter()
            .position(|a| matches!(a, Expr::Spread(_)))
            .unwrap_or(args.len());
        &args[..end]
    }

    pub fn for_each_child<'a>(&'a self, f: &mut impl FnMut(&'a Expr)) {
        match self {
            Expr::Lit(_)
            | Expr::Var(_)
            | Expr::Arguments { .. }
            | Expr::FnLit { .. }
            | Expr::StateVal(_)
            | Expr::StateSetter(_)
            | Expr::MemoVal(_)
            | Expr::CallbackVal(_)
            | Expr::HookMarker(..)
            | Expr::SummaryVal(_) => {}
            Expr::ObjectLit { fields, .. } => {
                for (_, v) in fields {
                    f(v);
                }
            }
            Expr::ArrayLit { elems, .. } => {
                for e in elems {
                    f(e);
                }
            }
            Expr::FieldAccess { obj, .. } => f(obj),
            Expr::IndexAccess { arr, idx } => {
                f(arr);
                f(idx);
            }
            Expr::BinOp { lhs, rhs, .. } => {
                f(lhs);
                f(rhs);
            }
            Expr::UnaryOp { arg, .. } => f(arg),
            Expr::Call { fn_, args, .. } | Expr::New { fn_, args, .. } => {
                f(fn_);
                for a in args {
                    f(a);
                }
            }
            Expr::CompApp { props, .. } => f(props),
            Expr::NativeElem {
                props, children, ..
            } => {
                f(props);
                for c in children {
                    f(c);
                }
            }
            Expr::TSAnnotated(inner) | Expr::Spread(inner) => f(inner),
        }
    }

    /// [`Self::for_each_child`], mutably: the same children, `FnLit` bodies
    /// again not crossed.
    pub fn for_each_child_mut(&mut self, f: &mut impl FnMut(&mut Expr)) {
        match self {
            Expr::Lit(_)
            | Expr::Var(_)
            | Expr::Arguments { .. }
            | Expr::FnLit { .. }
            | Expr::StateVal(_)
            | Expr::StateSetter(_)
            | Expr::MemoVal(_)
            | Expr::CallbackVal(_)
            | Expr::HookMarker(..)
            | Expr::SummaryVal(_) => {}
            Expr::ObjectLit { fields, .. } => {
                for (_, v) in fields {
                    f(v);
                }
            }
            Expr::ArrayLit { elems, .. } => {
                for e in elems {
                    f(e);
                }
            }
            Expr::FieldAccess { obj, .. } => f(obj),
            Expr::IndexAccess { arr, idx } => {
                f(arr);
                f(idx);
            }
            Expr::BinOp { lhs, rhs, .. } => {
                f(lhs);
                f(rhs);
            }
            Expr::UnaryOp { arg, .. } => f(arg),
            Expr::Call { fn_, args, .. } | Expr::New { fn_, args, .. } => {
                f(fn_);
                for a in args {
                    f(a);
                }
            }
            Expr::CompApp { props, .. } => f(props),
            Expr::NativeElem {
                props, children, ..
            } => {
                f(props);
                for c in children {
                    f(c);
                }
            }
            Expr::TSAnnotated(inner) | Expr::Spread(inner) => f(inner),
        }
    }

    /// Returns `true` iff the expression tree contains no `Call`, `New` or
    /// `CompApp` node. `FnLit` bodies are not crossed (they are leaves for
    /// `for_each_child`).
    pub fn is_call_free(&self) -> bool {
        match self {
            Expr::Call { .. }
            | Expr::New { .. }
            | Expr::CompApp { .. }
            | Expr::NativeElem { .. } => false,
            _ => {
                let mut free = true;
                self.for_each_child(&mut |c| free &= c.is_call_free());
                free
            }
        }
    }

    /// The plain binding a member chain is rooted at — `x` for `x`, `x.a`,
    /// `x[i].b`, through TS annotations — or `None` when the chain starts at
    /// anything else (a call, a literal, a hook value).
    pub fn root_var(&self) -> Option<&Symbol> {
        match self.peel_ts() {
            Expr::Var(v) => Some(v),
            Expr::FieldAccess { obj, .. } | Expr::IndexAccess { arr: obj, .. } => obj.root_var(),
            _ => None,
        }
    }

    /// Strip any `TSAnnotated` wrappers, returning the underlying expression.
    pub fn peel_ts(&self) -> &Expr {
        let mut e = self;
        while let Expr::TSAnnotated(inner) = e {
            e = inner;
        }
        e
    }

    /// [`Expr::peel_ts`] for an owned value — for the callers that destructure
    /// the peeled expression rather than borrowing it.
    pub fn peel_ts_owned(self) -> Expr {
        let mut e = self;
        while let Expr::TSAnnotated(inner) = e {
            e = *inner;
        }
        e
    }

    /// The pair a `useState` or `useReducer` call returns when the source keeps
    /// it whole (`const t = useState(0)`, #283): a fresh array of slot
    /// `label`'s value and its setter, allocated at `id` as React allocates
    /// one at every render.
    pub fn state_pair(id: ExprId, label: HookLabel) -> Expr {
        Expr::ArrayLit {
            id,
            elems: vec![Expr::StateVal(label), Expr::StateSetter(label)],
            arity: Arity::Exact(2),
            spread_at: Vec::new(),
        }
    }

    /// The slot whose [`Expr::state_pair`] this is.
    pub fn as_state_pair(&self) -> Option<HookLabel> {
        match self.peel_ts() {
            Expr::ArrayLit {
                elems, spread_at, ..
            } if spread_at.is_empty() => match elems.as_slice() {
                [Expr::StateVal(v), Expr::StateSetter(s)] if v == s => Some(*v),
                _ => None,
            },
            _ => None,
        }
    }
}

#[cfg(test)]
mod accessor_tests {
    use super::*;

    fn getter(id: u64) -> Expr {
        Expr::FnLit {
            id: ExprId(id),
            params: vec![],
            body_cfg: std::sync::Arc::new(crate::ir::cfg::CFG {
                entry: 0,
                blocks: Default::default(),
                edges: Vec::new(),
            }),
        }
    }

    #[test]
    fn an_accessor_key_says_its_half_and_its_member() {
        let k = accessor_key(7, Accessor::Get, Some("a:b"));
        assert!(is_synthetic_key(&k));
        assert_eq!(accessor_of(&k), Some((Accessor::Get, Some("a:b"))));
        let k = accessor_key(8, Accessor::Set, None);
        assert_eq!(accessor_of(&k), Some((Accessor::Set, None)));
        assert_eq!(accessor_of("[computed]3"), None);
        assert_eq!(accessor_of("p"), None);
    }

    #[test]
    fn the_last_definition_of_a_member_says_whether_a_getter_reads_it() {
        let get = |n| (accessor_key(n, Accessor::Get, Some("p")), getter(n));
        let set = (accessor_key(9, Accessor::Set, Some("p")), getter(9));
        let data = ("p".to_string(), Expr::Lit(Prim::Int(1)));
        let other = (accessor_key(10, Accessor::Get, Some("q")), getter(10));
        let spread = (format!("{SPREAD_KEY_PREFIX}11"), Expr::Var("x".into()));
        let found = |fields: Vec<(Symbol, Expr)>| {
            object_getter(&fields, "p").map(|e| match e {
                Expr::FnLit { id, .. } => id.0,
                _ => 0,
            })
        };
        assert_eq!(found(vec![data.clone(), get(1)]), Some(1));
        assert_eq!(found(vec![get(1), data.clone()]), None);
        assert_eq!(found(vec![get(1), set.clone(), other.clone()]), Some(1));
        assert_eq!(found(vec![get(1), spread]), None);
        assert_eq!(found(vec![data, set]), None);
        assert_eq!(found(vec![other]), None);
    }
}
