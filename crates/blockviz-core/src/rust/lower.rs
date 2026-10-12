//! syn's Rust tree → the shared syntax tree.
//!
//! Rust is expression-oriented and the shared tree is statement-oriented, like
//! the code a reader narrates. So a value computed by `if`, `match` or a block
//! is lowered by sending each arm's final expression where the value goes:
//! `let x = match k { A => a(), B => b() };` is a match whose arms assign `x`,
//! and a function's final expression is its return. The rest map to the
//! nearest shared shape:
//! - a struct is a class of its fields, with the methods of the `impl` blocks
//!   written for it in the same module; an `impl` for a type of another module
//!   is an extension of that type; a trait is an interface, with its provided
//!   methods as methods; an enum lists its variants;
//! - `Ok(x)`, `Some(x)` and smart-pointer constructors are `x`; `Err(e)`
//!   returned is an error raised; `?` passes the value on (errors propagate
//!   the way exceptions do);
//! - `if let` and `while let` bind like `:=`; `let … else` is a guard; a match
//!   arm's pattern binds from the subject;
//! - a range `a..b` is a slice, `[x; n]` and `vec![x; n]` are `[x] * n`, and
//!   `xs.map(|x| f(x)).collect()` is a comprehension;
//! - macros are the call, list, format string, assertion or panic they stand
//!   for; a macro whose body is not Rust reads the variables it names;
//! - glue that only borrows, clones or converts (`&x`, `.clone()`, `.unwrap()`,
//!   `.iter()`, `.into()`, `as`) is left out, as `self::`, `Self` and
//!   `crate::` paths resolve to what they name.

use super::resolve::{Resolution, Resolver};
use crate::code::syntax::*;
use proc_macro2::{Span, TokenTree};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;

pub struct Lower<'s> {
    pub text: &'s str,
    pub path: &'s str,
    pub resolver: &'s Resolver,
    /// Names this file defines or imports at its top level: a path starting
    /// with one is not a module path.
    pub local: HashSet<String>,
    /// The types of the `impl` blocks being lowered, innermost last (`Self`).
    pub self_ty: RefCell<Vec<String>>,
    /// Type parameters in scope with the trait each is bounded by
    /// (`M: Matcher`), innermost last: a value of one is a value of the
    /// trait, as `impl Matcher` is.
    pub bounds: RefCell<Vec<HashMap<String, syn::Path>>>,
    /// How many inline `mod … { }` the items being lowered are in.
    pub inline: std::cell::Cell<usize>,
    /// Modules and crates named by paths in code (`crate::db::connect()`,
    /// `std::fs::read(p)`): (local name, path as written, project file).
    pub implicit: RefCell<Vec<(String, String, Option<String>)>>,
}

/// Where the value of an expression goes.
#[derive(Clone)]
enum Dest {
    Discard,
    /// Handed back by the function; `true` inside a branch arm, where even
    /// `Ok(())` leaves early.
    Return(bool),
    Assign(Box<Expr>),
}

impl Dest {
    /// The destination of a branch arm's value.
    fn arm(&self) -> Dest {
        match self {
            Dest::Return(_) => Dest::Return(true),
            d => d.clone(),
        }
    }
}

fn cover(a: TextRange, b: TextRange) -> TextRange {
    TextRange::new(a.start().min(b.start()), a.end().max(b.end()))
}

fn atom(r: TextRange) -> Atom {
    Atom { range: r }
}

fn name(id: &str, r: TextRange, ctx: Ctx) -> Expr {
    Expr::Name(ExprName { id: id.to_string(), ctx, range: r })
}

/// Methods that only borrow, clone, convert or unwrap their receiver.
const GLUE_METHODS: &[&str] = &[
    "clone", "cloned", "copied", "to_owned", "as_ref", "as_mut", "as_deref", "as_deref_mut", "as_str", "as_slice", "as_path", "as_os_str", "borrow", "borrow_mut", "unwrap", "expect", "unwrap_unchecked", "into", "iter", "iter_mut", "into_iter", "collect", "lock", "to_string", "deref", "deref_mut", "by_ref", "as_bytes", "to_vec", "into_owned", "as_mut_slice", "get_ref", "get_mut", "into_inner",
];

/// Constructors that wrap a value without changing what it is.
const WRAPPERS: &[&str] = &["Ok", "Some", "Json", "Html", "Box::new", "Box::pin", "Arc::new", "Rc::new", "RefCell::new", "Cell::new", "Mutex::new", "RwLock::new", "Pin::new", "String::from", "PathBuf::from", "Cow::Owned", "Cow::Borrowed", "Wrapping"];

/// Generic types that hold one value of their first type argument.
const HOLDERS: &[&str] = &["Box", "Arc", "BufWriter", "BufReader", "BufStream", "LineWriter", "Rc", "RefCell", "Cell", "Mutex", "RwLock", "Option", "Result", "Pin", "Cow", "Weak", "OnceCell", "OnceLock", "Lazy", "LazyLock", "ManuallyDrop", "MaybeUninit", "UnsafeCell", "Ref", "RefMut", "MutexGuard", "Spanned", "Json", "State", "Extension", "Data", "Path", "Query", "Form", "Valid"];

/// Generic types that hold many values of their first type argument.
pub const SEQUENCES: &[&str] = &["Vec", "VecDeque", "LinkedList", "HashSet", "BTreeSet", "BinaryHeap", "IndexSet", "SmallVec"];

/// Attributes that say nothing about what code does.
const QUIET_ATTRS: &[&str] = &["doc", "unsafe", "export_name", "link_section", "naked", "allow", "warn", "deny", "forbid", "expect", "inline", "must_use", "cfg", "cfg_attr", "rustfmt", "clippy", "non_exhaustive", "repr", "deprecated", "track_caller", "cold", "automatically_derived", "rustversion", "doc_cfg", "macro_use", "macro_export", "no_mangle", "used", "link", "link_name", "path", "ignore", "should_panic", "rustc_diagnostic_item", "diagnostic"];

/// Last-segment names of a path (`std::fmt::Display` → `Display`).
fn last_ident(p: &syn::Path) -> String {
    p.segments.last().map(|s| s.ident.to_string()).unwrap_or_default()
}

/// Bounds that say what every value can do, not what this one is (data
/// that can be cloned, printed, serialized): a type parameter bounded only
/// by these stays unknown.
const PLAIN_BOUNDS: &[&str] = &["Send", "Sync", "Sized", "Unpin", "Copy", "Clone", "Debug", "Default", "PartialEq", "Eq", "PartialOrd", "Ord", "Hash", "Display", "Fn", "FnMut", "FnOnce", "Any", "Into", "From", "TryInto", "TryFrom", "AsRef", "AsMut", "Borrow", "BorrowMut", "ToOwned", "ToString", "IntoIterator", "Iterator", "Future", "Deref", "DerefMut", "Serialize", "Deserialize", "DeserializeOwned"];

/// The trait each type parameter is bounded by, where it is declared or in
/// a `where` clause.
fn bounds_of(g: &syn::Generics) -> HashMap<String, syn::Path> {
    let mut out: HashMap<String, syn::Path> = HashMap::new();
    let mut add = |name: String, bounds: &Punctuated<syn::TypeParamBound, syn::Token![+]>| {
        let found = bounds.iter().find_map(|b| match b {
            syn::TypeParamBound::Trait(t) if !PLAIN_BOUNDS.contains(&last_ident(&t.path).as_str()) => Some(t.path.clone()),
            _ => None,
        });
        if let Some(p) = found {
            out.entry(name).or_insert(p);
        }
    };
    for p in &g.params {
        if let syn::GenericParam::Type(t) = p {
            add(t.ident.to_string(), &t.bounds);
        }
    }
    for pred in g.where_clause.iter().flat_map(|w| &w.predicates) {
        if let syn::WherePredicate::Type(t) = pred {
            if let syn::Type::Path(p) = &t.bounded_ty {
                if let Some(id) = p.path.get_ident() {
                    add(id.to_string(), &t.bounds);
                }
            }
        }
    }
    out
}

/// The identifier a type is named by: `&mut Vec<T>` → `Vec`, `dyn Error` → `Error`.
fn type_name(t: &syn::Type) -> Option<String> {
    match t {
        syn::Type::Path(p) => Some(last_ident(&p.path)),
        syn::Type::Reference(r) => type_name(&r.elem),
        syn::Type::Paren(p) => type_name(&p.elem),
        syn::Type::Group(g) => type_name(&g.elem),
        syn::Type::TraitObject(o) => o.bounds.iter().find_map(|b| match b {
            syn::TypeParamBound::Trait(t) => Some(last_ident(&t.path)),
            _ => None,
        }),
        _ => None,
    }
}

fn first_type_arg(seg: &syn::PathSegment) -> Option<&syn::Type> {
    match &seg.arguments {
        syn::PathArguments::AngleBracketed(a) => a.args.iter().find_map(|g| match g {
            syn::GenericArgument::Type(t) => Some(t),
            _ => None,
        }),
        _ => None,
    }
}

/// Whether an identifier pattern binds a name: `x`, `ref mut x`, `x @ ..`;
/// `None` and `MAX` name a variant or a constant.
fn binding(i: &syn::PatIdent) -> bool {
    i.subpat.is_some() || i.by_ref.is_some() || i.mutability.is_some() || !i.ident.to_string().starts_with(|c: char| c.is_uppercase())
}

/// Whether a pattern binds names (`(a, Some(b))` binds a and b).
fn pat_binds(p: &syn::Pat) -> bool {
    match p {
        syn::Pat::Ident(i) => binding(i),
        syn::Pat::Type(t) => pat_binds(&t.pat),
        syn::Pat::Reference(r) => pat_binds(&r.pat),
        syn::Pat::Paren(x) => pat_binds(&x.pat),
        syn::Pat::Tuple(t) => t.elems.iter().any(pat_binds),
        syn::Pat::TupleStruct(t) => t.elems.iter().any(pat_binds),
        syn::Pat::Struct(s) => s.fields.iter().any(|f| pat_binds(&f.pat)),
        syn::Pat::Slice(s) => s.elems.iter().any(pat_binds),
        syn::Pat::Or(o) => o.cases.iter().any(pat_binds),
        _ => false,
    }
}

/// Whether an expression computes nothing worth a step: a literal, a name, a
/// field, arithmetic or a comparison of those.
fn is_simple(e: &syn::Expr) -> bool {
    match e {
        syn::Expr::Lit(_) | syn::Expr::Path(_) => true,
        syn::Expr::Field(f) => is_simple(&f.base),
        syn::Expr::Unary(u) => is_simple(&u.expr),
        syn::Expr::Reference(r) => is_simple(&r.expr),
        syn::Expr::Paren(p) => is_simple(&p.expr),
        syn::Expr::Cast(c) => is_simple(&c.expr),
        syn::Expr::Binary(b) => is_simple(&b.left) && is_simple(&b.right),
        syn::Expr::Index(i) => is_simple(&i.expr) && is_simple(&i.index),
        syn::Expr::Tuple(t) => t.elems.iter().all(is_simple),
        syn::Expr::Array(a) => a.elems.iter().all(is_simple),
        syn::Expr::Call(c) => matches!(&*c.func, syn::Expr::Path(p) if matches!(last_ident(&p.path).as_str(), "Some" | "Ok")) && c.args.iter().all(is_simple),
        syn::Expr::MethodCall(m) => GLUE_METHODS.contains(&m.method.to_string().as_str()) && m.args.is_empty() && is_simple(&m.receiver),
        _ => false,
    }
}

/// `{}` or `()`.
fn is_nothing(e: &syn::Expr) -> bool {
    match e {
        syn::Expr::Block(b) => b.block.stmts.is_empty() && b.label.is_none(),
        syn::Expr::Tuple(t) => t.elems.is_empty(),
        // `match err {}` on a value that cannot exist.
        syn::Expr::Match(m) => m.arms.is_empty(),
        _ => false,
    }
}

/// A block's only content, when it is a single expression.
fn sole(b: &syn::Block) -> Option<&syn::Expr> {
    match &b.stmts[..] {
        [syn::Stmt::Expr(e, None)] => Some(e),
        _ => None,
    }
}

/// Statement-like expressions: written as statements, they produce no value
/// a reader follows.
fn is_control(e: &syn::Expr) -> bool {
    matches!(e, syn::Expr::If(_) | syn::Expr::Match(_) | syn::Expr::Block(_) | syn::Expr::Unsafe(_) | syn::Expr::Loop(_) | syn::Expr::While(_) | syn::Expr::ForLoop(_))
}

impl<'s> Lower<'s> {
    // ----- ranges ------------------------------------------------------------------

    fn r(&self, s: Span) -> TextRange {
        let b = s.byte_range();
        let end = b.end.min(self.text.len()) as u32;
        TextRange::new(TextSize::from((b.start as u32).min(end)), TextSize::from(end))
    }

    fn of(&self, x: &impl Spanned) -> TextRange {
        self.r(x.span())
    }

    fn rs(&self, a: Span, b: Span) -> TextRange {
        cover(self.r(a), self.r(b))
    }

    fn erange(&self, e: &syn::Expr) -> TextRange {
        cover(self.r(first(e)), self.r(last(e)))
    }

    /// Where an item starts: its first attribute other than documentation,
    /// else `start` (documentation stays a comment above it).
    fn item_start(&self, attrs: &[syn::Attribute], vis: Option<&syn::Visibility>, start: Span) -> TextSize {
        let attr = attrs.iter().find(|a| !a.path().is_ident("doc")).map(|a| a.pound_token.span);
        let vis = vis.and_then(|v| match v {
            syn::Visibility::Public(p) => Some(p.span),
            syn::Visibility::Restricted(r) => Some(r.pub_token.span),
            syn::Visibility::Inherited => None,
        });
        self.r(attr.or(vis).unwrap_or(start)).start()
    }

    fn mac_range(&self, m: &syn::Macro) -> TextRange {
        let start = m.path.leading_colon.map(|c| c.spans[0]).or_else(|| m.path.segments.first().map(|s| s.ident.span())).unwrap_or_else(Span::call_site);
        self.rs(start, m.delimiter.span().close())
    }

    // ----- documentation and attributes -----------------------------------------------

    /// The first paragraph of an item's documentation (`///`).
    pub fn doc(&self, attrs: &[syn::Attribute]) -> Option<String> {
        doc_text(attrs, false)
    }

    fn decorators(&self, attrs: &[syn::Attribute]) -> Vec<Decorator> {
        let mut out = vec![];
        for a in attrs {
            if !matches!(a.style, syn::AttrStyle::Outer) {
                continue;
            }
            let path = a.path();
            let head = path.segments.first().map(|s| s.ident.to_string()).unwrap_or_default();
            // `#[cfg(unix)]`: when the item exists, as a short phrase.
            if head == "cfg" {
                if let Ok(list) = a.meta.require_list() {
                    if let Ok(pred) = list.parse_args::<syn::Meta>() {
                        let range = self.rs(a.pound_token.span, a.bracket_token.span.close());
                        let text = cfg_text(&pred);
                        if text != "test" {
                            out.push(Decorator { expression: Expr::Call(ExprCall { func: Box::new(name("cfg", range, Ctx::Load)), arguments: Arguments { args: vec![Expr::StringLiteral(ExprStringLiteral { value: text, range })], keywords: vec![] }, kind: CallKind::Call, range }), range });
                        }
                    }
                }
                continue;
            }
            if QUIET_ATTRS.contains(&head.as_str()) {
                continue;
            }
            let range = self.rs(a.pound_token.span, a.bracket_token.span.close());
            let func = self.path_expr(None, path);
            let expression = match &a.meta {
                syn::Meta::Path(_) => func,
                syn::Meta::List(l) => {
                    let parsed = l.parse_args_with(Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated).ok();
                    let mut arguments = Arguments::default();
                    match parsed {
                        Some(items) => {
                            for e in items {
                                match &e {
                                    // `#[post("/", data = "<form>")]`
                                    syn::Expr::Assign(x) => {
                                        if let syn::Expr::Path(p) = &*x.left {
                                            arguments.keywords.push(Keyword { arg: Some(last_ident(&p.path)), value: self.expr(&x.right), range: self.erange(&e) });
                                            continue;
                                        }
                                        arguments.args.push(self.expr(&e));
                                    }
                                    _ => arguments.args.push(self.expr(&e)),
                                }
                            }
                        }
                        None => arguments.args.push(Expr::Other(atom(self.rs(l.delimiter.span().open(), l.delimiter.span().close())))),
                    }
                    Expr::Call(ExprCall { func: Box::new(func), arguments, kind: CallKind::Call, range })
                }
                syn::Meta::NameValue(nv) => Expr::Call(ExprCall {
                    func: Box::new(func),
                    arguments: Arguments { args: vec![], keywords: vec![Keyword { arg: Some(last_ident(&nv.path)), value: self.expr(&nv.value), range }] },
                    kind: CallKind::Call,
                    range,
                }),
            };
            out.push(Decorator { expression, range });
        }
        out
    }

    // ----- modules and items --------------------------------------------------------

    pub fn file(&self, f: &syn::File) -> Vec<Stmt> {
        let body = self.items(&f.items);
        let mut out: Vec<Stmt> = self
            .implicit
            .borrow()
            .iter()
            .map(|(local, spec, file)| Stmt::Import(StmtImport { names: vec![Alias { name: spec.clone(), asname: Some(local.clone()), range: TextRange::default() }], style: ModuleStyle::Path, resolved: file.clone(), declaration: false, range: TextRange::default() }))
            .collect();
        out.extend(body);
        out
    }

    fn items(&self, items: &[syn::Item]) -> Vec<Stmt> {
        // Types defined here own the `impl` blocks written for them here.
        let mut types: HashSet<String> = HashSet::new();
        for i in items {
            match i {
                syn::Item::Struct(s) => {
                    types.insert(s.ident.to_string());
                }
                syn::Item::Enum(e) => {
                    types.insert(e.ident.to_string());
                }
                syn::Item::Union(u) => {
                    types.insert(u.ident.to_string());
                }
                _ => {}
            }
        }
        let mut impls: HashMap<String, Vec<&syn::ItemImpl>> = HashMap::new();
        for i in items {
            if let syn::Item::Impl(x) = i {
                if let Some(n) = self.impl_target(x).filter(|n| types.contains(n)) {
                    impls.entry(n).or_default().push(x);
                }
            }
        }
        let mut out = vec![];
        for item in items {
            match item {
                syn::Item::Impl(x) if self.impl_target(x).is_some_and(|n| types.contains(&n)) => {}
                item => out.extend(self.item(item, &impls)),
            }
        }
        out
    }

    /// The local type an `impl` adds to: `impl<T> Stack<T>` → `Stack`;
    /// `None` for a blanket impl over a type parameter.
    fn impl_target(&self, x: &syn::ItemImpl) -> Option<String> {
        let n = type_name(&x.self_ty)?;
        let generic = x.generics.params.iter().any(|p| matches!(p, syn::GenericParam::Type(t) if t.ident == n));
        (!generic).then_some(n)
    }

    fn item(&self, item: &syn::Item, impls: &HashMap<String, Vec<&syn::ItemImpl>>) -> Vec<Stmt> {
        match item {
            syn::Item::Fn(f) => {
                let start = self.item_start(&f.attrs, Some(&f.vis), sig_start(&f.sig));
                let range = TextRange::new(start, self.r(f.block.brace_token.span.close()).end());
                vec![Stmt::FunctionDef(self.function(&f.attrs, &f.sig, Some(&f.block), DefKind::Function, range))]
            }
            syn::Item::Struct(s) => {
                let start = self.item_start(&s.attrs, Some(&s.vis), s.struct_token.span);
                let end = s.semi_token.map(|t| t.span).unwrap_or_else(|| match &s.fields {
                    syn::Fields::Named(n) => n.brace_token.span.close(),
                    syn::Fields::Unnamed(u) => u.paren_token.span.close(),
                    syn::Fields::Unit => s.ident.span(),
                });
                let ident = s.ident.to_string();
                let mut body = self.bounded(self.field_bounds(&s.generics, impls.get(&ident)), || self.fields(&s.fields));
                let (methods, bases) = self.impl_bodies(impls.get(&ident));
                body.extend(methods);
                vec![Stmt::ClassDef(ClassDef { name: ident, bases, keywords: vec![], body, decorators: self.decorators(&s.attrs), kind: ClassKind::Class, doc: self.doc(&s.attrs), range: TextRange::new(start, self.r(end).end()) })]
            }
            syn::Item::Union(u) => {
                let start = self.item_start(&u.attrs, Some(&u.vis), u.union_token.span);
                let mut body = self.fields(&syn::Fields::Named(u.fields.clone()));
                let ident = u.ident.to_string();
                let (methods, bases) = self.impl_bodies(impls.get(&ident));
                body.extend(methods);
                vec![Stmt::ClassDef(ClassDef { name: ident, bases, keywords: vec![], body, decorators: self.decorators(&u.attrs), kind: ClassKind::Class, doc: self.doc(&u.attrs), range: TextRange::new(start, self.r(u.fields.brace_token.span.close()).end()) })]
            }
            syn::Item::Enum(e) => {
                let start = self.item_start(&e.attrs, Some(&e.vis), e.enum_token.span);
                let mut body: Vec<Stmt> = e.variants.iter().map(|v| self.variant(v)).collect();
                let ident = e.ident.to_string();
                let (methods, bases) = self.impl_bodies(impls.get(&ident));
                body.extend(methods);
                vec![Stmt::ClassDef(ClassDef { name: ident, bases, keywords: vec![], body, decorators: self.decorators(&e.attrs), kind: ClassKind::Enum, doc: self.doc(&e.attrs), range: TextRange::new(start, self.r(e.brace_token.span.close()).end()) })]
            }
            syn::Item::Trait(t) => vec![Stmt::ClassDef(self.trait_def(t))],
            syn::Item::Impl(x) => self.extension(x).into_iter().collect(),
            syn::Item::Const(c) => {
                let start = self.item_start(&c.attrs, Some(&c.vis), c.const_token.span);
                let range = TextRange::new(start, self.r(c.semi_token.span).end());
                vec![self.constant(&c.ident, &c.ty, &c.expr, range)]
            }
            syn::Item::Static(s) => {
                let start = self.item_start(&s.attrs, Some(&s.vis), s.static_token.span);
                let range = TextRange::new(start, self.r(s.semi_token.span).end());
                vec![self.constant(&s.ident, &s.ty, &s.expr, range)]
            }
            syn::Item::Type(t) => {
                let range = TextRange::new(self.item_start(&t.attrs, Some(&t.vis), t.type_token.span), self.r(t.semi_token.span).end());
                vec![Stmt::TypeAlias(StmtTypeAlias { name: Box::new(name(&t.ident.to_string(), self.r(t.ident.span()), Ctx::Store)), value: Box::new(self.type_expr(&t.ty)), range })]
            }
            syn::Item::Use(u) => self.use_item(u),
            syn::Item::Mod(m) => self.module(m),
            syn::Item::ExternCrate(x) => {
                let local = x.rename.as_ref().map(|(_, i)| i.to_string()).unwrap_or_else(|| x.ident.to_string());
                let range = self.rs(x.extern_token.span, x.semi_token.span);
                if x.ident == "self" {
                    return vec![];
                }
                // `extern crate grep_cli as cli;` of a crate in the workspace.
                if let Some(root) = self.resolver.crate_root(&x.ident.to_string()) {
                    return vec![self.import_module(&[&x.ident.to_string()], local, root.to_string(), range)];
                }
                vec![self.import_external(&[x.ident.to_string()], Some(local), range)]
            }
            syn::Item::Macro(m) => self.item_macro(m),
            syn::Item::ForeignMod(f) => f
                .items
                .iter()
                .filter_map(|i| match i {
                    // `extern "C" { fn f(x: i32); }` declares a function defined elsewhere.
                    syn::ForeignItem::Fn(x) => {
                        let range = TextRange::new(self.item_start(&x.attrs, Some(&x.vis), sig_start(&x.sig)), self.r(x.semi_token.span).end());
                        Some(Stmt::FunctionDef(self.function(&x.attrs, &x.sig, None, DefKind::Function, range)))
                    }
                    _ => None,
                })
                .collect(),
            _ => vec![],
        }
    }

    fn constant(&self, ident: &syn::Ident, ty: &syn::Type, value: &syn::Expr, range: TextRange) -> Stmt {
        Stmt::AnnAssign(StmtAnnAssign { target: Box::new(name(&ident.to_string(), self.r(ident.span()), Ctx::Store)), annotation: Box::new(self.type_expr(ty)), value: Some(Box::new(self.expr(value))), range })
    }

    /// Struct fields as declarations on `self`.
    fn fields(&self, f: &syn::Fields) -> Vec<Stmt> {
        f.iter()
            .enumerate()
            .map(|(i, field)| {
                let (key, at) = match &field.ident {
                    Some(id) => (id.to_string(), self.r(id.span())),
                    None => (i.to_string(), self.of(&field.ty)),
                };
                let target = Expr::Attribute(ExprAttribute { value: Box::new(name("self", at, Ctx::Load)), attr: key, range: at });
                let range = cover(at, self.of(&field.ty));
                Stmt::AnnAssign(StmtAnnAssign { target: Box::new(target), annotation: Box::new(self.type_expr(&field.ty)), value: None, range })
            })
            .collect()
    }

    /// An enum variant: its name, with what it carries as its type.
    fn variant(&self, v: &syn::Variant) -> Stmt {
        let at = self.r(v.ident.span());
        let target = Box::new(name(&v.ident.to_string(), at, Ctx::Store));
        let fields = match &v.fields {
            syn::Fields::Named(n) => Some(self.rs(n.brace_token.span.open(), n.brace_token.span.close())),
            syn::Fields::Unnamed(u) => Some(self.rs(u.paren_token.span.open(), u.paren_token.span.close())),
            syn::Fields::Unit => None,
        };
        // A variant carrying one value (`Get(Get)`) has that value's type.
        let single = match &v.fields {
            syn::Fields::Unnamed(u) if u.unnamed.len() == 1 => Some(self.type_expr(&u.unnamed[0].ty)),
            _ => None,
        };
        match (fields, &v.discriminant) {
            (Some(r), _) => Stmt::AnnAssign(StmtAnnAssign { target, annotation: Box::new(single.unwrap_or(Expr::Other(atom(r)))), value: None, range: cover(at, r) }),
            (None, Some((_, d))) => Stmt::Assign(StmtAssign { targets: vec![*target], value: Box::new(self.expr(d)), range: cover(at, self.erange(d)) }),
            (None, None) => Stmt::Expr(StmtExpr { value: Box::new(name(&v.ident.to_string(), at, Ctx::Load)), range: at }),
        }
    }

    /// A struct's type parameters with their bounds, from the struct or the
    /// `impl` blocks for it: `struct Searcher<M> { matcher: M }` with
    /// `impl<T: Matcher> Searcher<T>` holds a Matcher.
    fn field_bounds(&self, g: &syn::Generics, impls: Option<&Vec<&syn::ItemImpl>>) -> HashMap<String, syn::Path> {
        let mut out = bounds_of(g);
        let params: Vec<String> = g.params.iter().filter_map(|p| if let syn::GenericParam::Type(t) = p { Some(t.ident.to_string()) } else { None }).collect();
        for x in impls.into_iter().flatten() {
            let theirs = bounds_of(&x.generics);
            let syn::Type::Path(p) = &*x.self_ty else { continue };
            let Some(syn::PathArguments::AngleBracketed(a)) = p.path.segments.last().map(|s| &s.arguments) else { continue };
            let args = a.args.iter().filter_map(|g| if let syn::GenericArgument::Type(t) = g { Some(t) } else { None });
            for (ours, arg) in params.iter().zip(args) {
                let syn::Type::Path(ap) = arg else { continue };
                if let Some(bound) = ap.path.get_ident().and_then(|id| theirs.get(&id.to_string())) {
                    out.entry(ours.clone()).or_insert_with(|| bound.clone());
                }
            }
        }
        out
    }

    /// The methods and constants of `impl` blocks for one type, and the traits
    /// they implement.
    fn impl_bodies(&self, impls: Option<&Vec<&syn::ItemImpl>>) -> (Vec<Stmt>, Vec<Expr>) {
        let mut body = vec![];
        let mut bases: Vec<Expr> = vec![];
        for x in impls.into_iter().flatten() {
            if let Some((_, path, _)) = &x.trait_ {
                let b = self.path_expr(None, path);
                if !bases.iter().any(|e| self.text_of(e) == self.text_of(&b)) {
                    bases.push(b);
                }
            }
            body.extend(self.impl_items(x));
        }
        (body, bases)
    }

    fn text_of(&self, e: &Expr) -> &str {
        let r = e.range();
        self.text.get(usize::from(r.start())..usize::from(r.end())).unwrap_or("")
    }

    fn impl_items(&self, x: &syn::ItemImpl) -> Vec<Stmt> {
        self.bounded(bounds_of(&x.generics), || self.impl_items_inner(x))
    }

    fn impl_items_inner(&self, x: &syn::ItemImpl) -> Vec<Stmt> {
        let ty = type_name(&x.self_ty).unwrap_or_default();
        self.self_ty.borrow_mut().push(ty);
        // A trait's methods say which trait they implement (`fmt` of `Display`).
        let implements = x.trait_.as_ref().map(|(_, path, _)| {
            let r = self.of(path);
            Decorator { expression: Expr::Call(ExprCall { func: Box::new(name("impl", r, Ctx::Load)), arguments: Arguments { args: vec![Expr::Other(atom(r))], keywords: vec![] }, kind: CallKind::Call, range: r }), range: r }
        });
        let mut out = vec![];
        for item in &x.items {
            match item {
                syn::ImplItem::Fn(f) => {
                    let start = self.item_start(&f.attrs, Some(&f.vis), sig_start(&f.sig));
                    let range = TextRange::new(start, self.r(f.block.brace_token.span.close()).end());
                    let kind = if f.sig.receiver().is_some() { DefKind::Method } else { DefKind::StaticMethod };
                    let mut def = self.function(&f.attrs, &f.sig, Some(&f.block), kind, range);
                    // Placed at the method, so the method still starts where it is written.
                    def.decorators.extend(implements.clone().map(|d| Decorator { range: TextRange::empty(range.start()), ..d }));
                    out.push(Stmt::FunctionDef(def));
                }
                syn::ImplItem::Const(c) => {
                    let range = self.rs(c.const_token.span, c.semi_token.span);
                    out.push(self.constant(&c.ident, &c.ty, &c.expr, range));
                }
                _ => {}
            }
        }
        self.self_ty.borrow_mut().pop();
        out
    }

    /// An `impl` for a type this module does not define: methods added to it.
    fn extension(&self, x: &syn::ItemImpl) -> Option<Stmt> {
        let ty = self.impl_target(x)?;
        let target = self.type_expr(&x.self_ty);
        let mut bases = vec![target];
        // The type as written, generics aside: `impl Display for io::Error`.
        let written = crate::text::squash(self.text.get(usize::from(self.of(&x.self_ty).start())..usize::from(self.of(&x.self_ty).end())).unwrap_or(&ty));
        let written = written.split('<').next().unwrap_or(&written).trim_start_matches('&').trim().to_string();
        let title = match &x.trait_ {
            Some((_, path, _)) => {
                bases.push(self.path_expr(None, path));
                format!("impl {} for {written}", last_ident(path))
            }
            None => format!("impl {written}"),
        };
        let body = self.impl_items(x);
        if body.is_empty() {
            return None;
        }
        let start = self.item_start(&x.attrs, None, x.impl_token.span);
        let range = TextRange::new(start, self.r(x.brace_token.span.close()).end());
        Some(Stmt::ClassDef(ClassDef { name: title, bases, keywords: vec![], body, decorators: vec![], kind: ClassKind::Extension, doc: self.doc(&x.attrs), range }))
    }

    fn trait_def(&self, t: &syn::ItemTrait) -> ClassDef {
        self.bounded(bounds_of(&t.generics), || self.trait_def_inner(t))
    }

    fn trait_def_inner(&self, t: &syn::ItemTrait) -> ClassDef {
        self.self_ty.borrow_mut().push(t.ident.to_string());
        let mut body = vec![];
        for item in &t.items {
            match item {
                syn::TraitItem::Fn(f) => match &f.default {
                    Some(block) => {
                        let range = self.rs(f.sig.fn_token.span, block.brace_token.span.close());
                        let range = TextRange::new(self.item_start(&f.attrs, None, sig_start(&f.sig)), range.end());
                        let kind = if f.sig.receiver().is_some() { DefKind::Method } else { DefKind::StaticMethod };
                        body.push(Stmt::FunctionDef(self.function(&f.attrs, &f.sig, Some(block), kind, range)));
                    }
                    None => {
                        let sig = self.rs(f.sig.paren_token.span.open(), match &f.sig.output {
                            syn::ReturnType::Type(_, t) => t.span(),
                            syn::ReturnType::Default => f.sig.paren_token.span.close(),
                        });
                        let range = self.rs(f.sig.fn_token.span, f.semi_token.map(|s| s.span).unwrap_or_else(|| f.sig.paren_token.span.close()));
                        body.push(Stmt::AnnAssign(StmtAnnAssign { target: Box::new(name(&f.sig.ident.to_string(), self.r(f.sig.ident.span()), Ctx::Store)), annotation: Box::new(Expr::Other(atom(sig))), value: None, range }));
                    }
                },
                syn::TraitItem::Const(c) => {
                    let range = self.rs(c.const_token.span, c.semi_token.span);
                    body.push(match &c.default {
                        Some((_, v)) => self.constant(&c.ident, &c.ty, v, range),
                        None => Stmt::AnnAssign(StmtAnnAssign { target: Box::new(name(&c.ident.to_string(), self.r(c.ident.span()), Ctx::Store)), annotation: Box::new(self.type_expr(&c.ty)), value: None, range }),
                    });
                }
                syn::TraitItem::Type(ty) => {
                    let range = self.rs(ty.type_token.span, ty.semi_token.span);
                    body.push(Stmt::AnnAssign(StmtAnnAssign { target: Box::new(name(&ty.ident.to_string(), self.r(ty.ident.span()), Ctx::Store)), annotation: Box::new(Expr::Other(atom(range))), value: None, range }));
                }
                _ => {}
            }
        }
        self.self_ty.borrow_mut().pop();
        let start = self.item_start(&t.attrs, Some(&t.vis), t.trait_token.span);
        ClassDef {
            name: t.ident.to_string(),
            bases: t
                .supertraits
                .iter()
                .filter_map(|b| match b {
                    syn::TypeParamBound::Trait(tb) => Some(self.path_expr(None, &tb.path)),
                    _ => None,
                })
                .collect(),
            keywords: vec![],
            body,
            decorators: self.decorators(&t.attrs),
            kind: ClassKind::Trait,
            doc: self.doc(&t.attrs),
            range: TextRange::new(start, self.r(t.brace_token.span.close()).end()),
        }
    }

    /// `mod a;` declares a child module; `mod a { … }` is a namespace.
    fn module(&self, m: &syn::ItemMod) -> Vec<Stmt> {
        let ident = m.ident.to_string();
        let range = TextRange::new(self.item_start(&m.attrs, Some(&m.vis), m.mod_token.span), match (&m.content, &m.semi) {
            (Some((brace, _)), _) => self.r(brace.span.close()).end(),
            (None, Some(semi)) => self.r(semi.span).end(),
            _ => self.r(m.ident.span()).end(),
        });
        match &m.content {
            None => match self.resolver.resolve(self.path, &["self", &ident], |_| false) {
                Resolution::Module { file, used: 2 } => vec![Stmt::Import(StmtImport { names: vec![Alias { name: format!("self::{ident}"), asname: Some(ident), range }], style: ModuleStyle::Path, resolved: Some(file), declaration: true, range })],
                _ => vec![],
            },
            Some((_, items)) => {
                self.inline.set(self.inline.get() + 1);
                let body = self.items(items);
                self.inline.set(self.inline.get() - 1);
                let tests = m.attrs.iter().any(|a| a.path().is_ident("cfg") && a.meta.require_list().is_ok_and(|l| l.tokens.to_string() == "test"));
                let mut decorators = vec![];
                if tests {
                    decorators.push(Decorator { expression: name("test", range, Ctx::Load), range });
                }
                vec![Stmt::ClassDef(ClassDef { name: ident, bases: vec![], keywords: vec![], body, decorators, kind: ClassKind::Namespace, doc: self.doc(&m.attrs), range })]
            }
        }
    }

    // ----- imports ----------------------------------------------------------------

    fn use_item(&self, u: &syn::ItemUse) -> Vec<Stmt> {
        let mut leaves = vec![];
        let mut prefix: Vec<(String, Span)> = vec![];
        if u.leading_colon.is_some() {
            prefix.push((String::new(), u.use_token.span));
        }
        use_leaves(&u.tree, &mut prefix, &mut leaves);
        let range = self.rs(u.use_token.span, u.semi_token.span);
        let mut out = vec![];
        // `pub use db::Db;` re-exports: the module makes the name available
        // rather than using it.
        let public = !matches!(u.vis, syn::Visibility::Inherited);
        let exported: Vec<(String, String)> = leaves
            .iter()
            .filter(|_| public)
            .map(|(_, leaf)| match leaf {
                Leaf::Name(n) | Leaf::SelfModule(n) => (n.clone(), n.clone()),
                Leaf::Glob => ("*".to_string(), "*".to_string()),
            })
            .collect();
        for (segs, leaf) in leaves {
            let mut names: Vec<&str> = segs.iter().map(|(s, _)| s.as_str()).filter(|s| !s.is_empty()).collect();
            // In an inline module, `self::` and `super::` paths that stay in
            // this file name what the file already sees; one that climbs out
            // starts from the file's module.
            let depth = self.inline.get();
            if depth > 0 {
                let ups = names.iter().take_while(|s| **s == "super").count();
                if names.first() == Some(&"self") || (ups > 0 && ups <= depth) {
                    continue;
                }
                if ups > depth {
                    names.drain(..depth);
                }
            }
            if names.is_empty() {
                continue;
            }
            let external = segs.first().is_some_and(|(s, _)| s.is_empty());
            let res = if external { Resolution::External(names[0].to_string()) } else { self.resolver.resolve(self.path, &names, |n| self.local.contains(n)) };
            match (res, leaf) {
                (Resolution::Module { file, used }, _) if file == self.path => {
                    let _ = used;
                }
                (Resolution::Module { file, used }, Leaf::Name(local)) if used == names.len() => out.push(self.import_module(&names, local, file, range)),
                (Resolution::Module { file, used }, Leaf::SelfModule(local)) if used == names.len() => out.push(self.import_module(&names, local, file, range)),
                (Resolution::Module { file, used }, Leaf::Name(local)) if used + 1 == names.len() => {
                    let item = names[used].to_string();
                    let asname = (local != item).then_some(local);
                    out.push(Stmt::ImportFrom(StmtImportFrom { module: Some(names[..used].join("::")), names: vec![Alias { name: item, asname, range }], level: 0, style: ModuleStyle::Path, resolved: Some(file), range }));
                }
                // `use grep::matcher::Matcher` through a name the module
                // re-exports: the dependency, though not the name.
                (Resolution::Module { file, used }, Leaf::Name(_)) if used + 1 < names.len() => {
                    out.push(Stmt::Import(StmtImport { names: vec![Alias { name: names[..used].join("::"), asname: None, range }], style: ModuleStyle::Path, resolved: Some(file), declaration: false, range }));
                }
                (Resolution::Module { file, used }, Leaf::Glob) if used == names.len() => {
                    out.push(Stmt::ImportFrom(StmtImportFrom { module: Some(names.join("::")), names: vec![Alias { name: "*".into(), asname: None, range }], level: 0, style: ModuleStyle::Path, resolved: Some(file), range }));
                }
                (Resolution::External(_), Leaf::Name(local)) => out.push(self.import_external(&names, Some(local), range)),
                (Resolution::External(_), Leaf::SelfModule(local)) => out.push(self.import_external(&names, Some(local), range)),
                (Resolution::External(_), Leaf::Glob) => {
                    out.push(Stmt::ImportFrom(StmtImportFrom { module: Some(names.join(".")), names: vec![Alias { name: "*".into(), asname: None, range }], level: 0, style: ModuleStyle::Path, resolved: None, range }));
                }
                _ => {}
            }
        }
        if !exported.is_empty() && !out.is_empty() {
            out.push(Stmt::Export(StmtExport { names: exported, range: TextRange::empty(range.end()) }));
        }
        out
    }

    fn import_module(&self, names: &[&str], local: String, file: String, range: TextRange) -> Stmt {
        Stmt::Import(StmtImport { names: vec![Alias { name: names.join("::"), asname: Some(local), range }], style: ModuleStyle::Path, resolved: Some(file), declaration: false, range })
    }

    /// An item of another crate, by its dotted path (`std.collections.HashMap`).
    fn import_external(&self, names: &[impl AsRef<str>], local: Option<String>, range: TextRange) -> Stmt {
        let names: Vec<&str> = names.iter().map(|n| n.as_ref()).collect();
        match names.split_last() {
            Some((last, [])) => Stmt::Import(StmtImport { names: vec![Alias { name: last.to_string(), asname: local, range }], style: ModuleStyle::Path, resolved: None, declaration: false, range }),
            Some((last, rest)) => {
                let asname = local.filter(|l| l != last);
                Stmt::ImportFrom(StmtImportFrom { module: Some(rest.join(".")), names: vec![Alias { name: last.to_string(), asname, range }], level: 0, style: ModuleStyle::Path, resolved: None, range })
            }
            None => Stmt::Other(atom(range)),
        }
    }

    /// The local name for a module named by a path in code: its own name,
    /// unless the file already uses that name for something else.
    fn implicit_module(&self, spec: &str, file: &str) -> String {
        let own = match self.resolver.module(file) {
            Some((c, path)) => path.last().cloned().unwrap_or_else(|| c.name.clone()),
            None => spec.rsplit("::").next().unwrap_or(spec).to_string(),
        };
        let mut implicit = self.implicit.borrow_mut();
        if let Some((local, _, _)) = implicit.iter().find(|(_, _, f)| f.as_deref() == Some(file)) {
            return local.clone();
        }
        let taken = |n: &str| self.local.contains(n) || implicit.iter().any(|(l, _, _)| l == n);
        let local = if taken(&own) { spec.to_string() } else { own };
        implicit.push((local.clone(), spec.to_string(), Some(file.to_string())));
        local
    }

    /// A crate named by a path in code without a `use` (`std::fs::read(p)`,
    /// `sqlx::query!(…)`): bound as an import of that crate.
    fn implicit_crate(&self, name: &str) {
        let mut implicit = self.implicit.borrow_mut();
        if !implicit.iter().any(|(l, _, _)| l == name) {
            implicit.push((name.to_string(), name.to_string(), None));
        }
    }

    // ----- functions ------------------------------------------------------------------

    /// Lowers `f` with type parameters bounded as `bounds` in scope.
    fn bounded<T>(&self, bounds: HashMap<String, syn::Path>, f: impl FnOnce() -> T) -> T {
        self.bounds.borrow_mut().push(bounds);
        let out = f();
        self.bounds.borrow_mut().pop();
        out
    }

    fn function(&self, attrs: &[syn::Attribute], sig: &syn::Signature, block: Option<&syn::Block>, kind: DefKind, range: TextRange) -> FunctionDef {
        self.bounded(bounds_of(&sig.generics), || self.function_inner(attrs, sig, block, kind, range))
    }

    fn function_inner(&self, attrs: &[syn::Attribute], sig: &syn::Signature, block: Option<&syn::Block>, kind: DefKind, range: TextRange) -> FunctionDef {
        let params = self.params(&sig.inputs);
        let unit = match &sig.output {
            syn::ReturnType::Default => true,
            syn::ReturnType::Type(_, t) => matches!(&**t, syn::Type::Tuple(t) if t.elems.is_empty()),
        };
        let body = match block {
            Some(b) => self.block(b, if unit { &Dest::Discard } else { &Dest::Return(false) }),
            None => vec![],
        };
        FunctionDef {
            name: sig.ident.to_string(),
            params,
            body,
            decorators: self.decorators(attrs),
            returns: match &sig.output {
                syn::ReturnType::Type(_, t) => Some(Box::new(self.type_expr(t))),
                syn::ReturnType::Default => None,
            },
            is_async: sig.asyncness.is_some(),
            is_generator: false,
            kind,
            doc: self.doc(attrs),
            range,
        }
    }

    fn params(&self, inputs: &Punctuated<syn::FnArg, syn::Token![,]>) -> Vec<Param> {
        let mut out = vec![];
        for arg in inputs {
            match arg {
                syn::FnArg::Receiver(r) => out.push(Param { name: "self".into(), kind: ParamKind::Positional, default: None, annotation: None, range: self.r(r.self_token.span) }),
                syn::FnArg::Typed(t) => {
                    let annotation = self.type_expr(&t.ty);
                    out.extend(self.pat_params(&t.pat, Some(annotation)));
                }
            }
        }
        out
    }

    /// Parameters a pattern binds; a destructured parameter takes the names it binds.
    fn pat_params(&self, p: &syn::Pat, annotation: Option<Expr>) -> Vec<Param> {
        match p {
            syn::Pat::Ident(i) => vec![Param { name: i.ident.to_string(), kind: ParamKind::Positional, default: None, annotation, range: self.r(i.ident.span()) }],
            syn::Pat::Type(t) => self.pat_params(&t.pat, Some(self.type_expr(&t.ty))),
            syn::Pat::Reference(r) => self.pat_params(&r.pat, annotation),
            syn::Pat::Paren(x) => self.pat_params(&x.pat, annotation),
            syn::Pat::Wild(w) => vec![Param { name: "_".into(), kind: ParamKind::Positional, default: None, annotation, range: self.r(w.underscore_token.span) }],
            other => {
                let mut names = vec![];
                walk_pat(other, &mut |id: &syn::Ident| names.push((id.to_string(), id.span())));
                // An extractor (`State(db): State<Db>`) types what it binds.
                let single = names.len() == 1;
                names.into_iter().map(|(n, s)| Param { name: n, kind: ParamKind::Positional, default: None, annotation: if single { annotation.clone() } else { None }, range: self.r(s) }).collect()
            }
        }
    }

    // ----- statements -----------------------------------------------------------------

    fn block(&self, b: &syn::Block, dest: &Dest) -> Vec<Stmt> {
        let mut out = vec![];
        let n = b.stmts.len();
        for (i, s) in b.stmts.iter().enumerate() {
            let last = i + 1 == n;
            match s {
                syn::Stmt::Local(l) => out.extend(self.local(l)),
                syn::Stmt::Item(item) => out.extend(self.items(std::slice::from_ref(item))),
                syn::Stmt::Expr(e, None) if last => out.extend(self.into(e, dest)),
                syn::Stmt::Expr(e, _) => out.extend(self.into(e, &Dest::Discard)),
                syn::Stmt::Macro(m) => out.extend(self.macro_stmt(&m.mac, if last && m.semi_token.is_none() { dest } else { &Dest::Discard })),
            }
        }
        out
    }

    fn local(&self, l: &syn::Local) -> Vec<Stmt> {
        let range = self.rs(l.let_token.span, l.semi_token.span);
        let target = self.pat(&l.pat);
        let annotation = match &l.pat {
            syn::Pat::Type(t) => Some(self.type_expr(&t.ty)),
            _ => None,
        };
        // `let _ = e;` runs `e` for its effect.
        if matches!(&l.pat, syn::Pat::Wild(_)) {
            if let Some(init) = l.init.as_ref().filter(|i| i.diverge.is_none()) {
                return self.into(&init.expr, &Dest::Discard);
            }
        }
        let Some(init) = &l.init else {
            return annotation.map(|a| Stmt::AnnAssign(StmtAnnAssign { target: Box::new(target), annotation: Box::new(a), value: None, range })).into_iter().collect();
        };
        if let Some((_, diverge)) = &init.diverge {
            // `let Some(x) = e else { return };`: `x = e`, and a guard that
            // leaves unless it matched.
            let pat_range = self.of(&l.pat);
            // The guard reads as the pattern: `unless Ok(path)`.
            let mut matched = target.clone();
            set_range(&mut matched, pat_range);
            let test = Expr::UnaryOp(ExprUnaryOp { op: UnaryOp::Bang, operand: Box::new(matched), range: pat_range });
            return vec![
                Stmt::Assign(StmtAssign { targets: vec![target], value: Box::new(self.expr(&init.expr)), range }),
                Stmt::If(StmtIf { test: Box::new(test), body: self.into(diverge, &Dest::Discard), elif_else_clauses: vec![], range: cover(pat_range, self.erange(diverge)) }),
            ];
        }
        if is_control(&init.expr) && !self.is_ternary(&init.expr) {
            let mut out = self.into(&init.expr, &Dest::Assign(Box::new(target.clone())));
            if let Some(a) = annotation {
                out.insert(0, Stmt::AnnAssign(StmtAnnAssign { target: Box::new(target), annotation: Box::new(a), value: None, range }));
            }
            return out;
        }
        let value = Box::new(self.expr(&init.expr));
        vec![match annotation {
            Some(a) => Stmt::AnnAssign(StmtAnnAssign { target: Box::new(target), annotation: Box::new(a), value: Some(value), range }),
            None => Stmt::Assign(StmtAssign { targets: vec![target], value, range }),
        }]
    }

    /// `if c { a } else { b }` choosing between simple values reads as a
    /// conditional expression, not a branch.
    fn is_ternary(&self, e: &syn::Expr) -> bool {
        let syn::Expr::If(x) = e else { return false };
        let Some((_, els)) = &x.else_branch else { return false };
        let arm_simple = |b: &syn::Block| sole(b).is_some_and(is_simple);
        arm_simple(&x.then_branch)
            && match &**els {
                syn::Expr::Block(b) => arm_simple(&b.block),
                other => self.is_ternary(other),
            }
    }

    /// An expression whose value goes to `dest`.
    fn into(&self, e: &syn::Expr, dest: &Dest) -> Vec<Stmt> {
        let range = self.erange(e);
        match e {
            syn::Expr::If(x) if !(self.is_ternary(e) && !matches!(dest, Dest::Discard)) => vec![self.if_stmt(x, dest)],
            syn::Expr::Match(m) => vec![self.match_stmt(m, dest)],
            syn::Expr::Block(b) => self.block(&b.block, dest),
            syn::Expr::Unsafe(u) => self.block(&u.block, dest),
            syn::Expr::Const(c) => self.block(&c.block, dest),
            syn::Expr::TryBlock(t) => self.block(&t.block, dest),
            syn::Expr::Paren(p) => self.into(&p.expr, dest),
            syn::Expr::Group(g) => self.into(&g.expr, dest),
            syn::Expr::Return(r) => self.ret(r.expr.as_deref(), range),
            syn::Expr::Break(_) => vec![Stmt::Break(atom(range))],
            syn::Expr::Continue(_) => vec![Stmt::Continue(atom(range))],
            syn::Expr::ForLoop(f) => vec![Stmt::For(StmtFor { target: Box::new(self.pat(&f.pat)), iter: Box::new(self.expr(&f.expr)), body: self.block(&f.body, &Dest::Discard), orelse: vec![], kind: ForKind::In, is_async: false, range })],
            syn::Expr::While(w) => vec![Stmt::While(StmtWhile { test: Box::new(self.expr(&w.cond)), body: self.block(&w.body, &Dest::Discard), orelse: vec![], range })],
            syn::Expr::Loop(l) => vec![Stmt::While(StmtWhile { test: Box::new(Expr::BooleanLiteral(ExprBooleanLiteral { value: true, range: self.r(l.loop_token.span) })), body: self.block(&l.body, &Dest::Discard), orelse: vec![], range })],
            syn::Expr::Macro(m) => self.macro_stmt(&m.mac, dest),
            syn::Expr::Assign(a) => vec![Stmt::Assign(StmtAssign { targets: vec![self.target(&a.left)], value: Box::new(self.expr(&a.right)), range })],
            syn::Expr::Binary(b) if aug_op(&b.op).is_some() => vec![Stmt::AugAssign(StmtAugAssign { target: Box::new(self.target(&b.left)), op: aug_op(&b.op).unwrap(), value: Box::new(self.expr(&b.right)), range })],
            // A value handed back wrapped (`Ok(v)`, `Some(v)`) is the value;
            // an `Err` handed back is an error raised.
            syn::Expr::Call(c) if matches!(dest, Dest::Return(_)) && self.callee_is(c, "Err") && c.args.len() == 1 => vec![self.raise(Some(&c.args[0]), range)],
            syn::Expr::Call(c) if self.callee_is(c, "Ok") && c.args.len() == 1 && !matches!(dest, Dest::Discard) => match &c.args[0] {
                syn::Expr::Tuple(t) if t.elems.is_empty() && matches!(dest, Dest::Return(false)) => vec![],
                syn::Expr::Tuple(t) if t.elems.is_empty() && matches!(dest, Dest::Return(true)) => vec![Stmt::Return(StmtReturn { value: None, range })],
                inner => self.into(inner, dest),
            },
            syn::Expr::Tuple(t) if t.elems.is_empty() && matches!(dest, Dest::Return(true)) => vec![Stmt::Return(StmtReturn { value: None, range })],
            // `let res = if done { Ok(()) } else { … }`: the arm sets the value.
            syn::Expr::Tuple(t) if t.elems.is_empty() && matches!(dest, Dest::Assign(_)) => self.emit(dest, Expr::NoneLiteral(atom(range)), range),
            syn::Expr::Tuple(t) if t.elems.is_empty() => vec![],
            syn::Expr::Try(t) => self.into(&t.expr, dest),
            // `drop(guard)` ends a value's life, like `del`.
            syn::Expr::Call(c) if matches!(dest, Dest::Discard) && self.callee_is(c, "drop") && c.args.len() == 1 => vec![Stmt::Delete(StmtDelete { targets: vec![self.expr(&c.args[0])], range })],
            _ => self.emit(dest, self.expr(e), range),
        }
    }

    fn emit(&self, dest: &Dest, value: Expr, range: TextRange) -> Vec<Stmt> {
        match dest {
            Dest::Discard if matches!(value, Expr::Name(_) | Expr::NoneLiteral(_)) || value.is_literal_expr() => vec![],
            Dest::Discard => vec![Stmt::Expr(StmtExpr { value: Box::new(value), range })],
            Dest::Return(_) => vec![Stmt::Return(StmtReturn { value: Some(Box::new(value)), range })],
            Dest::Assign(t) => vec![Stmt::Assign(StmtAssign { targets: vec![(**t).clone()], value: Box::new(value), range })],
        }
    }

    fn callee_is(&self, c: &syn::ExprCall, n: &str) -> bool {
        matches!(&*c.func, syn::Expr::Path(p) if p.qself.is_none() && p.path.segments.len() == 1 && p.path.segments[0].ident == n)
    }

    fn ret(&self, value: Option<&syn::Expr>, range: TextRange) -> Vec<Stmt> {
        match value {
            None => vec![Stmt::Return(StmtReturn { value: None, range })],
            Some(syn::Expr::Call(c)) if self.callee_is(c, "Err") && c.args.len() == 1 => vec![self.raise(Some(&c.args[0]), range)],
            Some(syn::Expr::Call(c)) if self.callee_is(c, "Ok") && matches!(c.args.first(), Some(syn::Expr::Tuple(t)) if t.elems.is_empty()) => vec![Stmt::Return(StmtReturn { value: None, range })],
            Some(syn::Expr::Tuple(t)) if t.elems.is_empty() => vec![Stmt::Return(StmtReturn { value: None, range })],
            // An explicit `return` leaves early, even with `Ok(())`.
            Some(e) => {
                let mut out = self.into(e, &Dest::Return(true));
                if out.is_empty() {
                    out.push(Stmt::Return(StmtReturn { value: None, range }));
                }
                out
            }
        }
    }

    fn raise(&self, exc: Option<&syn::Expr>, range: TextRange) -> Stmt {
        Stmt::Raise(StmtRaise { exc: exc.map(|e| Box::new(self.expr(e))), cause: None, range })
    }

    fn if_stmt(&self, x: &syn::ExprIf, dest: &Dest) -> Stmt {
        let range = self.rs(x.if_token.span, x.else_branch.as_ref().map(|(_, e)| last(e)).unwrap_or(x.then_branch.brace_token.span.close()));
        let mut clauses = vec![];
        let mut els = x.else_branch.as_ref();
        while let Some((else_token, e)) = els {
            let r = cover(self.r(else_token.span), self.erange(e));
            match &**e {
                syn::Expr::If(inner) => {
                    clauses.push(ElifElseClause { test: Some(self.expr(&inner.cond)), body: self.block(&inner.then_branch, &dest.arm()), range: r });
                    els = inner.else_branch.as_ref();
                }
                // `else {}` adds nothing.
                other if is_nothing(other) => break,
                other => {
                    clauses.push(ElifElseClause { test: None, body: self.into(other, &dest.arm()), range: r });
                    break;
                }
            }
        }
        Stmt::If(StmtIf { test: Box::new(self.expr(&x.cond)), body: self.block(&x.then_branch, &dest.arm()), elif_else_clauses: clauses, range })
    }

    fn match_stmt(&self, m: &syn::ExprMatch, dest: &Dest) -> Stmt {
        let subject = self.expr(&m.expr);
        let cases: Vec<MatchCase> = m
            .arms
            .iter()
            // `_ => {}`, `Poll::Pending => ()`: cases that do nothing are not drawn.
            .filter(|arm| !is_nothing(&arm.body))
            .map(|arm| {
                let patterns: Vec<TextRange> = match &arm.pat {
                    syn::Pat::Wild(_) => vec![],
                    syn::Pat::Or(o) => o.cases.iter().map(|c| self.of(c)).collect(),
                    p => vec![self.of(p)],
                };
                let body = self.into(&arm.body, &dest.arm());
                let range = cover(self.of(&arm.pat), self.erange(&arm.body));
                let binds = pat_binds(&arm.pat).then(|| Box::new(self.pat(&arm.pat)));
                let variant = match &arm.pat {
                    syn::Pat::TupleStruct(t) if t.elems.len() == 1 && pat_binds(&t.elems[0]) => Some(last_ident(&t.path)),
                    _ => None,
                };
                MatchCase { patterns, binds, variant, guard: arm.guard.as_ref().map(|(_, g)| Box::new(self.expr(g))), body, range }
            })
            .collect();
        let range = self.rs(m.match_token.span, m.brace_token.span.close());
        // A match whose every arm does nothing only evaluates its subject.
        if cases.is_empty() {
            return Stmt::Expr(StmtExpr { value: Box::new(subject), range });
        }
        Stmt::Match(StmtMatch { subject: Box::new(subject), cases, range })
    }

    // ----- macros ------------------------------------------------------------------------

    fn mac_name(m: &syn::Macro) -> String {
        last_ident(&m.path)
    }

    /// The macro itself as a callee: `println!`, `log::info!`.
    fn mac_func(&self, m: &syn::Macro) -> Expr {
        let bang = self.r(m.bang_token.span);
        let mut segs: Vec<(String, TextRange)> = m.path.segments.iter().map(|s| (s.ident.to_string(), self.r(s.ident.span()))).collect();
        // `sqlx::query!`, `tokio::select!`: a crate's macro.
        if let [(first, _), _, ..] = &segs[..] {
            if !self.local.contains(first) && !self.resolver.is_crate(first) && (matches!(first.as_str(), "std" | "core" | "alloc") || self.resolver.dependencies.contains(first)) {
                self.implicit_crate(first);
            }
        }
        if let Some(l) = segs.last_mut() {
            l.0.push('!');
            l.1 = cover(l.1, bang);
        }
        chain(&segs)
    }

    fn mac_args(&self, m: &syn::Macro) -> Option<Vec<syn::Expr>> {
        m.parse_body_with(Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated).ok().map(|p| p.into_iter().collect())
    }

    /// A format string and its arguments as one template, the way an
    /// f-string reads: `format!("{} has {n} items", user)` holds `user` and `n`.
    fn template(&self, args: &[syn::Expr], range: TextRange) -> Expr {
        let Some((syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }), rest)) = args.split_first() else {
            return Expr::Template(ExprTemplate { kind: TemplateKind::Format, parts: args.iter().map(|a| TemplatePart::Hole { expr: self.expr(a), spec: vec![] }).collect(), tag: None, range });
        };
        let at = self.r(s.span());
        // Arguments by position, and by name (`name = value`).
        let mut positional: Vec<&syn::Expr> = vec![];
        let mut named: Vec<(String, &syn::Expr)> = vec![];
        for a in rest {
            match a {
                syn::Expr::Assign(x) => match &*x.left {
                    syn::Expr::Path(p) => named.push((last_ident(&p.path), &x.right)),
                    _ => positional.push(a),
                },
                other => positional.push(other),
            }
        }
        let mut parts = vec![];
        let mut used = vec![false; positional.len()];
        let mut next = 0;
        for piece in format_pieces(&s.value()) {
            match piece {
                Piece::Text(t) => parts.push(TemplatePart::Literal(t)),
                Piece::Arg(arg) => {
                    let expr = if arg.is_empty() {
                        next += 1;
                        positional.get(next - 1).map(|e| {
                            used[next - 1] = true;
                            self.expr(e)
                        })
                    } else if let Ok(i) = arg.parse::<usize>() {
                        positional.get(i).map(|e| {
                            used[i] = true;
                            self.expr(e)
                        })
                    } else if let Some((_, e)) = named.iter().find(|(n, _)| *n == arg) {
                        Some(self.expr(e))
                    } else {
                        Some(name(&arg, at, Ctx::Load))
                    };
                    if let Some(expr) = expr {
                        parts.push(TemplatePart::Hole { expr, spec: vec![] });
                    }
                }
            }
        }
        // Arguments used only as widths or precisions still flow in.
        for (i, e) in positional.iter().enumerate() {
            if !used[i] {
                parts.push(TemplatePart::Hole { expr: self.expr(e), spec: vec![] });
            }
        }
        Expr::Template(ExprTemplate { kind: TemplateKind::Format, parts, tag: None, range })
    }

    /// The source range of a macro's arguments.
    fn args_range(&self, args: &[syn::Expr]) -> TextRange {
        match (args.first(), args.last()) {
            (Some(a), Some(b)) => cover(self.erange(a), self.erange(b)),
            _ => TextRange::default(),
        }
    }

    fn mac_call(&self, m: &syn::Macro, args: Vec<Expr>, range: TextRange) -> Expr {
        Expr::Call(ExprCall { func: Box::new(self.mac_func(m)), arguments: Arguments { args, keywords: vec![] }, kind: CallKind::Call, range })
    }

    /// Variables a macro body that is not Rust names (`json!({"id": id})`).
    fn mac_reads(&self, m: &syn::Macro) -> Vec<Expr> {
        let mut out: Vec<Expr> = vec![];
        let mut prev: Option<String> = None;
        let mut stack: Vec<proc_macro2::TokenStream> = vec![m.tokens.clone()];
        while let Some(ts) = stack.pop() {
            let toks: Vec<TokenTree> = ts.into_iter().collect();
            for (i, t) in toks.iter().enumerate() {
                match t {
                    TokenTree::Group(g) => stack.push(g.stream()),
                    TokenTree::Ident(id) => {
                        let s = id.to_string();
                        let next = toks.get(i + 1);
                        let call_like = matches!(next, Some(TokenTree::Punct(p)) if p.as_char() == '!' || p.as_char() == ':') || matches!(next, Some(TokenTree::Group(_)));
                        let after_dot = prev.as_deref() == Some(".");
                        let lower = s.starts_with(|c: char| c.is_lowercase() || c == '_');
                        if lower && !call_like && !after_dot && !is_keyword(&s) && !out.iter().any(|e| matches!(e, Expr::Name(n) if n.id == s)) {
                            out.push(name(&s, self.r(id.span()), Ctx::Load));
                        }
                        prev = Some(s);
                        continue;
                    }
                    TokenTree::Punct(p) => {
                        prev = Some(p.as_char().to_string());
                        continue;
                    }
                    _ => {}
                }
                prev = None;
            }
        }
        out
    }

    fn macro_expr(&self, m: &syn::Macro) -> Expr {
        let range = self.mac_range(m);
        let n = Self::mac_name(m);
        match n.as_str() {
            "vec" => {
                // `vec![x; n]` is `[x] * n`.
                if let Ok((x, count)) = m.parse_body_with(|input: syn::parse::ParseStream| {
                    let x: syn::Expr = input.parse()?;
                    let _: syn::Token![;] = input.parse()?;
                    let n: syn::Expr = input.parse()?;
                    Ok((x, n))
                }) {
                    return repeat(self.expr(&x), self.expr(&count), range);
                }
                Expr::List(ExprSeq { elts: self.mac_args(m).unwrap_or_default().iter().map(|e| self.expr(e)).collect(), range })
            }
            "format" | "format_args" | "concat" => self.template(&self.mac_args(m).unwrap_or_default(), range),
            // `format!` is the text it makes; elsewhere a format string is an argument.
            "dbg" => match self.mac_args(m).as_deref() {
                Some([one]) => self.expr(one),
                _ => self.mac_call(m, vec![], range),
            },
            "matches" | "assert_matches" => {
                let parsed = m.parse_body_with(|input: syn::parse::ParseStream| {
                    let x: syn::Expr = input.parse()?;
                    let _: syn::Token![,] = input.parse()?;
                    let rest: proc_macro2::TokenStream = input.parse()?;
                    Ok((x, rest))
                });
                match parsed {
                    Ok((x, rest)) => {
                        let pat_range = rest.clone().into_iter().next().map(|t| self.r(t.span())).map(|r| TextRange::new(r.start(), range.end() - TextSize::from(1))).unwrap_or(range);
                        Expr::Compare(ExprCompare { ops: vec![CmpOp::Eq], operands: vec![self.expr(&x), Expr::Other(atom(pat_range))], range })
                    }
                    Err(_) => self.mac_call(m, self.mac_reads(m), range),
                }
            }
            "println" | "print" | "eprintln" | "eprint" | "panic" | "unreachable" | "todo" | "unimplemented" | "info" | "debug" | "warn" | "error" | "trace" | "bail" | "anyhow" | "format_err" => {
                let literal = |a: &[syn::Expr]| matches!(a.first(), Some(syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(_), .. })));
                let args = self
                    .mac_args(m)
                    .map(|a| match a.len() {
                        0 => vec![],
                        // `bail!(err)`: a value, not a format string.
                        _ if !literal(&a) => a.iter().map(|x| self.expr(x)).collect(),
                        _ => vec![self.template(&a, self.args_range(&a))],
                    })
                    .unwrap_or_else(|| self.mac_reads(m));
                self.mac_call(m, args, range)
            }
            "write" | "writeln" => match self.mac_args(m) {
                Some(a) if !a.is_empty() => {
                    let dest = self.expr(&a[0]);
                    let rest = if a.len() > 1 { vec![self.template(&a[1..], self.args_range(&a[1..]))] } else { vec![] };
                    self.mac_call(m, std::iter::once(dest).chain(rest).collect(), range)
                }
                _ => self.mac_call(m, self.mac_reads(m), range),
            },
            _ => match self.mac_args(m) {
                Some(args) => self.mac_call(m, args.iter().map(|a| self.expr(a)).collect(), range),
                None => self.mac_call(m, self.mac_reads(m), range),
            },
        }
    }

    /// A macro used as a statement: assertions, panics and early returns are
    /// statements of their own kind.
    fn macro_stmt(&self, m: &syn::Macro, dest: &Dest) -> Vec<Stmt> {
        let range = self.mac_range(m);
        let n = Self::mac_name(m);
        match n.as_str() {
            "panic" | "unreachable" | "todo" | "unimplemented" | "bail" => vec![Stmt::Raise(StmtRaise { exc: Some(Box::new(self.macro_expr(m))), cause: None, range })],
            "assert" | "debug_assert" | "ensure" | "assert_eq" | "assert_ne" | "debug_assert_eq" | "debug_assert_ne" => {
                let args = self.mac_args(m).unwrap_or_default();
                let test = match (n.as_str(), &args[..]) {
                    ("assert_eq" | "debug_assert_eq", [a, b, ..]) => Expr::Compare(ExprCompare { ops: vec![CmpOp::Eq], operands: vec![self.expr(a), self.expr(b)], range }),
                    ("assert_ne" | "debug_assert_ne", [a, b, ..]) => Expr::Compare(ExprCompare { ops: vec![CmpOp::NotEq], operands: vec![self.expr(a), self.expr(b)], range }),
                    (_, [a, ..]) => self.expr(a),
                    _ => Expr::Other(atom(range)),
                };
                vec![Stmt::Assert(StmtAssert { test: Box::new(test), msg: None, range })]
            }
            _ => self.emit(dest, self.macro_expr(m), range),
        }
    }

    /// Items written by a macro: `thread_local!`, `lazy_static!` and the like
    /// declare statics; `macro_rules!` defines syntax, not code to draw.
    fn item_macro(&self, m: &syn::ItemMacro) -> Vec<Stmt> {
        let n = Self::mac_name(&m.mac);
        if m.ident.is_some() || n == "macro_rules" {
            return vec![];
        }
        let parsed = m.mac.parse_body_with(|input: syn::parse::ParseStream| {
            let mut out = vec![];
            while !input.is_empty() {
                let _attrs = input.call(syn::Attribute::parse_outer)?;
                let _vis: syn::Visibility = input.parse()?;
                let _: syn::Token![static] = input.parse()?;
                let _: Option<syn::Token![ref]> = input.parse()?;
                let ident: syn::Ident = input.parse()?;
                let _: syn::Token![:] = input.parse()?;
                let ty: syn::Type = input.parse()?;
                let _: syn::Token![=] = input.parse()?;
                let value: syn::Expr = input.parse()?;
                let semi: Option<syn::Token![;]> = input.parse()?;
                out.push((ident, ty, value, semi));
            }
            Ok(out)
        });
        match parsed {
            Ok(statics) => statics.iter().map(|(ident, ty, value, semi)| {
                let end = semi.map(|s| self.r(s.span)).unwrap_or_else(|| self.erange(value));
                self.constant(ident, ty, value, cover(self.r(ident.span()), end))
            }).collect(),
            Err(_) => match m.mac.parse_body_with(syn::Block::parse_within) {
                Ok(stmts) => stmts
                    .iter()
                    .flat_map(|s| match s {
                        syn::Stmt::Item(i) => self.items(std::slice::from_ref(i)),
                        _ => vec![],
                    })
                    .collect(),
                Err(_) => vec![],
            },
        }
    }

    // ----- patterns and targets ---------------------------------------------------------

    /// A pattern as the names it binds: `(a, Some(b))` → `(a, b)`.
    fn pat(&self, p: &syn::Pat) -> Expr {
        let range = self.of(p);
        match p {
            syn::Pat::Ident(i) if binding(i) => name(&i.ident.to_string(), self.r(i.ident.span()), Ctx::Store),
            syn::Pat::Ident(_) => Expr::Other(atom(range)),
            syn::Pat::Type(t) => self.pat(&t.pat),
            syn::Pat::Reference(r) => self.pat(&r.pat),
            syn::Pat::Paren(x) => self.pat(&x.pat),
            syn::Pat::Tuple(t) => Expr::Tuple(ExprSeq { elts: t.elems.iter().map(|e| self.pat(e)).collect(), range }),
            // `Some(x)`, `Ok(v)`: the value inside.
            syn::Pat::TupleStruct(t) if t.elems.len() == 1 => self.pat(&t.elems[0]),
            syn::Pat::TupleStruct(t) => Expr::Tuple(ExprSeq { elts: t.elems.iter().map(|e| self.pat(e)).collect(), range }),
            syn::Pat::Struct(s) => Expr::Tuple(ExprSeq { elts: s.fields.iter().map(|f| self.pat(&f.pat)).collect(), range }),
            syn::Pat::Slice(s) => Expr::List(ExprSeq { elts: s.elems.iter().map(|e| self.pat(e)).collect(), range }),
            syn::Pat::Or(o) => o.cases.first().map(|c| self.pat(c)).unwrap_or_else(|| Expr::Other(atom(range))),
            syn::Pat::Wild(_) => name("_", range, Ctx::Store),
            _ => Expr::Other(atom(range)),
        }
    }

    fn target(&self, e: &syn::Expr) -> Expr {
        match e {
            syn::Expr::Unary(u) if matches!(u.op, syn::UnOp::Deref(_)) => self.target(&u.expr),
            syn::Expr::Paren(p) => self.target(&p.expr),
            other => self.expr(other),
        }
    }

    // ----- types ------------------------------------------------------------------------

    /// A type as an expression: the type a value is (`Arc<Mutex<Db>>` is a
    /// `Db`; `Vec<Block>` is `Vec[Block]`), spanning the type as written.
    pub fn type_expr(&self, t: &syn::Type) -> Expr {
        let range = self.of(t);
        let mut e = self.type_inner(t);
        set_range(&mut e, range);
        e
    }

    fn type_inner(&self, t: &syn::Type) -> Expr {
        match t {
            syn::Type::Reference(r) => self.type_inner(&r.elem),
            syn::Type::Paren(p) => self.type_inner(&p.elem),
            syn::Type::Group(g) => self.type_inner(&g.elem),
            syn::Type::Ptr(p) => self.type_inner(&p.elem),
            syn::Type::Slice(s) => self.sequence("Vec", &s.elem, self.of(t)),
            syn::Type::Array(a) => self.sequence("Vec", &a.elem, self.of(t)),
            syn::Type::Path(p) if p.qself.is_none() => {
                // `m: M` where `M: Matcher`: a Matcher.
                if let Some(id) = p.path.get_ident() {
                    let bound = self.bounds.borrow().iter().rev().find_map(|b| b.get(&id.to_string()).cloned());
                    if let Some(bound) = bound {
                        return self.path_expr(None, &bound);
                    }
                }
                let last = p.path.segments.last();
                let ident = last.map(|s| s.ident.to_string()).unwrap_or_default();
                if let Some(inner) = last.and_then(first_type_arg) {
                    if HOLDERS.contains(&ident.as_str()) {
                        return self.type_inner(inner);
                    }
                    if SEQUENCES.contains(&ident.as_str()) {
                        return self.sequence(&ident, inner, self.of(t));
                    }
                }
                // `HashMap<String, Entry>`: the type, with its arguments as a
                // subscript, so the types it names are seen.
                let args: Vec<Expr> = match last.map(|s| &s.arguments) {
                    Some(syn::PathArguments::AngleBracketed(a)) => a.args.iter().filter_map(|g| match g {
                        syn::GenericArgument::Type(t) => Some(self.type_expr(t)),
                        _ => None,
                    }).collect(),
                    _ => vec![],
                };
                let base = self.path_expr(None, &p.path);
                if args.is_empty() {
                    return base;
                }
                let range = self.of(t);
                let slice = if args.len() == 1 { args.into_iter().next().unwrap() } else { Expr::Tuple(ExprSeq { elts: args, range }) };
                Expr::Subscript(ExprSubscript { value: Box::new(base), slice: Box::new(slice), range })
            }
            syn::Type::TraitObject(o) => match o.bounds.iter().find_map(|b| match b {
                syn::TypeParamBound::Trait(t) => Some(&t.path),
                _ => None,
            }) {
                Some(p) => self.path_expr(None, p),
                None => Expr::Other(atom(self.of(t))),
            },
            syn::Type::ImplTrait(o) => match o.bounds.iter().find_map(|b| match b {
                syn::TypeParamBound::Trait(t) => Some(&t.path),
                _ => None,
            }) {
                Some(p) => self.path_expr(None, p),
                None => Expr::Other(atom(self.of(t))),
            },
            _ => Expr::Other(atom(self.of(t))),
        }
    }

    fn sequence(&self, kind: &str, elem: &syn::Type, range: TextRange) -> Expr {
        let at = TextRange::new(range.start(), range.start());
        Expr::Subscript(ExprSubscript { value: Box::new(name(kind, at, Ctx::Load)), slice: Box::new(self.type_expr(elem)), range })
    }

    // ----- expressions -----------------------------------------------------------------

    /// A path in code: `Self` is the type being implemented, `crate::`,
    /// `super::` and other crates of the workspace name modules, and the
    /// rest are names and their members.
    fn path_expr(&self, qself: Option<&syn::QSelf>, path: &syn::Path) -> Expr {
        let mut segs: Vec<(String, TextRange)> = vec![];
        // `<T as Trait>::f` is the trait's `f` (the path); `<T>::f` is the type's.
        if let Some(q) = qself.filter(|q| q.position == 0) {
            let base = self.type_inner(&q.ty);
            segs.push((crate::code::names::dotted(&base).unwrap_or_else(|| "_".into()), base.range()));
        }
        for s in &path.segments {
            let mut id = s.ident.to_string();
            if id == "Self" {
                if let Some(t) = self.self_ty.borrow().last() {
                    id = t.clone();
                }
            }
            segs.push((id, self.r(s.ident.span())));
        }
        if segs.is_empty() {
            return Expr::Other(atom(self.of(path)));
        }
        let first = segs[0].0.as_str();
        // Another crate's path: `std::fs::read`, `tokio::spawn` (a crate of
        // the workspace, even when a manifest lists it as a dependency, is
        // the project's).
        if segs.len() > 1 && qself.is_none() && !self.local.contains(first) && !self.resolver.is_crate(first) && (matches!(first, "std" | "core" | "alloc") || self.resolver.dependencies.contains(first)) {
            self.implicit_crate(first);
            return chain(&segs);
        }
        let module_path = qself.is_none() && path.leading_colon.is_none() && (matches!(first, "crate" | "self" | "super") || (!self.local.contains(first) && self.resolver.is_crate(first)));
        if module_path {
            let names: Vec<&str> = segs.iter().map(|(s, _)| s.as_str()).collect();
            if let Resolution::Module { file, used } = self.resolver.resolve(self.path, &names, |n| self.local.contains(n)) {
                if file == self.path {
                    // `self::f`, or `crate::f` in the crate root: this module's own name.
                    return chain(&segs[used.min(segs.len() - 1)..]);
                }
                let spec = names[..used].join("::");
                let local = self.implicit_module(&spec, &file);
                let mut rest = vec![(local, cover(segs[0].1, segs[used - 1].1))];
                rest.extend(segs[used..].iter().cloned());
                return chain(&rest);
            }
        }
        chain(&segs)
    }

    pub fn expr(&self, e: &syn::Expr) -> Expr {
        let range = self.erange(e);
        match e {
            syn::Expr::Lit(l) => self.lit(&l.lit, range),
            syn::Expr::Path(p) => {
                if p.qself.is_none() && p.path.is_ident("None") {
                    return Expr::NoneLiteral(atom(range));
                }
                self.path_expr(p.qself.as_ref(), &p.path)
            }
            syn::Expr::Call(c) => {
                let func_text = match &*c.func {
                    syn::Expr::Path(p) => p.path.segments.iter().map(|s| s.ident.to_string()).collect::<Vec<_>>().join("::"),
                    _ => String::new(),
                };
                if c.args.len() == 1 && WRAPPERS.iter().any(|w| func_text == *w || func_text.ends_with(&format!("::{w}"))) {
                    return self.expr(&c.args[0]);
                }
                Expr::Call(ExprCall { func: Box::new(self.expr(&c.func)), arguments: Arguments { args: c.args.iter().map(|a| self.expr(a)).collect(), keywords: vec![] }, kind: CallKind::Call, range })
            }
            syn::Expr::MethodCall(m) => self.method_call(m, range),
            syn::Expr::Field(f) => {
                let attr = match &f.member {
                    syn::Member::Named(i) => i.to_string(),
                    syn::Member::Unnamed(i) => i.index.to_string(),
                };
                Expr::Attribute(ExprAttribute { value: Box::new(self.expr(&f.base)), attr, range })
            }
            syn::Expr::Index(i) => Expr::Subscript(ExprSubscript { value: Box::new(self.expr(&i.expr)), slice: Box::new(self.expr(&i.index)), range }),
            syn::Expr::Binary(b) => self.binary(b, range),
            syn::Expr::Assign(a) => Expr::Named(ExprNamed { target: Box::new(self.target(&a.left)), value: Box::new(self.expr(&a.right)), range }),
            syn::Expr::Unary(u) => match u.op {
                syn::UnOp::Deref(_) => self.expr(&u.expr),
                syn::UnOp::Not(_) => Expr::UnaryOp(ExprUnaryOp { op: UnaryOp::Bang, operand: Box::new(self.expr(&u.expr)), range }),
                syn::UnOp::Neg(_) => Expr::UnaryOp(ExprUnaryOp { op: UnaryOp::USub, operand: Box::new(self.expr(&u.expr)), range }),
                _ => self.expr(&u.expr),
            },
            syn::Expr::Reference(r) => self.expr(&r.expr),
            syn::Expr::RawAddr(r) => self.expr(&r.expr),
            syn::Expr::Paren(p) => self.expr(&p.expr),
            syn::Expr::Group(g) => self.expr(&g.expr),
            syn::Expr::Cast(c) => self.expr(&c.expr),
            syn::Expr::Try(t) => self.expr(&t.expr),
            syn::Expr::Await(a) => Expr::Await(ExprInner { value: Box::new(self.expr(&a.base)), range }),
            syn::Expr::Tuple(t) if t.elems.is_empty() => Expr::NoneLiteral(atom(range)),
            syn::Expr::Tuple(t) => Expr::Tuple(ExprSeq { elts: t.elems.iter().map(|x| self.expr(x)).collect(), range }),
            syn::Expr::Array(a) => Expr::List(ExprSeq { elts: a.elems.iter().map(|x| self.expr(x)).collect(), range }),
            syn::Expr::Repeat(r) => repeat(self.expr(&r.expr), self.expr(&r.len), range),
            syn::Expr::Struct(s) => {
                let func = self.path_expr(s.qself.as_ref(), &s.path);
                let mut keywords: Vec<Keyword> = s
                    .fields
                    .iter()
                    .map(|f| {
                        let key = match &f.member {
                            syn::Member::Named(i) => i.to_string(),
                            syn::Member::Unnamed(i) => i.index.to_string(),
                        };
                        Keyword { arg: Some(key), value: self.expr(&f.expr), range: self.of(f) }
                    })
                    .collect();
                if let Some(rest) = &s.rest {
                    keywords.push(Keyword { arg: None, value: self.expr(rest), range: self.erange(rest) });
                }
                Expr::Call(ExprCall { func: Box::new(func), arguments: Arguments { args: vec![], keywords }, kind: CallKind::Call, range })
            }
            syn::Expr::Range(r) => Expr::Slice(ExprSlice { lower: r.start.as_ref().map(|x| Box::new(self.expr(x))), upper: r.end.as_ref().map(|x| Box::new(self.expr(x))), step: None, range }),
            syn::Expr::Closure(c) => self.closure(c, range),
            syn::Expr::Async(a) => Expr::Function(Box::new(FunctionDef { name: String::new(), params: vec![], body: self.block(&a.block, &Dest::Return(false)), decorators: vec![], returns: None, is_async: true, is_generator: false, kind: DefKind::Function, doc: None, range })),
            syn::Expr::Block(b) => match sole(&b.block) {
                Some(x) => self.expr(x),
                None => self.block_value(&b.block, range),
            },
            syn::Expr::Unsafe(u) => match sole(&u.block) {
                Some(x) => self.expr(x),
                None => self.block_value(&u.block, range),
            },
            syn::Expr::Const(c) => match sole(&c.block) {
                Some(x) => self.expr(x),
                None => Expr::Other(atom(range)),
            },
            syn::Expr::If(x) => {
                let body = sole(&x.then_branch).map(|b| self.expr(b)).unwrap_or_else(|| Expr::Other(atom(self.rs(x.then_branch.brace_token.span.open(), x.then_branch.brace_token.span.close()))));
                let orelse = match &x.else_branch {
                    Some((_, e)) => self.expr(e),
                    None => Expr::NoneLiteral(atom(range)),
                };
                Expr::If(ExprIf { test: Box::new(self.expr(&x.cond)), body: Box::new(body), orelse: Box::new(orelse), range })
            }
            syn::Expr::Macro(m) => self.macro_expr(&m.mac),
            // `let Some(x) = f()` in a condition binds like `:=`; it reads as its pattern.
            syn::Expr::Let(l) => {
                let mut target = self.pat(&l.pat);
                set_range(&mut target, self.of(&l.pat));
                Expr::Named(ExprNamed { target: Box::new(target), value: Box::new(self.expr(&l.expr)), range })
            }
            syn::Expr::Yield(y) => Expr::Yield(ExprYield { value: y.expr.as_ref().map(|x| Box::new(self.expr(x))), range }),
            syn::Expr::Infer(_) => name("_", range, Ctx::Load),
            _ => Expr::Other(atom(range)),
        }
    }

    /// A block used as a value inside a larger expression: its statements run
    /// in a function of their own whose result is the value.
    fn block_value(&self, b: &syn::Block, range: TextRange) -> Expr {
        Expr::Call(ExprCall {
            func: Box::new(Expr::Function(Box::new(FunctionDef { name: String::new(), params: vec![], body: self.block(b, &Dest::Return(false)), decorators: vec![], returns: None, is_async: false, is_generator: false, kind: DefKind::Function, doc: None, range }))),
            arguments: Arguments::default(),
            kind: CallKind::Call,
            range,
        })
    }

    fn lit(&self, l: &syn::Lit, range: TextRange) -> Expr {
        match l {
            syn::Lit::Str(s) => Expr::StringLiteral(ExprStringLiteral { value: s.value(), range }),
            syn::Lit::CStr(_) => Expr::StringLiteral(ExprStringLiteral { value: self.text.get(usize::from(range.start())..usize::from(range.end())).unwrap_or("").to_string(), range }),
            syn::Lit::ByteStr(b) => Expr::BytesLiteral(ExprBytesLiteral { value: b.value(), range }),
            syn::Lit::Char(c) => Expr::StringLiteral(ExprStringLiteral { value: c.value().to_string(), range }),
            syn::Lit::Bool(b) => Expr::BooleanLiteral(ExprBooleanLiteral { value: b.value, range }),
            _ => Expr::NumberLiteral(atom(range)),
        }
    }

    fn binary(&self, b: &syn::ExprBinary, range: TextRange) -> Expr {
        use syn::BinOp as B;
        if let Some(op) = aug_op(&b.op) {
            let target = self.target(&b.left);
            return Expr::Named(ExprNamed { target: Box::new(target.clone()), value: Box::new(Expr::BinOp(ExprBinOp { left: Box::new(target), op, right: Box::new(self.expr(&b.right)), range })), range });
        }
        let left = self.expr(&b.left);
        let right = self.expr(&b.right);
        let cmp = |op| Expr::Compare(ExprCompare { ops: vec![op], operands: vec![left.clone(), right.clone()], range });
        let arith = |op| Expr::BinOp(ExprBinOp { left: Box::new(left.clone()), op, right: Box::new(right.clone()), range });
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
            B::And(_) => boolean(BoolOp::AndAnd),
            B::Or(_) => boolean(BoolOp::OrOr),
            B::Eq(_) => cmp(CmpOp::Eq),
            B::Ne(_) => cmp(CmpOp::NotEq),
            B::Lt(_) => cmp(CmpOp::Lt),
            B::Le(_) => cmp(CmpOp::LtE),
            B::Gt(_) => cmp(CmpOp::Gt),
            B::Ge(_) => cmp(CmpOp::GtE),
            B::Add(_) => arith(Operator::Add),
            B::Sub(_) => arith(Operator::Sub),
            B::Mul(_) => arith(Operator::Mult),
            B::Div(_) => arith(Operator::Div),
            B::Rem(_) => arith(Operator::Mod),
            B::BitXor(_) => arith(Operator::BitXor),
            B::BitAnd(_) => arith(Operator::BitAnd),
            B::BitOr(_) => arith(Operator::BitOr),
            B::Shl(_) => arith(Operator::LShift),
            B::Shr(_) => arith(Operator::RShift),
            _ => arith(Operator::Add),
        }
    }

    fn method_call(&self, m: &syn::ExprMethodCall, range: TextRange) -> Expr {
        let method = m.method.to_string();
        // `xs.map(|x| f(x)).collect()` is `[f(x) for x in xs]`.
        if method == "collect" {
            if let Some(comp) = self.comprehension(&m.receiver, range) {
                return comp;
            }
        }
        // Glue keeps the receiver's own range, so names read as written.
        if GLUE_METHODS.contains(&method.as_str()) && (m.args.is_empty() || (method == "expect" && m.args.len() == 1)) {
            return self.expr(&m.receiver);
        }
        // `counts.entry(word).or_insert(0)` is the map's item: `counts[word]`.
        if matches!(method.as_str(), "or_insert" | "or_default" | "or_insert_with" | "or_insert_with_key") {
            if let syn::Expr::MethodCall(entry) = &*m.receiver {
                if entry.method == "entry" && entry.args.len() == 1 {
                    return Expr::Subscript(ExprSubscript { value: Box::new(self.expr(&entry.receiver)), slice: Box::new(self.expr(&entry.args[0])), range });
                }
            }
        }
        let attr_range = cover(self.erange(&m.receiver), self.r(m.method.span()));
        Expr::Call(ExprCall {
            func: Box::new(Expr::Attribute(ExprAttribute { value: Box::new(self.expr(&m.receiver)), attr: method, range: attr_range })),
            arguments: Arguments { args: m.args.iter().map(|a| self.expr(a)).collect(), keywords: vec![] },
            kind: CallKind::Call,
            range,
        })
    }

    fn comprehension(&self, e: &syn::Expr, range: TextRange) -> Option<Expr> {
        let mut e = e;
        let mut ifs = vec![];
        let syn::Expr::MethodCall(map) = e else { return None };
        if map.method != "map" || map.args.len() != 1 {
            return None;
        }
        let syn::Expr::Closure(c) = &map.args[0] else { return None };
        if c.inputs.len() != 1 {
            return None;
        }
        let elt = match &*c.body {
            syn::Expr::Block(b) => self.expr(sole(&b.block)?),
            body => self.expr(body),
        };
        let target = self.pat(&c.inputs[0]);
        e = &map.receiver;
        // Glue and filters between the source and the map.
        loop {
            match e {
                syn::Expr::MethodCall(x) if GLUE_METHODS.contains(&x.method.to_string().as_str()) && x.args.is_empty() => e = &x.receiver,
                syn::Expr::MethodCall(x) if x.method == "filter" && x.args.len() == 1 => {
                    if let syn::Expr::Closure(f) = &x.args[0] {
                        ifs.push(self.expr(&f.body));
                    }
                    e = &x.receiver;
                }
                syn::Expr::Paren(p) => e = &p.expr,
                _ => break,
            }
        }
        let iter = self.expr(e);
        let generators = vec![Comprehension { target, iter, ifs, range }];
        Some(Expr::Comp(ExprComp { kind: CompKind::List, elt: Box::new(elt), value: None, generators, range }))
    }

    fn closure(&self, c: &syn::ExprClosure, range: TextRange) -> Expr {
        let params: Vec<Param> = c.inputs.iter().flat_map(|p| self.pat_params(p, None)).collect();
        // `|req| async move { … }` is an async function of `req`.
        if let syn::Expr::Async(a) = &*c.body {
            return Expr::Function(Box::new(FunctionDef { name: String::new(), params, body: self.block(&a.block, &Dest::Return(false)), decorators: vec![], returns: None, is_async: true, is_generator: false, kind: DefKind::Function, doc: None, range }));
        }
        let body_expr: &syn::Expr = match &*c.body {
            syn::Expr::Block(b) => match sole(&b.block) {
                Some(x) if !is_control(x) || self.is_ternary(x) => x,
                _ => {
                    return Expr::Function(Box::new(FunctionDef { name: String::new(), params, body: self.block(&b.block, &Dest::Return(false)), decorators: vec![], returns: None, is_async: c.asyncness.is_some(), is_generator: false, kind: DefKind::Function, doc: None, range }));
                }
            },
            body if is_control(body) && !self.is_ternary(body) => {
                return Expr::Function(Box::new(FunctionDef { name: String::new(), params, body: self.into(body, &Dest::Return(false)), decorators: vec![], returns: None, is_async: c.asyncness.is_some(), is_generator: false, kind: DefKind::Function, doc: None, range }));
            }
            body => body,
        };
        Expr::Lambda(ExprLambda { params, body: Box::new(self.expr(body_expr)), range })
    }
}

/// A `cfg` predicate in words: `unix`, `not windows`, `feature otel`,
/// `unix, not macos`, `linux or macos`.
fn cfg_text(m: &syn::Meta) -> String {
    match m {
        syn::Meta::Path(p) => match last_ident(p).as_str() {
            "debug_assertions" => "debug".into(),
            other => other.to_string(),
        },
        syn::Meta::NameValue(nv) => {
            let value = match &nv.value {
                syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) => s.value(),
                _ => String::new(),
            };
            match last_ident(&nv.path).as_str() {
                "feature" => format!("feature {value}"),
                _ => value,
            }
        }
        syn::Meta::List(l) => {
            let inner: Vec<String> = l.parse_args_with(Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated).map(|ms| ms.iter().map(cfg_text).collect()).unwrap_or_default();
            match last_ident(&l.path).as_str() {
                "not" => format!("not {}", inner.join(", ")),
                "any" => inner.join(" or "),
                _ => inner.join(", "),
            }
        }
    }
}

/// `[x] * n`.
fn repeat(x: Expr, n: Expr, range: TextRange) -> Expr {
    let list = Expr::List(ExprSeq { elts: vec![x], range });
    Expr::BinOp(ExprBinOp { left: Box::new(list), op: Operator::Mult, right: Box::new(n), range })
}

fn aug_op(op: &syn::BinOp) -> Option<Operator> {
    use syn::BinOp as B;
    Some(match op {
        B::AddAssign(_) => Operator::Add,
        B::SubAssign(_) => Operator::Sub,
        B::MulAssign(_) => Operator::Mult,
        B::DivAssign(_) => Operator::Div,
        B::RemAssign(_) => Operator::Mod,
        B::BitXorAssign(_) => Operator::BitXor,
        B::BitAndAssign(_) => Operator::BitAnd,
        B::BitOrAssign(_) => Operator::BitOr,
        B::ShlAssign(_) => Operator::LShift,
        B::ShrAssign(_) => Operator::RShift,
        _ => return None,
    })
}

/// `a::b::c` as `a.b.c`, each prefix spanning its segments.
fn chain(segs: &[(String, TextRange)]) -> Expr {
    let (id, r) = &segs[0];
    let mut e = name(id, *r, Ctx::Load);
    let start = r.start();
    for (attr, r) in &segs[1..] {
        e = Expr::Attribute(ExprAttribute { value: Box::new(e), attr: attr.clone(), range: TextRange::new(start, r.end().max(start)) });
    }
    e
}

fn set_range(e: &mut Expr, r: TextRange) {
    match e {
        Expr::Name(x) => x.range = r,
        Expr::Tuple(x) | Expr::List(x) => x.range = r,
        Expr::Attribute(x) => x.range = r,
        Expr::Subscript(x) => x.range = r,
        Expr::Other(x) => x.range = r,
        _ => {}
    }
}

enum Piece {
    Text(String),
    /// A placeholder's argument: empty for the next positional one, an index,
    /// or a name.
    Arg(String),
}

/// A format string's text and placeholders: `"{user} has {:>3} items"` →
/// `user`, " has ", ``, " items".
fn format_pieces(fmt: &str) -> Vec<Piece> {
    let mut out = vec![];
    let mut text = String::new();
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                text.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                text.push('}');
            }
            '{' => {
                let mut inner = String::new();
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                    inner.push(c);
                }
                if !text.is_empty() {
                    out.push(Piece::Text(std::mem::take(&mut text)));
                }
                let arg = inner.split(':').next().unwrap_or("").trim().to_string();
                out.push(Piece::Arg(arg));
            }
            c => text.push(c),
        }
    }
    if !text.is_empty() {
        out.push(Piece::Text(text));
    }
    out
}

fn is_keyword(s: &str) -> bool {
    matches!(s, "as" | "break" | "const" | "continue" | "crate" | "else" | "enum" | "extern" | "false" | "fn" | "for" | "if" | "impl" | "in" | "let" | "loop" | "match" | "mod" | "move" | "mut" | "pub" | "ref" | "return" | "static" | "struct" | "super" | "trait" | "true" | "type" | "unsafe" | "use" | "where" | "while" | "async" | "await" | "dyn")
}

/// Calls `f` with each name a pattern binds.
fn walk_pat(p: &syn::Pat, f: &mut impl FnMut(&syn::Ident)) {
    match p {
        syn::Pat::Ident(i) if binding(i) => f(&i.ident),
        syn::Pat::Type(t) => walk_pat(&t.pat, f),
        syn::Pat::Reference(r) => walk_pat(&r.pat, f),
        syn::Pat::Paren(x) => walk_pat(&x.pat, f),
        syn::Pat::Tuple(t) => t.elems.iter().for_each(|e| walk_pat(e, f)),
        syn::Pat::TupleStruct(t) => t.elems.iter().for_each(|e| walk_pat(e, f)),
        syn::Pat::Struct(s) => s.fields.iter().for_each(|x| walk_pat(&x.pat, f)),
        syn::Pat::Slice(s) => s.elems.iter().for_each(|e| walk_pat(e, f)),
        syn::Pat::Or(o) => {
            if let Some(c) = o.cases.first() {
                walk_pat(c, f)
            }
        }
        _ => {}
    }
}

enum Leaf {
    /// `use a::b;` / `use a::b as c;`: binds a local name.
    Name(String),
    /// `use a::b::{self}`: binds the module `b`.
    SelfModule(String),
    /// `use a::b::*;`
    Glob,
}

fn use_leaves(t: &syn::UseTree, prefix: &mut Vec<(String, Span)>, out: &mut Vec<(Vec<(String, Span)>, Leaf)>) {
    match t {
        syn::UseTree::Path(p) => {
            prefix.push((p.ident.to_string(), p.ident.span()));
            use_leaves(&p.tree, prefix, out);
            prefix.pop();
        }
        syn::UseTree::Name(n) if n.ident == "self" => {
            if let Some((last, _)) = prefix.last() {
                out.push((prefix.clone(), Leaf::SelfModule(last.clone())));
            }
        }
        syn::UseTree::Name(n) => {
            let mut segs = prefix.clone();
            segs.push((n.ident.to_string(), n.ident.span()));
            out.push((segs, Leaf::Name(n.ident.to_string())));
        }
        syn::UseTree::Rename(r) if r.ident == "self" => out.push((prefix.clone(), Leaf::SelfModule(r.rename.to_string()))),
        syn::UseTree::Rename(r) => {
            if r.rename == "_" {
                return;
            }
            let mut segs = prefix.clone();
            segs.push((r.ident.to_string(), r.ident.span()));
            out.push((segs, Leaf::Name(r.rename.to_string())));
        }
        syn::UseTree::Glob(_) => out.push((prefix.clone(), Leaf::Glob)),
        syn::UseTree::Group(g) => g.items.iter().for_each(|i| use_leaves(i, prefix, out)),
    }
}

/// The names a file binds at its top level: items, and names `use` brings in.
pub fn top_names(items: &[syn::Item]) -> HashSet<String> {
    let mut out = HashSet::new();
    for i in items {
        let id = match i {
            syn::Item::Fn(f) => Some(&f.sig.ident),
            syn::Item::Struct(s) => Some(&s.ident),
            syn::Item::Enum(e) => Some(&e.ident),
            syn::Item::Union(u) => Some(&u.ident),
            syn::Item::Trait(t) => Some(&t.ident),
            syn::Item::Type(t) => Some(&t.ident),
            syn::Item::Const(c) => Some(&c.ident),
            syn::Item::Static(s) => Some(&s.ident),
            syn::Item::Mod(m) if m.content.is_some() => Some(&m.ident),
            syn::Item::Use(u) => {
                let mut leaves = vec![];
                use_leaves(&u.tree, &mut vec![], &mut leaves);
                for (_, leaf) in leaves {
                    if let Leaf::Name(n) | Leaf::SelfModule(n) = leaf {
                        out.insert(n);
                    }
                }
                None
            }
            _ => None,
        };
        if let Some(id) = id {
            out.insert(id.to_string());
        }
    }
    out
}

/// Documentation attributes of one style, first paragraph only.
pub fn doc_text(attrs: &[syn::Attribute], inner: bool) -> Option<String> {
    let mut lines = vec![];
    for a in attrs {
        if matches!(a.style, syn::AttrStyle::Inner(_)) != inner || !a.path().is_ident("doc") {
            continue;
        }
        if let syn::Meta::NameValue(nv) = &a.meta {
            if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &nv.value {
                let value = s.value();
                // An empty `///` line separates paragraphs.
                if value.trim().is_empty() {
                    lines.push(String::new());
                }
                for l in value.lines() {
                    lines.push(l.trim().to_string());
                }
            }
        }
    }
    let mut para = vec![];
    for l in &lines {
        if l.is_empty() {
            if para.is_empty() {
                continue;
            }
            break;
        }
        if l.starts_with('#') || l.starts_with("```") {
            if para.is_empty() {
                continue;
            }
            break;
        }
        para.push(l.as_str());
    }
    let text = crate::text::summary(&para.join(" "));
    (!text.is_empty()).then_some(text)
}

// ----- spans of expressions --------------------------------------------------------------

/// The first token of a function signature (`async`, `const`, `unsafe`, `extern`, `fn`).
fn sig_start(sig: &syn::Signature) -> Span {
    sig.constness.map(|t| t.span).or(sig.asyncness.map(|t| t.span)).or(sig.unsafety.map(|t| t.span)).or(sig.abi.as_ref().map(|a| a.extern_token.span)).unwrap_or(sig.fn_token.span)
}

fn path_first(p: &syn::Path) -> Span {
    p.leading_colon.map(|c| c.spans[0]).or_else(|| p.segments.first().map(|s| s.ident.span())).unwrap_or_else(Span::call_site)
}

fn path_last(p: &syn::Path) -> Span {
    match p.segments.last() {
        Some(s) => match &s.arguments {
            syn::PathArguments::None => s.ident.span(),
            syn::PathArguments::AngleBracketed(a) => a.gt_token.spans[0],
            syn::PathArguments::Parenthesized(x) => match &x.output {
                syn::ReturnType::Type(_, t) => t.span(),
                syn::ReturnType::Default => x.paren_token.span.close(),
            },
        },
        None => Span::call_site(),
    }
}

fn label_or(l: &Option<syn::Label>, s: Span) -> Span {
    l.as_ref().map(|l| l.name.apostrophe).unwrap_or(s)
}

/// The first token of an expression.
pub fn first(e: &syn::Expr) -> Span {
    use syn::Expr as E;
    match e {
        E::Array(x) => x.bracket_token.span.open(),
        E::Assign(x) => first(&x.left),
        E::Async(x) => x.async_token.span,
        E::Await(x) => first(&x.base),
        E::Binary(x) => first(&x.left),
        E::Block(x) => label_or(&x.label, x.block.brace_token.span.open()),
        E::Break(x) => x.break_token.span,
        E::Call(x) => first(&x.func),
        E::Cast(x) => first(&x.expr),
        E::Closure(x) => x.asyncness.map(|a| a.span).or(x.capture.map(|c| c.span)).unwrap_or(x.or1_token.spans[0]),
        E::Const(x) => x.const_token.span,
        E::Continue(x) => x.continue_token.span,
        E::Field(x) => first(&x.base),
        E::ForLoop(x) => label_or(&x.label, x.for_token.span),
        E::Group(x) => first(&x.expr),
        E::If(x) => x.if_token.span,
        E::Index(x) => first(&x.expr),
        E::Infer(x) => x.underscore_token.span,
        E::Let(x) => x.let_token.span,
        E::Lit(x) => x.lit.span(),
        E::Loop(x) => label_or(&x.label, x.loop_token.span),
        E::Macro(x) => path_first(&x.mac.path),
        E::Match(x) => x.match_token.span,
        E::MethodCall(x) => first(&x.receiver),
        E::Paren(x) => x.paren_token.span.open(),
        E::Path(x) => x.qself.as_ref().map(|q| q.lt_token.spans[0]).unwrap_or_else(|| path_first(&x.path)),
        E::Range(x) => x.start.as_ref().map(|s| first(s)).unwrap_or_else(|| match &x.limits {
            syn::RangeLimits::HalfOpen(t) => t.spans[0],
            syn::RangeLimits::Closed(t) => t.spans[0],
        }),
        E::RawAddr(x) => x.and_token.spans[0],
        E::Reference(x) => x.and_token.spans[0],
        E::Repeat(x) => x.bracket_token.span.open(),
        E::Return(x) => x.return_token.span,
        E::Struct(x) => x.qself.as_ref().map(|q| q.lt_token.spans[0]).unwrap_or_else(|| path_first(&x.path)),
        E::Try(x) => first(&x.expr),
        E::TryBlock(x) => x.try_token.span,
        E::Tuple(x) => x.paren_token.span.open(),
        E::Unary(x) => match x.op {
            syn::UnOp::Deref(t) => t.spans[0],
            syn::UnOp::Not(t) => t.spans[0],
            syn::UnOp::Neg(t) => t.spans[0],
            _ => first(&x.expr),
        },
        E::Unsafe(x) => x.unsafe_token.span,
        E::While(x) => label_or(&x.label, x.while_token.span),
        E::Yield(x) => x.yield_token.span,
        E::Verbatim(ts) => ts.clone().into_iter().next().map(|t| t.span()).unwrap_or_else(Span::call_site),
        _ => Span::call_site(),
    }
}

/// The last token of an expression.
pub fn last(e: &syn::Expr) -> Span {
    use syn::Expr as E;
    match e {
        E::Array(x) => x.bracket_token.span.close(),
        E::Assign(x) => last(&x.right),
        E::Async(x) => x.block.brace_token.span.close(),
        E::Await(x) => x.await_token.span,
        E::Binary(x) => last(&x.right),
        E::Block(x) => x.block.brace_token.span.close(),
        E::Break(x) => x.expr.as_ref().map(|e| last(e)).or(x.label.as_ref().map(|l| l.ident.span())).unwrap_or(x.break_token.span),
        E::Call(x) => x.paren_token.span.close(),
        E::Cast(x) => x.ty.span(),
        E::Closure(x) => last(&x.body),
        E::Const(x) => x.block.brace_token.span.close(),
        E::Continue(x) => x.label.as_ref().map(|l| l.ident.span()).unwrap_or(x.continue_token.span),
        E::Field(x) => match &x.member {
            syn::Member::Named(i) => i.span(),
            syn::Member::Unnamed(i) => i.span,
        },
        E::ForLoop(x) => x.body.brace_token.span.close(),
        E::Group(x) => last(&x.expr),
        E::If(x) => x.else_branch.as_ref().map(|(_, e)| last(e)).unwrap_or(x.then_branch.brace_token.span.close()),
        E::Index(x) => x.bracket_token.span.close(),
        E::Infer(x) => x.underscore_token.span,
        E::Let(x) => last(&x.expr),
        E::Lit(x) => x.lit.span(),
        E::Loop(x) => x.body.brace_token.span.close(),
        E::Macro(x) => x.mac.delimiter.span().close(),
        E::Match(x) => x.brace_token.span.close(),
        E::MethodCall(x) => x.paren_token.span.close(),
        E::Paren(x) => x.paren_token.span.close(),
        E::Path(x) => path_last(&x.path),
        E::Range(x) => x.end.as_ref().map(|s| last(s)).unwrap_or_else(|| match &x.limits {
            syn::RangeLimits::HalfOpen(t) => t.spans[1],
            syn::RangeLimits::Closed(t) => t.spans[2],
        }),
        E::RawAddr(x) => last(&x.expr),
        E::Reference(x) => last(&x.expr),
        E::Repeat(x) => x.bracket_token.span.close(),
        E::Return(x) => x.expr.as_ref().map(|e| last(e)).unwrap_or(x.return_token.span),
        E::Struct(x) => x.brace_token.span.close(),
        E::Try(x) => x.question_token.spans[0],
        E::TryBlock(x) => x.block.brace_token.span.close(),
        E::Tuple(x) => x.paren_token.span.close(),
        E::Unary(x) => last(&x.expr),
        E::Unsafe(x) => x.block.brace_token.span.close(),
        E::While(x) => x.body.brace_token.span.close(),
        E::Yield(x) => x.expr.as_ref().map(|e| last(e)).unwrap_or(x.yield_token.span),
        E::Verbatim(ts) => ts.clone().into_iter().last().map(|t| t.span()).unwrap_or_else(Span::call_site),
        _ => Span::call_site(),
    }
}
