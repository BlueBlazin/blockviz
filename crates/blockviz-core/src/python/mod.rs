//! The Python front end: Ruff parses each module (error-tolerant, current with
//! the newest grammar) and `lower` translates it into the shared syntax tree.

pub mod lower;

use crate::code::source::Source;
use crate::code::syntax::{Lang, TextRange};
use crate::code::{failed_file, CodeFile, Module};
use crate::ir::*;
use crate::text::summary;
use ruff_python_ast::token::TokenKind;
use ruff_python_ast::PySourceType;
use ruff_text_size::Ranged;

pub struct PyInput {
    pub file: u32,
    pub path: String,
    pub text: String,
    pub notebook: bool,
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

/// Parses and lowers every input. Inputs too deeply nested to parse safely
/// become file nodes marked "too complex".
pub fn parse(b: &mut Builder, inputs: Vec<PyInput>, diagnostics: &mut Vec<String>) -> (Vec<Module>, Vec<String>, Vec<CodeFile>) {
    let mut modules = vec![];
    let mut headers = vec![];
    let mut failed = vec![];
    for input in inputs {
        if let Some(why) = too_complex(&input.text) {
            diagnostics.push(format!("Skipped {}: {why}", input.path));
            failed.push(failed_file(b, input.file, &input.path, Lang::Python, input.notebook, why));
            continue;
        }
        // Ruff's parser recovers from syntax errors: a file with a typo still
        // yields a tree for everything around the error.
        let parsed = ruff_python_parser::parse_unchecked_source(&input.text, PySourceType::Python);
        let comments: Vec<(TextRange, String)> = parsed.tokens().iter().filter(|t| t.kind() == TokenKind::Comment).map(|t| (t.range(), input.text[t.range()].trim_start_matches('#').to_string())).collect();
        let errors: Vec<(usize, String)> = parsed.errors().iter().map(|e| (usize::from(e.location.start()), e.error.to_string())).collect();
        let body = lower::body(&parsed.into_syntax().body);
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
        let src = Source::new(input.text, input.file, &comments);
        headers.push(leading_comment(&src));
        modules.push(Module { file: input.file, path: input.path, dotted, package, src, ast: body, notebook: input.notebook, lang: Lang::Python, tags: vec![], error });
    }
    (modules, headers, failed)
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
