//! Objects that own functions, lowered as classes: a constructor function with
//! prototype methods (`function View() {}` and `View.prototype.render = …`), an
//! object with functions attached to it (`var app = {}` and `app.listen =
//! function …`, or `Object.create(proto)`), and an object literal of methods.
//! Their functions see the object as `this`, so `this.set(…)` inside
//! `app.init` is a call of `app.set`.

use super::*;
use std::collections::{HashMap, HashSet};

/// What a module-level name is defined as.
enum Base<'b, 'a> {
    /// `function View(name) {…}` or `var View = function (name) {…}`.
    Constructor(&'b js::Function<'a>),
    /// `{…}` or `Object.create(proto)`.
    Object { literal: Option<&'b js::ObjectExpression<'a>>, proto: Option<&'b js::Expression<'a>> },
}

/// A function attached to an owner after its definition.
struct Member<'b, 'a> {
    name: String,
    value: &'b js::Expression<'a>,
    kind: js::PropertyKind,
    /// Attached to a constructor rather than its prototype.
    is_static: bool,
    /// The statement or property that defines it: its code and documentation.
    span: Span,
}

struct Owner<'b, 'a> {
    base: Base<'b, 'a>,
    members: Vec<Member<'b, 'a>>,
    /// Statements folded into the definition, by start offset.
    folded: Vec<u32>,
}

impl Owner<'_, '_> {
    /// Owns enough functions to be read as one thing.
    fn qualifies(&self) -> bool {
        let attached = self.members.iter().any(|m| !m.is_static);
        match &self.base {
            Base::Constructor(_) => attached,
            Base::Object { literal: Some(o), .. } => attached || o.properties.iter().filter(|p| matches!(p, js::ObjectPropertyKind::ObjectProperty(x) if is_function(&x.value))).count() >= 2,
            Base::Object { literal: None, .. } => attached,
        }
    }
}

pub(super) struct Owners<'b, 'a> {
    owners: HashMap<String, Owner<'b, 'a>>,
    folded: HashSet<u32>,
}

fn is_function(e: &js::Expression) -> bool {
    matches!(unparen(e), js::Expression::FunctionExpression(_) | js::Expression::ArrowFunctionExpression(_))
}

/// `Object.create(proto)` → `proto`.
fn object_create<'b, 'a>(e: &'b js::Expression<'a>) -> Option<&'b js::Expression<'a>> {
    let js::Expression::CallExpression(c) = unparen(e) else { return None };
    let js::Expression::StaticMemberExpression(m) = &c.callee else { return None };
    let is_create = m.property.name.as_str() == "create" && matches!(&m.object, js::Expression::Identifier(i) if i.name.as_str() == "Object");
    if !is_create || c.arguments.len() != 1 {
        return None;
    }
    c.arguments[0].as_expression()
}

/// `X.prototype` → `X`.
fn prototype_of<'b, 'a>(e: &'b js::Expression<'a>) -> Option<&'b str> {
    match unparen(e) {
        js::Expression::StaticMemberExpression(m) if m.property.name.as_str() == "prototype" => match &m.object {
            js::Expression::Identifier(i) => Some(i.name.as_str()),
            _ => None,
        },
        _ => None,
    }
}

/// The module-level names a statement defines that could own functions.
fn definitions<'b, 'a>(s: &'b js::Statement<'a>) -> Vec<(String, Base<'b, 'a>)> {
    match s {
        js::Statement::FunctionDeclaration(f) => constructor(f).into_iter().collect(),
        js::Statement::VariableDeclaration(d) => variable(d).into_iter().collect(),
        js::Statement::ExportDeclaration(x) => match &x.declaration {
            js::Declaration::FunctionDeclaration(f) => constructor(f).into_iter().collect(),
            js::Declaration::VariableDeclaration(d) => variable(d).into_iter().collect(),
            _ => vec![],
        },
        _ => vec![],
    }
}

fn constructor<'b, 'a>(f: &'b js::Function<'a>) -> Option<(String, Base<'b, 'a>)> {
    let id = f.id.as_ref()?;
    f.body.as_ref()?;
    Some((id.name.to_string(), Base::Constructor(f)))
}

fn variable<'b, 'a>(d: &'b js::VariableDeclaration<'a>) -> Option<(String, Base<'b, 'a>)> {
    let [v] = &d.declarations[..] else { return None };
    let js::BindingPattern::BindingIdentifier(id) = &v.id else { return None };
    let (_, value) = chain(v.init.as_ref()?);
    let base = match unparen(value) {
        js::Expression::FunctionExpression(f) if f.body.is_some() => Base::Constructor(f),
        js::Expression::ObjectExpression(o) => Base::Object { literal: Some(o), proto: None },
        e => Base::Object { literal: None, proto: Some(object_create(e)?) },
    };
    Some((id.name.to_string(), base))
}

impl<'b, 'a> Owners<'b, 'a> {
    pub(super) fn scan(body: &'b [js::Statement<'a>]) -> Self {
        let mut owners: HashMap<String, Owner<'b, 'a>> = HashMap::new();
        let mut defined_twice: HashSet<String> = HashSet::new();
        for s in body {
            for (name, base) in definitions(s) {
                if owners.contains_key(&name) {
                    defined_twice.insert(name.clone());
                }
                owners.entry(name).or_insert(Owner { base, members: vec![], folded: vec![] });
            }
        }
        for s in body {
            let js::Statement::ExpressionStatement(x) = s else { continue };
            let (targets, value) = chain(&x.expression);
            if targets.is_empty() {
                continue;
            }
            // `app = …` rebinds the name: it is not one object.
            for t in &targets {
                if let js::AssignmentTarget::AssignmentTargetIdentifier(i) = t {
                    defined_twice.insert(i.name.to_string());
                }
            }
            // Every target must be a slot of the same owner: `X.prototype.m`,
            // `X.prototype` (given an object of methods) or `X.m`.
            let mut owner: Option<&str> = None;
            let mut props: Vec<(String, bool)> = vec![];
            let mut whole_prototype = false;
            let ok = targets.iter().all(|t| {
                let Some(m) = t.as_member_expression() else { return false };
                let (name, prop, is_static) = match (m.object(), m.static_property_name()) {
                    (o, Some(p)) if prototype_of(o).is_some() => (prototype_of(o).unwrap(), Some(p.to_string()), false),
                    (js::Expression::Identifier(i), Some("prototype")) => (i.name.as_str(), None, false),
                    (js::Expression::Identifier(i), Some(p)) => {
                        let is_static = matches!(owners.get(i.name.as_str()).map(|o| &o.base), Some(Base::Constructor(_)));
                        (i.name.as_str(), Some(p.to_string()), is_static)
                    }
                    _ => return false,
                };
                if owner.is_some_and(|o| o != name) || !owners.contains_key(name) {
                    return false;
                }
                owner = Some(name);
                match prop {
                    Some(p) => props.push((p, is_static)),
                    None => whole_prototype = true,
                }
                true
            });
            let Some(name) = owner.filter(|_| ok) else { continue };
            let value = unparen(value);
            let entry = owners.get_mut(name).unwrap();
            if whole_prototype {
                // `View.prototype = { render() {…}, … }`
                let (js::Expression::ObjectExpression(o), true) = (value, props.is_empty() && matches!(entry.base, Base::Constructor(_))) else { continue };
                for p in &o.properties {
                    if let js::ObjectPropertyKind::ObjectProperty(p) = p {
                        if is_function(&p.value) {
                            if let Some(key) = p.key.static_name() {
                                entry.members.push(Member { name: key.to_string(), value: &p.value, kind: p.kind, is_static: false, span: p.span });
                            }
                        }
                    }
                }
                entry.folded.push(x.span.start);
                continue;
            }
            if !is_function(value) {
                continue;
            }
            // `req.get = req.header = function header(…)`: one function, named
            // by itself when it is one of the names.
            let own = match value {
                js::Expression::FunctionExpression(f) => f.id.as_ref().map(|i| i.name.to_string()),
                _ => None,
            };
            let (prop, is_static) = props.iter().find(|(p, _)| Some(p) == own.as_ref()).cloned().unwrap_or_else(|| props[0].clone());
            entry.members.push(Member { name: prop, value, kind: js::PropertyKind::Init, is_static, span: x.span });
            entry.folded.push(x.span.start);
        }
        owners.retain(|name, o| !defined_twice.contains(name) && o.qualifies());
        let folded = owners.values().flat_map(|o| o.folded.iter().copied()).collect();
        Owners { owners, folded }
    }

    /// The statement attaches a function to an owner, which already holds it.
    pub(super) fn folds(&self, s: &js::Statement) -> bool {
        self.folded.contains(&s.span().start)
    }
}

impl Lower<'_> {
    /// The statement defining an owner, lowered as its class (with its export).
    pub(super) fn owner_definition(&self, s: &js::Statement, owners: &Owners) -> Option<Vec<Stmt>> {
        let span = s.span();
        let (name, exported) = match s {
            js::Statement::FunctionDeclaration(f) => (f.id.as_ref()?.name.to_string(), false),
            js::Statement::VariableDeclaration(d) => (variable_name(d)?, false),
            js::Statement::ExportDeclaration(x) => match &x.declaration {
                js::Declaration::FunctionDeclaration(f) => (f.id.as_ref()?.name.to_string(), true),
                js::Declaration::VariableDeclaration(d) => (variable_name(d)?, true),
                _ => return None,
            },
            _ => return None,
        };
        let owner = owners.owners.get(&name)?;
        let mut out = vec![Stmt::ClassDef(self.owner_class(&name, owner, span))];
        // `var app = exports = module.exports = {}` also exports it.
        let declaration = match s {
            js::Statement::VariableDeclaration(d) => Some(&**d),
            js::Statement::ExportDeclaration(x) => match &x.declaration {
                js::Declaration::VariableDeclaration(d) => Some(&**d),
                _ => None,
            },
            _ => None,
        };
        if let Some(init) = declaration.and_then(|d| d.declarations[0].init.as_ref()) {
            let (targets, _) = chain(init);
            out.extend(self.also_assigned(&targets, &name, span));
        }
        if exported {
            out.push(Stmt::Export(StmtExport { names: vec![(name.clone(), name)], range: end(span) }));
        }
        Some(out)
    }

    fn owner_class(&self, name: &str, owner: &Owner, span: Span) -> ClassDef {
        let mut body = vec![];
        let mut bases = vec![];
        let kind = match &owner.base {
            Base::Constructor(f) => {
                // The function is the constructor: it sets up each instance.
                let ctor_span = Span::new(f.params.span.start, f.span.end);
                let mut ctor = self.function(f, "constructor", ctor_span, DefKind::Constructor, true, &[]);
                // The comment above `function View` describes the class.
                ctor.doc = Some(String::new());
                body.push(Stmt::FunctionDef(ctor));
                ClassKind::Class
            }
            Base::Object { literal, proto } => {
                // `Object.create(http.IncomingMessage.prototype)` builds on `IncomingMessage`.
                if let Some(p) = proto {
                    bases.push(match unparen(p) {
                        js::Expression::StaticMemberExpression(m) if m.property.name.as_str() == "prototype" => self.expr(&m.object),
                        e => self.expr(e),
                    });
                }
                for p in literal.iter().flat_map(|o| o.properties.iter()) {
                    let js::ObjectPropertyKind::ObjectProperty(p) = p else { continue };
                    let key = self.key_name(&p.key);
                    if is_function(&p.value) {
                        body.push(self.owned_member(&Member { name: key, value: &p.value, kind: p.kind, is_static: false, span: p.span }));
                    } else {
                        body.push(Stmt::Assign(StmtAssign { targets: vec![name_expr(&key, p.key.span(), Ctx::Store)], value: Box::new(self.expr(&p.value)), range: r(p.span) }));
                    }
                }
                ClassKind::Object
            }
        };
        body.extend(owner.members.iter().map(|m| self.owned_member(m)));
        ClassDef { name: name.to_string(), bases, keywords: vec![], body, decorators: vec![], kind, doc: self.doc_before(span.start), range: r(span) }
    }

    fn owned_member(&self, m: &Member) -> Stmt {
        let kind = match (m.is_static, m.kind) {
            (true, _) => DefKind::StaticMethod,
            (_, js::PropertyKind::Get) => DefKind::Getter,
            (_, js::PropertyKind::Set) => DefKind::Setter,
            _ => DefKind::Method,
        };
        Stmt::FunctionDef(match unparen(m.value) {
            js::Expression::FunctionExpression(f) => self.function(f, &m.name, m.span, kind, !m.is_static, &[]),
            // An arrow has no `this` of its own: a member without a receiver.
            js::Expression::ArrowFunctionExpression(a) => self.arrow(a, &m.name, m.span, DefKind::StaticMethod, false),
            _ => unreachable!(),
        })
    }
}

fn variable_name(d: &js::VariableDeclaration) -> Option<String> {
    match &d.declarations[..] {
        [v] => match &v.id {
            js::BindingPattern::BindingIdentifier(id) => Some(id.name.to_string()),
            _ => None,
        },
        _ => None,
    }
}
