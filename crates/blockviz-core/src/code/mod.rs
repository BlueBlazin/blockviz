//! Program analysis shared by every language: front ends lower their syntax
//! into `syntax`, then this indexes symbols project-wide and builds one
//! dataflow diagram per module, class and function.

pub mod contracts;
pub mod effects;
pub mod flow;
pub mod frameworks;
pub mod index;
pub mod names;
pub mod source;
pub mod syntax;
pub mod walk;

use crate::hash::{Map, Set};
use crate::ir::*;
use crate::text::summary;
use crate::vocabulary::{self, Domains};
use flow::{Ctx, Fb, Relation};
use index::{docstring, Index};
pub use index::Module;
use std::collections::{BTreeSet, HashMap};
use syntax::Lang;

/// A module as the repository level sees it.
pub struct CodeFile {
    pub file: u32,
    pub node: Id,
    pub lang: Lang,
    pub entry: bool,
    pub doc: String,
    pub imports: Vec<(u32, Vec<String>)>,
    /// The modules that define what it imports (through re-exports), with
    /// the names: what it depends on.
    pub uses: Vec<(u32, Vec<String>)>,
    /// Project files that are not code it imports: stylesheets, data, images.
    pub assets: Vec<String>,
    pub externals: BTreeSet<String>,
    pub error: Option<String>,
    /// Creates the application object a server or CLI runs (`app = FastAPI()`).
    pub app: bool,
    /// Framework roles of its functions: routes, commands, tasks, hooks.
    pub handlers: Vec<frameworks::Handler>,
}

#[derive(Default)]
pub struct CodeResult {
    pub files: Vec<CodeFile>,
    pub execs: Vec<(u32, u32)>,
    /// (file, artifact name, is write)
    pub artifacts: Vec<(u32, String, bool)>,
    /// (file, template path as written)
    pub templates: Vec<(u32, String)>,
}

/// What a module's code reads: every name, in statements, nested
/// definitions, functions written as values and annotations; and the
/// members it reads of names (`store.Open` is `("store", "Open")`), in order.
#[derive(Default)]
struct Used<'a> {
    names: Set<&'a str>,
    members: Vec<(&'a str, &'a str)>,
}

fn names_used(body: &[syntax::Stmt]) -> Used<'_> {
    use syntax::{Expr, Stmt};
    fn expr<'a>(e: &'a Expr, out: &mut Used<'a>) {
        walk::walk(e, &mut |x| match x {
            Expr::Name(n) => {
                out.names.insert(&n.id);
            }
            Expr::Attribute(a) => {
                if let Expr::Name(n) = &*a.value {
                    out.members.push((&n.id, &a.attr));
                }
            }
            Expr::Function(f) => stmts(&f.body, out),
            _ => {}
        });
    }
    fn stmts<'a>(body: &'a [Stmt], out: &mut Used<'a>) {
        let mut roots = vec![];
        walk::stmt_exprs(body, false, &mut |e| roots.push(e));
        for e in roots {
            expr(e, out);
        }
        defs(body, out);
    }
    /// What `stmt_exprs` leaves out, in the control flow it already went
    /// through: definitions' annotations and bodies.
    fn defs<'a>(body: &'a [Stmt], out: &mut Used<'a>) {
        for s in body {
            match s {
                Stmt::FunctionDef(f) => {
                    for a in f.params.iter().filter_map(|p| p.annotation.as_ref()).chain(f.returns.as_deref()) {
                        expr(a, out);
                    }
                    stmts(&f.body, out);
                }
                Stmt::ClassDef(c) => stmts(&c.body, out),
                Stmt::AnnAssign(a) => expr(&a.annotation, out),
                Stmt::If(x) => {
                    defs(&x.body, out);
                    x.elif_else_clauses.iter().for_each(|c| defs(&c.body, out));
                }
                Stmt::For(x) => defs(&x.body, out),
                Stmt::While(x) => defs(&x.body, out),
                Stmt::With(x) => defs(&x.body, out),
                Stmt::Try(x) => {
                    defs(&x.body, out);
                    x.handlers.iter().for_each(|h| defs(&h.body, out));
                    defs(&x.orelse, out);
                    defs(&x.finalbody, out);
                }
                Stmt::Match(x) => x.cases.iter().for_each(|c| defs(&c.body, out)),
                _ => {}
            }
        }
    }
    let mut out = Used::default();
    stmts(body, &mut out);
    // Each member where it is first read.
    let mut seen = Set::default();
    out.members.retain(|m| seen.insert(*m));
    out
}

/// A file node for a module the front end could not analyze.
pub fn failed_file(b: &mut Builder, file: u32, path: &str, lang: Lang, notebook: bool, why: String) -> CodeFile {
    let node = b.add(None, if notebook { Kind::Notebook } else { Kind::File }, Shape::Box, path.rsplit('/').next().unwrap_or(path));
    b.get_mut(node).badges.push("too complex".into());
    CodeFile { file, node, lang, entry: false, doc: String::new(), imports: vec![], uses: vec![], assets: vec![], externals: BTreeSet::new(), error: Some(why), app: false, handlers: vec![] }
}

/// Builds diagrams for every module. `headers` are the comment blocks that
/// open each module, used when it has no docstring.
pub fn analyze(b: Builder, modules: Vec<Module>, headers: Vec<String>, failed: Vec<CodeFile>) -> (Builder, CodeResult) {
    let mut b = b;
    let file_nodes: Vec<Id> = modules
        .iter()
        .zip(&headers)
        .map(|(m, header)| {
            let title = m.path.rsplit('/').next().unwrap_or(&m.path).to_string();
            let id = b.add(None, if m.notebook { Kind::Notebook } else { Kind::File }, Shape::Box, title);
            let n = b.get_mut(id);
            n.category = "file";
            n.span = Some(Span { file: m.file, start: 0, end: m.src.text.len() as u32, line: 1, end_line: m.src.text.lines().count().max(1) as u32 });
            n.doc = docstring(&m.ast).map(summary).unwrap_or_else(|| header.clone());
            if m.error.is_some() {
                n.badges.push("syntax error".into());
            }
            n.badges.extend(m.tags.iter().cloned());
            id
        })
        .collect();
    let ix = Index::build(&modules, |name, class| b.add(None, if class { Kind::Class } else { Kind::Function }, Shape::Box, name));
    let module_by_path: HashMap<String, usize> = modules.iter().enumerate().map(|(i, m)| (m.path.clone(), i)).collect();
    let n = modules.len();
    let mut imported = vec![false; n];
    for (mi, imps) in ix.imports.iter().enumerate() {
        for (t, _) in imps {
            if *t != mi {
                imported[*t] = true;
            }
        }
    }
    // A module works in the domains of the libraries it imports, and of those
    // its project imports use (views that call a `db.py` work with a database).
    let direct: Vec<Domains> = ix.externals.iter().map(|ext| ext.iter().fold(Domains::default(), |d, e| d.union(vocabulary::library_domains(e)))).collect();
    // Modules that only re-export (`index.js` barrels) pass their modules' domains through.
    let barrel: Vec<bool> = modules.iter().map(|m| !m.ast.is_empty() && m.ast.iter().all(|s| matches!(s, syntax::Stmt::Import(_) | syntax::Stmt::ImportFrom(_) | syntax::Stmt::Export(_)))).collect();
    let reach = |mi: usize| {
        let mut d = direct[mi];
        let mut stack: Vec<usize> = ix.imports[mi].iter().map(|(t, _)| *t).collect();
        let mut seen = Set::default();
        while let Some(t) = stack.pop() {
            if !seen.insert(t) || seen.len() > 64 {
                continue;
            }
            d = d.union(direct[t]);
            if barrel[t] {
                stack.extend(ix.imports[t].iter().map(|(u, _)| *u));
            }
        }
        d
    };
    let mut domains: Vec<Domains> = (0..n).map(reach).collect();
    // The files of a namespace (a Go package) work in one set of domains.
    let mut shared: Vec<Domains> = vec![Domains::default(); ix.namespaces.len()];
    for (mi, ns) in ix.namespace_of.iter().enumerate() {
        if let Some(ns) = ns {
            shared[*ns] = shared[*ns].union(domains[mi]);
        }
    }
    for (mi, ns) in ix.namespace_of.iter().enumerate() {
        if let Some(ns) = ns {
            domains[mi] = domains[mi].union(shared[*ns]);
        }
    }
    let mounts = frameworks::mounts(&modules, &ix);
    let routes = frameworks::routes(&modules, &ix);
    let mut cx = Ctx { ix, b, file_nodes, relations: vec![], entry: vec![false; n], module_by_path, imported, app: vec![false; n], handlers: vec![vec![]; n], prefixes: HashMap::new(), mounts, domains, routes, components: HashMap::new(), component_calls: vec![], reads: HashMap::new(), scopes: vec![] };
    for mi in 0..n {
        Fb::build_module(&mut cx, mi);
    }
    // A component lists the blocks that call it, wherever they are drawn.
    for (k, path, call) in std::mem::take(&mut cx.component_calls) {
        if let Some(&c) = cx.components.get(&(k, path)) {
            cx.b.get_mut(c).used_by.push(call);
        }
    }
    // Code that is just the source at a node's span is read there by the page.
    let texts: Map<u32, &str> = modules.iter().map(|m| (m.file, m.src.text.as_str())).collect();
    for n in &mut cx.b.nodes {
        let Some(span) = &n.span else { continue };
        if texts.get(&span.file).and_then(|t| t.get(span.start as usize..span.end as usize)) == Some(n.code.as_str()) {
            n.code.clear();
        }
    }
    let mut result = CodeResult { files: failed, ..CodeResult::default() };
    for (mi, m) in modules.iter().enumerate() {
        result.files.push(CodeFile {
            file: m.file,
            node: cx.file_nodes[mi],
            lang: m.lang,
            entry: cx.entry[mi],
            doc: cx.b.get(cx.file_nodes[mi]).doc.clone(),
            imports: cx.ix.imports[mi].iter().map(|(t, names)| (modules[*t].file, names.clone())).collect(),
            uses: {
                let mut out: Vec<(u32, Vec<String>)> = vec![];
                let mut add = |t: usize, name: Option<&str>| {
                    let file = modules[t].file;
                    let i = out.iter().position(|(f, _)| *f == file).unwrap_or_else(|| {
                        out.push((file, vec![]));
                        out.len() - 1
                    });
                    if let Some(n) = name {
                        if !out[i].1.iter().any(|x| x == n) {
                            out[i].1.push(n.to_string());
                        }
                    }
                };
                // A module depends on a name it imports only if its own code
                // uses it: what it imports to re-export (`pub use db::Db;`,
                // `export { Button } from "./button"`, a crate root's
                // `use db::Db;` for `crate::Db`) is its importers' dependency
                // on where the name is defined.
                // Rust lowers every type it names, so what its code uses is
                // known; elsewhere (TypeScript types are kept as text) only
                // names re-exported by `export { … }` are left out.
                let Used { names: used, members } = names_used(&m.ast);
                let exported: Set<&str> = m.ast.iter().filter_map(|s| if let syntax::Stmt::Export(x) = s { Some(x) } else { None }).flat_map(|x| x.names.iter().filter(|(local, exported)| local == exported).map(|(local, _)| local.as_str())).collect();
                let local: Map<&str, &str> = m.ast.iter().filter_map(|s| if let syntax::Stmt::ImportFrom(x) = s { Some(x) } else { None }).flat_map(|x| x.names.iter().map(|a| (a.name.as_str(), a.asname.as_deref().unwrap_or(&a.name)))).collect();
                let depends = |n: &str| {
                    let l = *local.get(n).unwrap_or(&n);
                    if matches!(m.lang, Lang::Rust | Lang::Go) {
                        used.contains(l)
                    } else {
                        !exported.contains(l)
                    }
                };
                // Names another file of the namespace defines (a Go package's).
                let mut peer: Vec<(usize, &str)> = used.iter().filter(|n| !cx.ix.syms[mi].contains_key(**n)).filter_map(|&n| match cx.ix.namespace_member(mi, n) {
                    Some(index::Sym::Member(p, _)) => Some((*p, n)),
                    _ => None,
                }).collect();
                peer.sort();
                for (p, n) in peer {
                    add(p, Some(n));
                }
                for (t, names) in &cx.ix.imports[mi] {
                    // A module bound whole (`import utils`, Go's `import "…/store"`):
                    // the members code reads of it are defined where they are
                    // (`store.Open` in `store/db.go`).
                    if names.is_empty() {
                        let read: Vec<&str> = members.iter().filter(|(base, attr)| matches!(cx.ix.sym(mi, base), Some(index::Sym::Module(b)) if b == t) && cx.ix.sym(*t, attr).is_some()).map(|&(_, attr)| attr).collect();
                        if read.is_empty() {
                            add(*t, None);
                        }
                        for attr in read {
                            let d = cx.ix.defined_in(*t, attr);
                            if d != mi {
                                add(d, Some(attr));
                            }
                        }
                    }
                    for n in names.iter().filter(|n| depends(n)) {
                        let d = cx.ix.defined_in(*t, n);
                        if d != mi {
                            add(d, Some(n));
                        }
                    }
                }
                out
            },
            assets: cx.ix.assets[mi].clone(),
            externals: cx.ix.externals[mi].clone(),
            error: m.error.clone(),
            app: cx.app[mi],
            handlers: cx.handlers[mi].clone(),
        });
    }
    for r in &cx.relations {
        match r {
            Relation::Exec { from, to } => result.execs.push((modules[*from].file, modules[*to].file)),
            Relation::Artifact { module, name, write } => result.artifacts.push((modules[*module].file, name.clone(), *write)),
            Relation::Template { module, name } => result.templates.push((modules[*module].file, name.clone())),
        }
    }
    let Ctx { b, .. } = cx;
    (b, result)
}
