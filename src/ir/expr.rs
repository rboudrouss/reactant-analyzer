use std::sync::Arc;

use crate::ir::{
    cfg::CFG,
    hooks::Arity,
    source_range::SourceRange,
    types::{ExprId, HookLabel, Symbol, Var},
};

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
    Eq,
    Neq,
    Lt,
    Gt,
    Leq,
    Geq,
    /// Bitwise and shift: `&`, `|`, `^`, `<<`, `>>`, `>>>`.
    ///
    /// Real variants rather than one opaque `Unknown`, because they carry
    /// information `Unknown` cannot express: JS coerces both operands to int32
    /// (uint32 for `>>>`), so the result is *always* a number in a known range.
    /// Folding them into `Unknown` erased which operator it was, and with it
    /// every one of those guarantees.
    ///
    /// This is the shared decision for every unmodelled operator: give it a
    /// variant, never widen `Unknown`'s meaning. `Unknown` is for operators
    /// nothing is known about at all.
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    /// `>>>` — unsigned, so the result is a *uint32*, not an int32.
    UShr,
    /// An operator whose concrete semantics are not modeled by the abstract domain.
    Unknown,
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

/// The suffix of `fields` a name lookup may read: everything after the last
/// spread. Members written before one may have been overwritten by something
/// the per-member map cannot see, so they answer nothing.
///
/// Shared so the two readers that resolve members by name cannot drift apart
/// on the rule — the abstract interpreter's `obj_members` and the binding
/// chase in [`crate::engine::seeds`].
pub fn members_after_last_spread(fields: &[(Symbol, Expr)]) -> &[(Symbol, Expr)] {
    let start = fields
        .iter()
        .rposition(|(k, _)| k.starts_with(SPREAD_KEY_PREFIX))
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
    /// Returns the event name and the listener body. Shared between
    /// `extract_subscriptions` (which reifies it as a `HookEntry::Handler`)
    /// and the slot-writer walk (which skips the listener it would otherwise
    /// double-count as a ⊤-phase nested write — ADR-027 §1). One predicate,
    /// so the two sides cannot drift.
    pub fn subscription_listener(&self) -> Option<(&str, &Arc<CFG>)> {
        if let Expr::Call { fn_, args } = self
            && let Expr::FieldAccess { field, .. } = fn_.as_ref()
            && field == "addEventListener"
            && let (Some(Expr::Lit(Prim::String(event))), Some(Expr::FnLit { body_cfg, .. })) =
                (args.first(), args.get(1))
        {
            return Some((event, body_cfg));
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
    /// internal `#salt` never reaches the user.
    pub fn describe(&self) -> String {
        match self.peel_ts() {
            Expr::Var(v) => crate::ir::source_name(v).to_string(),
            Expr::FieldAccess { obj, field } => format!("{}.{field}", obj.describe()),
            Expr::IndexAccess { arr, .. } => format!("{}[…]", arr.describe()),
            Expr::Call { fn_, .. } => format!("{}(…)", fn_.describe()),
            Expr::ObjectLit { .. } => "{…}".to_string(),
            Expr::ArrayLit { .. } => "[…]".to_string(),
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
        }
    }

    pub fn for_each_child<'a>(&'a self, f: &mut impl FnMut(&'a Expr)) {
        match self {
            Expr::Lit(_)
            | Expr::Var(_)
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
            Expr::Call { fn_, args } => {
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
            Expr::TSAnnotated(inner) => f(inner),
        }
    }

    /// Returns `true` iff the expression tree contains no `Call` or `CompApp` node.
    /// `FnLit` bodies are not crossed (they are leaves for `for_each_child`).
    pub fn is_call_free(&self) -> bool {
        match self {
            Expr::Call { .. } | Expr::CompApp { .. } | Expr::NativeElem { .. } => false,
            _ => {
                let mut free = true;
                self.for_each_child(&mut |c| free &= c.is_call_free());
                free
            }
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
}
