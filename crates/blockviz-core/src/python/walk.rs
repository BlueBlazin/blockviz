//! Borrowing traversals over the Ruff Python AST.

use ruff_python_ast::{self as ast, Expr, InterpolatedStringElement, Stmt};
use std::collections::BTreeSet;

fn interpolations<'a>(elements: &'a ast::InterpolatedStringElements, out: &mut Vec<&'a Expr>) {
    for el in elements.iter() {
        if let InterpolatedStringElement::Interpolation(i) = el {
            out.push(&i.expression);
            if let Some(spec) = &i.format_spec {
                interpolations(&spec.elements, out);
            }
        }
    }
}

/// Immediate sub-expressions of an expression.
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
            if let Some(p) = &x.parameters {
                for a in p.posonlyargs.iter().chain(&p.args).chain(&p.kwonlyargs) {
                    if let Some(d) = &a.default {
                        out.push(d);
                    }
                }
            }
            out.push(&x.body)
        }
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
        Expr::Set(x) => out.extend(&x.elts),
        Expr::ListComp(x) => comprehension(&x.elt, None, &x.generators, out),
        Expr::SetComp(x) => comprehension(&x.elt, None, &x.generators, out),
        Expr::Generator(x) => comprehension(&x.elt, None, &x.generators, out),
        Expr::DictComp(x) => {
            // `{**d for d in ds}` (PEP 798) has no key.
            match &x.key {
                Some(k) => comprehension(k, Some(&x.value), &x.generators, out),
                None => comprehension(&x.value, None, &x.generators, out),
            }
        }
        Expr::Await(x) => out.push(&x.value),
        Expr::Yield(x) => out.extend(x.value.as_deref()),
        Expr::YieldFrom(x) => out.push(&x.value),
        Expr::Compare(x) => out.extend(x.operands.iter()),
        Expr::Call(x) => {
            out.push(&x.func);
            out.extend(&x.arguments.args);
            out.extend(x.arguments.keywords.iter().map(|k| &k.value))
        }
        Expr::FString(x) => {
            for part in x.value.iter() {
                if let ast::FStringPartRef::FString(f) = part {
                    interpolations(&f.elements, out);
                }
            }
        }
        Expr::TString(x) => {
            for t in x.value.iter() {
                interpolations(&t.elements, out);
            }
        }
        Expr::Attribute(x) => out.push(&x.value),
        Expr::Subscript(x) => {
            out.push(&x.value);
            out.push(&x.slice)
        }
        Expr::Starred(x) => out.push(&x.value),
        Expr::List(x) => out.extend(&x.elts),
        Expr::Tuple(x) => out.extend(&x.elts),
        Expr::Slice(x) => {
            out.extend(x.lower.as_deref());
            out.extend(x.upper.as_deref());
            out.extend(x.step.as_deref())
        }
        Expr::StringLiteral(_)
        | Expr::BytesLiteral(_)
        | Expr::NumberLiteral(_)
        | Expr::BooleanLiteral(_)
        | Expr::NoneLiteral(_)
        | Expr::EllipsisLiteral(_)
        | Expr::Name(_)
        | Expr::IpyEscapeCommand(_) => {}
    }
}

fn comprehension<'a>(elt: &'a Expr, value: Option<&'a Expr>, gens: &'a [ast::Comprehension], out: &mut Vec<&'a Expr>) {
    out.push(elt);
    out.extend(value);
    for g in gens {
        out.push(&g.target);
        out.push(&g.iter);
        out.extend(&g.ifs);
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

/// Names bound inside comprehensions and lambdas are local to them.
fn local_binders(e: &Expr, out: &mut BTreeSet<String>) {
    match e {
        Expr::ListComp(x) => gens_binders(&x.generators, out),
        Expr::SetComp(x) => gens_binders(&x.generators, out),
        Expr::Generator(x) => gens_binders(&x.generators, out),
        Expr::DictComp(x) => gens_binders(&x.generators, out),
        Expr::Lambda(x) => {
            if let Some(p) = &x.parameters {
                for name in param_names(p) {
                    out.insert(name);
                }
            }
        }
        _ => {}
    }
}

fn gens_binders(gens: &[ast::Comprehension], out: &mut BTreeSet<String>) {
    for g in gens {
        walk(&g.target, &mut |t| {
            if let Expr::Name(n) = t {
                out.insert(n.id.to_string());
            }
        });
    }
}

/// All parameter names, in declaration order.
pub fn param_names(p: &ast::Parameters) -> Vec<String> {
    let mut out: Vec<String> = p.posonlyargs.iter().chain(&p.args).map(|a| a.parameter.name.to_string()).collect();
    if let Some(v) = &p.vararg {
        out.push(v.name.to_string());
    }
    out.extend(p.kwonlyargs.iter().map(|a| a.parameter.name.to_string()));
    if let Some(k) = &p.kwarg {
        out.push(k.name.to_string());
    }
    out
}

/// Free names loaded by an expression, in first-use order.
pub fn loads(e: &Expr) -> Vec<String> {
    let mut bound = BTreeSet::new();
    walk(e, &mut |x| local_binders(x, &mut bound));
    let mut out: Vec<String> = vec![];
    walk(e, &mut |x| {
        if let Expr::Name(n) = x {
            if n.ctx == ast::ExprContext::Load && !bound.contains(n.id.as_str()) && !out.iter().any(|o| o == n.id.as_str()) {
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
        Expr::Tuple(x) => x.elts.iter().for_each(|e| target_names(e, out)),
        Expr::List(x) => x.elts.iter().for_each(|e| target_names(e, out)),
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
            x.decorator_list.iter().for_each(|d| f(&d.expression));
            for a in x.parameters.posonlyargs.iter().chain(&x.parameters.args).chain(&x.parameters.kwonlyargs) {
                if let Some(d) = &a.default {
                    f(d);
                }
            }
            if deep {
                stmt_exprs(&x.body, deep, f);
            }
        }
        Stmt::ClassDef(x) => {
            if let Some(args) = &x.arguments {
                args.args.iter().for_each(&mut *f);
                args.keywords.iter().for_each(|k| f(&k.value));
            }
            x.decorator_list.iter().for_each(|d| f(&d.expression));
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
            for ast::ExceptHandler::ExceptHandler(h) in &x.handlers {
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
        Stmt::Import(_) | Stmt::ImportFrom(_) | Stmt::Global(_) | Stmt::Nonlocal(_) | Stmt::Pass(_) | Stmt::Break(_) | Stmt::Continue(_) | Stmt::IpyEscapeCommand(_) => {}
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
                for ast::ExceptHandler::ExceptHandler(h) in &x.handlers {
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
                    out.insert(a.asname.as_ref().map(|n| n.to_string()).unwrap_or_else(|| a.name.split('.').next().unwrap_or("").to_string()));
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
