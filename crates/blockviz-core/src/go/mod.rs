//! Go source to syntax tree. No maintained Go parser exists in Rust, so this
//! is a port of Go's own: `scan` is go/scanner, `parse` is go/parser, `ast`
//! is go/ast node for node (positions included), and `walk` visits it in
//! go/ast's order. Tests compare its trees with go/parser's over the Go
//! distribution.
//!
//! Parsing never fails: syntax errors come back with the tree, which keeps
//! everything around them (`Bad*` nodes stand for what could not be read).
//! `resolve` places packages the way the go command does and `lower`
//! translates the tree into the shared syntax tree.

pub mod ast;
pub mod lower;
mod parse;
pub mod resolve;
pub mod scan;
pub mod walk;

use crate::code::source::Source;
use crate::code::syntax::{Lang, TextRange};
use crate::code::{CodeFile, Module};
use crate::ir::Builder;
pub use resolve::Resolver;
use std::collections::{BTreeMap, HashMap};

pub use parse::{MAX_DEPTH, MAX_NESTING};

/// A syntax error at a byte offset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub offset: u32,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct Parsed {
    pub file: ast::File,
    /// In source order.
    pub errors: Vec<SyntaxError>,
}

/// Parses a Go source file, or a snippet of one without the package clause.
pub fn parse(src: &str) -> Parsed {
    let (file, errors) = parse::parse_file(src);
    Parsed { file, errors }
}

pub struct GoInput {
    pub file: u32,
    pub path: String,
    pub text: String,
}

/// A package: the files of one directory with one package clause.
struct Package {
    name: String,
    files: Vec<usize>,
}

impl Package {
    /// The file that stands for the package where code imports it: the one
    /// named after the package, else `doc.go`, else the first.
    fn primary<'a>(&self, inputs: &'a [GoInput]) -> &'a str {
        let base = |i: usize| inputs[i].path.rsplit('/').next().unwrap_or(&inputs[i].path);
        let own = self.files.iter().copied().filter(|&i| !base(i).ends_with("_test.go"));
        let pick = own.clone().find(|&i| base(i) == format!("{}.go", self.name)).or_else(|| own.clone().find(|&i| base(i) == "doc.go")).or_else(|| own.clone().next()).unwrap_or(self.files[0]);
        &inputs[pick].path
    }
}

/// A build constraint in words: `linux && !arm64` → `linux, not arm64`.
fn constraint_words(c: &str) -> String {
    let words = c.replace("&&", ",").replace("||", " or ").replace('!', "not ");
    crate::text::squash(&words.replace(" ,", ","))
}

/// Parses and lowers every input; the files of a package share one namespace.
pub fn front_end(_b: &mut Builder, inputs: Vec<GoInput>, resolver: &Resolver, diagnostics: &mut Vec<String>) -> (Vec<Module>, Vec<String>, Vec<CodeFile>) {
    let parsed: Vec<Parsed> = inputs.iter().map(|i| parse(&i.text)).collect();
    // Packages by directory, then package name.
    let mut packages: BTreeMap<(String, String), Package> = BTreeMap::new();
    for (i, p) in parsed.iter().enumerate() {
        let dir = resolve::dir_of(&inputs[i].path).to_string();
        let pkg = p.file.name.as_ref().map(|n| n.name.clone()).unwrap_or_else(|| "main".into());
        packages.entry((dir, pkg.clone())).or_insert_with(|| Package { name: pkg, files: vec![] }).files.push(i);
    }
    // The package code imports from a directory: not a test package, and
    // the one with the most files where tools (`package main` under
    // `//go:build ignore`) share the directory.
    let mut importable: HashMap<&str, &Package> = HashMap::new();
    for ((dir, name), p) in &packages {
        if name.ends_with("_test") {
            continue;
        }
        match importable.get(dir.as_str()) {
            Some(q) if q.files.len() >= p.files.len() => {}
            _ => {
                importable.insert(dir, p);
            }
        }
    }
    // What code imports from a directory: (package name, the file standing for it).
    let targets: HashMap<String, (String, String)> = importable.iter().map(|(d, p)| (d.to_string(), (p.name.clone(), p.primary(&inputs).to_string()))).collect();
    let mut modules = vec![];
    let mut headers = vec![];
    for (input, p) in inputs.into_iter().zip(parsed) {
        let file = &p.file;
        let dir = resolve::dir_of(&input.path).to_string();
        let pkg_name = file.name.as_ref().map(|n| n.name.clone()).unwrap_or_else(|| "main".into());
        // What each import is called here and which file stands for it.
        let mut imports: HashMap<String, lower::Import> = HashMap::new();
        for spec in file.imports() {
            let path = lower::unquote(&spec.path.value);
            let target = resolver.resolve(&input.path, &path).and_then(|d| targets.get(&d));
            let name = target.map(|t| t.0.clone()).unwrap_or_else(|| resolve::assumed_name(&path));
            let local = spec.name.as_ref().map(|n| n.name.clone()).unwrap_or_else(|| name.clone());
            imports.insert(local, lower::Import { file: target.map(|t| t.1.clone()), path, name });
        }
        let lowerer = lower::Lower::new(&input.text, &input.path, file, imports);
        let body = lowerer.file(file);
        let error = p.errors.first().map(|e| {
            let line = input.text[..(e.offset as usize).min(input.text.len())].bytes().filter(|c| *c == b'\n').count() + 1;
            format!("{}:{}: {}", input.path, line, e.message)
        });
        if let Some(e) = &error {
            diagnostics.push(format!("Syntax error in {e}; the rest of the file is still shown."));
        }
        // Comments, one entry per line; doc comments document what follows.
        let mut comments: Vec<(TextRange, String)> = vec![];
        for g in &file.comments {
            for c in &g.list {
                let raw = c.text.as_str();
                if let Some(line) = raw.strip_prefix("//") {
                    comments.push((c.range, line.to_string()));
                    continue;
                }
                let mut offset = u32::from(c.range.start());
                let body = raw.trim_start_matches("/*").trim_end_matches("*/");
                for (k, line) in raw.split('\n').enumerate() {
                    let content = if k == 0 { body.split('\n').next().unwrap_or("") } else { line.trim_end_matches("*/") };
                    let cleaned = content.trim().trim_start_matches('*').trim();
                    let len = line.len() as u32;
                    comments.push((TextRange::new(offset.into(), (offset + len).into()), cleaned.to_string()));
                    offset += len + 1;
                }
            }
        }
        let mut docs: Vec<usize> = vec![];
        let mut doc_group = |g: &Option<ast::CommentGroup>| {
            if let Some(g) = g {
                docs.extend(g.list.iter().map(|c| usize::from(c.range.start())));
            }
        };
        doc_group(&file.doc);
        for d in &file.decls {
            match d {
                ast::Decl::Func(f) => doc_group(&f.doc),
                ast::Decl::Gen(g) => {
                    doc_group(&g.doc);
                    for s in &g.specs {
                        match s {
                            ast::Spec::Type(t) => {
                                doc_group(&t.doc);
                                if let ast::Expr::StructType(st) = &t.typ {
                                    for f in &st.fields.list {
                                        doc_group(&f.doc);
                                    }
                                }
                                if let ast::Expr::InterfaceType(it) = &t.typ {
                                    for f in &it.methods.list {
                                        doc_group(&f.doc);
                                    }
                                }
                            }
                            ast::Spec::Value(v) => doc_group(&v.doc),
                            ast::Spec::Import(_) => {}
                        }
                    }
                }
                ast::Decl::Bad(_) => {}
            }
        }
        let first_line = file.package.map(|p| input.text[..usize::from(p)].bytes().filter(|c| *c == b'\n').count() as u32 + 1).unwrap_or(1);
        let tags: Vec<String> = resolve::constraint(&input.path, &input.text).filter(|c| c != "ignore").map(|c| constraint_words(&c)).into_iter().collect();
        let mut src = Source::new(input.text, input.file, &comments);
        src.lang = Lang::Go;
        src.mark_docs(&docs);
        // The package comment describes the file; else comments above the package clause.
        headers.push(lower::doc_text(file.doc.as_ref()).unwrap_or_else(|| crate::js::header(&src, first_line)));
        let package = resolver.import_path(&dir);
        let dotted = input.path.trim_end_matches(".go").replace('/', ".");
        let namespace = Some(format!("{dir}\u{0}{pkg_name}"));
        modules.push(Module { file: input.file, path: input.path, dotted, package, src, ast: body, notebook: false, lang: Lang::Go, tags, error, namespace });
    }
    (modules, headers, vec![])
}
