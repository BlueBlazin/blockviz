//! go/ast's tree → the shared syntax tree.
//!
//! Most Go maps onto shapes the shared tree has; what Go adds goes to the
//! nearest one:
//! - a struct is a class of its fields, with the methods declared for it in
//!   the same file; methods declared in another file are an extension of the
//!   type; embedded fields are bases; an interface lists method signatures;
//! - a function's results are its return value, an `error` result aside:
//!   `return nil, err` is an error raised, and a check that only passes an
//!   error on (`if err != nil { return …, err }`) is left out with the `err`
//!   it checks, as Rust's `?` is;
//! - `go f()` and `defer f()` are blocks marked with when they run; deferred
//!   cleanup (`defer f.Close()`, `defer mu.Unlock()`) is left out;
//! - receiving from a channel (`<-ch`) waits for a value, like `await`; a
//!   send is a call of `send`;
//! - a switch is a match (without a tag, an `if`/`else if` chain), a type
//!   switch a match on types that binds the value, a `select` a match on the
//!   channel operation that happens first;
//! - `append`, conversions, `&`, `*` and type assertions are glue; composite
//!   literals are constructor calls, lists and dicts; `fmt.Sprintf` and the
//!   printing and error functions take templates.

use super::ast::{self, Token};
use crate::code::syntax::*;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

/// A package this file imports.
#[derive(Clone, Debug)]
pub struct Import {
    /// The import path.
    pub path: String,
    /// The project file standing for the package, when it is the project's.
    pub file: Option<String>,
    /// What the package is called where it is imported without a name.
    pub name: String,
}

pub struct Lower<'s> {
    pub text: &'s str,
    pub path: &'s str,
    /// Imported packages by the name code uses for them.
    pub imports: HashMap<String, Import>,
    /// Types this file declares: methods for them are drawn inside them.
    pub types: HashSet<String>,
    /// Interfaces of the package, by name: what a value of one may be.
    pub test_file: bool,
    depth: Cell<u32>,
    /// The function being lowered: its named results and whether the last
    /// result is an `error`.
    results: RefCell<Vec<Results>>,
    /// Errors known to be set here (inside `if err != nil { … }`).
    failed: RefCell<Vec<String>>,
    /// Route groups by variable: `api := r.Group("/api")` serves under `/api`.
    prefixes: RefCell<HashMap<String, String>>,
}

/// Methods that register a handler for a path, by router library: gin and
/// echo (`GET`), chi and fiber (`Get`), net/http and gorilla (`HandleFunc`).
const ROUTE_METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "Any", "Get", "Post", "Put", "Patch", "Delete", "Head", "Options", "All", "Handle", "HandleFunc", "Method", "MethodFunc", "Static", "File"];

/// `/api` + `/users` → `/api/users`.
fn join_route(prefix: &str, path: &str) -> String {
    if prefix.is_empty() {
        return path.to_string();
    }
    if path.is_empty() || path == "/" {
        return prefix.to_string();
    }
    format!("{}/{}", prefix.trim_end_matches('/'), path.trim_start_matches('/'))
}

#[derive(Clone, Default)]
struct Results {
    names: Vec<String>,
    count: usize,
    error: bool,
}

/// Deeper expressions (a generated 7,000-term string concatenation) are
/// kept as text: everything downstream recurses over the tree.
const MAX_DEPTH: u32 = 200;

/// Methods that release what was acquired: deferred, they are cleanup.
const CLEANUP: &[&str] = &["Close", "Unlock", "RUnlock", "Done", "Stop", "Release", "Flush", "Cancel", "Wait", "Sync", "Rollback", "Finish", "End", "cancel", "stop", "done", "close", "unlock", "cleanup", "Cleanup", "Shutdown", "Reset", "Free", "Remove", "RemoveAll"];

/// Builtin types: calling one converts.
const BASIC_TYPES: &[&str] = &["string", "int", "int8", "int16", "int32", "int64", "uint", "uint8", "uint16", "uint32", "uint64", "uintptr", "byte", "rune", "float32", "float64", "complex64", "complex128", "bool", "any", "error"];

/// Go's builtin functions.
pub const BUILTINS: &[&str] = &["append", "cap", "clear", "close", "complex", "copy", "delete", "imag", "len", "make", "max", "min", "new", "panic", "print", "println", "real", "recover"];

fn cover(a: TextRange, b: TextRange) -> TextRange {
    TextRange::new(a.start().min(b.start()), a.end().max(b.end()))
}

fn atom(r: TextRange) -> Atom {
    Atom { range: r }
}

fn name(id: &str, r: TextRange, ctx: Ctx) -> Expr {
    Expr::Name(ExprName { id: id.to_string(), ctx, range: r })
}

fn call(func: Expr, args: Vec<Expr>, range: TextRange) -> Expr {
    Expr::Call(ExprCall { func: Box::new(func), arguments: Arguments { args, keywords: vec![] }, kind: CallKind::Call, range })
}

fn ident(e: &ast::Expr) -> Option<&str> {
    match e {
        ast::Expr::Ident(i) => Some(i.name.as_str()),
        ast::Expr::Paren(p) => ident(&p.x),
        _ => None,
    }
}

fn unparen(e: &ast::Expr) -> &ast::Expr {
    match e {
        ast::Expr::Paren(p) => unparen(&p.x),
        e => e,
    }
}

/// `pkg.Name` as (`pkg`, `Name`).
fn selector(e: &ast::Expr) -> Option<(&str, &str)> {
    match unparen(e) {
        ast::Expr::Selector(s) => Some((ident(&s.x)?, s.sel.name.as_str())),
        _ => None,
    }
}

/// The name of the type a receiver or embedded field names: `*Server`,
/// `List[T]`, `pkg.Type` → `Server`, `List`, `Type`.
pub fn type_name(e: &ast::Expr) -> Option<&str> {
    match e {
        ast::Expr::Ident(i) => Some(i.name.as_str()),
        ast::Expr::Star(s) => type_name(&s.x),
        ast::Expr::Paren(p) => type_name(&p.x),
        ast::Expr::Index(i) => type_name(&i.x),
        ast::Expr::IndexList(i) => type_name(&i.x),
        ast::Expr::Selector(s) => Some(s.sel.name.as_str()),
        _ => None,
    }
}

/// The type a method is declared for.
pub fn receiver_type(f: &ast::FuncDecl) -> Option<&str> {
    let field = f.recv.as_ref()?.list.first()?;
    type_name(&field.typ)
}

fn is_nil(e: &ast::Expr) -> bool {
    ident(e) == Some("nil")
}

/// A value that says nothing: `nil`, `0`, `""`, `false`, `T{}`.
fn is_zero(e: &ast::Expr) -> bool {
    match unparen(e) {
        ast::Expr::Ident(i) => matches!(i.name.as_str(), "nil" | "false"),
        ast::Expr::BasicLit(l) => matches!(l.value.as_str(), "0" | "\"\"" | "``" | "0.0"),
        ast::Expr::CompositeLit(c) => c.elts.is_empty(),
        ast::Expr::Unary(u) if u.op == Token::And => is_zero(&u.x),
        _ => false,
    }
}

/// Whether an expression mentions a name.
fn mentions(e: &ast::Expr, n: &str) -> bool {
    let mut found = false;
    super::walk::inspect(e.into(), |node| {
        if let super::walk::Node::Ident(i) = node {
            if i.name == n {
                found = true;
            }
        }
        !found
    });
    found
}

fn stmt_mentions(s: &ast::Stmt, n: &str) -> bool {
    let mut found = false;
    super::walk::inspect(s.into(), |node| {
        if let super::walk::Node::Ident(i) = node {
            if i.name == n {
                found = true;
            }
        }
        !found
    });
    found
}

/// The first sentence-paragraph of a doc comment.
pub fn doc_text(g: Option<&ast::CommentGroup>) -> Option<String> {
    let text = g?.text();
    let mut para: Vec<&str> = vec![];
    for l in text.lines() {
        let l = l.trim();
        if l.is_empty() {
            if para.is_empty() {
                continue;
            }
            break;
        }
        para.push(l);
    }
    let out = para.join(" ");
    (!out.is_empty()).then_some(out)
}

/// The value of a Go string or rune literal.
pub fn unquote(lit: &str) -> String {
    if let Some(raw) = lit.strip_prefix('`') {
        return raw.strip_suffix('`').unwrap_or(raw).to_string();
    }
    let inner = lit.get(1..lit.len().saturating_sub(1)).unwrap_or("");
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(e) = chars.next() else { break };
        let hex = |chars: &mut std::iter::Peekable<std::str::Chars>, n: usize| -> Option<char> {
            let s: String = (0..n).filter_map(|_| chars.next()).collect();
            u32::from_str_radix(&s, 16).ok().and_then(char::from_u32)
        };
        match e {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            'a' => out.push('\u{7}'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'v' => out.push('\u{b}'),
            'x' => out.extend(hex(&mut chars, 2)),
            'u' => out.extend(hex(&mut chars, 4)),
            'U' => out.extend(hex(&mut chars, 8)),
            '0'..='7' => {
                let s: String = std::iter::once(e).chain((0..2).filter_map(|_| chars.next())).collect();
                out.extend(u32::from_str_radix(&s, 8).ok().and_then(char::from_u32));
            }
            other => out.push(other),
        }
    }
    out
}

/// A printf-style format string's pieces: text, and `%v` holes (`%%` is a percent sign).
fn format_pieces(f: &str) -> Vec<Option<String>> {
    let mut out: Vec<Option<String>> = vec![];
    let mut text = String::new();
    let mut chars = f.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            text.push(c);
            continue;
        }
        if chars.peek() == Some(&'%') {
            chars.next();
            text.push('%');
            continue;
        }
        // Flags, width, precision and an explicit index, then the verb.
        while chars.peek().is_some_and(|c| "+-# 0123456789.*[]".contains(*c)) {
            chars.next();
        }
        chars.next();
        if !text.is_empty() {
            out.push(Some(std::mem::take(&mut text)));
        }
        out.push(None);
    }
    if !text.is_empty() {
        out.push(Some(text));
    }
    out
}

fn op_of(t: Token) -> Option<Operator> {
    Some(match t {
        Token::Add | Token::AddAssign => Operator::Add,
        Token::Sub | Token::SubAssign => Operator::Sub,
        Token::Mul | Token::MulAssign => Operator::Mult,
        Token::Quo | Token::QuoAssign => Operator::Div,
        Token::Rem | Token::RemAssign => Operator::Mod,
        Token::And | Token::AndAssign | Token::AndNot | Token::AndNotAssign => Operator::BitAnd,
        Token::Or | Token::OrAssign => Operator::BitOr,
        Token::Xor | Token::XorAssign => Operator::BitXor,
        Token::Shl | Token::ShlAssign => Operator::LShift,
        Token::Shr | Token::ShrAssign => Operator::RShift,
        _ => return None,
    })
}

impl<'s> Lower<'s> {
    pub fn new(text: &'s str, path: &'s str, f: &ast::File, imports: HashMap<String, Import>) -> Self {
        let mut types = HashSet::new();
        for d in &f.decls {
            if let ast::Decl::Gen(g) = d {
                for s in &g.specs {
                    if let ast::Spec::Type(t) = s {
                        types.insert(t.name.name.clone());
                    }
                }
            }
        }
        let test_file = path.ends_with("_test.go");
        Lower { text, path, imports, types, test_file, depth: Cell::new(0), results: RefCell::new(vec![]), failed: RefCell::new(vec![]), prefixes: RefCell::new(HashMap::new()) }
    }

    fn text_of(&self, r: TextRange) -> &str {
        self.text.get(usize::from(r.start())..usize::from(r.end())).unwrap_or("")
    }

    // ----- declarations ---------------------------------------------------------------

    pub fn file(&self, f: &ast::File) -> Vec<Stmt> {
        let mut out = vec![];
        for spec in f.imports() {
            out.extend(self.import(spec));
        }
        // Methods by the type they are for, in file order.
        let mut methods: HashMap<&str, Vec<&ast::FuncDecl>> = HashMap::new();
        for d in &f.decls {
            if let ast::Decl::Func(fd) = d {
                if let Some(t) = receiver_type(fd) {
                    methods.entry(t).or_default().push(fd);
                }
            }
        }
        let mut extended: HashSet<&str> = HashSet::new();
        for d in &f.decls {
            match d {
                ast::Decl::Gen(g) if g.tok == Token::Import => {}
                ast::Decl::Gen(g) => out.extend(self.gen_decl(g, &methods)),
                ast::Decl::Func(fd) => match receiver_type(fd) {
                    Some(t) if self.types.contains(t) => {}
                    Some(t) => {
                        if extended.insert(t) {
                            out.extend(self.extension(t, &methods[t]));
                        }
                    }
                    None => out.push(Stmt::FunctionDef(self.func(fd))),
                },
                ast::Decl::Bad(_) => {}
            }
        }
        out
    }

    fn import(&self, spec: &ast::ImportSpec) -> Vec<Stmt> {
        let path = unquote(&spec.path.value);
        let local = spec.name.as_ref().map(|n| n.name.clone());
        let range = spec.range;
        let file = self.imports.values().find(|i| i.path == path).and_then(|i| i.file.clone());
        match local.as_deref() {
            // `import _ "github.com/lib/pq"` registers a driver: the package is used, nothing is named.
            Some("_") => vec![Stmt::Import(StmtImport { names: vec![Alias { name: path, asname: Some("_".into()), range }], style: ModuleStyle::Path, resolved: file, declaration: false, range })],
            Some(".") => vec![Stmt::ImportFrom(StmtImportFrom { module: Some(path), names: vec![Alias { name: "*".into(), asname: None, range }], level: 0, style: ModuleStyle::Path, resolved: file, range })],
            _ => {
                // A file may import a package twice (`tests` and `. tests`).
                let local = local.unwrap_or_else(|| self.imports.values().find(|i| i.path == path).map(|i| i.name.clone()).unwrap_or_else(|| super::resolve::assumed_name(&path)));
                vec![Stmt::Import(StmtImport { names: vec![Alias { name: path, asname: Some(local), range }], style: ModuleStyle::Path, resolved: file, declaration: false, range })]
            }
        }
    }

    fn gen_decl(&self, g: &ast::GenDecl, methods: &HashMap<&str, Vec<&ast::FuncDecl>>) -> Vec<Stmt> {
        let grouped = g.lparen.is_some();
        let mut out = vec![];
        // In a const group a spec without values repeats the one before (`iota`).
        let mut last_type: Option<&ast::Expr> = None;
        for spec in &g.specs {
            match spec {
                ast::Spec::Type(t) => {
                    let range = if grouped { t.range } else { g.range };
                    let doc = doc_text(t.doc.as_ref().or(if grouped { None } else { g.doc.as_ref() }));
                    out.push(self.type_spec(t, methods.get(t.name.name.as_str()), doc, range));
                }
                ast::Spec::Value(v) => {
                    if v.typ.is_some() || !v.values.is_empty() {
                        last_type = v.typ.as_ref();
                    }
                    let range = if grouped { v.range } else { g.range };
                    out.extend(self.value_spec(v, if v.values.is_empty() && g.tok == Token::Const { last_type } else { v.typ.as_ref() }, range));
                }
                ast::Spec::Import(_) => {}
            }
        }
        out
    }

    /// `var a, b T = x, y`: one declaration per name.
    fn value_spec(&self, v: &ast::ValueSpec, typ: Option<&ast::Expr>, range: TextRange) -> Vec<Stmt> {
        let mut out = vec![];
        // `a, b := f()`: several names from one call.
        if v.values.len() == 1 && v.names.len() > 1 {
            let targets = Expr::Tuple(ExprSeq { elts: v.names.iter().map(|n| name(&n.name, n.range, Ctx::Store)).collect(), range: cover(v.names[0].range, v.names[v.names.len() - 1].range) });
            return vec![Stmt::Assign(StmtAssign { targets: vec![targets], value: Box::new(self.expr(&v.values[0])), range })];
        }
        for (i, n) in v.names.iter().enumerate() {
            if n.name == "_" {
                if let Some(value) = v.values.get(i) {
                    out.extend(self.discard(value, range));
                }
                continue;
            }
            let target = Box::new(name(&n.name, n.range, Ctx::Store));
            let value = v.values.get(i).map(|e| Box::new(self.expr(e)));
            out.push(match (typ, value) {
                (Some(t), value) => Stmt::AnnAssign(StmtAnnAssign { target, annotation: Box::new(self.type_expr(t)), value, range }),
                (None, Some(value)) => Stmt::Assign(StmtAssign { targets: vec![*target], value, range }),
                (None, None) => continue,
            });
        }
        out
    }

    fn type_spec(&self, t: &ast::TypeSpec, methods: Option<&Vec<&ast::FuncDecl>>, doc: Option<String>, range: TextRange) -> Stmt {
        let name_ = t.name.name.clone();
        let methods: Vec<Stmt> = methods.into_iter().flatten().map(|f| Stmt::FunctionDef(self.func(f))).collect();
        match unparen(&t.typ) {
            ast::Expr::StructType(s) if t.assign.is_none() => {
                let (mut body, bases, decorators) = self.struct_fields(&s.fields);
                body.extend(methods);
                Stmt::ClassDef(ClassDef { name: name_, bases, keywords: vec![], body, decorators, kind: ClassKind::Class, doc, range })
            }
            ast::Expr::InterfaceType(i) if t.assign.is_none() => {
                let mut body = vec![];
                let mut bases = vec![];
                for f in &i.methods.list {
                    match (&f.names[..], &f.typ) {
                        ([n, ..], ast::Expr::FuncType(ft)) => body.push(Stmt::FunctionDef(FunctionDef {
                            name: n.name.clone(),
                            params: self.params(&ft.params),
                            body: vec![],
                            decorators: vec![],
                            returns: self.returns(ft.results.as_ref()),
                            is_async: false,
                            is_generator: false,
                            kind: DefKind::Method,
                            doc: doc_text(f.doc.as_ref()),
                            range: f.range,
                        })),
                        // An embedded interface; a constraint's type terms say nothing more.
                        ([], typ) if type_name(typ).is_some() => bases.push(self.type_expr(typ)),
                        _ => {}
                    }
                }
                body.extend(methods);
                Stmt::ClassDef(ClassDef { name: name_, bases, keywords: vec![], body, decorators: vec![], kind: ClassKind::Interface, doc, range })
            }
            // A named type with methods (`type HandlerFunc func(…)`, `type Kind int`).
            _ if !methods.is_empty() => Stmt::ClassDef(ClassDef { name: name_, bases: vec![], keywords: vec![], body: methods, decorators: vec![], kind: ClassKind::Class, doc, range }),
            typ => Stmt::TypeAlias(StmtTypeAlias { name: Box::new(name(&name_, t.name.range, Ctx::Store)), value: Box::new(self.type_expr(typ)), range }),
        }
    }

    /// Fields as declarations on `self`; embedded types are bases (their
    /// methods are the struct's); tags say how values are stored.
    fn struct_fields(&self, fields: &ast::FieldList) -> (Vec<Stmt>, Vec<Expr>, Vec<Decorator>) {
        let mut body = vec![];
        let mut bases = vec![];
        let mut tags: Vec<&str> = vec![];
        for f in &fields.list {
            if let Some(tag) = &f.tag {
                for part in unquote(&tag.value).split_whitespace() {
                    if let Some((key, _)) = part.split_once(':') {
                        tags.push(match key {
                            "json" | "yaml" | "xml" | "toml" | "msgpack" | "mapstructure" | "protobuf" | "bson" => "serializable",
                            "db" | "gorm" | "bun" | "pg" | "sql" | "ent" => "DB row",
                            "flag" | "arg" | "short" | "long" | "env" | "cli" => "arguments",
                            _ => continue,
                        });
                    }
                }
            }
            let annotation = self.type_expr(&f.typ);
            if f.names.is_empty() {
                // `gorm.Model` embedded makes a table row.
                if let Some(("gorm", "Model")) = selector(strip_star(&f.typ)) {
                    tags.push("DB row");
                }
                let key = type_name(&f.typ).unwrap_or("_").to_string();
                // An embedded `int` is a field; an embedded type lends its methods.
                if !BASIC_TYPES.contains(&key.as_str()) {
                    bases.push(annotation.clone());
                }
                let at = f.typ.range();
                let target = Expr::Attribute(ExprAttribute { value: Box::new(name("self", at, Ctx::Load)), attr: key, range: at });
                body.push(Stmt::AnnAssign(StmtAnnAssign { target: Box::new(target), annotation: Box::new(annotation), value: None, range: f.range }));
                continue;
            }
            for n in &f.names {
                let target = Expr::Attribute(ExprAttribute { value: Box::new(name("self", n.range, Ctx::Load)), attr: n.name.clone(), range: n.range });
                body.push(Stmt::AnnAssign(StmtAnnAssign { target: Box::new(target), annotation: Box::new(annotation.clone()), value: None, range: cover(n.range, f.typ.range()) }));
            }
        }
        let mut decorators = vec![];
        let mut seen = HashSet::new();
        for t in tags {
            if seen.insert(t) {
                decorators.push(self.badge(t, fields.range));
            }
        }
        // A row of a table is also stored as JSON often; the table says more.
        if seen.contains("DB row") {
            decorators.retain(|d| !matches!(&d.expression, Expr::Call(c) if matches!(c.arguments.args.first(), Some(Expr::StringLiteral(s)) if s.value == "serializable")));
        }
        (body, bases, decorators)
    }

    /// A badge the front end knows (`test`, `serializable`), as a decorator.
    fn badge(&self, text: &str, range: TextRange) -> Decorator {
        let at = TextRange::empty(range.start());
        Decorator { expression: call(name("badge", at, Ctx::Load), vec![Expr::StringLiteral(ExprStringLiteral { value: text.to_string(), range: at })], at), range: at }
    }

    /// Methods of a type declared in another file of the package.
    fn extension(&self, ty: &str, methods: &[&ast::FuncDecl]) -> Vec<Stmt> {
        let body: Vec<Stmt> = methods.iter().map(|f| Stmt::FunctionDef(self.func(f))).collect();
        let first = methods[0];
        let target = first.recv.as_ref().and_then(|r| r.list.first()).map(|f| self.type_expr(&f.typ)).unwrap_or_else(|| name(ty, first.name.range, Ctx::Load));
        // Starting where the first method's documentation does (or just
        // before it), so the group and its first method start apart.
        let start = match &first.doc {
            Some(d) if d.range.start() < first.range.start() => d.range.start(),
            _ => first.range.start() - TextSize::from(u32::from(first.range.start() > TextSize::from(0))),
        };
        let range = TextRange::new(start, methods[methods.len() - 1].range.end());
        vec![Stmt::ClassDef(ClassDef { name: ty.to_string(), bases: vec![target], keywords: vec![], body, decorators: vec![], kind: ClassKind::Extension, doc: None, range })]
    }

    // ----- functions ------------------------------------------------------------------

    fn params(&self, list: &ast::FieldList) -> Vec<Param> {
        let mut out = vec![];
        for f in &list.list {
            let variadic = matches!(f.typ, ast::Expr::Ellipsis(_));
            let annotation = match &f.typ {
                ast::Expr::Ellipsis(e) => e.elt.as_ref().map(|t| self.type_expr(t)),
                t => Some(self.type_expr(t)),
            };
            let kind = if variadic { ParamKind::Variadic } else { ParamKind::Positional };
            if f.names.is_empty() {
                out.push(Param { name: "_".into(), kind, default: None, annotation: annotation.clone(), range: f.typ.range() });
            }
            for n in &f.names {
                out.push(Param { name: n.name.clone(), kind, default: None, annotation: annotation.clone(), range: n.range });
            }
        }
        out
    }

    fn result_info(&self, results: Option<&ast::FieldList>) -> Results {
        let Some(list) = results else { return Results::default() };
        let mut names = vec![];
        let mut count = 0;
        let mut error = false;
        for f in &list.list {
            let n = f.names.len().max(1);
            count += n;
            error = ident(&f.typ) == Some("error");
            names.extend(f.names.iter().map(|n| n.name.clone()));
        }
        Results { names, count, error }
    }

    /// What a function hands back, an `error` result aside: one type, or a tuple.
    fn returns(&self, results: Option<&ast::FieldList>) -> Option<Box<Expr>> {
        let list = results?;
        let mut types: Vec<Expr> = vec![];
        for f in &list.list {
            for _ in 0..f.names.len().max(1) {
                types.push(self.type_expr(&f.typ));
            }
        }
        if list.list.last().is_some_and(|f| ident(&f.typ) == Some("error")) {
            types.pop();
        }
        match types.len() {
            0 => None,
            1 => types.pop().map(Box::new),
            _ => Some(Box::new(Expr::Tuple(ExprSeq { elts: types, range: list.range }))),
        }
    }

    fn func(&self, f: &ast::FuncDecl) -> FunctionDef {
        let mut params = vec![];
        let method = f.recv.is_some();
        if let Some(recv) = &f.recv {
            // The receiver comes first, as `self` does; an unnamed one is `_`.
            let mut p = self.params(recv);
            p.truncate(1);
            params.extend(p);
        }
        params.extend(self.params(&f.typ.params));
        let mut decorators = vec![];
        let name_ = f.name.name.as_str();
        if self.test_file && f.recv.is_none() {
            let param_type = f.typ.params.list.first().map(|p| self.text_of(p.typ.range()).trim_start_matches('*').to_string());
            let kind = |prefix: &str| name_.strip_prefix(prefix).is_some_and(|rest| rest.is_empty() || !rest.starts_with(|c: char| c.is_lowercase()));
            let badge = match param_type.as_deref() {
                Some("testing.T") if kind("Test") => Some("test"),
                Some("testing.B") if kind("Benchmark") => Some("benchmark"),
                Some("testing.F") if kind("Fuzz") => Some("fuzz test"),
                None if kind("Example") => Some("example"),
                _ => None,
            };
            if let Some(b) = badge {
                decorators.push(self.badge(b, f.range));
            }
        }
        let body = f.body.as_ref().map(|b| self.body(&f.typ, b)).unwrap_or_default();
        FunctionDef {
            name: f.name.name.clone(),
            params,
            body,
            decorators,
            returns: self.returns(f.typ.results.as_ref()),
            is_async: false,
            is_generator: false,
            kind: if method { DefKind::Method } else { DefKind::Function },
            doc: doc_text(f.doc.as_ref()),
            range: f.range,
        }
    }

    fn body(&self, typ: &ast::FuncType, b: &ast::BlockStmt) -> Vec<Stmt> {
        self.results.borrow_mut().push(self.result_info(typ.results.as_ref()));
        let out = self.block(&b.list);
        self.results.borrow_mut().pop();
        out
    }

    fn func_lit(&self, f: &ast::FuncLit) -> Expr {
        let params = self.params(&f.typ.params);
        // `func(a, b T) bool { return a.x < b.x }` is a lambda.
        if let [ast::Stmt::Return(r)] = &f.body.list[..] {
            if let [e] = &r.results[..] {
                if !matches!(unparen(e), ast::Expr::FuncLit(_)) {
                    return Expr::Lambda(ExprLambda { params, body: Box::new(self.expr(e)), range: f.range });
                }
            }
        }
        Expr::Function(Box::new(FunctionDef { name: String::new(), params, body: self.body(&f.typ, &f.body), decorators: vec![], returns: self.returns(f.typ.results.as_ref()), is_async: false, is_generator: false, kind: DefKind::Function, doc: None, range: f.range }))
    }

    // ----- statements -----------------------------------------------------------------

    fn block(&self, list: &[ast::Stmt]) -> Vec<Stmt> {
        let mut out = vec![];
        let mut i = 0;
        while i < list.len() {
            let s = &list[i];
            // `v, err := f()` then `if err != nil { return …, err }`: the call,
            // with the error passed on as `?` would.
            if let (ast::Stmt::Assign(a), Some(ast::Stmt::If(x))) = (s, list.get(i + 1)) {
                if let Some(var) = self.passes_error(x) {
                    if a.lhs.iter().any(|l| ident(l) == Some(var)) && x.else_.is_none() {
                        let used_later = list[i + 2..].iter().any(|s| stmt_mentions(s, var) && !self.only_passes(s, var));
                        out.extend(self.assign(a, if used_later { None } else { Some(var) }));
                        i += 2;
                        continue;
                    }
                }
            }
            out.extend(self.stmt(s));
            i += 1;
        }
        out
    }

    /// Whether a statement only passes an error on (or reassigns it).
    fn only_passes(&self, s: &ast::Stmt, var: &str) -> bool {
        match s {
            ast::Stmt::If(x) => self.passes_error(x) == Some(var),
            ast::Stmt::Assign(a) => a.lhs.iter().any(|l| ident(l) == Some(var)) && !a.rhs.iter().any(|r| mentions(r, var)),
            _ => false,
        }
    }

    /// `if err != nil { return …, err }` (or a wrapped error, a panic, a
    /// fatal log, an exit): the name of the error passed on.
    fn passes_error<'a>(&self, x: &'a ast::IfStmt) -> Option<&'a str> {
        let ast::Expr::Binary(b) = unparen(&x.cond) else { return None };
        if b.op != Token::Neq || !is_nil(&b.y) {
            return None;
        }
        let var = ident(&b.x)?;
        if x.body.list.is_empty() {
            return None;
        }
        let n = x.body.list.len();
        for (k, s) in x.body.list.iter().enumerate() {
            let last = k + 1 == n;
            match s {
                ast::Stmt::Return(r) if last => {
                    let results = self.results.borrow();
                    let info = results.last()?;
                    let e = r.results.last()?;
                    if !info.error || is_nil(e) || r.results.len() != info.count {
                        return None;
                    }
                    if !r.results[..r.results.len() - 1].iter().all(is_zero) {
                        return None;
                    }
                }
                ast::Stmt::Expr(e) if self.exits(&e.x) => {}
                // `fmt.Fprintln(os.Stderr, err)` before an exit; `conn.Close()` before passing the error on.
                ast::Stmt::Expr(e) if !last && (self.prints_to_stderr(&e.x) || is_cleanup(&e.x)) => {}
                ast::Stmt::Defer(_) if !last => {}
                _ => return None,
            }
        }
        Some(var)
    }

    /// `panic(…)`, `log.Fatal(…)`, `t.Fatalf(…)`, `os.Exit(1)`.
    fn exits(&self, e: &ast::Expr) -> bool {
        let ast::Expr::Call(c) = unparen(e) else { return false };
        match unparen(&c.fun) {
            ast::Expr::Ident(i) => i.name == "panic",
            ast::Expr::Selector(s) => {
                let m = s.sel.name.as_str();
                m.starts_with("Fatal") || m.starts_with("Panic") || selector(&c.fun) == Some(("os", "Exit")) || m == "FailNow" || m == "SkipNow"
            }
            _ => false,
        }
    }

    fn prints_to_stderr(&self, e: &ast::Expr) -> bool {
        let ast::Expr::Call(c) = unparen(e) else { return false };
        matches!(selector(&c.fun), Some(("fmt", f)) if f.starts_with("Fprint")) && c.args.first().and_then(selector) == Some(("os", "Stderr"))
    }

    fn stmt(&self, s: &ast::Stmt) -> Vec<Stmt> {
        let range = s.range();
        match s {
            ast::Stmt::Expr(e) => self.discard(&e.x, range),
            ast::Stmt::Assign(a) => self.assign(a, None),
            ast::Stmt::IncDec(x) => {
                let op = if x.tok == Token::Inc { Operator::Add } else { Operator::Sub };
                let one = Expr::NumberLiteral(atom(TextRange::new(x.tok_pos, range.end())));
                vec![Stmt::AugAssign(StmtAugAssign { target: Box::new(self.expr(&x.x)), op, value: Box::new(one), range })]
            }
            ast::Stmt::Send(x) => {
                let func = Expr::Attribute(ExprAttribute { value: Box::new(self.expr(&x.chan)), attr: "send".into(), range: cover(x.chan.range(), TextRange::new(x.arrow, x.arrow + TextSize::from(2))) });
                vec![Stmt::Expr(StmtExpr { value: Box::new(call(func, vec![self.expr(&x.value)], range)), range })]
            }
            ast::Stmt::Return(r) => self.ret(r),
            ast::Stmt::Block(b) => self.block(&b.list),
            ast::Stmt::Labeled(l) => self.stmt(&l.stmt),
            ast::Stmt::If(x) => self.if_stmt(x),
            ast::Stmt::Switch(x) => self.switch(x),
            ast::Stmt::TypeSwitch(x) => self.type_switch(x),
            ast::Stmt::Select(x) => self.select(x),
            ast::Stmt::For(x) => self.for_stmt(x),
            ast::Stmt::Range(x) => self.range_stmt(x),
            ast::Stmt::Go(g) => self.background(&g.call, "go", range),
            ast::Stmt::Defer(d) => {
                // Releasing what was acquired is bookkeeping, also when a
                // deferred function does only that (`defer func() { cerr = f.Close() }()`).
                let call = ast::Expr::Call(Box::new(d.call.clone()));
                if is_cleanup(&call) || is_sync_glue(&call) {
                    return vec![];
                }
                if let ast::Expr::FuncLit(f) = unparen(&d.call.fun) {
                    let mut calls = 0;
                    let mut other = false;
                    for s in &f.body.list {
                        super::walk::inspect(s.into(), |n| {
                            if let super::walk::Node::CallExpr(c) = n {
                                calls += 1;
                                let e = ast::Expr::Call(Box::new(c.clone()));
                                if !is_cleanup(&e) && !is_sync_glue(&e) {
                                    other = true;
                                }
                            }
                            !other
                        });
                    }
                    if calls > 0 && !other {
                        return vec![];
                    }
                }
                self.background(&d.call, "defer", range)
            }
            ast::Stmt::Branch(b) => match b.tok {
                Token::Break => vec![Stmt::Break(atom(range))],
                Token::Continue => vec![Stmt::Continue(atom(range))],
                _ => vec![],
            },
            ast::Stmt::Decl(d) => {
                let methods = HashMap::new();
                self.gen_decl(&d.decl, &methods)
            }
            ast::Stmt::Empty(_) | ast::Stmt::Bad(_) | ast::Stmt::CaseClause(_) | ast::Stmt::CommClause(_) => vec![],
        }
    }

    /// An expression statement: calls are steps; a receive waits.
    fn discard(&self, e: &ast::Expr, range: TextRange) -> Vec<Stmt> {
        if is_sync_glue(e) {
            return vec![];
        }
        let ast::Expr::Call(c) = unparen(e) else {
            let v = self.expr(e);
            return if matches!(v, Expr::Await(_)) { vec![Stmt::Expr(StmtExpr { value: Box::new(v), range })] } else { vec![] };
        };
        // `func() { mu.Lock(); defer mu.Unlock(); … }()`: code run in place, in
        // a scope of its own (a return in it would leave only the literal, so
        // one with a return stays a function).
        if let ast::Expr::FuncLit(f) = unparen(&c.fun) {
            let returns = f.body.list.iter().any(|s| {
                let mut found = false;
                super::walk::inspect(s.into(), |n| {
                    match n {
                        super::walk::Node::ReturnStmt(_) => found = true,
                        super::walk::Node::FuncLit(_) => return false,
                        _ => {}
                    }
                    !found
                });
                found
            });
            if !returns {
                let params = self.params(&f.typ.params);
                let mut out: Vec<Stmt> = params.iter().zip(&c.args).map(|(p, a)| Stmt::Assign(StmtAssign { targets: vec![name(&p.name, p.range, Ctx::Store)], value: Box::new(self.expr(a)), range: a.range() })).collect();
                out.extend(self.block(&f.body.list));
                return out;
            }
        }
        match ident(&c.fun) {
            // `delete(m, k)` removes an item.
            Some("delete") if c.args.len() == 2 => {
                return vec![Stmt::Delete(StmtDelete { targets: vec![Expr::Subscript(ExprSubscript { value: Box::new(self.expr(&c.args[0])), slice: Box::new(self.expr(&c.args[1])), range })], range })];
            }
            Some("panic") => return vec![self.raise_panic(c, range)],
            _ => {}
        }
        let v = self.expr(e);
        if matches!(v, Expr::Name(_)) || v.is_literal_expr() {
            return vec![];
        }
        vec![Stmt::Expr(StmtExpr { value: Box::new(v), range })]
    }

    /// `panic(err)` stops the program as an uncaught error would.
    fn raise_panic(&self, c: &ast::CallExpr, range: TextRange) -> Stmt {
        let func = name("panic", c.fun.range(), Ctx::Load);
        let exc = call(func, c.args.iter().map(|a| self.expr(a)).collect(), range);
        Stmt::Raise(StmtRaise { exc: Some(Box::new(exc)), cause: None, range })
    }

    /// `go f(x)`, `defer f(x)`: the call (or the function literal's body) in
    /// a block saying when it runs.
    fn background(&self, c: &ast::CallExpr, when: &str, range: TextRange) -> Vec<Stmt> {
        let mut body = vec![];
        match unparen(&c.fun) {
            ast::Expr::FuncLit(f) => {
                // Parameters are bound to the arguments, then the body runs.
                let params = self.params(&f.typ.params);
                for (p, a) in params.iter().zip(&c.args) {
                    body.push(Stmt::Assign(StmtAssign { targets: vec![name(&p.name, p.range, Ctx::Store)], value: Box::new(self.expr(a)), range: a.range() }));
                }
                self.results.borrow_mut().push(self.result_info(f.typ.results.as_ref()));
                body.extend(self.block(&f.body.list));
                self.results.borrow_mut().pop();
            }
            _ => body.extend(self.discard(&ast::Expr::Call(Box::new(c.clone())), c.range)),
        }
        if body.is_empty() {
            return vec![];
        }
        let at = TextRange::new(range.start(), range.start() + TextSize::from(when.len() as u32));
        vec![Stmt::With(StmtWith { items: vec![WithItem { context_expr: name(when, at, Ctx::Load), optional_vars: None, range: at }], body, is_async: false, range })]
    }

    /// `a, b := f()`, `x = y`, `x += 1`. `drop` names an error left out.
    fn assign(&self, a: &ast::AssignStmt, drop: Option<&str>) -> Vec<Stmt> {
        let range = a.range;
        // `v1 := r.Group("/v1")`, `api := r.PathPrefix("/api").Subrouter()`.
        if let ([l], [ast::Expr::Call(c)]) = (&a.lhs[..], &a.rhs[..]) {
            if let (Some(var), Some(prefix)) = (ident(l), self.group_prefix(c)) {
                self.prefixes.borrow_mut().insert(var.to_string(), prefix);
            }
        }
        if let Some(op) = op_of(a.tok) {
            if let ([l], [r]) = (&a.lhs[..], &a.rhs[..]) {
                return vec![Stmt::AugAssign(StmtAugAssign { target: Box::new(self.target(l)), op, value: Box::new(self.expr(r)), range })];
            }
        }
        // `xs = append(xs, x)` adds to the list.
        if let ([l], [ast::Expr::Call(c)]) = (&a.lhs[..], &a.rhs[..]) {
            if ident(&c.fun) == Some("append") && c.args.len() >= 2 && self.text_of(l.range()) == self.text_of(c.args[0].range()) {
                let method = if c.ellipsis.is_some() { "extend" } else { "append" };
                let func = Expr::Attribute(ExprAttribute { value: Box::new(self.expr(l)), attr: method.into(), range: cover(l.range(), c.fun.range()) });
                return vec![Stmt::Expr(StmtExpr { value: Box::new(call(func, c.args[1..].iter().map(|x| self.expr(x)).collect(), c.range)), range })];
            }
        }
        let skip = |l: &ast::Expr| ident(l) == Some("_") || (drop.is_some() && ident(l) == drop);
        if a.rhs.len() == 1 {
            let kept: Vec<&ast::Expr> = a.lhs.iter().filter(|l| !skip(l)).collect();
            let value = &a.rhs[0];
            return match &kept[..] {
                [] => self.discard(value, range),
                [one] => vec![Stmt::Assign(StmtAssign { targets: vec![self.target(one)], value: Box::new(self.expr(value)), range })],
                many => {
                    let r = cover(many[0].range(), many[many.len() - 1].range());
                    vec![Stmt::Assign(StmtAssign { targets: vec![Expr::Tuple(ExprSeq { elts: many.iter().map(|l| self.target(l)).collect(), range: r })], value: Box::new(self.expr(value)), range })]
                }
            };
        }
        // `a, b = b, a`: one assignment per pair.
        let mut out = vec![];
        for (l, r) in a.lhs.iter().zip(&a.rhs) {
            if ident(l) == Some("_") {
                out.extend(self.discard(r, r.range()));
                continue;
            }
            out.push(Stmt::Assign(StmtAssign { targets: vec![self.target(l)], value: Box::new(self.expr(r)), range: cover(l.range(), r.range()) }));
        }
        out
    }

    fn target(&self, e: &ast::Expr) -> Expr {
        match unparen(e) {
            ast::Expr::Ident(i) => name(&i.name, i.range, Ctx::Store),
            ast::Expr::Star(s) => self.target(&s.x),
            other => self.expr(other),
        }
    }

    fn ret(&self, r: &ast::ReturnStmt) -> Vec<Stmt> {
        let range = r.range;
        let info = self.results.borrow().last().cloned().unwrap_or_default();
        let ret = |value: Option<Expr>| vec![Stmt::Return(StmtReturn { value: value.map(Box::new), range })];
        if r.results.is_empty() {
            // A bare return hands back the named results.
            let names: Vec<&String> = info.names.iter().take(info.names.len().saturating_sub(info.error as usize)).filter(|n| *n != "_").collect();
            return match &names[..] {
                [] => ret(None),
                [n] => ret(Some(name(n, range, Ctx::Load))),
                many => ret(Some(Expr::Tuple(ExprSeq { elts: many.iter().map(|n| name(n, range, Ctx::Load)).collect(), range }))),
            };
        }
        let mut values: Vec<&ast::Expr> = r.results.iter().collect();
        // `return f()` hands back whatever f returns, its error included.
        let passthrough = values.len() == 1 && info.count > 1;
        if info.error && !passthrough {
            let err = values.pop().unwrap();
            // An error made here (`errors.New(…)`, `ErrClosed`), or one known
            // to be set (`return nil, err`, or `return err` where `err != nil`
            // was checked), leaves with an error.
            let failed = !is_nil(err)
                && (self.is_error_value(err)
                    || ident(err).is_some_and(|n| self.failed.borrow().iter().any(|f| f == n))
                    || (!values.is_empty() && values.iter().all(|v| is_zero(v)) && !matches!(unparen(err), ast::Expr::Call(_))));
            if failed {
                return vec![Stmt::Raise(StmtRaise { exc: Some(Box::new(self.expr(err))), cause: None, range })];
            }
            // `return x.Close()`, `return err`: what it hands back may be an
            // error or nothing; the call is still made.
            if !is_nil(err) && (values.is_empty() || matches!(unparen(err), ast::Expr::Call(_))) {
                values.push(err);
            }
        }
        match &values[..] {
            [] => ret(None),
            [v] => ret(Some(self.expr(v))),
            many => {
                let r = cover(many[0].range(), many[many.len() - 1].range());
                ret(Some(Expr::Tuple(ExprSeq { elts: many.iter().map(|v| self.expr(v)).collect(), range: r })))
            }
        }
    }

    /// An error made here: `errors.New(…)`, `fmt.Errorf(…)`, `ErrClosed`, `&NotFoundError{…}`.
    fn is_error_value(&self, e: &ast::Expr) -> bool {
        match unparen(e) {
            ast::Expr::Call(c) => matches!(selector(&c.fun), Some(("errors", "New")) | Some(("fmt", "Errorf"))) || type_name(&c.fun).is_some_and(|n| n.starts_with("New") && n.ends_with("Error") || n.starts_with("Err")),
            ast::Expr::Ident(i) => i.name.starts_with("Err") || i.name.starts_with("err") && i.name.len() > 3 && i.name.as_bytes()[3].is_ascii_uppercase(),
            ast::Expr::Selector(s) => s.sel.name.starts_with("Err"),
            ast::Expr::Unary(u) if u.op == Token::And => matches!(unparen(&u.x), ast::Expr::CompositeLit(c) if c.typ.as_ref().and_then(type_name).is_some_and(|n| n.ends_with("Error"))),
            ast::Expr::CompositeLit(c) => c.typ.as_ref().and_then(type_name).is_some_and(|n| n.ends_with("Error")),
            _ => false,
        }
    }

    /// In a test, `if got != want { t.Errorf(…) }` checks that got == want.
    fn assertion(&self, x: &ast::IfStmt) -> Option<Vec<Stmt>> {
        if !self.test_file || x.else_.is_some() {
            return None;
        }
        let [ast::Stmt::Expr(e)] = &x.body.list[..] else { return None };
        let ast::Expr::Call(c) = unparen(&e.x) else { return None };
        let ast::Expr::Selector(s) = unparen(&c.fun) else { return None };
        if !["Error", "Errorf", "Fatal", "Fatalf", "Fail", "FailNow"].contains(&s.sel.name.as_str()) {
            return None;
        }
        let test = negate(self.expr(&x.cond));
        let msg = c.args.first().map(|_| Box::new(self.call(c, c.range)));
        let mut out = x.init.as_ref().map(|i| self.stmt(i)).unwrap_or_default();
        out.push(Stmt::Assert(StmtAssert { test: Box::new(test), msg, range: x.range }));
        Some(out)
    }

    fn if_stmt(&self, x: &ast::IfStmt) -> Vec<Stmt> {
        if let Some(a) = self.assertion(x) {
            return a;
        }
        let mut out = vec![];
        // `if err := f(); err != nil { return err }`: the call alone.
        if let Some(var) = self.passes_error(x) {
            if let Some(ast::Stmt::Assign(a)) = &x.init {
                if a.lhs.iter().any(|l| ident(l) == Some(var)) {
                    out.extend(self.assign(a, Some(var)));
                    if let Some(e) = &x.else_ {
                        out.extend(self.else_stmts(e));
                    }
                    return out;
                }
            }
            if x.init.is_none() && x.else_.is_none() {
                return out;
            }
        }
        if let Some(init) = &x.init {
            out.extend(self.stmt(init));
        }
        let mut clauses = vec![];
        let mut els = x.else_.as_ref();
        while let Some(e) = els {
            match e {
                ast::Stmt::If(inner) if inner.init.is_none() => {
                    // `else if err != nil { return err }` passes the error on.
                    if self.passes_error(inner).is_some() && inner.else_.is_none() {
                        break;
                    }
                    clauses.push(ElifElseClause { test: Some(self.expr(&inner.cond)), body: self.block(&inner.body.list), range: inner.range });
                    els = inner.else_.as_ref();
                }
                other => {
                    let body = self.else_stmts(other);
                    if !body.is_empty() {
                        clauses.push(ElifElseClause { test: None, body, range: other.range() });
                    }
                    break;
                }
            }
        }
        let set = match unparen(&x.cond) {
            ast::Expr::Binary(b) if b.op == Token::Neq && is_nil(&b.y) => ident(&b.x).map(str::to_string),
            _ => None,
        };
        if let Some(v) = &set {
            self.failed.borrow_mut().push(v.clone());
        }
        let body = self.block(&x.body.list);
        if set.is_some() {
            self.failed.borrow_mut().pop();
        }
        if body.is_empty() && clauses.is_empty() {
            return out;
        }
        out.push(Stmt::If(StmtIf { test: Box::new(self.expr(&x.cond)), body, elif_else_clauses: clauses, range: x.range }));
        out
    }

    fn else_stmts(&self, s: &ast::Stmt) -> Vec<Stmt> {
        match s {
            ast::Stmt::Block(b) => self.block(&b.list),
            other => self.stmt(other),
        }
    }

    /// Case bodies, a `fallthrough` joining a case to the next one.
    fn clauses<'a>(&self, body: &'a ast::BlockStmt) -> Vec<(Vec<&'a ast::Expr>, bool, &'a [ast::Stmt], TextRange)> {
        let mut out: Vec<(Vec<&ast::Expr>, bool, &[ast::Stmt], TextRange)> = vec![];
        let mut pending: Vec<&ast::Expr> = vec![];
        let mut pending_default = false;
        for s in &body.list {
            let ast::Stmt::CaseClause(c) = s else { continue };
            let falls = matches!(c.body.last(), Some(ast::Stmt::Branch(b)) if b.tok == Token::Fallthrough);
            let mut list = std::mem::take(&mut pending);
            list.extend(c.list.iter());
            let default = std::mem::take(&mut pending_default) || c.is_default();
            if falls && c.body.len() == 1 {
                pending = list;
                pending_default = default;
                continue;
            }
            out.push((list, default, &c.body[..], c.range));
        }
        out
    }

    fn switch(&self, x: &ast::SwitchStmt) -> Vec<Stmt> {
        let mut out = vec![];
        if let Some(init) = &x.init {
            out.extend(self.stmt(init));
        }
        let clauses = self.clauses(&x.body);
        match &x.tag {
            // `switch { case a: … }` is an `if`/`else if` chain.
            None => {
                let mut arms: Vec<(Option<Expr>, Vec<Stmt>, TextRange)> = vec![];
                let mut default: Option<(Vec<Stmt>, TextRange)> = None;
                for (list, is_default, body, range) in clauses {
                    let body = self.block(body);
                    if is_default && list.is_empty() {
                        default = Some((body, range));
                        continue;
                    }
                    let tests: Vec<Expr> = list.iter().map(|e| self.expr(e)).collect();
                    let test = if tests.len() == 1 {
                        tests.into_iter().next().unwrap()
                    } else {
                        let r = cover(tests[0].range(), tests[tests.len() - 1].range());
                        Expr::BoolOp(ExprBoolOp { op: BoolOp::OrOr, values: tests, range: r })
                    };
                    arms.push((Some(test), body, range));
                }
                if let Some((body, range)) = default {
                    arms.push((None, body, range));
                }
                // Arms that do nothing (a default that only breaks) are not drawn.
                while arms.last().is_some_and(|(t, b, _)| t.is_none() && b.is_empty()) {
                    arms.pop();
                }
                let mut arms = arms.into_iter();
                let Some((Some(test), body, _)) = arms.next() else { return out };
                let clauses = arms.map(|(test, body, range)| ElifElseClause { test, body, range }).collect();
                out.push(Stmt::If(StmtIf { test: Box::new(test), body, elif_else_clauses: clauses, range: x.range }));
            }
            Some(tag) => {
                // Cases that do nothing (`default: break`) are not drawn.
                let cases: Vec<MatchCase> = clauses
                    .into_iter()
                    .map(|(list, _, body, range)| MatchCase { patterns: list.iter().map(|e| e.range()).collect(), binds: None, variant: None, guard: None, body: self.block(body), range })
                    .filter(|c| !c.body.is_empty())
                    .collect();
                if cases.is_empty() {
                    return out;
                }
                out.push(Stmt::Match(StmtMatch { subject: Box::new(self.expr(tag)), cases, range: x.range }));
            }
        }
        out
    }

    /// `switch v := x.(type) { case T: … }`: a match on what `x` is; `v`
    /// is the value as that type.
    fn type_switch(&self, x: &ast::TypeSwitchStmt) -> Vec<Stmt> {
        let mut out = vec![];
        if let Some(init) = &x.init {
            out.extend(self.stmt(init));
        }
        let (bound, subject) = match &x.assign {
            ast::Stmt::Assign(a) => (a.lhs.first().and_then(|l| match l {
                ast::Expr::Ident(i) => Some(i),
                _ => None,
            }), a.rhs.first()),
            ast::Stmt::Expr(e) => (None, Some(&e.x)),
            _ => (None, None),
        };
        let Some(subject) = subject else { return out };
        let subject = match unparen(subject) {
            ast::Expr::TypeAssert(t) => &t.x,
            e => e,
        };
        let cases: Vec<MatchCase> = self
            .clauses(&x.body)
            .into_iter()
            .map(|(list, _, body, range)| {
                let single = if let [t] = &list[..] { type_name(t).filter(|n| *n != "nil") } else { None };
                MatchCase {
                    patterns: list.iter().map(|e| e.range()).collect(),
                    binds: bound.filter(|b| b.name != "_").map(|b| Box::new(name(&b.name, b.range, Ctx::Store))),
                    variant: single.map(str::to_string),
                    guard: None,
                    body: self.block(body),
                    range,
                }
            })
            .filter(|c| !c.body.is_empty())
            .collect();
        if !cases.is_empty() {
            out.push(Stmt::Match(StmtMatch { subject: Box::new(self.expr(subject)), cases, range: x.range }));
        }
        out
    }

    /// `select { case v := <-in: … }`: whichever channel operation happens first.
    fn select(&self, x: &ast::SelectStmt) -> Vec<Stmt> {
        let mut cases = vec![];
        for s in &x.body.list {
            let ast::Stmt::CommClause(c) = s else { continue };
            let mut body = vec![];
            let mut binds = None;
            let mut patterns = vec![];
            match &c.comm {
                Some(ast::Stmt::Assign(a)) => {
                    patterns.push(a.range);
                    let names: Vec<Expr> = a.lhs.iter().filter(|l| ident(l) != Some("_")).map(|l| self.target(l)).collect();
                    binds = match names.len() {
                        0 => None,
                        1 => names.into_iter().next().map(Box::new),
                        _ => Some(Box::new(Expr::Tuple(ExprSeq { elts: names, range: a.range }))),
                    };
                }
                Some(other) => {
                    patterns.push(other.range());
                    // A send happens when the case is chosen.
                    if matches!(other, ast::Stmt::Send(_)) {
                        body.extend(self.stmt(other));
                    }
                }
                None => {}
            }
            body.extend(self.block(&c.body));
            // A case that does nothing (`default:` to not block) is not drawn.
            if body.is_empty() {
                continue;
            }
            cases.push(MatchCase { patterns, binds, variant: None, guard: None, body, range: c.range });
        }
        if cases.is_empty() {
            return vec![];
        }
        let at = TextRange::new(x.range.start(), x.range.start() + TextSize::from(6));
        vec![Stmt::Match(StmtMatch { subject: Box::new(name("select", at, Ctx::Load)), cases, range: x.range })]
    }

    fn for_stmt(&self, x: &ast::ForStmt) -> Vec<Stmt> {
        let range = x.range;
        let body = self.block(&x.body.list);
        match (&x.init, &x.cond, &x.post) {
            // `for i := 0; i < n; i++`: a counting loop.
            (Some(ast::Stmt::Assign(init)), Some(cond), _) if init.lhs.len() == 1 => {
                let counter = self.target(&init.lhs[0]);
                let mut out = vec![];
                // The counter starts somewhere the loop reads.
                if init.rhs.first().is_some_and(|r| !matches!(unparen(r), ast::Expr::BasicLit(_))) {
                    out.extend(self.assign(init, None));
                }
                out.push(Stmt::For(StmtFor { target: Box::new(counter), iter: Box::new(self.expr(cond)), body, orelse: vec![], kind: ForKind::Count, is_async: false, range }));
                out
            }
            (init, cond, _) => {
                let mut out = vec![];
                if let Some(i) = init {
                    out.extend(self.stmt(i));
                }
                let test = match cond {
                    Some(c) => self.expr(c),
                    None => Expr::BooleanLiteral(ExprBooleanLiteral { value: true, range: TextRange::new(range.start(), range.start() + TextSize::from(3)) }),
                };
                out.push(Stmt::While(StmtWhile { test: Box::new(test), body, orelse: vec![], range }));
                out
            }
        }
    }

    fn range_stmt(&self, x: &ast::RangeStmt) -> Vec<Stmt> {
        let range = x.range;
        let key = x.key.as_ref().filter(|k| ident(k) != Some("_"));
        let value = x.value.as_ref().filter(|v| ident(v) != Some("_"));
        let target = match (key, value) {
            (Some(k), Some(v)) => Expr::Tuple(ExprSeq { elts: vec![self.target(k), self.target(v)], range: cover(k.range(), v.range()) }),
            (Some(k), None) => self.target(k),
            (None, Some(v)) => self.target(v),
            (None, None) => name("_", TextRange::new(x.range_pos, x.range_pos + TextSize::from(5)), Ctx::Store),
        };
        // `for _, v := range xs` reads the values; with the index too, both.
        vec![Stmt::For(StmtFor { target: Box::new(target), iter: Box::new(self.expr(&x.x)), body: self.block(&x.body.list), orelse: vec![], kind: ForKind::In, is_async: false, range })]
    }

    // ----- types ------------------------------------------------------------------------

    /// A type as an expression: the type a value is (`*DB` is a `DB`;
    /// `[]Block` is `Vec[Block]`), spanning the type as written.
    pub fn type_expr(&self, t: &ast::Expr) -> Expr {
        let range = t.range();
        let seq = |kind: &str, elem: &ast::Expr| Expr::Subscript(ExprSubscript { value: Box::new(name(kind, TextRange::empty(range.start()), Ctx::Load)), slice: Box::new(self.type_expr(elem)), range });
        match t {
            ast::Expr::Ident(i) => name(&i.name, range, Ctx::Load),
            ast::Expr::Paren(p) => self.type_expr(&p.x),
            ast::Expr::Star(s) => {
                let mut e = self.type_expr(&s.x);
                set_range(&mut e, range);
                e
            }
            ast::Expr::Selector(s) => Expr::Attribute(ExprAttribute { value: Box::new(self.type_expr(&s.x)), attr: s.sel.name.clone(), range }),
            ast::Expr::ArrayType(a) => seq("Vec", &a.elt),
            ast::Expr::Ellipsis(e) => match &e.elt {
                Some(elt) => seq("Vec", elt),
                None => Expr::Other(atom(range)),
            },
            ast::Expr::ChanType(c) => seq("chan", &c.value),
            ast::Expr::MapType(m) => Expr::Subscript(ExprSubscript { value: Box::new(name("map", TextRange::empty(range.start()), Ctx::Load)), slice: Box::new(Expr::Tuple(ExprSeq { elts: vec![self.type_expr(&m.key), self.type_expr(&m.value)], range })), range }),
            ast::Expr::Index(i) => Expr::Subscript(ExprSubscript { value: Box::new(self.type_expr(&i.x)), slice: Box::new(self.type_expr(&i.index)), range }),
            ast::Expr::IndexList(i) => Expr::Subscript(ExprSubscript { value: Box::new(self.type_expr(&i.x)), slice: Box::new(Expr::Tuple(ExprSeq { elts: i.indices.iter().map(|x| self.type_expr(x)).collect(), range })), range }),
            _ => Expr::Other(atom(range)),
        }
    }

    // ----- expressions ------------------------------------------------------------------

    pub fn expr(&self, e: &ast::Expr) -> Expr {
        let d = self.depth.get();
        if d > MAX_DEPTH {
            return Expr::Other(atom(e.range()));
        }
        self.depth.set(d + 1);
        let out = self.expr_inner(e);
        self.depth.set(d);
        out
    }

    fn expr_inner(&self, e: &ast::Expr) -> Expr {
        let range = e.range();
        match e {
            ast::Expr::Ident(i) => match i.name.as_str() {
                "nil" => Expr::NoneLiteral(atom(range)),
                "true" => Expr::BooleanLiteral(ExprBooleanLiteral { value: true, range }),
                "false" => Expr::BooleanLiteral(ExprBooleanLiteral { value: false, range }),
                n => name(n, range, Ctx::Load),
            },
            ast::Expr::BasicLit(l) => match l.kind {
                ast::LitKind::String | ast::LitKind::Char => Expr::StringLiteral(ExprStringLiteral { value: unquote(&l.value), range }),
                _ => Expr::NumberLiteral(atom(range)),
            },
            ast::Expr::Paren(p) => self.expr(&p.x),
            // `db.Save(&u).Error`: gorm reports a query's error on its result; the query is the step.
            ast::Expr::Selector(s) if s.sel.name == "Error" && matches!(unparen(&s.x), ast::Expr::Call(_)) => self.expr(&s.x),
            ast::Expr::Selector(s) => Expr::Attribute(ExprAttribute { value: Box::new(self.expr(&s.x)), attr: s.sel.name.clone(), range }),
            ast::Expr::Index(i) => Expr::Subscript(ExprSubscript { value: Box::new(self.expr(&i.x)), slice: Box::new(self.expr(&i.index)), range }),
            ast::Expr::IndexList(i) => Expr::Subscript(ExprSubscript { value: Box::new(self.expr(&i.x)), slice: Box::new(Expr::Tuple(ExprSeq { elts: i.indices.iter().map(|x| self.type_expr(x)).collect(), range })), range }),
            ast::Expr::Slice(s) => {
                let slice = Expr::Slice(ExprSlice { lower: s.low.as_ref().map(|x| Box::new(self.expr(x))), upper: s.high.as_ref().map(|x| Box::new(self.expr(x))), step: None, range: TextRange::new(s.lbrack, s.rbrack + TextSize::from(1)) });
                Expr::Subscript(ExprSubscript { value: Box::new(self.expr(&s.x)), slice: Box::new(slice), range })
            }
            // `x.(T)` asserts what x is; the value is x.
            ast::Expr::TypeAssert(t) => self.expr(&t.x),
            ast::Expr::Star(s) => self.expr(&s.x),
            ast::Expr::Unary(u) => match u.op {
                Token::And | Token::Tilde | Token::Add => self.expr(&u.x),
                Token::Not => Expr::UnaryOp(ExprUnaryOp { op: UnaryOp::Bang, operand: Box::new(self.expr(&u.x)), range }),
                Token::Sub => Expr::UnaryOp(ExprUnaryOp { op: UnaryOp::USub, operand: Box::new(self.expr(&u.x)), range }),
                Token::Xor => Expr::UnaryOp(ExprUnaryOp { op: UnaryOp::Invert, operand: Box::new(self.expr(&u.x)), range }),
                // `<-ch` waits for a value.
                Token::Arrow => Expr::Await(ExprInner { value: Box::new(self.expr(&u.x)), range }),
                _ => self.expr(&u.x),
            },
            ast::Expr::Binary(b) => self.binary(b, range),
            ast::Expr::Call(c) => self.call(c, range),
            ast::Expr::CompositeLit(c) => self.composite(c, None, range),
            ast::Expr::FuncLit(f) => self.func_lit(f),
            ast::Expr::KeyValue(kv) => self.expr(&kv.value),
            ast::Expr::Ellipsis(_) | ast::Expr::Bad(_) => Expr::Other(atom(range)),
            // A type used as a value (`make(map[string]int)`, `new(T)`).
            t => self.type_expr(t),
        }
    }

    fn binary(&self, b: &ast::BinaryExpr, range: TextRange) -> Expr {
        let left = self.expr(&b.x);
        let right = self.expr(&b.y);
        let cmp = |op| Expr::Compare(ExprCompare { ops: vec![op], operands: vec![left.clone(), right.clone()], range });
        let boolean = |op: BoolOp| {
            let mut values = vec![];
            for side in [left.clone(), right.clone()] {
                match side {
                    Expr::BoolOp(inner) if inner.op == op => values.extend(inner.values),
                    v => values.push(v),
                }
            }
            Expr::BoolOp(ExprBoolOp { op, values, range })
        };
        match b.op {
            Token::LAnd => boolean(BoolOp::AndAnd),
            Token::LOr => boolean(BoolOp::OrOr),
            Token::Eql => cmp(CmpOp::Eq),
            Token::Neq => cmp(CmpOp::NotEq),
            Token::Lss => cmp(CmpOp::Lt),
            Token::Leq => cmp(CmpOp::LtE),
            Token::Gtr => cmp(CmpOp::Gt),
            Token::Geq => cmp(CmpOp::GtE),
            op => Expr::BinOp(ExprBinOp { left: Box::new(left.clone()), op: op_of(op).unwrap_or(Operator::Add), right: Box::new(right.clone()), range }),
        }
    }

    /// The path a router group serves under: `r.Group("/api")` under `r`'s own path.
    fn group_prefix(&self, c: &ast::CallExpr) -> Option<String> {
        let ast::Expr::Selector(s) = unparen(&c.fun) else { return None };
        // `r.PathPrefix("/api").Subrouter()`.
        if s.sel.name == "Subrouter" {
            if let ast::Expr::Call(inner) = unparen(&s.x) {
                return self.group_prefix(inner);
            }
        }
        if !matches!(s.sel.name.as_str(), "Group" | "PathPrefix" | "Prefix") {
            return None;
        }
        let path = match c.args.first().map(unparen) {
            Some(ast::Expr::BasicLit(l)) if l.kind == ast::LitKind::String => unquote(&l.value),
            _ => return None,
        };
        let outer = ident(&s.x).and_then(|r| self.prefixes.borrow().get(r).cloned()).unwrap_or_default();
        Some(join_route(&outer, &path))
    }

    fn call(&self, c: &ast::CallExpr, range: TextRange) -> Expr {
        // A route registered on a group serves under the group's path; chi's
        // `r.Route("/articles", func(r chi.Router) { … })` nests one.
        if let ast::Expr::Selector(s) = unparen(&c.fun) {
            // `r.With(auth).Get(…)` registers on `r`.
            let mut router = &s.x;
            while let ast::Expr::Call(inner) = unparen(router) {
                match unparen(&inner.fun) {
                    ast::Expr::Selector(m) if matches!(m.sel.name.as_str(), "With" | "Use" | "Middleware") => router = &m.x,
                    _ => break,
                }
            }
            let prefix = ident(router).and_then(|r| self.prefixes.borrow().get(r).cloned()).unwrap_or_default();
            let method = s.sel.name.as_str();
            if let (Some(ast::Expr::BasicLit(l)), true) = (c.args.first().map(unparen), ROUTE_METHODS.contains(&method) || matches!(method, "Route" | "Mount" | "Group" | "PathPrefix")) {
                if l.kind == ast::LitKind::String {
                    let path = unquote(&l.value);
                    // Go 1.22 patterns name the method: `"GET /items/{id}"`.
                    let (verb, bare) = match path.split_once(' ') {
                        Some((v, p)) if v.chars().all(|c| c.is_ascii_uppercase()) => (format!("{v} "), p.trim().to_string()),
                        _ => (String::new(), path.clone()),
                    };
                    let full = join_route(&prefix, &bare);
                    if method == "Route" {
                        if let Some(ast::Expr::FuncLit(f)) = c.args.get(1).map(unparen) {
                            let param = f.typ.params.list.first().and_then(|p| p.names.first()).map(|n| n.name.clone());
                            let saved = param.as_ref().map(|p| self.prefixes.borrow_mut().insert(p.clone(), full.clone()));
                            let out = call(self.expr(&c.fun), vec![Expr::StringLiteral(ExprStringLiteral { value: full.clone(), range: l.range }), self.func_lit(f)], range);
                            if let (Some(p), Some(prev)) = (param, saved) {
                                match prev {
                                    Some(v) => self.prefixes.borrow_mut().insert(p, v),
                                    None => self.prefixes.borrow_mut().remove(&p),
                                };
                            }
                            return out;
                        }
                    }
                    let mut args: Vec<Expr> = c.args.iter().map(|a| self.expr(a)).collect();
                    args[0] = Expr::StringLiteral(ExprStringLiteral { value: format!("{verb}{full}"), range: l.range });
                    return call(self.expr(&c.fun), args, range);
                }
            }
        }
        let args = || -> Vec<Expr> {
            let mut out: Vec<Expr> = c.args.iter().map(|a| self.expr(a)).collect();
            // `f(xs...)` spreads the list.
            if c.ellipsis.is_some() {
                if let Some(last) = out.pop() {
                    let r = last.range();
                    out.push(Expr::Starred(ExprInner { value: Box::new(last), range: r }));
                }
            }
            out
        };
        match unparen(&c.fun) {
            // Conversions: `[]byte(s)`, `string(b)`, `float64(n)`, `(*T)(p)`.
            ast::Expr::ArrayType(_) | ast::Expr::MapType(_) | ast::Expr::ChanType(_) | ast::Expr::FuncType(_) | ast::Expr::InterfaceType(_) | ast::Expr::Star(_) if c.args.len() == 1 => return self.expr(&c.args[0]),
            ast::Expr::Ident(i) if BASIC_TYPES.contains(&i.name.as_str()) && c.args.len() == 1 => return self.expr(&c.args[0]),
            ast::Expr::Ident(i) => match i.name.as_str() {
                // `append(xs, x)` as a value is the longer list.
                "append" if !c.args.is_empty() => return self.expr(&c.args[0]),
                // `make(map[K]V)` and `make([]T, n)` are empty collections; `new(T)` a T.
                "make" => {
                    return match c.args.first().map(unparen) {
                        Some(ast::Expr::MapType(_)) => Expr::Dict(ExprDict { items: vec![], range }),
                        Some(ast::Expr::ArrayType(_)) => Expr::List(ExprSeq { elts: vec![], range }),
                        Some(t) => call(name("make", i.range, Ctx::Load), vec![self.type_expr(t)], range),
                        None => Expr::Other(atom(range)),
                    }
                }
                "new" if c.args.len() == 1 => return call(self.type_expr(&c.args[0]), vec![], range),
                _ => {}
            },
            _ => {}
        }
        // Formatting functions take a template: `fmt.Sprintf("%s: %v", k, v)`.
        let formats = match selector(&c.fun) {
            Some((pkg, f)) if self.imports.get(pkg).is_some_and(|i| matches!(i.path.as_str(), "fmt" | "log")) || pkg == "fmt" || pkg == "log" => f.ends_with('f'),
            Some((_, f)) => matches!(f, "Errorf" | "Infof" | "Debugf" | "Warnf" | "Warningf" | "Fatalf" | "Panicf" | "Logf" | "Printf" | "Tracef" | "Skipf") || (f.ends_with('f') && f.starts_with("Error")),
            None => false,
        };
        if formats {
            // `fmt.Fprintf(w, "…", …)`: the format follows the writer.
            let at = c.args.iter().position(|a| matches!(unparen(a), ast::Expr::BasicLit(l) if l.kind == ast::LitKind::String));
            if let Some(at) = at.filter(|&a| a <= 1) {
                let mut out: Vec<Expr> = c.args[..at].iter().map(|a| self.expr(a)).collect();
                out.push(self.template(&c.args[at..]));
                return call(self.expr(&c.fun), out, range);
            }
        }
        call(self.expr(&c.fun), args(), range)
    }

    /// `"%s: %v", k, v` as a template of text and values.
    fn template(&self, args: &[ast::Expr]) -> Expr {
        let range = cover(args[0].range(), args[args.len() - 1].range());
        let ast::Expr::BasicLit(lit) = unparen(&args[0]) else { return self.expr(&args[0]) };
        let mut values = args[1..].iter();
        let mut parts = vec![];
        for piece in format_pieces(&unquote(&lit.value)) {
            match piece {
                Some(text) => parts.push(TemplatePart::Literal(text)),
                None => {
                    if let Some(v) = values.next() {
                        parts.push(TemplatePart::Hole { expr: self.expr(v), spec: vec![] });
                    }
                }
            }
        }
        Expr::Template(ExprTemplate { kind: TemplateKind::Format, parts, tag: None, range })
    }

    /// `T{…}`: a struct is built by its type; a slice is a list; a map a dict.
    /// `elem` is the element type a literal with its type left out has.
    fn composite(&self, c: &ast::CompositeLit, elem: Option<&ast::Expr>, range: TextRange) -> Expr {
        let typ = c.typ.as_ref().or(elem);
        let value = |e: &ast::Expr, elem: Option<&ast::Expr>| match unparen(e) {
            ast::Expr::CompositeLit(inner) if inner.typ.is_none() => self.composite(inner, elem, e.range()),
            ast::Expr::Unary(u) if u.op == Token::And && matches!(unparen(&u.x), ast::Expr::CompositeLit(inner) if inner.typ.is_none()) => match unparen(&u.x) {
                ast::Expr::CompositeLit(inner) => self.composite(inner, elem, e.range()),
                _ => unreachable!(),
            },
            other => self.expr(other),
        };
        match typ.map(unparen) {
            Some(ast::Expr::ArrayType(a)) => {
                let elem = strip_star(&a.elt);
                Expr::List(ExprSeq { elts: c.elts.iter().map(|e| value(e, Some(elem))).collect(), range })
            }
            Some(ast::Expr::MapType(m)) => {
                let elem = strip_star(&m.value);
                Expr::Dict(ExprDict {
                    items: c.elts.iter().map(|e| match e {
                        ast::Expr::KeyValue(kv) => DictItem { key: Some(value(&kv.key, Some(strip_star(&m.key)))), value: value(&kv.value, Some(elem)) },
                        other => DictItem { key: None, value: self.expr(other) },
                    }).collect(),
                    range,
                })
            }
            Some(ast::Expr::StructType(_)) | None => {
                // An anonymous struct: its fields as a dict.
                if c.elts.iter().all(|e| matches!(e, ast::Expr::KeyValue(_))) {
                    Expr::Dict(ExprDict {
                        items: c.elts.iter().filter_map(|e| match e {
                            ast::Expr::KeyValue(kv) => Some(DictItem { key: Some(Expr::StringLiteral(ExprStringLiteral { value: self.text_of(kv.key.range()).to_string(), range: kv.key.range() })), value: self.expr(&kv.value) }),
                            _ => None,
                        }).collect(),
                        range,
                    })
                } else {
                    Expr::Tuple(ExprSeq { elts: c.elts.iter().map(|e| self.expr(e)).collect(), range })
                }
            }
            // `gin.H{"error": msg}`, `M{"k": v}`: a map type by another name.
            Some(_) if !c.elts.is_empty() && c.elts.iter().all(|e| matches!(e, ast::Expr::KeyValue(kv) if matches!(unparen(&kv.key), ast::Expr::BasicLit(_)))) => Expr::Dict(ExprDict {
                items: c.elts.iter().filter_map(|e| match e {
                    ast::Expr::KeyValue(kv) => Some(DictItem { key: Some(self.expr(&kv.key)), value: value(&kv.value, None) }),
                    _ => None,
                }).collect(),
                range,
            }),
            Some(t) => {
                let func = self.type_expr(t);
                let mut arguments = Arguments::default();
                for e in &c.elts {
                    match e {
                        ast::Expr::KeyValue(kv) => match ident(&kv.key) {
                            Some(k) => arguments.keywords.push(Keyword { arg: Some(k.to_string()), value: value(&kv.value, None), range: kv.range }),
                            None => arguments.args.push(self.expr(&kv.value)),
                        },
                        other => arguments.args.push(value(other, None)),
                    }
                }
                Expr::Call(ExprCall { func: Box::new(func), arguments, kind: CallKind::Call, range })
            }
        }
    }
}

/// The opposite condition: `a != b` → `a == b`; else `!x`.
fn negate(e: Expr) -> Expr {
    match e {
        Expr::Compare(mut c) if c.ops.len() == 1 => {
            c.ops[0] = match c.ops[0] {
                CmpOp::Eq => CmpOp::NotEq,
                CmpOp::NotEq => CmpOp::Eq,
                CmpOp::Lt => CmpOp::GtE,
                CmpOp::LtE => CmpOp::Gt,
                CmpOp::Gt => CmpOp::LtE,
                CmpOp::GtE => CmpOp::Lt,
                op => op,
            };
            Expr::Compare(c)
        }
        Expr::UnaryOp(u) if u.op.is_not() => *u.operand,
        e => {
            let range = e.range();
            Expr::UnaryOp(ExprUnaryOp { op: UnaryOp::Bang, operand: Box::new(e), range })
        }
    }
}

/// `f.Close()`, `cancel()`, `mu.Unlock()`: releasing what was acquired.
fn is_cleanup(e: &ast::Expr) -> bool {
    let ast::Expr::Call(c) = unparen(e) else { return false };
    match unparen(&c.fun) {
        ast::Expr::Selector(s) => CLEANUP.contains(&s.sel.name.as_str()),
        ast::Expr::Ident(i) => CLEANUP.contains(&i.name.as_str()) || i.name.starts_with("cancel"),
        _ => false,
    }
}

/// `mu.Lock()`, `wg.Add(1)`: coordination that is not a step of its own.
fn is_sync_glue(e: &ast::Expr) -> bool {
    let ast::Expr::Call(c) = unparen(e) else { return false };
    let ast::Expr::Selector(s) = unparen(&c.fun) else { return false };
    let recv = match unparen(&s.x) {
        ast::Expr::Ident(i) => i.name.as_str(),
        ast::Expr::Selector(x) => x.sel.name.as_str(),
        _ => "",
    };
    let wait_group = recv.eq_ignore_ascii_case("wg") || recv.ends_with("Wg") || recv.ends_with("wg") || recv.ends_with("WaitGroup");
    match s.sel.name.as_str() {
        "Lock" | "Unlock" | "RLock" | "RUnlock" | "TryLock" => c.args.is_empty(),
        "Add" | "Done" | "Wait" => wait_group,
        _ => false,
    }
}

fn strip_star(e: &ast::Expr) -> &ast::Expr {
    match e {
        ast::Expr::Star(s) => strip_star(&s.x),
        ast::Expr::Paren(p) => strip_star(&p.x),
        e => e,
    }
}

/// Sets an expression's outer range (a type spans its `*` too).
fn set_range(e: &mut Expr, r: TextRange) {
    match e {
        Expr::Name(x) => x.range = r,
        Expr::Attribute(x) => x.range = r,
        Expr::Subscript(x) => x.range = r,
        Expr::Other(x) => x.range = r,
        _ => {}
    }
}
