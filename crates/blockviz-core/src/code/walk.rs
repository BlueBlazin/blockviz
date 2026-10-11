//! Borrowing traversals over the shared syntax tree.

use super::syntax::*;
use std::collections::BTreeSet;

/// Immediate sub-expressions of an expression. A function expression's body
/// is a scope of its own and is not included (see `function_bodies`).
pub fn children<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
    match e {
        Expr::BoolOp(x) => out.extend(&x.values),
        Expr::Named(x) => {
            out.push(&x.target);
            out.push(&x.value)
        }
        Expr::BinOp(x) => {
            out.push(&x.left);
            out.push(&x.right)
        }
        Expr::UnaryOp(x) => out.push(&x.operand),
        Expr::Lambda(x) => {
            out.extend(x.params.iter().filter_map(|p| p.default.as_ref()));
            out.push(&x.body)
        }
        Expr::Function(f) => out.extend(f.params.iter().filter_map(|p| p.default.as_ref())),
        Expr::If(x) => {
            out.push(&x.test);
            out.push(&x.body);
            out.push(&x.orelse)
        }
        Expr::Dict(x) => {
            for item in &x.items {
                out.extend(item.key.as_ref());
                out.push(&item.value);
            }
        }
        Expr::Set(x) | Expr::List(x) | Expr::Tuple(x) => out.extend(&x.elts),
        Expr::Comp(x) => {
            out.push(&x.elt);
            out.extend(x.value.as_deref());
            for g in &x.generators {
                out.push(&g.target);
                out.push(&g.iter);
                out.extend(&g.ifs);
            }
        }
        Expr::Await(x) | Expr::YieldFrom(x) | Expr::Starred(x) => out.push(&x.value),
        Expr::Yield(x) => out.extend(x.value.as_deref()),
        Expr::Compare(x) => out.extend(x.operands.iter()),
        Expr::Call(x) => {
            out.push(&x.func);
            out.extend(&x.arguments.args);
            out.extend(x.arguments.keywords.iter().map(|k| &k.value))
        }
        Expr::Template(x) => {
            out.extend(x.tag.as_deref());
            for p in &x.parts {
                if let TemplatePart::Hole { expr, spec } = p {
                    out.push(expr);
                    out.extend(spec);
                }
            }
        }
        Expr::Attribute(x) => out.push(&x.value),
        Expr::Subscript(x) => {
            out.push(&x.value);
            out.push(&x.slice)
        }
        Expr::Slice(x) => {
            out.extend(x.lower.as_deref());
            out.extend(x.upper.as_deref());
            out.extend(x.step.as_deref())
        }
        Expr::StringLiteral(_) | Expr::BytesLiteral(_) | Expr::NumberLiteral(_) | Expr::BooleanLiteral(_) | Expr::NoneLiteral(_) | Expr::EllipsisLiteral(_) | Expr::Name(_) | Expr::Other(_) => {}
    }
}

/// Pre-order walk of an expression tree.
pub fn walk<'a>(e: &'a Expr, f: &mut impl FnMut(&'a Expr)) {
    f(e);
    let mut kids = vec![];
    children(e, &mut kids);
    for k in kids {
        walk(k, f);
    }
}

/// Function expressions anywhere in an expression (callbacks, handlers).
pub fn function_bodies<'a>(e: &'a Expr, out: &mut Vec<&'a FunctionDef>) {
    walk(e, &mut |x| {
        if let Expr::Function(f) = x {
            out.push(f);
        }
    });
}

/// Names bound inside comprehensions and lambdas are local to them.
fn local_binders(e: &Expr, out: &mut BTreeSet<String>) {
    match e {
        Expr::Comp(x) => {
            for g in &x.generators {
                walk(&g.target, &mut |t| {
                    if let Expr::Name(n) = t {
                        out.insert(n.id.to_string());
                    }
                });
            }
        }
        Expr::Lambda(x) => {
            for p in &x.params {
                out.insert(p.name.clone());
            }
        }
        _ => {}
    }
}

/// All parameter names, in declaration order.
pub fn param_names(p: &[Param]) -> Vec<String> {
    p.iter().map(|a| a.name.clone()).collect()
}

/// Free names loaded by an expression, in first-use order.
pub fn loads(e: &Expr) -> Vec<String> {
    let mut bound = BTreeSet::new();
    walk(e, &mut |x| local_binders(x, &mut bound));
    let mut out: Vec<String> = vec![];
    walk(e, &mut |x| {
        if let Expr::Name(n) = x {
            if n.ctx == Ctx::Load && !bound.contains(n.id.as_str()) && !out.iter().any(|o| o == n.id.as_str()) {
                out.push(n.id.to_string());
            }
        }
    });
    out
}

/// Names a store target binds (`a`, `a, b`, `(a, *b)`).
pub fn target_names(t: &Expr, out: &mut Vec<String>) {
    match t {
        Expr::Name(n) => out.push(n.id.to_string()),
        Expr::Tuple(x) | Expr::List(x) => x.elts.iter().for_each(|e| target_names(e, out)),
        Expr::Starred(x) => target_names(&x.value, out),
        _ => {}
    }
}

/// Visits every expression rooted in a statement list, descending into nested
/// statements; nested function and class bodies are included when `deep`.
pub fn stmt_exprs<'a>(stmts: &'a [Stmt], deep: bool, f: &mut impl FnMut(&'a Expr)) {
    for s in stmts {
        stmt_expr(s, deep, f);
    }
}

pub fn stmt_expr<'a>(s: &'a Stmt, deep: bool, f: &mut impl FnMut(&'a Expr)) {
    match s {
        Stmt::FunctionDef(x) => {
            x.decorators.iter().for_each(|d| f(&d.expression));
            for a in &x.params {
                if let Some(d) = &a.default {
                    f(d);
                }
            }
            if deep {
                stmt_exprs(&x.body, deep, f);
            }
        }
        Stmt::ClassDef(x) => {
            x.bases.iter().for_each(&mut *f);
            x.keywords.iter().for_each(|k| f(&k.value));
            x.decorators.iter().for_each(|d| f(&d.expression));
            if deep {
                stmt_exprs(&x.body, deep, f);
            }
        }
        Stmt::Return(x) => {
            if let Some(v) = &x.value {
                f(v)
            }
        }
        Stmt::Delete(x) => x.targets.iter().for_each(&mut *f),
        Stmt::TypeAlias(x) => {
            f(&x.name);
            f(&x.value)
        }
        Stmt::Assign(x) => {
            x.targets.iter().for_each(&mut *f);
            f(&x.value)
        }
        Stmt::AugAssign(x) => {
            f(&x.target);
            f(&x.value)
        }
        Stmt::AnnAssign(x) => {
            f(&x.target);
            if let Some(v) = &x.value {
                f(v)
            }
        }
        Stmt::For(x) => {
            f(&x.target);
            f(&x.iter);
            stmt_exprs(&x.body, deep, f);
            stmt_exprs(&x.orelse, deep, f)
        }
        Stmt::While(x) => {
            f(&x.test);
            stmt_exprs(&x.body, deep, f);
            stmt_exprs(&x.orelse, deep, f)
        }
        Stmt::If(x) => {
            f(&x.test);
            stmt_exprs(&x.body, deep, f);
            for c in &x.elif_else_clauses {
                if let Some(t) = &c.test {
                    f(t);
                }
                stmt_exprs(&c.body, deep, f);
            }
        }
        Stmt::With(x) => {
            for i in &x.items {
                f(&i.context_expr);
                if let Some(v) = &i.optional_vars {
                    f(v);
                }
            }
            stmt_exprs(&x.body, deep, f)
        }
        Stmt::Match(x) => {
            f(&x.subject);
            for c in &x.cases {
                if let Some(g) = &c.guard {
                    f(g);
                }
                stmt_exprs(&c.body, deep, f);
            }
        }
        Stmt::Raise(x) => {
            if let Some(v) = &x.exc {
                f(v)
            }
            if let Some(v) = &x.cause {
                f(v)
            }
        }
        Stmt::Try(x) => {
            stmt_exprs(&x.body, deep, f);
            for h in &x.handlers {
                if let Some(t) = &h.type_ {
                    f(t);
                }
                stmt_exprs(&h.body, deep, f);
            }
            stmt_exprs(&x.orelse, deep, f);
            stmt_exprs(&x.finalbody, deep, f)
        }
        Stmt::Assert(x) => {
            f(&x.test);
            if let Some(m) = &x.msg {
                f(m)
            }
        }
        Stmt::Expr(x) => f(&x.value),
        Stmt::Import(_) | Stmt::ImportFrom(_) | Stmt::Global(_) | Stmt::Nonlocal(_) | Stmt::Pass(_) | Stmt::Break(_) | Stmt::Continue(_) | Stmt::Export(_) | Stmt::Other(_) => {}
    }
}

/// Free names read by a function-like body: loads minus parameters and local bindings.
pub fn free_names(params: &[String], body: &[Stmt]) -> Vec<String> {
    let mut local: BTreeSet<String> = params.iter().cloned().collect();
    let mut declared_global: BTreeSet<String> = BTreeSet::new();
    collect_bindings(body, &mut local, &mut declared_global);
    for g in &declared_global {
        local.remove(g);
    }
    let mut out: Vec<String> = vec![];
    stmt_exprs(body, true, &mut |e| {
        for n in loads(e) {
            if !local.contains(&n) && !out.contains(&n) {
                out.push(n);
            }
        }
    });
    out
}

/// Names bound at the top level of a body (not inside nested functions).
pub fn collect_bindings(body: &[Stmt], out: &mut BTreeSet<String>, globals: &mut BTreeSet<String>) {
    for s in body {
        let mut targets = vec![];
        match s {
            Stmt::FunctionDef(x) => {
                out.insert(x.name.to_string());
            }
            Stmt::ClassDef(x) => {
                out.insert(x.name.to_string());
            }
            Stmt::TypeAlias(x) => target_names(&x.name, &mut targets),
            Stmt::Assign(x) => x.targets.iter().for_each(|t| target_names(t, &mut targets)),
            Stmt::AugAssign(x) => target_names(&x.target, &mut targets),
            Stmt::AnnAssign(x) => target_names(&x.target, &mut targets),
            Stmt::For(x) => {
                target_names(&x.target, &mut targets);
                collect_bindings(&x.body, out, globals);
                collect_bindings(&x.orelse, out, globals);
            }
            Stmt::While(x) => {
                collect_bindings(&x.body, out, globals);
                collect_bindings(&x.orelse, out, globals);
            }
            Stmt::If(x) => {
                collect_bindings(&x.body, out, globals);
                for c in &x.elif_else_clauses {
                    collect_bindings(&c.body, out, globals);
                }
            }
            Stmt::With(x) => {
                for i in &x.items {
                    if let Some(v) = &i.optional_vars {
                        target_names(v, &mut targets);
                    }
                }
                collect_bindings(&x.body, out, globals);
            }
            Stmt::Try(x) => {
                collect_bindings(&x.body, out, globals);
                for h in &x.handlers {
                    if let Some(n) = &h.name {
                        out.insert(n.to_string());
                    }
                    collect_bindings(&h.body, out, globals);
                }
                collect_bindings(&x.orelse, out, globals);
                collect_bindings(&x.finalbody, out, globals);
            }
            Stmt::Match(x) => {
                for c in &x.cases {
                    collect_bindings(&c.body, out, globals);
                }
            }
            Stmt::Import(x) => {
                for a in &x.names {
                    match (&a.asname, x.style) {
                        (Some(n), _) => {
                            out.insert(n.to_string());
                        }
                        (None, ModuleStyle::Dotted) => {
                            out.insert(a.name.split('.').next().unwrap_or("").to_string());
                        }
                        (None, ModuleStyle::Path) => {}
                    }
                }
            }
            Stmt::ImportFrom(x) => {
                for a in &x.names {
                    out.insert(a.asname.as_ref().unwrap_or(&a.name).to_string());
                }
            }
            Stmt::Global(x) => x.names.iter().for_each(|n| {
                globals.insert(n.to_string());
            }),
            Stmt::Nonlocal(x) => x.names.iter().for_each(|n| {
                globals.insert(n.to_string());
            }),
            _ => {}
        }
        // Walrus targets bind in the enclosing scope.
        stmt_expr(s, false, &mut |e| {
            walk(e, &mut |x| {
                if let Expr::Named(n) = x {
                    target_names(&n.target, &mut targets);
                }
            })
        });
        out.extend(targets);
    }
}
