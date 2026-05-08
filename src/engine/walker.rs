use std::collections::HashMap;

use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::{SourceType, Span};

use crate::core::abs_env::{empty_env, extend, join_env, AbsEnv};
use crate::core::aval::AVal;
use crate::engine::expr_eval::{classify_setter_arg, eval_expr, resolve_value};
use crate::events::{
    AnalysisContext, AnalysisEvent, BranchKind, SourceLocation, ValueResolution,
};
use crate::registry::{HookRegistry, HookSemantics};

// ── Emitter trait ─────────────────────────────────────────────────────────────

pub trait Emitter {
    fn emit(&mut self, event: AnalysisEvent);
}

// ── Walker state ──────────────────────────────────────────────────────────────

struct WalkerState<'a> {
    file: &'a str,
    source: &'a str,
    component_name: String,
    ctx: AnalysisContext,
    cond_depth: u32,
    effect_stack: Vec<String>,
    effect_counter: u32,
    state_counter: u32,
    setter_map: HashMap<String, String>,
    state_value_map: HashMap<String, String>,
    registry: &'a dyn HookRegistry,
    emitter: &'a mut dyn Emitter,
}

impl<'a> WalkerState<'a> {
    fn emit(&mut self, event: AnalysisEvent) {
        self.emitter.emit(event);
    }
    fn current_effect_id(&self) -> Option<String> {
        self.effect_stack.last().cloned()
    }
    fn loc(&self, span: Span) -> SourceLocation {
        span_to_loc(self.source, span, self.file)
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

pub fn walk_file(
    source: &str,
    file: &str,
    emitter: &mut dyn Emitter,
    registry: &dyn HookRegistry,
) -> Result<(), String> {
    let allocator = Allocator::default();
    let source_type = source_type_from_path(file);
    let ret = Parser::new(&allocator, source, source_type)
        .with_options(ParseOptions { parse_regular_expression: false, ..Default::default() })
        .parse();

    if !ret.errors.is_empty() {
        let msg = ret.errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ");
        return Err(msg);
    }

    let mut state = WalkerState {
        file,
        source,
        component_name: String::new(),
        ctx: AnalysisContext::Render,
        cond_depth: 0,
        effect_stack: vec![],
        effect_counter: 0,
        state_counter: 0,
        setter_map: HashMap::new(),
        state_value_map: HashMap::new(),
        registry,
        emitter,
    };

    for stmt in &ret.program.body {
        visit_top_level(stmt, &mut state);
    }

    Ok(())
}

// ── Top-level component detection ─────────────────────────────────────────────

fn visit_top_level(stmt: &Statement, state: &mut WalkerState) {
    match stmt {
        Statement::FunctionDeclaration(func) => {
            if let Some(id) = &func.id {
                if is_component_name(id.name.as_str()) {
                    analyze_component(id.name.as_str(), &func.params, func.body.as_deref(), state);
                }
            }
        }
        Statement::VariableDeclaration(decl) => {
            for vd in &decl.declarations {
                visit_var_declarator_top(vd, state);
            }
        }
        Statement::ExportDefaultDeclaration(exp) => match &exp.declaration {
            ExportDefaultDeclarationKind::FunctionDeclaration(func) => {
                let name = func.id.as_ref().map(|i| i.name.as_str()).unwrap_or("DefaultExport");
                analyze_component(name, &func.params, func.body.as_deref(), state);
            }
            ExportDefaultDeclarationKind::ArrowFunctionExpression(arrow) => {
                analyze_component("DefaultExport", &arrow.params, Some(&arrow.body), state);
            }
            _ => {}
        },
        Statement::ExportNamedDeclaration(exp) => {
            if let Some(inner) = &exp.declaration {
                visit_declaration_top(inner, state);
            }
        }
        _ => {}
    }
}

fn visit_declaration_top(decl: &Declaration, state: &mut WalkerState) {
    match decl {
        Declaration::FunctionDeclaration(func) => {
            if let Some(id) = &func.id {
                if is_component_name(id.name.as_str()) {
                    analyze_component(id.name.as_str(), &func.params, func.body.as_deref(), state);
                }
            }
        }
        Declaration::VariableDeclaration(vd) => {
            for vd in &vd.declarations {
                visit_var_declarator_top(vd, state);
            }
        }
        _ => {}
    }
}

fn visit_var_declarator_top(vd: &VariableDeclarator, state: &mut WalkerState) {
    let name = match &vd.id {
        BindingPattern::BindingIdentifier(id) => id.name.as_str(),
        _ => return,
    };
    if !is_component_name(name) {
        return;
    }
    let Some(init) = &vd.init else { return };
    match init {
        Expression::ArrowFunctionExpression(arrow) => {
            analyze_component(name, &arrow.params, Some(&arrow.body), state);
        }
        Expression::FunctionExpression(func) => {
            analyze_component(name, &func.params, func.body.as_deref(), state);
        }
        _ => {}
    }
}

// ── Component analysis ─────────────────────────────────────────────────────────

fn analyze_component(
    name: &str,
    params: &FormalParameters,
    body: Option<&FunctionBody>,
    state: &mut WalkerState,
) {
    let saved_name = std::mem::replace(&mut state.component_name, name.to_owned());
    let saved_ctx = std::mem::replace(&mut state.ctx, AnalysisContext::Render);
    let saved_depth = std::mem::replace(&mut state.cond_depth, 0);
    let saved_effects = std::mem::replace(&mut state.effect_stack, vec![]);
    let saved_ec = std::mem::replace(&mut state.effect_counter, 0);
    let saved_sc = std::mem::replace(&mut state.state_counter, 0);
    let saved_setter = std::mem::replace(&mut state.setter_map, HashMap::new());
    let saved_value = std::mem::replace(&mut state.state_value_map, HashMap::new());

    let mut env = empty_env();
    for param in &params.items {
        env = bind_pattern(env, &param.pattern, AVal::Top);
    }

    let loc = state.loc(params.span);
    state.emit(AnalysisEvent::ComponentEnter { component_name: name.to_owned(), loc: loc.clone() });

    if let Some(body) = body {
        walk_stmts(&body.statements, env, state);
    }

    state.emit(AnalysisEvent::ComponentExit { component_name: name.to_owned(), loc });

    state.component_name = saved_name;
    state.ctx = saved_ctx;
    state.cond_depth = saved_depth;
    state.effect_stack = saved_effects;
    state.effect_counter = saved_ec;
    state.state_counter = saved_sc;
    state.setter_map = saved_setter;
    state.state_value_map = saved_value;
}

// ── Statement walking ─────────────────────────────────────────────────────────

fn walk_stmts(stmts: &[Statement], mut env: AbsEnv, state: &mut WalkerState) -> AbsEnv {
    for stmt in stmts {
        env = walk_stmt(stmt, env, state);
    }
    env
}

fn walk_stmt(stmt: &Statement, env: AbsEnv, state: &mut WalkerState) -> AbsEnv {
    match stmt {
        Statement::BlockStatement(block) => walk_stmts(&block.body, env, state),
        Statement::VariableDeclaration(decl) => walk_var_decl(decl, env, state),
        Statement::ExpressionStatement(es) => {
            walk_expr(&es.expression, &env, state);
            env
        }
        Statement::ReturnStatement(ret) => {
            if let Some(arg) = &ret.argument {
                walk_expr(arg, &env, state);
            }
            env
        }
        Statement::IfStatement(if_stmt) => walk_if(if_stmt, env, state),
        Statement::WhileStatement(w) => {
            let loc = state.loc(w.span);
            branch_enter(BranchKind::Loop, state, loc.clone());
            state.cond_depth += 1;
            walk_expr(&w.test, &env, state);
            let body_env = walk_stmt(&w.body, env.clone(), state);
            state.cond_depth -= 1;
            branch_exit(BranchKind::Loop, state, loc);
            join_env(&env, &body_env)
        }
        Statement::ForStatement(f) => {
            let loc = state.loc(f.span);
            branch_enter(BranchKind::Loop, state, loc.clone());
            state.cond_depth += 1;
            let env = match &f.init {
                Some(ForStatementInit::VariableDeclaration(vd)) => walk_var_decl(vd, env, state),
                _ => env,
            };
            if let Some(test) = &f.test {
                walk_expr(test, &env, state);
            }
            let body_env = walk_stmt(&f.body, env.clone(), state);
            state.cond_depth -= 1;
            branch_exit(BranchKind::Loop, state, loc);
            join_env(&env, &body_env)
        }
        Statement::ForInStatement(f) => {
            let loc = state.loc(f.span);
            branch_enter(BranchKind::Loop, state, loc.clone());
            state.cond_depth += 1;
            walk_expr(&f.right, &env, state);
            let body_env = walk_stmt(&f.body, env.clone(), state);
            state.cond_depth -= 1;
            branch_exit(BranchKind::Loop, state, loc);
            join_env(&env, &body_env)
        }
        Statement::ForOfStatement(f) => {
            let loc = state.loc(f.span);
            branch_enter(BranchKind::Loop, state, loc.clone());
            state.cond_depth += 1;
            walk_expr(&f.right, &env, state);
            let body_env = walk_stmt(&f.body, env.clone(), state);
            state.cond_depth -= 1;
            branch_exit(BranchKind::Loop, state, loc);
            join_env(&env, &body_env)
        }
        Statement::DoWhileStatement(dw) => {
            let loc = state.loc(dw.span);
            branch_enter(BranchKind::Loop, state, loc.clone());
            state.cond_depth += 1;
            let body_env = walk_stmt(&dw.body, env.clone(), state);
            walk_expr(&dw.test, &body_env, state);
            state.cond_depth -= 1;
            branch_exit(BranchKind::Loop, state, loc);
            join_env(&env, &body_env)
        }
        Statement::SwitchStatement(sw) => walk_switch(sw, env, state),
        Statement::TryStatement(tr) => walk_try(tr, env, state),
        Statement::LabeledStatement(ls) => walk_stmt(&ls.body, env, state),
        Statement::ThrowStatement(t) => {
            walk_expr(&t.argument, &env, state);
            env
        }
        Statement::FunctionDeclaration(func) => {
            let name = func.id.as_ref().map(|i| i.name.as_str()).unwrap_or("__anon");
            extend(&env, name, AVal::Top)
        }
        _ => env,
    }
}

// ── Variable declarations ─────────────────────────────────────────────────────

fn walk_var_decl(decl: &VariableDeclaration, mut env: AbsEnv, state: &mut WalkerState) -> AbsEnv {
    for vd in &decl.declarations {
        env = walk_var_declarator(vd, env, state);
    }
    env
}

fn walk_var_declarator(vd: &VariableDeclarator, env: AbsEnv, state: &mut WalkerState) -> AbsEnv {
    let Some(init) = &vd.init else {
        return bind_pattern(env, &vd.id, AVal::Top);
    };

    if let Expression::CallExpression(call) = init {
        if let Some(new_env) = handle_hook_call(&vd.id, call, env.clone(), state) {
            return new_env;
        }
    }

    let val = eval_expr(&env, init);
    walk_expr(init, &env, state);
    bind_pattern(env, &vd.id, val)
}

// ── Hook dispatch ─────────────────────────────────────────────────────────────

fn handle_hook_call(
    pattern: &BindingPattern,
    call: &CallExpression,
    env: AbsEnv,
    state: &mut WalkerState,
) -> Option<AbsEnv> {
    let name = resolve_callee_name(&call.callee)?;
    let def = state.registry.resolve(name)?;

    let loc = state.loc(call.span);
    state.emit(AnalysisEvent::HookCall {
        hook_name: name.to_owned(),
        cond_depth: state.cond_depth,
        ctx: state.ctx.clone(),
        loc: loc.clone(),
    });

    match def.semantics {
        HookSemantics::State => Some(handle_state_hook(name, pattern, call, env, state, &def, loc)),
        HookSemantics::Effect => {
            handle_effect_hook(name, call, &env, state, &def);
            Some(bind_pattern(env, pattern, AVal::Top))
        }
        _ => {
            for arg in &call.arguments {
                if let Some(e) = arg.as_expression() {
                    walk_expr(e, &env, state);
                }
            }
            Some(bind_pattern(env, pattern, AVal::Top))
        }
    }
}

fn handle_state_hook(
    _name: &str,
    pattern: &BindingPattern,
    call: &CallExpression,
    env: AbsEnv,
    state: &mut WalkerState,
    def: &crate::registry::HookDefinition,
    loc: SourceLocation,
) -> AbsEnv {
    let component = state.component_name.clone();
    let state_id = format!("{}.state{}", component, state.state_counter);
    state.state_counter += 1;

    let initial_value = call
        .arguments
        .first()
        .and_then(|a| a.as_expression())
        .map(|e| resolve_value(&env, e))
        .unwrap_or(ValueResolution::Top);

    let sp = def.state_position.as_ref().cloned()
        .unwrap_or(crate::registry::StatePosition { value: 0, setter: 1 });

    let (value_name, setter_name) = extract_state_names(pattern, &sp, state.state_counter - 1);

    state.emit(AnalysisEvent::StateDeclaration {
        state_id: state_id.clone(),
        value_name: value_name.clone(),
        setter_name: setter_name.clone(),
        initial_value,
        loc,
    });

    state.setter_map.insert(setter_name.clone(), state_id.clone());
    state.state_value_map.insert(value_name.clone(), state_id.clone());

    match pattern {
        BindingPattern::ArrayPattern(arr) => {
            let mut e = env;
            for (i, el) in arr.elements.iter().enumerate() {
                let Some(bp) = el else { continue };
                let val = if i == sp.setter { AVal::Setter(state_id.clone()) } else { AVal::Top };
                e = bind_pattern(e, bp, val);
            }
            e
        }
        _ => bind_pattern(env, pattern, AVal::Top),
    }
}

fn extract_state_names(
    pattern: &BindingPattern,
    sp: &crate::registry::StatePosition,
    counter: u32,
) -> (String, String) {
    match pattern {
        BindingPattern::ArrayPattern(arr) => {
            let val = arr.elements.get(sp.value)
                .and_then(|e| e.as_ref())
                .and_then(|bp| match bp {
                    BindingPattern::BindingIdentifier(id) => Some(id.name.as_str().to_owned()),
                    _ => None,
                })
                .unwrap_or_else(|| format!("_state{counter}"));
            let set = arr.elements.get(sp.setter)
                .and_then(|e| e.as_ref())
                .and_then(|bp| match bp {
                    BindingPattern::BindingIdentifier(id) => Some(id.name.as_str().to_owned()),
                    _ => None,
                })
                .unwrap_or_else(|| format!("_setter{counter}"));
            (val, set)
        }
        _ => (format!("_state{counter}"), format!("_setter{counter}")),
    }
}

fn handle_effect_hook(
    _name: &str,
    call: &CallExpression,
    env: &AbsEnv,
    state: &mut WalkerState,
    def: &crate::registry::HookDefinition,
) {
    let component = state.component_name.clone();
    let effect_id = format!("{}.effect{}", component, state.effect_counter);
    state.effect_counter += 1;

    let cb_pos = def.effect_callback_position.unwrap_or(0);
    let deps_pos = def.deps_position.unwrap_or(1);

    let deps_arg = call.arguments.get(deps_pos).and_then(|a| a.as_expression());
    let (declared_deps, empty_deps) = match deps_arg {
        None => (None, false),
        Some(Expression::ArrayExpression(arr)) => {
            let names: Vec<String> = arr.elements.iter()
                .filter_map(|el| match el {
                    ArrayExpressionElement::Identifier(id) => Some(id.name.as_str().to_owned()),
                    _ => None,
                })
                .collect();
            let empty = names.is_empty();
            (Some(names), empty)
        }
        Some(_) => (None, false),
    };

    let loc = state.loc(call.span);
    state.emit(AnalysisEvent::EffectDeclaration {
        effect_id: effect_id.clone(),
        declared_deps,
        empty_deps,
        loc: loc.clone(),
    });

    let cb = call.arguments.get(cb_pos).and_then(|a| a.as_expression());
    let Some(cb_expr) = cb else { return };

    let saved_ctx = std::mem::replace(&mut state.ctx, AnalysisContext::Effect);
    let saved_depth = state.cond_depth;
    state.cond_depth = 0;
    state.effect_stack.push(effect_id.clone());

    state.emit(AnalysisEvent::EffectEnter { effect_id: effect_id.clone(), loc: loc.clone() });

    match cb_expr {
        Expression::ArrowFunctionExpression(arrow) => {
            walk_stmts(&arrow.body.statements, env.clone(), state);
        }
        Expression::FunctionExpression(func) => {
            if let Some(body) = &func.body {
                walk_stmts(&body.statements, env.clone(), state);
            }
        }
        _ => {}
    }

    state.emit(AnalysisEvent::EffectExit { effect_id, loc });
    state.effect_stack.pop();
    state.ctx = saved_ctx;
    state.cond_depth = saved_depth;
}

// ── Control-flow statement handlers ──────────────────────────────────────────

fn walk_if(stmt: &IfStatement, env: AbsEnv, state: &mut WalkerState) -> AbsEnv {
    let loc = state.loc(stmt.span);
    branch_enter(BranchKind::If, state, loc.clone());
    walk_expr(&stmt.test, &env, state);
    state.cond_depth += 1;
    let then_env = walk_stmt(&stmt.consequent, env.clone(), state);
    let else_env = stmt.alternate.as_ref()
        .map(|alt| walk_stmt(alt, env.clone(), state))
        .unwrap_or(env.clone());
    state.cond_depth -= 1;
    branch_exit(BranchKind::If, state, loc);
    join_env(&then_env, &else_env)
}

fn walk_switch(stmt: &SwitchStatement, env: AbsEnv, state: &mut WalkerState) -> AbsEnv {
    let loc = state.loc(stmt.span);
    branch_enter(BranchKind::Switch, state, loc.clone());
    walk_expr(&stmt.discriminant, &env, state);
    state.cond_depth += 1;
    let mut merged = env.clone();
    for case in &stmt.cases {
        if let Some(test) = &case.test {
            walk_expr(test, &env, state);
        }
        let case_env = walk_stmts(&case.consequent, env.clone(), state);
        merged = join_env(&merged, &case_env);
    }
    state.cond_depth -= 1;
    branch_exit(BranchKind::Switch, state, loc);
    merged
}

fn walk_try(stmt: &TryStatement, env: AbsEnv, state: &mut WalkerState) -> AbsEnv {
    let try_env = walk_stmts(&stmt.block.body, env.clone(), state);
    let catch_env = stmt.handler.as_ref().map_or(env.clone(), |h| {
        let e = if let Some(param) = &h.param {
            bind_pattern(env.clone(), &param.pattern, AVal::Top)
        } else {
            env.clone()
        };
        walk_stmts(&h.body.body, e, state)
    });
    let merged = join_env(&try_env, &catch_env);
    if let Some(fin) = &stmt.finalizer {
        walk_stmts(&fin.body, merged, state)
    } else {
        merged
    }
}

// ── Expression walking ────────────────────────────────────────────────────────

fn walk_expr(expr: &Expression, env: &AbsEnv, state: &mut WalkerState) {
    match expr {
        Expression::Identifier(id) => check_state_read(id, state),
        Expression::CallExpression(call) => walk_call(call, env, state),
        Expression::BinaryExpression(b) => {
            walk_expr(&b.left, env, state);
            walk_expr(&b.right, env, state);
        }
        Expression::LogicalExpression(l) => {
            walk_expr(&l.left, env, state);
            let loc = state.loc(l.span);
            branch_enter(BranchKind::Logical, state, loc.clone());
            state.cond_depth += 1;
            walk_expr(&l.right, env, state);
            state.cond_depth -= 1;
            branch_exit(BranchKind::Logical, state, loc);
        }
        Expression::ConditionalExpression(c) => {
            walk_expr(&c.test, env, state);
            let loc = state.loc(c.span);
            branch_enter(BranchKind::Ternary, state, loc.clone());
            state.cond_depth += 1;
            walk_expr(&c.consequent, env, state);
            walk_expr(&c.alternate, env, state);
            state.cond_depth -= 1;
            branch_exit(BranchKind::Ternary, state, loc);
        }
        Expression::AssignmentExpression(a) => walk_expr(&a.right, env, state),
        Expression::StaticMemberExpression(m) => walk_expr(&m.object, env, state),
        Expression::ComputedMemberExpression(m) => {
            walk_expr(&m.object, env, state);
            walk_expr(&m.expression, env, state);
        }
        Expression::ArrayExpression(a) => {
            for el in &a.elements {
                if let Some(e) = el.as_expression() {
                    walk_expr(e, env, state);
                }
            }
        }
        Expression::ObjectExpression(o) => {
            for prop in &o.properties {
                match prop {
                    ObjectPropertyKind::ObjectProperty(p) => walk_expr(&p.value, env, state),
                    ObjectPropertyKind::SpreadProperty(s) => walk_expr(&s.argument, env, state),
                }
            }
        }
        Expression::SequenceExpression(s) => {
            for e in &s.expressions {
                walk_expr(e, env, state);
            }
        }
        Expression::TSAsExpression(a) => walk_expr(&a.expression, env, state),
        Expression::TSTypeAssertion(a) => walk_expr(&a.expression, env, state),
        Expression::TSSatisfiesExpression(a) => walk_expr(&a.expression, env, state),
        Expression::TSNonNullExpression(a) => walk_expr(&a.expression, env, state),
        Expression::JSXElement(el) => walk_jsx_element(el, env, state),
        Expression::JSXFragment(fr) => walk_jsx_fragment(fr, env, state),
        _ => {}
    }
}

fn walk_call(call: &CallExpression, env: &AbsEnv, state: &mut WalkerState) {
    let callee_name = resolve_callee_name(&call.callee);

    if let Some(name) = callee_name {
        // Check for setter call first (most common hot path)
        if let Some(state_id) = state.setter_map.get(name).cloned() {
            handle_setter_call(name, &state_id, call, env, state);
            return;
        }

        // Check for effect hook called as statement (not bound)
        if let Some(def) = state.registry.resolve(name) {
            if def.semantics == HookSemantics::Effect {
                handle_effect_hook(name, call, env, state, &def);
                return;
            }
        }
    }

    // Generic: walk callee + all non-spread args
    walk_expr(&call.callee, env, state);
    for arg in &call.arguments {
        if let Some(e) = arg.as_expression() {
            walk_expr(e, env, state);
        }
    }
}

fn handle_setter_call(
    setter_name: &str,
    state_id: &str,
    call: &CallExpression,
    env: &AbsEnv,
    state: &mut WalkerState,
) {
    let loc = state.loc(call.span);
    let arg = call.arguments.first().and_then(|a| a.as_expression());
    let (classif, value) = match arg {
        Some(e) => classify_setter_arg(env, e),
        None => (crate::events::SetterArgClassif::Unknown, ValueResolution::Top),
    };
    state.emit(AnalysisEvent::SetterCall {
        state_id: state_id.to_owned(),
        setter_name: setter_name.to_owned(),
        cond_depth: state.cond_depth,
        ctx: state.ctx.clone(),
        argument_classif: classif,
        argument_value: value,
        loc,
    });
}

fn check_state_read(id: &IdentifierReference, state: &mut WalkerState) {
    let name = id.name.as_str();
    if let Some(state_id) = state.state_value_map.get(name).cloned() {
        let loc = state.loc(id.span);
        let effect_id = state.current_effect_id();
        state.emit(AnalysisEvent::StateRead {
            state_id,
            value_name: name.to_owned(),
            cond_depth: state.cond_depth,
            ctx: state.ctx.clone(),
            effect_id,
            loc,
        });
    }
}

// ── JSX walking ───────────────────────────────────────────────────────────────

fn walk_jsx_element(el: &JSXElement, env: &AbsEnv, state: &mut WalkerState) {
    for attr in &el.opening_element.attributes {
        match attr {
            JSXAttributeItem::Attribute(a) => {
                if let Some(JSXAttributeValue::ExpressionContainer(ec)) = &a.value {
                    if let Some(e) = ec.expression.as_expression() {
                        walk_expr(e, env, state);
                    }
                }
            }
            JSXAttributeItem::SpreadAttribute(s) => walk_expr(&s.argument, env, state),
        }
    }
    for child in &el.children {
        walk_jsx_child(child, env, state);
    }
}

fn walk_jsx_fragment(fr: &JSXFragment, env: &AbsEnv, state: &mut WalkerState) {
    for child in &fr.children {
        walk_jsx_child(child, env, state);
    }
}

fn walk_jsx_child(child: &JSXChild, env: &AbsEnv, state: &mut WalkerState) {
    match child {
        JSXChild::ExpressionContainer(ec) => {
            if let Some(e) = ec.expression.as_expression() {
                walk_expr(e, env, state);
            }
        }
        JSXChild::Element(el) => walk_jsx_element(el, env, state),
        JSXChild::Fragment(fr) => walk_jsx_fragment(fr, env, state),
        _ => {}
    }
}

// ── Pattern binding ───────────────────────────────────────────────────────────

pub fn bind_pattern(env: AbsEnv, pattern: &BindingPattern, val: AVal) -> AbsEnv {
    match pattern {
        BindingPattern::BindingIdentifier(id) => extend(&env, id.name.as_str(), val),
        BindingPattern::AssignmentPattern(ap) => bind_pattern(env, &ap.left, val),
        BindingPattern::ArrayPattern(arr) => {
            let mut e = env;
            for el in &arr.elements {
                if let Some(bp) = el {
                    e = bind_pattern(e, bp, AVal::Top);
                }
            }
            if let Some(rest) = &arr.rest {
                e = bind_pattern(e, &rest.argument, AVal::Top);
            }
            e
        }
        BindingPattern::ObjectPattern(obj) => {
            let mut e = env;
            for prop in &obj.properties {
                e = bind_pattern(e, &prop.value, AVal::Top);
            }
            if let Some(rest) = &obj.rest {
                e = bind_pattern(e, &rest.argument, AVal::Top);
            }
            e
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn branch_enter(kind: BranchKind, state: &mut WalkerState, loc: SourceLocation) {
    state.emit(AnalysisEvent::BranchEnter { branch_kind: kind, cond_depth: state.cond_depth, loc });
}

fn branch_exit(kind: BranchKind, state: &mut WalkerState, loc: SourceLocation) {
    state.emit(AnalysisEvent::BranchExit { branch_kind: kind, cond_depth: state.cond_depth, loc });
}

fn resolve_callee_name<'a>(expr: &'a Expression) -> Option<&'a str> {
    match expr {
        Expression::Identifier(id) => Some(id.name.as_str()),
        Expression::StaticMemberExpression(m) => Some(m.property.name.as_str()),
        _ => None,
    }
}

fn is_component_name(name: &str) -> bool {
    name.chars().next().map_or(false, |c| c.is_uppercase())
}

fn source_type_from_path(file: &str) -> SourceType {
    if file.ends_with(".tsx") {
        SourceType::tsx()
    } else if file.ends_with(".ts") {
        SourceType::ts()
    } else if file.ends_with(".jsx") {
        SourceType::jsx()
    } else {
        SourceType::mjs()
    }
}

fn span_to_loc(source: &str, span: Span, file: &str) -> SourceLocation {
    let offset = span.start as usize;
    let before = &source[..offset.min(source.len())];
    let line = before.chars().filter(|&c| c == '\n').count() as u32 + 1;
    let col = before.rfind('\n').map_or(offset, |p| offset - p - 1) as u32 + 1;
    SourceLocation { file: file.to_owned(), line, column: col }
}
