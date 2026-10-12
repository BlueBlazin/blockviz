//! Statements → dataflow diagrams.
//!
//! Abstraction rules (the heart of the tool):
//! * A block is created only for operations a reader would name: calls into
//!   project code, calls on sub-modules (`self.attn(x)`), library operations,
//!   and I/O. Glue (reshapes, builtins, arithmetic on scalars) is fused into
//!   the block it feeds, or into one block per statement.
//! * `a + b` of two computed values becomes a ⊕ glyph (residual connections).
//! * `print`, `assert`, imports, docstrings and `super().__init__()` produce no
//!   blocks; prints become effects on the enclosing block.
//! * Blocks are named by `crate::naming`: the operation for calls, the
//!   developer's name for a value or field, never cut-off code.
//! * Control flow becomes regions: loops and branches drawn inline when small
//!   (with "× n" repeat badges), lanes for if/elif/else, and a guard tag on a
//!   lone guarded statement instead of a box. Every exit and every branch arm
//!   shows a step: `return None · if value is None`.
//! * Conditions reach the steps they guard as control edges; settings and
//!   properties of values (`len`, `shape`) as context edges.
//! * Long statement lists are split into sections at comment-headed
//!   paragraphs; runs of constants become a settings table.
//! * Values crossing region boundaries get ports automatically, so every
//!   diagram has explicit inputs (top) and outputs (bottom).

use super::effects::{self, Io};
use super::frameworks::{self, Handler};
use super::index::*;
use super::names::Namer;
use super::walk;
use crate::ir::*;
use crate::naming::{self, Dest, Step};
use crate::text::{clip, heading, is_directive, squash, summary};
use crate::vocabulary::{self, Domains};
use super::syntax::{self as ast, Ctx as Load, Expr, Lang, Ranged, Stmt, TemplateKind, TextRange};
use std::collections::{BTreeSet, HashMap, HashSet};

/// A cross-file relation discovered while building flows.
pub enum Relation {
    Exec { from: usize, to: usize },
    Artifact { module: usize, name: String, write: bool },
    /// A view renders a template file: `render_template("blog/index.html")`.
    Template { module: usize, name: String },
}

pub struct Ctx<'p> {
    pub ix: Index<'p>,
    pub b: Builder,
    pub file_nodes: Vec<Id>,
    pub relations: Vec<Relation>,
    pub entry: Vec<bool>,
    /// Repo-relative paths of every file, for resolving `exec(open('x.py'))`.
    pub module_by_path: HashMap<String, usize>,
    /// Modules some other project module imports.
    pub imported: Vec<bool>,
    /// Modules that create an application object (`app = FastAPI()`).
    pub app: Vec<bool>,
    /// Framework roles found among each module's functions.
    pub handlers: Vec<Vec<Handler>>,
    /// Router prefixes per module, computed on first use.
    pub prefixes: HashMap<usize, HashMap<String, String>>,
    /// Paths modules' routers are mounted at (`app.use("/api/users", users)`).
    pub mounts: HashMap<usize, String>,
    /// The domains each module works in, from the libraries it uses.
    pub domains: Vec<Domains>,
    /// Routes router builders register (axum, actix-web).
    pub routes: frameworks::Routes,
    /// Each class's component blocks, by field path (`transformer.wte`).
    pub components: HashMap<(usize, String), Id>,
    /// Calls of components, as (class, field path, call block).
    pub component_calls: Vec<(usize, String, Id)>,
}

impl Ctx<'_> {
    /// The architecture-figure category of the first name that has one, in
    /// modules that build neural networks; elsewhere names carry no colour.
    pub fn category(&self, mi: usize, names: &[&str]) -> Option<&'static str> {
        if !self.domains[mi].ml {
            return None;
        }
        names.iter().find_map(|n| vocabulary::ml_category(n))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Style {
    Data,
    Ctx,
    Def,
}

#[derive(Clone, Debug)]
enum Bind {
    Src(Vec<Id>, Style),
    Static,
    Const(String),
}

struct Region {
    node: Id,
    /// One shared port carries every configuration value into the region.
    ctx_port: Option<Id>,
    ctx_names: Vec<String>,
    env: HashMap<String, Bind>,
    assigned: Vec<String>,
    /// Lanes of a branch merge their outputs instead of replacing them.
    merging: bool,
    merged: HashSet<String>,
    returns: Vec<Val>,
    effects: BTreeSet<String>,
}

impl Region {
    fn new(node: Id) -> Self {
        Region { node, ctx_port: None, ctx_names: vec![], env: HashMap::new(), assigned: vec![], merging: false, merged: HashSet::new(), returns: vec![], effects: BTreeSet::new() }
    }
}

#[derive(Clone, Debug)]
struct Val {
    srcs: Vec<Id>,
    label: String,
    style: Style,
    scalar: bool,
}

#[derive(Default)]
struct Gather {
    effects: BTreeSet<String>,
    reads: Vec<String>,
    writes: Vec<String>,
    minor_calls: Vec<String>,
}

#[derive(Clone, Debug)]
enum Callee {
    Func(usize),
    /// A method a contract leaves to its implementations, called on a value
    /// of the contract's type: the types it may be, all of them or (`partial`)
    /// those known by name.
    Dispatch { contract: usize, name: String, types: Vec<usize>, partial: bool },
    Ctor(usize),
    Instance { class: usize, var: String },
    /// A component called (`self.attn(x)`): its field, and the class and
    /// field path (`transformer.wte`) it is declared under, when known.
    Component { name: String, ty: Ty, of: Option<(usize, String)> },
    Ext(String),
    Builtin(String),
    Value(String),
    Method(String),
    ExecFile(usize),
    Super,
    Unknown,
}

struct TypesView<'a>(&'a HashMap<String, Ty>);
impl Locals for TypesView<'_> {
    fn local(&self, name: &str) -> Option<Ty> {
        self.0.get(name).cloned()
    }
}

const SECTION_MIN: usize = 12;
/// Branches with more arms are drawn as a table of the arms.
const MAX_LANES: usize = 24;
const MINOR_EXT: &[&str] = &[
    "os.path.", "math.", "operator.", "typing.", "functools.", "itertools.", "collections.", "copy.", "inspect.", "string.", "re.", "builtins.", "dataclasses.", "time.", "sys.exit",
    // JavaScript: language globals and helpers are glue, like Python builtins.
    "globals.", "path.", "url.", "util.", "lodash", "clsx", "classnames", "tailwind-merge",
    // Rust: the standard library, and synchronisation and byte-buffer plumbing.
    "std.", "core.", "alloc.", "tokio.sync.", "parking_lot.", "crossbeam", "once_cell.", "bytes.", "anyhow.", "thiserror.",
    // Query builders are part of the query that runs them (`.fetch_one(&pool)`).
    "sqlx.", "diesel.", "sea_orm.",
    // Go: the standard library's text, sorting, synchronisation, context and
    // error plumbing; formatting builds values (`fmt.Sprintf`).
    "context.", "errors.", "strings.", "strconv.", "unicode.", "sort.", "slices.", "maps.", "cmp.", "sync.", "sync/atomic.", "path/filepath.", "math/", "reflect.", "regexp.", "fmt.Sprint", "fmt.Errorf", "flag.", "encoding/binary.", "container/", "iter.", "unicode/",
    // Method routers (`get(handler)`) are parts of the route they register.
    "axum.routing.", "actix_web.web.get", "actix_web.web.post", "actix_web.web.put", "actix_web.web.patch", "actix_web.web.delete", "actix_web.web.resource", "actix_web.web.scope",
];

pub struct Fb<'c, 'p> {
    cx: &'c mut Ctx<'p>,
    mi: usize,
    m: &'p Module,
    regions: Vec<Region>,
    types: HashMap<String, Ty>,
    files: HashMap<String, Vec<String>>,
    this: Option<(String, usize)>,
    outer: HashSet<String>,
    module_level: bool,
    /// Building a module's top level or one of its top-level functions.
    top: bool,
    in_init: bool,
    out_ports: HashMap<(Id, String), Id>,
    globals_declared: HashSet<String>,
    /// Loop variables that alias elements of a collection (`param_group` → `optimizer`).
    loop_alias: HashMap<String, String>,
    /// An imported module without a main guard: its top level defines things
    /// rather than doing things, so its constants form one table.
    library: bool,
    /// Module-level statements already accounted for by `declarations`.
    handled: HashSet<u32>,
    /// Building the body of a single guarded statement (`if c: x`).
    guarded: bool,
    /// Collecting the arguments of an iteration (`xs.map(…)`): blocks made
    /// inside a lambda run once per item.
    each: bool,
    /// Collecting an event handler (`onClick={() => …}`): blocks made inside
    /// the lambda run when the event happens ("on click").
    event: Option<String>,
    /// Collecting a promise callback (`.then(r => …)`): its parameter is the
    /// value the promise settles with.
    settled: Option<Vec<Val>>,
    /// Names a module-level definition uses that were not yet defined where
    /// it was written (`impl Db` below `struct DbDropGuard { db: Db }`):
    /// linked once the whole module is read.
    pending: Vec<(Id, String)>,
    /// The function being built.
    fid: Option<usize>,
}

fn compact(s: &str, max: usize) -> String {
    clip(&squash(s), max)
}

impl<'c, 'p> Fb<'c, 'p> {
    pub fn new(cx: &'c mut Ctx<'p>, mi: usize, root: Id) -> Self {
        let modules: &'p [Module] = cx.ix.modules;
        Fb {
            cx,
            mi,
            m: &modules[mi],
            regions: vec![Region::new(root)],
            types: HashMap::new(),
            files: HashMap::new(),
            this: None,
            outer: HashSet::new(),
            module_level: false,
            top: false,
            in_init: false,
            out_ports: HashMap::new(),
            globals_declared: HashSet::new(),
            loop_alias: HashMap::new(),
            library: false,
            handled: HashSet::new(),
            guarded: false,
            each: false,
            event: None,
            settled: None,
            pending: vec![],
            fid: None,
        }
    }

    // ----- module and definitions ------------------------------------------------

    pub fn build_module(cx: &'c mut Ctx<'p>, mi: usize) {
        let root = cx.file_nodes[mi];
        let modules: &'p [Module] = cx.ix.modules;
        let m = &modules[mi];
        let library = cx.imported[mi] && !m.ast.iter().any(|s| matches!(s, Stmt::If(x) if is_main_guard(&x.test)));
        let mut fb = Fb::new(cx, mi, root);
        fb.module_level = true;
        fb.top = true;
        fb.library = library;
        // Nothing runs after the module body, so nothing is "read later" at its end.
        let later = HashSet::new();
        let body: &'p [Stmt] = if docstring(&m.ast).is_some() { &m.ast[1..] } else { &m.ast };
        fb.declarations(body);
        fb.statements(body, &later, true);
        // Definitions use each other whatever order they are written in.
        for (node, name) in std::mem::take(&mut fb.pending) {
            if let Some(Bind::Src(srcs, Style::Def)) = fb.peek(&name).cloned() {
                for s in srcs.into_iter().filter(|&s| s != node) {
                    fb.link(&Val { srcs: vec![s], label: String::new(), style: Style::Def, scalar: false }, node);
                }
            }
        }
        let effects = fb.regions[0].effects.clone();
        fb.cx.b.get_mut(root).effects.extend(effects);
    }

    fn container(&self) -> Id {
        self.regions.last().unwrap().node
    }

    fn src(&self) -> &'p super::source::Source {
        &self.m.src
    }

    /// Describes expressions for naming; chains of calls stop at blocks.
    fn namer(&self) -> Namer<'_> {
        Namer { src: self.src(), is_block: Box::new(move |c| self.is_major(&self.classify(c), c)) }
    }

    fn add(&mut self, kind: Kind, shape: Shape, title: impl Into<String>) -> Id {
        let parent = self.container();
        self.cx.b.add(Some(parent), kind, shape, title)
    }

    fn set_span(&mut self, id: Id, n: &impl Ranged) {
        let span = self.src().span(n.range());
        self.cx.b.get_mut(id).span = Some(span);
    }

    fn bind(&mut self, name: &str, b: Bind) {
        let r = self.regions.last_mut().unwrap();
        r.env.insert(name.to_string(), b);
        r.assigned.push(name.to_string());
        if self.globals_declared.contains(name) && !self.module_level {
            r.effects.insert(format!("mutates global {name}"));
        }
    }

    fn peek(&self, name: &str) -> Option<&Bind> {
        self.regions.iter().rev().find_map(|r| r.env.get(name))
    }

    fn effect(&mut self, e: impl Into<String>) {
        self.regions.last_mut().unwrap().effects.insert(e.into());
    }

    /// Looks a name up, threading it through region ports as needed.
    fn lookup(&mut self, name: &str) -> Option<(Vec<Id>, Style)> {
        let top = self.regions.len() - 1;
        self.lookup_in(top, name)
    }

    fn lookup_in(&mut self, ri: usize, name: &str) -> Option<(Vec<Id>, Style)> {
        match self.regions[ri].env.get(name) {
            Some(Bind::Src(s, st)) => return Some((s.clone(), *st)),
            Some(_) => return None,
            None => {}
        }
        if ri == 0 {
            if !self.outer.contains(name) {
                return None;
            }
            let port = self.ctx_port(0, name);
            self.regions[0].env.insert(name.to_string(), Bind::Src(vec![port], Style::Ctx));
            return Some((vec![port], Style::Ctx));
        }
        let (srcs, style) = self.lookup_in(ri - 1, name)?;
        let region = self.regions[ri].node;
        if style == Style::Ctx {
            let port = self.ctx_port(ri, name);
            for s in srcs {
                let from_port = self.out_ports.get(&(s, name.to_string())).copied();
                self.connect(s, region, EdgeKind::Data, name, from_port, Some(port));
            }
            self.regions[ri].env.insert(name.to_string(), Bind::Src(vec![port], style));
            return Some((vec![port], style));
        }
        let port = self.cx.b.add(Some(region), Kind::Input, Shape::Port, name);
        self.cx.b.get_mut(port).category = match style {
            Style::Data => "port",
            Style::Ctx => "port",
            Style::Def => "uses",
        };
        for s in srcs {
            let from_port = self.out_ports.get(&(s, name.to_string())).copied();
            let label = if style == Style::Def { "" } else { name };
            self.connect(s, region, kind_of(style), label, from_port, Some(port));
        }
        self.regions[ri].env.insert(name.to_string(), Bind::Src(vec![port], style));
        Some((vec![port], style))
    }

    /// The region's shared configuration port, titled with what it carries.
    fn ctx_port(&mut self, ri: usize, name: &str) -> Id {
        let region = self.regions[ri].node;
        let port = match self.regions[ri].ctx_port {
            Some(p) => p,
            None => {
                let p = self.cx.b.add(Some(region), Kind::Input, Shape::Port, "");
                self.cx.b.get_mut(p).category = "port";
                self.regions[ri].ctx_port = Some(p);
                p
            }
        };
        let r = &mut self.regions[ri];
        if !r.ctx_names.iter().any(|n| n == name) {
            r.ctx_names.push(name.to_string());
        }
        let names = r.ctx_names.clone();
        let more = if names.len() > 3 { format!(" +{}", names.len() - 3) } else { String::new() };
        self.cx.b.get_mut(port).title = format!("{}{more}", names.iter().take(3).cloned().collect::<Vec<_>>().join(", "));
        self.cx.b.get_mut(port).inputs = names;
        port
    }

    fn connect(&mut self, from: Id, to: Id, kind: EdgeKind, label: &str, from_port: Option<Id>, to_port: Option<Id>) {
        if from == to || self.cx.b.get(from).parent != self.cx.b.get(to).parent {
            return;
        }
        let parent = self.cx.b.get(from).parent.unwrap();
        let edges = &mut self.cx.b.get_mut(parent).edges;
        if let Some(e) = edges.iter_mut().find(|e| e.from == from && e.to == to && e.from_port == from_port && e.to_port == to_port) {
            if kind < e.kind {
                e.kind = kind;
            }
            if !label.is_empty() && !e.label.split(", ").any(|l| l == label) {
                if !e.label.is_empty() {
                    e.label.push_str(", ");
                }
                e.label.push_str(label);
            }
            return;
        }
        edges.push(Edge { from, to, kind, label: label.to_string(), points: vec![], label_at: None, open: false, to_port, from_port });
    }

    fn link(&mut self, v: &Val, to: Id) {
        self.link_as(v, to, kind_of(v.style));
    }

    /// A value that decides whether or which way `to` runs.
    fn link_control(&mut self, v: &Val, to: Id) {
        self.link_as(v, to, if v.style == Style::Def { EdgeKind::Uses } else { EdgeKind::Control });
    }

    fn link_as(&mut self, v: &Val, to: Id, kind: EdgeKind) {
        let label = if v.style == Style::Def { String::new() } else { v.label.clone() };
        for &s in &v.srcs {
            let from_port = self.out_ports.get(&(s, label.clone())).copied();
            self.connect(s, to, kind, &label, from_port, None);
        }
    }

    fn read(&mut self, name: &str, g: &mut Gather) -> Vec<Val> {
        if let Some(Bind::Const(c)) = self.peek(name) {
            if let Some(f) = effects::file_name(c) {
                g.reads.push(f);
            }
            return vec![];
        }
        match self.lookup(name) {
            Some((srcs, style)) => vec![Val { srcs, label: name.to_string(), style, scalar: style == Style::Ctx }],
            None => vec![],
        }
    }

    // ----- statement lists -------------------------------------------------------

    fn units(&self, stmts: &[Stmt]) -> usize {
        let mut total = 0;
        let mut settings_end = 0;
        for (i, s) in stmts.iter().enumerate() {
            if self.handled.contains(&u32::from(s.start())) || i < settings_end {
                continue;
            }
            let u = match s {
                Stmt::Expr(e) => match &*e.value {
                    e if is_constant(e) => 0,
                    Expr::Call(c) if self.is_print(c) || is_super(c) => 0,
                    _ => 1,
                },
                Stmt::Pass(_) | Stmt::Break(_) | Stmt::Continue(_) | Stmt::Global(_) | Stmt::Nonlocal(_) | Stmt::Import(_) | Stmt::ImportFrom(_) | Stmt::Assert(_) => 0,
                Stmt::Assign(_) | Stmt::AnnAssign(_) if self.module_level && self.is_setting(s) => {
                    settings_end = self.setting_run(stmts, i);
                    1
                }
                Stmt::Assign(a) if !self.module_level && is_literal(&a.value) => 0,
                Stmt::Assign(a) if matches!(&*a.value, Expr::Name(_)) => 0,
                Stmt::If(x) => 1 + (self.units(&x.body) + x.elif_else_clauses.iter().map(|c| self.units(&c.body)).sum::<usize>()) / 2,
                Stmt::For(x) => 1 + self.units(&x.body) / 2,
                Stmt::While(x) => 1 + self.units(&x.body) / 2,
                Stmt::With(x) => self.units(&x.body),
                Stmt::Try(x) => self.units(&x.body),
                _ => 1,
            };
            total += u;
        }
        total
    }

    fn statements(&mut self, stmts: &'p [Stmt], later: &HashSet<String>, sections: bool) {
        let n = stmts.len();
        let loads: Vec<HashSet<String>> = stmts.iter().map(stmt_loads).collect();
        let mut suffix: Vec<HashSet<String>> = vec![HashSet::new(); n + 1];
        suffix[n] = later.clone();
        for i in (0..n).rev() {
            let mut s = suffix[i + 1].clone();
            s.extend(loads[i].iter().cloned());
            suffix[i] = s;
        }
        if sections && self.units(stmts) > SECTION_MIN {
            let paras = self.join_runs(stmts, self.paragraphs(stmts));
            if paras.len() > 1 {
                for (start, end, title, doc) in paras {
                    let (mut start, mut title, mut doc) = (start, title, doc);
                    while start < end && self.units(&stmts[start..start + 1]) == 0 && !self.is_setting(&stmts[start]) {
                        self.stmt(&stmts[start], &suffix[start + 1]);
                        start += 1;
                    }
                    if start == end {
                        continue;
                    }
                    // A paragraph opening with settings keeps the table at this level.
                    if self.module_level {
                        let j = self.setting_run(&stmts[..end], start);
                        if j > start && (j == end || j - start >= 4) {
                            let run: Vec<&'p Stmt> = stmts[start..j].iter().collect();
                            self.settings(&run, title.take(), std::mem::take(&mut doc));
                            start = j;
                            if start == end {
                                continue;
                            }
                        }
                    }
                    let slice = &stmts[start..end];
                    // A paragraph that is one run of statements is one block, not a section.
                    if self.subject_run(stmts, start) >= end && end - start > 1 {
                        self.run(slice, &suffix[start..=end], title, doc);
                        continue;
                    }
                    let units = self.units(slice);
                    let defs = slice.iter().filter(|s| matches!(s, Stmt::FunctionDef(_) | Stmt::ClassDef(_))).count();
                    match title {
                        Some(t) if units >= 3 => self.section(t, doc, slice, &suffix[end]),
                        // Untitled runs of code get a generated title; runs of definitions don't need one.
                        None if units >= 5 && defs * 2 < slice.len() => {
                            let t = self.generated_title(slice);
                            self.section(t, doc, slice, &suffix[end])
                        }
                        t => self.run(slice, &suffix[start..=end], t, doc),
                    }
                }
                return;
            }
        }
        self.run(stmts, &suffix, None, String::new());
    }

    /// Paragraphs separated by blank lines; a paragraph opened by a comment takes it as its title.
    fn paragraphs(&self, stmts: &[Stmt]) -> Vec<(usize, usize, Option<String>, String)> {
        let src = self.src();
        let mut paras: Vec<(usize, usize, Option<String>, String)> = vec![];
        for (i, s) in stmts.iter().enumerate() {
            let start_line = stmt_start_line(src, s);
            let new_para = i == 0 || {
                let prev_end = src.line_of(usize::from(stmts[i - 1].end()).saturating_sub(1));
                // `# ------` rulers separate parts of a script as clearly as blank lines.
                let ruler = src.comments_between(prev_end, start_line).iter().any(|c| {
                    let t = c.text.trim();
                    t.len() >= 8 && t.chars().all(|ch| !ch.is_alphanumeric())
                });
                ruler || src.paragraph_break(prev_end, start_line)
            };
            if new_para {
                // The comment block above a module's first statement is the file's
                // header. A notebook's markdown cells are its structure, though.
                let header = self.module_level && !self.m.notebook && self.m.ast.first().is_some_and(|f| f.start() == s.start());
                let comments = if header { vec![] } else { src.comments_above(start_line) };
                let title = comment_title(&comments);
                let doc = comments.iter().map(|c| c.text.trim()).filter(|t| !t.chars().all(|c| !c.is_alphanumeric()) && !is_directive(t)).collect::<Vec<_>>().join(" ");
                // An untitled paragraph continues the titled one before it, unless it
                // starts a definition: definitions stand on their own.
                let definition = matches!(s, Stmt::FunctionDef(_) | Stmt::ClassDef(_));
                if title.is_none() && !definition && paras.last().is_some_and(|p| p.2.is_some()) {
                    paras.last_mut().unwrap().1 = i + 1;
                    continue;
                }
                paras.push((i, i + 1, title, doc));
            } else {
                paras.last_mut().unwrap().1 = i + 1;
            }
        }
        paras
    }

    /// Paragraphs that split a run of statements on one subject are joined:
    /// `app.use(helmet())` and `app.use(cors())` under their own comments are
    /// still one middleware list.
    fn join_runs(&self, stmts: &[Stmt], paras: Vec<(usize, usize, Option<String>, String)>) -> Vec<(usize, usize, Option<String>, String)> {
        let mut out: Vec<(usize, usize, Option<String>, String)> = vec![];
        for p in paras {
            if let Some(last) = out.last_mut() {
                let same = match (self.subject(&stmts[last.1 - 1]), self.subject(&stmts[p.0])) {
                    (Some(a), Some(b)) => a.recv == b.recv && a.kind == b.kind && a.project == b.project,
                    _ => false,
                };
                if same {
                    last.1 = p.1;
                    last.2 = None;
                    last.3 = String::new();
                    continue;
                }
            }
            out.push(p);
        }
        out
    }

    /// A name for an untitled paragraph: "Setup" for library plumbing, else what it defines.
    fn generated_title(&self, stmts: &[Stmt]) -> String {
        let mut project = false;
        walk::stmt_exprs(stmts, false, &mut |e| {
            walk::walk(e, &mut |x| {
                if let Expr::Call(c) = x {
                    if matches!(self.classify(c), Callee::Func(_) | Callee::Dispatch { .. } | Callee::Ctor(_) | Callee::Instance { .. } | Callee::Component { .. } | Callee::ExecFile(_)) {
                        project = true;
                    }
                }
            })
        });
        let mut names: Vec<String> = vec![];
        for s in stmts {
            if let Stmt::Assign(a) = s {
                for t in &a.targets {
                    walk::target_names(t, &mut names);
                }
            }
        }
        names.dedup();
        if !project || names.is_empty() {
            return "Setup".into();
        }
        let more = if names.len() > 3 { ", …" } else { "" };
        format!("{}{more}", names.iter().take(3).cloned().collect::<Vec<_>>().join(", "))
    }

    fn section(&mut self, title: String, doc: String, stmts: &'p [Stmt], later: &HashSet<String>) {
        let id = self.add(Kind::Section, Shape::Box, title);
        {
            let n = self.cx.b.get_mut(id);
            n.category = "section";
            n.doc = doc;
        }
        let range = TextRange::new(stmts[0].start(), stmts[stmts.len() - 1].end());
        let span = self.src().span(range);
        self.cx.b.get_mut(id).span = Some(span);
        self.regions.push(Region::new(id));
        self.statements(stmts, later, true);
        self.close_region(later);
        let outs: Vec<String> = self.cx.b.get(id).children.iter().filter(|&&c| self.cx.b.get(c).kind == Kind::Output).map(|&c| self.cx.b.get(c).title.clone()).collect();
        self.cx.b.get_mut(id).outputs = outs;
    }

    fn run(&mut self, stmts: &'p [Stmt], suffix: &[HashSet<String>], title: Option<String>, doc: String) {
        let mut i = 0;
        let first_child = self.cx.b.get(self.container()).children.len();
        while i < stmts.len() {
            if self.handled.contains(&u32::from(stmts[i].start())) {
                i += 1;
                continue;
            }
            // Settings are declared at the top of a script, not chosen by a branch.
            if self.module_level && self.is_setting(&stmts[i]) && !self.in_branch() {
                let j = self.setting_run(stmts, i);
                let t = if i == 0 { title.clone() } else { None };
                let d = if i == 0 { doc.clone() } else { String::new() };
                let run: Vec<&'p Stmt> = stmts[i..j].iter().collect();
                self.settings(&run, t, d);
                i = j;
                continue;
            }
            // `if c: …; return x` followed by more code is an if/else: what
            // follows only runs otherwise, so it becomes the else branch.
            // A guarded bare exit (`if c: return x`) stays a tagged step instead.
            if let Stmt::If(x) = &stmts[i] {
                let exits = matches!(x.body.last(), Some(Stmt::Return(_) | Stmt::Continue(_) | Stmt::Break(_)));
                let bare = matches!(&x.body[..], [only] if self.bare_exit(only));
                if x.elif_else_clauses.is_empty() && i + 1 < stmts.len() && exits && !bare && self.units(&x.body) >= 1 {
                    let tail = &stmts[i + 1..];
                    let lanes = vec![
                        (format!("if {}", self.namer().code(&x.test, 40)), &x.body[..], Some(&*x.test), x.range),
                        ("else".to_string(), tail, None, TextRange::new(tail[0].start(), tail[tail.len() - 1].end())),
                    ];
                    let whole = RangeOf(TextRange::new(x.range.start(), tail[tail.len() - 1].end()));
                    self.branch(&whole, lanes, &suffix[stmts.len()]);
                    break;
                }
            }
            // `session.add(x); session.commit()`, `parser.add_argument(…)` × 12 and
            // `opts["tls"] = …; opts["ssl"] = …` each say one thing to a reader:
            // fuse runs of statements on one subject.
            let run = self.subject_run(stmts, i);
            if run > i + 1 {
                self.fused_run(&stmts[i..run], &suffix[run]);
                i = run;
                continue;
            }
            let run = self.assign_run(stmts, i);
            if run > i + 2 {
                self.fused_run(&stmts[i..run], &suffix[run]);
                i = run;
                continue;
            }
            self.stmt(&stmts[i], &suffix[i + 1]);
            i += 1;
        }
        // A titled paragraph that became a single block lends it its comment.
        if !doc.is_empty() {
            let kids: Vec<Id> = self.cx.b.get(self.container()).children[first_child..].iter().copied().filter(|&c| !self.cx.b.get(c).is_port()).collect();
            let def = kids.iter().copied().find(|&c| matches!(self.cx.b.get(c).kind, Kind::Function | Kind::Class));
            if let Some(first) = def.or(kids.first().copied()) {
                let n = self.cx.b.get_mut(first);
                if n.doc.is_empty() {
                    n.doc = doc;
                }
            }
        }
    }

    // ----- fused runs ---------------------------------------------------------------

    /// What a statement acts on, for fusing runs of statements on one
    /// subject: a library method call (`session.commit()`), a store into a
    /// named field (`smtp_options["tls"] = True`) or an update of a variable
    /// (`text += "…"`). Project methods only fuse as a table of identical calls.
    fn subject(&self, s: &Stmt) -> Option<Subject> {
        match s {
            Stmt::Expr(e) => {
                let Expr::Call(c) = &*e.value else { return None };
                let Expr::Attribute(a) = &*c.func else { return None };
                if self.is_print(c) || is_super(c) || contains_call(&a.value) || !matches!(&*a.value, Expr::Name(_) | Expr::Attribute(_)) || self.is_self(&a.value) {
                    return None;
                }
                // A call that takes a function (a route, a listener) is a step of its own.
                if c.arguments.args.iter().chain(c.arguments.keywords.iter().map(|k| &k.value)).any(|a| matches!(a, Expr::Function(_))) {
                    return None;
                }
                let project = match self.classify(c) {
                    Callee::Method(_) | Callee::Ext(_) => false,
                    Callee::Func(_) | Callee::Dispatch { .. } => true,
                    _ => return None,
                };
                Some(Subject { recv: squash(self.src().of(&*a.value)), verb: a.attr.to_string(), project, kind: RunKind::Call })
            }
            Stmt::Assign(a) if a.targets.len() == 1 => {
                let (owner, key) = match &a.targets[0] {
                    Expr::Attribute(t) => (&*t.value, t.attr.to_string()),
                    Expr::Subscript(t) => (&*t.value, string_literal(&t.slice)?.to_string()),
                    _ => return None,
                };
                // A value with a block of its own keeps it; `__init__` stores are not steps.
                if !matches!(owner, Expr::Name(_) | Expr::Attribute(_)) || contains_call(owner) || (self.in_init && self.is_self(owner)) || self.has_block(&a.value) || matches!(&*a.value, Expr::Function(_)) {
                    return None;
                }
                Some(Subject { recv: squash(self.src().of(owner)), verb: key, project: false, kind: RunKind::Store })
            }
            Stmt::AugAssign(a) => match &*a.target {
                Expr::Name(n) if !self.has_block(&a.value) => Some(Subject { recv: n.id.to_string(), verb: format!("{}=", a.op.as_str()), project: false, kind: RunKind::Update }),
                _ => None,
            },
            _ => None,
        }
    }

    /// Whether an expression contains a call that is a block of its own.
    fn has_block(&self, e: &Expr) -> bool {
        let mut found = false;
        walk::walk(e, &mut |x| {
            if let Expr::Call(c) = x {
                found |= self.is_major(&self.classify(c), c);
            }
        });
        found
    }

    /// End of a run of at least two statements on one subject starting at `i`.
    fn subject_run(&self, stmts: &[Stmt], i: usize) -> usize {
        let Some(first) = self.subject(&stmts[i]) else { return i };
        let mut j = i + 1;
        let mut project = first.project;
        while j < stmts.len() && !self.handled.contains(&u32::from(stmts[j].start())) {
            match self.subject(&stmts[j]) {
                Some(x) if x.recv == first.recv && x.kind == first.kind => {
                    project |= x.project;
                    if project && x.verb != first.verb {
                        break;
                    }
                    j += 1;
                }
                _ => break,
            }
        }
        // Project calls keep their own blocks unless they repeat as a table.
        if project && j - i < 3 {
            return i;
        }
        j
    }

    /// End of a run of module-level assignments from one callee
    /// (`template_rendered = _signals.signal("template-rendered")` × 10).
    fn assign_run(&self, stmts: &[Stmt], i: usize) -> usize {
        if !self.module_level {
            return i;
        }
        let callee = |s: &Stmt| match s {
            Stmt::Assign(a) if a.targets.len() == 1 && matches!(a.targets[0], Expr::Name(_)) => match &*a.value {
                Expr::Call(c) if !contains_call(&c.func) => Some(squash(self.src().of(&*c.func))),
                _ => None,
            },
            _ => None,
        };
        let Some(f) = callee(&stmts[i]) else { return i };
        let mut j = i + 1;
        while j < stmts.len() && !self.handled.contains(&u32::from(stmts[j].start())) && callee(&stmts[j]).as_ref() == Some(&f) {
            j += 1;
        }
        j
    }

    /// Builds a run's statements as usual, then merges their blocks into one:
    /// a table when the same call repeats (each row one call), else one block
    /// naming the steps (`session` · add → commit → refresh).
    fn fused_run(&mut self, stmts: &'p [Stmt], later: &HashSet<String>) {
        let container = self.container();
        let before: HashSet<Id> = self.cx.b.get(container).children.iter().copied().collect();
        for (k, s) in stmts.iter().enumerate() {
            let _ = k;
            self.stmt(s, later);
        }
        let made: Vec<Id> = self.cx.b.get(container).children.iter().copied().filter(|c| !before.contains(c) && !self.cx.b.get(*c).is_port()).collect();
        // A residual `x += f(x)` stays a ⊕; a block with an interior of its own
        // (a function, a hosted callback) is not merged into a list.
        if made.len() < 2 || made.iter().any(|&m| self.cx.b.get(m).kind == Kind::Glyph || !self.cx.b.get(m).children.is_empty()) {
            return;
        }
        let src = self.src();
        let namer = Namer::plain(src);
        // Each statement in order, shown when the fused block is opened.
        let mut steps: Vec<Row> = vec![];
        for s in stmts {
            let key = match s {
                Stmt::Expr(e) => match &*e.value {
                    Expr::Call(c) => format!("{}({})", match &*c.func {
                        Expr::Attribute(a) => a.attr.to_string(),
                        f => namer.code(f, 24),
                    }, namer.args(c, 48)),
                    v => namer.code(v, 56),
                },
                Stmt::Assign(a) => format!("{} = {}", namer.code(&a.targets[0], 24), namer.code(&a.value, 40)),
                Stmt::AugAssign(a) => format!("{}= {}", a.op.as_str(), namer.code(&a.value, 48)),
                _ => continue,
            };
            steps.push(Row { key, value: String::new(), note: String::new(), heading: false });
        }
        // What each statement does, for titles and rows.
        let mut recv = String::new();
        let mut methods: Vec<String> = vec![];
        let mut rows: Vec<Row> = vec![];
        let mut names: Vec<String> = vec![];
        for s in stmts {
            let (call, target) = match s {
                Stmt::Expr(e) => (&*e.value, None),
                Stmt::Assign(a) => (&*a.value, Some(src.of(&a.targets[0]).to_string())),
                _ => continue,
            };
            let Expr::Call(c) = call else { continue };
            let namer = Namer::plain(src);
            let args: Vec<String> = c.arguments.args.iter().map(|a| namer.code(a, 40)).chain(c.arguments.keywords.iter().map(|k| match &k.arg {
                Some(a) => format!("{a}={}", namer.code(&k.value, 36)),
                None => format!("**{}", namer.code(&k.value, 36)),
            })).collect();
            match &*c.func {
                Expr::Attribute(a) if target.is_none() => {
                    recv = squash(src.of(&*a.value));
                    methods.push(a.attr.to_string());
                }
                f => methods.push(squash(src.of(f))),
            }
            match target {
                Some(t) => {
                    rows.push(Row { key: t.clone(), value: naming::list(&args, 48), note: String::new(), heading: false });
                    names.push(t);
                }
                None => {
                    let (key, rest) = match args.split_first() {
                        Some((k, r)) => (k.clone(), naming::list(r, 48)),
                        None => ("()".into(), String::new()),
                    };
                    rows.push(Row { key, value: rest, note: String::new(), heading: false });
                }
            }
        }
        let same = methods.windows(2).all(|w| w[0] == w[1]);
        // `router.get("/", …); router.post("/", …)`: a table of routes.
        let mount = self.cx.mounts.get(&self.mi).cloned().unwrap_or_default();
        let routes: Option<Vec<Row>> = stmts
            .iter()
            .map(|s| {
                let Stmt::Expr(e) = s else { return None };
                let Expr::Call(c) = &*e.value else { return None };
                let Expr::Attribute(a) = &*c.func else { return None };
                let path = c.arguments.args.first().and_then(string_literal)?;
                if c.arguments.args.len() < 2 {
                    return None;
                }
                let handler = namer.code(c.arguments.args.last().unwrap(), 36);
                let middle: Vec<String> = c.arguments.args[1..c.arguments.args.len() - 1].iter().map(|m| namer.code(m, 24)).collect();
                // Go: `r.GET(…)`, `r.Get(…)`, `mux.HandleFunc("GET /items/{id}", …)`;
                // paths are already under their group's.
                if self.m.lang == Lang::Go {
                    let attr = a.attr.as_str();
                    // Routes registered on a group a caller passed in serve under its path.
                    let prefix = self.fid.and_then(|f| self.cx.routes.prefixes.get(&f)).cloned().unwrap_or_default();
                    let (verb, p) = match path.split_once(' ') {
                        Some((v, p)) if v.chars().all(|c| c.is_ascii_uppercase()) && p.starts_with('/') => (format!("{v} "), p),
                        // `router.GET("", …)` serves the group's own path.
                        _ if !path.starts_with('/') && !path.is_empty() => return None,
                        _ if is_http_verb(&attr.to_ascii_lowercase()) => (format!("{} ", attr.to_ascii_uppercase()), path),
                        _ if matches!(attr, "Handle" | "HandleFunc" | "Any") => (String::new(), path),
                        _ => return None,
                    };
                    let full = frameworks::join_path(&prefix, p);
                    let full = if full.is_empty() { "/".to_string() } else { full };
                    return Some(Row { key: format!("{verb}{full}"), value: handler, note: naming::list(&middle, 40), heading: false });
                }
                if !is_http_verb(&a.attr) || !(path.starts_with('/') || path == "*") {
                    return None;
                }
                let full = if path == "/" && !mount.is_empty() { mount.clone() } else { frameworks::join_path(&mount, path) };
                Some(Row { key: format!("{} {full}", a.attr.to_uppercase()), value: handler, note: naming::list(&middle, 40), heading: false })
            })
            .collect();
        let fused = made[0];
        let others: Vec<Id> = made[1..].to_vec();
        // Rewire edges of the merged blocks onto the first one.
        let merged: HashSet<Id> = made.iter().copied().collect();
        let edges = std::mem::take(&mut self.cx.b.get_mut(container).edges);
        let mut kept: Vec<Edge> = vec![];
        for mut e in edges {
            let from_in = merged.contains(&e.from);
            let to_in = merged.contains(&e.to);
            if from_in && to_in {
                continue;
            }
            if from_in {
                e.from = fused;
            }
            if to_in {
                e.to = fused;
            }
            match kept.iter_mut().find(|k| k.from == e.from && k.to == e.to && k.from_port == e.from_port && k.to_port == e.to_port) {
                Some(k) => {
                    for l in e.label.split(", ").filter(|l| !l.is_empty()) {
                        if !k.label.split(", ").any(|x| x == l) {
                            if !k.label.is_empty() {
                                k.label.push_str(", ");
                            }
                            k.label.push_str(l);
                        }
                    }
                    if e.kind < k.kind {
                        k.kind = e.kind;
                    }
                }
                None => kept.push(e),
            }
        }
        self.cx.b.get_mut(container).edges = kept;
        let mut effects: BTreeSet<String> = BTreeSet::new();
        let mut outputs: Vec<String> = vec![];
        let mut io = false;
        for &m in &made {
            let n = self.cx.b.get(m);
            effects.extend(n.effects.iter().cloned());
            io |= n.category == "io";
            for o in &n.outputs {
                if !outputs.contains(o) {
                    outputs.push(o.clone());
                }
            }
        }
        for &o in &others {
            self.cx.b.remove(o);
        }
        // Names bound to merged blocks now come from the fused one.
        for r in self.regions.iter_mut() {
            for b in r.env.values_mut() {
                if let Bind::Src(srcs, _) = b {
                    for s in srcs.iter_mut() {
                        if merged.contains(s) {
                            *s = fused;
                        }
                    }
                    srcs.dedup();
                }
            }
        }
        let range = TextRange::new(stmts[0].start(), stmts[stmts.len() - 1].end());
        let span = self.src().span(range);
        let code = compact(self.src().of(&RangeOf(range)), 200);
        let table = same && (rows.len() >= 3 || !names.is_empty());
        let n = self.cx.b.get_mut(fused);
        n.effects = effects.into_iter().collect();
        n.span = Some(span);
        n.code = code;
        n.outputs = outputs;
        n.reference = None;
        n.link = None;
        n.badges.clear();
        if let Some(mut rows) = routes {
            // `/users` and `/users/` for one handler are one route.
            let mut seen: Vec<(String, String)> = vec![];
            rows.retain(|r| {
                let k = (r.key.trim_end_matches('/').to_string(), r.value.clone());
                let new = !seen.contains(&k);
                seen.push(k);
                new
            });
            if !self.cx.handlers[self.mi].contains(&Handler::Route) {
                self.cx.handlers[self.mi].push(Handler::Route);
            }
            let n = self.cx.b.get_mut(fused);
            n.kind = Kind::Table;
            n.title = recv.clone();
            n.subtitle = naming::count(rows.len(), "route");
            n.category = "endpoint";
            n.rows = rows;
            return;
        }
        let subject = self.subject(&stmts[0]);
        match subject.as_ref().map(|x| x.kind) {
            // Fields set one after another read as a table of what was set.
            Some(RunKind::Store) => {
                let rows: Vec<Row> = stmts
                    .iter()
                    .filter_map(|s| match s {
                        Stmt::Assign(a) => Some(Row { key: namer.dest(&a.targets[0]).field_key(), value: namer.code(&a.value, 40), note: String::new(), heading: false }),
                        _ => None,
                    })
                    .collect();
                let n = self.cx.b.get_mut(fused);
                n.kind = Kind::Table;
                n.title = naming::qualifier(&subject.unwrap().recv, naming::TITLE_MAX);
                n.subtitle = format!("set × {}", rows.len());
                n.rows = rows;
                return;
            }
            Some(RunKind::Update) => {
                let x = subject.unwrap();
                let n = self.cx.b.get_mut(fused);
                n.kind = Kind::Op;
                n.title = x.recv;
                n.subtitle = format!("{} × {}", x.verb, stmts.len());
                n.rows = steps;
                return;
            }
            _ => {}
        }
        let n = self.cx.b.get_mut(fused);
        if table {
            n.kind = Kind::Table;
            n.title = if names.is_empty() { format!("{recv}.{}", methods[0]) } else { methods[0].clone() };
            n.subtitle = format!("× {}", rows.len());
            n.rows = rows;
            if !matches!(n.category, "io") {
                n.category = "config";
            }
        } else if same {
            n.title = format!("{recv}.{}", methods[0]);
            n.subtitle = format!("× {}", methods.len());
            n.rows = steps;
        } else {
            // `session` · add → commit → refresh; opening it shows each call in order.
            n.title = recv;
            n.subtitle = naming::join(&methods, " → ", 40);
            n.rows = steps;
            if io {
                n.category = "io";
            }
        }
    }

    // ----- module declarations --------------------------------------------------------

    /// Module-level statements that declare rather than compute: `__all__` and
    /// other metadata, type aliases, loggers, optional and type-checking
    /// imports. None of them is a step a reader would draw. Type aliases are
    /// listed in a table when a module has several; a library module's
    /// constants are gathered into one table.
    fn declarations(&mut self, body: &'p [Stmt]) {
        let mut types: Vec<&'p Stmt> = vec![];
        let mut consts: Vec<&'p Stmt> = vec![];
        // Names whose values are computed by blocks: anything reading them is dataflow.
        let mut dynamic: HashSet<String> = HashSet::new();
        let mut const_names: HashSet<String> = HashSet::new();
        // Route tables: `urlpatterns = [path(...), ...]` and later `+=` additions.
        let mut tables: Vec<(String, Vec<Row>, &'p Stmt)> = vec![];
        for s in body {
            if self.route_rows(s, &mut tables, None) {
                self.handled.insert(u32::from(s.start()));
                continue;
            }
            let decl = self.declaration(s);
            match decl {
                Some(Decl::Exports(names)) => {
                    let file = self.cx.file_nodes[self.mi];
                    let n = self.cx.b.get_mut(file);
                    for name in names {
                        if !n.outputs.contains(&name) {
                            n.outputs.push(name);
                        }
                    }
                }
                Some(Decl::Type) => types.push(s),
                Some(Decl::Quiet) => {}
                None => {
                    if self.library && self.is_constant(s, &dynamic) {
                        consts.push(s);
                        let mut names = BTreeSet::new();
                        walk::collect_bindings(std::slice::from_ref(s), &mut names, &mut BTreeSet::new());
                        const_names.extend(names);
                    } else if self.library && adjusts(std::slice::from_ref(s), &const_names) {
                        // Folded into the constants it adjusts.
                    } else {
                        let mut names = BTreeSet::new();
                        walk::collect_bindings(std::slice::from_ref(s), &mut names, &mut BTreeSet::new());
                        if !matches!(s, Stmt::Import(_) | Stmt::ImportFrom(_) | Stmt::FunctionDef(_) | Stmt::ClassDef(_)) {
                            dynamic.extend(names);
                        }
                        continue;
                    }
                }
            }
            self.handled.insert(u32::from(s.start()));
        }
        for &s in &types {
            if types.len() < 3 {
                let mut names = BTreeSet::new();
                walk::collect_bindings(std::slice::from_ref(s), &mut names, &mut BTreeSet::new());
                for n in names {
                    self.bind(&n, Bind::Static);
                }
            }
        }
        if types.len() >= 3 {
            self.type_table(&types);
        }
        for s in body {
            if matches!(self.declaration(s), Some(Decl::Quiet) | Some(Decl::Exports(_))) {
                let mut names = BTreeSet::new();
                walk::collect_bindings(std::slice::from_ref(s), &mut names, &mut BTreeSet::new());
                for n in names {
                    if self.peek(&n).is_none() {
                        self.bind(&n, Bind::Static);
                    }
                }
            }
        }
        if !consts.is_empty() {
            self.settings(&consts, Some("Constants".into()), String::new());
        }
        for (name, rows, first) in tables {
            let id = self.add(Kind::Table, Shape::Box, name.clone());
            let span = self.src().span(first.range());
            let n = self.cx.b.get_mut(id);
            n.category = "endpoint";
            n.subtitle = naming::count(rows.len(), "route");
            n.rows = rows;
            n.span = Some(span);
            n.outputs = vec![name.clone()];
            self.bind(&name, Bind::Src(vec![id], Style::Ctx));
        }
    }

    /// Collects rows of a route list (`urlpatterns = [path("admin/", …), …]`,
    /// `urlpatterns += [...]`, and `if settings.DEBUG:` blocks adding to it).
    /// Returns whether the statement was one.
    fn route_rows(&self, s: &'p Stmt, tables: &mut Vec<(String, Vec<Row>, &'p Stmt)>, guard: Option<&str>) -> bool {
        let src = self.src();
        let row = |call: &Expr| -> Row {
            match call {
                Expr::Call(c) => {
                    let namer = Namer::plain(src);
                    let key = c.arguments.args.first().map(|a| string_literal(a).map(|t| if t.is_empty() { "/".to_string() } else { t.to_string() }).unwrap_or_else(|| namer.code(a, 40))).unwrap_or_default();
                    let value = c.arguments.args.get(1).map(|a| namer.code(a, 44)).unwrap_or_default();
                    Row { key, value, note: guard.map(|g| format!("if {g}")).unwrap_or_default(), heading: false }
                }
                e => Row { key: Namer::plain(src).code(e, 40), value: String::new(), note: guard.map(|g| format!("if {g}")).unwrap_or_default(), heading: false },
            }
        };
        let calls = |e: &'p Expr| -> Option<&'p [Expr]> {
            let elts: &[Expr] = match e {
                Expr::List(l) => &l.elts,
                Expr::Tuple(t) => &t.elts,
                _ => return None,
            };
            (elts.len() >= 2 && elts.iter().all(|x| matches!(x, Expr::Call(c) if !contains_call(&c.func))) && elts.iter().filter(|x| matches!(x, Expr::Call(c) if c.arguments.args.first().is_some_and(|a| string_literal(a).is_some()))).count() * 2 >= elts.len()).then_some(elts)
        };
        match s {
            Stmt::Assign(a) if guard.is_none() && a.targets.len() == 1 => {
                let (Expr::Name(n), Some(elts)) = (&a.targets[0], calls(&a.value)) else { return false };
                let rows = elts.iter().map(row).collect();
                tables.push((n.id.to_string(), rows, s));
                true
            }
            Stmt::AugAssign(a) => {
                let Expr::Name(n) = &*a.target else { return false };
                let Some(t) = tables.iter_mut().find(|t| t.0 == n.id.as_str()) else { return false };
                match calls(&a.value) {
                    Some(elts) => t.1.extend(elts.iter().map(row)),
                    None => match &*a.value {
                        Expr::List(l) => t.1.extend(l.elts.iter().map(row)),
                        v => t.1.push(row(v)),
                    },
                }
                true
            }
            Stmt::If(x) if guard.is_none() && !tables.is_empty() => {
                let only = |body: &[Stmt]| body.iter().all(|b| matches!(b, Stmt::Import(_) | Stmt::ImportFrom(_) | Stmt::Pass(_)) || matches!(b, Stmt::AugAssign(a) if matches!(&*a.target, Expr::Name(n) if tables.iter().any(|t| t.0 == n.id.as_str()))));
                if !only(&x.body) || x.elif_else_clauses.iter().any(|c| !only(&c.body)) {
                    return false;
                }
                let test = Namer::plain(src).code(&x.test, 30);
                for b in &x.body {
                    self.route_rows(b, tables, Some(&test));
                }
                true
            }
            _ => false,
        }
    }

    fn declaration(&self, s: &Stmt) -> Option<Decl> {
        fn assigned(t: &Expr) -> Option<&str> {
            match t {
                Expr::Name(n) => Some(n.id.as_str()),
                _ => None,
            }
        }
        if is_plumbing(s) {
            return Some(Decl::Quiet);
        }
        match s {
            // `export { a, b as c }`, `export default App`: what the module offers.
            Stmt::Export(x) => Some(Decl::Exports(x.names.iter().map(|(local, exported)| if exported == "default" { local.clone() } else { exported.clone() }).filter(|n| !n.is_empty()).collect())),
            Stmt::TypeAlias(_) => Some(Decl::Type),
            Stmt::Assign(a) if a.targets.len() == 1 => {
                let name = assigned(&a.targets[0])?;
                if name == "__all__" {
                    return Some(Decl::Exports(strings_in(&a.value)));
                }
                if is_dunder(name) {
                    return Some(Decl::Quiet);
                }
                if is_logger(&a.value) {
                    return Some(Decl::Quiet);
                }
                if self.is_type_expr(&a.value) || is_type_factory(&a.value) {
                    return Some(Decl::Type);
                }
                None
            }
            Stmt::AnnAssign(a) => {
                let name = assigned(&a.target)?;
                if is_dunder(name) {
                    return Some(if name == "__all__" { Decl::Exports(a.value.as_deref().map(strings_in).unwrap_or_default()) } else { Decl::Quiet });
                }
                let alias = matches!(&*a.annotation, Expr::Name(n) if n.id.as_str() == "TypeAlias") || matches!(&*a.annotation, Expr::Attribute(x) if x.attr.as_str() == "TypeAlias");
                if alias {
                    return Some(Decl::Type);
                }
                if a.value.as_deref().is_some_and(is_logger) {
                    return Some(Decl::Quiet);
                }
                None
            }
            Stmt::AugAssign(a) if assigned(&a.target) == Some("__all__") => Some(Decl::Exports(strings_in(&a.value))),
            Stmt::Expr(e) => match &*e.value {
                Expr::Call(c) => match &*c.func {
                    Expr::Attribute(x) if matches!(&*x.value, Expr::Name(n) if n.id.as_str() == "__all__") => Some(Decl::Exports(c.arguments.args.iter().flat_map(strings_in).collect())),
                    _ => None,
                },
                _ => None,
            },
            // `if TYPE_CHECKING:` imports exist for annotations only.
            Stmt::If(x) if matches!(&*x.test, Expr::Name(n) if n.id.as_str() == "TYPE_CHECKING") || matches!(&*x.test, Expr::Attribute(a) if a.attr.as_str() == "TYPE_CHECKING") => Some(Decl::Quiet),
            // Optional and version-dependent imports: `try: import x except ImportError: x = None`.
            Stmt::If(x) if !is_main_guard(&x.test) && !x.body.is_empty() && import_only(&x.body) && x.elif_else_clauses.iter().all(|c| import_only(&c.body)) => Some(Decl::Quiet),
            Stmt::Try(x) if !x.body.is_empty() && import_only(&x.body) && x.handlers.iter().all(|h| import_only(&h.body)) && import_only(&x.orelse) && import_only(&x.finalbody) => Some(Decl::Quiet),
            _ => None,
        }
    }

    /// `Optional[int]`, `dict[str, Any]`, `int | None`: a type, not a value.
    fn is_type_expr(&self, e: &Expr) -> bool {
        match e {
            Expr::Subscript(x) => self.is_type_name(&x.value),
            Expr::BinOp(b) if b.op == ast::Operator::BitOr => {
                let side = |e: &Expr| self.is_type_expr(e) || matches!(e, Expr::NoneLiteral(_)) || self.is_type_name(e) || matches!(e, Expr::Name(n) if matches!(self.cx.ix.sym_ty(self.mi, &n.id, 0), Ty::Class(_)));
                (self.is_type_expr(&b.left) || self.is_type_expr(&b.right) || self.is_type_name(&b.left) || self.is_type_name(&b.right)) && side(&b.left) && side(&b.right)
            }
            _ => false,
        }
    }

    fn is_type_name(&self, e: &Expr) -> bool {
        let name = match e {
            Expr::Name(n) => {
                if matches!(self.peek(&n.id), Some(Bind::Src(..))) {
                    return false;
                }
                n.id.as_str()
            }
            Expr::Attribute(a) => a.attr.as_str(),
            _ => return false,
        };
        TYPING_NAMES.contains(&name) || (matches!(e, Expr::Name(_)) && matches!(name, "list" | "dict" | "tuple" | "set" | "frozenset" | "type" | "int" | "str" | "float" | "bool" | "bytes"))
    }

    /// A library module's constant: a name bound to a value computed without
    /// I/O or other blocks. UPPER_CASE names may also hold constructed objects.
    fn is_constant(&self, s: &Stmt, dynamic: &HashSet<String>) -> bool {
        let (target, value) = match s {
            Stmt::Assign(a) if a.targets.len() == 1 => (&a.targets[0], &*a.value),
            Stmt::AnnAssign(a) => match &a.value {
                Some(v) => (&*a.target, &**v),
                None => return false,
            },
            _ => return false,
        };
        let Expr::Name(n) = target else { return false };
        let upper = n.id.chars().any(|c| c.is_ascii_uppercase()) && !n.id.chars().any(|c| c.is_ascii_lowercase());
        let mut ok = true;
        walk::walk(value, &mut |e| match e {
            Expr::Lambda(_) | Expr::Function(_) | Expr::Await(_) | Expr::Yield(_) | Expr::Named(_) => ok = false,
            Expr::Name(x) if dynamic.contains(x.id.as_str()) => ok = false,
            Expr::Call(c) => {
                let callee = self.classify(c);
                let io = matches!(&callee, Callee::Ext(p) if effects::io_of(p, c, self.src()).is_some() || effects::ext_effect(p).is_some())
                    || matches!(&callee, Callee::Builtin(b) if matches!(b.as_str(), "open" | "input" | "exec" | "eval" | "print"));
                let allowed = match &callee {
                    Callee::Ctor(_) | Callee::Ext(_) => upper || !self.is_major(&callee, c),
                    Callee::Func(_) | Callee::Dispatch { .. } | Callee::Instance { .. } | Callee::Component { .. } | Callee::Value(_) | Callee::ExecFile(_) => false,
                    _ => true,
                };
                if io || !allowed {
                    ok = false;
                }
            }
            _ => {}
        });
        ok
    }

    fn type_table(&mut self, stmts: &[&'p Stmt]) {
        let src = self.src();
        let mut rows = vec![];
        let mut names = vec![];
        for &s in stmts {
            let (name, value) = match s {
                Stmt::Assign(a) => (src.of(&a.targets[0]).to_string(), Namer::plain(src).code(&a.value, 40)),
                Stmt::AnnAssign(a) => (src.of(&*a.target).to_string(), a.value.as_deref().map(|v| Namer::plain(src).code(v, 40)).unwrap_or_default()),
                Stmt::TypeAlias(t) => (src.of(&*t.name).to_string(), Namer::plain(src).code(&t.value, 40)),
                _ => continue,
            };
            rows.push(Row { key: name.clone(), value, note: String::new(), heading: false });
            names.push(name);
        }
        let id = self.add(Kind::Table, Shape::Box, "Types");
        let span = self.src().span(stmts[0].range());
        let n = self.cx.b.get_mut(id);
        n.category = "config";
        n.rows = rows;
        n.span = Some(span);
        n.doc = "Type aliases and type variables used in annotations.".into();
        for name in names {
            self.bind(&name, Bind::Static);
        }
    }

    // ----- settings ----------------------------------------------------------------

    fn is_setting(&self, s: &Stmt) -> bool {
        self.is_setting_after(s, &HashSet::new())
    }

    /// A setting, given the names that settings just above it define.
    fn is_setting_after(&self, s: &Stmt, earlier: &HashSet<String>) -> bool {
        let (target, value) = match s {
            Stmt::Assign(a) if a.targets.len() == 1 => (&a.targets[0], &*a.value),
            Stmt::AnnAssign(a) => match &a.value {
                Some(v) => (&*a.target, &**v),
                None => return false,
            },
            _ => return false,
        };
        matches!(target, Expr::Name(n) if !is_dunder(&n.id)) && self.literalish(value, earlier)
    }

    /// Where a run of settings starting at `i` ends. Settings build on the
    /// ones above them: `BLOCK = 128; N = BLOCK * 2`, `total ||= 1n`.
    fn setting_run(&self, stmts: &[Stmt], i: usize) -> usize {
        let mut earlier = HashSet::new();
        let mut j = i;
        // Statements already gathered into the module's constants are not
        // listed again.
        while j < stmts.len() && !self.handled.contains(&u32::from(stmts[j].start())) && self.is_setting_after(&stmts[j], &earlier) {
            let mut names = vec![];
            match &stmts[j] {
                Stmt::Assign(a) => walk::target_names(&a.targets[0], &mut names),
                Stmt::AnnAssign(a) => walk::target_names(&a.target, &mut names),
                _ => {}
            }
            earlier.extend(names);
            j += 1;
        }
        j
    }

    /// Constants, other settings, module attributes (`torch.float32`) and
    /// arithmetic or conditionals over them. Calls only inside a condition.
    fn literalish(&self, e: &Expr, earlier: &HashSet<String>) -> bool {
        match e {
            e if is_constant(e) => true,
            Expr::Name(n) if earlier.contains(n.id.as_str()) => true,
            Expr::Name(n) => match self.peek(&n.id) {
                Some(Bind::Src(_, Style::Ctx)) | Some(Bind::Const(_)) | Some(Bind::Static) => true,
                Some(Bind::Src(..)) => false,
                None => is_builtin(&n.id) || matches!(self.cx.ix.sym(self.mi, &n.id), Some(Sym::Ext(_)) | Some(Sym::Module(_))),
            },
            Expr::Attribute(a) => self.literalish(&a.value, earlier),
            Expr::UnaryOp(u) => self.literalish(&u.operand, earlier),
            Expr::BinOp(b) => self.literalish(&b.left, earlier) && self.literalish(&b.right, earlier),
            Expr::BoolOp(b) => b.values.iter().all(|v| self.literalish(v, earlier)),
            Expr::Compare(c) => c.operands.iter().all(|v| self.literalish(v, earlier)),
            Expr::If(x) => self.test_is_static(&x.test) && self.literalish(&x.body, earlier) && self.literalish(&x.orelse, earlier),
            Expr::List(l) => l.elts.iter().all(|v| self.literalish(v, earlier)),
            Expr::Tuple(l) => l.elts.iter().all(|v| self.literalish(v, earlier)),
            Expr::Set(l) => l.elts.iter().all(|v| self.literalish(v, earlier)),
            Expr::Dict(d) => d.items.iter().all(|i| i.key.as_ref().is_none_or(|k| self.literalish(k, earlier)) && self.literalish(&i.value, earlier)),
            Expr::Subscript(x) => self.literalish(&x.value, earlier) && self.literalish(&x.slice, earlier),
            Expr::Template(t) if t.kind == TemplateKind::Format && t.tag.is_none() => {
                let mut kids = vec![];
                walk::children(e, &mut kids);
                kids.iter().all(|v| self.literalish(v, earlier))
            }
            _ => false,
        }
    }

    fn test_is_static(&self, test: &Expr) -> bool {
        let mut ok = true;
        walk::walk(test, &mut |e| match e {
            Expr::Name(n) => {
                if let Some(Bind::Src(_, st)) = self.peek(&n.id) {
                    if *st != Style::Ctx {
                        ok = false;
                    }
                }
            }
            Expr::Call(c) => {
                if !matches!(self.classify(c), Callee::Ext(_) | Callee::Builtin(_) | Callee::Method(_)) {
                    ok = false;
                }
            }
            _ => {}
        });
        ok
    }

    fn settings(&mut self, stmts: &[&'p Stmt], title: Option<String>, doc: String) {
        if stmts.len() == 1 && self.regions.len() > 1 {
            if let Stmt::Assign(a) = stmts[0] {
                let text = compact(self.src().of(&*a.value), 40);
                let mut names = vec![];
                walk::target_names(&a.targets[0], &mut names);
                for n in names {
                    self.bind(&n, Bind::Const(text.clone()));
                }
                return;
            }
        }
        let src = self.src();
        let mut rows = vec![];
        let mut names = vec![];
        let mut prev_end: Option<u32> = None;
        // Comments between rows are headings only within one stretch of code.
        let contiguous = stmts.windows(2).all(|w| src.line_of(usize::from(w[1].start())) <= src.line_of(usize::from(w[0].end())) + 3);
        for &s in stmts {
            let (target, value) = match s {
                Stmt::Assign(a) => (&a.targets[0], &*a.value),
                Stmt::AnnAssign(a) => match a.value.as_deref() {
                    Some(v) => (&*a.target, v),
                    None => continue,
                },
                _ => continue,
            };
            let line = src.line_of(usize::from(s.start()));
            if let (Some(pe), true) = (prev_end, contiguous) {
                for c in src.comments_between(pe, line) {
                    if let Some(h) = heading(&c.text) {
                        rows.push(Row { key: h, value: String::new(), note: String::new(), heading: true });
                    }
                }
            }
            prev_end = Some(src.line_of(usize::from(s.end()).saturating_sub(1)));
            let name = src.of(target).trim().to_string();
            // `total ||= 1n` updates the setting above: the row shows the update.
            let shown = if value.start() == target.start() { compact(src.slice(TextRange::new(target.end(), value.end())).trim(), 40) } else { Namer::plain(src).code(value, 40) };
            rows.push(Row { key: name.clone(), value: shown, note: src.trailing_comment(line).filter(|c| !is_directive(c)).unwrap_or("").to_string(), heading: false });
            names.push(name.clone());
            let ty = self.cx.ix.infer(self.mi, value, &TypesView(&self.types));
            if ty != Ty::Unknown {
                self.types.insert(name.clone(), ty.clone());
                self.cx.ix.var_types.borrow_mut().insert((self.mi, name.clone()), ty);
            }
            if let Expr::StringLiteral(c) = value {
                self.files.insert(name.clone(), effects::file_name(&c.value).into_iter().collect());
            }
        }
        let single = rows.iter().filter(|r| !r.heading).count() == 1;
        if single {
            let value = match stmts[0] {
                Stmt::Assign(a) => Some(&*a.value),
                Stmt::AnnAssign(a) => a.value.as_deref(),
                _ => None,
            };
            if value.is_some_and(|v| matches!(v, Expr::NoneLiteral(_)) || matches!(v, Expr::Dict(d) if d.items.is_empty()) || matches!(v, Expr::List(l) if l.elts.is_empty())) {
                for n in &names {
                    self.bind(n, Bind::Const("None".into()));
                }
                return;
            }
        }
        // A lone object of settings (`const conf = { port: 3000, host: … }`)
        // lists its keys, under its own name.
        let (mut title, mut doc) = (title, doc);
        if single {
            let value = match stmts[0] {
                Stmt::Assign(a) => Some(&*a.value),
                Stmt::AnnAssign(a) => a.value.as_deref(),
                _ => None,
            };
            if let Some(Expr::Dict(d)) = value {
                let keys: Option<Vec<&str>> = d.items.iter().map(|i| match &i.key {
                    Some(Expr::StringLiteral(k)) => Some(k.value.as_str()),
                    _ => None,
                }).collect();
                if let Some(keys) = keys.filter(|k| (2..=24).contains(&k.len())) {
                    rows = keys.iter().zip(&d.items).map(|(k, i)| Row { key: k.to_string(), value: Namer::plain(src).code(&i.value, 40), note: String::new(), heading: false }).collect();
                    // A paragraph's comment describes the object.
                    if let Some(t) = title.replace(names[0].clone()).filter(|t| t != "Constants" && doc.is_empty()) {
                        doc = t;
                    }
                }
            }
        }
        let big = rows.iter().filter(|r| !r.heading).count() >= 8;
        let doc = match (&title, big) {
            (Some(t), true) if doc.is_empty() && !single => t.clone(),
            _ => doc,
        };
        let title = match title {
            Some(t) if !big || t == "Constants" || single => t,
            _ if single => names[0].clone(),
            _ => "Settings".into(),
        };
        let id = self.add(Kind::Table, Shape::Box, title);
        // A table gathered from across a module points at its first row.
        // (Statements a front end hoisted can come out of source order.)
        let first = stmts.iter().map(|s| s.start()).min().unwrap_or_default();
        let last = if contiguous { stmts.iter().map(|s| s.end()).max().unwrap_or(first) } else { stmts[0].end() };
        let span = self.src().span(TextRange::new(first.min(last), last.max(first)));
        {
            let n = self.cx.b.get_mut(id);
            n.category = "config";
            n.rows = rows;
            n.doc = doc;
            n.span = Some(span);
            n.outputs = names.clone();
        }
        for name in names {
            self.bind(&name, Bind::Src(vec![id], Style::Ctx));
        }
    }

    // ----- statements ------------------------------------------------------------

    fn stmt(&mut self, s: &'p Stmt, later: &HashSet<String>) {
        if self.module_level && self.handled.contains(&u32::from(s.start())) {
            return;
        }
        match s {
            Stmt::Expr(e) => self.expr_stmt(s, &e.value),
            Stmt::Assign(a) => self.assign(s, &a.targets, &a.value),
            Stmt::AnnAssign(a) => {
                if let Some(v) = &a.value {
                    self.assign(s, std::slice::from_ref(&*a.target), v)
                }
            }
            Stmt::AugAssign(a) => self.aug_assign(s, a),
            Stmt::Return(r) => self.ret(s, r.value.as_deref(), "return"),
            // Inside a branch arm, leaving the loop is what the arm does.
            Stmt::Continue(_) | Stmt::Break(_) if self.in_branch() => {
                self.exit_step(s, if matches!(s, Stmt::Continue(_)) { "continue" } else { "break" }, None, vec![], &Gather::default());
            }
            Stmt::If(x) => self.if_stmt(s, x, later),
            Stmt::For(x) => self.for_stmt(s, x, later),
            Stmt::While(x) => self.while_stmt(s, &x.test, &x.body, later),
            Stmt::With(x) => self.with_stmt(&x.items, &x.body, later),
            Stmt::Try(x) => self.try_stmt(s, &x.body, &x.handlers, &x.orelse, &x.finalbody, later),
            Stmt::Match(x) => self.match_stmt(s, x, later),
            Stmt::FunctionDef(_) => self.def_stmt(s),
            Stmt::ClassDef(c) => self.class_stmt(c),
            Stmt::Import(_) | Stmt::ImportFrom(_) => self.import_stmt(s),
            Stmt::Raise(r) => self.raise_stmt(s, r),
            Stmt::Delete(d) => {
                for t in &d.targets {
                    if let Expr::Name(n) = t {
                        self.regions.last_mut().unwrap().env.insert(n.id.to_string(), Bind::Static);
                    }
                }
            }
            Stmt::Global(g) => {
                for n in &g.names {
                    self.globals_declared.insert(n.to_string());
                }
            }
            _ => {}
        }
    }

    fn import_stmt(&mut self, s: &Stmt) {
        let mut names = BTreeSet::new();
        walk::collect_bindings(std::slice::from_ref(s), &mut names, &mut BTreeSet::new());
        for n in names {
            let t = self.cx.ix.sym_ty(self.mi, &n, 0);
            if t != Ty::Unknown {
                self.types.insert(n.clone(), t);
            }
            self.bind(&n, Bind::Static);
        }
    }

    fn is_print(&self, c: &ast::ExprCall) -> bool {
        match &*c.func {
            Expr::Name(n) => matches!(n.id.as_str(), "print" | "pprint") || is_print_macro(&n.id) || is_log_macro(&n.id),
            // `log::info!`, `tracing::warn!`
            Expr::Attribute(a) if is_log_macro(&a.attr) => true,
            // Go: `fmt.Println`, `log.Printf`, `slog.Info`, `logger.Infof`, `fmt.Fprintf(os.Stderr, …)`.
            Expr::Attribute(a) if self.m.lang == Lang::Go => {
                let base = self.src().of(&*a.value);
                let m = a.attr.as_str();
                let leveled = ["Debug", "Info", "Warn", "Warning", "Error", "Trace", "Print", "Notice"].iter().any(|l| m.strip_prefix(l).is_some_and(|rest| matches!(rest, "" | "f" | "ln" | "w" | "Context" | "Ctx")));
                (base == "fmt" && matches!(m, "Print" | "Println" | "Printf"))
                    || (base == "fmt" && m.starts_with("Fprint") && matches!(c.arguments.args.first(), Some(Expr::Attribute(w)) if self.src().of(w) == "os.Stderr" || self.src().of(w) == "os.Stdout"))
                    || (matches!(base, "log" | "slog" | "klog" | "glog" | "logrus" | "zap" | "zerolog") || base.ends_with("log") || base.ends_with("logger") || base.ends_with("Logger") || base.ends_with(".log") || base == "l" || base == "lg") && leveled
            }
            Expr::Attribute(a) => {
                let base = self.src().of(&*a.value);
                (base == "console" && matches!(a.attr.as_str(), "log" | "info" | "warn" | "error" | "debug" | "trace" | "table" | "dir" | "group" | "groupEnd" | "time" | "timeEnd"))
                    || (matches!(base, "logging" | "logger" | "log" | "LOGGER" | "LOG" | "tqdm" | "sys.stdout" | "sys.stderr" | "warnings" | "pbar")
                    && matches!(a.attr.as_str(), "debug" | "info" | "warning" | "warn" | "error" | "exception" | "critical" | "write" | "set_description" | "set_postfix"))
                    || (base == "pprint" && a.attr.as_str() == "pprint")
            }
            _ => false,
        }
    }

    fn expr_stmt(&mut self, s: &'p Stmt, value: &'p Expr) {
        match value {
            e if is_constant(e) => {}
            Expr::Call(c) if self.is_print(c) => {
                let label = match &*c.func {
                    Expr::Name(n) if is_log_macro(&n.id) => "logs",
                    Expr::Name(_) => "prints",
                    // Go's `fmt.Println` prints; loggers log.
                    Expr::Attribute(a) if self.m.lang == Lang::Go && self.src().of(&*a.value) == "fmt" => "prints",
                    _ => "logs",
                };
                self.effect(label);
                // Reading values to print is not a dataflow worth drawing.
            }
            Expr::Call(c) if is_super(c) => {}
            Expr::Yield(y) => self.ret(s, y.value.as_deref(), "yield"),
            Expr::YieldFrom(y) => self.ret(s, Some(&y.value), "yield"),
            _ => {
                let mut g = Gather::default();
                let outcome = self.value(value, &mut g, &Dest::None);
                let Some(id) = outcome.node else { return };
                self.finish_node(id, s, &g);
                // `model.to(device)` / `optimizer.zero_grad()` change their receiver.
                if let Expr::Call(c) = value {
                    if let Expr::Attribute(a) = &*c.func {
                        if let Expr::Name(n) = &*a.value {
                            if let Some(Bind::Src(_, st)) = self.peek(&n.id) {
                                if *st != Style::Def {
                                    let st = *st;
                                    let name = n.id.to_string();
                                    self.bind(&name, Bind::Src(vec![id], if st == Style::Ctx { Style::Data } else { st }));
                                    self.cx.b.get_mut(id).outputs.push(name);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn finish_node(&mut self, id: Id, s: &impl Ranged, g: &Gather) {
        if self.cx.b.get(id).span.is_none() {
            self.set_span(id, s);
        }
        let line = self.src().line_of(usize::from(s.start()));
        let comment = self.src().trailing_comment(line).filter(|c| !is_directive(c)).map(|c| c.to_string());
        let n = self.cx.b.get_mut(id);
        n.effects.extend(g.effects.iter().cloned());
        n.effects.sort();
        n.effects.dedup();
        if n.doc.is_empty() {
            if let Some(c) = comment {
                n.doc = c;
            }
        }
        if n.category == "op" && n.effects.iter().any(|e| e.starts_with("reads") || e.starts_with("writes") || e.contains("network") || e == "database") {
            n.category = "io";
        }
        for f in &g.reads {
            self.cx.relations.push(Relation::Artifact { module: self.mi, name: f.clone(), write: false });
        }
        for f in &g.writes {
            self.cx.relations.push(Relation::Artifact { module: self.mi, name: f.clone(), write: true });
        }
    }

    fn assign(&mut self, s: &'p Stmt, targets: &'p [Expr], value: &'p Expr) {
        let mut names = vec![];
        let simple = targets.iter().all(|t| matches!(t, Expr::Name(_) | Expr::Tuple(_) | Expr::List(_)));
        if simple {
            targets.iter().for_each(|t| walk::target_names(t, &mut names));
        }
        // self.x = ...
        let self_attr = targets.iter().find_map(|t| match t {
            Expr::Attribute(a) if self.is_self(&a.value) => Some(a.attr.as_str()),
            _ => None,
        });
        if simple {
            if !self.module_level && is_literal(value) && !self.in_branch() {
                let text = compact(self.src().of(value), 40);
                for n in &names {
                    if let Some(f) = string_literal(value).and_then(effects::file_name) {
                        self.files.insert(n.clone(), vec![f]);
                    }
                    self.bind(n, Bind::Const(text.clone()));
                }
                return;
            }
            // `a, b = x.a, x.b` only gives values names: no step of its own.
            if let ([Expr::Tuple(tt)], Expr::Tuple(vt)) = (targets, value) {
                let plain = |v: &Expr| is_literal(v) || (root_name(v).is_some() && !contains_call(v) && !matches!(v, Expr::Call(_)));
                if tt.elts.len() == vt.elts.len() && tt.elts.iter().all(|t| matches!(t, Expr::Name(_))) && vt.elts.iter().all(plain) && !self.in_branch() {
                    for (t, v) in tt.elts.iter().zip(&vt.elts) {
                        let Expr::Name(n) = t else { continue };
                        let mut g = Gather::default();
                        let mut vals = if is_literal(v) { vec![] } else { self.collect(v, &mut g, false) };
                        dedup_vals(&mut vals);
                        match vals.as_slice() {
                            [one] => {
                                let style = if one.scalar || one.style == Style::Ctx { Style::Ctx } else { Style::Data };
                                self.bind(&n.id, Bind::Src(one.srcs.clone(), style));
                            }
                            _ => self.bind(&n.id, Bind::Static),
                        }
                    }
                    return;
                }
            }
            // `const Post = z.object({ title: z.string() })`: a schema is a table of fields.
            if let Some(rows) = self.schema_rows(value) {
                let id = self.add(Kind::Table, Shape::Box, naming::list(&names, naming::TITLE_MAX));
                let code = compact(self.src().of(s), 200);
                let n = self.cx.b.get_mut(id);
                n.subtitle = "schema".into();
                n.category = "config";
                n.rows = rows;
                n.code = code;
                self.set_span(id, s);
                for name in &names {
                    self.bind(name, Bind::Src(vec![id], Style::Def));
                }
                return;
            }
            let mut g = Gather::default();
            let outcome = self.value(value, &mut g, &Dest::Names(names.clone()));
            // Types and file names flow through assignments.
            if names.len() == 1 {
                let ty = self.cx.ix.infer(self.mi, value, &TypesView(&self.types));
                if ty != Ty::Unknown {
                    self.types.insert(names[0].clone(), ty.clone());
                    if self.module_level {
                        self.cx.ix.var_types.borrow_mut().insert((self.mi, names[0].clone()), ty);
                    }
                } else {
                    self.types.remove(&names[0]);
                }
            } else if let (Expr::Tuple(tt), Expr::Tuple(vt)) = (&targets[0], value) {
                for (t, v) in tt.elts.iter().zip(&vt.elts) {
                    if let Expr::Name(n) = t {
                        let ty = self.cx.ix.infer(self.mi, v, &TypesView(&self.types));
                        self.types.insert(n.id.to_string(), ty);
                    }
                }
            } else if let (Expr::Tuple(tt), Lang::Go) = (&targets[0], self.m.lang) {
                // `db, err := Open(…)`, `v, ok := m[k]`: the first value is the
                // result; what follows is an error or a flag.
                if let Some(Expr::Name(n)) = tt.elts.first() {
                    let ty = self.cx.ix.infer(self.mi, value, &TypesView(&self.types));
                    if ty != Ty::Unknown {
                        self.types.insert(n.id.to_string(), ty);
                    }
                }
            }
            if is_path_expr(value) {
                let files = g_strings_files(self, value);
                for n in &names {
                    if !files.is_empty() {
                        self.files.insert(n.clone(), files.clone());
                    }
                }
            }
            let style = if outcome.scalar { Style::Ctx } else { Style::Data };
            match outcome {
                Outcome { node: Some(id), .. } => {
                    self.finish_node(id, s, &g);
                    self.cx.b.get_mut(id).outputs = names.clone();
                    for n in &names {
                        self.bind(n, Bind::Src(vec![id], style));
                    }
                }
                // In a branch arm, choosing a value is what the arm does: show it.
                Outcome { alias, .. } if self.in_branch() => {
                    let vals: Vec<Val> = alias.into_iter().collect();
                    let id = self.store_step(s, &Dest::Names(names.clone()), value, &vals);
                    self.finish_node(id, s, &g);
                    self.cx.b.get_mut(id).outputs = names.clone();
                    for n in &names {
                        self.bind(n, Bind::Src(vec![id], Style::Data));
                    }
                }
                Outcome { alias: Some(v), .. } => {
                    // `device = x.device` is metadata like a setting; `h = x` is `x`.
                    let style = match v.style {
                        Style::Def => Style::Data,
                        _ if v.scalar => Style::Ctx,
                        st => st,
                    };
                    for n in &names {
                        self.bind(n, Bind::Src(v.srcs.clone(), style));
                    }
                }
                _ => {
                    for n in &names {
                        self.bind(n, Bind::Static);
                    }
                }
            }
            return;
        }
        if let Some(attr) = self_attr {
            if self.in_init && !self.builds_component(value) && !self.in_branch() {
                return;
            }
            // Component construction in __init__ reads as "c_attn: Linear".
            let mut g = Gather::default();
            let dest = Dest::Field { owner: self.this.as_ref().map(|t| t.0.clone()).unwrap_or_default(), key: attr.to_string(), item: false };
            let outcome = self.value(value, &mut g, &dest);
            if let Some(id) = outcome.node {
                self.finish_node(id, s, &g);
                if outcome.root_call {
                    let written = match value {
                        Expr::Call(c) => squash(self.src().of(&*c.func)),
                        Expr::If(x) => match (&*x.body, &*x.orelse) {
                            (Expr::Call(c), _) | (_, Expr::Call(c)) => squash(self.src().of(&*c.func)),
                            _ => String::new(),
                        },
                        _ => String::new(),
                    };
                    let n = self.cx.b.get_mut(id);
                    let ctor = std::mem::take(&mut n.title);
                    if !n.subtitle.contains('×') {
                        n.subtitle = if written.is_empty() { ctor } else { naming::qualifier(&written, 28) };
                    }
                    n.title = attr.to_string();
                    let sub = n.subtitle.clone();
                    if matches!(n.category, "library" | "class" | "component") {
                        if let Some(c) = self.cx.category(self.mi, &[&sub, attr]) {
                            self.cx.b.get_mut(id).category = c;
                        }
                    }
                }
                if !self.in_init {
                    let effect = format!("mutates {}.{attr}", self.receiver());
                    self.cx.b.get_mut(id).effects.push(effect);
                }
            } else if !self.in_init || self.in_branch() {
                // A plain state update outside the constructor is still worth showing,
                // and so is one that a branch of the constructor chooses.
                let vals = {
                    let mut g = Gather::default();
                    self.collect(value, &mut g, false)
                };
                if !vals.is_empty() || self.in_branch() {
                    let name = naming::name(&Step::Other, &dest, "");
                    let id = self.add(Kind::Op, Shape::Box, name.title);
                    self.cx.b.get_mut(id).subtitle = name.caption;
                    self.cx.b.get_mut(id).category = "op";
                    self.cx.b.get_mut(id).code = compact(self.src().of(s), 120);
                    let effect = format!("mutates {}.{attr}", self.receiver());
                    self.cx.b.get_mut(id).effects.push(effect);
                    self.set_span(id, s);
                    for v in vals {
                        self.link(&v, id);
                    }
                }
            }
            return;
        }
        // `schema.statics.isTaken = function …`: the function is the definition.
        if let Expr::Function(f) = value {
            let name = if f.name.is_empty() { "function".to_string() } else { f.name.clone() };
            self.callback_node(f, &name);
            return;
        }
        // Writes into an object or container: `out[split] = ...`, `model.flag = ...`.
        let dest = self.namer().dest(&targets[0]);
        let mut g = Gather::default();
        let outcome = self.value(value, &mut g, &dest);
        let mut vals = vec![];
        let mut root_var = None;
        let mut module_attr = false;
        for t in targets {
            let (base, rest) = split_target(t);
            if let Some(b) = base {
                if let Some(Bind::Src(..)) = self.peek(b) {
                    root_var = Some(b.to_string());
                } else if matches!(self.cx.ix.sym(self.mi, b), Some(Sym::Ext(_)) | Some(Sym::Module(_))) {
                    module_attr = true;
                }
            }
            for r in rest {
                vals.extend(self.collect(r, &mut g, false));
            }
            if let Some(b) = base {
                vals.extend(self.read(b, &mut g));
            }
        }
        let id = match outcome {
            // A call stored under a name is named by it, like a component:
            // `Authorization` ← _build_auth_header.
            Outcome { node: Some(id), root_call: true, .. } => {
                if let Dest::Field { key, .. } = &dest {
                    let n = self.cx.b.get_mut(id);
                    let callee = std::mem::replace(&mut n.title, key.clone());
                    n.subtitle = callee;
                }
                id
            }
            Outcome { node: Some(id), .. } => id,
            // A plain value stored: the store is the step.
            Outcome { alias, .. } => {
                let name = naming::name(&Step::Other, &dest, "");
                let id = self.add(Kind::Op, Shape::Box, name.title);
                self.cx.b.get_mut(id).subtitle = name.caption;
                self.cx.b.get_mut(id).category = "op";
                vals.extend(alias);
                id
            }
        };
        if module_attr {
            self.cx.b.get_mut(id).category = "config";
            g.effects.insert("sets library state".into());
        }
        self.cx.b.get_mut(id).code = compact(self.src().of(s), 160);
        self.set_span(id, s);
        dedup_vals(&mut vals);
        for v in &vals {
            self.link(v, id);
        }
        self.finish_node(id, s, &g);
        if let Some(var) = root_var {
            let var = self.loop_alias.get(&var).cloned().unwrap_or(var);
            let st = match self.peek(&var) {
                Some(Bind::Src(_, Style::Ctx)) => Style::Data,
                Some(Bind::Src(_, st)) => *st,
                _ => Style::Data,
            };
            self.cx.b.get_mut(id).outputs = vec![var.clone()];
            self.bind(&var, Bind::Src(vec![id], st));
        }
    }

    fn aug_assign(&mut self, s: &'p Stmt, a: &'p ast::StmtAugAssign) {
        let mut g = Gather::default();
        let mut vals = self.collect(&a.target, &mut g, false);
        let rhs = self.collect(&a.value, &mut g, false);

        let both_rich = vals.iter().any(|v| !v.scalar) && rhs.iter().any(|v| !v.scalar);
        vals.extend(rhs);
        dedup_vals(&mut vals);
        let glyph = both_rich && matches!(a.op, ast::Operator::Add | ast::Operator::Mult | ast::Operator::MatMult);
        let id = if glyph {
            let id = self.add(Kind::Glyph, Shape::Glyph, op_symbol(a.op));
            self.cx.b.get_mut(id).category = "op";
            id
        } else {
            let name = {
                let namer = self.namer();
                let step = Step::Update { op: format!("{}=", a.op.as_str()), operand: namer.code(&a.value, 24) };
                naming::name(&step, &namer.dest(&a.target), "")
            };
            let id = self.add(Kind::Op, Shape::Box, name.title);
            self.cx.b.get_mut(id).subtitle = name.caption;
            self.cx.b.get_mut(id).category = "op";
            id
        };
        self.cx.b.get_mut(id).code = compact(self.src().of(s), 160);
        for v in &vals {
            self.link(v, id);
        }
        self.finish_node(id, s, &g);
        if let Expr::Name(n) = &*a.target {
            let name = n.id.to_string();
            self.cx.b.get_mut(id).outputs = vec![name.clone()];
            self.bind(&name, Bind::Src(vec![id], Style::Data));
        } else if let (Some(base), _) = split_target(&a.target) {
            if let Some(Bind::Src(..)) = self.peek(base) {
                let base = base.to_string();
                self.bind(&base, Bind::Src(vec![id], Style::Data));
            }
        }
    }

    /// Inside an arm of a branch (a lane, or a single guarded statement),
    /// where every statement shows what the arm does.
    fn in_branch(&self) -> bool {
        self.guarded || self.cx.b.get(self.container()).kind == Kind::Lane
    }

    /// `return x`, `continue` or `break` whose value makes no block of its own.
    fn bare_exit(&self, s: &Stmt) -> bool {
        match s {
            Stmt::Continue(_) | Stmt::Break(_) => true,
            Stmt::Return(r) => r.value.as_deref().is_none_or(|v| {
                let mut work = false;
                walk::walk(v, &mut |x| match x {
                    Expr::Call(c) => work |= self.is_major(&self.classify(c), c),
                    Expr::Yield(_) | Expr::YieldFrom(_) | Expr::Await(_) | Expr::Lambda(_) => work = true,
                    _ => {}
                });
                !work
            }),
            _ => false,
        }
    }

    /// A result: `return x` or `yield x`. Values packed into a tuple flow on
    /// as they are. Inside a branch arm, a result that makes no block of its
    /// own (`return None`, `return value`) is drawn as a step, so the arm
    /// shows what it does; the value flows through it to the result.
    fn ret(&mut self, s: &impl Ranged, value: Option<&'p Expr>, kind: &str) {
        let in_branch = self.in_branch();
        let Some(v) = value else {
            if in_branch {
                self.exit_step(s, kind, None, vec![], &Gather::default());
            }
            return;
        };
        let mut g = Gather::default();
        let mut made = false;
        let mut vals: Vec<Val> = match v {
            Expr::Tuple(t) if t.elts.iter().all(|e| self.passes_through(e)) => {
                let mut out = vec![];
                for e in &t.elts {
                    match e {
                        Expr::Name(n) => out.extend(self.read(&n.id, &mut g)),
                        e if is_literal(e) => {}
                        e => {
                            let before = self.cx.b.nodes.len();
                            out.extend(self.collect(e, &mut g, false));
                            made |= self.cx.b.nodes.len() > before;
                        }
                    }
                }
                out
            }
            _ => {
                let outcome = self.value(v, &mut g, &Dest::None);
                if let Some(id) = outcome.node {
                    made = true;
                    let stmt_range = v.range();
                    self.finish_node(id, &RangeOf(stmt_range), &g);
                    // Glue that a branch returns is the arm's exit: `return int(…)`.
                    if in_branch && self.cx.b.get(id).kind == Kind::Op {
                        let code = self.namer().code(v, naming::CAPTION_MAX);
                        let name = naming::name(&Step::Exit { verb: kind.into(), value: code.clone() }, &Dest::None, &code);
                        let n = self.cx.b.get_mut(id);
                        n.title = name.title;
                        n.subtitle = name.caption;
                    }
                    vec![Val { srcs: vec![id], label: String::new(), style: Style::Data, scalar: false }]
                } else if let Some(a) = outcome.alias {
                    vec![a]
                } else {
                    vec![]
                }
            }
        };
        dedup_vals(&mut vals);
        if in_branch && !made {
            let id = self.exit_step(s, kind, Some(v), vals, &g);
            let label = if let Expr::Name(n) = v { n.id.to_string() } else { String::new() };
            vals = vec![Val { srcs: vec![id], label, style: Style::Data, scalar: false }];
        }
        let label_text = compact(self.src().of(v), 28);
        let r = self.regions.last_mut().unwrap();
        r.returns.extend(vals);
        if r.returns.is_empty() {
            r.returns.push(Val { srcs: vec![], label: String::new(), style: Style::Data, scalar: false });
        }
        r.effects.insert(format!("__{kind}:{label_text}"));
    }

    /// Values a tuple can pack without a step of its own: names, constants,
    /// attribute reads and calls that are blocks anyway.
    fn passes_through(&self, e: &Expr) -> bool {
        match e {
            Expr::Name(_) => true,
            e if is_literal(e) => true,
            Expr::Attribute(_) => root_name(e).is_some() && !contains_call(e),
            Expr::Call(c) => self.is_major(&self.classify(c), c),
            _ => false,
        }
    }

    /// A step for leaving early or producing a value without other work:
    /// `return None`, `yield chunk`, `continue`. `vals` flow into it.
    fn exit_step(&mut self, s: &impl Ranged, verb: &str, value: Option<&'p Expr>, vals: Vec<Val>, g: &Gather) -> Id {
        let code = value.map(|v| self.namer().code(v, naming::CAPTION_MAX)).unwrap_or_default();
        let name = naming::name(&Step::Exit { verb: verb.into(), value: code.clone() }, &Dest::None, &code);
        let id = self.add(Kind::Op, Shape::Box, name.title);
        {
            let src = self.src();
            let n = self.cx.b.get_mut(id);
            n.subtitle = name.caption;
            n.category = "op";
            n.code = compact(src.of(s), 160);
        }
        self.set_span(id, s);
        for v in &vals {
            self.link(v, id);
        }
        self.finish_node(id, s, g);
        id
    }

    /// An arm that drew nothing still shows what it does when that is an
    /// import (`try: import h2`, an optional dependency) or a `super()` call;
    /// an arm of only `pass` or logging stays empty.
    fn fill_empty_arm(&mut self, body: &'p [Stmt]) {
        let lane = self.container();
        if self.cx.b.get(lane).children.iter().any(|&c| !self.cx.b.get(c).is_port()) {
            return;
        }
        for st in body {
            let (title, caption) = match st {
                Stmt::Import(i) => (format!("import {}", naming::list(&i.names.iter().map(|a| a.name.to_string()).collect::<Vec<_>>(), 24)), String::new()),
                Stmt::ImportFrom(i) => (format!("import {}", i.module.as_ref().map(|m| m.to_string()).unwrap_or_default()), String::new()),
                Stmt::Expr(e) => match &*e.value {
                    Expr::Call(c) if is_super(c) => match &*c.func {
                        Expr::Attribute(a) => (a.attr.to_string(), "super".to_string()),
                        _ => continue,
                    },
                    // `console.log("Directory already exists")`: an arm that only reports.
                    Expr::Call(c) if self.is_print(c) => {
                        let verb = match &*c.func {
                            Expr::Attribute(a) => a.attr.trim_end_matches('!').to_string(),
                            Expr::Name(n) if is_print_macro(&n.id) => "print".to_string(),
                            f => self.src().of(f).trim_end_matches('!').to_string(),
                        };
                        let what = c.arguments.args.first().map(|a| match string_literal(a) {
                            Some(t) => naming::shorten(t, naming::CAPTION_MAX),
                            None => self.namer().code(a, naming::CAPTION_MAX),
                        });
                        (verb, what.unwrap_or_default())
                    }
                    // `close(done)`, `cache.clear()`: an arm whose only step is a small call.
                    Expr::Call(c) => {
                        let func = self.namer().brief(&c.func);
                        let (verb, owner) = match func.rsplit_once('.') {
                            Some((o, v)) => (v.to_string(), o.to_string()),
                            None => (func, String::new()),
                        };
                        let what = if owner.is_empty() { naming::list(&c.arguments.args.iter().map(|a| self.namer().code(a, 20)).collect::<Vec<_>>(), naming::CAPTION_MAX) } else { owner };
                        (verb.trim_end_matches('!').to_string(), what)
                    }
                    _ => continue,
                },
                // `case Err(e): assert …`: an arm that only checks.
                Stmt::Assert(a) => ("assert".to_string(), self.namer().code(&a.test, naming::CAPTION_MAX)),
                // `drop(guard)`, `del cache[key]`: an arm that releases something.
                Stmt::Delete(d) => (if self.m.lang == Lang::Rust { "drop" } else { "del" }.to_string(), naming::list(&d.targets.iter().map(|t| self.namer().code(t, 20)).collect::<Vec<_>>(), naming::CAPTION_MAX)),
                _ => continue,
            };
            let id = self.add(Kind::Op, Shape::Box, title);
            let code = compact(self.src().of(st), 160);
            let n = self.cx.b.get_mut(id);
            n.subtitle = caption;
            n.category = if matches!(st, Stmt::Expr(_) | Stmt::Assert(_) | Stmt::Delete(_)) { "op" } else { "library" };
            n.code = code;
            self.set_span(id, st);
        }
    }

    /// A step for an assignment that makes no block of its own (`x = y`,
    /// `error = "…"`), named by where it stores; `vals` flow into it.
    fn store_step(&mut self, s: &Stmt, dest: &Dest, value: &Expr, vals: &[Val]) -> Id {
        let code = self.namer().code(value, naming::CAPTION_MAX);
        let name = naming::name(&Step::Other, dest, &code);
        let id = self.add(Kind::Op, Shape::Box, name.title);
        {
            let src = self.src();
            let n = self.cx.b.get_mut(id);
            n.subtitle = name.caption;
            n.category = "op";
            n.code = compact(src.of(s), 160);
        }
        self.set_span(id, s);
        for v in vals {
            self.link(v, id);
        }
        id
    }

    fn raise_stmt(&mut self, s: &'p Stmt, r: &'p ast::StmtRaise) {
        // Named by the exception: `raise ValueError`, not its module path or message.
        let (verb, effect) = match self.m.lang {
            Lang::Python => ("raise", "raises"),
            Lang::JavaScript | Lang::TypeScript => ("throw", "throws"),
            Lang::Rust | Lang::Go => ("error", "returns error"),
        };
        let go = self.m.lang == Lang::Go;
        let macro_name = |c: &ast::ExprCall| match &*c.func {
            Expr::Name(n) if n.id.ends_with('!') => Some(n.id.trim_end_matches('!').to_string()),
            // Go's `panic(err)`.
            Expr::Name(n) if go && n.id == "panic" => Some(n.id.clone()),
            Expr::Attribute(a) if a.attr.ends_with('!') => Some(a.attr.trim_end_matches('!').to_string()),
            _ => None,
        };
        let title = match r.exc.as_deref() {
            // Rust: `panic!("…")`, `unreachable!()`, `anyhow::bail!("…")`.
            Some(Expr::Call(c)) if macro_name(c).is_some() => {
                let mac = macro_name(c).unwrap();
                let mac = mac.as_str();
                let what = c.arguments.args.first().map(|a| self.namer().code(a, 24)).unwrap_or_default();
                match mac {
                    "bail" | "anyhow" | "format_err" if !what.is_empty() => format!("error {what}"),
                    "bail" | "anyhow" | "format_err" => "error".to_string(),
                    _ if what.is_empty() => mac.to_string(),
                    _ => format!("{mac} {what}"),
                }
            }
            // Go's `errors.New("not serving")`, `fmt.Errorf("read %s: %w", …)`: the message says which error.
            Some(Expr::Call(c)) if go && matches!(&*c.func, Expr::Attribute(a) if matches!(a.attr.as_str(), "New" | "Errorf") && matches!(self.src().of(&*a.value), "errors" | "fmt")) && !c.arguments.args.is_empty() => {
                format!("{verb} {}", self.namer().code(&c.arguments.args[0], 24))
            }
            Some(Expr::Call(c)) => format!("{verb} {}", self.namer().brief(&c.func)),
            // `Err((StatusCode::BAD_REQUEST, message))`: named by what leads.
            Some(Expr::Tuple(t)) if !t.elts.is_empty() => format!("{verb} {}", self.namer().brief(&t.elts[0])),
            // `Err(())`: an error that carries nothing.
            Some(Expr::NoneLiteral(_)) if matches!(self.m.lang, Lang::Rust | Lang::Go) => verb.to_string(),
            Some(e) => format!("{verb} {}", self.namer().brief(e)),
            None => format!("re-{verb}"),
        };
        let panics = matches!(r.exc.as_deref(), Some(Expr::Call(c)) if matches!(macro_name(c).as_deref(), Some("panic" | "unreachable" | "todo" | "unimplemented")));
        let effect = if panics { "panics" } else { effect };
        let id = self.add(Kind::Op, Shape::Box, title);
        self.cx.b.get_mut(id).category = "error";
        self.cx.b.get_mut(id).code = compact(self.src().of(s), 160);
        self.cx.b.get_mut(id).effects.push(effect.into());
        self.set_span(id, s);
        if let Some(e) = &r.exc {
            let mut g = Gather::default();
            let mut vals = self.collect(e, &mut g, true);
            dedup_vals(&mut vals);
            for v in &vals {
                self.link(v, id);
            }
        }
    }

    // ----- control flow ------------------------------------------------------------

    fn open_region(&mut self, kind: Kind, title: String, s: &impl Ranged) -> Id {
        let id = self.add(kind, Shape::Region, title);
        self.cx.b.get_mut(id).category = "region";
        self.set_span(id, s);
        self.regions.push(Region::new(id));
        id
    }

    fn close_region(&mut self, later: &HashSet<String>) {
        let r = self.regions.pop().unwrap();
        let node = r.node;
        let parent = self.regions.len() - 1;
        let visible_effects: Vec<String> = r.effects.iter().filter(|e| !e.starts_with("__")).cloned().collect();
        self.cx.b.get_mut(node).effects.extend(visible_effects);
        for e in &r.effects {
            if !e.starts_with("__") {
                self.regions[parent].effects.insert(e.clone());
            }
        }
        // Returns travel outward through an output port.
        let returns: Vec<Val> = r.returns.iter().filter(|v| !v.srcs.is_empty()).cloned().collect();
        if !r.returns.is_empty() {
            let label = return_label(&returns);
            if !returns.is_empty() {
                let port = self.cx.b.add(Some(node), Kind::Output, Shape::Port, if label.is_empty() { "return".to_string() } else { label.clone() });
                for v in &returns {
                    for &s in &v.srcs {
                        let fp = self.out_ports.get(&(s, v.label.clone())).copied();
                        self.connect(s, port, kind_of(v.style), &v.label, fp, None);
                    }
                }
                self.out_ports.insert((node, label.clone()), port);
            }
            self.regions[parent].returns.push(Val { srcs: if returns.is_empty() { vec![] } else { vec![node] }, label, style: Style::Data, scalar: false });
            for e in r.effects.iter().filter(|e| e.starts_with("__")) {
                self.regions[parent].effects.insert(e.clone());
            }
        }
        let mut seen = HashSet::new();
        for name in &r.assigned {
            if !seen.insert(name.clone()) || !later.contains(name) {
                continue;
            }
            match r.env.get(name) {
                Some(Bind::Src(srcs, style)) => {
                    if srcs.len() == 1 && self.cx.b.get(srcs[0]).parent == Some(node) && self.cx.b.get(srcs[0]).kind == Kind::Input && self.cx.b.get(srcs[0]).title == *name {
                        continue;
                    }
                    let port = match self.out_ports.get(&(node, name.clone())) {
                        Some(&p) => p,
                        None => {
                            let p = self.cx.b.add(Some(node), Kind::Output, Shape::Port, name.clone());
                            self.out_ports.insert((node, name.clone()), p);
                            p
                        }
                    };
                    for &s in srcs {
                        let fp = self.out_ports.get(&(s, name.clone())).copied();
                        let label = if *style == Style::Def { "" } else { name.as_str() };
                        self.connect(s, port, kind_of(*style), label, fp, None);
                    }
                    let pr = &mut self.regions[parent];
                    if pr.merging && pr.merged.contains(name) {
                        if let Some(Bind::Src(v, _)) = pr.env.get_mut(name) {
                            if !v.contains(&node) {
                                v.push(node);
                            }
                        }
                    } else {
                        pr.env.insert(name.clone(), Bind::Src(vec![node], *style));
                        if pr.merging {
                            pr.merged.insert(name.clone());
                        }
                    }
                    pr.assigned.push(name.clone());
                }
                Some(b) => {
                    let b = b.clone();
                    let pr = &mut self.regions[parent];
                    if pr.merging && pr.merged.contains(name) {
                        continue;
                    }
                    pr.env.insert(name.clone(), b);
                    pr.assigned.push(name.clone());
                }
                None => {}
            }
        }
    }

    fn link_condition(&mut self, cond: &[Val], region: Id) {
        let parent = self.cx.b.get(region).parent;
        let fed: HashSet<Id> = parent.map(|p| self.cx.b.get(p).edges.iter().filter(|e| e.to == region).map(|e| e.from).collect()).unwrap_or_default();
        for v in cond {
            if v.srcs.iter().all(|s| fed.contains(s)) {
                continue;
            }
            self.link_control(v, region);
        }
    }

    fn condition_vals(&mut self, test: &'p Expr) -> Vec<Val> {
        let mut g = Gather::default();
        let mut vals = self.collect(test, &mut g, false);
        dedup_vals(&mut vals);
        vals
    }

    fn if_stmt(&mut self, s: &'p Stmt, x: &'p ast::StmtIf, later: &HashSet<String>) {
        if self.module_level && self.regions.len() == 1 && is_main_guard(&x.test) {
            let file = self.cx.file_nodes[self.mi];
            self.cx.entry[self.mi] = true;
            let _ = file;
            if self.units(&x.body) >= 3 {
                self.section("Main".into(), "Runs when the file is executed as a script.".into(), &x.body, later);
            } else {
                self.statements(&x.body, later, true);
            }
            return;
        }
        if x.elif_else_clauses.len() >= MAX_LANES {
            let mut cond = self.condition_vals(&x.test);
            for c in x.elif_else_clauses.iter().filter_map(|c| c.test.as_ref()) {
                cond.extend(self.condition_vals(c));
            }
            dedup_vals(&mut cond);
            let mut arms = vec![(self.namer().code(&x.test, 30), &x.body[..])];
            arms.extend(x.elif_else_clauses.iter().map(|c| (c.test.as_ref().map(|t| self.namer().code(t, 30)).unwrap_or_else(|| "else".into()), &c.body[..])));
            self.case_table(s, "if / else if".to_string(), arms, &cond);
            return;
        }
        let mut lanes: Vec<(String, &'p [Stmt], Option<&'p Expr>, TextRange)> = vec![(format!("if {}", self.namer().code(&x.test, 40)), &x.body, Some(&*x.test), x.range)];
        for c in &x.elif_else_clauses {
            match &c.test {
                Some(t) => lanes.push((format!("{} {}", if self.m.lang == Lang::Python { "elif" } else { "else if" }, self.namer().code(t, 40)), &c.body, Some(t), c.range)),
                None => lanes.push(("else".into(), &c.body, None, c.range)),
            }
        }
        let tests: Vec<&'p Expr> = lanes.iter().filter_map(|l| l.2).collect();
        let mut cond: Vec<Val> = vec![];
        for t in &tests {
            cond.extend(self.condition_vals(t));
        }
        dedup_vals(&mut cond);

        if lanes.len() == 1 && self.units(lanes[0].1) <= 1 && lanes[0].1.iter().all(|s| !is_compound(s)) {
            // Guard tag instead of a box for a single guarded statement.
            let container = self.container();
            let before: HashSet<Id> = self.cx.b.get(container).children.iter().copied().collect();
            let guarded = std::mem::replace(&mut self.guarded, true);
            self.statements(lanes[0].1, later, false);
            self.guarded = guarded;
            let new: Vec<Id> = self.cx.b.get(container).children.iter().copied().filter(|c| !before.contains(c) && !self.cx.b.get(*c).is_port()).collect();
            // `if not user: raise …` guards with a negation: "unless user".
            let badge = match &*x.test {
                Expr::UnaryOp(u) if u.op.is_not() => format!("unless {}", self.namer().code(&u.operand, 31)),
                t => format!("if {}", self.namer().code(t, 31)),
            };
            for id in &new {
                self.cx.b.get_mut(*id).badges.push(badge.clone());
                for v in &cond {
                    self.link_control(v, *id);
                }
            }
            return;
        }
        if lanes.len() == 1 {
            let id = self.open_region(Kind::Branch, lanes[0].0.clone(), s);
            self.statements(lanes[0].1, later, true);
            self.close_region(later);
            self.link_condition(&cond, id);
            return;
        }
        self.branch(s, lanes, later);
    }

    /// A branch drawn as side-by-side lanes, one per arm.
    fn branch(&mut self, s: &impl Ranged, lanes: Vec<(String, &'p [Stmt], Option<&'p Expr>, TextRange)>, later: &HashSet<String>) {
        let mut cond: Vec<Val> = vec![];
        for t in lanes.iter().filter_map(|l| l.2) {
            cond.extend(self.condition_vals(t));
        }
        dedup_vals(&mut cond);
        let id = self.open_region(Kind::Branch, String::new(), s);
        self.cx.b.get_mut(id).hints.lanes = true;
        self.regions.last_mut().unwrap().merging = true;
        for (title, body, _, lane_stmt) in &lanes {
            let lane = self.add(Kind::Lane, Shape::Region, title.clone());
            self.cx.b.get_mut(lane).category = "lane";
            let span_range = if body.is_empty() { *lane_stmt } else { TextRange::new(body[0].start(), body[body.len() - 1].end()) };
            let span = self.src().span(span_range);
            self.cx.b.get_mut(lane).span = Some(span);
            self.regions.push(Region::new(lane));
            self.statements(body, later, true);
            self.fill_empty_arm(body);
            self.close_region(later);
        }
        self.close_region(later);
        self.link_condition(&cond, id);
    }

    fn for_stmt(&mut self, s: &'p Stmt, x: &'p ast::StmtFor, later: &HashSet<String>) {
        let (target, iter, body): (&'p Expr, &'p Expr, &'p [Stmt]) = (&x.target, &x.iter, &x.body);
        let title = match x.kind {
            ast::ForKind::In | ast::ForKind::Keys => format!("for {} in {}", self.namer().code(target, 16), self.namer().code(iter, 32)),
            ast::ForKind::Of => format!("for {} of {}", self.namer().code(target, 16), self.namer().code(iter, 32)),
            ast::ForKind::Count => format!("for {}", self.namer().code(iter, 40)),
        };
        let iter_ty = self.cx.ix.infer(self.mi, iter, &TypesView(&self.types));
        let count = match (&iter_ty, x.kind) {
            (Ty::List(_, Some(n)), _) => Some(n.clone()),
            // `for (let i = 0; i < n; i++)` runs n times.
            (_, ast::ForKind::Count) => match iter {
                Expr::Compare(c) if c.ops == [ast::CmpOp::Lt] => Some(count_text(self.src().of(&c.operands[1]))),
                _ => None,
            },
            _ => range_count(self.src(), iter),
        };
        let elem_ty = match &iter_ty {
            Ty::List(t, _) => (**t).clone(),
            _ => Ty::Unknown,
        };
        let iter_root = root_name(iter).map(|s| s.to_string());
        let mut g = Gather::default();
        let mut iter_vals = self.collect(iter, &mut g, true);
        dedup_vals(&mut iter_vals);
        let id = self.open_region(Kind::Loop, title, s);
        if let Some(n) = &count {
            self.cx.b.get_mut(id).badges.push(format!("× {n}"));
        }
        let mut names = vec![];
        walk::target_names(target, &mut names);
        let data_iter = iter_root.as_deref().and_then(|r| match self.peek(r) {
            Some(Bind::Src(_, st)) if *st != Style::Def => Some(r.to_string()),
            _ => None,
        });
        for n in &names {
            if elem_ty != Ty::Unknown && names.len() == 1 {
                self.types.insert(n.clone(), elem_ty.clone());
            }
        }
        match &data_iter {
            Some(root) => {
                if let Some((srcs, st)) = self.lookup(root) {
                    for n in &names {
                        self.regions.last_mut().unwrap().env.insert(n.clone(), Bind::Src(srcs.clone(), st));
                        self.loop_alias.insert(n.clone(), root.clone());
                    }
                }
            }
            None => {
                for n in &names {
                    self.regions.last_mut().unwrap().env.insert(n.clone(), Bind::Static);
                }
            }
        }
        let mut body_later = later.clone();
        for st in body {
            body_later.extend(stmt_loads(st));
        }
        self.statements(body, &body_later, true);
        self.close_region(later);
        if data_iter.is_none() {
            for v in &iter_vals {
                self.link(v, id);
            }
        }
    }

    fn while_stmt(&mut self, s: &'p Stmt, test: &'p Expr, body: &'p [Stmt], later: &HashSet<String>) {
        let forever = matches!(test, Expr::BooleanLiteral(b) if b.value);
        let title = if forever { "repeat".to_string() } else { format!("while {}", self.namer().code(test, 36)) };
        let cond = if forever { vec![] } else { self.condition_vals(test) };
        let id = self.open_region(Kind::Loop, title, s);
        if forever && has_break(body) {
            self.cx.b.get_mut(id).badges.push("until break".into());
        }
        let mut body_later = later.clone();
        for st in body {
            body_later.extend(stmt_loads(st));
        }
        body_later.extend(walk::loads(test));
        self.statements(body, &body_later, true);
        self.close_region(later);
        self.link_condition(&cond, id);
    }

    fn with_stmt(&mut self, items: &'p [ast::WithItem], body: &'p [Stmt], later: &HashSet<String>) {
        // Go's `go f()` and `defer f()`: the steps run, marked with when.
        if let ([item], Lang::Go) = (items, self.m.lang) {
            if let Expr::Name(n) = &item.context_expr {
                let when = match n.id.as_str() {
                    "go" => Some("in background"),
                    "defer" => Some("on return"),
                    _ => None,
                };
                if let Some(when) = when {
                    let first = self.cx.b.nodes.len() as Id;
                    self.statements(body, later, false);
                    let container = self.container();
                    for id in first..self.cx.b.nodes.len() as Id {
                        let n = self.cx.b.get_mut(id);
                        if n.parent == Some(container) && !n.is_port() && !n.badges.iter().any(|b| b == when) {
                            n.badges.push(when.to_string());
                        }
                    }
                    return;
                }
            }
        }
        for item in items {
            let e = &item.context_expr;
            let callee = match e {
                Expr::Call(c) => Some(self.classify(c)),
                _ => None,
            };
            let major = callee.as_ref().is_some_and(|c| self.is_major(c, match e {
                Expr::Call(c) => c,
                _ => unreachable!(),
            }));
            let mut names = vec![];
            if let Some(v) = &item.optional_vars {
                walk::target_names(v, &mut names);
            }
            if major && !names.is_empty() {
                let mut g = Gather::default();
                let outcome = self.value(e, &mut g, &Dest::Names(names.clone()));
                if let Some(id) = outcome.node {
                    self.finish_node(id, e, &g);
                    self.cx.b.get_mut(id).outputs = names.clone();
                    let files: Vec<String> = g.reads.iter().chain(&g.writes).cloned().collect();
                    for n in &names {
                        self.files.insert(n.clone(), files.clone());
                        self.bind(n, Bind::Src(vec![id], Style::Data));
                    }
                    continue;
                }
            }
            for n in &names {
                self.bind(n, Bind::Static);
            }
        }
        self.statements(body, later, false);
    }

    fn try_stmt(&mut self, s: &'p Stmt, body: &'p [Stmt], handlers: &'p [ast::ExceptHandler], orelse: &'p [Stmt], finalbody: &'p [Stmt], later: &HashSet<String>) {
        let trivial = handlers.iter().all(|h| self.units(&h.body) == 0) && self.units(finalbody) == 0;
        if trivial {
            self.statements(body, later, true);
            self.statements(orelse, later, true);
            return;
        }
        let id = self.open_region(Kind::Branch, String::new(), s);
        self.cx.b.get_mut(id).hints.lanes = true;
        self.regions.last_mut().unwrap().merging = true;
        let mut lanes: Vec<(String, &'p [Stmt])> = vec![("try".into(), body)];
        for h in handlers {
            // JavaScript catches everything; Python names what it catches.
            let title = match &h.type_ {
                _ if self.m.lang.is_js() => "catch".to_string(),
                Some(t) => format!("except {}", self.namer().code(t, 28)),
                None => "except Exception".into(),
            };
            lanes.push((title, &h.body));
        }
        if !orelse.is_empty() {
            lanes.push(("else".into(), orelse));
        }
        if !finalbody.is_empty() {
            lanes.push(("finally".into(), finalbody));
        }
        for (title, b) in lanes {
            let lane = self.add(Kind::Lane, Shape::Region, title);
            self.cx.b.get_mut(lane).category = "lane";
            if let (Some(first), Some(last)) = (b.first(), b.last()) {
                let span = self.src().span(TextRange::new(first.start(), last.end()));
                self.cx.b.get_mut(lane).span = Some(span);
            }
            self.regions.push(Region::new(lane));
            self.statements(b, later, true);
            self.fill_empty_arm(b);
            self.close_region(later);
        }
        self.close_region(later);
    }

    /// A branch with more arms than anyone reads side by side (an option
    /// parser's `switch arg` with hundreds of cases) is a table of its arms:
    /// what each one matches and what it does first.
    fn case_table(&mut self, s: &'p Stmt, title: String, arms: Vec<(String, &'p [Stmt])>, cond: &[Val]) {
        let rows: Vec<Row> = arms
            .iter()
            .map(|(key, body)| {
                let first = body.iter().find(|st| !matches!(st, Stmt::Pass(_)));
                let value = first.map(|st| compact(self.src().of(st), 40)).unwrap_or_default();
                Row { key: naming::shorten(key, 30), value, note: String::new(), heading: false }
            })
            .collect();
        let id = self.add(Kind::Table, Shape::Box, title);
        let code = compact(self.src().of(s), 160);
        let n = self.cx.b.get_mut(id);
        n.subtitle = naming::count(arms.len(), "case");
        n.category = "config";
        n.rows = rows;
        n.code = code;
        self.set_span(id, s);
        // What the arms do still shows: errors, printing, logging.
        let mut effects: BTreeSet<String> = BTreeSet::new();
        let (raises, _) = match self.m.lang {
            Lang::Python => ("raises", ""),
            Lang::JavaScript | Lang::TypeScript => ("throws", ""),
            Lang::Rust | Lang::Go => ("returns error", ""),
        };
        let mut stack: Vec<&[Stmt]> = arms.iter().map(|(_, b)| *b).collect();
        while let Some(body) = stack.pop() {
            for st in body {
                match st {
                    Stmt::Raise(_) => {
                        effects.insert(raises.to_string());
                    }
                    Stmt::Expr(e) => {
                        if let Expr::Call(c) = &*e.value {
                            if self.is_print(c) {
                                effects.insert("prints".to_string());
                            }
                        }
                    }
                    Stmt::If(i) => {
                        stack.push(&i.body);
                        stack.extend(i.elif_else_clauses.iter().map(|c| &c.body[..]));
                    }
                    Stmt::For(f) => stack.push(&f.body),
                    Stmt::While(w) => stack.push(&w.body),
                    Stmt::With(w) => stack.push(&w.body),
                    _ => {}
                }
            }
        }
        self.cx.b.get_mut(id).effects = effects.into_iter().collect();
        for v in cond {
            self.link_control(v, id);
        }
    }

    fn match_stmt(&mut self, s: &'p Stmt, x: &'p ast::StmtMatch, later: &HashSet<String>) {
        let cond = self.condition_vals(&x.subject);
        if x.cases.len() > MAX_LANES {
            let title = format!("{} {}", if self.m.lang.is_js() || self.m.lang == Lang::Go { "switch" } else { "match" }, self.namer().code(&x.subject, 30));
            let arms = x.cases.iter().map(|c| {
                let key = match &c.patterns[..] {
                    [] => "default".to_string(),
                    ps => naming::list(&ps.iter().map(|p| compact(self.src().slice(*p), 20)).collect::<Vec<_>>(), 30),
                };
                (key, &c.body[..])
            }).collect();
            self.case_table(s, title, arms, &cond);
            return;
        }
        // A Go `select` waits on channels; there is no value to switch on.
        let title = match &*x.subject {
            Expr::Name(n) if self.m.lang == Lang::Go && n.id == "select" => "select".to_string(),
            subject => format!("{} {}", if self.m.lang.is_js() || self.m.lang == Lang::Go { "switch" } else { "match" }, self.namer().code(subject, 30)),
        };
        let id = self.open_region(Kind::Branch, title, s);
        self.cx.b.get_mut(id).hints.lanes = true;
        self.regions.last_mut().unwrap().merging = true;
        // What the patterns bind comes from the subject.
        let binds = x.cases.iter().any(|c| c.binds.is_some());
        let subject: Option<(Vec<Id>, Style)> = match &*x.subject {
            _ if !binds => None,
            Expr::Name(n) => self.lookup(&n.id),
            _ => {
                let srcs: Vec<Id> = cond.iter().filter(|v| v.style != Style::Def).flat_map(|v| v.srcs.clone()).collect();
                (!srcs.is_empty()).then_some((srcs, Style::Data))
            }
        };
        // `match command { Get(cmd) => cmd.apply(…) }`: an arm unpacking one
        // value of an enum variant gives it the variant's type.
        let subject_class = match self.ty(&x.subject) {
            Ty::Inst(c) | Ty::Class(c) if self.cx.ix.classes[c].def.kind == ast::ClassKind::Enum => Some(c),
            _ => None,
        };
        // Rust arms are their patterns; Python and JavaScript say `case`.
        let case = if self.m.lang == Lang::Rust { "" } else { "case " };
        for case_ in &x.cases {
            let case_title = |p: &str| format!("{case}{p}");
            let title = match &case_.patterns[..] {
                [] if self.m.lang == Lang::Rust => "otherwise".to_string(),
                [] => "default".to_string(),
                [p] => case_title(&compact(self.src().slice(*p), 30)),
                many => case_title(&naming::list(&many.iter().map(|p| compact(self.src().slice(*p), 20)).collect::<Vec<_>>(), 30)),
            };
            // `case x if x > 0`: the guard tells arms with one pattern apart.
            let title = match &case_.guard {
                Some(g) => format!("{title} if {}", self.namer().code(g, 30)),
                None => title,
            };
            let case = case_;
            let lane = self.add(Kind::Lane, Shape::Region, title);
            self.cx.b.get_mut(lane).category = "lane";
            let span = self.src().span(case.range);
            self.cx.b.get_mut(lane).span = Some(span);
            self.regions.push(Region::new(lane));
            if let (Some(c), Some(v), Some(Expr::Name(n))) = (subject_class, &case.variant, case.binds.as_deref()) {
                let payload = self.cx.ix.classes[c].def.body.iter().find_map(|s| match s {
                    Stmt::AnnAssign(a) if matches!(&*a.target, Expr::Name(t) if t.id == *v) => Some(&*a.annotation),
                    _ => None,
                });
                let mi = self.cx.ix.classes[c].module;
                if let Some(t) = payload.map(|p| self.cx.ix.annotated(mi, p, &NoLocals)).filter(|t| *t != Ty::Unknown) {
                    self.types.insert(n.id.to_string(), t);
                }
            }
            // A Go type switch's case names the type the value has in it.
            if let (None, Some(v), Some(Expr::Name(n)), Lang::Go) = (subject_class, &case.variant, case.binds.as_deref(), self.m.lang) {
                if let Ty::Class(c) = self.cx.ix.sym_ty(self.mi, v, 0) {
                    self.types.insert(n.id.to_string(), Ty::Inst(c));
                }
            }
            if let Some(b) = &case.binds {
                let mut names = vec![];
                walk::target_names(b, &mut names);
                for n in names {
                    let bind = match &subject {
                        Some((srcs, st)) => Bind::Src(srcs.clone(), *st),
                        None => Bind::Static,
                    };
                    self.regions.last_mut().unwrap().env.insert(n, bind);
                }
            }
            self.statements(&case.body, later, true);
            self.fill_empty_arm(&case.body);
            self.close_region(later);
        }
        self.close_region(later);
        for v in &cond {
            self.link_control(v, id);
        }
    }

    // ----- definitions -------------------------------------------------------------

    fn def_stmt(&mut self, s: &'p Stmt) {
        let f = FnDef::from_stmt(s).unwrap();
        let key = (self.mi, u32::from(f.range.start()));
        let fid = match self.cx.ix.defs_at.get(&key) {
            Some(Sym::Func(id)) => *id,
            _ => {
                let node = self.cx.b.add(None, Kind::Function, Shape::Box, f.name);
                let class = None;
                self.cx.ix.register_nested_func(self.mi, f, class, node)
            }
        };
        let node = self.cx.ix.funcs[fid].node;
        let container = self.container();
        attach(&mut self.cx.b, container, node);
        self.describe_function(fid, s);
        let outer = self.link_free_names(f, node);
        self.types.insert(f.name.to_string(), Ty::Func(fid));
        self.bind(f.name, Bind::Src(vec![node], Style::Def));
        let types = self.types.clone();
        build_function(self.cx, fid, outer, types);
    }

    /// Values a nested function reads from the code around it flow into its
    /// node; returns the names it can look up from outside.
    fn link_free_names(&mut self, f: FnDef<'p>, node: Id) -> HashSet<String> {
        let params = f.params();
        let free = walk::free_names(&params, f.body);
        let mut outer = HashSet::new();
        for name in free {
            if self.peek(&name).is_none() && self.module_level && self.regions.len() == 1 {
                self.pending.push((node, name));
                continue;
            }
            if let Some(Bind::Src(_, st)) = self.peek(&name) {
                let st = *st;
                let mut g = Gather::default();
                for v in self.read(&name, &mut g) {
                    let v = Val { style: if st == Style::Def { Style::Def } else { Style::Ctx }, ..v };
                    self.link(&v, node);
                }
                if st != Style::Def {
                    outer.insert(name);
                }
            }
        }
        outer
    }

    /// A function written where it is used (a callback, a handler): its own
    /// node beside the code that uses it, named by where it is passed.
    fn callback_node(&mut self, f: &'p ast::FunctionDef, context: &str) -> Id {
        let def = FnDef::of(f);
        let name = if f.name.is_empty() { format!("{context} callback") } else { f.name.clone() };
        let node = self.cx.b.add(None, Kind::Function, Shape::Box, name.clone());
        let fid = self.cx.ix.register_nested_func(self.mi, def, None, node);
        self.cx.ix.funcs[fid].name = name;
        let container = self.container();
        attach(&mut self.cx.b, container, node);
        let line = self.src().line_of(usize::from(f.range.start()));
        self.describe_def(fid, f.range, line);
        if f.name.is_empty() {
            let n = self.cx.b.get_mut(node);
            n.title = format!("{context} callback");
        }
        let outer = self.link_free_names(def, node);
        let types = self.types.clone();
        build_function(self.cx, fid, outer, types);
        node
    }

    /// A call that takes one function draws it as its interior: zooming into
    /// `useEffect` or `router.get("/:id")` shows the callback's steps.
    fn host_callback(&mut self, node: Id, f: &'p ast::FunctionDef) {
        let def = FnDef::of(f);
        let fid = self.cx.ix.register_nested_func(self.mi, def, None, node);
        let outer = self.link_free_names(def, node);
        let types = self.types.clone();
        build_function(self.cx, fid, outer, types);
        let params = def.params();
        let n = self.cx.b.get_mut(node);
        if n.inputs.is_empty() {
            n.inputs = params;
        }
        if def.is_async && !n.badges.iter().any(|b| b == "async") {
            n.badges.push("async".into());
        }
    }

    fn describe_function(&mut self, fid: usize, s: &'p Stmt) {
        let line = stmt_start_line(self.src(), s);
        self.describe_def(fid, s.range(), line);
    }

    fn describe_def(&mut self, fid: usize, range: TextRange, line: u32) {
        let info = &self.cx.ix.funcs[fid];
        let class_prefix = info.class.and_then(|k| frameworks::controller_prefix(&self.cx.ix.classes[k].def.decorators));
        let def = info.def;
        let node = info.node;
        let kind = info.kind;
        let is_method = info.class.is_some() && kind != FnKind::StaticMethod;
        let mut params = def.params();
        if is_method && !params.is_empty() {
            params.remove(0);
        }
        let doc = def.docstring().map(summary).unwrap_or_else(|| self.src().prose_above(line));
        let span = self.src().span(range);
        let mut badges = vec![];
        let mi = self.mi;
        if !self.cx.prefixes.contains_key(&mi) {
            let p = frameworks::prefixes(&self.m.ast);
            self.cx.prefixes.insert(mi, p);
        }
        let src = self.src();
        let mut role = None;
        for d in def.decorators {
            let namer = Namer::plain(src);
            match frameworks::role(&d.expression, def.name, &self.cx.prefixes[&mi], class_prefix.as_deref(), |e| namer.code(e, 24)) {
                Some(r) if role.is_none() => role = Some(r),
                Some(_) => {}
                None => badges.extend(frameworks::tag(d, self.m.lang, |e| namer.code(e, 24))),
            }
        }
        // An object's functions are all its own: none is "static".
        let object = info.class.is_some_and(|k| self.cx.ix.classes[k].def.kind == ast::ClassKind::Object);
        match kind {
            FnKind::ClassMethod => badges.push("classmethod".into()),
            // Rust's associated functions (`new`) are not called static.
            FnKind::StaticMethod if !object && self.m.lang != Lang::Rust => badges.push("static".into()),
            FnKind::Property if !self.m.lang.is_js() => badges.push("property".into()),
            _ => {}
        }
        match def.kind {
            ast::DefKind::Getter if self.m.lang.is_js() => badges.push("getter".into()),
            ast::DefKind::Setter if self.m.lang.is_js() => badges.push("setter".into()),
            _ => {}
        }
        if def.is_async {
            badges.push("async".into());
        }
        if role.is_none() {
            role = self.cx.routes.handlers.get(&fid).map(|label| frameworks::Role { label: label.clone(), kind: Handler::Route });
        }
        let js = self.m.lang.is_js();
        // File-based routes: a Next.js `route.ts` exports one function per method.
        if role.is_none() && js && info.class.is_none() {
            role = frameworks::file_route(&self.m.path, def.name).map(|label| frameworks::Role { label, kind: Handler::Route });
        }
        // React: a capitalised function that returns JSX is a component; `useX` a hook.
        let component = js && def.name.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && returns_jsx(def.body);
        if js && def.name.len() > 3 && def.name.starts_with("use") && def.name[3..].starts_with(|c: char| c.is_ascii_uppercase()) {
            badges.push("hook".into());
        }
        if component && !self.cx.handlers[mi].contains(&Handler::Component) {
            self.cx.handlers[mi].push(Handler::Component);
        }
        if let Some(r) = &role {
            if !self.cx.handlers[mi].contains(&r.kind) {
                self.cx.handlers[mi].push(r.kind);
            }
        }
        // The trait a method implements, with its type arguments only where
        // the type implements that trait more than once (`From<io::Error>`).
        let implements = frameworks::implemented_trait(def.decorators, |e| squash(src.of(e))).map(|t| {
            let base = |t: &str| t.split('<').next().unwrap_or(t).to_string();
            let siblings = info.class.map(|k| self.cx.ix.classes[k].methods.iter().filter_map(|(_, f)| frameworks::implemented_trait(self.cx.ix.funcs[*f].def.decorators, |e| squash(src.of(e)))).filter(|o| base(o) == base(&t) && *o != t).count()).unwrap_or(0);
            if siblings > 0 { t } else { base(&t) }
        });
        let n = self.cx.b.get_mut(node);
        n.kind = Kind::Function;
        n.shape = Shape::Box;
        n.title = def.name.to_string();
        n.subtitle = match &role {
            Some(r) => r.label.clone(),
            // `__init__` is a constructor; `constructor` already says so.
            None if def.name == "__init__" => "constructor".into(),
            // `fmt` of `impl Display`: the trait says what it is for.
            None if implements.is_some() => implements.unwrap(),
            None => format!("({})", naming::list(&params, 40)),
        };
        n.category = if role.as_ref().is_some_and(|r| r.kind != Handler::Hook) {
            "endpoint"
        } else if component {
            "component"
        } else {
            "function"
        };
        n.doc = doc;
        n.span = Some(span);
        n.badges = badges;
        n.inputs = params;
    }

    fn class_stmt(&mut self, c: &'p ast::ClassDef) {
        let key = (self.mi, u32::from(c.range.start()));
        let Some(Sym::Class(cid)) = self.cx.ix.defs_at.get(&key).cloned() else { return };
        let node = self.cx.ix.classes[cid].node;
        let container = self.container();
        attach(&mut self.cx.b, container, node);
        // Uses of other definitions anywhere in the class body.
        let mut free: Vec<String> = vec![];
        walk::stmt_exprs(&c.body, true, &mut |e| {
            for n in walk::loads(e) {
                if !free.contains(&n) {
                    free.push(n);
                }
            }
        });
        // Bases, and what decorators wire in (`@Module({ controllers: [CatsController] })`).
        for e in class_bases(c).iter().chain(c.decorators.iter().map(|d| &d.expression)) {
            for n in walk::loads_deep(e) {
                if !free.contains(&n) {
                    free.push(n);
                }
            }
        }
        let mut outer = HashSet::new();
        for name in free {
            if name == c.name.as_str() {
                continue;
            }
            if self.peek(&name).is_none() && self.module_level && self.regions.len() == 1 {
                self.pending.push((node, name));
                continue;
            }
            if let Some(Bind::Src(_, st)) = self.peek(&name) {
                let st = *st;
                let mut g = Gather::default();
                for v in self.read(&name, &mut g) {
                    let v = Val { style: if st == Style::Def { Style::Def } else { Style::Ctx }, ..v };
                    self.link(&v, node);
                }
                if st != Style::Def {
                    outer.insert(name);
                }
            }
        }
        self.types.insert(c.name.to_string(), Ty::Class(cid));
        self.bind(&c.name, Bind::Src(vec![node], Style::Def));
        let types = self.types.clone();
        build_class(self.cx, cid, outer, types);
    }

    // ----- expressions -------------------------------------------------------------

    /// Does this value construct something (a sub-module, a parameter)?
    fn builds_component(&self, value: &Expr) -> bool {
        match value {
            Expr::Call(c) => self.is_major(&self.classify(c), c),
            Expr::If(x) => self.builds_component(&x.body) || self.builds_component(&x.orelse),
            _ => false,
        }
    }

    /// A database call by its receiver's name, in a module that works with a
    /// database, unless the receiver is an import from a library that is not
    /// one (Flask's cookie `session`).
    fn is_database(&self, a: &ast::ExprAttribute) -> bool {
        // An object defined in a module that uses a database library (a model:
        // `User.create(…)`) is the database's, whatever its name.
        if let Some(t) = root_name(&a.value).and_then(|r| self.origin(r)) {
            if self.cx.ix.externals[t].iter().any(|e| vocabulary::library_domains(e).database) && vocabulary::is_database_verb(a.attr.as_str()) {
                return true;
            }
        }
        // An object whose type is a database library's: `this.prisma.user.findUnique(…)`
        // where `prisma` is a service extending `PrismaClient`.
        if vocabulary::is_database_verb(a.attr.as_str()) {
            if let Some(p) = self.library_of(&a.value) {
                let root = self.library_root(&p);
                if vocabulary::library_domains(&root).database {
                    return true;
                }
            }
        }
        // `sqlx::query!(…).fetch_one(&pool)`: a query a database library built, run.
        if vocabulary::is_database_verb(a.attr.as_str()) {
            let mut cur: &Expr = &a.value;
            let root = loop {
                match cur {
                    Expr::Call(c) => cur = &c.func,
                    Expr::Await(x) => cur = &x.value,
                    Expr::Attribute(x) => match self.ty(cur) {
                        Ty::Ext(p) => break Some(p),
                        _ => cur = &x.value,
                    },
                    Expr::Name(_) => break match self.ty(cur) {
                        Ty::Ext(p) => Some(p),
                        _ => None,
                    },
                    _ => break None,
                }
            };
            if root.is_some_and(|p| matches!(self.m.lang, Lang::Rust | Lang::Go) && vocabulary::library_domains(&self.library_root(&p)).database) {
                return true;
            }
        }
        if !self.cx.domains[self.mi].database || !vocabulary::database_call(&squash(self.src().of(&*a.value)), a.attr.as_str()) {
            return false;
        }
        let Some(root) = root_name(&a.value) else { return true };
        match self.cx.ix.sym(self.mi, root) {
            Some(Sym::Ext(p)) => vocabulary::library_domains(&self.library_root(p)).database,
            _ => true,
        }
    }

    /// The library a dotted path is from, as `library_domains` names it:
    /// `prisma.user` → `prisma`, `@prisma/client.X` → `@prisma/client`,
    /// Go's `github.com/jmoiron/sqlx.DB` → `github.com/jmoiron/sqlx`.
    fn library_root(&self, p: &str) -> String {
        match self.m.lang {
            Lang::Go => {
                let pkg = crate::go::resolve::package_of(p);
                if crate::go::resolve::is_std(pkg) { pkg.to_string() } else { crate::go::resolve::module_path(pkg) }
            }
            Lang::Rust | Lang::Python => p.split('.').next().unwrap_or(p).to_string(),
            _ => crate::js::resolve::package_name(p.split('.').next().unwrap_or(p)).0.to_string(),
        }
    }

    /// The library an object comes from, by the type of the expression or of
    /// what it is a part of: an instance of an external class, or of a project
    /// class built on one.
    fn library_of(&self, e: &Expr) -> Option<String> {
        let mut cur = e;
        for _ in 0..8 {
            match self.ty(cur) {
                Ty::Inst(mut k) | Ty::Class(mut k) => {
                    for _ in 0..6 {
                        let bases = &self.cx.ix.classes[k].bases;
                        if let Some(Ty::Ext(p) | Ty::ExtInst(p)) = bases.iter().find(|b| matches!(b, Ty::Ext(_) | Ty::ExtInst(_))) {
                            return Some(p.clone());
                        }
                        match bases.iter().find_map(|b| if let Ty::Class(b) = b { Some(*b) } else { None }) {
                            Some(b) => k = b,
                            None => break,
                        }
                    }
                }
                // An instance, not a namespace: `func.count()` builds SQL, it does not run it.
                Ty::ExtInst(p) if !p.starts_with("globals.") && !p.starts_with("builtins.") => return Some(p),
                _ => {}
            }
            cur = match cur {
                Expr::Attribute(a) => &a.value,
                Expr::Subscript(x) => &x.value,
                _ => return None,
            };
        }
        None
    }

    /// The project module a name's value is defined in, following imports and
    /// re-exports.
    fn origin(&self, name: &str) -> Option<usize> {
        let mut sym = self.cx.ix.sym(self.mi, name)?.clone();
        for _ in 0..6 {
            match sym {
                Sym::Module(t) => return Some(t),
                Sym::Member(t, n) => match self.cx.ix.sym(t, &n) {
                    Some(Sym::Member(..) | Sym::Module(_)) => sym = self.cx.ix.sym(t, &n).unwrap().clone(),
                    _ => return Some(t),
                },
                _ => return None,
            }
        }
        None
    }

    /// `router.route("/:id").get(a).patch(b)` → `GET, PATCH /users/:id`.
    fn route_chain(&mut self, e: &Expr) -> Option<String> {
        let mut verbs = vec![];
        let mut cur = e;
        loop {
            let Expr::Call(c) = cur else { return None };
            let Expr::Attribute(a) = &*c.func else { return None };
            if a.attr == "route" {
                let path = c.arguments.args.first().and_then(string_literal)?;
                if verbs.is_empty() {
                    return None;
                }
                verbs.reverse();
                let mount = self.cx.mounts.get(&self.mi).cloned().unwrap_or_default();
                let full = if path == "/" && !mount.is_empty() { mount } else { frameworks::join_path(&mount, path) };
                if !self.cx.handlers[self.mi].contains(&Handler::Route) {
                    self.cx.handlers[self.mi].push(Handler::Route);
                }
                return Some(format!("{} {full}", verbs.join(", ")));
            }
            if !is_http_verb(&a.attr) {
                return None;
            }
            verbs.push(a.attr.to_uppercase());
            cur = &a.value;
        }
    }

    /// Fields of a schema built by a validation or ORM library: `z.object({…})`,
    /// `yup.object({…}).required()`, `new mongoose.Schema({…})`.
    fn schema_rows(&self, value: &Expr) -> Option<Vec<Row>> {
        let mut cur = value;
        // Modifiers after the object (`.strict()`, `.refine(…)`) keep it a schema.
        let dict = loop {
            let Expr::Call(c) = cur else { return None };
            let (base, attr) = match &*c.func {
                Expr::Attribute(a) => (Some(&*a.value), a.attr.as_str()),
                Expr::Name(n) => (None, n.id.as_str()),
                _ => return None,
            };
            if matches!(attr, "object" | "Object" | "strictObject" | "looseObject" | "Struct" | "Schema" | "type" | "interface") {
                if let Some(Expr::Dict(d)) = c.arguments.args.first() {
                    let root = base.and_then(root_name).or(match &*c.func {
                        Expr::Name(n) => Some(n.id.as_str()),
                        _ => None,
                    })?;
                    let lib = match self.cx.ix.sym(self.mi, root) {
                        Some(Sym::Ext(p)) => crate::js::resolve::package_name(p.split('.').next().unwrap_or(p)).0.to_string(),
                        _ => return None,
                    };
                    if !matches!(lib.as_str(), "zod" | "yup" | "joi" | "valibot" | "@sinclair/typebox" | "superstruct" | "effect" | "@effect/schema" | "io-ts" | "mongoose" | "arktype" | "@hapi/joi") {
                        return None;
                    }
                    break d;
                }
            }
            cur = base?;
        };
        let namer = Namer::plain(self.src());
        let rows: Vec<Row> = dict
            .items
            .iter()
            .filter_map(|i| {
                let key = i.key.as_ref().and_then(string_literal)?;
                Some(Row { key: key.to_string(), value: namer.code(&i.value, 40), note: String::new(), heading: false })
            })
            .collect();
        (!rows.is_empty()).then_some(rows)
    }

    /// Whether a call is passed a project function by name (`router.get("/", listUsers)`).
    fn passes_project_function(&self, c: &ast::ExprCall) -> bool {
        c.arguments.args.iter().any(|a| match a {
            Expr::Name(n) => matches!(self.peek(&n.id), Some(Bind::Src(_, Style::Def))) || matches!(self.cx.ix.sym(self.mi, &n.id), Some(Sym::Func(_))),
            // A method value: `r.GET("/", s.list)`.
            Expr::Attribute(_) => matches!(self.ty(a), Ty::Func(_)),
            _ => false,
        })
    }

    /// A call that registers a handler, named by what it handles: an HTTP
    /// route (`router.get("/:id", …)` → `GET /users/:id` with the router's
    /// mount path), a mount (`app.use("/api", router)`), or any callback keyed
    /// by a string (`it("returns the user", …)`, `on("message", …)`).
    fn registration(&mut self, c: &ast::ExprCall, func_text: &str, handler: bool) -> Option<(String, String, Option<&'static str>)> {
        let (recv, method) = match func_text.rsplit_once('.') {
            Some((r, m)) => (r, m),
            None => ("", func_text),
        };
        let key = c.arguments.args.first().and_then(string_literal)?;
        // Go routers: `r.GET("/x", h)` (gin, echo), `r.Get(…)` (chi, fiber),
        // `mux.HandleFunc("GET /items/{id}", h)` (net/http), each path already
        // under its group's.
        if self.m.lang == Lang::Go && handler && !recv.is_empty() && (key.starts_with('/') || key.contains(" /") || key.is_empty()) {
            let prefix = self.fid.and_then(|f| self.cx.routes.prefixes.get(&f)).cloned().unwrap_or_default();
            let full = |p: &str| {
                let f = frameworks::join_path(&prefix, p);
                if f.is_empty() { "/".to_string() } else { f }
            };
            let label = match key.split_once(' ') {
                Some((v, p)) if v.chars().all(|c| c.is_ascii_uppercase()) => Some(format!("{v} {}", full(p))),
                _ if is_http_verb(&method.to_ascii_lowercase()) => Some(format!("{} {}", method.to_ascii_uppercase(), full(key))),
                _ if matches!(method, "Any" | "Handle" | "HandleFunc" | "Method" | "MethodFunc") => Some(full(key)),
                _ => None,
            };
            if let Some(label) = label {
                if !self.cx.handlers[self.mi].contains(&Handler::Route) {
                    self.cx.handlers[self.mi].push(Handler::Route);
                }
                return Some((label, recv.to_string(), Some("endpoint")));
            }
        }
        let verb = is_http_verb(method) || method == "del";
        let path_like = key.starts_with('/') || key == "*";
        if verb && path_like && handler && !recv.is_empty() {
            let mount = self.cx.mounts.get(&self.mi).cloned().unwrap_or_default();
            // `router.get("/")` under `/api/users` serves `/api/users`.
            let path = if key == "/" && !mount.is_empty() { mount } else { frameworks::join_path(&mount, key) };
            let verb = if method == "del" { "DELETE".to_string() } else { method.to_uppercase() };
            if !self.cx.handlers[self.mi].contains(&Handler::Route) {
                self.cx.handlers[self.mi].push(Handler::Route);
            }
            return Some((format!("{verb} {path}"), recv.to_string(), Some("endpoint")));
        }
        if method == "use" && path_like && !recv.is_empty() {
            let mount = self.cx.mounts.get(&self.mi).cloned().unwrap_or_default();
            return Some((format!("mount {}", frameworks::join_path(&mount, key)), recv.to_string(), None));
        }
        if handler && !key.is_empty() {
            return Some((naming::shorten(&squash(key), naming::TITLE_MAX), method.to_string(), None));
        }
        None
    }

    /// The method's receiver as the code names it: `self`, `this`.
    fn receiver(&self) -> &str {
        self.this.as_ref().map(|t| t.0.as_str()).unwrap_or("self")
    }

    fn is_self(&self, e: &Expr) -> bool {
        matches!((e, &self.this), (Expr::Name(n), Some((this, _))) if n.id.as_str() == this)
    }

    fn ty(&self, e: &Expr) -> Ty {
        self.cx.ix.infer(self.mi, e, &TypesView(&self.types))
    }

    fn classify(&self, c: &ast::ExprCall) -> Callee {
        if is_super(c) {
            return Callee::Super;
        }
        match &*c.func {
            Expr::Name(n) => {
                let name = n.id.as_str();
                if name == "exec" || name == "eval" {
                    if let Some(t) = self.exec_target(c) {
                        return Callee::ExecFile(t);
                    }
                    return Callee::Builtin(name.into());
                }
                if let Some(t) = self.types.get(name) {
                    match t {
                        Ty::Inst(k) => return Callee::Instance { class: *k, var: name.into() },
                        Ty::Func(f) => return Callee::Func(*f),
                        Ty::Class(k) => return Callee::Ctor(*k),
                        Ty::Ext(p) if !p.starts_with("builtins.") => return Callee::Ext(p.clone()),
                        _ => {}
                    }
                }
                if let Some(Bind::Src(_, st)) = self.peek(name) {
                    if *st != Style::Def {
                        return Callee::Value(name.into());
                    }
                }
                // `const helper = require("./helper"); helper(x)`: calling a module
                // calls what it exports as itself (`module.exports = …`), through
                // any modules that re-export it.
                let mut ty = self.cx.ix.sym_ty(self.mi, name, 0);
                for _ in 0..4 {
                    match ty {
                        Ty::Module(m) if self.m.lang.is_js() => ty = self.cx.ix.sym_ty(m, "default", 0),
                        _ => break,
                    }
                }
                match ty {
                    Ty::Class(k) => Callee::Ctor(k),
                    Ty::Func(f) => Callee::Func(f),
                    Ty::Ext(p) if p.starts_with("builtins.") => Callee::Builtin(name.into()),
                    Ty::Ext(p) => Callee::Ext(p),
                    _ if is_builtin(name) => Callee::Builtin(name.into()),
                    _ => Callee::Unknown,
                }
            }
            Expr::Attribute(a) => {
                let attr = a.attr.as_str();
                // `self.wte.forward(idx)` (candle, burn) calls the component, as
                // `self.wte(idx)` does in PyTorch.
                if attr == "forward" && self.cx.domains[self.mi].ml {
                    if let Expr::Attribute(f) = &*a.value {
                        if let (true, Some((_, cid))) = (self.is_self(&f.value), self.this.clone()) {
                            if let Some(field) = self.cx.ix.field(cid, f.attr.as_str()).filter(|x| is_callable_ty(&x.ty)) {
                                return Callee::Component { name: f.attr.to_string(), ty: field.ty.clone(), of: Some((cid, f.attr.to_string())) };
                            }
                        }
                    }
                    if let (Ty::Inst(k), Expr::Name(n), false) = (self.ty(&a.value), &*a.value, self.is_self(&a.value)) {
                        return Callee::Instance { class: k, var: n.id.to_string() };
                    }
                }
                if self.is_self(&a.value) {
                    let (_, cid) = self.this.clone().unwrap();
                    if let Some(callee) = self.through_contract(cid, attr) {
                        return callee;
                    }
                    if let Some(f) = self.cx.ix.method(cid, attr) {
                        return Callee::Func(f);
                    }
                    if let Some(field) = self.cx.ix.field(cid, attr) {
                        if is_callable_ty(&field.ty) {
                            return Callee::Component { name: attr.into(), ty: field.ty.clone(), of: Some((cid, attr.into())) };
                        }
                    }
                    return Callee::Method(attr.into());
                }
                match self.ty(&a.value) {
                    // A library object's I/O method: `stream.write_all(…)` on a
                    // `TcpStream`, `session.get(url)` on a `requests.Session`.
                    Ty::ExtInst(p) if effects::io_of(&format!("{p}.{attr}"), c, self.src()).is_some() => Callee::Ext(format!("{p}.{attr}")),
                    Ty::Module(m) => match self.cx.ix.module_member(m, attr) {
                        Ty::Class(k) => Callee::Ctor(k),
                        Ty::Func(f) => Callee::Func(f),
                        Ty::Ext(p) => Callee::Ext(p),
                        _ => Callee::Unknown,
                    },
                    Ty::Ext(p) => Callee::Ext(format!("{p}.{attr}")),
                    Ty::Inst(k) => match (self.through_contract(k, attr), self.cx.ix.method(k, attr)) {
                        (Some(callee), _) => callee,
                        (None, Some(f)) => Callee::Func(f),
                        (None, None) => match self.cx.ix.field(k, attr) {
                            Some(field) if is_callable_ty(&field.ty) => Callee::Component { name: attr.into(), ty: field.ty.clone(), of: Some((k, attr.into())) },
                            _ => Callee::Method(attr.into()),
                        },
                    },
                    Ty::Class(k) => match self.cx.ix.method(k, attr) {
                        Some(f) => Callee::Func(f),
                        None => Callee::Method(attr.into()),
                    },
                    Ty::Dict(fields) => match fields.into_iter().find(|(k, _)| k == attr) {
                        // `self.transformer.wte(idx)`: a component declared in a group.
                        Some((_, t)) if is_callable_ty(&t) => {
                            let of = match (&*a.value, self.this.clone()) {
                                (Expr::Attribute(g), Some((_, cid))) if self.is_self(&g.value) => Some((cid, format!("{}.{attr}", g.attr))),
                                _ => None,
                            };
                            Callee::Component { name: attr.into(), ty: t, of }
                        }
                        _ => Callee::Method(attr.into()),
                    },
                    _ => Callee::Method(attr.into()),
                }
            }
            other => match self.ty(other) {
                Ty::Inst(k) => Callee::Instance { class: k, var: self.namer().code(other, 20) },
                Ty::Func(f) => Callee::Func(f),
                Ty::Class(k) => Callee::Ctor(k),
                _ => Callee::Unknown,
            },
        }
    }

    /// A call of a method a contract leaves to the types implementing it:
    /// into the one implementation, or naming the several.
    fn through_contract(&self, k: usize, name: &str) -> Option<Callee> {
        let found = self.cx.ix.implementations(k, name, self.mi)?;
        // Accessors everywhere (`w.Width()`): read as the value, as before.
        let accessors = !found.open && found.funcs.iter().all(|&f| self.cx.ix.is_accessor(f));
        match (&found.funcs[..], found.open) {
            _ if found.types.is_empty() || accessors => None,
            ([f], false) if !found.unsure => Some(Callee::Func(*f)),
            _ => Some(Callee::Dispatch { contract: found.contract, name: name.into(), types: found.types, partial: found.partial }),
        }
    }

    /// `exec(open('configurator.py').read())` → the project module it runs.
    fn exec_target(&self, c: &ast::ExprCall) -> Option<usize> {
        let mut found = None;
        for a in &c.arguments.args {
            walk::walk(a, &mut |e| {
                if let Some(s) = string_literal(e) {
                    if s.ends_with(".py") {
                        let dir = self.m.path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                        let candidates = [s.trim_start_matches("./").to_string(), if dir.is_empty() { s.to_string() } else { format!("{dir}/{s}") }];
                        for cand in candidates {
                            if let Some(&m) = self.cx.module_by_path.get(&cand) {
                                found = Some(m);
                            }
                        }
                    }
                }
            });
        }
        found
    }

    fn is_major(&self, callee: &Callee, c: &ast::ExprCall) -> bool {
        // A call that is handed a function to run later (a route, a listener,
        // an effect) is a step of its own: it holds that function.
        if !matches!(callee, Callee::Super) && (has_inline_function(c) || (self.passes_project_function(c) && c.arguments.args.first().and_then(string_literal).is_some())) {
            return true;
        }
        // A library element given no data (`<Plus className="size-4" />`) is decoration.
        if c.kind == ast::CallKind::Jsx && matches!(callee, Callee::Ext(_)) && c.arguments.args.iter().chain(c.arguments.keywords.iter().map(|k| &k.value)).all(is_literal) {
            return false;
        }
        match callee {
            Callee::Func(_) | Callee::Dispatch { .. } | Callee::Ctor(_) | Callee::Instance { .. } | Callee::Component { .. } | Callee::Value(_) | Callee::ExecFile(_) => true,
            Callee::Ext(p) => !MINOR_EXT.iter().any(|m| p.starts_with(m) || p == m.trim_end_matches('.')) || effects::io_of(p, c, self.src()).is_some(),
            Callee::Builtin(b) => matches!(b.as_str(), "open" | "input" | "exec" | "eval"),
            Callee::Method(m) => self.m.lang == Lang::Python && effects::method_io(m).is_some()
                // `x.gelu()`, `att.softmax(…)` in a network: a layer of its own.
                || self.cx.domains[self.mi].ml && matches!(vocabulary::ml_category(m), Some("activation" | "dropout")),
            Callee::Super | Callee::Unknown => false,
        }
    }

    /// Calls that read a property of their inputs (`len(x)`, `x.size()`): the
    /// result is metadata and the inputs are used only for that property.
    /// Conversions and arithmetic (`int`, `round`, `sum`) are not: their
    /// results are metadata only when their inputs are.
    fn metadata(callee: &Callee) -> bool {
        let name = match callee {
            Callee::Ext(p) => p.rsplit('.').next().unwrap_or(p),
            Callee::Builtin(b) | Callee::Method(b) => b.as_str(),
            _ => return false,
        };
        vocabulary::is_metadata(name)
    }

    /// Collects the dataflow sources of an expression, creating blocks for major calls.
    fn collect(&mut self, e: &'p Expr, g: &mut Gather, fold: bool) -> Vec<Val> {
        match e {
            Expr::Name(n) => {
                if n.ctx == Load::Load {
                    self.read(&n.id, g)
                } else {
                    vec![]
                }
            }
            e if is_constant(e) => vec![],
            Expr::Attribute(a) => {
                if self.is_self(&a.value) {
                    return vec![];
                }
                let text = self.src().of(e);
                if text == "os.environ" || text.starts_with("os.environ.") || text == "process.env" || text.starts_with("process.env.") || text.starts_with("import.meta.env") {
                    g.effects.insert("reads environment".into());
                }
                if text == "sys.argv" {
                    g.effects.insert("reads CLI args".into());
                }
                let mut vals = self.collect(&a.value, g, fold);
                // `x.shape`, `x.device`: a property, not the value itself.
                if vocabulary::is_metadata_attr(a.attr.as_str()) {
                    for v in &mut vals {
                        v.scalar = true;
                    }
                }
                vals
            }
            Expr::Call(c) => {
                let callee = self.classify(c);
                // `fetch(url).then(r => r.json()).catch(log)`: a promise chain
                // passes each result on, the way `await` does; it is not a step.
                // Each callback receives the value the promise settles with.
                if let (true, Callee::Method(m), Expr::Attribute(a)) = (self.m.lang.is_js(), &callee, &*c.func) {
                    if matches!(m.as_str(), "then" | "catch" | "finally") && !has_inline_function(c) {
                        let mut settled = self.collect(&a.value, g, fold);
                        dedup_vals(&mut settled);
                        let mut results = vec![];
                        for (i, arg) in c.arguments.args.iter().enumerate() {
                            // `catch` and `then`'s second callback receive the error instead.
                            self.settled = (m == "then" && i == 0 && matches!(arg, Expr::Lambda(_))).then(|| settled.clone());
                            results.extend(self.collect(arg, g, fold));
                            self.settled = None;
                        }
                        return match m.as_str() {
                            "then" if !results.is_empty() => results,
                            // A handled failure produces the handler's value instead.
                            "catch" => settled.into_iter().chain(results).collect(),
                            _ => settled,
                        };
                    }
                }
                if !fold && self.is_major(&callee, c) {
                    let id = self.call_node(e, &callee, g);
                    return vec![Val { srcs: vec![id], label: String::new(), style: Style::Data, scalar: Self::metadata(&callee) }];
                }
                if let Callee::Ext(p) = &callee {
                    if let Some((eff, _)) = effects::io_of(p, c, self.src()) {
                        g.effects.insert(eff);
                    }
                    if let Some(e) = effects::ext_effect(p) {
                        g.effects.insert(e.into());
                    }
                }
                if let Callee::Builtin(b) = &callee {
                    if b == "print" {
                        g.effects.insert("prints".into());
                    }
                    if b == "exec" || b == "eval" {
                        g.effects.insert("executes code".into());
                    }
                }
                if let Callee::Super = callee {
                    return vec![];
                }
                if let Expr::Attribute(a) = &*c.func {
                    if !matches!(callee, Callee::Instance { .. } | Callee::Func(_) | Callee::Dispatch { .. } | Callee::Ctor(_)) && self.is_database(a) {
                        g.effects.insert("database".into());
                    }
                }
                let name = callee_name(&callee, self);
                let mut out = vec![];
                if let Expr::Attribute(a) = &*c.func {
                    out.extend(self.collect(&a.value, g, fold));
                } else if let Callee::Value(v) = &callee {
                    out.extend(self.read(v, g));
                }
                let each = std::mem::replace(&mut self.each, iterates(&c.func));
                for a in &c.arguments.args {
                    match a {
                        // `promise.then(res => {…})`: the callback is named by the call.
                        Expr::Function(f) => {
                            let context = if name.is_empty() { "function".to_string() } else { name.clone() };
                            let node = self.callback_node(f, &context);
                            out.push(Val { srcs: vec![node], label: String::new(), style: Style::Def, scalar: false });
                        }
                        a => out.extend(self.collect(a, g, fold)),
                    }
                }
                self.each = each;
                if !name.is_empty() {
                    g.minor_calls.push(name);
                }
                let dict_like = matches!(&callee, Callee::Builtin(b) if b == "dict");
                for k in &c.arguments.keywords {
                    self.event = k.arg.as_deref().and_then(event).filter(|_| matches!(k.value, Expr::Lambda(_)));
                    let vs = self.collect(&k.value, g, fold);
                    self.event = None;
                    if dict_like {
                        self.name_by_key(k.arg.as_deref(), &k.value, &vs);
                    }
                    out.extend(vs);
                }
                if Self::metadata(&callee) {
                    for v in &mut out {
                        v.scalar = true;
                    }
                }
                out
            }
            // A lambda or comprehension is glue around what it calls: its own
            // names are local, and calls into project code or libraries are
            // blocks (`users.map(u => <UserCard user={u} />)`), done for each item.
            Expr::Lambda(_) | Expr::Comp(_) => {
                let mut locals: Vec<String> = vec![];
                // Evaluated once (defaults, iterables), then per item.
                let mut once: Vec<&'p Expr> = vec![];
                let mut parts: Vec<&'p Expr> = vec![];
                match e {
                    Expr::Lambda(l) => {
                        locals.extend(l.params.iter().map(|p| p.name.clone()));
                        once.extend(l.params.iter().filter_map(|p| p.default.as_ref()));
                        parts.push(&l.body);
                    }
                    Expr::Comp(x) => {
                        for gen in &x.generators {
                            walk::target_names(&gen.target, &mut locals);
                        }
                        once.extend(x.generators.iter().map(|gen| &gen.iter));
                        parts.push(&x.elt);
                        parts.extend(x.value.as_deref());
                        parts.extend(x.generators.iter().flat_map(|gen| gen.ifs.iter()));
                    }
                    _ => unreachable!(),
                }
                let each = std::mem::replace(&mut self.each, false);
                let event = if matches!(e, Expr::Lambda(_)) { self.event.take() } else { None };
                let settled = if matches!(e, Expr::Lambda(_)) { self.settled.take() } else { None };
                // Glue inside (`k.startswith("_")`) does not make the statement a step.
                let minor = g.minor_calls.len();
                let mut out = vec![];
                // Defaults and the iterable are read outside the lambda's scope.
                for p in once {
                    out.extend(self.collect(p, g, fold));
                }
                let saved: Vec<(String, Option<Bind>)> = locals.iter().map(|n| (n.clone(), self.regions.last().unwrap().env.get(n).cloned())).collect();
                for n in &locals {
                    self.regions.last_mut().unwrap().env.insert(n.clone(), Bind::Static);
                }
                if let (Some(vals), Expr::Lambda(l)) = (settled, e) {
                    let srcs: Vec<Id> = vals.iter().flat_map(|v| v.srcs.iter().copied()).collect();
                    if let (Some(p), false) = (l.params.first(), srcs.is_empty()) {
                        self.regions.last_mut().unwrap().env.insert(p.name.clone(), Bind::Src(srcs, Style::Data));
                    }
                }
                let first = self.cx.b.nodes.len() as Id;
                for p in parts {
                    out.extend(self.collect(p, g, fold));
                }
                g.minor_calls.truncate(minor);
                self.each = each;
                for (n, b) in saved {
                    let env = &mut self.regions.last_mut().unwrap().env;
                    match b {
                        Some(b) => env.insert(n, b),
                        None => env.remove(&n),
                    };
                }
                // Blocks inside run per item, or when an event happens; a
                // handler inside an iteration still runs on its event.
                let badge = match event {
                    Some(ev) => Some(ev),
                    None if each || matches!(e, Expr::Comp(_)) => Some("each".to_string()),
                    None => None,
                };
                if let Some(badge) = badge {
                    let container = self.container();
                    for id in first..self.cx.b.nodes.len() as Id {
                        let n = self.cx.b.get_mut(id);
                        if n.parent == Some(container) && !n.is_port() && !n.badges.iter().any(|b| b == "each" || b.starts_with("on ")) {
                            n.badges.push(badge.clone());
                        }
                    }
                }
                out
            }
            // A function written as a value (an event handler, an option) is a
            // definition of its own, used by whatever receives it.
            Expr::Function(f) => {
                let name = if f.name.is_empty() { "function".to_string() } else { f.name.clone() };
                let node = self.callback_node(f, &name);
                vec![Val { srcs: vec![node], label: String::new(), style: Style::Def, scalar: false }]
            }
            Expr::Named(n) => {
                let vals = self.collect(&n.value, g, fold);
                // `(m := f())`, and Rust's `if let Some((a, b)) = f()`.
                let mut names = vec![];
                walk::target_names(&n.target, &mut names);
                if let Some(v) = vals.first() {
                    let st = v.style;
                    let srcs: Vec<Id> = vals.iter().flat_map(|v| v.srcs.clone()).collect();
                    for t in names {
                        self.bind(&t, Bind::Src(srcs.clone(), st));
                    }
                }
                vals
            }
            Expr::Await(a) => {
                g.effects.insert("awaits".into());
                self.collect(&a.value, g, fold)
            }
            // Inside a larger expression only additions earn a glyph (residuals,
            // positional encodings); other arithmetic folds into the statement.
            Expr::BinOp(b) if !fold && matches!(b.op, ast::Operator::Add) => {
                let mut left = self.collect(&b.left, g, fold);
                let mut right = self.collect(&b.right, g, fold);
                dedup_vals(&mut left);
                dedup_vals(&mut right);
                let rich = |v: &Vec<Val>| v.len() == 1 && !v[0].scalar && v[0].style == Style::Data;
                if rich(&left) && rich(&right) {
                    let id = self.add(Kind::Glyph, Shape::Glyph, op_symbol(b.op));
                    let span = self.src().span(e.range());
                    let code = compact(self.src().of(e), 160);
                    let n = self.cx.b.get_mut(id);
                    n.category = "op";
                    n.code = code;
                    n.span = Some(span);
                    for v in left.iter().chain(&right) {
                        self.link(v, id);
                    }
                    return vec![Val { srcs: vec![id], label: String::new(), style: Style::Data, scalar: false }];
                }
                left.extend(right);
                left
            }
            Expr::Dict(d) => {
                let mut out = vec![];
                for item in &d.items {
                    if let Some(k) = &item.key {
                        out.extend(self.collect(k, g, fold));
                    }
                    let vs = self.collect(&item.value, g, fold);
                    self.name_by_key(item.key.as_ref().and_then(string_literal), &item.value, &vs);
                    out.extend(vs);
                }
                out
            }
            _ => {
                let mut kids = vec![];
                walk::children(e, &mut kids);
                let mut out = vec![];
                for k in kids {
                    out.extend(self.collect(k, g, fold));
                }
                out
            }
        }
    }

    fn call_node(&mut self, call_expr: &'p Expr, callee: &Callee, g: &mut Gather) -> Id {
        let Expr::Call(c) = call_expr else { unreachable!("call_node needs a call") };
        // Inputs first so nested calls are created (and ordered) before this one.
        let mut vals = vec![];
        if let Expr::Attribute(a) = &*c.func {
            if !matches!(callee, Callee::Component { .. }) {
                vals.extend(self.collect(&a.value, g, false));
            }
        } else if let Callee::Value(v) = callee {
            vals.extend(self.read(v, g));
        } else if let Callee::Instance { var, .. } = callee {
            if !self.is_self(&c.func) {
                vals.extend(self.read(var, g));
            }
        }
        let mut node_effects: BTreeSet<String> = BTreeSet::new();
        let mut inner = Gather::default();
        let exec_file = matches!(callee, Callee::ExecFile(_));
        let container_ctor = matches!(callee, Callee::Ext(p) if p.ends_with("Dict"));
        // Functions passed in are drawn once the call's block exists.
        let callbacks: Vec<&'p ast::FunctionDef> = c.arguments.args.iter().chain(c.arguments.keywords.iter().map(|k| &k.value)).filter_map(|a| if let Expr::Function(f) = a { Some(&**f) } else { None }).collect();
        let each = std::mem::replace(&mut self.each, iterates(&c.func));
        for a in c.arguments.args.iter().filter(|a| !matches!(a, Expr::Function(_))) {
            vals.extend(self.collect(a, &mut inner, exec_file));
        }
        for k in c.arguments.keywords.iter().filter(|k| !matches!(k.value, Expr::Function(_))) {
            self.event = k.arg.as_deref().and_then(event).filter(|_| matches!(k.value, Expr::Lambda(_)));
            let vs = self.collect(&k.value, &mut inner, exec_file);
            self.event = None;
            if container_ctor {
                self.name_by_key(k.arg.as_deref(), &k.value, &vs);
            }
            vals.extend(vs);
        }
        self.each = each;
        node_effects.extend(inner.effects.iter().cloned());
        g.reads.extend(inner.reads.iter().cloned());
        g.writes.extend(inner.writes.iter().cloned());
        dedup_vals(&mut vals);

        let src = self.src();
        let func_text = squash(src.of(&*c.func));
        let mut link: Option<Id> = None;
        let (title, subtitle, category, reference): (String, String, &'static str, Option<Id>) = match callee {
            Callee::Func(f) => {
                let info = &self.cx.ix.funcs[*f];
                let owner = info.class.map(|k| self.cx.ix.classes[k].name.clone()).unwrap_or_default();
                let cat = self.cx.category(self.mi, &[&info.name]).unwrap_or("function");
                // `Connection::new(socket)` makes a Connection.
                if info.kind == FnKind::StaticMethod && is_constructor_name(&info.name) && !owner.is_empty() {
                    (owner, info.name.clone(), self.cx.category(self.mi, &[&info.name]).unwrap_or("class"), Some(info.node))
                } else {
                    (info.name.clone(), owner, cat, Some(info.node))
                }
            }
            // `a.Sound()` on an Animal: the types it can be, and the contract.
            Callee::Dispatch { contract, name, types, partial } => {
                let names: Vec<&str> = types.iter().map(|&k| self.cx.ix.classes[k].name.as_str()).collect();
                let distinct = names.iter().enumerate().all(|(i, n)| !names[..i].contains(n));
                // A few are named; more, or two of one name, are the contract they share.
                let sub = match &names[..] {
                    _ if !distinct || *partial => self.cx.ix.classes[*contract].name.clone(),
                    [a] => a.to_string(),
                    [a, b] => format!("{a} or {b}"),
                    [a, b, c] if a.len() + b.len() + c.len() < 30 => format!("{a}, {b} or {c}"),
                    _ => self.cx.ix.classes[*contract].name.clone(),
                };
                let reference = self.cx.ix.method(*contract, name).map(|f| self.cx.ix.funcs[f].node).unwrap_or(self.cx.ix.classes[*contract].node);
                (name.clone(), sub, self.cx.category(self.mi, &[name]).unwrap_or("function"), Some(reference))
            }
            Callee::Ctor(k) => {
                let cls = &self.cx.ix.classes[*k];
                link = Some(cls.node);
                (cls.name.clone(), "new instance".into(), self.cx.category(self.mi, &[&cls.name]).unwrap_or("class"), None)
            }
            Callee::Instance { class, var } => {
                let cls = &self.cx.ix.classes[*class];
                let target = self.cx.ix.call_target(*class);
                let reference = target.map(|f| self.cx.ix.funcs[f].node).or(Some(cls.node));
                let (title, sub) = if self.is_self(&c.func) {
                    (target.map(|f| self.cx.ix.funcs[f].name.clone()).unwrap_or_else(|| "self".into()), "self".to_string())
                } else {
                    (var.clone(), cls.name.clone())
                };
                (title, sub, self.cx.category(self.mi, &[&cls.name, var]).unwrap_or("component"), reference)
            }
            Callee::Component { name, ty, .. } => {
                let display = self.cx.ix.display(ty);
                let reference = match ty {
                    Ty::Inst(k) => self.cx.ix.call_target(*k).map(|f| self.cx.ix.funcs[f].node).or(Some(self.cx.ix.classes[*k].node)),
                    _ => None,
                };
                let project = matches!(ty, Ty::Inst(_));
                let cat = self.cx.category(self.mi, &[&display, name]).unwrap_or(if project { "component" } else { "library" });
                (name.clone(), display, cat, reference)
            }
            Callee::Ext(p) => {
                let resolved = p.rsplit('.').next().unwrap_or(p).to_string();
                // An application is created by a module's own code or by a factory
                // it defines (`create_app()`), not by a library's methods
                // (express's `app.listen` calling `http.createServer`).
                if self.top && ((frameworks::APPS.contains(&resolved.as_str()) && !resolved.is_empty()) || p.ends_with("NestFactory.create")) {
                    self.cx.app[self.mi] = true;
                }
                let split = match func_text.rfind("::").max(func_text.rfind('.')) {
                    Some(i) => Some((&func_text[..i], func_text[i..].trim_start_matches([':', '.']))),
                    None => None,
                };
                let (prefix, last) = match split {
                    // `<Dialog.Root>`: a namespaced element is named by its whole tag.
                    Some(_) if c.kind == ast::CallKind::Jsx => (String::new(), func_text.clone()),
                    // `Semaphore::new(n)` makes a Semaphore.
                    Some((a, b)) if is_constructor_name(b) && self.m.lang == Lang::Rust => (b.to_string(), a.rsplit("::").next().unwrap_or(a).to_string()),
                    Some((a, b)) => (a.to_string(), b.trim_end_matches('!').to_string()),
                    None => (String::new(), func_text.trim_end_matches('!').to_string()),
                };
                let cat = if effects::io_of(p, c, src).is_some() || effects::ext_effect(p).is_some_and(|e| e.contains("network") || e.contains("file")) {
                    "io"
                } else {
                    self.cx.category(self.mi, &[&last, &resolved]).unwrap_or("library")
                };
                let mut sub = if prefix.is_empty() && !last.ends_with(&format!(".{resolved}")) && resolved != last { resolved.clone() } else { prefix };
                // Containers of project modules say what they hold.
                if let t @ (Ty::List(..) | Ty::Dict(_)) = self.ty(call_expr) {
                    if let Ty::List(e, _) = &t {
                        if let Ty::Inst(k) = &**e {
                            link = Some(self.cx.ix.classes[*k].node);
                        }
                    }
                    let shown = self.cx.ix.display(&t);
                    if !shown.is_empty() && shown != "dict" {
                        sub = shown;
                    }
                }
                let sub = if sub.chars().count() <= 28 { sub } else if sub.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') { naming::qualifier(&sub, 28) } else { naming::shorten(&sub, 28) };
                // A command-line command built by a library (cobra's and
                // urfave/cli's `Command{Use: "serve", …}`) is named by its name.
                let named = (last == "Command").then(|| c.arguments.keywords.iter().find(|k| matches!(k.arg.as_deref(), Some("Use" | "Name"))).and_then(|k| string_literal(&k.value))).flatten();
                if let Some(cmd) = named.and_then(|u| u.split_whitespace().next()).map(str::to_string) {
                    if !self.cx.handlers[self.mi].contains(&Handler::Command) {
                        self.cx.handlers[self.mi].push(Handler::Command);
                    }
                    (cmd, "command".to_string(), "endpoint", None)
                } else {
                    (last, sub, cat, None)
                }
            }
            // `open(path, mode)`: the arguments tell two calls of a builtin apart.
            Callee::Builtin(b) => (b.clone(), Namer::plain(src).args(c, 34), if b == "open" || b == "input" { "io" } else { "op" }, None),
            Callee::Value(v) => (v.clone(), String::new(), "function", None),
            Callee::Method(m) => {
                let recv = match &*c.func {
                    Expr::Attribute(a) => Namer::plain(src).code(&a.value, 20),
                    _ => String::new(),
                };
                let cat = if self.m.lang == Lang::Python && effects::method_io(m).is_some() { "io" } else { self.cx.category(self.mi, &[m]).unwrap_or("op") };
                (m.trim_end_matches('!').to_string(), recv, cat, None)
            }
            Callee::ExecFile(m) => {
                let path = self.cx.ix.modules[*m].path.clone();
                self.cx.relations.push(Relation::Exec { from: self.mi, to: *m });
                node_effects.insert("executes code".into());
                (path.rsplit('/').next().unwrap_or(&path).to_string(), "exec".into(), "config", Some(self.cx.file_nodes[*m]))
            }
            // A macro is named without its `!`.
            Callee::Super | Callee::Unknown => (func_text.trim_end_matches('!').to_string(), String::new(), "op", None),
        };
        // I/O: effects and the files touched.
        let mut files: Vec<String> = vec![];
        for a in c.arguments.args.iter().chain(c.arguments.keywords.iter().map(|k| &k.value)).filter(|_| !exec_file) {
            walk::walk(a, &mut |x| {
                if let Some(s) = string_literal(x).and_then(effects::file_name) {
                    files.push(s);
                }
                if let Expr::Template(j) = x {
                    if let Some(p) = effects::fstring_file(j) {
                        files.push(p);
                    }
                }
                if let Expr::Name(n) = x {
                    if let Some(f) = self.files.get(n.id.as_str()) {
                        files.extend(f.iter().cloned());
                    }
                }
            });
        }
        // A file handle's reads and writes touch the file it was opened on.
        if let (Expr::Attribute(a), Callee::Method(m)) = (&*c.func, callee) {
            if matches!(m.as_str(), "read" | "readlines" | "readline" | "write" | "writelines" | "flush") {
                if let Some(root) = root_name(&a.value) {
                    if let Some(f) = self.files.get(root) {
                        files.extend(f.iter().cloned());
                    }
                }
            }
        }
        files.sort();
        files.dedup();
        let io = match callee {
            Callee::Ext(p) => effects::io_of(p, c, src).map(|(_, io)| io),
            Callee::Builtin(b) if b == "open" => Some(effects::open_mode(c)),
            // File handles: Python's `f.write(…)`. JavaScript does file I/O through `fs`.
            Callee::Method(m) if self.m.lang == Lang::Python => effects::method_io(m),
            _ => None,
        };
        if let Some(io) = io {
            let what = if files.is_empty() { String::new() } else { format!(" {}", files.join(", ")) };
            match io {
                Io::Read => {
                    node_effects.insert(format!("reads file{what}"));
                    g.reads.extend(files.iter().cloned());
                }
                Io::Write => {
                    node_effects.insert(format!("writes file{what}"));
                    g.writes.extend(files.iter().cloned());
                }
                Io::Network => {
                    node_effects.insert("network".into());
                }
            }
        }
        if let Callee::Ext(p) = callee {
            if let Some(e) = effects::ext_effect(p) {
                node_effects.insert(e.into());
            }
        }
        // A call into a project method is drawn as that call, whatever the receiver is named.
        let project = matches!(callee, Callee::Instance { .. } | Callee::Func(_) | Callee::Dispatch { .. } | Callee::Ctor(_));
        let database = !project && matches!(&*c.func, Expr::Attribute(a) if self.is_database(a));
        let last = func_text.rsplit('.').next().unwrap_or(&func_text);
        if matches!(last, "render_template" | "render" | "TemplateResponse" | "render_to_response" | "render_to_string" | "get_template" | "select_template" | "render_template_string") {
            for a in c.arguments.args.iter().chain(c.arguments.keywords.iter().map(|k| &k.value)) {
                if let Some(t) = string_literal(a).filter(|t| t.contains('.') && !t.contains(char::is_whitespace)) {
                    self.cx.relations.push(Relation::Template { module: self.mi, name: t.to_string() });
                }
            }
        }
        if database {
            node_effects.insert("database".into());
        }
        if let Callee::Builtin(b) = callee {
            if b == "exec" || b == "eval" {
                node_effects.insert("executes code".into());
            }
            if b == "input" {
                node_effects.insert("reads stdin".into());
            }
        }

        // A call that registers a handler is named by what it handles:
        // `GET /users/:id`, `click`, `returns the user`.
        let handler = !callbacks.is_empty() || c.arguments.args.iter().any(|a| matches!(a, Expr::Lambda(_))) || self.passes_project_function(c);
        let (title, subtitle, category) = match self.registration(c, &func_text, handler) {
            Some((t, s, cat)) => (t, s, cat.unwrap_or(category)),
            None => (title, subtitle, category),
        };
        let id = self.add(Kind::Call, Shape::Box, title.clone());
        {
            let span = src.span(c.range());
            let n = self.cx.b.get_mut(id);
            n.subtitle = subtitle;
            n.category = if database && matches!(category, "op" | "library") { "io" } else { category };
            n.reference = reference;
            n.link = link.or(reference);
            n.code = compact(src.of(c), 200);
            n.span = Some(span);
            n.effects = node_effects.into_iter().collect();
        }
        if let Callee::Component { of: Some((k, path)), .. } = callee {
            self.cx.component_calls.push((*k, path.clone(), id));
        }
        for v in &vals {
            self.link(v, id);
        }
        match callbacks.as_slice() {
            [f] if self.cx.b.get(id).reference.is_none() => self.host_callback(id, f),
            fs => {
                for f in fs {
                    let node = self.callback_node(f, &title);
                    self.connect(node, id, EdgeKind::Uses, "", None, None);
                }
            }
        }
        if let Callee::ExecFile(_) = callee {
            // The executed file may rebind every module-level setting.
            let names: Vec<String> = self.regions.last().unwrap().env.iter().filter(|(_, b)| matches!(b, Bind::Src(_, Style::Ctx))).map(|(k, _)| k.clone()).collect();
            let mut all: Vec<String> = vec![];
            for r in &self.regions {
                for (k, b) in &r.env {
                    if matches!(b, Bind::Src(_, Style::Ctx)) && !all.contains(k) {
                        all.push(k.clone());
                    }
                }
            }
            let _ = names;
            if !all.is_empty() {
                let mut g2 = Gather::default();
                let mut sources = vec![];
                for k in &all {
                    sources.extend(self.read(k, &mut g2));
                }
                let mut seen = HashSet::new();
                for v in sources {
                    for s in v.srcs {
                        if seen.insert(s) {
                            self.connect(s, id, EdgeKind::Data, "globals", None, None);
                        }
                    }
                }
                all.sort();
                for k in &all {
                    self.bind(k, Bind::Src(vec![id], Style::Ctx));
                }
                self.cx.b.get_mut(id).outputs = all;
            }
        }
        id
    }

    /// Builds the value of an expression: one block for the statement unless it
    /// is a plain alias or a single major call.
    fn value(&mut self, e: &'p Expr, g: &mut Gather, dest: &Dest) -> Outcome {
        match e {
            // `return (req, res, next) => {…}`: a function that makes a function
            // produces that function, drawn as a definition of its own.
            Expr::Function(f) => {
                let id = self.callback_node(f, "returned");
                if f.name.is_empty() {
                    let title = match dest {
                        Dest::Names(ns) => naming::list(ns, naming::TITLE_MAX),
                        _ => "returned function".to_string(),
                    };
                    self.cx.b.get_mut(id).title = title;
                }
                return Outcome { node: Some(id), ..Outcome::none() };
            }
            Expr::Name(n) => {
                let vals = self.read(&n.id, g);
                return Outcome { alias: vals.into_iter().next(), ..Outcome::none() };
            }
            Expr::Attribute(_) | Expr::Subscript(_) if root_name(e).is_some() && !contains_call(e) => {
                let mut vals = self.collect(e, g, false);
                dedup_vals(&mut vals);
                if vals.len() == 1 {
                    let mut v = vals.remove(0);
                    v.scalar = is_metadata_read(e);
                    return Outcome { alias: Some(v), ..Outcome::none() };
                }
                // `current_app.config["KEY"]`, `process.env.PORT`: a lookup into
                // state the diagram does not otherwise show is still where the
                // value comes from.
                let lookup = matches!(e, Expr::Subscript(_)) || g.effects.contains("reads environment");
                if vals.is_empty() && !(lookup && matches!(dest, Dest::Names(_))) {
                    return Outcome::none();
                }
                let id = self.fused(e, vals, dest);
                return Outcome { node: Some(id), ..Outcome::none() };
            }
            e if is_constant(e) => return Outcome::none(),
            Expr::Await(a) => {
                g.effects.insert("awaits".into());
                return self.value(&a.value, g, dest);
            }
            // `X(...) if cond else None`: the block exists conditionally.
            Expr::If(x) => {
                let simple = |e: &Expr| is_literal(e) || (root_name(e).is_some() && !contains_call(e));
                if simple(&x.body) && simple(&x.orelse) && !is_literal(&x.body) && root_name(&x.body) == root_name(&x.orelse) {
                    let mut vals = self.collect(&x.body, g, false);
                    dedup_vals(&mut vals);
                    if vals.len() == 1 {
                        return Outcome { alias: vals.pop(), ..Outcome::none() };
                    }
                }
                if let (Expr::Call(a), Expr::Call(b)) = (&*x.body, &*x.orelse) {
                    let mut vals = self.collect(e, g, true);
                    dedup_vals(&mut vals);
                    let name = |c: &ast::ExprCall| squash(self.src().of(&*c.func)).rsplit('.').next().unwrap_or("").to_string();
                    let label = format!("{} / {}", name(a), name(b));
                    let id = self.fused(e, vals, dest);
                    let n = self.cx.b.get_mut(id);
                    if matches!(dest, Dest::Names(_) | Dest::Field { .. }) {
                        n.subtitle = label;
                    } else {
                        n.title = label;
                        n.subtitle.clear();
                    }
                    n.category = "library";
                    return Outcome { node: Some(id), ..Outcome::none() };
                }
                let (call, other, negate) = if simple(&x.orelse) { (&*x.body, &*x.orelse, false) } else { (&*x.orelse, &*x.body, true) };
                if simple(other) {
                    if let Expr::Call(c) = call {
                        let callee = self.classify(c);
                        if self.is_major(&callee, c) {
                            let cond = self.condition_vals(&x.test);
                            let id = self.call_node(call, &callee, g);
                            let test = self.namer().code(&x.test, 28);
                            let badge = if negate { format!("unless {test}") } else { format!("if {test}") };
                            self.cx.b.get_mut(id).badges.push(badge);
                            for v in &cond {
                                self.link_control(v, id);
                            }
                            // The fallback value flows through when the condition fails.
                            let mut fallback = self.collect(other, g, false);
                            dedup_vals(&mut fallback);
                            for v in &fallback {
                                self.link(v, id);
                            }
                            return Outcome { node: Some(id), root_call: true, ..Outcome::none() };
                        }
                    }
                }
            }
            _ => {}
        }
        if let Expr::Call(c) = e {
            let callee = self.classify(c);
            if self.is_major(&callee, c) {
                let id = self.call_node(e, &callee, g);
                return Outcome { node: Some(id), root_call: true, ..Outcome::none() };
            }
            if let Callee::Super = callee {
                return Outcome::none();
            }
            // A minor call at the root (`x.size()`, `len(data)`) is the statement's block;
            // its inputs are real inputs, and a scalar result is drawn lightly downstream.
            if Self::metadata(&callee) {
                let mut vals = self.collect(e, g, false);
                for v in &mut vals {
                    v.scalar = v.style == Style::Ctx;
                }
                dedup_vals(&mut vals);
                if vals.is_empty() {
                    return Outcome::none();
                }
                let id = self.fused(e, vals, dest);
                return Outcome { node: Some(id), scalar: true, ..Outcome::none() };
            }
        }
        if let Expr::BinOp(b) = e {
            if matches!(b.op, ast::Operator::Add | ast::Operator::Mult | ast::Operator::MatMult | ast::Operator::Sub) {
                let mut left = self.collect(&b.left, g, false);
                let mut right = self.collect(&b.right, g, false);
                dedup_vals(&mut left);
                dedup_vals(&mut right);
                let rich = |v: &Vec<Val>| v.iter().any(|x| !x.scalar && x.style == Style::Data);
                if rich(&left) && rich(&right) && left.len() == 1 && right.len() == 1 {
                    let id = self.add(Kind::Glyph, Shape::Glyph, op_symbol(b.op));
                    self.cx.b.get_mut(id).category = "op";
                    self.cx.b.get_mut(id).code = compact(self.src().of(e), 160);
                    let span = self.src().span(e.range());
                    self.cx.b.get_mut(id).span = Some(span);
                    for v in left.iter().chain(&right) {
                        self.link(v, id);
                    }
                    return Outcome { node: Some(id), ..Outcome::none() };
                }
                let scalar = !rich(&left) && !rich(&right);
                let mut vals = left;
                vals.extend(right);
                dedup_vals(&mut vals);
                if vals.is_empty() {
                    return Outcome::none();
                }
                let id = self.fused(e, vals, dest);
                return Outcome { node: Some(id), scalar, ..Outcome::none() };
            }
        }
        let first_new = self.cx.b.nodes.len() as Id;
        let mut vals = self.collect(e, g, false);
        dedup_vals(&mut vals);
        if vals.is_empty() && g.minor_calls.is_empty() {
            return Outcome::none();
        }
        // A single block wrapped in trivial glue (`f(x)[0]`, `g(x).attr`) keeps its identity.
        let made_here = |s: Id| s >= first_new && !self.cx.b.get(s).is_port();
        if vals.len() == 1 && vals[0].srcs.len() == 1 && made_here(vals[0].srcs[0]) && (is_glue(e) || self.m.lang.is_js() && is_promise_chain(e)) {
            return Outcome { node: Some(vals[0].srcs[0]), ..Outcome::none() };
        }
        // A collection is data even when only sizes went into it.
        let collection = matches!(e, Expr::Comp(_) | Expr::List(_) | Expr::Set(_) | Expr::Dict(_));
        let scalar = !collection && !vals.is_empty() && vals.iter().all(|v| v.scalar);
        let id = self.fused(e, vals, dest);
        Outcome { node: Some(id), scalar, ..Outcome::none() }
    }

    /// `dict(wte=nn.Embedding(...))`: the block built for a value takes its key as name.
    fn name_by_key(&mut self, key: Option<&str>, value: &Expr, vals: &[Val]) {
        let (Some(key), Expr::Call(c)) = (key, value) else { return };
        let Some(v) = vals.first() else { return };
        if vals.len() != 1 || v.srcs.len() != 1 {
            return;
        }
        let id = v.srcs[0];
        let n = self.cx.b.get(id);
        if n.span.as_ref().map(|s| s.start) != Some(u32::from(c.range().start())) {
            return;
        }
        let n = self.cx.b.get_mut(id);
        let ctor = std::mem::replace(&mut n.title, key.to_string());
        if !n.subtitle.contains('×') {
            n.subtitle = ctor.clone();
        }
        if let Some(cat) = self.cx.category(self.mi, &[&ctor, key]) {
            self.cx.b.get_mut(id).category = cat;
        }
    }

    /// One block for glue: named by where its result goes and what it does
    /// (see `crate::naming`).
    fn fused(&mut self, e: &'p Expr, vals: Vec<Val>, dest: &Dest) -> Id {
        let src = self.src();
        // In Go an `err` only reports how a call went (`err := row.Scan(…)`):
        // the call names the step.
        let reports = self.m.lang == Lang::Go && matches!(dest, Dest::Names(ns) if ns.iter().all(|n| naming::is_error_name(n)));
        let dest = if reports { &Dest::None } else { dest };
        let name = {
            let namer = self.namer();
            naming::name(&namer.step(e), dest, &namer.code(e, naming::CAPTION_MAX))
        };
        // `Router::new().route("/users", get(list))…`: a table of the routes it registers.
        if self.m.lang == Lang::Rust {
            let mut found = vec![];
            frameworks::chain_routes(e, self.fid.and_then(|f| self.cx.routes.prefixes.get(&f)).map(String::as_str).unwrap_or(""), &mut found, &mut vec![]);
            if !found.is_empty() {
                let rows: Vec<Row> = found
                    .iter()
                    .map(|r| Row { key: format!("{} {}", r.verb, r.path), value: match frameworks::handler_of(r.handler) {
                        Expr::Lambda(_) | Expr::Function(_) => "inline handler".into(),
                        h => self.namer().code(h, 30),
                    }, note: String::new(), heading: false })
                    .collect();
                if !self.cx.handlers[self.mi].contains(&Handler::Route) {
                    self.cx.handlers[self.mi].push(Handler::Route);
                }
                let id = self.add(Kind::Table, Shape::Box, "routes");
                let span = src.span(e.range());
                let n = self.cx.b.get_mut(id);
                n.subtitle = naming::count(rows.len(), "route");
                n.category = "endpoint";
                n.rows = rows;
                n.code = compact(src.of(e), 200);
                n.span = Some(span);
                for v in &vals {
                    self.link(v, id);
                }
                return id;
            }
        }
        // `router.route("/:id").get(…).patch(…)`: the routes it defines.
        let (name, endpoint) = match self.route_chain(e) {
            Some(title) => (naming::Name { title, caption: name.caption }, true),
            None => (name, false),
        };
        let id = self.add(Kind::Op, Shape::Box, name.title);
        let span = src.span(e.range());
        {
            let n = self.cx.b.get_mut(id);
            n.subtitle = name.caption;
            n.category = if endpoint { "endpoint" } else { "op" };
            n.code = compact(src.of(e), 200);
            n.span = Some(span);
        }
        for v in &vals {
            self.link(v, id);
        }
        id
    }

    fn finish_function(&mut self, node: Id) {
        let returns: Vec<Val> = self.regions[0].returns.iter().filter(|v| !v.srcs.is_empty()).cloned().collect();
        let kinds: Vec<String> = self.regions[0].effects.iter().filter_map(|e| e.strip_prefix("__")).map(|s| s.to_string()).collect();
        let yields = kinds.iter().any(|k| k.starts_with("yield"));
        if !returns.is_empty() {
            let label = return_label(&returns);
            let title = if yields { format!("yield {label}") } else if label.is_empty() { "return".into() } else { label.clone() };
            let port = self.cx.b.add(Some(node), Kind::Output, Shape::Port, title.trim());
            for v in &returns {
                for &s in &v.srcs {
                    let fp = self.out_ports.get(&(s, v.label.clone())).copied();
                    self.connect(s, port, kind_of(v.style), &v.label, fp, None);
                }
            }
            self.cx.b.get_mut(node).outputs = vec![label];
        }
        let effects: Vec<String> = self.regions[0].effects.iter().filter(|e| !e.starts_with("__")).cloned().collect();
        let n = self.cx.b.get_mut(node);
        n.effects.extend(effects);
        if yields {
            n.badges.push("generator".into());
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RunKind {
    Call,
    Store,
    Update,
}

/// What a statement acts on, for fusing runs (see `Fb::subject`).
struct Subject {
    recv: String,
    verb: String,
    project: bool,
    kind: RunKind,
}

struct Outcome {
    node: Option<Id>,
    alias: Option<Val>,
    root_call: bool,
    /// The value is a scalar (a size, a count): drawn lightly downstream.
    scalar: bool,
}

impl Outcome {
    fn none() -> Self {
        Outcome { node: None, alias: None, root_call: false, scalar: false }
    }
}

struct RangeOf(TextRange);
impl Ranged for RangeOf {
    fn range(&self) -> TextRange {
        self.0
    }
}

pub fn attach(b: &mut Builder, parent: Id, child: Id) {
    if b.get(child).parent == Some(parent) {
        return;
    }
    if let Some(p) = b.get(child).parent {
        b.get_mut(p).children.retain(|c| *c != child);
    }
    let order = b.get(parent).children.len() as u32;
    b.get_mut(child).parent = Some(parent);
    b.get_mut(child).hints.order = order;
    b.get_mut(parent).children.push(child);
}

pub fn build_function<'p>(cx: &mut Ctx<'p>, fid: usize, outer: HashSet<String>, types: HashMap<String, Ty>) {
    let info = &cx.ix.funcs[fid];
    let (mi, node, def, class, kind) = (info.module, info.node, info.def, info.class, info.kind);
    let top = class.is_none() && matches!(cx.ix.defs_at.get(&(mi, u32::from(def.range.start()))), Some(Sym::Func(f)) if *f == fid);
    let mut fb = Fb::new(cx, mi, node);
    fb.fid = Some(fid);
    fb.top = top;
    fb.types = types;
    fb.outer = outer;
    fb.in_init = def.name == "__init__" || def.kind == ast::DefKind::Constructor;
    let params = def.params();
    for (i, p) in params.iter().enumerate() {
        if i == 0 && class.is_some() && kind != FnKind::StaticMethod {
            let cid = class.unwrap();
            if kind == FnKind::ClassMethod {
                fb.types.insert(p.clone(), Ty::Class(cid));
            } else {
                fb.types.insert(p.clone(), Ty::Inst(cid));
                fb.this = Some((p.clone(), cid));
            }
            fb.regions[0].env.insert(p.clone(), Bind::Static);
            continue;
        }
        let port = fb.cx.b.add(Some(node), Kind::Input, Shape::Port, p.clone());
        fb.cx.b.get_mut(port).category = "port";
        fb.regions[0].env.insert(p.clone(), Bind::Src(vec![port], Style::Data));
        let annotation = def.args.iter().filter(|a| matches!(a.kind, ast::ParamKind::Positional | ast::ParamKind::KeywordOnly)).find(|a| a.name == *p).and_then(|a| a.annotation.as_ref());
        match annotation.map(|a| fb.cx.ix.annotated(mi, a, &TypesView(&fb.types))) {
            Some(t @ (Ty::Inst(_) | Ty::List(..))) => {
                fb.types.insert(p.clone(), t);
            }
            _ => {
                fb.types.remove(p);
            }
        }
    }
    let body: &[Stmt] = if docstring(def.body).is_some() { &def.body[1..] } else { def.body };
    fb.statements(body, &HashSet::new(), true);
    fb.finish_function(node);
    // A body with a single plain step is a leaf: the code says it all.
    let b = &mut fb.cx.b;
    let steps: Vec<Id> = b.get(node).children.iter().copied().filter(|&c| !b.get(c).is_port()).collect();
    let trivial = steps.len() <= 1 && steps.iter().all(|&c| b.get(c).reference.is_none() && !b.get(c).has_interior());
    if trivial {
        let kids = std::mem::take(&mut b.get_mut(node).children);
        b.get_mut(node).edges.clear();
        for k in &kids {
            let effects = b.get(*k).effects.clone();
            b.get_mut(node).effects.extend(effects);
            b.get_mut(*k).parent = None;
        }
    }
    let n = b.get_mut(node);
    n.effects.sort();
    n.effects.dedup();
}

pub fn build_class<'p>(cx: &mut Ctx<'p>, cid: usize, outer: HashSet<String>, types: HashMap<String, Ty>) {
    let (mi, node, def) = {
        let c = &cx.ix.classes[cid];
        (c.module, c.node, c.def)
    };
    let modules: &'p [Module] = cx.ix.modules;
    let src = &modules[mi].src;
    let ext_base = cx.ix.external_base(cid);
    // An extension (`impl Foo` in another module) leads to the type it extends.
    let extends = match (def.kind, cx.ix.classes[cid].bases.first()) {
        (ast::ClassKind::Extension, Some(Ty::Class(t))) => Some(cx.ix.classes[*t].node),
        _ => None,
    };
    let project_bases: Vec<String> = cx.ix.classes[cid].bases.iter().filter_map(|b| if let Ty::Class(k) = b { Some(cx.ix.classes[*k].name.clone()) } else { None }).collect();
    {
        let name = cx.ix.classes[cid].name.clone();
        let doc = docstring(&def.body).map(summary).unwrap_or_else(|| {
            let line = src.line_of(usize::from(def.range.start()));
            src.prose_above(line)
        });
        let span = src.span(def.range);
        let data = cx.ix.classes[cid].data;
        let category = cx.category(mi, &[&name]);
        let n = cx.b.get_mut(node);
        n.kind = Kind::Class;
        n.shape = Shape::Box;
        n.title = name.clone();
        let mut sub: Vec<String> = project_bases.clone();
        if let Some(e) = &ext_base {
            sub.push(e.clone());
        }
        n.subtitle = sub.join(", ");
        // A Go type satisfies the project's interfaces whose methods it has,
        // without saying so: it lists them, as a Rust type lists its traits.
        if modules[mi].lang == Lang::Go && def.kind == ast::ClassKind::Class {
            let mut satisfied: Vec<String> = cx.ix.implements[cid].iter().map(|&k| cx.ix.classes[k].name.clone()).collect();
            satisfied.sort();
            satisfied.dedup();
            if !satisfied.is_empty() {
                let mut sub: Vec<String> = n.subtitle.split(", ").filter(|s| !s.is_empty()).map(str::to_string).collect();
                for i in satisfied {
                    if !sub.contains(&i) {
                        sub.push(i);
                    }
                }
                n.subtitle = naming::list(&sub, 36);
            }
        }
        // A Rust type lists the traits it implements: `Display, Error, From`.
        if modules[mi].lang == Lang::Rust {
            let mut traits: Vec<String> = vec![];
            for b in class_bases(def) {
                let t = squash(src.of(b));
                let head = t.split('<').next().unwrap_or(&t);
                let name = head.rsplit("::").next().unwrap_or(head).to_string();
                if !name.is_empty() && !traits.contains(&name) {
                    traits.push(name);
                }
            }
            n.subtitle = naming::list(&traits, 36);
        }
        match def.kind {
            ast::ClassKind::Namespace => n.subtitle = "namespace".into(),
            ast::ClassKind::Object if n.subtitle.is_empty() => n.subtitle = "object".into(),
            ast::ClassKind::Trait => n.subtitle = if n.subtitle.is_empty() { "trait".into() } else { format!("trait: {}", n.subtitle) },
            // `impl Display for Frame`: the title says it all.
            ast::ClassKind::Extension => {
                n.subtitle = if modules[mi].lang == Lang::Go { "methods".into() } else { String::new() };
                n.link = extends;
            }
            _ => {}
        }
        n.category = if data { "config" } else { category.unwrap_or("class") };
        n.doc = doc;
        n.span = Some(span);
        for d in &def.decorators {
            n.badges.extend(super::frameworks::tag(d, modules[mi].lang, |e| Namer::plain(src).code(e, 20)));
        }
        // ORM models are the database's tables: SQLModel `table=True`,
        // Django `models.Model`, SQLAlchemy declarative bases.
        let keywords = &def.keywords[..];
        let table = keywords.iter().any(|k| k.arg.as_deref() == Some("table") && matches!(k.value, Expr::BooleanLiteral(ref b) if b.value))
            || class_bases(def).iter().any(|b| {
                let t = squash(src.of(b));
                // `Base = declarative_base()`: a base that is a variable, not a project class.
                t.ends_with("models.Model") || t == "db.Model" || t == "DeclarativeBase" || t == "Base" && ext_base.is_none() && !project_bases.iter().any(|b| b == "Base")
            });
        if table {
            n.badges.push("DB table".into());
        }
    }
    if cx.ix.classes[cid].data {
        let mut rows = vec![];
        for s in &def.body {
            match s {
                Stmt::AnnAssign(a) => rows.push(Row {
                    // A declared field is keyed by its name (`this.x`, `self.0`).
                    key: match &*a.target {
                        Expr::Attribute(x) => x.attr.to_string(),
                        t => Namer::plain(src).code(t, 30),
                    },
                    value: match &a.value {
                        Some(v) => format!("{} = {}", Namer::plain(src).code(&a.annotation, 16), Namer::plain(src).code(v, 30)),
                        None => Namer::plain(src).code(&a.annotation, 30),
                    },
                    note: src.trailing_comment(src.line_of(usize::from(a.range.start()))).unwrap_or("").to_string(),
                    heading: false,
                }),
                Stmt::Assign(a) => rows.push(Row { key: Namer::plain(src).code(&a.targets[0], 30), value: Namer::plain(src).code(&a.value, 34), note: String::new(), heading: false }),
                // An enum member without a value: `enum Color { Red, Green }`.
                Stmt::Expr(e) if matches!(&*e.value, Expr::Name(_)) => rows.push(Row { key: Namer::plain(src).code(&e.value, 30), value: String::new(), note: String::new(), heading: false }),
                // A Go interface's method: its signature as written.
                Stmt::FunctionDef(f) if f.body.is_empty() => {
                    let text = squash(src.of(f));
                    let sig = text.strip_prefix(f.name.as_str()).unwrap_or(&text).trim().to_string();
                    rows.push(Row { key: f.name.clone(), value: naming::shorten(&sig, 40), note: String::new(), heading: false });
                }
                _ => {}
            }
        }
        cx.b.get_mut(node).rows = rows;
        return;
    }
    // Components: what the class is built from.
    let comps: Vec<(String, Ty, String, TextRange)> = cx.ix.classes[cid].fields.iter().filter(|f| is_component(&f.ty)).map(|f| (f.name.clone(), f.ty.clone(), f.ctor.clone(), f.range)).collect();
    if !comps.is_empty() {
        let group = cx.b.add(Some(node), Kind::Group, Shape::Region, "components");
        cx.b.get_mut(group).category = "region";
        cx.b.get_mut(group).hints.pack = -1;
        for (name, ty, ctor, range) in comps {
            component_node(cx, mi, cid, group, &name, &name, &ty, &ctor, Some(src.span(range)));
        }
    }
    let methods = cx.ix.classes[cid].methods.clone();
    let this_class = cid;
    for (_, fid) in &methods {
        let fnode = cx.ix.funcs[*fid].node;
        attach(&mut cx.b, node, fnode);
    }
    // Describe and build each method.
    for (_, fid) in &methods {
        let def_stmt = def.body.iter().find(|s| FnDef::from_stmt(s).is_some_and(|f| f.range == cx.ix.funcs[*fid].def.range)).unwrap();
        {
            let mut fb = Fb::new(cx, mi, node);
            fb.describe_function(*fid, def_stmt);
        }
        build_function(cx, *fid, outer.clone(), types.clone());
    }
    // A namespace's own types, drawn inside it.
    for (_, k) in cx.ix.classes[cid].inner.clone() {
        attach(&mut cx.b, node, cx.ix.classes[k].node);
        build_class(cx, k, outer.clone(), types.clone());
    }
    // Method → method dependencies (callee above caller).
    for (_, fid) in &methods {
        let info = &cx.ix.funcs[*fid];
        let this = info.def.params().first().cloned().unwrap_or_default();
        let mut used: Vec<usize> = vec![];
        walk::stmt_exprs(info.def.body, true, &mut |root| walk::walk(root, &mut |e| match e {
            Expr::Attribute(a) => {
                if matches!(&*a.value, Expr::Name(n) if n.id.as_str() == this || n.id.as_str() == "cls") {
                    if let Some(f) = methods.iter().find(|(n, _)| n == a.attr.as_str()).map(|(_, f)| *f) {
                        used.push(f);
                    }
                }
            }
            Expr::Call(c) => {
                if let Expr::Name(n) = &*c.func {
                    if n.id.as_str() == this {
                        if let Some(f) = cx.ix.call_target(this_class) {
                            used.push(f);
                        }
                    }
                    if n.id.as_str() == "cls" || n.id.as_str() == cx.ix.classes[this_class].name {
                        if let Some(f) = methods.iter().find(|(n, _)| n == "__init__").map(|(_, f)| *f) {
                            used.push(f);
                        }
                    }
                }
            }
            _ => {}
        }));
        used.sort();
        used.dedup();
        let caller = info.node;
        for f in used {
            if f == *fid {
                continue;
            }
            let callee = cx.ix.funcs[f].node;
            cx.b.connect(callee, caller, EdgeKind::Uses, "");
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn component_node(cx: &mut Ctx, mi: usize, cid: usize, parent: Id, name: &str, path: &str, ty: &Ty, ctor: &str, span: Option<Span>) {
    if let Ty::Dict(fields) = ty {
        let group = cx.b.add(Some(parent), Kind::Group, Shape::Region, name);
        cx.b.get_mut(group).category = "region";
        cx.b.get_mut(group).span = span.clone();
        for (k, t) in fields {
            if is_component(t) {
                component_node(cx, mi, cid, group, k, &format!("{path}.{k}"), t, "", span.clone());
            }
        }
        return;
    }
    let display = cx.ix.display(ty);
    let reference = match ty {
        Ty::Inst(k) => Some(cx.ix.classes[*k].node),
        Ty::List(e, _) => match &**e {
            Ty::Inst(k) => Some(cx.ix.classes[*k].node),
            _ => None,
        },
        _ => None,
    };
    let id = cx.b.add(Some(parent), Kind::Symbol, Shape::Box, name);
    cx.components.insert((cid, path.to_string()), id);
    let project = reference.is_some();
    let category = cx.category(mi, &[&display, name]);
    let n = cx.b.get_mut(id);
    n.subtitle = if display.is_empty() { naming::qualifier(&squash(ctor), 24) } else { display.clone() };
    n.category = category.unwrap_or(if project { "component" } else { "library" });
    n.link = reference;
    n.span = span;
}

fn is_component(t: &Ty) -> bool {
    match t {
        // A `Map` or `Date` held by an object is state, not a component, as
        // is a `PathBuf` or `Duration` from Rust's standard library, or a
        // channel or lock that only coordinates tasks.
        Ty::ExtInst(p) => !["globals.", "std.", "core.", "alloc.", "tokio.sync.", "parking_lot.", "crossbeam", "flume.", "async_channel.", "futures.channel.", "bytes.", "sync.", "sync/atomic.", "context.", "time.", "strings.", "math/", "regexp.", "container/"].iter().any(|s| p.starts_with(s)),
        Ty::Inst(_) => true,
        Ty::Ext(p) => p == "buffer",
        Ty::Dict(f) => f.iter().any(|(_, t)| is_component(t)),
        Ty::List(e, _) => is_component(e),
        _ => false,
    }
}

fn is_callable_ty(t: &Ty) -> bool {
    matches!(t, Ty::Inst(_) | Ty::ExtInst(_))
}

/// Every value is drawn the same way, whatever it is: what a value means
/// cannot be told from its name in every language.
fn kind_of(style: Style) -> EdgeKind {
    match style {
        Style::Data | Style::Ctx => EdgeKind::Data,
        Style::Def => EdgeKind::Uses,
    }
}

/// Removes repeated values. A value used both as data and for a property
/// (`x[i]` and `len(x)`) is data.
fn dedup_vals(vals: &mut Vec<Val>) {
    let mut out: Vec<Val> = vec![];
    for v in vals.drain(..) {
        if v.srcs.is_empty() {
            continue;
        }
        match out.iter_mut().find(|o| o.srcs == v.srcs && o.label == v.label) {
            Some(o) => o.scalar &= v.scalar,
            None => out.push(v),
        }
    }
    *vals = out;
}

fn return_label(vals: &[Val]) -> String {
    let mut labels: Vec<&str> = vec![];
    for v in vals {
        for part in v.label.split(", ") {
            if !part.is_empty() && !labels.contains(&part) {
                labels.push(part);
            }
        }
    }
    let labels: Vec<String> = labels.into_iter().map(str::to_string).collect();
    naming::list(&labels, 32)
}

fn op_symbol(op: ast::Operator) -> &'static str {
    match op {
        ast::Operator::Add => "+",
        ast::Operator::Sub => "−",
        ast::Operator::Mult => "×",
        ast::Operator::MatMult => "@",
        ast::Operator::Div => "÷",
        _ => "∘",
    }
}

/// `super().__init__(…)` in Python; `super(…)` and `super.method(…)` in JavaScript.
fn is_super(c: &ast::ExprCall) -> bool {
    match &*c.func {
        Expr::Attribute(a) => match &*a.value {
            Expr::Call(inner) => matches!(&*inner.func, Expr::Name(n) if n.id.as_str() == "super"),
            Expr::Name(n) => n.id == "super",
            _ => false,
        },
        Expr::Name(n) => n.id == "super",
        _ => false,
    }
}

/// `if __name__ == "__main__"`, and JavaScript's equivalents: `require.main
/// === module`, `import.meta.main`, and comparing `import.meta.url` with the
/// script `process.argv[1]` names.
fn is_main_guard(test: &Expr) -> bool {
    if let Expr::Compare(c) = test {
        let parts: Vec<&Expr> = c.operands.iter().collect();
        let has_name = parts.iter().any(|e| matches!(e, Expr::Name(n) if n.id.as_str() == "__name__"));
        let has_main = parts.iter().any(|e| string_literal(e) == Some("__main__"));
        if has_name && has_main {
            return true;
        }
        let require_main = parts.iter().any(|e| matches!(e, Expr::Attribute(a) if a.attr == "main" && matches!(&*a.value, Expr::Name(n) if n.id == "require"))) && parts.iter().any(|e| matches!(e, Expr::Name(n) if n.id == "module"));
        let mut meta_url = false;
        let mut argv = false;
        walk::walk(test, &mut |x| {
            if let Expr::Attribute(a) = x {
                meta_url |= a.attr == "url" && matches!(&*a.value, Expr::Attribute(m) if m.attr == "meta" && matches!(&*m.value, Expr::Name(n) if n.id == "import"));
                argv |= a.attr == "argv" && matches!(&*a.value, Expr::Name(n) if n.id == "process");
            }
        });
        return require_main || (meta_url && argv);
    }
    matches!(test, Expr::Attribute(a) if a.attr == "main" && matches!(&*a.value, Expr::Attribute(m) if m.attr == "meta" && matches!(&*m.value, Expr::Name(n) if n.id == "import")))
}

/// Literal constants (strings, numbers, booleans, `None`, `...`, bytes).
fn is_constant(e: &Expr) -> bool {
    matches!(e, Expr::StringLiteral(_) | Expr::BytesLiteral(_) | Expr::NumberLiteral(_) | Expr::BooleanLiteral(_) | Expr::NoneLiteral(_) | Expr::EllipsisLiteral(_))
}

fn is_literal(e: &Expr) -> bool {
    match e {
        e if is_constant(e) => true,
        Expr::UnaryOp(u) => is_literal(&u.operand),
        Expr::List(l) => l.elts.iter().all(is_literal),
        Expr::Tuple(l) => l.elts.iter().all(is_literal),
        Expr::Set(l) => l.elts.iter().all(is_literal),
        Expr::Dict(d) => d.items.iter().all(|i| i.key.as_ref().is_none_or(is_literal) && is_literal(&i.value)),
        _ => false,
    }
}

fn is_compound(s: &Stmt) -> bool {
    matches!(s, Stmt::If(_) | Stmt::For(_) | Stmt::While(_) | Stmt::Try(_) | Stmt::With(_) | Stmt::Match(_) | Stmt::FunctionDef(_) | Stmt::ClassDef(_))
}

fn has_break(body: &[Stmt]) -> bool {
    body.iter().any(|s| match s {
        Stmt::Break(_) => true,
        Stmt::If(x) => has_break(&x.body) || x.elif_else_clauses.iter().any(|c| has_break(&c.body)),
        Stmt::With(x) => has_break(&x.body),
        Stmt::Try(x) => has_break(&x.body),
        _ => false,
    })
}

pub fn string_literal(e: &Expr) -> Option<&str> {
    if let Expr::StringLiteral(s) = e {
        return Some(&s.value);
    }
    None
}

fn root_name(e: &Expr) -> Option<&str> {
    match e {
        Expr::Name(n) => Some(&n.id),
        Expr::Attribute(a) => root_name(&a.value),
        Expr::Subscript(s) => root_name(&s.value),
        Expr::Call(c) => match &*c.func {
            Expr::Attribute(a) => root_name(&a.value),
            _ => None,
        },
        _ => None,
    }
}

fn contains_call(e: &Expr) -> bool {
    let mut found = false;
    walk::walk(e, &mut |x| {
        if matches!(x, Expr::Call(_)) {
            found = true;
        }
    });
    found
}

/// `x.shape`, `x.shape[0]`, `x.device`: a property of a value.
fn is_metadata_read(e: &Expr) -> bool {
    match e {
        Expr::Attribute(a) => vocabulary::is_metadata_attr(a.attr.as_str()),
        Expr::Subscript(s) => is_metadata_read(&s.value),
        _ => false,
    }
}

/// Glue that does not deserve its own block around a single major call:
/// tuple packing, subscripts, attribute access and unpacking.
/// `p.then(…)`, `p.catch(…)`: passes a promise's value on.
fn is_promise_chain(e: &Expr) -> bool {
    matches!(e, Expr::Call(c) if matches!(&*c.func, Expr::Attribute(a) if matches!(a.attr.as_str(), "then" | "catch" | "finally")) && !has_inline_function(c))
}

fn is_glue(e: &Expr) -> bool {
    matches!(e, Expr::Subscript(_) | Expr::Attribute(_) | Expr::Starred(_))
}

/// Splits a store target into its root variable and the expressions it reads.
fn split_target(t: &Expr) -> (Option<&str>, Vec<&Expr>) {
    match t {
        Expr::Name(n) => (Some(&n.id), vec![]),
        Expr::Attribute(a) => split_target(&a.value),
        Expr::Subscript(s) => {
            let (b, mut rest) = split_target(&s.value);
            rest.push(&s.slice);
            (b, rest)
        }
        Expr::Call(c) => (None, c.arguments.args.iter().collect()),
        _ => (None, vec![]),
    }
}

enum Decl {
    Exports(Vec<String>),
    Type,
    Quiet,
}

const TYPING_NAMES: &[&str] = &[
    "Union", "Optional", "Callable", "Literal", "Annotated", "Tuple", "List", "Dict", "Set", "FrozenSet", "Type", "Iterable", "Iterator", "Mapping", "MutableMapping", "Sequence", "MutableSequence", "Awaitable", "Coroutine", "AsyncIterator", "AsyncIterable", "Generator", "AsyncGenerator", "ClassVar", "Final", "TypeGuard", "TypeIs", "Required", "NotRequired", "ReadOnly", "Unpack", "Concatenate", "Collection", "Container", "AbstractSet", "MutableSet", "Deque", "DefaultDict", "OrderedDict", "Counter", "ChainMap", "Pattern", "IO", "Generic", "Protocol",
];

fn is_dunder(name: &str) -> bool {
    name.len() > 4 && name.starts_with("__") && name.ends_with("__")
}

/// `logging.getLogger(__name__)` and friends.
fn is_logger(e: &Expr) -> bool {
    let Expr::Call(c) = e else { return false };
    let last = match &*c.func {
        Expr::Name(n) => n.id.as_str(),
        Expr::Attribute(a) => a.attr.as_str(),
        _ => return false,
    };
    matches!(last, "getLogger" | "get_logger" | "getChild")
}

/// `TypeVar("T")`, `NewType("UserId", int)`, `NamedTuple("P", ...)`.
fn is_type_factory(e: &Expr) -> bool {
    let Expr::Call(c) = e else { return false };
    let last = match &*c.func {
        Expr::Name(n) => n.id.as_str(),
        Expr::Attribute(a) => a.attr.as_str(),
        _ => return false,
    };
    matches!(last, "TypeVar" | "ParamSpec" | "TypeVarTuple" | "NewType" | "TypeAliasType") || (matches!(last, "NamedTuple" | "TypedDict") && c.arguments.args.first().is_some_and(|a| matches!(a, Expr::StringLiteral(_))))
}

/// String literals in a list or tuple (an `__all__`).
fn strings_in(e: &Expr) -> Vec<String> {
    let elts: &[Expr] = match e {
        Expr::List(l) => &l.elts,
        Expr::Tuple(t) => &t.elts,
        _ => return vec![],
    };
    elts.iter().filter_map(|x| string_literal(x).map(str::to_string)).collect()
}

/// Statements that only import or bind fallbacks, as in optional imports.
fn import_only(body: &[Stmt]) -> bool {
    body.iter().all(|s| match s {
        Stmt::Import(_) | Stmt::ImportFrom(_) | Stmt::Pass(_) | Stmt::Raise(_) => true,
        Stmt::Expr(e) => is_constant(&e.value),
        Stmt::Assign(a) => matches!(&*a.value, Expr::Name(_) | Expr::Attribute(_)) || is_constant(&a.value),
        Stmt::If(x) => import_only(&x.body) && x.elif_else_clauses.iter().all(|c| import_only(&c.body)),
        Stmt::Try(x) => import_only(&x.body) && x.handlers.iter().all(|h| import_only(&h.body)) && import_only(&x.orelse),
        _ => false,
    })
}

/// Statements that only adjust a module's constant tables:
/// `if brotli is None: SUPPORTED_DECODERS.pop("br")`.
fn adjusts(body: &[Stmt], consts: &HashSet<String>) -> bool {
    let on_const = |e: &Expr| matches!(split_target(e).0, Some(b) if consts.contains(b));
    !body.is_empty()
        && body.iter().all(|s| match s {
            Stmt::Expr(e) => matches!(&*e.value, Expr::Call(c) if matches!(&*c.func, Expr::Attribute(a) if on_const(&a.value))),
            Stmt::Assign(a) => a.targets.iter().all(|t| !matches!(t, Expr::Name(_)) && on_const(t)),
            Stmt::AugAssign(a) => on_const(&a.target),
            Stmt::If(x) => adjusts(&x.body, consts) && x.elif_else_clauses.iter().all(|c| adjusts(&c.body, consts)),
            Stmt::Pass(_) => true,
            _ => false,
        })
}

/// `__locals = locals()` and loops over `__all__`: module plumbing.
fn is_plumbing(s: &Stmt) -> bool {
    let reflective = |e: &Expr| matches!(e, Expr::Call(c) if matches!(&*c.func, Expr::Name(n) if matches!(n.id.as_str(), "locals" | "globals" | "vars" | "dir")));
    match s {
        Stmt::Assign(a) => reflective(&a.value),
        Stmt::For(f) => matches!(&*f.iter, Expr::Name(n) if n.id.as_str() == "__all__") || reflective(&f.iter),
        _ => false,
    }
}


/// The title a comment block gives the code below it. One line is a heading
/// (`# training loop`); a longer block is an explanation, titled only by a
/// short first line (`# Optimizer\n# AdamW with weight decay on matrices`).
fn comment_title(comments: &[&super::source::Comment]) -> Option<String> {
    let prose: Vec<&str> = comments
        .iter()
        .filter(|c| !c.doc)
        .map(|c| c.text.trim())
        .filter(|t| t.chars().any(|c| c.is_alphanumeric()) && !is_directive(t))
        .collect();
    match prose.as_slice() {
        [] => None,
        [one] => heading(one),
        [first, ..] => {
            let short = first.split_whitespace().count() <= 5 && first.chars().count() <= 40 && !first.ends_with([',', ';', '(']);
            if short {
                heading(first)
            } else {
                None
            }
        }
    }
}

fn stmt_start_line(src: &super::source::Source, s: &Stmt) -> u32 {
    let mut start = usize::from(s.start());
    let decorators: &[ast::Decorator] = match s {
        Stmt::FunctionDef(f) => &f.decorators,
        Stmt::ClassDef(c) => &c.decorators,
        _ => &[],
    };
    for d in decorators {
        // Decorator ranges start at the '@'.
        start = start.min(usize::from(d.start()));
    }
    src.line_of(start)
}

/// Names a statement reads, ignoring what is only printed or logged.
fn stmt_loads(s: &Stmt) -> HashSet<String> {
    let mut out = HashSet::new();
    walk::stmt_expr(s, true, &mut |e| {
        if let Expr::Call(c) = e {
            if matches!(&*c.func, Expr::Name(n) if matches!(n.id.as_str(), "print" | "pprint")) {
                return;
            }
        }
        out.extend(walk::loads_deep(e))
    });
    out
}

/// Expressions that compute a path (and so carry file names to a variable).
fn is_path_expr(e: &Expr) -> bool {
    match e {
        Expr::StringLiteral(_) | Expr::Name(_) => true,
        Expr::Template(t) => t.kind == TemplateKind::Format && t.tag.is_none(),
        Expr::BinOp(b) => is_path_expr(&b.left) && is_path_expr(&b.right),
        Expr::Call(c) => {
            let f = match &*c.func {
                Expr::Attribute(a) => a.attr.as_str(),
                Expr::Name(n) => n.id.as_str(),
                _ => "",
            };
            matches!(f, "join" | "Path" | "abspath" | "expanduser" | "format" | "with_suffix" | "with_name" | "joinpath" | "resolve" | "str")
        }
        _ => false,
    }
}

fn callee_name(c: &Callee, fb: &Fb) -> String {
    match c {
        Callee::Ext(p) => p.rsplit('.').next().unwrap_or(p).trim_end_matches('!').to_string(),
        Callee::Builtin(b) | Callee::Method(b) => b.trim_end_matches('!').to_string(),
        Callee::Func(f) => fb.cx.ix.funcs[*f].name.clone(),
        Callee::Dispatch { name, .. } => name.clone(),
        _ => String::new(),
    }
}

fn g_strings_files(fb: &Fb, e: &Expr) -> Vec<String> {
    let mut out = vec![];
    walk::walk(e, &mut |x| {
        if let Some(s) = string_literal(x).and_then(effects::file_name) {
            out.push(s);
        }
        if let Expr::Template(j) = x {
            if let Some(p) = effects::fstring_file(j) {
                out.push(p);
            }
        }
        if let Expr::Name(n) = x {
            if let Some(f) = fb.files.get(n.id.as_str()) {
                out.extend(f.iter().cloned());
            }
        }
    });
    out.sort();
    out.dedup();
    out
}

/// Names of functions that make a value of their type (`Vec::new`,
/// `Config::default`, `Vec::with_capacity`).
fn is_constructor_name(name: &str) -> bool {
    matches!(name, "new" | "default") || name.starts_with("new_") || name.starts_with("with_")
}

/// Rust's printing macros.
fn is_print_macro(name: &str) -> bool {
    matches!(name, "println!" | "print!" | "eprintln!" | "eprint!" | "dbg!")
}

/// Logging macros of Rust's `log` and `tracing` crates.
fn is_log_macro(name: &str) -> bool {
    matches!(name, "info!" | "debug!" | "warn!" | "error!" | "trace!" | "event!" | "info_span!" | "debug_span!" | "trace_span!")
}

/// The event a handler property answers: `onClick` → "on click",
/// `onOpenChange` → "on open change".
fn event(prop: &str) -> Option<String> {
    let rest = prop.strip_prefix("on")?;
    if !rest.starts_with(|c: char| c.is_ascii_uppercase()) {
        return None;
    }
    Some(format!("on {}", crate::vocabulary::words(rest).join(" ")))
}

/// Calls that run their function argument once per item.
fn iterates(func: &Expr) -> bool {
    matches!(func, Expr::Attribute(a) if matches!(a.attr.as_str(), "map" | "flatMap" | "forEach" | "filter" | "find" | "findIndex" | "some" | "every" | "reduce" | "sort" | "toSorted" | "flat_map"))
}

/// Whether a function body returns JSX (a React component).
fn returns_jsx(body: &[Stmt]) -> bool {
    let mut found = false;
    walk::stmt_exprs(body, false, &mut |e| {
        walk::walk(e, &mut |x| {
            if matches!(x, Expr::Call(c) if c.kind == ast::CallKind::Jsx) {
                found = true;
            }
        })
    });
    found && body.iter().any(contains_return)
}

fn contains_return(s: &Stmt) -> bool {
    match s {
        Stmt::Return(r) => r.value.is_some(),
        Stmt::If(x) => x.body.iter().any(contains_return) || x.elif_else_clauses.iter().any(|c| c.body.iter().any(contains_return)),
        Stmt::Try(x) => x.body.iter().any(contains_return),
        Stmt::Match(x) => x.cases.iter().any(|c| c.body.iter().any(contains_return)),
        _ => false,
    }
}

/// A call given a function with statements (`router.get("/", async (req, res) => {…})`).
fn has_inline_function(c: &ast::ExprCall) -> bool {
    c.arguments.args.iter().chain(c.arguments.keywords.iter().map(|k| &k.value)).any(|a| matches!(a, Expr::Function(_)))
}

fn is_http_verb(m: &str) -> bool {
    matches!(m, "get" | "post" | "put" | "patch" | "delete" | "all" | "options" | "head")
}
