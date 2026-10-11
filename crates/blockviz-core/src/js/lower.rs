//! oxc's JavaScript/TypeScript tree → the shared syntax tree.
//!
//! Most constructs have a direct counterpart. The rest are lowered to the
//! shape a reader means: `const f = () => {…}` defines a function; JSX
//! elements are calls of their component with props as keywords; destructured
//! parameters are the parameters they bind (`({ user, onSave })` takes `user`
//! and `onSave`); a method's receiver is an implicit `this` parameter;
//! `require("x")` is an import; `switch` is a match whose fall-through cases
//! share a body; `module.exports = x` is an export.

use crate::code::syntax::*;
use oxc_ast::ast as js;
use oxc_span::{GetSpan, Span};

mod objects;

pub struct Lower<'s> {
    pub text: &'s str,
    /// `require("x")` and `import("x")` met inside expressions: (specifier, span).
    pub required: std::cell::RefCell<Vec<(String, Span)>>,
    /// The file's stem, naming an anonymous default export.
    pub stem: String,
    /// Documentation comments (`/** … */`): (start, end, text), by position.
    pub docs: Vec<(u32, u32, String)>,
    /// Resolves an import specifier to a project file.
    pub resolve: &'s dyn Fn(&str) -> Option<String>,
    /// The documentation of a function's overload signatures, for its
    /// implementation: (name, doc).
    pub overload_doc: std::cell::RefCell<Option<(String, String)>>,
}

/// A TypeScript parameter property (`constructor(private users: UsersService)`):
/// the field, its annotation and where it is declared.
type ParamProperty = (String, Option<Expr>, Span);

fn r(s: Span) -> TextRange {
    TextRange::new(TextSize::from(s.start), TextSize::from(s.end))
}

/// An empty range at the end of `s`: for records (exports) that must not share
/// a start offset with the definition they describe.
fn end(s: Span) -> TextRange {
    TextRange::empty(TextSize::from(s.end))
}

fn atom(s: Span) -> Atom {
    Atom { range: r(s) }
}

fn name_expr(id: &str, s: Span, ctx: Ctx) -> Expr {
    Expr::Name(ExprName { id: id.to_string(), ctx, range: r(s) })
}

fn string(value: &str, s: Span) -> Expr {
    Expr::StringLiteral(ExprStringLiteral { value: value.to_string(), range: r(s) })
}

fn unparen<'b, 'a>(e: &'b js::Expression<'a>) -> &'b js::Expression<'a> {
    match e {
        js::Expression::ParenthesizedExpression(p) => unparen(&p.expression),
        js::Expression::TSAsExpression(x) => unparen(&x.expression),
        js::Expression::TSSatisfiesExpression(x) => unparen(&x.expression),
        js::Expression::TSNonNullExpression(x) => unparen(&x.expression),
        js::Expression::TSTypeAssertion(x) => unparen(&x.expression),
        js::Expression::TSInstantiationExpression(x) => unparen(&x.expression),
        e => e,
    }
}

/// `require("x")` → `x`.
fn require_spec<'a>(e: &js::Expression<'a>) -> Option<&'a str> {
    let js::Expression::CallExpression(c) = unparen(e) else { return None };
    require_call(c)
}

fn require_call<'a>(c: &js::CallExpression<'a>) -> Option<&'a str> {
    let js::Expression::Identifier(f) = &c.callee else { return None };
    if f.name.as_str() != "require" || c.arguments.len() != 1 {
        return None;
    }
    match c.arguments[0].as_expression() {
        Some(js::Expression::StringLiteral(s)) => Some(s.value.as_str()),
        _ => None,
    }
}

/// Functions that wrap a function and return one like it, the way a
/// decorator does: `catchAsync(async (req, res) => …)`, `memo(Card)`,
/// `useCallback(() => …, deps)`. A variable holding the result is a function.
fn is_wrapper(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(name, "memo" | "forwardRef" | "observer" | "useCallback" | "useEvent" | "useEffectEvent" | "debounce" | "throttle" | "once" | "memoize" | "cache" | "defineEventHandler" | "eventHandler" | "defineHandler" | "action" | "authorized" | "protect")
        || lower.ends_with("handler")
        || lower.starts_with("catch")
        || lower.starts_with("wrap")
        || lower.starts_with("with")
        || lower.starts_with("asynchandler")
        || lower.starts_with("tryc")
}

/// `wrapper(fn, …)` where the wrapper returns a function like `fn`: the wrapper
/// and the function.
fn wrapped<'b, 'a>(e: &'b js::Expression<'a>) -> Option<(&'b js::Expression<'a>, &'b js::Expression<'a>)> {
    let js::Expression::CallExpression(c) = unparen(e) else { return None };
    let name = match &c.callee {
        js::Expression::Identifier(i) => i.name.as_str(),
        js::Expression::StaticMemberExpression(m) => m.property.name.as_str(),
        _ => return None,
    };
    if !is_wrapper(name) {
        return None;
    }
    let mut fns = c.arguments.iter().filter_map(|a| a.as_expression()).filter(|a| matches!(unparen(a), js::Expression::ArrowFunctionExpression(_) | js::Expression::FunctionExpression(_)));
    match (fns.next(), fns.next()) {
        (Some(f), None) => Some((&c.callee, unparen(f))),
        _ => None,
    }
}

/// A function called where it is written: `(function () {…})()`,
/// `(() => {…})()`, `(function () {…}).call(this)`.
fn is_iife_call(c: &js::CallExpression) -> bool {
    let callee = match unparen(&c.callee) {
        js::Expression::StaticMemberExpression(m) if m.property.name.as_str() == "call" => unparen(&m.object),
        e => e,
    };
    matches!(callee, js::Expression::FunctionExpression(_) | js::Expression::ArrowFunctionExpression(_))
}

fn is_iife(e: &js::Expression) -> bool {
    matches!(unparen(e), js::Expression::CallExpression(c) if is_iife_call(c))
}

/// `a = b = value` → the targets `[a, b]` (outermost first) and `value`.
fn chain<'b, 'a>(e: &'b js::Expression<'a>) -> (Vec<&'b js::AssignmentTarget<'a>>, &'b js::Expression<'a>) {
    let mut targets = vec![];
    let mut cur = e;
    while let js::Expression::AssignmentExpression(a) = unparen(cur) {
        if a.operator != oxc_syntax::operator::AssignmentOperator::Assign {
            break;
        }
        targets.push(&a.left);
        cur = &a.right;
    }
    (targets, cur)
}

/// The target `module.exports` itself.
fn is_exports_expr(t: &js::AssignmentTarget) -> bool {
    t.as_member_expression().is_some_and(|m| m.static_property_name().is_some_and(|p| p == "exports") && matches!(m.object(), js::Expression::Identifier(i) if i.name.as_str() == "module"))
}

/// `exports` or `module.exports`.
fn is_exports(e: &js::Expression) -> bool {
    match e {
        js::Expression::Identifier(i) => i.name.as_str() == "exports",
        js::Expression::StaticMemberExpression(m) => m.property.name.as_str() == "exports" && matches!(&m.object, js::Expression::Identifier(i) if i.name.as_str() == "module"),
        _ => false,
    }
}

fn export_name(n: &js::ModuleExportName) -> String {
    match n {
        js::ModuleExportName::IdentifierName(i) => i.name.to_string(),
        js::ModuleExportName::IdentifierReference(i) => i.name.to_string(),
        js::ModuleExportName::StringLiteral(s) => s.value.to_string(),
    }
}

/// Names bound by a binding pattern, in order.
fn pattern_names(p: &js::BindingPattern, out: &mut Vec<(String, Span)>) {
    match p {
        js::BindingPattern::BindingIdentifier(i) => out.push((i.name.to_string(), i.span)),
        js::BindingPattern::ObjectPattern(o) => {
            for prop in &o.properties {
                pattern_names(&prop.value, out);
            }
            if let Some(rest) = &o.rest {
                pattern_names(&rest.argument, out);
            }
        }
        js::BindingPattern::ArrayPattern(a) => {
            for el in a.elements.iter().flatten() {
                pattern_names(el, out);
            }
            if let Some(rest) = &a.rest {
                pattern_names(&rest.argument, out);
            }
        }
        js::BindingPattern::AssignmentPattern(a) => pattern_names(&a.left, out),
    }
}

impl<'s> Lower<'s> {
    // ----- statements ---------------------------------------------------------

    pub fn program(&self, p: &js::Program) -> Vec<Stmt> {
        // Objects and constructor functions that own functions become classes.
        let owners = objects::Owners::scan(&p.body);
        let mut out = vec![];
        for s in &p.body {
            if owners.folds(s) {
                continue;
            }
            let lowered = self.owner_definition(s, &owners).unwrap_or_else(|| self.stmt(s));
            out.extend(lowered);
            self.required_imports(&mut out);
        }
        out
    }

    fn stmts(&self, list: &[js::Statement]) -> Vec<Stmt> {
        let mut out = vec![];
        for s in list {
            out.extend(self.stmt(s));
            self.required_imports(&mut out);
        }
        out
    }

    /// `require()`/`import()` inside an expression still depends on the module.
    fn required_imports(&self, out: &mut Vec<Stmt>) {
        for (spec, span) in self.required.borrow_mut().drain(..) {
            out.push(Stmt::Import(StmtImport { names: vec![Alias { name: spec.clone(), asname: None, range: end(span) }], style: ModuleStyle::Path, resolved: (self.resolve)(&spec), declaration: false, range: end(span) }));
        }
    }

    fn body_of(&self, s: &js::Statement) -> Vec<Stmt> {
        match s {
            js::Statement::BlockStatement(b) => self.stmts(&b.body),
            other => self.stmt(other),
        }
    }

    fn doc_before(&self, start: u32) -> Option<String> {
        let i = self.docs.partition_point(|d| d.1 <= start);
        let (_, end, text) = self.docs.get(i.checked_sub(1)?)?;
        let between = self.text.get(*end as usize..start as usize)?;
        (between.trim().is_empty() && !text.is_empty()).then(|| text.clone())
    }

    fn stmt(&self, s: &js::Statement) -> Vec<Stmt> {
        let span = s.span();
        match s {
            js::Statement::BlockStatement(b) => self.stmts(&b.body),
            js::Statement::BreakStatement(x) => vec![Stmt::Break(atom(x.span))],
            js::Statement::ContinueStatement(x) => vec![Stmt::Continue(atom(x.span))],
            js::Statement::DebuggerStatement(_) | js::Statement::EmptyStatement(_) => vec![],
            js::Statement::DoWhileStatement(x) => vec![Stmt::While(StmtWhile { test: Box::new(self.expr(&x.test)), body: self.body_of(&x.body), orelse: vec![], range: r(x.span) })],
            js::Statement::WhileStatement(x) => vec![Stmt::While(StmtWhile { test: Box::new(self.expr(&x.test)), body: self.body_of(&x.body), orelse: vec![], range: r(x.span) })],
            js::Statement::ExpressionStatement(x) => self.expr_stmt(&x.expression, x.span),
            js::Statement::ForInStatement(x) => vec![Stmt::For(StmtFor { target: Box::new(self.for_left(&x.left)), iter: Box::new(self.expr(&x.right)), body: self.body_of(&x.body), orelse: vec![], kind: ForKind::Keys, is_async: false, range: r(x.span) })],
            js::Statement::ForOfStatement(x) => vec![Stmt::For(StmtFor { target: Box::new(self.for_left(&x.left)), iter: Box::new(self.expr(&x.right)), body: self.body_of(&x.body), orelse: vec![], kind: ForKind::Of, is_async: x.r#await, range: r(x.span) })],
            js::Statement::ForStatement(x) => self.for_count(x),
            js::Statement::IfStatement(x) => vec![Stmt::If(StmtIf { test: Box::new(self.expr(&x.test)), body: self.body_of(&x.consequent), elif_else_clauses: self.clauses(x.alternate.as_ref()), range: r(x.span) })],
            js::Statement::LabeledStatement(x) => self.stmt(&x.body),
            js::Statement::ReturnStatement(x) => vec![Stmt::Return(StmtReturn { value: x.argument.as_ref().map(|a| Box::new(self.expr(a))), range: r(x.span) })],
            js::Statement::SwitchStatement(x) => vec![self.switch(x)],
            js::Statement::ThrowStatement(x) => vec![Stmt::Raise(StmtRaise { exc: Some(Box::new(self.expr(&x.argument))), cause: None, range: r(x.span) })],
            js::Statement::TryStatement(x) => vec![Stmt::Try(StmtTry {
                body: self.stmts(&x.block.body),
                handlers: x
                    .handler
                    .iter()
                    .map(|h| {
                        let mut names = vec![];
                        if let Some(p) = &h.param {
                            pattern_names(&p.pattern, &mut names);
                        }
                        ExceptHandler { type_: None, name: names.first().map(|n| n.0.clone()), body: self.stmts(&h.body.body), range: r(h.span) }
                    })
                    .collect(),
                orelse: vec![],
                finalbody: x.finalizer.as_ref().map(|f| self.stmts(&f.body)).unwrap_or_default(),
                range: r(x.span),
            })],
            js::Statement::WithStatement(x) => vec![Stmt::Other(atom(x.span))],
            js::Statement::VariableDeclaration(d) => self.var_decl(d, span),
            js::Statement::FunctionDeclaration(f) => self.function_decl(f, span),
            js::Statement::ClassDeclaration(c) => vec![Stmt::ClassDef(self.class(c, None, span))],
            js::Statement::TSTypeAliasDeclaration(t) => self.type_alias(t),
            js::Statement::TSInterfaceDeclaration(i) => vec![Stmt::ClassDef(self.interface(i, span))],
            js::Statement::TSEnumDeclaration(e) => vec![Stmt::ClassDef(self.enumeration(e, span))],
            js::Statement::TSImportEqualsDeclaration(x) => match &x.module_reference {
                js::TSModuleReference::ExternalModuleReference(m) => {
                    let spec = m.expression.value.as_str();
                    vec![Stmt::Import(StmtImport { names: vec![Alias { name: spec.into(), asname: Some(x.id.name.to_string()), range: r(x.span) }], style: ModuleStyle::Path, resolved: (self.resolve)(spec), declaration: false, range: r(x.span) })]
                }
                _ => vec![],
            },
            js::Statement::TSNamespaceDeclaration(n) => self.namespace(n, span),
            // Ambient declarations describe code that lives elsewhere.
            js::Statement::TSExternalModuleDeclaration(_) | js::Statement::TSGlobalDeclaration(_) => vec![],
            js::Statement::ImportDeclaration(x) => self.import(x),
            js::Statement::ExportAllDeclaration(x) => {
                let spec = x.source.value.as_str();
                let resolved = (self.resolve)(spec);
                match &x.exported {
                    Some(ns) => {
                        let ns = export_name(ns);
                        vec![
                            Stmt::Import(StmtImport { names: vec![Alias { name: spec.into(), asname: Some(ns.clone()), range: r(x.span) }], style: ModuleStyle::Path, resolved, declaration: false, range: r(x.span) }),
                            Stmt::Export(StmtExport { names: vec![(ns.clone(), ns)], range: end(x.span) }),
                        ]
                    }
                    None => vec![Stmt::ImportFrom(StmtImportFrom { module: Some(spec.into()), names: vec![Alias { name: "*".into(), asname: None, range: r(x.span) }], level: 0, style: ModuleStyle::Path, resolved, range: r(x.span) })],
                }
            }
            js::Statement::ExportDefaultDeclaration(x) => self.export_default(x),
            js::Statement::ExportDeclaration(x) => {
                let mut out = self.decl(&x.declaration, x.span);
                let mut names = vec![];
                for s in &out {
                    match s {
                        Stmt::FunctionDef(f) => names.push(f.name.clone()),
                        Stmt::ClassDef(c) => names.push(c.name.clone()),
                        Stmt::Assign(a) => a.targets.iter().for_each(|t| crate::code::walk::target_names(t, &mut names)),
                        Stmt::AnnAssign(a) => crate::code::walk::target_names(&a.target, &mut names),
                        Stmt::TypeAlias(t) => crate::code::walk::target_names(&t.name, &mut names),
                        _ => {}
                    }
                }
                out.push(Stmt::Export(StmtExport { names: names.into_iter().map(|n| (n.clone(), n)).collect(), range: end(x.span) }));
                out
            }
            js::Statement::ExportNamedDeclaration(x) => vec![Stmt::Export(StmtExport { names: x.specifiers.iter().map(|s| (export_name(&s.local), export_name(&s.exported))).collect(), range: end(x.span) })],
            js::Statement::ExportFromDeclaration(x) => {
                let spec = x.source.value.as_str();
                let names: Vec<Alias> = x.specifiers.iter().map(|s| Alias { name: export_name(&s.local), asname: Some(export_name(&s.exported)), range: r(s.span) }).collect();
                let exported = names.iter().map(|a| (a.asname.clone().unwrap(), a.asname.clone().unwrap())).collect();
                vec![Stmt::ImportFrom(StmtImportFrom { module: Some(spec.into()), names, level: 0, style: ModuleStyle::Path, resolved: (self.resolve)(spec), range: r(x.span) }), Stmt::Export(StmtExport { names: exported, range: end(x.span) })]
            }
            js::Statement::TSExportAssignment(x) => match unparen(&x.expression) {
                js::Expression::Identifier(i) => vec![Stmt::Export(StmtExport { names: vec![(i.name.to_string(), "default".into())], range: end(x.span) })],
                e => vec![Stmt::Expr(StmtExpr { value: Box::new(self.expr(e)), range: r(x.span) })],
            },
            js::Statement::TSNamespaceExportDeclaration(_) => vec![],
        }
    }

    fn decl(&self, d: &js::Declaration, span: Span) -> Vec<Stmt> {
        match d {
            js::Declaration::VariableDeclaration(v) => self.var_decl(v, span),
            js::Declaration::FunctionDeclaration(f) => self.function_decl(f, span),
            js::Declaration::ClassDeclaration(c) => vec![Stmt::ClassDef(self.class(c, None, span))],
            js::Declaration::TSTypeAliasDeclaration(t) => self.type_alias(t),
            js::Declaration::TSInterfaceDeclaration(i) => vec![Stmt::ClassDef(self.interface(i, span))],
            js::Declaration::TSEnumDeclaration(e) => vec![Stmt::ClassDef(self.enumeration(e, span))],
            js::Declaration::TSNamespaceDeclaration(n) => self.namespace(n, span),
            _ => vec![],
        }
    }

    fn clauses(&self, alt: Option<&js::Statement>) -> Vec<ElifElseClause> {
        match alt {
            None => vec![],
            Some(js::Statement::IfStatement(y)) => {
                let mut out = vec![ElifElseClause { test: Some(self.expr(&y.test)), body: self.body_of(&y.consequent), range: r(y.span) }];
                out.extend(self.clauses(y.alternate.as_ref()));
                out
            }
            Some(s) => vec![ElifElseClause { test: None, body: self.body_of(s), range: r(s.span()) }],
        }
    }

    fn for_left(&self, left: &js::ForStatementLeft) -> Expr {
        match left {
            js::ForStatementLeft::VariableDeclaration(d) => match d.declarations.first() {
                Some(v) => self.pattern(&v.id),
                None => Expr::Other(atom(d.span)),
            },
            other => match other.as_assignment_target() {
                Some(t) => self.target(t),
                None => Expr::Other(atom(other.span())),
            },
        }
    }

    /// `for (let i = 0; i < n; i++)`: the counter and its condition; the
    /// update is loop plumbing.
    fn for_count(&self, x: &js::ForStatement) -> Vec<Stmt> {
        let mut out = vec![];
        let mut counter = None;
        match &x.init {
            Some(js::ForStatementInit::VariableDeclaration(d)) => {
                if let Some(v) = d.declarations.first() {
                    counter = Some(self.pattern(&v.id));
                }
                out.extend(self.var_decl(d, d.span));
            }
            Some(init) => {
                if let Some(e) = init.as_expression() {
                    out.extend(self.expr_stmt(e, e.span()));
                }
            }
            None => {}
        }
        let body = self.body_of(&x.body);
        match (&x.test, counter) {
            (Some(test), Some(counter)) => out.push(Stmt::For(StmtFor { target: Box::new(counter), iter: Box::new(self.expr(test)), body, orelse: vec![], kind: ForKind::Count, is_async: false, range: r(x.span) })),
            (test, _) => {
                let test = match test {
                    Some(t) => self.expr(t),
                    None => Expr::BooleanLiteral(ExprBooleanLiteral { value: true, range: r(Span::new(x.span.start, x.span.start)) }),
                };
                out.push(Stmt::While(StmtWhile { test: Box::new(test), body, orelse: vec![], range: r(x.span) }));
            }
        }
        out
    }

    /// Cases without a body fall through to the next: they share its lane.
    /// A case's closing `break` leaves the switch, not a loop.
    fn switch(&self, x: &js::SwitchStatement) -> Stmt {
        let mut cases = vec![];
        let mut patterns: Vec<TextRange> = vec![];
        let mut start: Option<u32> = None;
        for c in &x.cases {
            start.get_or_insert(c.span.start);
            if let Some(t) = &c.test {
                patterns.push(r(t.span()));
            }
            if c.consequent.is_empty() {
                continue;
            }
            let mut body = self.stmts(&c.consequent);
            if matches!(body.last(), Some(Stmt::Break(_))) {
                body.pop();
            }
            // `default` shares the lane of any cases that fall into it.
            let patterns = if c.test.is_none() { vec![] } else { std::mem::take(&mut patterns) };
            cases.push(MatchCase { patterns, binds: None, variant: None, guard: None, body, range: r(Span::new(start.take().unwrap_or(c.span.start), c.span.end)) });
        }
        Stmt::Match(StmtMatch { subject: Box::new(self.expr(&x.discriminant)), cases, range: r(x.span) })
    }

    fn var_decl(&self, d: &js::VariableDeclaration, span: Span) -> Vec<Stmt> {
        if d.declare {
            return vec![];
        }
        let mut out = vec![];
        for v in &d.declarations {
            // A single declarator speaks for the whole statement (and its comments).
            let span = if d.declarations.len() == 1 { span } else { v.span };
            let Some(init) = &v.init else {
                if let Some(t) = &v.type_annotation {
                    out.push(Stmt::AnnAssign(StmtAnnAssign { target: Box::new(self.pattern(&v.id)), annotation: Box::new(self.type_expr(&t.type_annotation)), value: None, range: r(span) }));
                }
                continue;
            };
            // `var app = exports = module.exports = express()`: the variable holds
            // the value; the other targets receive it too.
            let (targets, init) = chain(init);
            out.extend(self.declarator(v, init, span));
            if let js::BindingPattern::BindingIdentifier(id) = &v.id {
                out.extend(self.also_assigned(&targets, id.name.as_str(), span));
            }
        }
        out
    }

    /// The other targets of an assignment chain, given the variable that holds
    /// its value: `module.exports` exports it, `exports.x` exports it as `x`,
    /// anything else is assigned it.
    fn also_assigned(&self, targets: &[&js::AssignmentTarget], name: &str, span: Span) -> Vec<Stmt> {
        let mut out = vec![];
        for t in targets {
            if let Some(m) = t.as_member_expression() {
                if is_exports(m.object()) || is_exports_expr(t) {
                    let exported = if is_exports_expr(t) { "default".to_string() } else { m.static_property_name().map(|p| p.to_string()).unwrap_or_else(|| "default".into()) };
                    out.push(Stmt::Export(StmtExport { names: vec![(name.to_string(), exported)], range: end(span) }));
                    continue;
                }
            }
            // `exports = …` only re-points the module's `exports` alias.
            if matches!(t, js::AssignmentTarget::AssignmentTargetIdentifier(i) if i.name.as_str() == "exports") {
                continue;
            }
            out.push(Stmt::Assign(StmtAssign { targets: vec![self.target(t)], value: Box::new(name_expr(name, Span::new(span.end, span.end), Ctx::Load)), range: end(span) }));
        }
        out
    }

    /// One declarator, given its value.
    fn declarator(&self, v: &js::VariableDeclarator, init: &js::Expression, span: Span) -> Vec<Stmt> {
        let mut out = vec![];
        if let js::BindingPattern::BindingIdentifier(id) = &v.id {
            let name = id.name.as_str();
            match unparen(init) {
                js::Expression::ArrowFunctionExpression(a) => {
                    out.push(Stmt::FunctionDef(self.arrow(a, name, span, DefKind::Function, false)));
                    return out;
                }
                js::Expression::FunctionExpression(f) => {
                    out.push(Stmt::FunctionDef(self.function(f, name, span, DefKind::Function, false, &[])));
                    return out;
                }
                js::Expression::ClassExpression(c) => {
                    out.push(Stmt::ClassDef(self.class(c, Some(name), span)));
                    return out;
                }
                e if wrapped(e).is_some() => {
                    let (wrapper, f) = wrapped(e).unwrap();
                    out.push(Stmt::FunctionDef(self.wrapped_function(wrapper, f, name, span)));
                    return out;
                }
                js::Expression::StaticMemberExpression(m) if require_spec(&m.object).is_some() => {
                    let spec = require_spec(&m.object).unwrap();
                    out.push(Stmt::ImportFrom(StmtImportFrom {
                        module: Some(spec.into()),
                        names: vec![Alias { name: m.property.name.to_string(), asname: Some(name.into()), range: r(v.span) }],
                        level: 0,
                        style: ModuleStyle::Path,
                        resolved: (self.resolve)(spec),
                        range: r(span),
                    }));
                    return out;
                }
                e => {
                    if let Some(spec) = require_spec(e) {
                        out.push(Stmt::Import(StmtImport { names: vec![Alias { name: spec.into(), asname: Some(name.into()), range: r(v.span) }], style: ModuleStyle::Path, resolved: (self.resolve)(spec), declaration: false, range: r(span) }));
                        return out;
                    }
                }
            }
        }
        // `const { a, b: c } = require("x")`
        if let (js::BindingPattern::ObjectPattern(p), Some(spec)) = (&v.id, require_spec(init)) {
            let names = p
                .properties
                .iter()
                .filter_map(|prop| {
                    let key = prop.key.static_name()?.to_string();
                    let mut bound = vec![];
                    pattern_names(&prop.value, &mut bound);
                    let local = bound.first()?.0.clone();
                    Some(Alias { asname: (local != key).then_some(local), name: key, range: r(prop.span) })
                })
                .collect();
            out.push(Stmt::ImportFrom(StmtImportFrom { module: Some(spec.into()), names, level: 0, style: ModuleStyle::Path, resolved: (self.resolve)(spec), range: r(span) }));
            return out;
        }
        let target = Box::new(self.pattern(&v.id));
        let value = Box::new(self.expr(init));
        out.push(match &v.type_annotation {
            Some(t) => Stmt::AnnAssign(StmtAnnAssign { target, annotation: Box::new(self.type_expr(&t.type_annotation)), value: Some(value), range: r(span) }),
            None => Stmt::Assign(StmtAssign { targets: vec![*target], value, range: r(span) }),
        });
        out
    }

    fn function_decl(&self, f: &js::Function, span: Span) -> Vec<Stmt> {
        let name = f.id.as_ref().map(|i| i.name.to_string()).unwrap_or_else(|| self.stem.clone());
        // Overload signatures and ambient declarations have no body; the first
        // overload usually carries the documentation.
        if f.body.is_none() || f.declare {
            let mut pending = self.overload_doc.borrow_mut();
            if pending.as_ref().is_none_or(|(n, _)| *n != name) {
                *pending = self.doc_before(span.start).map(|d| (name, d));
            }
            return vec![];
        }
        let mut def = self.function(f, &name, span, DefKind::Function, false, &[]);
        if let Some((n, doc)) = self.overload_doc.borrow_mut().take() {
            if n == name && def.doc.is_none() {
                def.doc = Some(doc);
            }
        }
        vec![Stmt::FunctionDef(def)]
    }

    fn type_alias(&self, t: &js::TSTypeAliasDeclaration) -> Vec<Stmt> {
        if t.declare {
            return vec![];
        }
        vec![Stmt::TypeAlias(StmtTypeAlias { name: Box::new(name_expr(t.id.name.as_str(), t.id.span, Ctx::Store)), value: Box::new(self.type_expr(&t.type_annotation)), range: r(t.span) })]
    }

    fn import(&self, x: &js::ImportDeclaration) -> Vec<Stmt> {
        let spec = x.source.value.as_str();
        let resolved = (self.resolve)(spec);
        let Some(specifiers) = &x.specifiers else {
            return vec![Stmt::Import(StmtImport { names: vec![Alias { name: spec.into(), asname: None, range: r(x.span) }], style: ModuleStyle::Path, resolved, declaration: false, range: r(x.span) })];
        };
        let mut out = vec![];
        let mut names = vec![];
        for s in specifiers {
            match s {
                js::ImportDeclarationSpecifier::ImportNamespaceSpecifier(n) => out.push(Stmt::Import(StmtImport {
                    names: vec![Alias { name: spec.into(), asname: Some(n.local.name.to_string()), range: r(n.span) }],
                    style: ModuleStyle::Path,
                    resolved: resolved.clone(),
                    declaration: false,
                    range: r(x.span),
                })),
                js::ImportDeclarationSpecifier::ImportDefaultSpecifier(d) => names.push(Alias { name: "default".into(), asname: Some(d.local.name.to_string()), range: r(d.span) }),
                js::ImportDeclarationSpecifier::ImportSpecifier(i) => {
                    let imported = export_name(&i.imported);
                    let local = i.local.name.to_string();
                    names.push(Alias { asname: (local != imported).then_some(local), name: imported, range: r(i.span) });
                }
            }
        }
        if !names.is_empty() || out.is_empty() {
            out.push(Stmt::ImportFrom(StmtImportFrom { module: Some(spec.into()), names, level: 0, style: ModuleStyle::Path, resolved, range: r(x.span) }));
        }
        out
    }

    fn export_default(&self, x: &js::ExportDefaultDeclaration) -> Vec<Stmt> {
        let span = x.span;
        let export = |name: &str| Stmt::Export(StmtExport { names: vec![(name.to_string(), "default".into())], range: end(span) });
        match &x.declaration {
            js::ExportDefaultDeclarationKind::FunctionDeclaration(f) => {
                let name = f.id.as_ref().map(|i| i.name.to_string()).unwrap_or_else(|| self.stem.clone());
                vec![Stmt::FunctionDef(self.function(f, &name, span, DefKind::Function, false, &[])), export(&name)]
            }
            js::ExportDefaultDeclarationKind::ClassDeclaration(c) => {
                let class = self.class(c, None, span);
                let name = class.name.clone();
                vec![Stmt::ClassDef(class), export(&name)]
            }
            js::ExportDefaultDeclarationKind::TSInterfaceDeclaration(i) => {
                let class = self.interface(i, span);
                let name = class.name.clone();
                vec![Stmt::ClassDef(class), export(&name)]
            }
            other => {
                let Some(e) = other.as_expression() else { return vec![] };
                if let Some((wrapper, f)) = wrapped(e) {
                    return vec![Stmt::FunctionDef(self.wrapped_function(wrapper, f, &self.stem, span)), export(&self.stem)];
                }
                match unparen(e) {
                    js::Expression::Identifier(i) => vec![export(i.name.as_str())],
                    js::Expression::ArrowFunctionExpression(a) => vec![Stmt::FunctionDef(self.arrow(a, &self.stem, span, DefKind::Function, false)), export(&self.stem)],
                    js::Expression::FunctionExpression(f) => {
                        let name = f.id.as_ref().map(|i| i.name.to_string()).unwrap_or_else(|| self.stem.clone());
                        vec![Stmt::FunctionDef(self.function(f, &name, span, DefKind::Function, false, &[])), export(&name)]
                    }
                    js::Expression::ClassExpression(c) => {
                        let class = self.class(c, Some(&self.stem), span);
                        let name = class.name.clone();
                        vec![Stmt::ClassDef(class), export(&name)]
                    }
                    e => vec![Stmt::Expr(StmtExpr { value: Box::new(self.expr(e)), range: r(span) })],
                }
            }
        }
    }

    fn expr_stmt(&self, e: &js::Expression, span: Span) -> Vec<Stmt> {
        use oxc_syntax::operator::UnaryOperator as U;
        match unparen(e) {
            // `void promise()` only discards the result; so does `!function () {…}()`.
            js::Expression::UnaryExpression(u) if u.operator == U::Void || u.operator == U::LogicalNot && is_iife(&u.argument) => self.expr_stmt(&u.argument, span),
            js::Expression::CallExpression(c) if is_iife_call(c) => self.iife(c),
            js::Expression::AssignmentExpression(a) => self.assignment(a, span),
            js::Expression::SequenceExpression(s) => s.expressions.iter().flat_map(|x| self.expr_stmt(x, x.span())).collect(),
            js::Expression::UpdateExpression(u) => {
                let target = self.simple_target(&u.argument);
                let op = if matches!(u.operator, oxc_syntax::operator::UpdateOperator::Increment) { Operator::Add } else { Operator::Sub };
                vec![Stmt::AugAssign(StmtAugAssign { target: Box::new(target), op, value: Box::new(Expr::NumberLiteral(atom(u.span))), range: r(span) })]
            }
            other => {
                if let Some(spec) = require_spec(other) {
                    return vec![Stmt::Import(StmtImport { names: vec![Alias { name: spec.into(), asname: None, range: r(span) }], style: ModuleStyle::Path, resolved: (self.resolve)(spec), declaration: false, range: r(span) })];
                }
                vec![Stmt::Expr(StmtExpr { value: Box::new(self.expr(other)), range: r(span) })]
            }
        }
    }

    /// `(function (root, factory) {…})(this, function () {…})`, `(async () => {…})()`:
    /// code run in place, in a scope of its own. Its parameters are bound to
    /// the arguments and its body runs here.
    fn iife(&self, c: &js::CallExpression) -> Vec<Stmt> {
        let (callee, args): (&js::Expression, &[js::Argument]) = match unparen(&c.callee) {
            // `(function () {…}).call(this, …)`: the first argument is `this`.
            js::Expression::StaticMemberExpression(m) => (unparen(&m.object), c.arguments.get(1..).unwrap_or(&[])),
            e => (e, &c.arguments[..]),
        };
        let (params, body) = match callee {
            js::Expression::FunctionExpression(f) => (&f.params, self.function_body(f.body.as_deref())),
            js::Expression::ArrowFunctionExpression(a) => (
                &a.params,
                match &a.body {
                    js::ArrowFunctionBody::FunctionBody(b) => self.stmts(&b.statements),
                    other => other.as_expression().map(|e| self.expr_stmt(e, e.span())).unwrap_or_default(),
                },
            ),
            _ => unreachable!(),
        };
        let mut out = vec![];
        for (p, a) in params.items.iter().zip(args) {
            let (js::BindingPattern::BindingIdentifier(id), Some(a)) = (&p.pattern, a.as_expression()) else { continue };
            let name = id.name.as_str();
            let span = a.span();
            out.push(match unparen(a) {
                js::Expression::FunctionExpression(f) => Stmt::FunctionDef(self.function(f, name, span, DefKind::Function, false, &[])),
                js::Expression::ArrowFunctionExpression(f) => Stmt::FunctionDef(self.arrow(f, name, span, DefKind::Function, false)),
                v => Stmt::Assign(StmtAssign { targets: vec![name_expr(name, id.span, Ctx::Store)], value: Box::new(self.expr(v)), range: r(span) }),
            });
        }
        out.extend(body);
        out
    }

    fn assignment(&self, a: &js::AssignmentExpression, span: Span) -> Vec<Stmt> {
        use oxc_syntax::operator::AssignmentOperator as A;
        // `exports = module.exports = createApplication`: the innermost
        // assignment defines; the outer targets receive what it assigned.
        if let (A::Assign, js::Expression::AssignmentExpression(inner)) = (a.operator, unparen(&a.right)) {
            if inner.operator == A::Assign {
                let mut out = self.assignment(inner, span);
                let (_, value) = chain(&a.right);
                let holder = match (&inner.left, unparen(value)) {
                    (js::AssignmentTarget::AssignmentTargetIdentifier(i), _) => Some(i.name.as_str()),
                    (_, js::Expression::Identifier(i)) => Some(i.name.as_str()),
                    _ => None,
                };
                match holder {
                    Some(name) => out.extend(self.also_assigned(&[&a.left], name, span)),
                    None if !matches!(&a.left, js::AssignmentTarget::AssignmentTargetIdentifier(i) if i.name.as_str() == "exports") => {
                        out.push(Stmt::Assign(StmtAssign { targets: vec![self.target(&a.left)], value: Box::new(self.target_as_load(&inner.left)), range: r(span) }));
                    }
                    None => {}
                }
                return out;
            }
        }
        // CommonJS exports: `module.exports = router`, `exports.handler = …`.
        if let Some(m) = a.left.as_member_expression() {
            let object = m.object();
            let module_exports = matches!(object, js::Expression::Identifier(i) if i.name.as_str() == "module") && m.static_property_name().is_some_and(|p| p == "exports");
            let exports_field = matches!(object, js::Expression::Identifier(i) if i.name.as_str() == "exports") || matches!(object, js::Expression::StaticMemberExpression(o) if matches!(&o.object, js::Expression::Identifier(i) if i.name.as_str() == "module") && o.property.name.as_str() == "exports");
            if module_exports && matches!(a.operator, A::Assign) {
                return match unparen(&a.right) {
                    js::Expression::Identifier(i) => vec![Stmt::Export(StmtExport { names: vec![(i.name.to_string(), "default".into())], range: end(span) })],
                    // `module.exports = require("./lib/express")` re-exports a module.
                    e if require_spec(e).is_some() => {
                        let spec = require_spec(e).unwrap();
                        vec![
                            Stmt::Import(StmtImport { names: vec![Alias { name: spec.into(), asname: Some(self.stem.clone()), range: r(span) }], style: ModuleStyle::Path, resolved: (self.resolve)(spec), declaration: false, range: r(span) }),
                            Stmt::Export(StmtExport { names: vec![(self.stem.clone(), "default".into())], range: end(span) }),
                        ]
                    }
                    js::Expression::ObjectExpression(o) if o.properties.iter().all(|p| matches!(p, js::ObjectPropertyKind::ObjectProperty(x) if x.shorthand)) => {
                        let names = o.properties.iter().filter_map(|p| match p {
                            js::ObjectPropertyKind::ObjectProperty(x) => x.key.static_name().map(|n| (n.to_string(), n.to_string())),
                            _ => None,
                        });
                        vec![Stmt::Export(StmtExport { names: names.collect(), range: end(span) })]
                    }
                    js::Expression::ArrowFunctionExpression(f) => vec![Stmt::FunctionDef(self.arrow(f, &self.stem, span, DefKind::Function, false)), Stmt::Export(StmtExport { names: vec![(self.stem.clone(), "default".into())], range: end(span) })],
                    js::Expression::FunctionExpression(f) => {
                        let name = f.id.as_ref().map(|i| i.name.to_string()).unwrap_or_else(|| self.stem.clone());
                        vec![Stmt::FunctionDef(self.function(f, &name, span, DefKind::Function, false, &[])), Stmt::Export(StmtExport { names: vec![(name, "default".into())], range: end(span) })]
                    }
                    js::Expression::ClassExpression(c) => {
                        let class = self.class(c, Some(&self.stem), span);
                        let name = class.name.clone();
                        vec![Stmt::ClassDef(class), Stmt::Export(StmtExport { names: vec![(name, "default".into())], range: end(span) })]
                    }
                    e => vec![Stmt::Expr(StmtExpr { value: Box::new(self.expr(e)), range: r(span) })],
                };
            }
            if exports_field && matches!(a.operator, A::Assign) {
                if let Some(field) = m.static_property_name() {
                    let field = field.to_string();
                    let export = Stmt::Export(StmtExport { names: vec![(field.clone(), field.clone())], range: end(span) });
                    // `exports.users = require("./users")` re-exports a module.
                    if let Some(spec) = require_spec(&a.right) {
                        return vec![Stmt::Import(StmtImport { names: vec![Alias { name: spec.into(), asname: Some(field), range: r(span) }], style: ModuleStyle::Path, resolved: (self.resolve)(spec), declaration: false, range: r(span) }), export];
                    }
                    return match unparen(&a.right) {
                        js::Expression::ArrowFunctionExpression(f) => vec![Stmt::FunctionDef(self.arrow(f, &field, span, DefKind::Function, false)), export],
                        js::Expression::FunctionExpression(f) => vec![Stmt::FunctionDef(self.function(f, &field, span, DefKind::Function, false, &[])), export],
                        js::Expression::Identifier(i) => vec![Stmt::Export(StmtExport { names: vec![(i.name.to_string(), field)], range: end(span) })],
                        e => vec![Stmt::Assign(StmtAssign { targets: vec![name_expr(&field, m.span(), Ctx::Store)], value: Box::new(self.expr(e)), range: r(span) }), export],
                    };
                }
            }
        }
        let target = self.target(&a.left);
        // `schema.statics.isEmailTaken = async function …` is named by its field.
        let field = a.left.as_member_expression().and_then(|m| m.static_property_name()).map(|p| p.to_string()).unwrap_or_default();
        let value = self.function_value(unparen(&a.right), &field).unwrap_or_else(|| self.expr(&a.right));
        let op = match a.operator {
            A::Assign => None,
            A::Addition => Some(Operator::Add),
            A::Subtraction => Some(Operator::Sub),
            A::Multiplication => Some(Operator::Mult),
            A::Division => Some(Operator::Div),
            A::Remainder => Some(Operator::Mod),
            A::Exponential => Some(Operator::Pow),
            A::ShiftLeft => Some(Operator::LShift),
            A::ShiftRight => Some(Operator::RShift),
            A::ShiftRightZeroFill => Some(Operator::URShift),
            A::BitwiseOR => Some(Operator::BitOr),
            A::BitwiseXOR => Some(Operator::BitXor),
            A::BitwiseAnd => Some(Operator::BitAnd),
            A::LogicalOr | A::LogicalAnd | A::LogicalNullish => {
                let op = match a.operator {
                    A::LogicalOr => BoolOp::OrOr,
                    A::LogicalAnd => BoolOp::AndAnd,
                    _ => BoolOp::Coalesce,
                };
                let current = self.target_as_load(&a.left);
                let value = Expr::BoolOp(ExprBoolOp { op, values: vec![current, value], range: r(a.span) });
                return vec![Stmt::Assign(StmtAssign { targets: vec![target], value: Box::new(value), range: r(span) })];
            }
        };
        match op {
            None => vec![Stmt::Assign(StmtAssign { targets: vec![target], value: Box::new(value), range: r(span) })],
            Some(op) => vec![Stmt::AugAssign(StmtAugAssign { target: Box::new(target), op, value: Box::new(value), range: r(span) })],
        }
    }

    // ----- definitions ------------------------------------------------------------

    fn params(&self, p: &js::FormalParameters, receiver: Option<Span>) -> (Vec<Param>, Vec<ParamProperty>) {
        let mut out = vec![];
        // TypeScript parameter properties: `constructor(private users: UsersService)`.
        let mut properties = vec![];
        if let Some(s) = receiver {
            out.push(Param { name: "this".into(), kind: ParamKind::Positional, default: None, annotation: None, range: r(Span::new(s.start, s.start)) });
        }
        for item in &p.items {
            let annotation = item.type_annotation.as_ref().map(|t| self.type_expr(&t.type_annotation));
            let default = item.initializer.as_deref().map(|d| self.expr(d));
            let mut names = vec![];
            pattern_names(&item.pattern, &mut names);
            let single = matches!(item.pattern, js::BindingPattern::BindingIdentifier(_)) || (matches!(item.pattern, js::BindingPattern::AssignmentPattern(_)) && names.len() == 1);
            if (item.accessibility.is_some() || item.readonly) && names.len() == 1 {
                properties.push((names[0].0.clone(), annotation.clone(), names[0].1));
            }
            for (i, (name, span)) in names.into_iter().enumerate() {
                out.push(Param {
                    name,
                    kind: ParamKind::Positional,
                    default: if single && i == 0 { default.clone() } else { None },
                    annotation: if single { annotation.clone() } else { None },
                    range: r(span),
                });
            }
        }
        if let Some(rest) = &p.rest {
            let mut names = vec![];
            pattern_names(&rest.rest.argument, &mut names);
            for (name, span) in names {
                out.push(Param { name, kind: ParamKind::Variadic, default: None, annotation: None, range: r(span) });
            }
        }
        (out, properties)
    }

    /// `const createUser = catchAsync(async (req, res) => …)`: the function,
    /// decorated by its wrapper.
    fn wrapped_function(&self, wrapper: &js::Expression, f: &js::Expression, name: &str, span: Span) -> FunctionDef {
        let mut def = match f {
            js::Expression::ArrowFunctionExpression(a) => self.arrow(a, name, span, DefKind::Function, false),
            js::Expression::FunctionExpression(x) => self.function(x, name, span, DefKind::Function, false, &[]),
            _ => unreachable!(),
        };
        def.decorators.push(Decorator { expression: self.expr(wrapper), range: r(wrapper.span()) });
        def
    }

    fn function_body(&self, body: Option<&js::FunctionBody>) -> Vec<Stmt> {
        body.map(|b| self.stmts(&b.statements)).unwrap_or_default()
    }

    fn function(&self, f: &js::Function, name: &str, span: Span, kind: DefKind, receiver: bool, fields: &[Stmt]) -> FunctionDef {
        let (params, properties) = self.params(&f.params, receiver.then_some(f.params.span));
        let mut body: Vec<Stmt> = fields.to_vec();
        // Parameter properties are fields the constructor sets.
        for (field, annotation, s) in properties {
            let target = Box::new(Expr::Attribute(ExprAttribute { value: Box::new(name_expr("this", s, Ctx::Load)), attr: field.clone(), range: r(s) }));
            let value = Box::new(name_expr(&field, s, Ctx::Load));
            body.push(match annotation {
                Some(a) => Stmt::AnnAssign(StmtAnnAssign { target, annotation: Box::new(a), value: Some(value), range: r(s) }),
                None => Stmt::Assign(StmtAssign { targets: vec![*target], value, range: r(s) }),
            });
        }
        body.extend(self.function_body(f.body.as_deref()));
        FunctionDef {
            name: name.to_string(),
            params,
            body,
            decorators: vec![],
            returns: f.return_type.as_ref().map(|t| Box::new(self.type_expr(&t.type_annotation))),
            is_async: f.r#async,
            is_generator: f.generator,
            kind,
            doc: self.doc_before(span.start),
            range: r(span),
        }
    }

    fn arrow(&self, a: &js::ArrowFunctionExpression, name: &str, span: Span, kind: DefKind, receiver: bool) -> FunctionDef {
        let (params, _) = self.params(&a.params, receiver.then_some(a.params.span));
        let body = match &a.body {
            js::ArrowFunctionBody::FunctionBody(b) => self.stmts(&b.statements),
            other => match other.as_expression() {
                Some(e) => vec![Stmt::Return(StmtReturn { value: Some(Box::new(self.expr(e))), range: r(e.span()) })],
                None => vec![],
            },
        };
        FunctionDef {
            name: name.to_string(),
            params,
            body,
            decorators: vec![],
            returns: a.return_type.as_ref().map(|t| Box::new(self.type_expr(&t.type_annotation))),
            is_async: a.r#async,
            is_generator: false,
            kind,
            doc: self.doc_before(span.start),
            range: r(span),
        }
    }

    fn decorators(&self, ds: &[js::Decorator]) -> Vec<Decorator> {
        ds.iter().map(|d| Decorator { expression: self.expr(&d.expression), range: r(d.span) }).collect()
    }

    fn key_name(&self, k: &js::PropertyKey) -> String {
        match k {
            js::PropertyKey::PrivateIdentifier(p) => format!("#{}", p.name),
            other => other.static_name().map(|n| n.to_string()).unwrap_or_else(|| self.text.get(other.span().start as usize..other.span().end as usize).unwrap_or("").to_string()),
        }
    }

    fn class(&self, c: &js::Class, name: Option<&str>, span: Span) -> ClassDef {
        let name = c.id.as_ref().map(|i| i.name.to_string()).or_else(|| name.map(str::to_string)).unwrap_or_else(|| self.stem.clone());
        let mut body = vec![];
        // Instance fields with initial values are set as the constructor starts.
        let mut fields: Vec<Stmt> = vec![];
        for el in &c.body.body {
            if let js::ClassElement::PropertyDefinition(p) = el {
                if p.r#static {
                    continue;
                }
                let is_fn = matches!(p.value.as_ref().map(unparen), Some(js::Expression::ArrowFunctionExpression(_) | js::Expression::FunctionExpression(_)));
                if is_fn {
                    continue;
                }
                let key = self.key_name(&p.key);
                let target = Box::new(Expr::Attribute(ExprAttribute { value: Box::new(name_expr("this", p.key.span(), Ctx::Load)), attr: key, range: r(p.key.span()) }));
                let annotation = p.type_annotation.as_ref().map(|t| self.type_expr(&t.type_annotation));
                match (annotation, &p.value) {
                    (Some(a), v) => fields.push(Stmt::AnnAssign(StmtAnnAssign { target, annotation: Box::new(a), value: v.as_ref().map(|v| Box::new(self.expr(v))), range: r(p.span) })),
                    (None, Some(v)) => fields.push(Stmt::Assign(StmtAssign { targets: vec![*target], value: Box::new(self.expr(v)), range: r(p.span) })),
                    (None, None) => {}
                }
            }
        }
        let has_constructor = c.body.body.iter().any(|el| matches!(el, js::ClassElement::MethodDefinition(m) if m.kind == js::MethodDefinitionKind::Constructor));
        // Without a constructor, the fields still belong to the class.
        if !has_constructor {
            body.extend(fields.iter().cloned());
        }
        // Overloads: the method with a body is the method.
        let implemented: Vec<String> = c.body.body.iter().filter_map(|el| match el {
            js::ClassElement::MethodDefinition(m) if m.value.body.is_some() => Some(self.key_name(&m.key)),
            _ => None,
        }).collect();
        for el in &c.body.body {
            match el {
                js::ClassElement::MethodDefinition(m) if m.value.body.is_none() && implemented.contains(&self.key_name(&m.key)) => {}
                js::ClassElement::MethodDefinition(m) => {
                    let key = self.key_name(&m.key);
                    let (kind, mname) = match m.kind {
                        js::MethodDefinitionKind::Constructor => (DefKind::Constructor, "constructor".to_string()),
                        js::MethodDefinitionKind::Get => (DefKind::Getter, key),
                        js::MethodDefinitionKind::Set => (DefKind::Setter, key),
                        js::MethodDefinitionKind::Method if m.r#static => (DefKind::StaticMethod, key),
                        js::MethodDefinitionKind::Method => (DefKind::Method, key),
                    };
                    let prologue: &[Stmt] = if kind == DefKind::Constructor { &fields } else { &[] };
                    let mut f = self.function(&m.value, &mname, m.span, kind, !m.r#static, prologue);
                    f.decorators = self.decorators(&m.decorators);
                    body.push(Stmt::FunctionDef(f));
                }
                js::ClassElement::PropertyDefinition(p) => {
                    let key = self.key_name(&p.key);
                    match p.value.as_ref().map(unparen) {
                        Some(js::Expression::ArrowFunctionExpression(a)) => {
                            let mut f = self.arrow(a, &key, p.span, if p.r#static { DefKind::StaticMethod } else { DefKind::Method }, !p.r#static);
                            f.decorators = self.decorators(&p.decorators);
                            body.push(Stmt::FunctionDef(f));
                        }
                        Some(js::Expression::FunctionExpression(fx)) => {
                            let mut f = self.function(fx, &key, p.span, if p.r#static { DefKind::StaticMethod } else { DefKind::Method }, !p.r#static, &[]);
                            f.decorators = self.decorators(&p.decorators);
                            body.push(Stmt::FunctionDef(f));
                        }
                        // Static fields are class attributes.
                        Some(v) if p.r#static => body.push(Stmt::Assign(StmtAssign { targets: vec![name_expr(&key, p.key.span(), Ctx::Store)], value: Box::new(self.expr(v)), range: r(p.span) })),
                        _ => {}
                    }
                }
                js::ClassElement::AccessorProperty(p) => {
                    if let Some(v) = &p.value {
                        body.push(Stmt::Assign(StmtAssign { targets: vec![name_expr(&self.key_name(&p.key), p.key.span(), Ctx::Store)], value: Box::new(self.expr(v)), range: r(p.span) }));
                    }
                }
                js::ClassElement::StaticBlock(b) => body.push(Stmt::Other(atom(b.span))),
                js::ClassElement::TSIndexSignature(_) => {}
            }
        }
        ClassDef {
            name,
            bases: c.heritage.as_ref().map(|h| vec![self.expr(&h.expression)]).unwrap_or_default(),
            keywords: vec![],
            body,
            decorators: self.decorators(&c.decorators),
            kind: ClassKind::Class,
            doc: self.doc_before(span.start),
            range: r(span),
        }
    }

    fn interface(&self, i: &js::TSInterfaceDeclaration, span: Span) -> ClassDef {
        let mut body = vec![];
        for sig in &i.body.body {
            match sig {
                js::TSSignature::TSPropertySignature(p) => {
                    let key = self.key_name(&p.key);
                    let annotation = match &p.type_annotation {
                        Some(t) => self.type_expr(&t.type_annotation),
                        None => Expr::Other(atom(p.span)),
                    };
                    body.push(Stmt::AnnAssign(StmtAnnAssign { target: Box::new(name_expr(&key, p.key.span(), Ctx::Store)), annotation: Box::new(annotation), value: None, range: r(p.span) }));
                }
                js::TSSignature::TSMethodSignature(m) => {
                    let key = self.key_name(&m.key);
                    let sig = Span::new(m.params.span.start, m.return_type.as_ref().map(|t| t.span.end).unwrap_or(m.params.span.end));
                    body.push(Stmt::AnnAssign(StmtAnnAssign { target: Box::new(name_expr(&key, m.key.span(), Ctx::Store)), annotation: Box::new(Expr::Other(atom(sig))), value: None, range: r(m.span) }));
                }
                _ => {}
            }
        }
        ClassDef {
            name: i.id.name.to_string(),
            bases: i.extends.iter().map(|h| self.type_name(&h.type_name)).collect(),
            keywords: vec![],
            body,
            decorators: vec![],
            kind: ClassKind::Interface,
            doc: self.doc_before(span.start),
            range: r(span),
        }
    }

    fn enumeration(&self, e: &js::TSEnumDeclaration, span: Span) -> ClassDef {
        let body = e
            .body
            .members
            .iter()
            .map(|m| {
                let (name, s) = match &m.id {
                    js::TSEnumMemberName::Identifier(i) => (i.name.to_string(), i.span),
                    js::TSEnumMemberName::String(x) | js::TSEnumMemberName::ComputedString(x) => (x.value.to_string(), x.span),
                    js::TSEnumMemberName::ComputedTemplateString(t) => (self.text.get(t.span.start as usize..t.span.end as usize).unwrap_or("").to_string(), t.span),
                };
                match &m.initializer {
                    Some(v) => Stmt::Assign(StmtAssign { targets: vec![name_expr(&name, s, Ctx::Store)], value: Box::new(self.expr(v)), range: r(m.span) }),
                    None => Stmt::Expr(StmtExpr { value: Box::new(name_expr(&name, s, Ctx::Load)), range: r(m.span) }),
                }
            })
            .collect();
        ClassDef { name: e.id.name.to_string(), bases: vec![], keywords: vec![], body, decorators: vec![], kind: ClassKind::Enum, doc: self.doc_before(span.start), range: r(span) }
    }

    /// `namespace Geometry { export function scale() … }`: a named group of
    /// functions and constants, like a class of static members.
    fn namespace(&self, n: &js::TSNamespaceDeclaration, span: Span) -> Vec<Stmt> {
        if n.declare {
            return vec![];
        }
        let body = match &n.body {
            js::TSNamespaceDeclarationBody::TSModuleBlock(b) => self.stmts(&b.body),
            js::TSNamespaceDeclarationBody::TSNamespaceDeclaration(inner) => self.namespace(inner, inner.span),
        };
        let body = body
            .into_iter()
            .filter(|s| !matches!(s, Stmt::Export(_)))
            .map(|s| match s {
                Stmt::FunctionDef(mut f) if f.kind == DefKind::Function => {
                    f.kind = DefKind::StaticMethod;
                    Stmt::FunctionDef(f)
                }
                s => s,
            })
            .collect();
        vec![Stmt::ClassDef(ClassDef { name: n.id.name.to_string(), bases: vec![], keywords: vec![], body, decorators: vec![], kind: ClassKind::Namespace, doc: self.doc_before(span.start), range: r(span) })]
    }

    /// A type as an expression: a named type is the name (so `users: UsersService`
    /// types the field); anything else is kept as text.
    fn type_expr(&self, t: &js::TSType) -> Expr {
        match t {
            js::TSType::TSTypeReference(x) => self.type_name(&x.type_name),
            other => Expr::Other(atom(other.span())),
        }
    }

    fn type_name(&self, n: &js::TSTypeName) -> Expr {
        match n {
            js::TSTypeName::IdentifierReference(i) => name_expr(i.name.as_str(), i.span, Ctx::Load),
            js::TSTypeName::QualifiedName(q) => Expr::Attribute(ExprAttribute { value: Box::new(self.type_name(&q.left)), attr: q.right.name.to_string(), range: r(q.span) }),
            js::TSTypeName::ThisExpression(t) => name_expr("this", t.span, Ctx::Load),
        }
    }

    // ----- patterns and targets ----------------------------------------------------

    fn pattern(&self, p: &js::BindingPattern) -> Expr {
        match p {
            js::BindingPattern::BindingIdentifier(i) => name_expr(i.name.as_str(), i.span, Ctx::Store),
            js::BindingPattern::ObjectPattern(o) => {
                let mut elts: Vec<Expr> = o.properties.iter().map(|prop| self.pattern(&prop.value)).collect();
                if let Some(rest) = &o.rest {
                    elts.push(Expr::Starred(ExprInner { value: Box::new(self.pattern(&rest.argument)), range: r(rest.span) }));
                }
                Expr::Tuple(ExprSeq { elts, range: r(o.span) })
            }
            js::BindingPattern::ArrayPattern(a) => {
                let mut elts: Vec<Expr> = a.elements.iter().flatten().map(|el| self.pattern(el)).collect();
                if let Some(rest) = &a.rest {
                    elts.push(Expr::Starred(ExprInner { value: Box::new(self.pattern(&rest.argument)), range: r(rest.span) }));
                }
                Expr::Tuple(ExprSeq { elts, range: r(a.span) })
            }
            js::BindingPattern::AssignmentPattern(a) => self.pattern(&a.left),
        }
    }

    fn simple_target(&self, t: &js::SimpleAssignmentTarget) -> Expr {
        match t {
            js::SimpleAssignmentTarget::AssignmentTargetIdentifier(i) => name_expr(i.name.as_str(), i.span, Ctx::Store),
            js::SimpleAssignmentTarget::TSAsExpression(x) => self.expr(&x.expression),
            js::SimpleAssignmentTarget::TSSatisfiesExpression(x) => self.expr(&x.expression),
            js::SimpleAssignmentTarget::TSNonNullExpression(x) => self.expr(&x.expression),
            js::SimpleAssignmentTarget::TSTypeAssertion(x) => self.expr(&x.expression),
            other => match other.as_member_expression() {
                Some(m) => self.member(m),
                None => Expr::Other(atom(other.span())),
            },
        }
    }

    fn target(&self, t: &js::AssignmentTarget) -> Expr {
        if let Some(s) = t.as_simple_assignment_target() {
            return self.simple_target(s);
        }
        match t.as_assignment_target_pattern() {
            Some(js::AssignmentTargetPattern::ArrayAssignmentTarget(a)) => {
                let mut elts: Vec<Expr> = a.elements.iter().flatten().map(|el| self.maybe_default(el)).collect();
                if let Some(rest) = &a.rest {
                    elts.push(Expr::Starred(ExprInner { value: Box::new(self.target(&rest.target)), range: r(rest.span) }));
                }
                Expr::Tuple(ExprSeq { elts, range: r(a.span) })
            }
            Some(js::AssignmentTargetPattern::ObjectAssignmentTarget(o)) => {
                let mut elts: Vec<Expr> = o
                    .properties
                    .iter()
                    .map(|p| match p {
                        js::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(i) => name_expr(i.binding.name.as_str(), i.binding.span, Ctx::Store),
                        js::AssignmentTargetProperty::AssignmentTargetPropertyProperty(x) => self.maybe_default(&x.binding),
                    })
                    .collect();
                if let Some(rest) = &o.rest {
                    elts.push(Expr::Starred(ExprInner { value: Box::new(self.target(&rest.target)), range: r(rest.span) }));
                }
                Expr::Tuple(ExprSeq { elts, range: r(o.span) })
            }
            None => Expr::Other(atom(t.span())),
        }
    }

    fn maybe_default(&self, t: &js::AssignmentTargetMaybeDefault) -> Expr {
        match t {
            js::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(d) => self.target(&d.binding),
            other => match other.as_assignment_target() {
                Some(t) => self.target(t),
                None => Expr::Other(atom(other.span())),
            },
        }
    }

    /// The value an assignment target currently holds (`a ||= b` reads `a`).
    fn target_as_load(&self, t: &js::AssignmentTarget) -> Expr {
        match self.target(t) {
            Expr::Name(mut n) => {
                n.ctx = Ctx::Load;
                Expr::Name(n)
            }
            other => other,
        }
    }

    // ----- expressions ----------------------------------------------------------------

    fn member(&self, m: &js::MemberExpression) -> Expr {
        match m {
            js::MemberExpression::StaticMemberExpression(x) => Expr::Attribute(ExprAttribute { value: Box::new(self.expr(&x.object)), attr: x.property.name.to_string(), range: r(x.span) }),
            js::MemberExpression::ComputedMemberExpression(x) => Expr::Subscript(ExprSubscript { value: Box::new(self.expr(&x.object)), slice: Box::new(self.expr(&x.expression)), range: r(x.span) }),
            js::MemberExpression::PrivateFieldExpression(x) => Expr::Attribute(ExprAttribute { value: Box::new(self.expr(&x.object)), attr: format!("#{}", x.field.name), range: r(x.span) }),
        }
    }

    fn arguments(&self, args: &[js::Argument]) -> Vec<Expr> {
        // `app.post("/users", (req, res) => res.json(…))`, `emitter.on("data", …)`:
        // a function registered under a name is a handler, however short.
        let keyed = matches!(args.first().and_then(|a| a.as_expression()).map(unparen), Some(js::Expression::StringLiteral(_) | js::Expression::TemplateLiteral(_)));
        args.iter()
            .map(|a| match a {
                js::Argument::SpreadElement(s) => Expr::Starred(ExprInner { value: Box::new(self.expr(&s.argument)), range: r(s.span) }),
                other => match other.as_expression().map(unparen) {
                    Some(js::Expression::ArrowFunctionExpression(f)) if keyed => Expr::Function(Box::new(self.arrow(f, "", f.span, DefKind::Function, false))),
                    Some(e) => self.expr(e),
                    None => Expr::Other(atom(other.span())),
                },
            })
            .collect()
    }

    fn call(&self, c: &js::CallExpression) -> Expr {
        if let Some(spec) = require_call(c) {
            self.required.borrow_mut().push((spec.to_string(), c.span));
        }
        Expr::Call(ExprCall { func: Box::new(self.expr(&c.callee)), arguments: Arguments { args: self.arguments(&c.arguments), keywords: vec![] }, kind: CallKind::Call, range: r(c.span) })
    }

    /// A function used as a value: an expression-bodied arrow is a lambda
    /// (glue folded into what uses it); one with statements is a function.
    fn function_value(&self, e: &js::Expression, name: &str) -> Option<Expr> {
        match e {
            js::Expression::ArrowFunctionExpression(a) => Some(match &a.body {
                js::ArrowFunctionBody::FunctionBody(_) => Expr::Function(Box::new(self.arrow(a, name, a.span, DefKind::Function, false))),
                // `h1: (props) => <h1 {...props} />`: a named function that renders is a component.
                other if !name.is_empty() && other.as_expression().is_some_and(|b| matches!(unparen(b), js::Expression::JSXElement(_) | js::Expression::JSXFragment(_))) => {
                    Expr::Function(Box::new(self.arrow(a, name, a.span, DefKind::Function, false)))
                }
                other => {
                    let (params, _) = self.params(&a.params, None);
                    match other.as_expression() {
                        Some(body) => Expr::Lambda(ExprLambda { params, body: Box::new(self.expr(body)), range: r(a.span) }),
                        None => Expr::Other(atom(a.span)),
                    }
                }
            }),
            js::Expression::FunctionExpression(f) => {
                let name = f.id.as_ref().map(|i| i.name.to_string()).unwrap_or_else(|| name.to_string());
                Some(Expr::Function(Box::new(self.function(f, &name, f.span, DefKind::Function, false, &[]))))
            }
            _ => None,
        }
    }

    fn logical(&self, l: &js::LogicalExpression) -> Expr {
        use oxc_syntax::operator::LogicalOperator as L;
        let op = match l.operator {
            L::And => BoolOp::AndAnd,
            L::Or => BoolOp::OrOr,
            L::Coalesce => BoolOp::Coalesce,
        };
        // `a && b && c` is one test of three values.
        let mut values = vec![];
        let mut flatten = |e: &js::Expression| match unparen(e) {
            js::Expression::LogicalExpression(inner) if inner.operator == l.operator => match self.logical(inner) {
                Expr::BoolOp(b) => values.extend(b.values),
                other => values.push(other),
            },
            other => values.push(self.expr(other)),
        };
        flatten(&l.left);
        flatten(&l.right);
        Expr::BoolOp(ExprBoolOp { op, values, range: r(l.span) })
    }

    pub fn expr(&self, e: &js::Expression) -> Expr {
        use oxc_syntax::operator::{BinaryOperator as B, UnaryOperator as U};
        match e {
            js::Expression::BooleanLiteral(x) => Expr::BooleanLiteral(ExprBooleanLiteral { value: x.value, range: r(x.span) }),
            js::Expression::NullLiteral(x) => Expr::NoneLiteral(atom(x.span)),
            js::Expression::NumericLiteral(x) => Expr::NumberLiteral(atom(x.span)),
            js::Expression::BigIntLiteral(x) => Expr::NumberLiteral(atom(x.span)),
            // A pattern is a constant, like the string Python passes to `re.compile`.
            js::Expression::RegExpLiteral(x) => string(self.text.get(x.span.start as usize..x.span.end as usize).unwrap_or(""), x.span),
            js::Expression::StringLiteral(x) => string(x.value.as_str(), x.span),
            js::Expression::TemplateLiteral(t) => self.template(t, None),
            js::Expression::TaggedTemplateExpression(t) => self.template(&t.quasi, Some(&t.tag)),
            js::Expression::Identifier(i) if i.name.as_str() == "undefined" => Expr::NoneLiteral(atom(i.span)),
            js::Expression::Identifier(i) => name_expr(i.name.as_str(), i.span, Ctx::Load),
            js::Expression::Super(s) => name_expr("super", s.span, Ctx::Load),
            js::Expression::ThisExpression(t) => name_expr("this", t.span, Ctx::Load),
            js::Expression::ArrayExpression(a) => Expr::List(ExprSeq {
                elts: a
                    .elements
                    .iter()
                    .filter_map(|el| match el {
                        js::ArrayExpressionElement::SpreadElement(s) => Some(Expr::Starred(ExprInner { value: Box::new(self.expr(&s.argument)), range: r(s.span) })),
                        js::ArrayExpressionElement::Elision(_) => None,
                        other => other.as_expression().map(|x| self.expr(x)),
                    })
                    .collect(),
                range: r(a.span),
            }),
            js::Expression::ObjectExpression(o) => Expr::Dict(ExprDict {
                items: o
                    .properties
                    .iter()
                    .map(|p| match p {
                        js::ObjectPropertyKind::SpreadProperty(s) => DictItem { key: None, value: self.expr(&s.argument) },
                        js::ObjectPropertyKind::ObjectProperty(x) => {
                            let name = self.key_name(&x.key);
                            let key = if x.computed { x.key.as_expression().map(|k| self.expr(k)).unwrap_or_else(|| string(&name, x.key.span())) } else { string(&name, x.key.span()) };
                            let value = match unparen(&x.value) {
                                // `beforeError: [(ctx) => {…}]`: functions listed under a key take its name.
                                js::Expression::ArrayExpression(a) => Expr::List(ExprSeq {
                                    elts: a
                                        .elements
                                        .iter()
                                        .filter_map(|el| el.as_expression().map(|e| self.function_value(unparen(e), &name).unwrap_or_else(|| self.expr(e))))
                                        .collect(),
                                    range: r(a.span),
                                }),
                                v => self.function_value(v, &name).unwrap_or_else(|| self.expr(&x.value)),
                            };
                            DictItem { key: Some(key), value }
                        }
                    })
                    .collect(),
                range: r(o.span),
            }),
            js::Expression::ArrowFunctionExpression(_) | js::Expression::FunctionExpression(_) => self.function_value(e, "").unwrap(),
            js::Expression::AssignmentExpression(a) => {
                let stmts = self.assignment(a, a.span);
                match stmts.into_iter().next() {
                    Some(Stmt::Assign(x)) => Expr::Named(ExprNamed { target: Box::new(x.targets.into_iter().next().unwrap()), value: x.value, range: r(a.span) }),
                    Some(Stmt::AugAssign(x)) => Expr::Named(ExprNamed { target: x.target.clone(), value: Box::new(Expr::BinOp(ExprBinOp { left: x.target, op: x.op, right: x.value, range: r(a.span) })), range: r(a.span) }),
                    _ => self.expr(&a.right),
                }
            }
            js::Expression::AwaitExpression(a) => Expr::Await(ExprInner { value: Box::new(self.expr(&a.argument)), range: r(a.span) }),
            js::Expression::BinaryExpression(b) => {
                let left = Box::new(self.expr(&b.left));
                let right = Box::new(self.expr(&b.right));
                let arith = |op| Expr::BinOp(ExprBinOp { left: left.clone(), op, right: right.clone(), range: r(b.span) });
                let cmp = |op| Expr::Compare(ExprCompare { ops: vec![op], operands: vec![(*left).clone(), (*right).clone()], range: r(b.span) });
                match b.operator {
                    B::Equality => cmp(CmpOp::Eq),
                    B::Inequality => cmp(CmpOp::NotEq),
                    B::StrictEquality => cmp(CmpOp::StrictEq),
                    B::StrictInequality => cmp(CmpOp::StrictNotEq),
                    B::LessThan => cmp(CmpOp::Lt),
                    B::LessEqualThan => cmp(CmpOp::LtE),
                    B::GreaterThan => cmp(CmpOp::Gt),
                    B::GreaterEqualThan => cmp(CmpOp::GtE),
                    B::In => cmp(CmpOp::In),
                    B::Instanceof => cmp(CmpOp::InstanceOf),
                    B::Addition => arith(Operator::Add),
                    B::Subtraction => arith(Operator::Sub),
                    B::Multiplication => arith(Operator::Mult),
                    B::Division => arith(Operator::Div),
                    B::Remainder => arith(Operator::Mod),
                    B::Exponential => arith(Operator::Pow),
                    B::ShiftLeft => arith(Operator::LShift),
                    B::ShiftRight => arith(Operator::RShift),
                    B::ShiftRightZeroFill => arith(Operator::URShift),
                    B::BitwiseOR => arith(Operator::BitOr),
                    B::BitwiseXOR => arith(Operator::BitXor),
                    B::BitwiseAnd => arith(Operator::BitAnd),
                }
            }
            js::Expression::CallExpression(c) => self.call(c),
            js::Expression::ChainExpression(c) => match &c.expression {
                js::ChainElement::CallExpression(x) => self.call(x),
                js::ChainElement::TSNonNullExpression(x) => self.expr(&x.expression),
                other => match other.as_member_expression() {
                    Some(m) => self.member(m),
                    None => Expr::Other(atom(c.span)),
                },
            },
            js::Expression::ClassExpression(c) => Expr::Other(atom(c.span)),
            js::Expression::ConditionalExpression(c) => Expr::If(ExprIf { test: Box::new(self.expr(&c.test)), body: Box::new(self.expr(&c.consequent)), orelse: Box::new(self.expr(&c.alternate)), range: r(c.span) }),
            js::Expression::ImportExpression(i) => {
                if let js::Expression::StringLiteral(s) = &i.source {
                    self.required.borrow_mut().push((s.value.to_string(), i.span));
                }
                self.import_call(i)
            }
            js::Expression::LogicalExpression(l) => self.logical(l),
            js::Expression::NewExpression(n) => Expr::Call(ExprCall { func: Box::new(self.expr(&n.callee)), arguments: Arguments { args: self.arguments(&n.arguments), keywords: vec![] }, kind: CallKind::New, range: r(n.span) }),
            js::Expression::ParenthesizedExpression(p) => self.expr(&p.expression),
            js::Expression::SequenceExpression(s) => Expr::Tuple(ExprSeq { elts: s.expressions.iter().map(|x| self.expr(x)).collect(), range: r(s.span) }),
            js::Expression::UnaryExpression(u) => Expr::UnaryOp(ExprUnaryOp {
                op: match u.operator {
                    U::UnaryPlus => UnaryOp::UAdd,
                    U::UnaryNegation => UnaryOp::USub,
                    U::LogicalNot => UnaryOp::Bang,
                    U::BitwiseNot => UnaryOp::Invert,
                    U::Typeof => UnaryOp::TypeOf,
                    U::Void => UnaryOp::Void,
                    U::Delete => UnaryOp::Delete,
                },
                operand: Box::new(self.expr(&u.argument)),
                range: r(u.span),
            }),
            js::Expression::UpdateExpression(u) => self.simple_target(&u.argument),
            js::Expression::YieldExpression(y) => match (&y.argument, y.delegate) {
                (Some(a), true) => Expr::YieldFrom(ExprInner { value: Box::new(self.expr(a)), range: r(y.span) }),
                (a, _) => Expr::Yield(ExprYield { value: a.as_ref().map(|a| Box::new(self.expr(a))), range: r(y.span) }),
            },
            js::Expression::PrivateInExpression(p) => Expr::Compare(ExprCompare { ops: vec![CmpOp::In], operands: vec![string(&format!("#{}", p.left.name), p.left.span), self.expr(&p.right)], range: r(p.span) }),
            js::Expression::ImportMeta(m) => Expr::Attribute(ExprAttribute { value: Box::new(name_expr("import", m.span, Ctx::Load)), attr: "meta".into(), range: r(m.span) }),
            js::Expression::NewTarget(n) => Expr::Other(atom(n.span)),
            js::Expression::JSXElement(x) => self.jsx_element(x),
            js::Expression::JSXFragment(f) => self.jsx_fragment(f),
            js::Expression::TSAsExpression(x) => self.expr(&x.expression),
            js::Expression::TSSatisfiesExpression(x) => self.expr(&x.expression),
            js::Expression::TSTypeAssertion(x) => self.expr(&x.expression),
            js::Expression::TSNonNullExpression(x) => self.expr(&x.expression),
            js::Expression::TSInstantiationExpression(x) => self.expr(&x.expression),
            js::Expression::V8IntrinsicExpression(x) => Expr::Other(atom(x.span)),
            // `exports.run(…)` inside a CommonJS module calls its own `run`.
            js::Expression::StaticMemberExpression(m) if is_exports(&m.object) => name_expr(m.property.name.as_str(), m.span, Ctx::Load),
            other => match other.as_member_expression() {
                Some(m) => self.member(m),
                None => Expr::Other(atom(other.span())),
            },
        }
    }

    fn import_call(&self, i: &js::ImportExpression) -> Expr {
        Expr::Call(ExprCall { func: Box::new(name_expr("import", Span::new(i.span.start, i.span.start + 6), Ctx::Load)), arguments: Arguments { args: vec![self.expr(&i.source)], keywords: vec![] }, kind: CallKind::Call, range: r(i.span) })
    }

    fn template(&self, t: &js::TemplateLiteral, tag: Option<&js::Expression>) -> Expr {
        let mut parts = vec![];
        for (i, q) in t.quasis.iter().enumerate() {
            let text = q.value.cooked.as_ref().map(|c| c.as_str()).unwrap_or(q.value.raw.as_str());
            if !text.is_empty() {
                parts.push(TemplatePart::Literal(text.to_string()));
            }
            if let Some(e) = t.expressions.get(i) {
                parts.push(TemplatePart::Hole { expr: self.expr(e), spec: vec![] });
            }
        }
        let range = match tag {
            Some(tag) => r(Span::new(tag.span().start, t.span.end)),
            None => r(t.span),
        };
        Expr::Template(ExprTemplate { kind: TemplateKind::Format, parts, tag: tag.map(|x| Box::new(self.expr(x))), range })
    }

    // ----- JSX ----------------------------------------------------------------------------

    fn jsx_name(&self, n: &js::JSXElementName) -> Expr {
        match n {
            // Lower-case names are intrinsic elements (`div`), not variables.
            js::JSXElementName::Identifier(i) => string(i.name.as_str(), i.span),
            js::JSXElementName::IdentifierReference(i) => name_expr(i.name.as_str(), i.span, Ctx::Load),
            js::JSXElementName::NamespacedName(x) => string(&format!("{}:{}", x.namespace.name, x.name.name), x.span),
            js::JSXElementName::MemberExpression(m) => self.jsx_member(m),
            js::JSXElementName::ThisExpression(t) => name_expr("this", t.span, Ctx::Load),
        }
    }

    fn jsx_member(&self, m: &js::JSXMemberExpression) -> Expr {
        let object = match &m.object {
            js::JSXMemberExpressionObject::IdentifierReference(i) => name_expr(i.name.as_str(), i.span, Ctx::Load),
            js::JSXMemberExpressionObject::MemberExpression(inner) => self.jsx_member(inner),
            js::JSXMemberExpressionObject::ThisExpression(t) => name_expr("this", t.span, Ctx::Load),
        };
        Expr::Attribute(ExprAttribute { value: Box::new(object), attr: m.property.name.to_string(), range: r(m.span) })
    }

    fn jsx_children(&self, children: &[js::JSXChild]) -> Vec<Expr> {
        children
            .iter()
            .filter_map(|c| match c {
                js::JSXChild::Text(t) => {
                    let s = t.value.as_str().split_whitespace().collect::<Vec<_>>().join(" ");
                    (!s.is_empty()).then(|| string(&s, t.span))
                }
                js::JSXChild::Element(e) => Some(self.jsx_element(e)),
                js::JSXChild::Fragment(f) => Some(self.jsx_fragment(f)),
                js::JSXChild::ExpressionContainer(x) => x.expression.as_expression().map(|e| self.expr(e)),
                js::JSXChild::Spread(s) => Some(Expr::Starred(ExprInner { value: Box::new(self.expr(&s.expression)), range: r(s.span) })),
            })
            .collect()
    }

    fn jsx_element(&self, x: &js::JSXElement) -> Expr {
        let open = &x.opening_element;
        let keywords = open
            .attributes
            .iter()
            .filter_map(|a| match a {
                js::JSXAttributeItem::SpreadAttribute(s) => Some(Keyword { arg: None, value: self.expr(&s.argument), range: r(s.span) }),
                js::JSXAttributeItem::Attribute(attr) => {
                    let name = match &attr.name {
                        js::JSXAttributeName::Identifier(i) => i.name.to_string(),
                        js::JSXAttributeName::NamespacedName(n) => format!("{}:{}", n.namespace.name, n.name.name),
                    };
                    let value = match &attr.value {
                        None => Expr::BooleanLiteral(ExprBooleanLiteral { value: true, range: r(attr.span) }),
                        Some(js::JSXAttributeValue::StringLiteral(s)) => string(s.value.as_str(), s.span),
                        Some(js::JSXAttributeValue::ExpressionContainer(c)) => match c.expression.as_expression() {
                            Some(e) => self.function_value(unparen(e), &name).unwrap_or_else(|| self.expr(e)),
                            None => return None,
                        },
                        Some(js::JSXAttributeValue::Element(e)) => self.jsx_element(e),
                        Some(js::JSXAttributeValue::Fragment(f)) => self.jsx_fragment(f),
                    };
                    Some(Keyword { arg: Some(name), value, range: r(attr.span) })
                }
            })
            .collect();
        Expr::Call(ExprCall { func: Box::new(self.jsx_name(&open.name)), arguments: Arguments { args: self.jsx_children(&x.children), keywords }, kind: CallKind::Jsx, range: r(x.span) })
    }

    fn jsx_fragment(&self, f: &js::JSXFragment) -> Expr {
        let open = f.opening_fragment.span;
        Expr::Call(ExprCall { func: Box::new(string("Fragment", open)), arguments: Arguments { args: self.jsx_children(&f.children), keywords: vec![] }, kind: CallKind::Jsx, range: r(f.span) })
    }
}
