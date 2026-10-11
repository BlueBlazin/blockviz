//! The JavaScript/TypeScript front end: oxc parses each module (error-tolerant,
//! JSX and TypeScript included), imports are resolved to project files the
//! way bundlers and `tsc` do, and `lower` translates the tree into the shared
//! syntax tree.

pub mod lower;
pub mod resolve;

use crate::code::source::Source;
use crate::code::syntax::{Lang, TextRange, TextSize};
use crate::code::{failed_file, CodeFile, Module};
use crate::ir::*;
use crate::text::{is_directive, summary};
use oxc_allocator::Allocator;
use oxc_ast::ast::CommentKind;
use oxc_span::SourceType;
pub use resolve::Resolver;

pub struct JsInput {
    pub file: u32,
    pub path: String,
    pub text: String,
}

/// JavaScript or TypeScript, by extension.
pub fn lang_of(path: &str) -> Option<Lang> {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
    match ext.as_str() {
        "js" | "jsx" | "mjs" | "cjs" => Some(Lang::JavaScript),
        "ts" | "tsx" | "mts" | "cts" => Some(Lang::TypeScript),
        _ => None,
    }
}

fn source_type(path: &str) -> SourceType {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "ts" | "mts" | "cts" if path.ends_with(".d.ts") || path.ends_with(".d.mts") || path.ends_with(".d.cts") => SourceType::ts(),
        "ts" | "mts" | "cts" => SourceType::ts(),
        "tsx" => SourceType::tsx(),
        "cjs" => SourceType::cjs().with_jsx(true),
        // Plain `.js` may hold JSX (React projects) and may be a script or a module.
        _ => SourceType::jsx().with_unambiguous(true),
    }
}

/// A module name for display and indexing: the path without extension, dotted.
fn dotted(path: &str) -> (String, String) {
    let without = path.rsplit_once('.').map(|(p, _)| p).unwrap_or(path);
    let dotted = without.replace('/', ".");
    let package = without.rsplit_once('/').map(|(d, _)| d.replace('/', ".")).unwrap_or_default();
    (dotted, package)
}

/// A documentation comment: (start, end, prose).
type Doc = (u32, u32, String);

/// Comment lines, without markers, for `Source`; and documentation comments
/// (`/** … */`) with their spans for definitions.
fn comments(text: &str, list: &[oxc_ast::ast::Comment]) -> (Vec<(TextRange, String)>, Vec<Doc>) {
    let mut lines = vec![];
    let mut docs = vec![];
    for c in list {
        let content = c.content_span();
        let Some(body) = text.get(content.start as usize..content.end as usize) else { continue };
        match c.kind {
            CommentKind::Line => lines.push((TextRange::new(TextSize::from(c.span.start), TextSize::from(c.span.end)), body.to_string())),
            CommentKind::SingleLineBlock | CommentKind::MultiLineBlock => {
                let mut offset = content.start;
                let mut doc = vec![];
                for (i, line) in body.split('\n').enumerate() {
                    // Leading `*` of JSDoc continuation lines is decoration.
                    let t = line.trim().trim_start_matches('*').trim();
                    let start = if i == 0 { c.span.start } else { offset };
                    lines.push((TextRange::new(TextSize::from(start), TextSize::from(offset + line.len() as u32)), t.to_string()));
                    offset += line.len() as u32 + 1;
                    doc.push(t.to_string());
                }
                if body.starts_with('*') {
                    // Prose before the first `@tag`.
                    let prose: Vec<&str> = doc.iter().map(String::as_str).take_while(|l| !l.starts_with('@')).filter(|l| !l.is_empty()).collect();
                    docs.push((c.span.start, c.span.end, prose.join(" ")));
                }
            }
            _ => {}
        }
    }
    (lines, docs)
}

/// The comment block that opens a file, minus licence and tool headers.
pub(crate) fn header(src: &Source, first_code_line: u32) -> String {
    // Comment lines before the code, in blocks of consecutive lines.
    let mut blocks: Vec<Vec<(u32, &str)>> = vec![];
    for l in 1..first_code_line {
        if let Some(c) = src.comments_above(l + 1).last().filter(|c| c.line == l) {
            match blocks.last_mut() {
                Some(b) if b.last().is_some_and(|(prev, _)| *prev + 1 == l) => b.push((l, c.text.trim())),
                _ => blocks.push(vec![(l, c.text.trim())]),
            }
        }
    }
    let mut lines = vec![];
    for block in blocks {
        // A licence header is not a description, its first line (the package name) included.
        if block.iter().any(|(_, t)| {
            let lower = t.to_ascii_lowercase();
            lower.contains("copyright") || lower.contains("license") || lower.contains("licence")
        }) {
            continue;
        }
        for (_, t) in block {
            // `@file Helpers for …` describes the file.
            let t = ["@fileoverview", "@file", "@module"].iter().find_map(|tag| t.strip_prefix(tag).filter(|rest| rest.starts_with(' ')).map(str::trim)).unwrap_or(t);
            let lower = t.to_ascii_lowercase();
            // Tool directives and `/// <reference …/>` lines are not a description.
            if t.starts_with('@') || t.starts_with('/') || lower.starts_with("eslint") || lower.starts_with("prettier") || lower.starts_with("tslint") || lower.starts_with("biome") || is_directive(t) || t.starts_with('#') {
                continue;
            }
            lines.push(t.to_string());
        }
    }
    summary(&lines.join(" "))
}

/// The first syntax error's offset.
fn error_offset(ret: &oxc_parser::ParserReturn) -> Option<usize> {
    Some(ret.diagnostics.errors().next()?.labels.first()?.offset() as usize)
}

/// oxc gives up at some syntax errors, but the rest of the file is still worth
/// showing (Ruff recovers within Python files the same way): the top-level
/// statement holding the error is blanked out, offsets kept, and the file is
/// parsed again.
fn recover(text: &str, st: SourceType) -> Option<String> {
    let mut t = text.to_string();
    for _ in 0..8 {
        let allocator = Allocator::default();
        let ret = oxc_parser::Parser::new(&allocator, &t, st).parse();
        if !ret.fatal_error {
            return Some(t);
        }
        let blanked = blank_statement(&t, error_offset(&ret)?)?;
        drop(ret);
        t = blanked;
    }
    None
}

/// Blanks the top-level statement around `offset`: from the last line at or
/// before it that starts at the margin, to the next line after it that starts
/// at the margin and does not close a bracket. `None` when nothing changes.
pub(crate) fn blank_statement(text: &str, offset: usize) -> Option<String> {
    let starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).filter(|&i| i < text.len()).collect();
    let line = starts.partition_point(|&s| s <= offset).saturating_sub(1);
    let opens = |l: usize| text[starts[l]..].starts_with(|c: char| !c.is_whitespace() && !matches!(c, '}' | ')' | ']'));
    let first = (0..=line).rev().find(|&l| opens(l)).unwrap_or(0);
    let end = (line + 1..starts.len()).find(|&l| opens(l)).map(|l| starts[l]).unwrap_or(text.len());
    let start = starts.get(first).copied().unwrap_or(0);
    if text.as_bytes()[start..end].iter().all(|b| b.is_ascii_whitespace()) {
        return None;
    }
    let mut bytes = text.as_bytes().to_vec();
    for b in &mut bytes[start..end] {
        if *b != b'\n' {
            *b = b' ';
        }
    }
    String::from_utf8(bytes).ok()
}

/// Parses and lowers every input. Inputs too deeply nested to parse safely
/// become file nodes marked "too complex".
pub fn parse(b: &mut Builder, inputs: Vec<JsInput>, resolver: &Resolver, diagnostics: &mut Vec<String>) -> (Vec<Module>, Vec<String>, Vec<CodeFile>) {
    let mut modules = vec![];
    let mut headers = vec![];
    let mut failed = vec![];
    for input in inputs {
        let lang = lang_of(&input.path).unwrap_or(Lang::JavaScript);
        if let Some(why) = crate::python::too_complex(&input.text) {
            diagnostics.push(format!("Skipped {}: {why}", input.path));
            failed.push(failed_file(b, input.file, &input.path, lang, false, why));
            continue;
        }
        let st = source_type(&input.path);
        let allocator = Allocator::default();
        let first = oxc_parser::Parser::new(&allocator, &input.text, st).parse();
        let error = first.diagnostics.errors().next().map(|d| {
            let offset = (d.labels.first().map(|l| l.offset()).unwrap_or(0) as usize).min(input.text.len());
            let line = input.text[..offset].bytes().filter(|c| *c == b'\n').count() + 1;
            let more = first.diagnostics.errors().count().saturating_sub(1);
            let more = if more > 0 { format!(" (+{more} more)") } else { String::new() };
            format!("{}:{}: {}{}", input.path, line, d.message, more)
        });
        let fatal = first.fatal_error;
        let recovered = if fatal { recover(&input.text, st) } else { None };
        if let Some(e) = &error {
            let rest = if fatal && recovered.is_none() { "nothing in the file could be read" } else { "the rest of the file is still shown" };
            diagnostics.push(format!("Syntax error in {e}; {rest}."));
        }
        let again = Allocator::default();
        let text: &str = recovered.as_deref().unwrap_or(&input.text);
        let ret = match &recovered {
            Some(t) => oxc_parser::Parser::new(&again, t, st).parse(),
            None => first,
        };
        let (comment_lines, docs) = comments(&input.text, &ret.program.comments);
        // A doc comment directly above the first statement documents that
        // statement, not the file, unless it says so (`@file`, `@module`).
        let line_of = |offset: u32| input.text[..(offset as usize).min(input.text.len())].bytes().filter(|c| *c == b'\n').count() as u32 + 1;
        let first_start = ret.program.body.first().map(|s| oxc_span::GetSpan::span(s).start);
        let statement_doc = first_start
            .and_then(|f| docs.iter().find(|d| input.text.get(d.1 as usize..f as usize).is_some_and(|between| between.trim().is_empty())))
            .filter(|d| !input.text.get(d.0 as usize..d.1 as usize).is_some_and(|raw| raw.contains("@file") || raw.contains("@module")))
            .map(|d| line_of(d.0));
        let stem = {
            let base = input.path.rsplit('/').next().unwrap_or(&input.path);
            let stem = base.split('.').next().unwrap_or(base);
            // `users/index.ts` is the `users` module.
            if stem == "index" {
                input.path.rsplit('/').nth(1).unwrap_or(stem).to_string()
            } else {
                stem.to_string()
            }
        };
        let from = input.path.clone();
        let resolve = |spec: &str| resolver.resolve(&from, spec);
        let lowerer = lower::Lower { text, required: Default::default(), stem, docs, resolve: &resolve, overload_doc: Default::default() };
        let body = lowerer.program(&ret.program);
        let first_code_line = ret.program.body.first().map(|s| oxc_span::GetSpan::span(s).start as usize).map(|o| input.text[..o.min(input.text.len())].bytes().filter(|c| *c == b'\n').count() as u32 + 1).unwrap_or(u32::MAX);
        let directives: Vec<String> = ret.program.directives.iter().map(|d| d.directive.to_string()).collect();
        drop(lowerer);
        let (dotted, package) = dotted(&input.path);
        let mut src = Source::new(input.text, input.file, &comment_lines);
        src.lang = lang;
        let header_end = first_code_line.min(statement_doc.unwrap_or(u32::MAX));
        headers.push(header(&src, header_end.min(src.text.lines().count() as u32 + 1)));
        // React Server Components: where a module's code runs.
        let tags = directives
            .iter()
            .filter_map(|d| match d.as_str() {
                "use client" => Some("client".to_string()),
                "use server" => Some("server actions".to_string()),
                _ => None,
            })
            .collect();
        modules.push(Module { file: input.file, path: input.path, dotted, package, src, ast: body, notebook: false, lang, tags, error });
    }
    (modules, headers, failed)
}
