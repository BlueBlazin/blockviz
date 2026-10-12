//! Borrowing traversals over the shared syntax tree.

use super::syntax::*;
use crate::hash::Set;
use std::collections::BTreeSet;

/// Immediate sub-expressions of an expression. A function expression's body
/// is a scope of its own and is not included (see `loads_deep`).
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
    // One stack for the whole walk: children go on reversed, to come off in order.
    let mut stack = vec![e];
    while let Some(e) = stack.pop() {
        f(e);
        let at = stack.len();
        children(e, &mut stack);
        stack[at..].reverse();
    }
}

/// Names bound inside comprehensions and lambdas are local to them.
fn local_binders<'a>(e: &'a Expr, out: &mut Set<&'a str>) {
    match e {
        Expr::Comp(x) => {
            for g in &x.generators {
                walk(&g.target, &mut |t| {
                    if let Expr::Name(n) = t {
                        out.insert(&n.id);
                    }
                });
            }
        }
        Expr::Lambda(x) => {
            for p in &x.params {
                out.insert(&p.name);
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
pub fn loads(e: &Expr) -> Vec<&str> {
    scan(e, &mut vec![])
}

/// `loads`, and the function expressions anywhere in `e` (callbacks,
/// handlers), in one walk.
fn scan<'a>(e: &'a Expr, fns: &mut Vec<&'a FunctionDef>) -> Vec<&'a str> {
    let mut bound = Set::default();
    let mut seen = Set::default();
    let mut out = vec![];
    walk(e, &mut |x| match x {
        Expr::Name(n) => {
            if n.ctx == Ctx::Load && seen.insert(n.id.as_str()) {
                out.push(n.id.as_str());
            }
        }
        Expr::Function(f) => fns.push(f),
        _ => local_binders(x, &mut bound),
    });
    out.retain(|n| !bound.contains(n));
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

/// What a statement is made of, in source order.
pub enum Part<'a> {
    Expr(&'a Expr),
    /// A statement list in it, and whether it is a function's or class's body.
    Body(&'a [Stmt], bool),
}

/// Visits every expression rooted in a statement list, descending into nested
/// statements; nested function and class bodies are included when `deep`.
pub fn stmt_exprs<'a>(stmts: &'a [Stmt], deep: bool, f: &mut impl FnMut(&'a Expr)) {
    for s in stmts {
        stmt_expr(s, deep, f);
    }
}

pub fn stmt_expr<'a>(s: &'a Stmt, deep: bool, f: &mut impl FnMut(&'a Expr)) {
    parts(s, &mut |p| match p {
        Part::Expr(e) => f(e),
        Part::Body(body, def) => {
            if deep || !def {
                stmt_exprs(body, deep, f)
            }
        }
    });
}

/// A statement's own expressions and the statement lists nested in it.
pub fn parts<'a>(s: &'a Stmt, f: &mut impl FnMut(Part<'a>)) {
    use Part::{Body, Expr as E};
    match s {
        Stmt::FunctionDef(x) => {
            x.decorators.iter().for_each(|d| f(E(&d.expression)));
            for a in &x.params {
                if let Some(d) = &a.default {
                    f(E(d));
                }
            }
            f(Body(&x.body, true))
        }
        Stmt::ClassDef(x) => {
            x.bases.iter().for_each(|b| f(E(b)));
            x.keywords.iter().for_each(|k| f(E(&k.value)));
            x.decorators.iter().for_each(|d| f(E(&d.expression)));
            f(Body(&x.body, true))
        }
        Stmt::Return(x) => {
            if let Some(v) = &x.value {
                f(E(v))
            }
        }
        Stmt::Delete(x) => x.targets.iter().for_each(|t| f(E(t))),
        Stmt::TypeAlias(x) => {
            f(E(&x.name));
            f(E(&x.value))
        }
        Stmt::Assign(x) => {
            x.targets.iter().for_each(|t| f(E(t)));
            f(E(&x.value))
        }
        Stmt::AugAssign(x) => {
            f(E(&x.target));
            f(E(&x.value))
        }
        Stmt::AnnAssign(x) => {
            f(E(&x.target));
            if let Some(v) = &x.value {
                f(E(v))
            }
        }
        Stmt::For(x) => {
            f(E(&x.target));
            f(E(&x.iter));
            f(Body(&x.body, false));
            f(Body(&x.orelse, false))
        }
        Stmt::While(x) => {
            f(E(&x.test));
            f(Body(&x.body, false));
            f(Body(&x.orelse, false))
        }
        Stmt::If(x) => {
            f(E(&x.test));
            f(Body(&x.body, false));
            for c in &x.elif_else_clauses {
                if let Some(t) = &c.test {
                    f(E(t));
                }
                f(Body(&c.body, false));
            }
        }
        Stmt::With(x) => {
            for i in &x.items {
                f(E(&i.context_expr));
                if let Some(v) = &i.optional_vars {
                    f(E(v));
                }
            }
            f(Body(&x.body, false))
        }
        Stmt::Match(x) => {
            f(E(&x.subject));
            for c in &x.cases {
                if let Some(g) = &c.guard {
                    f(E(g));
                }
                f(Body(&c.body, false));
            }
        }
        Stmt::Raise(x) => {
            if let Some(v) = &x.exc {
                f(E(v))
            }
            if let Some(v) = &x.cause {
                f(E(v))
            }
        }
        Stmt::Try(x) => {
            f(Body(&x.body, false));
            for h in &x.handlers {
                if let Some(t) = &h.type_ {
                    f(E(t));
                }
                f(Body(&h.body, false));
            }
            f(Body(&x.orelse, false));
            f(Body(&x.finalbody, false))
        }
        Stmt::Assert(x) => {
            f(E(&x.test));
            if let Some(m) = &x.msg {
                f(E(m))
            }
        }
        Stmt::Expr(x) => f(E(&x.value)),
        Stmt::Import(_) | Stmt::ImportFrom(_) | Stmt::Global(_) | Stmt::Nonlocal(_) | Stmt::Pass(_) | Stmt::Break(_) | Stmt::Continue(_) | Stmt::Export(_) | Stmt::Other(_) => {}
    }
}

/// Free names read by a function-like body: loads minus parameters and local bindings.
pub fn free_names<'a>(params: &[String], body: &'a [Stmt]) -> Vec<&'a str> {
    let mut local: BTreeSet<String> = params.iter().cloned().collect();
    let mut declared_global: BTreeSet<String> = BTreeSet::new();
    collect_bindings(body, &mut local, &mut declared_global);
    for g in &declared_global {
        local.remove(g);
    }
    let mut seen = Set::default();
    let mut out = vec![];
    stmt_exprs(body, true, &mut |e| {
        out.extend(loads_deep(e).into_iter().filter(|n| !local.contains(*n) && seen.insert(*n)));
    });
    out
}

/// Free names an expression reads, including inside the functions it
/// contains (a callback reads what it closes over when it runs).
pub fn loads_deep(e: &Expr) -> Vec<&str> {
    let mut fns = vec![];
    let mut out = scan(e, &mut fns);
    if !fns.is_empty() {
        let mut seen: Set<&str> = out.iter().copied().collect();
        for f in fns {
            out.extend(free_names(&param_names(&f.params), &f.body).into_iter().filter(|n| seen.insert(*n)));
        }
    }
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
