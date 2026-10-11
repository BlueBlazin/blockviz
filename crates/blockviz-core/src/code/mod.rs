//! Program analysis shared by every language: front ends lower their syntax
//! into `syntax`, then this indexes symbols project-wide and builds one
//! dataflow diagram per module, class and function.

pub mod effects;
pub mod flow;
pub mod frameworks;
pub mod index;
pub mod names;
pub mod source;
pub mod syntax;
pub mod walk;

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

/// A file node for a module the front end could not analyze.
pub fn failed_file(b: &mut Builder, file: u32, path: &str, lang: Lang, notebook: bool, why: String) -> CodeFile {
    let node = b.add(None, if notebook { Kind::Notebook } else { Kind::File }, Shape::Box, path.rsplit('/').next().unwrap_or(path));
    b.get_mut(node).badges.push("too complex".into());
    CodeFile { file, node, lang, entry: false, doc: String::new(), imports: vec![], externals: BTreeSet::new(), error: Some(why), app: false, handlers: vec![] }
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
    let domains: Vec<Domains> = (0..n).map(|mi| ix.imports[mi].iter().fold(direct[mi], |d, (t, _)| d.union(direct[*t]))).collect();
    let mut cx = Ctx { ix, b, file_nodes, relations: vec![], entry: vec![false; n], module_by_path, imported, app: vec![false; n], handlers: vec![vec![]; n], prefixes: HashMap::new(), domains };
    for mi in 0..n {
        Fb::build_module(&mut cx, mi);
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
