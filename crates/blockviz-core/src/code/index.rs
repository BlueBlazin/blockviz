//! Project-wide symbol index and a deliberately small type inference.
//!
//! The goal is not soundness; it is to name things the way a reader would:
//! `self.transformer.wte(idx)` is an `Embedding`, `block` in
//! `for block in self.transformer.h` is a `Block` repeated `n_layer` times, and
//! `model.configure_optimizers(...)` is `GPT.configure_optimizers`.

use super::source::Source;
use super::syntax::{self as ast, DefKind, Expr, Lang, Ranged, Stmt, TextRange};
use super::walk;
use crate::ir::Id;
use crate::text::{clip, squash};
use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// One source file, parsed and lowered to the shared syntax tree.
pub struct Module {
    pub file: u32,
    pub path: String,
    /// Python's dotted module name (`app.core.db`); for other languages the
    /// path without its extension, dotted the same way.
    pub dotted: String,
    pub package: String,
    pub src: Source,
    pub ast: Vec<Stmt>,
    pub notebook: bool,
    pub lang: Lang,
    /// Badges the front end found (`client` for a React "use client" module).
    pub tags: Vec<String>,
    /// First syntax error (the parser recovered and the tree is still usable).
    pub error: Option<String>,
    /// Modules of one namespace (the files of a Go package) see each other's
    /// top-level definitions without importing them.
    pub namespace: Option<String>,
}

#[derive(Clone, Copy)]
pub struct FnDef<'p> {
    pub name: &'p str,
    pub args: &'p [ast::Param],
    pub body: &'p [Stmt],
    pub decorators: &'p [ast::Decorator],
    pub range: TextRange,
    pub is_async: bool,
    pub kind: DefKind,
    pub doc: Option<&'p str>,
    /// The declared return type.
    pub returns: Option<&'p Expr>,
}

impl<'p> FnDef<'p> {
    pub fn of(f: &'p ast::FunctionDef) -> Self {
        FnDef { name: &f.name, args: &f.params, body: &f.body, decorators: &f.decorators, range: f.range, is_async: f.is_async, kind: f.kind, doc: f.doc.as_deref(), returns: f.returns.as_deref() }
    }
    pub fn from_stmt(s: &'p Stmt) -> Option<Self> {
        match s {
            Stmt::FunctionDef(f) => Some(FnDef::of(f)),
            _ => None,
        }
    }
    pub fn params(&self) -> Vec<String> {
        walk::param_names(self.args)
    }
    pub fn decorated(&self, name: &str) -> bool {
        self.decorators.iter().any(|d| decorator_name(&d.expression) == name)
    }
    /// A Python docstring, or documentation written above the definition.
    pub fn docstring(&self) -> Option<&'p str> {
        docstring(self.body).or(self.doc)
    }
}

pub fn decorator_name(d: &Expr) -> &str {
    match d {
        Expr::Name(n) => n.id.as_str(),
        Expr::Attribute(a) => a.attr.as_str(),
        Expr::Call(c) => decorator_name(&c.func),
        _ => "",
    }
}

pub fn docstring(body: &[Stmt]) -> Option<&str> {
    if let Some(Stmt::Expr(e)) = body.first() {
        if let Expr::StringLiteral(s) = &*e.value {
            return Some(&s.value);
        }
    }
    None
}

/// Base class expressions of a class definition.
pub fn class_bases(c: &ast::ClassDef) -> &[Expr] {
    &c.bases
}

#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Unknown,
    Module(usize),
    /// An external symbol by dotted path (`torch.nn.functional.softmax`, `builtins.len`).
    Ext(String),
    /// An instance of an external class.
    ExtInst(String),
    Class(usize),
    Inst(usize),
    Func(usize),
    /// Attribute-style container (`nn.ModuleDict(dict(wte=...))`).
    Dict(Vec<(String, Ty)>),
    /// Homogeneous container with an optional repeat count (`nn.ModuleList([Block(c) for _ in range(n)])`).
    List(Box<Ty>, Option<String>),
}

#[derive(Clone, Debug)]
pub enum Sym {
    Class(usize),
    Func(usize),
    Module(usize),
    Ext(String),
    Member(usize, String),
    Var,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FnKind {
    Function,
    Method,
    ClassMethod,
    StaticMethod,
    Property,
}

pub struct Field {
    pub name: String,
    pub ty: Ty,
    pub ctor: String,
    pub range: TextRange,
}

pub struct ClassInfo<'p> {
    pub name: String,
    pub module: usize,
    pub def: &'p ast::ClassDef,
    pub bases: Vec<Ty>,
    pub base_text: Vec<String>,
    pub methods: Vec<(String, usize)>,
    /// Methods other modules add to the class (Rust `impl` blocks elsewhere):
    /// found by lookup, drawn where they are written.
    pub extra: Vec<(String, usize)>,
    pub fields: Vec<Field>,
    pub data: bool,
    pub node: Id,
}

pub struct FuncInfo<'p> {
    pub name: String,
    pub module: usize,
    pub class: Option<usize>,
    pub def: FnDef<'p>,
    pub node: Id,
    pub kind: FnKind,
}

pub struct Index<'p> {
    pub modules: &'p [Module],
    /// Python modules by dotted name.
    pub by_dotted: HashMap<String, usize>,
    /// Every module by repository path.
    pub by_path: HashMap<String, usize>,
    /// Modules whose names a module re-exports wholesale (`export * from "./x"`).
    pub stars: Vec<Vec<usize>>,
    /// Non-code project files a module imports (`import "./styles.css"`).
    pub assets: Vec<Vec<String>>,
    pub syms: Vec<HashMap<String, Sym>>,
    pub classes: Vec<ClassInfo<'p>>,
    pub funcs: Vec<FuncInfo<'p>>,
    /// Definitions found anywhere at module level, keyed by (module, def start offset).
    pub defs_at: HashMap<(usize, u32), Sym>,
    pub var_types: RefCell<HashMap<(usize, String), Ty>>,
    returns: RefCell<HashMap<usize, Ty>>,
    /// Which project modules each module imports, with the imported names.
    pub imports: Vec<Vec<(usize, Vec<String>)>>,
    /// External top-level packages each module imports.
    pub externals: Vec<BTreeSet<String>>,
    /// The namespace each module shares with others (a Go package), if any.
    pub namespace_of: Vec<Option<usize>>,
    /// Each namespace's definitions, as members of the module defining them.
    pub namespaces: Vec<HashMap<String, Sym>>,
    /// The project types that implement each contract (see `contracts`),
    /// by class; `implements` is the same relation from the other side.
    pub implementers: Vec<Vec<usize>>,
    pub implements: Vec<Vec<usize>>,
    /// A Go interface's method signatures, embedded interfaces included.
    pub(super) go_needs: Vec<Option<HashSet<String>>>,
    /// The types Go code puts where each interface is expected, and whether
    /// in test code.
    pub(super) flows: Vec<Vec<(usize, bool)>>,
    /// Contracts some type names as a base without it resolving.
    pub(super) unresolved: Vec<bool>,
    /// Whether non-test code names each class outside its own definition.
    pub(super) named: OnceCell<Vec<bool>>,
}

pub trait Locals {
    fn local(&self, name: &str) -> Option<Ty>;
}

impl Locals for HashMap<String, Ty> {
    fn local(&self, name: &str) -> Option<Ty> {
        self.get(name).cloned()
    }
}

pub struct NoLocals;
impl Locals for NoLocals {
    fn local(&self, _: &str) -> Option<Ty> {
        None
    }
}

const BUILTINS: &[&str] = &[
    "abs", "all", "any", "bool", "bytes", "callable", "chr", "dict", "dir", "divmod", "enumerate", "eval", "exec", "filter", "float", "format", "frozenset", "getattr", "globals", "hasattr", "hash", "hex", "id", "input", "int", "isinstance", "issubclass", "iter", "len", "list", "locals", "map", "max", "min", "next", "object", "open", "ord", "pow", "print", "range", "repr", "reversed", "round", "set", "setattr", "slice", "sorted", "str", "sum", "super", "tuple", "type", "vars", "zip", "ValueError", "TypeError", "KeyError", "RuntimeError", "Exception", "NotImplementedError", "AssertionError", "IndexError", "StopIteration", "AttributeError", "OSError", "FileNotFoundError",
];

pub fn is_builtin(name: &str) -> bool {
    BUILTINS.contains(&name)
}

/// JavaScript globals of browsers and Node: library calls, not project code.
pub const JS_GLOBALS: &[&str] = &[
    "console", "process", "fetch", "JSON", "Math", "Object", "Array", "Promise", "Number", "String", "Boolean", "Symbol", "Date", "RegExp", "Error", "TypeError", "RangeError", "SyntaxError", "Map", "Set", "WeakMap", "WeakSet", "Reflect", "Proxy", "Intl", "URL", "URLSearchParams", "Buffer", "setTimeout", "setInterval", "clearTimeout", "clearInterval", "queueMicrotask", "structuredClone", "parseInt", "parseFloat", "isNaN", "isFinite", "encodeURIComponent", "decodeURIComponent", "encodeURI", "decodeURI", "atob", "btoa", "document", "window", "globalThis", "navigator", "location", "history", "localStorage", "sessionStorage", "crypto", "performance", "require", "module", "exports", "__dirname", "__filename", "AbortController", "Headers", "Request", "Response", "FormData", "Blob", "File", "FileReader", "WebSocket", "EventSource", "TextEncoder", "TextDecoder", "Uint8Array", "ArrayBuffer", "BigInt", "alert", "confirm", "requestAnimationFrame", "addEventListener", "removeEventListener", "Event", "CustomEvent", "MutationObserver", "IntersectionObserver", "ResizeObserver", "Worker",
];

/// Last segment of a count expression: `config.n_layer` → `n_layer`.
pub fn count_text(e: &str) -> String {
    let s = squash(e);
    let tail = if s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') { s.rsplit('.').next().unwrap_or(&s).to_string() } else { s };
    clip(&tail, 18)
}

impl<'p> Index<'p> {
    /// Builds symbols for all modules. `alloc` creates a detached IR node for each definition.
    pub fn build(modules: &'p [Module], mut alloc: impl FnMut(&str, bool) -> Id) -> Self {
        let mut by_dotted = HashMap::new();
        for (i, m) in modules.iter().enumerate().filter(|(_, m)| m.lang == Lang::Python) {
            by_dotted.insert(m.dotted.clone(), i);
        }
        let by_path = modules.iter().enumerate().map(|(i, m)| (m.path.clone(), i)).collect();
        let mut ix = Index {
            modules,
            by_dotted,
            by_path,
            stars: vec![vec![]; modules.len()],
            assets: vec![vec![]; modules.len()],
            syms: vec![HashMap::new(); modules.len()],
            classes: vec![],
            funcs: vec![],
            defs_at: HashMap::new(),
            var_types: RefCell::new(HashMap::new()),
            returns: RefCell::new(HashMap::new()),
            imports: vec![vec![]; modules.len()],
            externals: vec![BTreeSet::new(); modules.len()],
            namespace_of: vec![None; modules.len()],
            namespaces: vec![],
            implementers: vec![],
            implements: vec![],
            go_needs: vec![],
            flows: vec![],
            unresolved: vec![],
            named: OnceCell::new(),
        };
        for (mi, m) in modules.iter().enumerate() {
            ix.collect_defs(mi, &m.ast, &mut alloc, true);
        }
        for (mi, m) in modules.iter().enumerate() {
            ix.collect_imports(mi, &m.ast, true);
        }
        // A namespace's files use each other's definitions as if each had
        // imported them by name (what a file defines or imports itself takes
        // precedence): one table per namespace, looked up on demand.
        let mut namespaces: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (mi, m) in modules.iter().enumerate() {
            if let Some(ns) = &m.namespace {
                namespaces.entry(ns.as_str()).or_default().push(mi);
            }
        }
        for peers in namespaces.values().filter(|p| p.len() > 1) {
            let ns = ix.namespaces.len();
            let mut table: HashMap<String, Sym> = HashMap::new();
            for &p in peers {
                ix.namespace_of[p] = Some(ns);
                let mut names: Vec<&String> = ix.syms[p].iter().filter(|(_, s)| matches!(s, Sym::Class(_) | Sym::Func(_) | Sym::Var)).map(|(n, _)| n).collect();
                names.sort();
                for n in names {
                    table.entry(n.clone()).or_insert(Sym::Member(p, n.clone()));
                }
            }
            ix.namespaces.push(table);
        }
        for (mi, m) in modules.iter().enumerate() {
            ix.collect_exports(mi, &m.ast);
        }
        for c in 0..ix.classes.len() {
            let def = ix.classes[c].def;
            let m = ix.classes[c].module;
            let bases: Vec<Ty> = class_bases(def).iter().map(|b| ix.infer(m, b, &NoLocals)).collect();
            ix.classes[c].bases = bases;
        }
        // Extensions lend their methods to the type they extend.
        for c in 0..ix.classes.len() {
            if ix.classes[c].def.kind != ast::ClassKind::Extension {
                continue;
            }
            if let Some(Ty::Class(t)) = ix.classes[c].bases.first().cloned() {
                if t != c {
                    let methods = ix.classes[c].methods.clone();
                    ix.classes[t].extra.extend(methods);
                }
            }
        }
        for c in 0..ix.classes.len() {
            let fields = ix.infer_fields(c);
            ix.classes[c].fields = fields;
        }
        ix.find_contracts();
        // Objects a module creates once (`const users = new UsersService()`) are
        // known everywhere in it, including in functions written above them.
        for (mi, m) in modules.iter().enumerate() {
            let mut seen: HashMap<&str, usize> = HashMap::new();
            for s in &m.ast {
                if let Stmt::Assign(a) = s {
                    if let [Expr::Name(n)] = &a.targets[..] {
                        *seen.entry(n.id.as_str()).or_default() += 1;
                    }
                }
            }
            for s in &m.ast {
                let Stmt::Assign(a) = s else { continue };
                let [Expr::Name(n)] = &a.targets[..] else { continue };
                if seen[n.id.as_str()] != 1 || !matches!(&*a.value, Expr::Call(_)) {
                    continue;
                }
                let ty = ix.infer(mi, &a.value, &NoLocals);
                if matches!(ty, Ty::Inst(_) | Ty::ExtInst(_)) {
                    ix.var_types.borrow_mut().insert((mi, n.id.clone()), ty);
                }
            }
        }
        ix
    }

    fn collect_defs(&mut self, mi: usize, body: &'p [Stmt], alloc: &mut impl FnMut(&str, bool) -> Id, top: bool) {
        for s in body {
            match s {
                Stmt::ClassDef(c) => {
                    let id = self.register_class(mi, c, alloc);
                    // An extension adds to a type named elsewhere; it binds no name.
                    if c.kind != ast::ClassKind::Extension && (top || !self.syms[mi].contains_key(c.name.as_str())) {
                        self.syms[mi].insert(c.name.to_string(), Sym::Class(id));
                    }
                }
                Stmt::FunctionDef(_) => {
                    let f = FnDef::from_stmt(s).unwrap();
                    let node = alloc(f.name, false);
                    let id = self.funcs.len();
                    self.funcs.push(FuncInfo { name: f.name.to_string(), module: mi, class: None, def: f, node, kind: FnKind::Function });
                    self.defs_at.insert((mi, u32::from(f.range.start())), Sym::Func(id));
                    self.syms[mi].entry(f.name.to_string()).or_insert(Sym::Func(id));
                }
                // Definitions under top-level control flow (`if __name__ == ...`, `try`) are still module symbols.
                Stmt::If(x) => {
                    self.collect_defs(mi, &x.body, alloc, false);
                    for c in &x.elif_else_clauses {
                        self.collect_defs(mi, &c.body, alloc, false);
                    }
                }
                Stmt::Try(x) => {
                    self.collect_defs(mi, &x.body, alloc, false);
                    for h in &x.handlers {
                        self.collect_defs(mi, &h.body, alloc, false);
                    }
                }
                Stmt::With(x) => self.collect_defs(mi, &x.body, alloc, false),
                Stmt::Assign(a) => {
                    let mut names = vec![];
                    a.targets.iter().for_each(|t| walk::target_names(t, &mut names));
                    for n in names {
                        self.syms[mi].entry(n).or_insert(Sym::Var);
                    }
                }
                Stmt::AnnAssign(a) => {
                    let mut names = vec![];
                    walk::target_names(&a.target, &mut names);
                    for n in names {
                        self.syms[mi].entry(n).or_insert(Sym::Var);
                    }
                }
                _ => {}
            }
        }
    }

    fn register_class(&mut self, mi: usize, c: &'p ast::ClassDef, alloc: &mut impl FnMut(&str, bool) -> Id) -> usize {
        let node = alloc(&c.name, true);
        let cid = self.classes.len();
        let src = &self.modules[mi].src;
        let base_text: Vec<String> = class_bases(c).iter().map(|b| squash(src.of(b))).collect();
        self.classes.push(ClassInfo { name: c.name.to_string(), module: mi, def: c, bases: vec![], base_text, methods: vec![], extra: vec![], fields: vec![], data: false, node });
        self.defs_at.insert((mi, u32::from(c.range.start())), Sym::Class(cid));
        let mut methods = vec![];
        // A Go interface lists signatures (its rows); there is nothing to draw or call.
        let contract = self.modules[mi].lang == Lang::Go && c.kind == ast::ClassKind::Interface && c.body.iter().all(|s| matches!(s, Stmt::FunctionDef(f) if f.body.is_empty()));
        for s in c.body.iter().filter(|_| !contract) {
            if let Some(f) = FnDef::from_stmt(s) {
                let node = alloc(f.name, false);
                let kind = match f.kind {
                    DefKind::StaticMethod => FnKind::StaticMethod,
                    DefKind::ClassMethod => FnKind::ClassMethod,
                    DefKind::Getter => FnKind::Property,
                    _ => FnKind::Method,
                };
                let id = self.funcs.len();
                self.funcs.push(FuncInfo { name: f.name.to_string(), module: mi, class: Some(cid), def: f, node, kind });
                self.defs_at.insert((mi, u32::from(f.range.start())), Sym::Func(id));
                methods.push((f.name.to_string(), id));
            }
        }
        let data_decorator = c.decorators.iter().any(|d| matches!(decorator_name(&d.expression), "dataclass" | "dataclasses" | "attrs" | "define" | "frozen"));
        let data_base = self.classes[cid].base_text.iter().any(|b| {
            let tail = b.rsplit('.').next().unwrap_or(b);
            matches!(tail, "NamedTuple" | "TypedDict" | "BaseModel" | "Enum" | "IntEnum" | "StrEnum" | "BaseSettings")
        });
        let behaviour = methods.iter().any(|(n, _)| !n.starts_with("__"));
        // Interfaces, enums and traits without methods describe data, as does
        // a class of field declarations only (a DTO, a Rust struct).
        let lang = self.modules[mi].lang;
        let fields_only = lang.declares_fields() && methods.is_empty() && c.kind == ast::ClassKind::Class && c.body.iter().any(|s| matches!(s, Stmt::AnnAssign(_) | Stmt::Assign(_)));
        let declared = matches!(c.kind, ast::ClassKind::Interface | ast::ClassKind::Enum | ast::ClassKind::Trait) && methods.is_empty() || fields_only || contract;
        self.classes[cid].data = declared || (data_decorator || data_base) && !behaviour;
        self.classes[cid].methods = methods;
        cid
    }

    fn collect_imports(&mut self, mi: usize, body: &'p [Stmt], top: bool) {
        for s in body {
            match s {
                Stmt::Import(x) if x.style == ast::ModuleStyle::Path && x.declaration => {
                    if let Some(t) = x.resolved.as_deref().and_then(|p| self.by_path.get(p).copied()) {
                        for a in &x.names {
                            let local = a.asname.clone().unwrap_or_else(|| a.name.clone());
                            self.syms[mi].entry(local).or_insert(Sym::Module(t));
                        }
                    }
                }
                Stmt::Import(x) if x.style == ast::ModuleStyle::Path => self.path_import(mi, x.resolved.as_deref(), &x.names[0].name, x.names.iter().map(|a| (None, a.asname.clone())).collect(), top),
                Stmt::ImportFrom(x) if x.style == ast::ModuleStyle::Path => {
                    let names = x.names.iter().map(|a| (Some(a.name.clone()), Some(a.asname.clone().unwrap_or_else(|| a.name.clone())))).collect();
                    self.path_import(mi, x.resolved.as_deref(), x.module.as_deref().unwrap_or(""), names, top)
                }
                Stmt::Import(x) => {
                    for a in &x.names {
                        let full = a.name.to_string();
                        let local = a.asname.as_ref().map(|n| n.to_string()).unwrap_or_else(|| full.split('.').next().unwrap_or("").to_string());
                        let target = if a.asname.is_some() { full.clone() } else { local.clone() };
                        let sym = match self.resolve_module(mi, &target, 0) {
                            Some(t) => {
                                self.note_import(mi, t, vec![]);
                                Sym::Module(t)
                            }
                            None => {
                                self.externals[mi].insert(full.split('.').next().unwrap_or("").to_string());
                                Sym::Ext(target)
                            }
                        };
                        if top || !self.syms[mi].contains_key(&local) {
                            self.syms[mi].insert(local, sym);
                        }
                    }
                }
                Stmt::ImportFrom(x) => {
                    let base = x.module.as_ref().map(|m| m.to_string()).unwrap_or_default();
                    let level = x.level;
                    let target = self.resolve_module(mi, &base, level);
                    let mut names = vec![];
                    for a in &x.names {
                        let name = a.name.to_string();
                        if name == "*" {
                            continue;
                        }
                        let local = a.asname.as_ref().unwrap_or(&a.name).to_string();
                        let sym = match target {
                            Some(t) => {
                                // `from pkg import submodule` imports the submodule, not the package.
                                let sub = format!("{}.{}", self.modules[t].dotted, name);
                                match self.by_dotted.get(&sub) {
                                    Some(&s) => {
                                        self.note_import(mi, s, vec![]);
                                        Sym::Module(s)
                                    }
                                    None => {
                                        names.push(name.clone());
                                        Sym::Member(t, name.clone())
                                    }
                                }
                            }
                            None => {
                                if level == 0 {
                                    self.externals[mi].insert(base.split('.').next().unwrap_or("").to_string());
                                }
                                Sym::Ext(if base.is_empty() { name.clone() } else { format!("{base}.{name}") })
                            }
                        };
                        if top || !self.syms[mi].contains_key(&local) {
                            self.syms[mi].insert(local, sym);
                        }
                    }
                    if let Some(t) = target {
                        if !names.is_empty() || x.names.iter().any(|a| a.name.as_str() == "*") {
                            self.note_import(mi, t, names);
                        }
                    }
                }
                Stmt::If(x) => {
                    self.collect_imports(mi, &x.body, false);
                    for c in &x.elif_else_clauses {
                        self.collect_imports(mi, &c.body, false);
                    }
                }
                Stmt::Try(x) => {
                    self.collect_imports(mi, &x.body, false);
                    for h in &x.handlers {
                        self.collect_imports(mi, &h.body, false);
                    }
                }
                Stmt::FunctionDef(f) => self.collect_imports(mi, &f.body, false),
                Stmt::ClassDef(c) => {
                    for s in &c.body {
                        if let Stmt::FunctionDef(f) = s {
                            self.collect_imports(mi, &f.body, false);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// An import by path or package (JavaScript): `names` are (imported name,
    /// local name); an imported name of `None` binds the whole module, `*`
    /// re-exports it, and `default` is a default export.
    fn path_import(&mut self, mi: usize, resolved: Option<&str>, spec: &str, names: Vec<(Option<String>, Option<String>)>, top: bool) {
        let target = resolved.and_then(|p| self.by_path.get(p).copied());
        if target.is_none() {
            match resolved {
                // A stylesheet, image or data file in the project.
                Some(p) => {
                    if !self.assets[mi].iter().any(|a| a == p) {
                        self.assets[mi].push(p.to_string());
                    }
                }
                // Go: modules outside the standard library, by module path.
                // (A standard package that says what the code works with, like
                // `database/sql`, is recorded too.)
                None if self.modules[mi].lang == Lang::Go => {
                    if !crate::go::resolve::is_std(spec) {
                        self.externals[mi].insert(crate::go::resolve::module_path(spec));
                    } else if crate::vocabulary::library_domains(spec) != crate::vocabulary::Domains::default() {
                        self.externals[mi].insert(spec.to_string());
                    }
                }
                None if self.modules[mi].lang == Lang::Rust => {
                    let krate = spec.split(['.', ':']).next().unwrap_or(spec);
                    if !matches!(krate, "crate" | "self" | "super" | "") {
                        self.externals[mi].insert(krate.to_string());
                    }
                }
                None if !spec.starts_with('.') && !spec.starts_with('/') => {
                    let (package, _) = crate::js::resolve::package_name(spec);
                    self.externals[mi].insert(package.to_string());
                }
                None => {}
            }
        }
        // A project file that is missing or is not code (`./data.json`) is
        // known by its file name; a package by its specifier.
        let relative = spec.starts_with('.') || spec.starts_with('/') || spec.contains("::");
        let spec = if relative { spec.rsplit('/').next().unwrap_or(spec).split('.').next().unwrap_or(spec) } else { spec };
        let mut used = vec![];
        for (imported, local) in names {
            let sym = match (target, imported.as_deref()) {
                (Some(t), None) => Sym::Module(t),
                (Some(t), Some("*")) => {
                    if !self.stars[mi].contains(&t) {
                        self.stars[mi].push(t);
                    }
                    used.push("*".to_string());
                    continue;
                }
                (Some(t), Some(name)) => {
                    used.push(name.to_string());
                    Sym::Member(t, name.to_string())
                }
                // A package's default export is the package itself.
                (None, None | Some("default") | Some("*")) => Sym::Ext(spec.to_string()),
                (None, Some(name)) => Sym::Ext(format!("{spec}.{name}")),
            };
            if let Some(local) = local {
                if top || !self.syms[mi].contains_key(&local) {
                    self.syms[mi].insert(local, sym);
                }
            }
        }
        if let Some(t) = target {
            self.note_import(mi, t, used.into_iter().filter(|n| n != "*").collect());
        }
    }

    /// `export { a as b }`, `export default x`: exported names refer to local ones.
    fn collect_exports(&mut self, mi: usize, body: &[Stmt]) {
        for s in body {
            if let Stmt::Export(x) = s {
                for (local, exported) in &x.names {
                    if local != exported {
                        if let Some(sym) = self.syms[mi].get(local).cloned() {
                            self.syms[mi].entry(exported.clone()).or_insert(sym);
                        }
                    }
                }
            }
        }
    }

    fn note_import(&mut self, mi: usize, target: usize, names: Vec<String>) {
        if target == mi {
            return;
        }
        match self.imports[mi].iter_mut().find(|(t, _)| *t == target) {
            Some((_, n)) => n.extend(names),
            None => self.imports[mi].push((target, names)),
        }
    }

    /// Resolves an import to a project module: relative to the importer first,
    /// then from the root, then by unique suffix (for `src/` layouts).
    pub fn resolve_module(&self, mi: usize, name: &str, level: u32) -> Option<usize> {
        let pkg = &self.modules[mi].package;
        if level > 0 {
            let mut parts: Vec<&str> = if pkg.is_empty() { vec![] } else { pkg.split('.').collect() };
            for _ in 1..level {
                parts.pop();
            }
            if !name.is_empty() {
                parts.push(name);
            }
            return self.by_dotted.get(&parts.join(".")).copied();
        }
        if name.is_empty() {
            return None;
        }
        if !pkg.is_empty() {
            if let Some(&t) = self.by_dotted.get(&format!("{pkg}.{name}")) {
                return Some(t);
            }
        }
        if let Some(&t) = self.by_dotted.get(name) {
            return Some(t);
        }
        let suffix = format!(".{name}");
        let mut hits = self.by_dotted.iter().filter(|(d, _)| d.ends_with(&suffix));
        match (hits.next(), hits.next()) {
            (Some((_, &t)), None) => Some(t),
            _ => None,
        }
    }

    /// The module that defines a name `m` makes available, following
    /// re-exports (`pub use db::Db;` in `lib.rs`, `export * from "./x"`,
    /// `from .models import User` in `__init__.py`).
    pub fn defined_in(&self, m: usize, name: &str) -> usize {
        let (mut m, mut name) = (m, name.to_string());
        for _ in 0..8 {
            match self.sym(m, &name) {
                Some(Sym::Member(t, n)) => {
                    m = *t;
                    name = n.clone();
                }
                Some(Sym::Module(t)) => return *t,
                Some(_) => return m,
                None => match self.stars[m].iter().find(|&&t| self.syms[t].contains_key(&name)) {
                    Some(&t) => m = t,
                    None => return m,
                },
            }
        }
        m
    }

    /// What a name is in a module: its own definitions and imports, then
    /// what the other files of its namespace define.
    pub fn sym(&self, mi: usize, name: &str) -> Option<&Sym> {
        self.syms[mi].get(name).or_else(|| self.namespace_member(mi, name))
    }

    /// A definition of another file of the module's namespace.
    pub fn namespace_member(&self, mi: usize, name: &str) -> Option<&Sym> {
        let ns = self.namespace_of[mi]?;
        self.namespaces[ns].get(name).filter(|s| !matches!(s, Sym::Member(p, _) if *p == mi))
    }

    pub fn sym_ty(&self, mi: usize, name: &str, depth: u32) -> Ty {
        if depth > 6 {
            return Ty::Unknown;
        }
        match self.sym(mi, name) {
            Some(Sym::Class(c)) => Ty::Class(*c),
            Some(Sym::Func(f)) => Ty::Func(*f),
            Some(Sym::Module(m)) => Ty::Module(*m),
            Some(Sym::Ext(p)) => Ty::Ext(p.clone()),
            Some(Sym::Member(m, n)) => self.sym_ty(*m, n, depth + 1),
            Some(Sym::Var) => self.var_types.borrow().get(&(mi, name.to_string())).cloned().unwrap_or(Ty::Unknown),
            None if self.modules[mi].lang.resolves_paths() => {
                // Names re-exported wholesale from another module.
                if let Some(t) = self.stars[mi].iter().copied().find(|&t| self.syms[t].contains_key(name) || !self.stars[t].is_empty()) {
                    let ty = self.sym_ty(t, name, depth + 1);
                    if ty != Ty::Unknown {
                        return ty;
                    }
                }
                if self.modules[mi].lang.is_js() && JS_GLOBALS.contains(&name) {
                    Ty::Ext(format!("globals.{name}"))
                } else {
                    Ty::Unknown
                }
            }
            None if is_builtin(name) => Ty::Ext(format!("builtins.{name}")),
            None => Ty::Unknown,
        }
    }

    /// A member of a module: one of its names, or, for a JavaScript module
    /// that exports one object (`module.exports = app`), that object's.
    pub fn module_member(&self, m: usize, attr: &str) -> Ty {
        let ty = self.sym_ty(m, attr, 0);
        if ty != Ty::Unknown || !self.modules[m].lang.is_js() {
            return ty;
        }
        let mut whole = self.sym_ty(m, "default", 0);
        for _ in 0..4 {
            match whole {
                Ty::Module(next) => whole = self.sym_ty(next, "default", 0),
                Ty::Class(c) | Ty::Inst(c) => return self.method(c, attr).map(Ty::Func).unwrap_or(Ty::Unknown),
                _ => break,
            }
        }
        Ty::Unknown
    }

    pub fn method(&self, c: usize, name: &str) -> Option<usize> {
        self.method_depth(c, name, 0)
    }

    fn method_depth(&self, c: usize, name: &str, depth: u32) -> Option<usize> {
        if depth > 8 {
            return None;
        }
        let cls = &self.classes[c];
        if let Some((_, f)) = cls.methods.iter().chain(&cls.extra).find(|(n, _)| n == name) {
            return Some(*f);
        }
        cls.bases.iter().find_map(|b| if let Ty::Class(p) = b { self.method_depth(*p, name, depth + 1) } else { None })
    }

    pub fn field(&self, c: usize, name: &str) -> Option<&Field> {
        self.field_depth(c, name, 0)
    }

    fn field_depth(&self, c: usize, name: &str, depth: u32) -> Option<&Field> {
        if depth > 8 {
            return None;
        }
        let cls = &self.classes[c];
        if let Some(f) = cls.fields.iter().find(|f| f.name == name) {
            return Some(f);
        }
        cls.bases.iter().find_map(|b| if let Ty::Class(p) = b { self.field_depth(*p, name, depth + 1) } else { None })
    }

    /// What runs when an instance is called: `__call__`, else `forward` (nn.Module convention).
    pub fn call_target(&self, c: usize) -> Option<usize> {
        self.method(c, "__call__").or_else(|| self.method(c, "forward"))
    }

    /// External base classes, for display (`nn.Module`).
    pub fn external_base(&self, c: usize) -> Option<String> {
        let cls = &self.classes[c];
        cls.bases.iter().zip(&cls.base_text).find(|(t, _)| !matches!(t, Ty::Class(_))).map(|(_, text)| text.clone())
    }

    pub fn infer(&self, mi: usize, e: &Expr, locals: &dyn Locals) -> Ty {
        self.infer_depth(mi, e, locals, 0)
    }

    fn infer_depth(&self, mi: usize, e: &Expr, locals: &dyn Locals, depth: u32) -> Ty {
        if depth > 24 {
            return Ty::Unknown;
        }
        let infer = |x: &Expr| self.infer_depth(mi, x, locals, depth + 1);
        match e {
            Expr::Name(n) => locals.local(n.id.as_str()).unwrap_or_else(|| self.sym_ty(mi, n.id.as_str(), 0)),
            Expr::Attribute(a) => {
                let attr = a.attr.as_str();
                match infer(&a.value) {
                    Ty::Module(m) => {
                        let sub = format!("{}.{}", self.modules[m].dotted, attr);
                        match self.by_dotted.get(&sub) {
                            Some(&s) => Ty::Module(s),
                            None => self.module_member(m, attr),
                        }
                    }
                    Ty::Ext(p) => Ty::Ext(format!("{p}.{attr}")),
                    Ty::Inst(c) => match self.field(c, attr) {
                        Some(f) => f.ty.clone(),
                        None => self.method(c, attr).map(Ty::Func).unwrap_or(Ty::Unknown),
                    },
                    Ty::Class(c) => self.method(c, attr).map(Ty::Func).unwrap_or(Ty::Unknown),
                    Ty::Dict(fields) => fields.into_iter().find(|(k, _)| k == attr).map(|(_, t)| t).unwrap_or(Ty::Unknown),
                    _ => Ty::Unknown,
                }
            }
            Expr::Call(c) => {
                let f = infer(&c.func);
                match f {
                    Ty::Class(k) => Ty::Inst(k),
                    Ty::Func(fid) => {
                        // `cls(...)` style constructors in classmethods.
                        self.return_ty(fid)
                    }
                    Ty::Ext(p) => {
                        let last = p.rsplit('.').next().unwrap_or(&p).to_string();
                        let first_arg = c.arguments.args.first();
                        if matches!(last.as_str(), "ModuleDict" | "ParameterDict") || (p == "builtins.dict" && !c.arguments.keywords.is_empty()) {
                            let mut fields = vec![];
                            if p == "builtins.dict" {
                                for k in c.arguments.keywords.iter() {
                                    if let Some(name) = &k.arg {
                                        fields.push((name.to_string(), infer(&k.value)));
                                    }
                                }
                            } else if let Some(a) = first_arg {
                                if let Ty::Dict(inner) = infer(a) {
                                    fields = inner;
                                }
                            }
                            return Ty::Dict(fields);
                        }
                        if matches!(last.as_str(), "ModuleList" | "ParameterList" | "Sequential") {
                            if let Some(a) = first_arg {
                                if let t @ Ty::List(..) = infer(a) {
                                    return t;
                                }
                            }
                            return Ty::List(Box::new(Ty::Unknown), None);
                        }
                        // Wrappers (`torch.compile(model)`, `DDP(model)`) still behave like what they wrap.
                        if let Some(a) = first_arg {
                            if let t @ Ty::Inst(_) = infer(a) {
                                return t;
                            }
                        }
                        if last.chars().next().is_some_and(|ch| ch.is_uppercase()) {
                            Ty::ExtInst(p)
                        } else {
                            Ty::Unknown
                        }
                    }
                    Ty::Inst(k) => self.call_target(k).map(|f| self.return_ty(f)).unwrap_or(Ty::Unknown),
                    _ => Ty::Unknown,
                }
            }
            Expr::Comp(lc) if lc.kind == ast::CompKind::List => {
                let elem = infer(&lc.elt);
                let count = lc.generators.first().and_then(|g| range_count(&self.modules[mi].src, &g.iter));
                Ty::List(Box::new(elem), count)
            }
            Expr::List(l) if !l.elts.is_empty() => Ty::List(Box::new(infer(&l.elts[0])), Some(l.elts.len().to_string())),
            Expr::Tuple(l) if !l.elts.is_empty() => Ty::List(Box::new(infer(&l.elts[0])), Some(l.elts.len().to_string())),
            Expr::Dict(d) => {
                let mut fields = vec![];
                for item in &d.items {
                    if let Some(Expr::StringLiteral(k)) = &item.key {
                        fields.push((k.value.clone(), infer(&item.value)));
                    }
                }
                Ty::Dict(fields)
            }
            Expr::If(x) => match infer(&x.body) {
                Ty::Unknown => infer(&x.orelse),
                t => t,
            },
            Expr::Subscript(s) => match infer(&s.value) {
                Ty::List(t, _) => *t,
                _ => Ty::Unknown,
            },
            Expr::Await(a) => infer(&a.value),
            _ => Ty::Unknown,
        }
    }

    /// The type of a value declared with an annotation: an instance of the
    /// class it names (`users: UsersService`, `config: &Config`), or a list of
    /// them (`blocks: Vec<Block>`). Library types count in languages whose
    /// annotations are checked (TypeScript, Rust).
    pub fn annotated(&self, mi: usize, annotation: &Expr, locals: &dyn Locals) -> Ty {
        if let Expr::Subscript(s) = annotation {
            if let Expr::Name(n) = &*s.value {
                if matches!(n.id.as_str(), "list" | "List" | "Sequence" | "Array" | "ReadonlyArray") || crate::rust::lower::SEQUENCES.contains(&n.id.as_str()) {
                    return match self.annotated(mi, &s.slice, locals) {
                        Ty::Unknown => Ty::Unknown,
                        t => Ty::List(Box::new(t), None),
                    };
                }
            }
            // A generic type (`Client<Body>`) is an instance of the type.
            if self.modules[mi].lang == Lang::Rust {
                return self.annotated(mi, &s.value, locals);
            }
        }
        match self.infer(mi, annotation, locals) {
            Ty::Class(k) => Ty::Inst(k),
            Ty::Ext(p) if self.modules[mi].lang.declares_fields() => Ty::ExtInst(p),
            _ => Ty::Unknown,
        }
    }

    /// Return type of a project function, when it returns a local built from
    /// a known type, else the class its declared return type names.
    pub fn return_ty(&self, f: usize) -> Ty {
        if let Some(t) = self.returns.borrow().get(&f) {
            return t.clone();
        }
        self.returns.borrow_mut().insert(f, Ty::Unknown);
        let info = &self.funcs[f];
        let mut locals: HashMap<String, Ty> = HashMap::new();
        if let Some(c) = info.class {
            if info.kind == FnKind::ClassMethod {
                if let Some(p) = info.def.params().first() {
                    locals.insert(p.clone(), Ty::Class(c));
                }
            } else if info.kind != FnKind::StaticMethod {
                if let Some(p) = info.def.params().first() {
                    locals.insert(p.clone(), Ty::Inst(c));
                }
            }
        }
        let mut result = Ty::Unknown;
        self.scan_returns(info.module, info.def.body, &mut locals, &mut result);
        if result == Ty::Unknown {
            if let Some(r) = info.def.returns {
                if let Ty::Inst(k) = self.annotated(info.module, r, &locals) {
                    result = Ty::Inst(k);
                }
            }
        }
        self.returns.borrow_mut().insert(f, result.clone());
        result
    }

    fn scan_returns(&self, mi: usize, body: &[Stmt], locals: &mut HashMap<String, Ty>, result: &mut Ty) {
        for s in body {
            match s {
                Stmt::Assign(a) => {
                    if let [Expr::Name(n)] = a.targets.as_slice() {
                        let t = self.infer(mi, &a.value, locals);
                        if t != Ty::Unknown {
                            locals.insert(n.id.to_string(), t);
                        }
                    }
                }
                Stmt::Return(r) => {
                    if let Some(v) = &r.value {
                        let t = self.infer(mi, v, locals);
                        if *result == Ty::Unknown {
                            *result = t;
                        }
                    }
                }
                Stmt::If(x) => {
                    self.scan_returns(mi, &x.body, locals, result);
                    for c in &x.elif_else_clauses {
                        self.scan_returns(mi, &c.body, locals, result);
                    }
                }
                Stmt::For(x) => self.scan_returns(mi, &x.body, locals, result),
                Stmt::While(x) => self.scan_returns(mi, &x.body, locals, result),
                Stmt::With(x) => self.scan_returns(mi, &x.body, locals, result),
                Stmt::Try(x) => self.scan_returns(mi, &x.body, locals, result),
                _ => {}
            }
        }
    }

    /// Instance attributes assigned in methods (`self.attn = CausalSelfAttention(config)`).
    fn infer_fields(&self, c: usize) -> Vec<Field> {
        let cls = &self.classes[c];
        let mi = cls.module;
        let src = &self.modules[mi].src;
        let mut fields: Vec<Field> = vec![];
        let mut methods: Vec<usize> = cls.methods.iter().map(|(_, f)| *f).collect();
        methods.sort_by_key(|&f| self.funcs[f].name != "__init__");
        // Fields declared in the class body (`users = new Map()`, `users: Users`).
        let lang = self.modules[mi].lang;
        if lang.declares_fields() {
            self.collect_self_fields(mi, src, &cls.def.body, lang.receiver(), &mut HashMap::new(), &mut fields);
        }
        for f in methods.iter().copied() {
            let def = self.funcs[f].def;
            let params = def.params();
            let Some(this) = params.first().cloned() else { continue };
            let mut locals: HashMap<String, Ty> = HashMap::new();
            self.collect_self_fields(mi, src, def.body, &this, &mut locals, &mut fields);
        }
        // A constructor's struct literal (`Self { blocks, .. }`) says what the
        // declared fields hold: `blocks` built from `(0..n_layer).map(…)` is
        // n_layer blocks.
        if lang == Lang::Rust {
            for f in methods {
                let mut locals: HashMap<String, Ty> = HashMap::new();
                self.struct_literal_fields(c, mi, self.funcs[f].def.body, &mut locals, &mut fields);
            }
        }
        fields
    }

    fn struct_literal_fields(&self, c: usize, mi: usize, body: &[Stmt], locals: &mut HashMap<String, Ty>, fields: &mut [Field]) {
        let name = self.classes[c].name.as_str();
        for s in body {
            if let Stmt::Assign(a) = s {
                if let [Expr::Name(n)] = &a.targets[..] {
                    let t = self.infer(mi, &a.value, locals);
                    if t != Ty::Unknown {
                        locals.insert(n.id.to_string(), t);
                    }
                }
            }
            walk::stmt_exprs(std::slice::from_ref(s), false, &mut |root| {
                walk::walk(root, &mut |e| {
                    let Expr::Call(call) = e else { return };
                    if !matches!(&*call.func, Expr::Name(n) if n.id == name) {
                        return;
                    }
                    for k in &call.arguments.keywords {
                        let Some(key) = &k.arg else { continue };
                        let Some(field) = fields.iter_mut().find(|f| &f.name == key) else { continue };
                        let t = self.infer(mi, &k.value, locals);
                        let better = match (&field.ty, &t) {
                            (Ty::List(_, None), Ty::List(e, Some(_))) => !matches!(**e, Ty::Unknown),
                            (Ty::Unknown, t) => *t != Ty::Unknown,
                            _ => false,
                        };
                        if better {
                            field.ty = t;
                        }
                    }
                });
            });
        }
    }

    fn collect_self_fields(&self, mi: usize, src: &Source, body: &[Stmt], this: &str, locals: &mut HashMap<String, Ty>, fields: &mut Vec<Field>) {
        for s in body {
            match s {
                Stmt::Assign(a) => {
                    for t in &a.targets {
                        match t {
                            Expr::Attribute(attr) if matches!(&*attr.value, Expr::Name(n) if n.id.as_str() == this) => {
                                let name = attr.attr.to_string();
                                if fields.iter().any(|f| f.name == name) {
                                    continue;
                                }
                                let ty = self.infer(mi, &a.value, locals);
                                let ctor = match &*a.value {
                                    Expr::Call(call) => squash(src.of(&*call.func)),
                                    _ => String::new(),
                                };
                                fields.push(Field { name, ty, ctor, range: a.range });
                            }
                            Expr::Name(n) => {
                                let ty = self.infer(mi, &a.value, locals);
                                locals.insert(n.id.to_string(), ty);
                            }
                            _ => {}
                        }
                    }
                }
                Stmt::AnnAssign(a) => {
                    if let Expr::Attribute(attr) = &*a.target {
                        if matches!(&*attr.value, Expr::Name(n) if n.id.as_str() == this) && !fields.iter().any(|f| f.name == attr.attr.as_str()) {
                            // `this.users: UsersService = users`: the annotation types the field.
                            let value = a.value.as_deref().map(|v| self.infer(mi, v, locals)).unwrap_or(Ty::Unknown);
                            let ty = match value {
                                Ty::Unknown => self.annotated(mi, &a.annotation, locals),
                                t => t,
                            };
                            fields.push(Field { name: attr.attr.to_string(), ty, ctor: String::new(), range: a.range });
                        }
                    }
                }
                Stmt::Expr(e) => {
                    // self.register_buffer("bias", ...)
                    if let Expr::Call(call) = &*e.value {
                        if let Expr::Attribute(f) = &*call.func {
                            if f.attr.as_str() == "register_buffer" {
                                if let Some(Expr::StringLiteral(k)) = call.arguments.args.first() {
                                    let name = k.value.as_str();
                                    if !fields.iter().any(|x| x.name == name) {
                                        fields.push(Field { name: name.to_string(), ty: Ty::Ext("buffer".into()), ctor: "buffer".into(), range: call.range() });
                                    }
                                }
                            }
                        }
                    }
                }
                Stmt::If(x) => {
                    self.collect_self_fields(mi, src, &x.body, this, locals, fields);
                    for c in &x.elif_else_clauses {
                        self.collect_self_fields(mi, src, &c.body, this, locals, fields);
                    }
                }
                Stmt::For(x) => self.collect_self_fields(mi, src, &x.body, this, locals, fields),
                Stmt::With(x) => self.collect_self_fields(mi, src, &x.body, this, locals, fields),
                Stmt::Try(x) => self.collect_self_fields(mi, src, &x.body, this, locals, fields),
                _ => {}
            }
        }
    }

    pub fn register_nested_func(&mut self, mi: usize, def: FnDef<'p>, class: Option<usize>, node: Id) -> usize {
        let id = self.funcs.len();
        self.funcs.push(FuncInfo { name: def.name.to_string(), module: mi, class, def, node, kind: FnKind::Function });
        id
    }

    pub fn display(&self, t: &Ty) -> String {
        match t {
            Ty::Inst(c) | Ty::Class(c) => self.classes[*c].name.clone(),
            Ty::Ext(p) | Ty::ExtInst(p) => p.rsplit('.').next().unwrap_or(p).to_string(),
            Ty::Func(f) => self.funcs[*f].name.clone(),
            Ty::Module(m) => self.modules[*m].dotted.clone(),
            Ty::List(e, Some(n)) => format!("{} × {}", self.display(e), n),
            Ty::List(e, None) => format!("[{}]", self.display(e)),
            Ty::Dict(_) => "dict".into(),
            Ty::Unknown => String::new(),
        }
    }
}

/// `range(n)` → "n", `range(a, b)` → "b - a" (shown as text), lists → their length.
pub fn range_count(src: &Source, iter: &Expr) -> Option<String> {
    match iter {
        Expr::Call(c) if matches!(&*c.func, Expr::Name(n) if n.id.as_str() == "range") => match &c.arguments.args[..] {
            [n] => Some(count_text(src.of(n))),
            [a, b] => Some(format!("{} − {}", count_text(src.of(b)), count_text(src.of(a)))),
            _ => None,
        },
        Expr::List(l) => Some(l.elts.len().to_string()),
        Expr::Tuple(l) => Some(l.elts.len().to_string()),
        // A Rust range: `0..n`, `a..b`.
        Expr::Slice(s) => match (&s.lower, &s.upper) {
            (None, Some(n)) => Some(count_text(src.of(&**n))),
            (Some(a), Some(n)) if matches!(&**a, Expr::NumberLiteral(_)) && src.of(&**a) == "0" => Some(count_text(src.of(&**n))),
            (Some(a), Some(b)) => Some(format!("{} − {}", count_text(src.of(&**b)), count_text(src.of(&**a)))),
            _ => None,
        },
        _ => None,
    }
}
