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
//! * Control flow becomes regions: loops and branches drawn inline when small
//!   (with "× n" repeat badges), lanes for if/elif/else, and a guard tag on a
//!   lone guarded statement instead of a box.
//! * Long statement lists are split into sections at comment-headed
//!   paragraphs; runs of constants become a settings table.
//! * Values crossing region boundaries get ports automatically, so every
//!   diagram has explicit inputs (top) and outputs (bottom).

use super::effects::{self, Io};
use super::frameworks::{self, Handler};
use super::index::*;
use super::walk;
use crate::ir::*;
use crate::text::{clip, heading, is_directive, squash, summary};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::{Ranged, TextRange};
use std::collections::{BTreeSet, HashMap, HashSet};

/// A cross-file relation discovered while building flows.
pub enum Relation {
    Exec { from: usize, to: usize },
    Artifact { module: usize, name: String, write: bool },
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
    Ctor(usize),
    Instance { class: usize, var: String },
    Component { name: String, ty: Ty },
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
const MINOR_EXT: &[&str] = &["os.path.", "math.", "operator.", "typing.", "functools.", "itertools.", "collections.", "copy.", "inspect.", "string.", "re.", "builtins.", "dataclasses.", "time.", "sys.exit"];
const SCALAR_FNS: &[&str] = &["len", "int", "float", "bool", "str", "round", "abs", "min", "max", "sum", "size", "item", "numel", "dim", "time", "perf_counter", "sqrt", "log", "exp", "ceil", "floor", "isinstance", "hasattr", "exists", "join", "dirname", "basename"];

pub struct Fb<'c, 'p> {
    cx: &'c mut Ctx<'p>,
    mi: usize,
    m: &'p PyModule,
    regions: Vec<Region>,
    types: HashMap<String, Ty>,
    files: HashMap<String, Vec<String>>,
    this: Option<(String, usize)>,
    outer: HashSet<String>,
    module_level: bool,
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
}

fn compact(s: &str, max: usize) -> String {
    clip(&squash(s), max)
}

/// Category from well-known names, so ML code reads like an architecture figure.
pub fn name_category(name: &str) -> Option<&'static str> {
    let n = name.to_lowercase();
    let n = n.as_str();
    if n.contains("attn") || n.contains("attention") {
        Some("attention")
    } else if n.contains("norm") || n.starts_with("ln_") || n == "ln" || n.starts_with("ln_f") {
        Some("norm")
    } else if n.contains("embed") || n == "wte" || n == "wpe" {
        Some("embed")
    } else if n.contains("softmax") || matches!(n, "gelu" | "relu" | "silu" | "tanh" | "sigmoid" | "swish" | "mish" | "elu" | "leaky_relu" | "glu" | "swiglu" | "act" | "activation") {
        Some("activation")
    } else if n.contains("dropout") {
        Some("dropout")
    } else if n.contains("mlp") || n.contains("feedforward") || n.contains("feed_forward") || n.contains("ffn") || n == "ff" {
        Some("mlp")
    } else if n.contains("linear") || n.contains("proj") || n == "dense" || n.contains("lm_head") || n.contains("head") || n.starts_with("c_fc") || n.starts_with("c_attn") || n.contains("conv") {
        Some("linear")
    } else if n.contains("loss") || n.contains("cross_entropy") {
        Some("loss")
    } else {
        None
    }
}

impl<'c, 'p> Fb<'c, 'p> {
    pub fn new(cx: &'c mut Ctx<'p>, mi: usize, root: Id) -> Self {
        let modules: &'p [PyModule] = cx.ix.modules;
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
            in_init: false,
            out_ports: HashMap::new(),
            globals_declared: HashSet::new(),
            loop_alias: HashMap::new(),
            library: false,
            handled: HashSet::new(),
        }
    }

    // ----- module and definitions ------------------------------------------------

    pub fn build_module(cx: &'c mut Ctx<'p>, mi: usize) {
        let root = cx.file_nodes[mi];
        let modules: &'p [PyModule] = cx.ix.modules;
        let m = &modules[mi];
        let library = cx.imported[mi] && !m.ast.iter().any(|s| matches!(s, Stmt::If(x) if is_main_guard(&x.test)));
        let mut fb = Fb::new(cx, mi, root);
        fb.module_level = true;
        fb.library = library;
        // Nothing runs after the module body, so nothing is "read later" at its end.
        let later = HashSet::new();
        let body: &'p [Stmt] = if docstring(&m.ast).is_some() { &m.ast[1..] } else { &m.ast };
        fb.declarations(body);
        fb.statements(body, &later, true);
        let effects = fb.regions[0].effects.clone();
        fb.cx.b.get_mut(root).effects.extend(effects);
    }

    fn container(&self) -> Id {
        self.regions.last().unwrap().node
    }

    fn src(&self) -> &'p super::source::Source {
        &self.m.src
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
                self.connect(s, region, EdgeKind::Context, name, from_port, Some(port));
            }
            self.regions[ri].env.insert(name.to_string(), Bind::Src(vec![port], style));
            return Some((vec![port], style));
        }
        let port = self.cx.b.add(Some(region), Kind::Input, Shape::Port, name);
        self.cx.b.get_mut(port).category = match style {
            Style::Data => "port",
            Style::Ctx => "context",
            Style::Def => "uses",
        };
        for s in srcs {
            let from_port = self.out_ports.get(&(s, name.to_string())).copied();
            let label = if style == Style::Def { "" } else { name };
            self.connect(s, region, kind_of(style, false), label, from_port, Some(port));
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
                self.cx.b.get_mut(p).category = "context";
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
        let kind = kind_of(v.style, v.scalar);
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
        let mut in_settings = false;
        for s in stmts {
            if self.handled.contains(&u32::from(s.start())) {
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
                    let u = if in_settings { 0 } else { 1 };
                    in_settings = true;
                    total += u;
                    continue;
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
            in_settings = false;
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
            let paras = self.paragraphs(stmts);
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
                        let mut j = start;
                        while j < end && self.is_setting(&stmts[j]) {
                            j += 1;
                        }
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
                    let t = c.text.trim_start_matches('#').trim();
                    t.len() >= 8 && t.chars().all(|ch| !ch.is_alphanumeric())
                });
                ruler || src.paragraph_break(prev_end, start_line)
            };
            if new_para {
                let comments = src.comments_above(start_line);
                let title = comment_title(&comments);
                let doc = comments.iter().map(|c| c.text.trim_start_matches('#').trim()).filter(|t| !t.chars().all(|c| !c.is_alphanumeric()) && !is_directive(t)).collect::<Vec<_>>().join(" ");
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

    /// A name for an untitled paragraph: "Setup" for library plumbing, else what it defines.
    fn generated_title(&self, stmts: &[Stmt]) -> String {
        let mut project = false;
        walk::stmt_exprs(stmts, false, &mut |e| {
            walk::walk(e, &mut |x| {
                if let Expr::Call(c) = x {
                    if matches!(self.classify(c), Callee::Func(_) | Callee::Ctor(_) | Callee::Instance { .. } | Callee::Component { .. } | Callee::ExecFile(_)) {
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
            if self.module_level && self.is_setting(&stmts[i]) {
                let mut j = i + 1;
                while j < stmts.len() && self.is_setting(&stmts[j]) {
                    j += 1;
                }
                let t = if i == 0 { title.clone() } else { None };
                let d = if i == 0 { doc.clone() } else { String::new() };
                let run: Vec<&'p Stmt> = stmts[i..j].iter().collect();
                self.settings(&run, t, d);
                i = j;
                continue;
            }
            // `session.add(x); session.commit()` and `parser.add_argument(…)` × 12
            // say one thing to a reader: fuse runs of calls on one receiver.
            let run = self.call_run(stmts, i);
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

    /// The receiver and method of a statement that is a library method call:
    /// `session.commit()`, `parser.add_argument(...)`.
    /// Also says whether the method is project code (`group.add_argument(...)`
    /// on a project class), which only fuses as a table of identical calls.
    fn receiver_call(&self, s: &Stmt) -> Option<(String, String, bool)> {
        let Stmt::Expr(e) = s else { return None };
        let Expr::Call(c) = &*e.value else { return None };
        let Expr::Attribute(a) = &*c.func else { return None };
        if self.is_print(c) || is_super(c) || contains_call(&a.value) || !matches!(&*a.value, Expr::Name(_) | Expr::Attribute(_)) || self.is_self(&a.value) {
            return None;
        }
        let recv = squash(self.src().of(&*a.value));
        match self.classify(c) {
            Callee::Method(_) | Callee::Ext(_) => Some((recv, a.attr.to_string(), false)),
            Callee::Func(_) => Some((recv, a.attr.to_string(), true)),
            _ => None,
        }
    }

    /// End of a run of at least two calls on one receiver starting at `i`.
    fn call_run(&self, stmts: &[Stmt], i: usize) -> usize {
        let Some((recv, method, _)) = self.receiver_call(&stmts[i]) else { return i };
        let mut j = i + 1;
        let mut project = false;
        while j < stmts.len() && !self.handled.contains(&u32::from(stmts[j].start())) {
            match self.receiver_call(&stmts[j]) {
                Some((r, m, p)) if r == recv => {
                    project |= p || self.receiver_call(&stmts[i]).is_some_and(|x| x.2);
                    if project && m != method {
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
        if made.len() < 2 {
            return;
        }
        let src = self.src();
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
            let args: Vec<String> = c.arguments.args.iter().map(|a| compact(src.of(a), 40)).chain(c.arguments.keywords.iter().map(|k| compact(src.of(k), 40))).collect();
            match &*c.func {
                Expr::Attribute(a) if target.is_none() => {
                    recv = squash(src.of(&*a.value));
                    methods.push(a.attr.to_string());
                }
                f => methods.push(squash(src.of(f))),
            }
            match target {
                Some(t) => {
                    rows.push(Row { key: t.clone(), value: clip(&args.join(", "), 48), note: String::new(), heading: false });
                    names.push(t);
                }
                None => {
                    let (key, rest) = match args.split_first() {
                        Some((k, r)) => (k.clone(), r.join(", ")),
                        None => ("()".into(), String::new()),
                    };
                    rows.push(Row { key, value: clip(&rest, 48), note: String::new(), heading: false });
                }
            }
        }
        let same = methods.windows(2).all(|w| w[0] == w[1]);
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
        } else {
            n.title = recv;
            n.subtitle = clip(&methods.join(" → "), 40);
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
            n.subtitle = format!("{} routes", rows.len());
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
                    let key = c.arguments.args.first().map(|a| string_literal(a).map(|t| if t.is_empty() { "/".to_string() } else { t.to_string() }).unwrap_or_else(|| compact(src.of(a), 40))).unwrap_or_default();
                    let value = c.arguments.args.get(1).map(|a| compact(src.of(a), 44)).unwrap_or_default();
                    Row { key, value, note: guard.map(|g| format!("if {g}")).unwrap_or_default(), heading: false }
                }
                e => Row { key: compact(src.of(e), 40), value: String::new(), note: guard.map(|g| format!("if {g}")).unwrap_or_default(), heading: false },
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
                let test = compact(src.of(&*x.test), 30);
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
            Stmt::Try(x) if !x.body.is_empty() && import_only(&x.body) && x.handlers.iter().all(|ast::ExceptHandler::ExceptHandler(h)| import_only(&h.body)) && import_only(&x.orelse) && import_only(&x.finalbody) => Some(Decl::Quiet),
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
            Expr::Lambda(_) | Expr::Await(_) | Expr::Yield(_) | Expr::Named(_) => ok = false,
            Expr::Name(x) if dynamic.contains(x.id.as_str()) => ok = false,
            Expr::Call(c) => {
                let callee = self.classify(c);
                let io = matches!(&callee, Callee::Ext(p) if effects::io_of(p, c, self.src()).is_some() || effects::ext_effect(p).is_some())
                    || matches!(&callee, Callee::Builtin(b) if matches!(b.as_str(), "open" | "input" | "exec" | "eval" | "print"));
                let allowed = match &callee {
                    Callee::Ctor(_) | Callee::Ext(_) => upper || !self.is_major(&callee, c),
                    Callee::Func(_) | Callee::Instance { .. } | Callee::Component { .. } | Callee::Value(_) | Callee::ExecFile(_) => false,
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
                Stmt::Assign(a) => (src.of(&a.targets[0]).to_string(), compact(src.of(&*a.value), 40)),
                Stmt::AnnAssign(a) => (src.of(&*a.target).to_string(), a.value.as_deref().map(|v| compact(src.of(v), 40)).unwrap_or_default()),
                Stmt::TypeAlias(t) => (src.of(&*t.name).to_string(), compact(src.of(&*t.value), 40)),
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
        let (target, value) = match s {
            Stmt::Assign(a) if a.targets.len() == 1 => (&a.targets[0], &*a.value),
            Stmt::AnnAssign(a) => match &a.value {
                Some(v) => (&*a.target, &**v),
                None => return false,
            },
            _ => return false,
        };
        matches!(target, Expr::Name(n) if !is_dunder(&n.id)) && self.literalish(value)
    }

    /// Constants, other settings, module attributes (`torch.float32`) and
    /// arithmetic or conditionals over them. Calls only inside a condition.
    fn literalish(&self, e: &Expr) -> bool {
        match e {
            e if is_constant(e) => true,
            Expr::Name(n) => match self.peek(&n.id) {
                Some(Bind::Src(_, Style::Ctx)) | Some(Bind::Const(_)) | Some(Bind::Static) => true,
                Some(Bind::Src(..)) => false,
                None => is_builtin(&n.id) || matches!(self.cx.ix.sym(self.mi, &n.id), Some(Sym::Ext(_)) | Some(Sym::Module(_))),
            },
            Expr::Attribute(a) => self.literalish(&a.value),
            Expr::UnaryOp(u) => self.literalish(&u.operand),
            Expr::BinOp(b) => self.literalish(&b.left) && self.literalish(&b.right),
            Expr::BoolOp(b) => b.values.iter().all(|v| self.literalish(v)),
            Expr::Compare(c) => c.operands.iter().all(|v| self.literalish(v)),
            Expr::If(x) => self.test_is_static(&x.test) && self.literalish(&x.body) && self.literalish(&x.orelse),
            Expr::List(l) => l.elts.iter().all(|v| self.literalish(v)),
            Expr::Tuple(l) => l.elts.iter().all(|v| self.literalish(v)),
            Expr::Set(l) => l.elts.iter().all(|v| self.literalish(v)),
            Expr::Dict(d) => d.items.iter().all(|i| i.key.as_ref().is_none_or(|k| self.literalish(k)) && self.literalish(&i.value)),
            Expr::Subscript(x) => self.literalish(&x.value) && self.literalish(&x.slice),
            Expr::FString(_) => {
                let mut kids = vec![];
                walk::children(e, &mut kids);
                kids.iter().all(|v| self.literalish(v))
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
            rows.push(Row { key: name.clone(), value: compact(src.of(value), 40), note: src.trailing_comment(line).unwrap_or("").to_string(), heading: false });
            names.push(name.clone());
            let ty = self.cx.ix.infer(self.mi, value, &TypesView(&self.types));
            if ty != Ty::Unknown {
                self.types.insert(name.clone(), ty.clone());
                self.cx.ix.var_types.borrow_mut().insert((self.mi, name.clone()), ty);
            }
            if let Expr::StringLiteral(c) = value {
                self.files.insert(name.clone(), effects::file_name(c.value.to_str()).into_iter().collect());
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
        let big = rows.iter().filter(|r| !r.heading).count() >= 8;
        let doc = match (&title, big) {
            (Some(t), true) if doc.is_empty() => t.clone(),
            _ => doc,
        };
        let title = match title {
            Some(t) if !big || t == "Constants" => t,
            _ if single => names[0].clone(),
            _ => "Settings".into(),
        };
        let id = self.add(Kind::Table, Shape::Box, title);
        // A table gathered from across a module points at its first row.
        let last = if contiguous { stmts[stmts.len() - 1].end() } else { stmts[0].end() };
        let span = self.src().span(TextRange::new(stmts[0].start(), last));
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
            Stmt::Return(r) => self.ret(r.value.as_deref(), "return"),
            Stmt::If(x) => self.if_stmt(s, x, later),
            Stmt::For(x) => self.for_stmt(s, &x.target, &x.iter, &x.body, later),
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
            Expr::Name(n) => matches!(n.id.as_str(), "print" | "pprint"),
            Expr::Attribute(a) => {
                let base = self.src().of(&*a.value);
                (matches!(base, "logging" | "logger" | "log" | "LOGGER" | "LOG" | "tqdm" | "sys.stdout" | "sys.stderr" | "warnings" | "pbar")
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
                let label = if matches!(&*c.func, Expr::Name(_)) { "prints" } else { "logs" };
                self.effect(label);
                // Reading values to print is not a dataflow worth drawing.
            }
            Expr::Call(c) if is_super(c) => {}
            Expr::Yield(y) => self.ret(y.value.as_deref(), "yield"),
            Expr::YieldFrom(y) => self.ret(Some(&y.value), "yield"),
            _ => {
                let mut g = Gather::default();
                let outcome = self.value(value, &mut g, "");
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
        let comment = self.src().trailing_comment(line).map(|c| c.to_string());
        let n = self.cx.b.get_mut(id);
        n.effects.extend(g.effects.iter().cloned());
        n.effects.sort();
        n.effects.dedup();
        if n.doc.is_empty() {
            if let Some(c) = comment {
                n.doc = c;
            }
        }
        if n.category == "op" && n.effects.iter().any(|e| e.starts_with("reads") || e.starts_with("writes") || e.contains("network")) {
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
            if !self.module_level && is_literal(value) {
                let text = compact(self.src().of(value), 40);
                for n in &names {
                    if let Some(f) = string_literal(value).and_then(effects::file_name) {
                        self.files.insert(n.clone(), vec![f]);
                    }
                    self.bind(n, Bind::Const(text.clone()));
                }
                return;
            }
            let mut g = Gather::default();
            let label = names.join(", ");
            let outcome = self.value(value, &mut g, &label);
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
                Outcome { alias: Some(v), .. } => {
                    for n in &names {
                        self.bind(n, Bind::Src(v.srcs.clone(), if v.style == Style::Def { Style::Data } else { v.style }));
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
            if self.in_init && !self.builds_component(value) {
                return;
            }
            // Component construction in __init__ reads as "c_attn: Linear".
            let mut g = Gather::default();
            let outcome = self.value(value, &mut g, "");
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
                        n.subtitle = if written.is_empty() { ctor } else { clip(&written, 28) };
                    }
                    n.title = attr.to_string();
                    if n.category == "library" || n.category == "class" || n.category == "component" {
                        if let Some(c) = name_category(&n.subtitle).or_else(|| name_category(attr)) {
                            n.category = c;
                        }
                    }
                } else {
                    let n = self.cx.b.get_mut(id);
                    n.title = format!("self.{attr} = …");
                }
                if !self.in_init {
                    self.cx.b.get_mut(id).effects.push(format!("mutates self.{attr}"));
                }
            } else if !self.in_init {
                // A plain state update outside the constructor is still worth showing.
                let vals = {
                    let mut g = Gather::default();
                    self.collect(value, &mut g, false)
                };
                if !vals.is_empty() {
                    let id = self.add(Kind::Op, Shape::Box, format!("self.{attr} = …"));
                    self.cx.b.get_mut(id).category = "op";
                    self.cx.b.get_mut(id).code = compact(self.src().of(s), 120);
                    self.cx.b.get_mut(id).effects.push(format!("mutates self.{attr}"));
                    self.set_span(id, s);
                    for v in vals {
                        self.link(&v, id);
                    }
                }
            }
            return;
        }
        // Writes into an object or container: `out[split] = ...`, `model.flag = ...`.
        let mut g = Gather::default();
        let mut vals = self.collect(value, &mut g, false);
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
        let text = squash(self.src().of(&targets[0]));
        let id = self.add(Kind::Op, Shape::Box, clip(&text, 30));
        self.cx.b.get_mut(id).category = if module_attr { "config" } else { "op" };
        self.cx.b.get_mut(id).code = compact(self.src().of(s), 160);
        if module_attr {
            g.effects.insert("sets library state".into());
        }
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
            let id = self.add(Kind::Op, Shape::Box, clip(&squash(self.src().of(s)), 30));
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

    fn ret(&mut self, value: Option<&'p Expr>, kind: &str) {
        let Some(v) = value else { return };
        let mut g = Gather::default();
        let vals: Vec<Val> = match v {
            Expr::Tuple(t) if t.elts.iter().all(|e| matches!(e, Expr::Name(_)) || is_constant(e)) => {
                let mut out = vec![];
                for e in &t.elts {
                    if let Expr::Name(n) = e {
                        out.extend(self.read(&n.id, &mut g));
                    }
                }
                out
            }
            _ => {
                let outcome = self.value(v, &mut g, "");
                if let Some(id) = outcome.node {
                    let stmt_range = v.range();
                    self.finish_node(id, &RangeOf(stmt_range), &g);
                    vec![Val { srcs: vec![id], label: String::new(), style: Style::Data, scalar: false }]
                } else if let Some(a) = outcome.alias {
                    vec![a]
                } else {
                    vec![]
                }
            }
        };
        let label_text = compact(self.src().of(v), 28);
        let r = self.regions.last_mut().unwrap();
        for mut val in vals {
            if val.label.is_empty() {
                val.label = String::new();
            }
            r.returns.push(val);
        }
        if r.returns.is_empty() {
            r.returns.push(Val { srcs: vec![], label: String::new(), style: Style::Data, scalar: false });
        }
        r.effects.insert(format!("__{kind}:{label_text}"));
    }

    fn raise_stmt(&mut self, s: &'p Stmt, r: &'p ast::StmtRaise) {
        let what = r
            .exc
            .as_ref()
            .map(|e| match &**e {
                Expr::Call(c) => squash(self.src().of(&*c.func)),
                other => squash(self.src().of(other)),
            })
            .unwrap_or_else(|| "re-raise".into());
        let id = self.add(Kind::Op, Shape::Box, format!("raise {}", clip(&what, 24)));
        self.cx.b.get_mut(id).category = "error";
        self.cx.b.get_mut(id).code = compact(self.src().of(s), 160);
        self.cx.b.get_mut(id).effects.push("raises".into());
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
                        self.connect(s, port, kind_of(v.style, v.scalar), &v.label, fp, None);
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
                        self.connect(s, port, kind_of(*style, false), label, fp, None);
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
            self.link(v, region);
        }
    }

    fn condition_vals(&mut self, test: &'p Expr) -> Vec<Val> {
        let mut g = Gather::default();
        let mut vals = self.collect(test, &mut g, false);
        dedup_vals(&mut vals);
        for v in &mut vals {
            v.scalar = true;
        }
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
        let mut lanes: Vec<(String, &'p [Stmt], Option<&'p Expr>, TextRange)> = vec![(format!("if {}", compact(self.src().of(&*x.test), 40)), &x.body, Some(&*x.test), x.range)];
        for c in &x.elif_else_clauses {
            match &c.test {
                Some(t) => lanes.push((format!("elif {}", compact(self.src().of(t), 40)), &c.body, Some(t), c.range)),
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
            self.statements(lanes[0].1, later, false);
            let new: Vec<Id> = self.cx.b.get(container).children.iter().copied().filter(|c| !before.contains(c) && !self.cx.b.get(*c).is_port()).collect();
            let badge = clip(&lanes[0].0, 34);
            for id in &new {
                self.cx.b.get_mut(*id).badges.push(badge.clone());
                for v in &cond {
                    self.link(v, *id);
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
            self.close_region(later);
        }
        self.close_region(later);
        self.link_condition(&cond, id);
    }

    fn for_stmt(&mut self, s: &'p Stmt, target: &'p Expr, iter: &'p Expr, body: &'p [Stmt], later: &HashSet<String>) {
        let title = format!("for {} in {}", compact(self.src().of(target), 16), compact(self.src().of(iter), 32));
        let iter_ty = self.cx.ix.infer(self.mi, iter, &TypesView(&self.types));
        let count = match &iter_ty {
            Ty::List(_, Some(n)) => Some(n.clone()),
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
        let title = if forever { "repeat".to_string() } else { format!("while {}", compact(self.src().of(test), 36)) };
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
                let outcome = self.value(e, &mut g, &names.join(", "));
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
        let trivial = handlers.iter().all(|ast::ExceptHandler::ExceptHandler(h)| self.units(&h.body) == 0) && self.units(finalbody) == 0;
        if trivial {
            self.statements(body, later, true);
            self.statements(orelse, later, true);
            return;
        }
        let id = self.open_region(Kind::Branch, String::new(), s);
        self.cx.b.get_mut(id).hints.lanes = true;
        self.regions.last_mut().unwrap().merging = true;
        let mut lanes: Vec<(String, &'p [Stmt])> = vec![("try".into(), body)];
        for ast::ExceptHandler::ExceptHandler(h) in handlers {
            let t = h.type_.as_ref().map(|t| compact(self.src().of(&**t), 28)).unwrap_or_else(|| "Exception".into());
            lanes.push((format!("except {t}"), &h.body));
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
            self.regions.push(Region::new(lane));
            self.statements(b, later, true);
            self.close_region(later);
        }
        self.close_region(later);
    }

    fn match_stmt(&mut self, s: &'p Stmt, x: &'p ast::StmtMatch, later: &HashSet<String>) {
        let cond = self.condition_vals(&x.subject);
        let id = self.open_region(Kind::Branch, format!("match {}", compact(self.src().of(&*x.subject), 30)), s);
        self.cx.b.get_mut(id).hints.lanes = true;
        self.regions.last_mut().unwrap().merging = true;
        for case in &x.cases {
            let title = format!("case {}", compact(self.src().of(&case.pattern), 30));
            let lane = self.add(Kind::Lane, Shape::Region, title);
            self.cx.b.get_mut(lane).category = "lane";
            self.regions.push(Region::new(lane));
            self.statements(&case.body, later, true);
            self.close_region(later);
        }
        self.close_region(later);
        for v in &cond {
            self.link(v, id);
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
        let params = f.params();
        let free = walk::free_names(&params, f.body);
        let mut outer = HashSet::new();
        for name in free {
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
        self.types.insert(f.name.to_string(), Ty::Func(fid));
        self.bind(f.name, Bind::Src(vec![node], Style::Def));
        let types = self.types.clone();
        build_function(self.cx, fid, outer, types);
    }

    fn describe_function(&mut self, fid: usize, s: &'p Stmt) {
        let info = &self.cx.ix.funcs[fid];
        let def = info.def;
        let node = info.node;
        let kind = info.kind;
        let is_method = info.class.is_some() && kind != FnKind::StaticMethod;
        let mut params = def.params();
        if is_method && !params.is_empty() {
            params.remove(0);
        }
        let doc = def.docstring().map(summary).unwrap_or_else(|| {
            let line = stmt_start_line(self.src(), s);
            let c: Vec<&str> = self.src().comments_above(line).iter().map(|c| c.text.trim_start_matches('#').trim()).collect();
            summary(&c.join(" "))
        });
        let span = self.src().span(s.range());
        let mut badges = vec![];
        let mi = self.mi;
        if !self.cx.prefixes.contains_key(&mi) {
            let p = frameworks::prefixes(&self.m.ast);
            self.cx.prefixes.insert(mi, p);
        }
        let src = self.src();
        let mut role = None;
        for d in def.decorators {
            match frameworks::role(&d.expression, def.name, &self.cx.prefixes[&mi], |e| compact(src.of(e), 24)) {
                Some(r) if role.is_none() => role = Some(r),
                Some(_) => {}
                None => badges.extend(frameworks::tag(d, |e| compact(src.of(e), 24))),
            }
        }
        match kind {
            FnKind::ClassMethod => badges.push("classmethod".into()),
            FnKind::StaticMethod => badges.push("static".into()),
            FnKind::Property => badges.push("property".into()),
            _ => {}
        }
        if def.is_async {
            badges.push("async".into());
        }
        if let Some(r) = &role {
            if !self.cx.handlers[mi].contains(&r.kind) {
                self.cx.handlers[mi].push(r.kind);
            }
        }
        let n = self.cx.b.get_mut(node);
        n.kind = Kind::Function;
        n.shape = Shape::Box;
        n.title = def.name.to_string();
        n.subtitle = match &role {
            Some(r) => r.label.clone(),
            None if def.name == "__init__" => "constructor".into(),
            None => format!("({})", clip(&params.join(", "), 40)),
        };
        n.category = if role.as_ref().is_some_and(|r| r.kind != Handler::Hook) { "endpoint" } else { "function" };
        n.doc = doc;
        n.span = Some(span);
        n.badges = badges;
        n.inputs = params;
    }

    fn class_stmt(&mut self, c: &'p ast::StmtClassDef) {
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
        for e in class_bases(c) {
            for n in walk::loads(e) {
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
                match self.cx.ix.sym_ty(self.mi, name, 0) {
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
                if self.is_self(&a.value) {
                    let (_, cid) = self.this.clone().unwrap();
                    if let Some(f) = self.cx.ix.method(cid, attr) {
                        return Callee::Func(f);
                    }
                    if let Some(field) = self.cx.ix.field(cid, attr) {
                        if is_callable_ty(&field.ty) {
                            return Callee::Component { name: attr.into(), ty: field.ty.clone() };
                        }
                    }
                    return Callee::Method(attr.into());
                }
                match self.ty(&a.value) {
                    Ty::Module(m) => match self.cx.ix.sym_ty(m, attr, 0) {
                        Ty::Class(k) => Callee::Ctor(k),
                        Ty::Func(f) => Callee::Func(f),
                        Ty::Ext(p) => Callee::Ext(p),
                        _ => Callee::Unknown,
                    },
                    Ty::Ext(p) => Callee::Ext(format!("{p}.{attr}")),
                    Ty::Inst(k) => match self.cx.ix.method(k, attr) {
                        Some(f) => Callee::Func(f),
                        None => match self.cx.ix.field(k, attr) {
                            Some(field) if is_callable_ty(&field.ty) => Callee::Component { name: attr.into(), ty: field.ty.clone() },
                            _ => Callee::Method(attr.into()),
                        },
                    },
                    Ty::Class(k) => match self.cx.ix.method(k, attr) {
                        Some(f) => Callee::Func(f),
                        None => Callee::Method(attr.into()),
                    },
                    Ty::Dict(fields) => match fields.into_iter().find(|(k, _)| k == attr) {
                        Some((_, t)) if is_callable_ty(&t) => Callee::Component { name: attr.into(), ty: t },
                        _ => Callee::Method(attr.into()),
                    },
                    _ => Callee::Method(attr.into()),
                }
            }
            other => match self.ty(other) {
                Ty::Inst(k) => Callee::Instance { class: k, var: compact(self.src().of(other), 20) },
                Ty::Func(f) => Callee::Func(f),
                Ty::Class(k) => Callee::Ctor(k),
                _ => Callee::Unknown,
            },
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
        match callee {
            Callee::Func(_) | Callee::Ctor(_) | Callee::Instance { .. } | Callee::Component { .. } | Callee::Value(_) | Callee::ExecFile(_) => true,
            Callee::Ext(p) => !MINOR_EXT.iter().any(|m| p.starts_with(m) || p == m.trim_end_matches('.')) || effects::io_of(p, c, self.src()).is_some(),
            Callee::Builtin(b) => matches!(b.as_str(), "open" | "input" | "exec" | "eval"),
            Callee::Method(m) => effects::method_io(m).is_some(),
            Callee::Super | Callee::Unknown => false,
        }
    }

    fn scalar_result(callee: &Callee) -> bool {
        let name = match callee {
            Callee::Ext(p) => p.rsplit('.').next().unwrap_or(p),
            Callee::Builtin(b) | Callee::Method(b) => b.as_str(),
            _ => return false,
        };
        SCALAR_FNS.contains(&name) || matches!(callee, Callee::Ext(p) if p.starts_with("math."))
    }

    /// Collects the dataflow sources of an expression, creating blocks for major calls.
    fn collect(&mut self, e: &'p Expr, g: &mut Gather, fold: bool) -> Vec<Val> {
        match e {
            Expr::Name(n) => {
                if n.ctx == ast::ExprContext::Load {
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
                if text == "os.environ" || text.starts_with("os.environ.") {
                    g.effects.insert("reads environment".into());
                }
                if text == "sys.argv" {
                    g.effects.insert("reads CLI args".into());
                }
                self.collect(&a.value, g, fold)
            }
            Expr::Call(c) => {
                let callee = self.classify(c);
                if !fold && self.is_major(&callee, c) {
                    let id = self.call_node(e, &callee, g);
                    return vec![Val { srcs: vec![id], label: String::new(), style: Style::Data, scalar: Self::scalar_result(&callee) }];
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
                let name = callee_name(&callee, self);
                let mut out = vec![];
                if let Expr::Attribute(a) = &*c.func {
                    out.extend(self.collect(&a.value, g, fold));
                } else if let Callee::Value(v) = &callee {
                    out.extend(self.read(v, g));
                }
                if !name.is_empty() {
                    g.minor_calls.push(name);
                }
                for a in &c.arguments.args {
                    out.extend(self.collect(a, g, fold));
                }
                let dict_like = matches!(&callee, Callee::Builtin(b) if b == "dict");
                for k in &c.arguments.keywords {
                    let vs = self.collect(&k.value, g, fold);
                    if dict_like {
                        self.name_by_key(k.arg.as_deref(), &k.value, &vs);
                    }
                    out.extend(vs);
                }
                if Self::scalar_result(&callee) {
                    for v in &mut out {
                        v.scalar = true;
                    }
                }
                out
            }
            Expr::Lambda(_) | Expr::ListComp(_) | Expr::SetComp(_) | Expr::DictComp(_) | Expr::Generator(_) => {
                let mut out = vec![];
                for n in walk::loads(e) {
                    out.extend(self.read(&n, g));
                }
                walk::walk(e, &mut |x| {
                    if let Expr::Call(c) = x {
                        if let Expr::Name(n) = &*c.func {
                            if n.id.as_str() == "print" {
                                g.effects.insert("prints".into());
                            }
                        }
                    }
                });
                out
            }
            Expr::Named(n) => {
                let vals = self.collect(&n.value, g, fold);
                if let Expr::Name(t) = &*n.target {
                    if let Some(v) = vals.first() {
                        let st = v.style;
                        let srcs = v.srcs.clone();
                        self.bind(&t.id, Bind::Src(srcs, st));
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
        for a in &c.arguments.args {
            vals.extend(self.collect(a, &mut inner, exec_file));
        }
        for k in &c.arguments.keywords {
            let vs = self.collect(&k.value, &mut inner, exec_file);
            if container_ctor {
                self.name_by_key(k.arg.as_deref(), &k.value, &vs);
            }
            vals.extend(vs);
        }
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
                let cat = name_category(&info.name).unwrap_or("function");
                (info.name.clone(), owner, cat, Some(info.node))
            }
            Callee::Ctor(k) => {
                let cls = &self.cx.ix.classes[*k];
                link = Some(cls.node);
                (cls.name.clone(), "new instance".into(), name_category(&cls.name).unwrap_or("class"), None)
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
                (title, sub, name_category(&cls.name).or_else(|| name_category(var)).unwrap_or("component"), reference)
            }
            Callee::Component { name, ty } => {
                let display = self.cx.ix.display(ty);
                let reference = match ty {
                    Ty::Inst(k) => self.cx.ix.call_target(*k).map(|f| self.cx.ix.funcs[f].node).or(Some(self.cx.ix.classes[*k].node)),
                    _ => None,
                };
                let project = matches!(ty, Ty::Inst(_));
                let cat = name_category(&display).or_else(|| name_category(name)).unwrap_or(if project { "component" } else { "library" });
                (name.clone(), display, cat, reference)
            }
            Callee::Ext(p) => {
                let resolved = p.rsplit('.').next().unwrap_or(p).to_string();
                if frameworks::APPS.contains(&resolved.as_str()) && !resolved.is_empty() {
                    self.cx.app[self.mi] = true;
                }
                let (prefix, last) = match func_text.rsplit_once('.') {
                    Some((a, b)) => (a.to_string(), b.to_string()),
                    None => (String::new(), func_text.clone()),
                };
                let cat = if effects::io_of(p, c, src).is_some() || effects::ext_effect(p).is_some_and(|e| e.contains("network") || e.contains("file")) {
                    "io"
                } else {
                    name_category(&last).or_else(|| name_category(&resolved)).unwrap_or("library")
                };
                let mut sub = if prefix.is_empty() && resolved != last { resolved.clone() } else { prefix };
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
                (last, clip(&sub, 28), cat, None)
            }
            Callee::Builtin(b) => (b.clone(), String::new(), if b == "open" || b == "input" { "io" } else { "op" }, None),
            Callee::Value(v) => (v.clone(), String::new(), "function", None),
            Callee::Method(m) => {
                let recv = match &*c.func {
                    Expr::Attribute(a) => compact(src.of(&*a.value), 20),
                    _ => String::new(),
                };
                (m.clone(), recv, if effects::method_io(m).is_some() { "io" } else { "op" }, None)
            }
            Callee::ExecFile(m) => {
                let path = self.cx.ix.modules[*m].path.clone();
                self.cx.relations.push(Relation::Exec { from: self.mi, to: *m });
                node_effects.insert("executes code".into());
                (path.rsplit('/').next().unwrap_or(&path).to_string(), "exec".into(), "config", Some(self.cx.file_nodes[*m]))
            }
            Callee::Super | Callee::Unknown => (func_text.clone(), String::new(), "op", None),
        };
        // I/O: effects and the files touched.
        let mut files: Vec<String> = vec![];
        for a in c.arguments.args.iter().chain(c.arguments.keywords.iter().map(|k| &k.value)).filter(|_| !exec_file) {
            walk::walk(a, &mut |x| {
                if let Some(s) = string_literal(x).and_then(effects::file_name) {
                    files.push(s);
                }
                if let Expr::FString(j) = x {
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
            Callee::Method(m) => effects::method_io(m),
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
        if let Callee::Builtin(b) = callee {
            if b == "exec" || b == "eval" {
                node_effects.insert("executes code".into());
            }
            if b == "input" {
                node_effects.insert("reads stdin".into());
            }
        }

        let id = self.add(Kind::Call, Shape::Box, title);
        {
            let span = src.span(c.range());
            let n = self.cx.b.get_mut(id);
            n.subtitle = subtitle;
            n.category = category;
            n.reference = reference;
            n.link = link.or(reference);
            n.code = compact(src.of(c), 200);
            n.span = Some(span);
            n.effects = node_effects.into_iter().collect();
        }
        for v in &vals {
            self.link(v, id);
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
                            self.connect(s, id, EdgeKind::Context, "globals", None, None);
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
    fn value(&mut self, e: &'p Expr, g: &mut Gather, target: &str) -> Outcome {
        match e {
            Expr::Name(n) => {
                let vals = self.read(&n.id, g);
                return Outcome { alias: vals.into_iter().next(), ..Outcome::none() };
            }
            Expr::Attribute(_) | Expr::Subscript(_) if root_name(e).is_some() && !contains_call(e) => {
                let mut vals = self.collect(e, g, false);
                dedup_vals(&mut vals);
                if vals.len() == 1 {
                    let mut v = vals.remove(0);
                    v.scalar = false;
                    return Outcome { alias: Some(v), ..Outcome::none() };
                }
                if vals.is_empty() {
                    return Outcome::none();
                }
                let id = self.fused(e, vals, g, target);
                return Outcome { node: Some(id), ..Outcome::none() };
            }
            e if is_constant(e) => return Outcome::none(),
            Expr::Await(a) => {
                g.effects.insert("awaits".into());
                return self.value(&a.value, g, target);
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
                    let id = self.fused(e, vals, g, target);
                    self.cx.b.get_mut(id).title = clip(&label, 28);
                    self.cx.b.get_mut(id).category = "library";
                    return Outcome { node: Some(id), ..Outcome::none() };
                }
                let (call, other, negate) = if simple(&x.orelse) { (&*x.body, &*x.orelse, false) } else { (&*x.orelse, &*x.body, true) };
                if simple(other) {
                    if let Expr::Call(c) = call {
                        let callee = self.classify(c);
                        if self.is_major(&callee, c) {
                            let cond = self.condition_vals(&x.test);
                            let id = self.call_node(call, &callee, g);
                            let test = compact(self.src().of(&*x.test), 28);
                            let badge = if negate { format!("unless {test}") } else { format!("if {test}") };
                            self.cx.b.get_mut(id).badges.push(badge);
                            for v in &cond {
                                self.link(v, id);
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
            if Self::scalar_result(&callee) {
                let mut vals = self.collect(e, g, false);
                for v in &mut vals {
                    v.scalar = v.style == Style::Ctx;
                }
                dedup_vals(&mut vals);
                if vals.is_empty() {
                    return Outcome::none();
                }
                let id = self.fused(e, vals, g, target);
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
                let id = self.fused(e, vals, g, target);
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
        let made_here = |s: Id| s >= first_new;
        if vals.len() == 1 && vals[0].srcs.len() == 1 && made_here(vals[0].srcs[0]) && is_glue(e) {
            return Outcome { node: Some(vals[0].srcs[0]), ..Outcome::none() };
        }
        let scalar = !vals.is_empty() && vals.iter().all(|v| v.scalar);
        let id = self.fused(e, vals, g, target);
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
        if let Some(cat) = name_category(&ctor).or_else(|| name_category(key)) {
            n.category = cat;
        }
    }

    fn fused(&mut self, e: &'p Expr, vals: Vec<Val>, g: &Gather, target: &str) -> Id {
        let src = self.src();
        let label = fused_label(e, src, g, target);
        let id = self.add(Kind::Op, Shape::Box, label);
        let span = src.span(e.range());
        {
            let n = self.cx.b.get_mut(id);
            n.category = "op";
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
                    self.connect(s, port, kind_of(v.style, v.scalar), &v.label, fp, None);
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
    let mut fb = Fb::new(cx, mi, node);
    fb.types = types;
    fb.outer = outer;
    fb.in_init = def.name == "__init__";
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
        let annotation = def.args.posonlyargs.iter().chain(&def.args.args).chain(&def.args.kwonlyargs).find(|a| a.parameter.name.as_str() == p).and_then(|a| a.parameter.annotation.as_deref());
        match annotation.map(|a| fb.cx.ix.infer(mi, a, &TypesView(&fb.types))) {
            Some(Ty::Class(k)) => {
                fb.types.insert(p.clone(), Ty::Inst(k));
            }
            _ => {
                fb.types.remove(p);
            }
        }
    }
    let body: &[Stmt] = if def.docstring().is_some() { &def.body[1..] } else { def.body };
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
    let modules: &'p [PyModule] = cx.ix.modules;
    let src = &modules[mi].src;
    let ext_base = cx.ix.external_base(cid);
    let project_bases: Vec<String> = cx.ix.classes[cid].bases.iter().filter_map(|b| if let Ty::Class(k) = b { Some(cx.ix.classes[*k].name.clone()) } else { None }).collect();
    {
        let name = cx.ix.classes[cid].name.clone();
        let doc = docstring(&def.body).map(summary).unwrap_or_else(|| {
            let line = src.line_of(usize::from(def.range.start()));
            summary(&src.comments_above(line).iter().map(|c| c.text.trim_start_matches('#').trim()).collect::<Vec<_>>().join(" "))
        });
        let span = src.span(def.range);
        let data = cx.ix.classes[cid].data;
        let n = cx.b.get_mut(node);
        n.kind = Kind::Class;
        n.shape = Shape::Box;
        n.title = name.clone();
        let mut sub: Vec<String> = project_bases.clone();
        if let Some(e) = &ext_base {
            sub.push(e.clone());
        }
        n.subtitle = sub.join(", ");
        n.category = if data { "config" } else { name_category(&name).unwrap_or("class") };
        n.doc = doc;
        n.span = Some(span);
        for d in &def.decorator_list {
            n.badges.push(format!("@{}", compact(src.of(d), 20)));
        }
    }
    if cx.ix.classes[cid].data {
        let mut rows = vec![];
        for s in &def.body {
            match s {
                Stmt::AnnAssign(a) => rows.push(Row {
                    key: compact(src.of(&*a.target), 30),
                    value: match &a.value {
                        Some(v) => format!("{} = {}", compact(src.of(&*a.annotation), 16), compact(src.of(&**v), 30)),
                        None => compact(src.of(&*a.annotation), 30),
                    },
                    note: src.trailing_comment(src.line_of(usize::from(a.range.start()))).unwrap_or("").to_string(),
                    heading: false,
                }),
                Stmt::Assign(a) => rows.push(Row { key: compact(src.of(&a.targets[0]), 30), value: compact(src.of(&*a.value), 34), note: String::new(), heading: false }),
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
            component_node(cx, group, &name, &ty, &ctor, Some(src.span(range)));
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

fn component_node(cx: &mut Ctx, parent: Id, name: &str, ty: &Ty, ctor: &str, span: Option<Span>) {
    if let Ty::Dict(fields) = ty {
        let group = cx.b.add(Some(parent), Kind::Group, Shape::Region, name);
        cx.b.get_mut(group).category = "region";
        cx.b.get_mut(group).span = span.clone();
        for (k, t) in fields {
            if is_component(t) {
                component_node(cx, group, k, t, "", span.clone());
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
    let project = reference.is_some();
    let n = cx.b.get_mut(id);
    n.subtitle = if display.is_empty() { compact(ctor, 24) } else { display.clone() };
    n.category = name_category(&display).or_else(|| name_category(name)).unwrap_or(if project { "component" } else { "library" });
    n.link = reference;
    n.span = span;
}

fn is_component(t: &Ty) -> bool {
    match t {
        Ty::Inst(_) | Ty::ExtInst(_) => true,
        Ty::Ext(p) => p == "buffer",
        Ty::Dict(f) => f.iter().any(|(_, t)| is_component(t)),
        Ty::List(e, _) => is_component(e),
        _ => false,
    }
}

fn is_callable_ty(t: &Ty) -> bool {
    matches!(t, Ty::Inst(_) | Ty::ExtInst(_))
}

fn kind_of(style: Style, scalar: bool) -> EdgeKind {
    match style {
        Style::Data if scalar => EdgeKind::Context,
        Style::Data => EdgeKind::Data,
        Style::Ctx => EdgeKind::Context,
        Style::Def => EdgeKind::Uses,
    }
}

fn dedup_vals(vals: &mut Vec<Val>) {
    let mut seen: Vec<(Vec<Id>, String)> = vec![];
    vals.retain(|v| {
        if v.srcs.is_empty() {
            return false;
        }
        let key = (v.srcs.clone(), v.label.clone());
        if seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });
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
    clip(&labels.join(", "), 32)
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

fn is_super(c: &ast::ExprCall) -> bool {
    if let Expr::Attribute(a) = &*c.func {
        if let Expr::Call(inner) = &*a.value {
            return matches!(&*inner.func, Expr::Name(n) if n.id.as_str() == "super");
        }
    }
    false
}

fn is_main_guard(test: &Expr) -> bool {
    if let Expr::Compare(c) = test {
        let parts: Vec<&Expr> = c.operands.iter().collect();
        let has_name = parts.iter().any(|e| matches!(e, Expr::Name(n) if n.id.as_str() == "__name__"));
        let has_main = parts.iter().any(|e| string_literal(e) == Some("__main__"));
        return has_name && has_main;
    }
    false
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
        return Some(s.value.to_str());
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

/// Glue that does not deserve its own block around a single major call:
/// tuple packing, subscripts, attribute access and unpacking.
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
        Stmt::Try(x) => import_only(&x.body) && x.handlers.iter().all(|ast::ExceptHandler::ExceptHandler(h)| import_only(&h.body)) && import_only(&x.orelse),
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
        .map(|c| c.text.trim_start_matches('#').trim())
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
        Stmt::FunctionDef(f) => &f.decorator_list,
        Stmt::ClassDef(c) => &c.decorator_list,
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
        out.extend(walk::loads(e))
    });
    out
}

/// Expressions that compute a path (and so carry file names to a variable).
fn is_path_expr(e: &Expr) -> bool {
    match e {
        Expr::StringLiteral(_) | Expr::FString(_) | Expr::Name(_) => true,
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
        Callee::Ext(p) => p.rsplit('.').next().unwrap_or(p).to_string(),
        Callee::Builtin(b) | Callee::Method(b) => b.clone(),
        Callee::Func(f) => fb.cx.ix.funcs[*f].name.clone(),
        _ => String::new(),
    }
}

fn g_strings_files(fb: &Fb, e: &Expr) -> Vec<String> {
    let mut out = vec![];
    walk::walk(e, &mut |x| {
        if let Some(s) = string_literal(x).and_then(effects::file_name) {
            out.push(s);
        }
        if let Expr::FString(j) = x {
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

/// Label for a fused statement block: the call chain when there is one
/// (`view · transpose`), otherwise the expression itself.
fn fused_label(e: &Expr, src: &super::source::Source, g: &Gather, target: &str) -> String {
    let mut calls: Vec<String> = vec![];
    for c in &g.minor_calls {
        if !calls.contains(c) && !matches!(c.as_str(), "len" | "int" | "float" | "str" | "range" | "list" | "dict" | "tuple" | "join" | "dirname" | "min" | "max" | "isinstance" | "hasattr" | "getattr") {
            calls.push(c.clone());
        }
    }
    match e {
        Expr::Dict(d) => {
            let keys: Vec<String> = d.items.iter().filter_map(|i| i.key.as_ref().and_then(string_literal).map(|s| s.to_string())).collect();
            if !keys.is_empty() {
                let more = if keys.len() > 3 { format!(", +{}", keys.len() - 3) } else { String::new() };
                return format!("{{{}{}}}", keys.iter().take(3).cloned().collect::<Vec<_>>().join(", "), more);
            }
        }
        Expr::Call(c) if matches!(&*c.func, Expr::Name(n) if n.id.as_str() == "dict") => {
            let keys: Vec<String> = c.arguments.keywords.iter().filter_map(|k| k.arg.as_ref().map(|a| a.to_string())).collect();
            if !keys.is_empty() {
                let more = if keys.len() > 3 { format!(", +{}", keys.len() - 3) } else { String::new() };
                return format!("{{{}{}}}", keys.iter().take(3).cloned().collect::<Vec<_>>().join(", "), more);
            }
        }
        _ => {}
    }
    if !calls.is_empty() && calls.len() <= 3 && !matches!(e, Expr::BinOp(_) | Expr::Compare(_) | Expr::BoolOp(_)) {
        return calls.join(" · ");
    }
    let _ = target;
    clip(&squash(src.of(e)), 28)
}
