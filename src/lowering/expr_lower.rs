use std::collections::HashMap;

use crate::ir::source_range::SourceRange;
use oxc_ast::ast::*;
use oxc_span::GetSpan;

use std::sync::Arc;

use crate::ir::{
    cfg::{BasicBlock, CFG, EdgeKind, Terminator},
    expr::{BinOp as IrBinOp, Expr, Prim, SummaryValue, UnaryOp as IrUnaryOp},
    stmt::{MemberKey, Stmt},
};

use super::cfg_builder::{BlockBuilder, build_expr_fn_body_cfg, build_fn_body_cfg};

/// An opaque value of unknown kind (⊤). Used for expressions the analysis does
/// not model (exotic operators, `this`/`super`, unhandled syntax). A typed
/// sentinel rather than a magic `Expr::Var("__opaque")`: it evaluates directly
/// to `StateValue::top()` instead of relying on a name lookup missing in the
/// env, so a real user variable can never collide with it and it never shows up
/// as a spurious free-variable capture.
fn opaque() -> Expr {
    Expr::SummaryVal(SummaryValue::Top)
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Lower an Oxc expression to IR.
///
/// Branching expressions (ternary, `&&`, `||`, `??`) split `builder` into new
/// blocks and return a `Var` temp. All other expressions lower structurally.
pub(super) fn lower_expr(expr: &Expression, builder: &mut BlockBuilder) -> Expr {
    match expr {
        // ── Literals ──────────────────────────────────────────────────────────
        Expression::BooleanLiteral(b) => Expr::Lit(Prim::Bool(b.value)),
        Expression::NullLiteral(_) => Expr::Lit(Prim::Null),
        Expression::NumericLiteral(n) => {
            if n.value.fract() == 0.0 && n.value.abs() < i32::MAX as f64 {
                Expr::Lit(Prim::Int(n.value as i32))
            } else {
                Expr::Lit(Prim::Float(n.value))
            }
        }
        Expression::StringLiteral(s) => Expr::Lit(Prim::String(s.value.to_string())),
        Expression::TemplateLiteral(tl) => {
            // Fold quasis and `${…}` interpolations into a left-associated
            // string-concat chain (`"q0" + e0 + "q1" + …`). The concatenated
            // value is irrelevant to the numeric domain, but the chain keeps the
            // reads of every interpolated expression visible to the analysis
            // (deps, captures) — dropping them was a silent read FN.
            let first = tl
                .quasis
                .first()
                .map(|q| q.value.raw.as_str())
                .unwrap_or_default();
            let mut acc = Expr::Lit(Prim::String(first.to_string()));
            for (i, e) in tl.expressions.iter().enumerate() {
                let interp = lower_expr(e, builder);
                acc = Expr::BinOp {
                    op: IrBinOp::Add,
                    lhs: Box::new(acc),
                    rhs: Box::new(interp),
                };
                if let Some(q) = tl.quasis.get(i + 1) {
                    acc = Expr::BinOp {
                        op: IrBinOp::Add,
                        lhs: Box::new(acc),
                        rhs: Box::new(Expr::Lit(Prim::String(q.value.raw.to_string()))),
                    };
                }
            }
            acc
        }

        // ── Identifiers ───────────────────────────────────────────────────────
        Expression::Identifier(id) => {
            if id.name == "undefined" {
                Expr::Lit(Prim::Unit)
            } else {
                Expr::Var(id.name.to_string())
            }
        }
        // `this`/`super` carry no tracked value in a function component; model
        // them as an opaque ⊤ rather than a magic `Var("this")` binding.
        Expression::ThisExpression(_) | Expression::Super(_) => opaque(),

        // ── Arithmetic / comparison ───────────────────────────────────────────
        Expression::BinaryExpression(bin) => Expr::BinOp {
            op: lower_binop(bin.operator),
            lhs: Box::new(lower_expr(&bin.left, builder)),
            rhs: Box::new(lower_expr(&bin.right, builder)),
        },
        Expression::UnaryExpression(un) => {
            // `delete obj.f` is a mutation of `obj`'s pointee, not a read.
            if un.operator == UnaryOperator::Delete
                && let Some((obj, key)) = lower_member_target_expr(&un.argument, builder)
            {
                builder.push_stmt(Stmt::MemberWrite {
                    obj,
                    key,
                    rhs: Expr::Lit(Prim::Unit),
                    span: builder.span_at(un.span.start),
                });
                return Expr::Lit(Prim::Bool(true));
            }
            let arg = lower_expr(&un.argument, builder);
            match un.operator {
                UnaryOperator::UnaryNegation => Expr::UnaryOp {
                    op: IrUnaryOp::Neg,
                    arg: Box::new(arg),
                },
                UnaryOperator::LogicalNot => Expr::UnaryOp {
                    op: IrUnaryOp::Not,
                    arg: Box::new(arg),
                },
                _ => arg,
            }
        }
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
            // `obj.f++` / `arr[i]--`: an in-place write to an untracked cell —
            // the new value is unknown, the mutation of `obj` is the payload.
            SimpleAssignmentTarget::StaticMemberExpression(m) => {
                let obj = lower_expr(&m.object, builder);
                builder.push_stmt(Stmt::MemberWrite {
                    obj,
                    key: MemberKey::Field(m.property.name.to_string()),
                    rhs: opaque(),
                    span: builder.span_at(upd.span.start),
                });
                opaque()
            }
            SimpleAssignmentTarget::ComputedMemberExpression(m) => {
                let obj = lower_expr(&m.object, builder);
                let idx = lower_expr(&m.expression, builder);
                builder.push_stmt(Stmt::MemberWrite {
                    obj,
                    key: MemberKey::Index(idx),
                    rhs: opaque(),
                    span: builder.span_at(upd.span.start),
                });
                opaque()
            }
            _ => opaque(),
        },

        // ── Short-circuit / ternary → block-splitting ─────────────────────────
        Expression::LogicalExpression(log) => lower_logical(log, builder),
        Expression::ConditionalExpression(cond) => lower_ternary(cond, builder),

        // ── Calls ─────────────────────────────────────────────────────────────
        Expression::CallExpression(call) => lower_call(call, builder),
        Expression::NewExpression(new_) => {
            let fn_ = lower_expr(&new_.callee, builder);
            let args = new_
                .arguments
                .iter()
                .filter_map(|a| a.as_expression().map(|e| lower_expr(e, builder)))
                .collect();
            Expr::Call {
                fn_: Box::new(fn_),
                args,
            }
        }
        Expression::TaggedTemplateExpression(t) => Expr::Call {
            // Pass the `${…}` interpolations as call args so their reads survive
            // (the constant quasi strings carry none). The tag receives the
            // cooked-strings array in reality; only the interpolations read vars.
            fn_: Box::new(lower_expr(&t.tag, builder)),
            args: t
                .quasi
                .expressions
                .iter()
                .map(|e| lower_expr(e, builder))
                .collect(),
        },

        // ── Member access ─────────────────────────────────────────────────────
        Expression::ChainExpression(chain) => lower_chain_element(&chain.expression, builder),
        Expression::StaticMemberExpression(m) => Expr::FieldAccess {
            obj: Box::new(lower_expr(&m.object, builder)),
            field: m.property.name.to_string(),
        },
        Expression::ComputedMemberExpression(m) => Expr::IndexAccess {
            arr: Box::new(lower_expr(&m.object, builder)),
            idx: Box::new(lower_expr(&m.expression, builder)),
        },
        Expression::PrivateFieldExpression(p) => Expr::FieldAccess {
            obj: Box::new(lower_expr(&p.object, builder)),
            field: format!("#{}", p.field.name),
        },

        // ── Composites ────────────────────────────────────────────────────────
        Expression::ObjectExpression(obj) => {
            let id = builder.next_expr_id();
            let fields = obj
                .properties
                .iter()
                .filter_map(|prop| match prop {
                    ObjectPropertyKind::ObjectProperty(p) => {
                        let key = match &p.key {
                            PropertyKey::StaticIdentifier(ident) => ident.name.to_string(),
                            PropertyKey::StringLiteral(s) => s.value.to_string(),
                            _ => return None,
                        };
                        Some((key, lower_expr(&p.value, builder)))
                    }
                    _ => None,
                })
                .collect();
            Expr::ObjectLit { id, fields }
        }
        Expression::ArrayExpression(arr) => {
            let id = builder.next_expr_id();
            let elems = arr
                .elements
                .iter()
                .filter_map(|el| el.as_expression().map(|e| lower_expr(e, builder)))
                .collect();
            Expr::ArrayLit { id, elems }
        }

        // ── Functions ─────────────────────────────────────────────────────────
        Expression::ArrowFunctionExpression(arrow) => {
            let id = builder.next_expr_id();
            let smap = builder.smap.clone();
            // Concise body (`x => expr`) carries an implicit return; block body
            // (`x => { ... }`) lowers like any function body.
            let (params, body_cfg) = if arrow.expression {
                build_expr_fn_body_cfg(&arrow.params, &arrow.body, &smap)
            } else {
                build_fn_body_cfg(&arrow.params, &arrow.body, &smap)
            };
            Expr::FnLit {
                id,
                params,
                body_cfg: Arc::new(body_cfg),
            }
        }
        Expression::FunctionExpression(func) => {
            let id = builder.next_expr_id();
            let smap = builder.smap.clone();
            let (params, body_cfg) = if let Some(body) = func.body.as_deref() {
                build_fn_body_cfg(&func.params, body, &smap)
            } else {
                (vec![], empty_cfg())
            };
            Expr::FnLit {
                id,
                params,
                body_cfg: Arc::new(body_cfg),
            }
        }

        // ── JSX ───────────────────────────────────────────────────────────────
        Expression::JSXElement(jsx) => lower_jsx_element(jsx, builder),
        Expression::JSXFragment(frag) => {
            let id = builder.next_expr_id();
            let children = frag
                .children
                .iter()
                .filter_map(|c| lower_jsx_child(c, builder))
                .collect();
            Expr::NativeElem {
                tag: "Fragment".to_string(),
                props: Box::new(Expr::ObjectLit { id, fields: vec![] }),
                children,
                prop_spans: HashMap::new(),
            }
        }

        // ── TypeScript wrappers ───────────────────────────────────────────────
        Expression::ParenthesizedExpression(p) => lower_expr(&p.expression, builder),
        Expression::TSAsExpression(ts) => {
            Expr::TSAnnotated(Box::new(lower_expr(&ts.expression, builder)))
        }
        Expression::TSNonNullExpression(ts) => lower_expr(&ts.expression, builder),
        Expression::TSSatisfiesExpression(ts) => lower_expr(&ts.expression, builder),
        Expression::TSTypeAssertion(ts) => lower_expr(&ts.expression, builder),

        // ── Misc ──────────────────────────────────────────────────────────────
        // Assignment is both a value (its RHS) and an effect (the write). Emit
        // the write as `Stmt::Assign` when the target is a plain identifier so
        // reassignments / compound updates flow into the abstract env (loop
        // counters, accumulators). Member targets (`obj.f`, `arr[i]`) emit
        // `Stmt::MemberWrite`: the cell is untracked but the *mutation* of the
        // object is an observable fact (state-mutation rule, heap field update).
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
                None => match assign_target_member(&assign.left, builder) {
                    Some((obj, key)) => {
                        // Compound ops read the old cell value (untracked → the
                        // written value is unknown); plain `=` writes the RHS.
                        let (rhs, value) = if assign.operator.is_assign() {
                            (rhs_val.clone(), rhs_val)
                        } else {
                            let opaque = opaque();
                            (opaque.clone(), opaque)
                        };
                        builder.push_stmt(Stmt::MemberWrite {
                            obj,
                            key,
                            rhs,
                            span: builder.span_at(assign.span.start),
                        });
                        value
                    }
                    None => rhs_val,
                },
            }
        }
        Expression::SequenceExpression(seq) => {
            // `(a, b, c)` evaluates every operand for its side effects and yields
            // the last. Earlier operands are emitted as `ExprStmt` (exactly like
            // a top-level expression statement) so their setter calls / writes
            // fire — lowering only the last operand dropped them (effect FN).
            let n = seq.expressions.len();
            let mut value = Expr::Lit(Prim::Unit);
            for (i, e) in seq.expressions.iter().enumerate() {
                let lowered = lower_expr(e, builder);
                if i + 1 == n {
                    value = lowered;
                } else {
                    builder.push_stmt(Stmt::ExprStmt(lowered, builder.span_at(e.span().start)));
                }
            }
            value
        }
        Expression::AwaitExpression(aw) => lower_expr(&aw.argument, builder),
        Expression::YieldExpression(y) => y
            .argument
            .as_ref()
            .map(|e| lower_expr(e, builder))
            .unwrap_or(Expr::Lit(Prim::Unit)),

        _ => opaque(),
    }
}

fn lower_call(call: &CallExpression, builder: &mut BlockBuilder) -> Expr {
    let fn_ = lower_expr(&call.callee, builder);
    let args = call
        .arguments
        .iter()
        .filter_map(|a| a.as_expression().map(|e| lower_expr(e, builder)))
        .collect();
    let call_expr = Expr::Call {
        fn_: Box::new(fn_),
        args,
    };
    match &call.type_arguments {
        Some(params) if !params.params.is_empty() => Expr::TSAnnotated(Box::new(call_expr)),
        _ => call_expr,
    }
}

/// `a?.b` / `a?.[i]` / `f?.(x)` — optional chaining lowers like its
/// non-optional counterpart. The nullish short-circuit needs no separate
/// branch: a `Loc`-bound receiver is an object literal on every path (never
/// nullish), and any other receiver already evaluates the access to ⊤,
/// which covers the `undefined` outcome.
fn lower_chain_element(elem: &ChainElement, builder: &mut BlockBuilder) -> Expr {
    match elem {
        ChainElement::CallExpression(call) => lower_call(call, builder),
        ChainElement::TSNonNullExpression(ts) => lower_expr(&ts.expression, builder),
        ChainElement::StaticMemberExpression(m) => Expr::FieldAccess {
            obj: Box::new(lower_expr(&m.object, builder)),
            field: m.property.name.to_string(),
        },
        ChainElement::ComputedMemberExpression(m) => Expr::IndexAccess {
            arr: Box::new(lower_expr(&m.object, builder)),
            idx: Box::new(lower_expr(&m.expression, builder)),
        },
        ChainElement::PrivateFieldExpression(p) => Expr::FieldAccess {
            obj: Box::new(lower_expr(&p.object, builder)),
            field: format!("#{}", p.field.name),
        },
    }
}

// ── Block-splitting lowering ──────────────────────────────────────────────────

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
    let cons = lower_expr(&cond.consequent, builder);
    builder.push_stmt(Stmt::Let {
        var: tmp.clone(),
        rhs: cons,
        span: None,
    });
    let t = builder.seal_with(Terminator::Jump(join_id));
    builder.add_edge(t, join_id, EdgeKind::Unconditional);

    builder.start_block(else_id);
    let alt = lower_expr(&cond.alternate, builder);
    builder.push_stmt(Stmt::Let {
        var: tmp.clone(),
        rhs: alt,
        span: None,
    });
    let e = builder.seal_with(Terminator::Jump(join_id));
    builder.add_edge(e, join_id, EdgeKind::Unconditional);

    builder.start_block(join_id);
    Expr::Var(tmp)
}

/// Short-circuit logical: `a && b`, `a || b`, `a ?? b`
///
/// `&&`: if a truthy → b, else → a
///   current:  Let __tN = a; Branch(Var(__tN), rhs, join)
///   rhs:      Assign __tN = b; Jump(join)
///   join:     Var(__tN)   ← result
///
/// `||`: if a truthy → a, else → b
///   current:  Let __tN = a; Branch(Var(__tN), join, rhs)
///   rhs:      Assign __tN = b; Jump(join)
///   join:     Var(__tN)   ← result
///
/// Pre-declare + Assign: stability(__tN) = stability(a) ⊔ stability(b)
fn lower_logical(log: &LogicalExpression, builder: &mut BlockBuilder) -> Expr {
    let tmp = builder.fresh_temp();
    let left = lower_expr(&log.left, builder);
    builder.push_stmt(Stmt::Let {
        var: tmp.clone(),
        rhs: left,
        span: None,
    });

    let rhs_id = builder.new_block();
    let join_id = builder.new_block();

    let (then_, else_) = match log.operator {
        LogicalOperator::And => (rhs_id, join_id), // truthy → rhs; falsy → join (keep a)
        LogicalOperator::Or | LogicalOperator::Coalesce => (join_id, rhs_id), // truthy → join (keep a); falsy → rhs
    };

    let span = builder.span_at(log.left.span().start);
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
    let right = lower_expr(&log.right, builder);
    builder.push_stmt(Stmt::Assign {
        var: tmp.clone(),
        rhs: right,
        span: None,
    });
    let r = builder.seal_with(Terminator::Jump(join_id));
    builder.add_edge(r, join_id, EdgeKind::Unconditional);

    builder.start_block(join_id);
    Expr::Var(tmp)
}

// ── JSX lowering ──────────────────────────────────────────────────────────────

fn lower_jsx_element(jsx: &JSXElement, builder: &mut BlockBuilder) -> Expr {
    let name = jsx_element_name(&jsx.opening_element.name);
    let children: Vec<Expr> = jsx
        .children
        .iter()
        .filter_map(|c| lower_jsx_child(c, builder))
        .collect();
    let (props, prop_spans) = lower_jsx_props(&jsx.opening_element.attributes, builder);

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
                    elems: children,
                },
            ));
        }
        Expr::CompApp {
            name,
            props: Box::new(props),
            span: builder.span_at(jsx.opening_element.span.start),
        }
    } else {
        Expr::NativeElem {
            tag: name,
            props: Box::new(props),
            children,
            prop_spans,
        }
    }
}

/// Lower JSX attributes to an `ObjectLit`, collecting spans for `onX` props.
/// Returns `(props_expr, prop_spans)` keyed by prop name.
fn lower_jsx_props(
    attrs: &[JSXAttributeItem],
    builder: &mut BlockBuilder,
) -> (Expr, HashMap<String, Option<SourceRange>>) {
    let id = builder.next_expr_id();
    let mut prop_spans: HashMap<String, Option<SourceRange>> = HashMap::new();
    let fields: Vec<(String, Expr)> = attrs
        .iter()
        .filter_map(|attr| match attr {
            JSXAttributeItem::Attribute(a) => {
                let key = match &a.name {
                    JSXAttributeName::Identifier(ident) => ident.name.to_string(),
                    JSXAttributeName::NamespacedName(n) => {
                        format!("{}:{}", n.namespace.name, n.name.name)
                    }
                };
                // Capture span for event-handler props so hook_extractor can set
                // HookEntry::Handler.span without needing the original Oxc AST.
                if is_event_prop_key(&key) {
                    prop_spans.insert(key.clone(), builder.span_at(a.span.start));
                }
                let val = match &a.value {
                    Some(JSXAttributeValue::StringLiteral(s)) => {
                        Expr::Lit(Prim::String(s.value.to_string()))
                    }
                    Some(JSXAttributeValue::ExpressionContainer(ec)) => ec
                        .expression
                        .as_expression()
                        .map(|e| lower_expr(e, builder))
                        .unwrap_or(Expr::Lit(Prim::Unit)),
                    Some(JSXAttributeValue::Element(el)) => lower_jsx_element(el, builder),
                    Some(JSXAttributeValue::Fragment(_)) => Expr::Lit(Prim::Unit),
                    None => Expr::Lit(Prim::Bool(true)), // boolean attribute: <Comp disabled />
                };
                Some((key, val))
            }
            // Keep spreads under a synthetic `...N` key: `<X {...props}/>`
            // forwards every prop — dropping it makes forwarded setters
            // vanish (unknown-child havoc must see them, TODO.md F4).
            // The key can't collide with a real JSX attribute name.
            JSXAttributeItem::SpreadAttribute(s) => {
                let spread_id = builder.next_expr_id();
                Some((
                    format!("...{}", spread_id.0),
                    lower_expr(&s.argument, builder),
                ))
            }
        })
        .collect();
    (Expr::ObjectLit { id, fields }, prop_spans)
}

fn is_event_prop_key(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next() == Some('o')
        && chars.next() == Some('n')
        && chars.next().is_some_and(|c| c.is_ascii_uppercase())
}

fn jsx_element_name(name: &JSXElementName) -> String {
    match name {
        JSXElementName::Identifier(id) => id.name.to_string(),
        JSXElementName::IdentifierReference(id) => id.name.to_string(),
        JSXElementName::MemberExpression(m) => {
            format!("{}.{}", jsx_member_obj_name(&m.object), m.property.name)
        }
        JSXElementName::NamespacedName(n) => format!("{}:{}", n.namespace.name, n.name.name),
        JSXElementName::ThisExpression(_) => "this".to_string(),
    }
}

fn jsx_member_obj_name(obj: &JSXMemberExpressionObject) -> String {
    match obj {
        JSXMemberExpressionObject::IdentifierReference(id) => id.name.to_string(),
        JSXMemberExpressionObject::MemberExpression(m) => {
            format!("{}.{}", jsx_member_obj_name(&m.object), m.property.name)
        }
        JSXMemberExpressionObject::ThisExpression(_) => "this".to_string(),
    }
}

fn lower_jsx_child(child: &JSXChild, builder: &mut BlockBuilder) -> Option<Expr> {
    match child {
        JSXChild::Element(el) => Some(lower_jsx_element(el, builder)),
        JSXChild::Fragment(frag) => {
            let id = builder.next_expr_id();
            let children = frag
                .children
                .iter()
                .filter_map(|c| lower_jsx_child(c, builder))
                .collect();
            Some(Expr::NativeElem {
                tag: "Fragment".to_string(),
                props: Box::new(Expr::ObjectLit { id, fields: vec![] }),
                children,
                prop_spans: HashMap::new(),
            })
        }
        JSXChild::ExpressionContainer(ec) => ec
            .expression
            .as_expression()
            .map(|e| lower_expr(e, builder)),
        JSXChild::Text(_) | JSXChild::Spread(_) => None,
    }
}

// ── Operator mapping ──────────────────────────────────────────────────────────

/// Identifier name of a plain assignment target, or `None` for member/index/
/// pattern targets (which the abstract env does not track as a single cell).
pub(super) fn assign_target_ident(target: &AssignmentTarget) -> Option<String> {
    match target {
        AssignmentTarget::AssignmentTargetIdentifier(id) => Some(id.name.to_string()),
        _ => None,
    }
}

/// Lower a member assignment target (`obj.f = …`, `arr[i] = …`) to its object
/// expression and member key. `None` for identifier/pattern targets.
fn assign_target_member(
    target: &AssignmentTarget,
    builder: &mut BlockBuilder,
) -> Option<(Expr, MemberKey)> {
    match target {
        AssignmentTarget::StaticMemberExpression(m) => Some((
            lower_expr(&m.object, builder),
            MemberKey::Field(m.property.name.to_string()),
        )),
        AssignmentTarget::ComputedMemberExpression(m) => {
            let obj = lower_expr(&m.object, builder);
            let idx = lower_expr(&m.expression, builder);
            Some((obj, MemberKey::Index(idx)))
        }
        AssignmentTarget::PrivateFieldExpression(p) => Some((
            lower_expr(&p.object, builder),
            MemberKey::Field(format!("#{}", p.field.name)),
        )),
        _ => None,
    }
}

/// Like [`assign_target_member`] but for a member *expression* in write
/// position (`delete obj.f`).
fn lower_member_target_expr(
    expr: &Expression,
    builder: &mut BlockBuilder,
) -> Option<(Expr, MemberKey)> {
    match expr {
        Expression::StaticMemberExpression(m) => Some((
            lower_expr(&m.object, builder),
            MemberKey::Field(m.property.name.to_string()),
        )),
        Expression::ComputedMemberExpression(m) => {
            let obj = lower_expr(&m.object, builder);
            let idx = lower_expr(&m.expression, builder);
            Some((obj, MemberKey::Index(idx)))
        }
        _ => None,
    }
}

/// `IrBinOp` for a compound-assignment operator, restricted to those
/// [`lower_binop`] maps faithfully. Returns `None` for `=` and for operators
/// that would silently fall back to `Add` (%=, **=, bitwise, logical).
fn faithful_compound_binop(op: AssignmentOperator) -> Option<IrBinOp> {
    match op {
        AssignmentOperator::Addition => Some(IrBinOp::Add),
        AssignmentOperator::Subtraction => Some(IrBinOp::Sub),
        AssignmentOperator::Multiplication => Some(IrBinOp::Mul),
        AssignmentOperator::Division => Some(IrBinOp::Div),
        _ => None,
    }
}

fn lower_binop(op: BinaryOperator) -> IrBinOp {
    match op {
        BinaryOperator::Addition => IrBinOp::Add,
        BinaryOperator::Subtraction => IrBinOp::Sub,
        BinaryOperator::Multiplication => IrBinOp::Mul,
        BinaryOperator::Division => IrBinOp::Div,
        BinaryOperator::Equality | BinaryOperator::StrictEquality => IrBinOp::Eq,
        BinaryOperator::Inequality | BinaryOperator::StrictInequality => IrBinOp::Neq,
        BinaryOperator::LessThan => IrBinOp::Lt,
        BinaryOperator::GreaterThan => IrBinOp::Gt,
        BinaryOperator::LessEqualThan => IrBinOp::Leq,
        BinaryOperator::GreaterEqualThan => IrBinOp::Geq,
        _ => IrBinOp::Unknown, // keep unsupported operators soundly opaque.
    }
}

// ── Shared helpers (used by cfg_builder.rs too) ───────────────────────────────

pub(super) fn empty_cfg() -> CFG {
    let mut blocks = HashMap::new();
    blocks.insert(
        0,
        BasicBlock {
            id: 0,
            stmts: vec![],
            term: Terminator::Unreachable,
        },
    );
    CFG {
        entry: 0,
        blocks,
        edges: vec![],
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::cfg::EdgeKind;
    use crate::lowering::cfg_builder::build_cfg;
    use oxc_allocator::Allocator;
    use oxc_ast::ast::Statement;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;

    fn build(src: &str) -> CFG {
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, src, SourceType::tsx())
            .with_options(ParseOptions::default())
            .parse();
        assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
        let func = ret.program.body.iter().find_map(|s| match s {
            Statement::FunctionDeclaration(f) => f
                .body
                .as_ref()
                .map(|b| build_cfg(b, &crate::ir::SourceMap::empty())),
            _ => None,
        });
        func.expect("no function found")
    }

    #[test]
    fn ternary_splits_three_blocks() {
        // const x = cond ? a : b; return x;
        let cfg = build("function f(cond, a, b) { const x = cond ? a : b; return x; }");
        // entry(Branch) + then(Let tmp=a, Jump) + else(Let tmp=b, Jump) + join(Let x=tmp, Return)
        assert!(
            cfg.blocks.len() >= 4,
            "expected ≥4 blocks, got {}",
            cfg.blocks.len()
        );
        let if_true = cfg
            .edges
            .iter()
            .filter(|e| matches!(e.kind, EdgeKind::IfTrue))
            .count();
        let if_false = cfg
            .edges
            .iter()
            .filter(|e| matches!(e.kind, EdgeKind::IfFalse))
            .count();
        assert_eq!(if_true, 1);
        assert_eq!(if_false, 1);
    }

    #[test]
    fn logical_and_splits_blocks() {
        // enabled && doSomething() the call must be inside a conditional block
        let cfg = build("function f(enabled) { enabled && doSomething(); }");
        // entry: Let __t0=enabled, Branch(Var(__t0), rhs, join)
        // rhs: Assign __t0 = doSomething(), Jump(join)
        // join: ExprStmt(Var(__t0)) [pushed by ExprStmt handler] + Unreachable
        assert!(
            cfg.blocks.len() >= 3,
            "expected ≥3 blocks, got {}",
            cfg.blocks.len()
        );
        let branches: Vec<_> = cfg
            .blocks
            .values()
            .filter(|b| matches!(b.term, crate::ir::cfg::Terminator::Branch { .. }))
            .collect();
        assert_eq!(branches.len(), 1, "expected 1 branch block");
    }

    #[test]
    fn logical_or_splits_blocks() {
        let cfg = build("function f(a, b) { return a || b; }");
        assert!(cfg.blocks.len() >= 3);
        let back_edges = cfg
            .edges
            .iter()
            .filter(|e| matches!(e.kind, EdgeKind::Back))
            .count();
        assert_eq!(back_edges, 0); // no loops
    }

    #[test]
    fn nested_ternary() {
        let cfg = build("function f(a, b, c) { return a ? b : c ? 1 : 0; }");
        // outer ternary + inner ternary = 2 branches
        let branches = cfg
            .blocks
            .values()
            .filter(|b| matches!(b.term, crate::ir::cfg::Terminator::Branch { .. }))
            .count();
        assert!(
            branches >= 2,
            "expected ≥2 branches for nested ternary, got {branches}"
        );
    }

    #[test]
    fn concise_arrow_body_returns_its_expression() {
        // A concise-body arrow (`c => c + 1`) must lower its implicit return into a
        // `Return` terminator carrying the expression; otherwise the body evaluates
        // to Bottom and functional-updater infinite loops (`setCount(c => c + 1)`)
        // go undetected.
        let cfg = build("function f() { const cb = (c) => c + 1; return cb; }");
        let entry = cfg.blocks.get(&cfg.entry).unwrap();
        let body = match entry.stmts.first() {
            Some(Stmt::Let {
                rhs: Expr::FnLit { body_cfg, .. },
                ..
            }) => body_cfg,
            other => panic!("expected `const cb = FnLit`, got {other:?}"),
        };
        let body_entry = body.blocks.get(&body.entry).expect("body entry block");
        assert!(
            matches!(body_entry.term, Terminator::Return(Expr::BinOp { .. })),
            "concise arrow body must Return its expression, got {:?}",
            body_entry.term
        );
    }

    #[test]
    fn jsx_no_panic() {
        build("function App() { return <div className=\"foo\"><span>{x}</span></div>; }");
    }

    #[test]
    fn arrow_fn_gets_sub_cfg() {
        let cfg = build("function f() { const cb = () => 42; return cb; }");
        let entry = cfg.blocks.get(&cfg.entry).unwrap();
        // First stmt should be Let { var: "cb", rhs: FnLit { ... } }
        assert!(
            matches!(
                entry.stmts.first(),
                Some(crate::ir::stmt::Stmt::Let {
                    rhs: Expr::FnLit { .. },
                    ..
                })
            ),
            "expected FnLit for arrow function"
        );
    }

    #[test]
    fn coalesce_splits_blocks() {
        let cfg = build("function f(a, b) { return a ?? b; }");
        assert!(cfg.blocks.len() >= 3);
    }

    /// All `Stmt::Assign { var, rhs }` in `cfg`'s entry block, for assertions.
    fn entry_assigns(cfg: &CFG) -> Vec<(String, Expr)> {
        cfg.blocks
            .get(&cfg.entry)
            .unwrap()
            .stmts
            .iter()
            .filter_map(|s| match s {
                Stmt::Assign { var, rhs, .. } => Some((var.clone(), rhs.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn reassignment_emits_assign() {
        // `i = i + 1` must write back to `i` via a `Stmt::Assign`.
        let cfg = build("function f() { let i = 0; i = i + 1; }");
        let assigns = entry_assigns(&cfg);
        assert!(
            assigns.iter().any(|(v, rhs)| v == "i"
                && matches!(
                    rhs,
                    Expr::BinOp {
                        op: IrBinOp::Add,
                        ..
                    }
                )),
            "expected `Assign i = i + 1`, got {assigns:?}"
        );
    }

    #[test]
    fn compound_assignment_reconstructs_binop() {
        // `s += 2` → `s = s + 2`.
        let cfg = build("function f() { let s = 0; s += 2; }");
        let assigns = entry_assigns(&cfg);
        let (_, rhs) = assigns
            .iter()
            .find(|(v, _)| v == "s")
            .expect("expected Assign for s");
        match rhs {
            Expr::BinOp { op, lhs, .. } => {
                assert!(matches!(op, IrBinOp::Add));
                assert!(matches!(lhs.as_ref(), Expr::Var(v) if v == "s"));
            }
            other => panic!("expected `s + 2`, got {other:?}"),
        }
    }

    #[test]
    fn update_expression_emits_increment() {
        // `i++` → `i = i + 1`.
        let cfg = build("function f() { let i = 0; i++; }");
        let assigns = entry_assigns(&cfg);
        assert!(
            assigns.iter().any(|(v, rhs)| v == "i"
                && matches!(
                    rhs,
                    Expr::BinOp {
                        op: IrBinOp::Add,
                        ..
                    }
                )),
            "expected `Assign i = i + 1` from i++, got {assigns:?}"
        );
    }

    #[test]
    fn decrement_emits_sub() {
        let cfg = build("function f() { let i = 0; i--; }");
        let assigns = entry_assigns(&cfg);
        assert!(
            assigns.iter().any(|(v, rhs)| v == "i"
                && matches!(
                    rhs,
                    Expr::BinOp {
                        op: IrBinOp::Sub,
                        ..
                    }
                )),
            "expected `Assign i = i - 1` from i--, got {assigns:?}"
        );
    }

    #[test]
    fn exotic_compound_havocs_target() {
        // `x %= 3`: `%` has no faithful IrBinOp → havoc to opaque ⊤,
        // never silently aliased onto `Add`.
        let cfg = build("function f() { let x = 9; x %= 3; }");
        let assigns = entry_assigns(&cfg);
        let (_, rhs) = assigns
            .iter()
            .find(|(v, _)| v == "x")
            .expect("expected Assign for x");
        assert!(
            matches!(rhs, Expr::SummaryVal(SummaryValue::Top)),
            "exotic compound must havoc, got {rhs:?}"
        );
    }

    #[test]
    fn unsupported_binary_operators_remain_opaque() {
        for source in [
            "function f(n) { return n % 2; }",
            "function f(n) { return n >> 3; }",
        ] {
            let cfg = build(source);
            let body = cfg.blocks.get(&0).expect("expected entry block");
            assert!(
                matches!(
                    body.term,
                    Terminator::Return(Expr::BinOp {
                        op: IrBinOp::Unknown,
                        ..
                    })
                ),
                "unsupported operators must not be modeled as addition: {source}"
            );
        }
    }

    #[test]
    fn member_assignment_emits_no_write() {
        // `obj.f = 1`: untracked cell → no `Assign` (RHS still lowered).
        let cfg = build("function f(obj) { obj.f = 1; }");
        assert!(
            entry_assigns(&cfg).is_empty(),
            "member-target assignment must not emit a tracked Assign"
        );
    }
}
