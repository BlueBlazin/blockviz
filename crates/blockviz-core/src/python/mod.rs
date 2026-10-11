//! Python analysis: parse every module, index symbols project-wide, then build
//! one dataflow diagram per module, class and function.

pub mod effects;
pub mod flow;
pub mod frameworks;
pub mod index;
pub mod names;
pub mod source;
pub mod walk;

use crate::ir::*;
use crate::text::summary;
use crate::vocabulary::{self, Domains};
use flow::{Ctx, Fb, Relation};
use index::{docstring, Index, PyModule};
use ruff_python_ast::token::TokenKind;
use ruff_python_ast::PySourceType;
use ruff_text_size::{Ranged, TextRange};
use source::Source;
use ruff_python_ast as ast_stmt;
use std::collections::{BTreeSet, HashMap};

pub struct PyInput {
    pub file: u32,
    pub path: String,
    pub text: String,
    pub notebook: bool,
}

pub struct PyFile {
    pub file: u32,
    pub node: Id,
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

pub struct PyResult {
    pub files: Vec<PyFile>,
    pub execs: Vec<(u32, u32)>,
    /// (file, artifact name, is write)
    pub artifacts: Vec<(u32, String, bool)>,
    /// (file, template path as written)
    pub templates: Vec<(u32, String)>,
}

pub fn dotted_name(path: &str) -> (String, String) {
    let without = path.trim_end_matches(".py").trim_end_matches(".ipynb").trim_end_matches(".pyi");
    let parts: Vec<&str> = without.split('/').filter(|p| !p.is_empty()).collect();
    let is_init = parts.last() == Some(&"__init__");
    let module_parts: Vec<&str> = if is_init { parts[..parts.len() - 1].to_vec() } else { parts.clone() };
    let dotted = module_parts.join(".");
    let package = if is_init { dotted.clone() } else { module_parts[..module_parts.len().saturating_sub(1)].join(".") };
    (dotted, package)
}

/// Strips IPython magics and shell escapes so notebook code parses as Python.
pub fn clean_notebook_line(line: &str) -> String {
    let t = line.trim_start();
    if t.starts_with('%') || t.starts_with('!') {
        String::new()
    } else {
        line.to_string()
    }
}

pub fn analyze(b: Builder, inputs: Vec<PyInput>, diagnostics: &mut Vec<String>) -> (Builder, PyResult) {
    let mut b = b;
    let mut modules: Vec<PyModule> = vec![];
    let mut failed: Vec<PyFile> = vec![];
    for input in inputs {
        if let Some(why) = too_complex(&input.text) {
            diagnostics.push(format!("Skipped {}: {why}", input.path));
            let node = b.add(None, if input.notebook { Kind::Notebook } else { Kind::File }, Shape::Box, input.path.rsplit('/').next().unwrap_or(&input.path));
            b.get_mut(node).badges.push("too complex".into());
            failed.push(PyFile { file: input.file, node, entry: false, doc: String::new(), imports: vec![], externals: BTreeSet::new(), error: Some(why), app: false, handlers: vec![] });
            continue;
        }
        // Ruff's parser recovers from syntax errors: a file with a typo still
        // yields a tree for everything around the error.
        let parsed = ruff_python_parser::parse_unchecked_source(&input.text, PySourceType::Python);
        let comments: Vec<TextRange> = parsed.tokens().iter().filter(|t| t.kind() == TokenKind::Comment).map(|t| t.range()).collect();
        let errors: Vec<(usize, String)> = parsed.errors().iter().map(|e| (usize::from(e.location.start()), e.error.to_string())).collect();
        let body: Vec<ast_stmt::Stmt> = parsed.into_syntax().body.into_iter().collect();
        let (dotted, package) = dotted_name(&input.path);
        let error = errors.first().map(|(offset, msg)| {
            let offset = (*offset).min(input.text.len());
            let line = input.text[..offset].bytes().filter(|c| *c == b'\n').count() + 1;
            let more = if errors.len() > 1 { format!(" (+{} more)", errors.len() - 1) } else { String::new() };
            format!("{}:{}: {}{}", input.path, line, msg, more)
        });
        if let Some(e) = &error {
            diagnostics.push(format!("Syntax error in {e}; the rest of the file is still shown."));
        }
        modules.push(PyModule { file: input.file, path: input.path, dotted, package, src: Source::new(input.text, input.file, &comments), ast: body, notebook: input.notebook, error });
    }
    let file_nodes: Vec<Id> = modules
        .iter()
        .map(|m| {
            let title = m.path.rsplit('/').next().unwrap_or(&m.path).to_string();
            let id = b.add(None, if m.notebook { Kind::Notebook } else { Kind::File }, Shape::Box, title);
            let n = b.get_mut(id);
            n.category = "file";
            n.span = Some(Span { file: m.file, start: 0, end: m.src.text.len() as u32, line: 1, end_line: m.src.text.lines().count().max(1) as u32 });
            n.doc = docstring(&m.ast).map(summary).unwrap_or_else(|| leading_comment(&m.src));
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
    let mut result = PyResult { files: failed, execs: vec![], artifacts: vec![], templates: vec![] };
    for (mi, m) in modules.iter().enumerate() {
        result.files.push(PyFile {
            file: m.file,
            node: cx.file_nodes[mi],
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

/// Rejects inputs that would recurse too deeply in the parser or analysis
/// (generated code with hundreds of nested brackets or enormous statements).
/// A small string- and comment-aware scan; it never recurses.
pub(crate) fn too_complex(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let (mut depth, mut i, mut line_len) = (0i32, 0usize, 0usize);
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'"' | b'\'' => {
                let triple = bytes.get(i + 1) == Some(&c) && bytes.get(i + 2) == Some(&c);
                let quote_len = if triple { 3 } else { 1 };
                i += quote_len;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == c && (!triple || (bytes.get(i + 1) == Some(&c) && bytes.get(i + 2) == Some(&c))) {
                        i += quote_len;
                        break;
                    }
                    if !triple && bytes[i] == b'\n' {
                        break;
                    }
                    i += 1;
                }
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = (depth - 1).max(0),
            b'\n' if depth == 0 => line_len = 0,
            _ => {}
        }
        line_len += 1;
        if depth > 120 {
            return Some("brackets nest more than 120 levels deep".into());
        }
        if line_len > 400_000 {
            return Some("a single statement is longer than 400 KB".into());
        }
        i += 1;
    }
    None
}

/// A module without a docstring is often introduced by a comment block.
fn leading_comment(src: &Source) -> String {
    let mut lines = vec![];
    for line in src.text.lines() {
        let t = line.trim();
        if let Some(c) = t.strip_prefix('#') {
            if t.starts_with("#!") || t.contains("coding") {
                continue;
            }
            lines.push(c.trim().to_string());
        } else if t.is_empty() && lines.is_empty() {
            continue;
        } else {
            break;
        }
    }
    summary(&lines.join(" "))
}
