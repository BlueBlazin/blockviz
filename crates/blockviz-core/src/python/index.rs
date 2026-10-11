//! Project-wide symbol index and a deliberately small type inference.
//!
//! The goal is not soundness; it is to name things the way a reader would:
//! `self.transformer.wte(idx)` is an `Embedding`, `block` in
//! `for block in self.transformer.h` is a `Block` repeated `n_layer` times, and
//! `model.configure_optimizers(...)` is `GPT.configure_optimizers`.

use super::source::Source;
use super::walk;
use crate::ir::Id;
use crate::text::{clip, squash};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::{Ranged, TextRange};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};

pub struct PyModule {
    pub file: u32,
    pub path: String,
    pub dotted: String,
    pub package: String,
    pub src: Source,
    pub ast: Vec<Stmt>,
    pub notebook: bool,
    /// First syntax error (the parser recovered and the tree is still usable).
    pub error: Option<String>,
}

#[derive(Clone, Copy)]
pub struct FnDef<'p> {
    pub name: &'p str,
    pub args: &'p ast::Parameters,
    pub body: &'p [Stmt],
    pub decorators: &'p [ast::Decorator],
    pub range: TextRange,
    pub is_async: bool,
}

impl<'p> FnDef<'p> {
    pub fn from_stmt(s: &'p Stmt) -> Option<Self> {
        match s {
            Stmt::FunctionDef(f) => Some(FnDef { name: &f.name, args: &f.parameters, body: &f.body, decorators: &f.decorator_list, range: f.range, is_async: f.is_async }),
            _ => None,
        }
    }
    pub fn params(&self) -> Vec<String> {
        walk::param_names(self.args)
    }
    pub fn decorated(&self, name: &str) -> bool {
        self.decorators.iter().any(|d| decorator_name(&d.expression) == name)
    }
    pub fn docstring(&self) -> Option<&'p str> {
        docstring(self.body)
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
            return Some(s.value.to_str());
        }
    }
    None
}

/// Base class expressions of a class definition.
pub fn class_bases(c: &ast::StmtClassDef) -> &[Expr] {
    c.arguments.as_ref().map(|a| &a.args[..]).unwrap_or(&[])
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
    pub def: &'p ast::StmtClassDef,
    pub bases: Vec<Ty>,
    pub base_text: Vec<String>,
    pub methods: Vec<(String, usize)>,
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
    pub modules: &'p [PyModule],
    pub by_dotted: HashMap<String, usize>,
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

/// Last segment of a count expression: `config.n_layer` → `n_layer`.
pub fn count_text(e: &str) -> String {
    let s = squash(e);
    let tail = if s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') { s.rsplit('.').next().unwrap_or(&s).to_string() } else { s };
    clip(&tail, 18)
}

impl<'p> Index<'p> {
    /// Builds symbols for all modules. `alloc` creates a detached IR node for each definition.
    pub fn build(modules: &'p [PyModule], mut alloc: impl FnMut(&str, bool) -> Id) -> Self {
        let mut by_dotted = HashMap::new();
        for (i, m) in modules.iter().enumerate() {
            by_dotted.insert(m.dotted.clone(), i);
        }
        let mut ix = Index {
            modules,
            by_dotted,
            syms: vec![HashMap::new(); modules.len()],
            classes: vec![],
            funcs: vec![],
            defs_at: HashMap::new(),
            var_types: RefCell::new(HashMap::new()),
            returns: RefCell::new(HashMap::new()),
            imports: vec![vec![]; modules.len()],
            externals: vec![BTreeSet::new(); modules.len()],
        };
        for (mi, m) in modules.iter().enumerate() {
            ix.collect_defs(mi, &m.ast, &mut alloc, true);
        }
        for (mi, m) in modules.iter().enumerate() {
            ix.collect_imports(mi, &m.ast, true);
        }
        for c in 0..ix.classes.len() {
            let def = ix.classes[c].def;
            let m = ix.classes[c].module;
            let bases: Vec<Ty> = class_bases(def).iter().map(|b| ix.infer(m, b, &NoLocals)).collect();
            ix.classes[c].bases = bases;
        }
        for c in 0..ix.classes.len() {
            let fields = ix.infer_fields(c);
            ix.classes[c].fields = fields;
        }
        ix
    }

    fn collect_defs(&mut self, mi: usize, body: &'p [Stmt], alloc: &mut impl FnMut(&str, bool) -> Id, top: bool) {
        for s in body {
            match s {
                Stmt::ClassDef(c) => {
                    let id = self.register_class(mi, c, alloc);
                    if top || !self.syms[mi].contains_key(c.name.as_str()) {
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
                    for ast::ExceptHandler::ExceptHandler(h) in &x.handlers {
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

    fn register_class(&mut self, mi: usize, c: &'p ast::StmtClassDef, alloc: &mut impl FnMut(&str, bool) -> Id) -> usize {
        let node = alloc(&c.name, true);
        let cid = self.classes.len();
        let src = &self.modules[mi].src;
        let base_text: Vec<String> = class_bases(c).iter().map(|b| squash(src.of(b))).collect();
        self.classes.push(ClassInfo { name: c.name.to_string(), module: mi, def: c, bases: vec![], base_text, methods: vec![], fields: vec![], data: false, node });
        self.defs_at.insert((mi, u32::from(c.range.start())), Sym::Class(cid));
        let mut methods = vec![];
        for s in &c.body {
            if let Some(f) = FnDef::from_stmt(s) {
                let node = alloc(f.name, false);
                let kind = if f.decorated("staticmethod") {
                    FnKind::StaticMethod
                } else if f.decorated("classmethod") {
                    FnKind::ClassMethod
                } else if f.decorated("property") || f.decorated("cached_property") {
                    FnKind::Property
                } else {
                    FnKind::Method
                };
                let id = self.funcs.len();
                self.funcs.push(FuncInfo { name: f.name.to_string(), module: mi, class: Some(cid), def: f, node, kind });
                self.defs_at.insert((mi, u32::from(f.range.start())), Sym::Func(id));
                methods.push((f.name.to_string(), id));
            }
        }
        let data_decorator = c.decorator_list.iter().any(|d| matches!(decorator_name(&d.expression), "dataclass" | "dataclasses" | "attrs" | "define" | "frozen"));
        let data_base = self.classes[cid].base_text.iter().any(|b| {
            let tail = b.rsplit('.').next().unwrap_or(b);
            matches!(tail, "NamedTuple" | "TypedDict" | "BaseModel" | "Enum" | "IntEnum" | "StrEnum" | "BaseSettings")
        });
        let behaviour = methods.iter().any(|(n, _)| !n.starts_with("__"));
        self.classes[cid].data = (data_decorator || data_base) && !behaviour;
        self.classes[cid].methods = methods;
        cid
    }

    fn collect_imports(&mut self, mi: usize, body: &'p [Stmt], top: bool) {
        for s in body {
            match s {
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
                                names.push(name.clone());
                                // `from pkg import submodule`
                                let sub = format!("{}.{}", self.modules[t].dotted, name);
                                match self.by_dotted.get(&sub) {
                                    Some(&s) => Sym::Module(s),
                                    None => Sym::Member(t, name.clone()),
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
                        self.note_import(mi, t, names);
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
                    for ast::ExceptHandler::ExceptHandler(h) in &x.handlers {
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

    pub fn sym(&self, mi: usize, name: &str) -> Option<&Sym> {
        self.syms[mi].get(name)
    }

    pub fn sym_ty(&self, mi: usize, name: &str, depth: u32) -> Ty {
        if depth > 6 {
            return Ty::Unknown;
        }
        match self.syms[mi].get(name) {
            Some(Sym::Class(c)) => Ty::Class(*c),
            Some(Sym::Func(f)) => Ty::Func(*f),
            Some(Sym::Module(m)) => Ty::Module(*m),
            Some(Sym::Ext(p)) => Ty::Ext(p.clone()),
            Some(Sym::Member(m, n)) => self.sym_ty(*m, n, depth + 1),
            Some(Sym::Var) => self.var_types.borrow().get(&(mi, name.to_string())).cloned().unwrap_or(Ty::Unknown),
            None if is_builtin(name) => Ty::Ext(format!("builtins.{name}")),
            None => Ty::Unknown,
        }
    }

    pub fn method(&self, c: usize, name: &str) -> Option<usize> {
        self.method_depth(c, name, 0)
    }

    fn method_depth(&self, c: usize, name: &str, depth: u32) -> Option<usize> {
        if depth > 8 {
            return None;
        }
        let cls = &self.classes[c];
        if let Some((_, f)) = cls.methods.iter().find(|(n, _)| n == name) {
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
                            None => self.sym_ty(m, attr, 0),
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
            Expr::ListComp(lc) => {
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
                        fields.push((k.value.to_str().to_string(), infer(&item.value)));
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

    /// Return type of a project function, when it returns a local built from a known type.
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
        for f in methods {
            let def = self.funcs[f].def;
            let params = def.params();
            let Some(this) = params.first().cloned() else { continue };
            let mut locals: HashMap<String, Ty> = HashMap::new();
            self.collect_self_fields(mi, src, def.body, &this, &mut locals, &mut fields);
        }
        fields
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
                    if let (Expr::Attribute(attr), Some(v)) = (&*a.target, &a.value) {
                        if matches!(&*attr.value, Expr::Name(n) if n.id.as_str() == this) && !fields.iter().any(|f| f.name == attr.attr.as_str()) {
                            let ty = self.infer(mi, v, locals);
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
                                    let name = k.value.to_str();
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
        _ => None,
    }
}
