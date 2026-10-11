//! The Rust front end: syn parses each module, `resolve` places every file in
//! its crate's module tree the way Cargo and rustc do, and `lower` translates
//! the tree into the shared syntax tree.

pub mod lex;
pub mod lower;
pub mod resolve;

use crate::code::source::Source;
use crate::code::syntax::Lang;
use crate::code::{failed_file, CodeFile, Module};
use crate::ir::*;
pub use resolve::Resolver;

pub struct RsInput {
    pub file: u32,
    pub path: String,
    pub text: String,
}

/// Blanks what precedes the code (a byte-order mark, a `#!` line) without
/// moving any offset.
fn blank_preamble(text: &str) -> String {
    let mut bytes = text.as_bytes().to_vec();
    let mut start = 0;
    if text.starts_with('\u{feff}') {
        bytes[..3].fill(b' ');
        start = 3;
    }
    if text[start..].starts_with("#!") && !text[start + 2..].trim_start().starts_with('[') {
        let end = text[start..].find('\n').map(|i| start + i).unwrap_or(text.len());
        bytes[start..end].fill(b' ');
    }
    String::from_utf8(bytes).unwrap_or_else(|_| text.to_string())
}

/// The body of the innermost function around `offset`, as the byte range
/// between its braces.
fn fn_body_around(text: &str, offset: usize) -> Option<(usize, usize)> {
    let toks: Vec<lex::Token> = lex::lex(text).into_iter().filter(|t| t.kind != lex::Tok::Comment).collect();
    let mut best: Option<(usize, usize)> = None;
    for (i, t) in toks.iter().enumerate() {
        if t.kind != lex::Tok::Ident || t.text(text) != "fn" {
            continue;
        }
        let Some(open) = toks[i + 1..].iter().position(|x| matches!(x.text(text), "{" | ";")).map(|p| i + 1 + p) else { continue };
        if toks[open].text(text) != "{" {
            continue;
        }
        let mut depth = 0;
        let mut close = None;
        for x in &toks[open..] {
            match x.text(text) {
                "{" => depth += 1,
                "}" => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(x.start as usize);
                        break;
                    }
                }
                _ => {}
            }
        }
        let (a, b) = (toks[open].end as usize, close.unwrap_or(text.len()));
        if a <= offset && offset <= b && best.is_none_or(|(x, y)| b - a < y - x) {
            best = Some((a, b));
        }
    }
    best
}

fn blank(text: &str, a: usize, b: usize) -> Option<String> {
    if text.as_bytes()[a..b].iter().all(|c| c.is_ascii_whitespace()) {
        return None;
    }
    let mut bytes = text.as_bytes().to_vec();
    for c in &mut bytes[a..b] {
        if *c != b'\n' {
            *c = b' ';
        }
    }
    String::from_utf8(bytes).ok()
}

/// Parses a file; on a syntax error, the body of the function holding it
/// (else the top-level item) is blanked and the file parsed again, so the
/// rest of the file is still shown. The error is the first one found.
fn parse_file(text: &str) -> (Option<syn::File>, Option<(usize, String)>) {
    let mut err = match syn::parse_str::<syn::File>(text) {
        Ok(f) => return (Some(f), None),
        Err(e) => e,
    };
    let first = (err.span().byte_range().start.min(text.len()), err.to_string());
    let mut t = text.to_string();
    for _ in 0..8 {
        let offset = err.span().byte_range().start.min(t.len());
        let next = fn_body_around(&t, offset).and_then(|(a, b)| blank(&t, a, b)).or_else(|| crate::js::blank_statement(&t, offset));
        let Some(next) = next else { break };
        t = next;
        match syn::parse_str::<syn::File>(&t) {
            Ok(f) => return (Some(f), Some(first)),
            Err(e) => err = e,
        }
    }
    (None, Some(first))
}

/// The line of the first code token, past inner attributes (`#![…]`).
fn first_code_line(text: &str) -> u32 {
    let toks: Vec<lex::Token> = lex::lex(text).into_iter().filter(|t| t.kind != lex::Tok::Comment).collect();
    let mut i = 0;
    while i < toks.len() {
        if toks[i].text(text) == "#" && toks.get(i + 1).is_some_and(|t| t.text(text) == "!") && toks.get(i + 2).is_some_and(|t| t.text(text) == "[") {
            let mut depth = 0;
            i += 2;
            while i < toks.len() {
                match toks[i].text(text) {
                    "[" => depth += 1,
                    "]" => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        return text[..toks[i].start as usize].bytes().filter(|c| *c == b'\n').count() as u32 + 1;
    }
    u32::MAX
}

/// Where the file's own header ends: the first code line, or the start of
/// the comment block directly above it when that block describes the first
/// item rather than the file (anything but `use` and `extern crate`).
fn adjacent_comment_start(text: &str, tokens: &[lex::Token], first_line: u32) -> u32 {
    let Some(first) = tokens.iter().find(|t| t.kind != lex::Tok::Comment && text[..t.start as usize].bytes().filter(|c| *c == b'\n').count() as u32 + 1 == first_line) else { return first_line };
    let mut i = tokens.iter().position(|t| t.start == first.start).unwrap_or(0);
    // Past attributes and visibility to the item's keyword.
    while i < tokens.len() && matches!(tokens[i].text(text), "#" | "[" | "]" | "pub" | "(" | ")" | "crate") {
        i += 1;
    }
    if matches!(tokens.get(i).map(|t| t.text(text)), Some("use" | "extern")) {
        return first_line;
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut line = first_line;
    while line > 1 && lines.get(line as usize - 2).is_some_and(|l| l.trim_start().starts_with("//")) {
        line -= 1;
    }
    line
}

/// Parses and lowers every input. Inputs too deeply nested to parse safely
/// become file nodes marked "too complex".
pub fn parse(b: &mut Builder, inputs: Vec<RsInput>, resolver: &Resolver, diagnostics: &mut Vec<String>) -> (Vec<Module>, Vec<String>, Vec<CodeFile>) {
    let mut modules = vec![];
    let mut headers = vec![];
    let mut failed = vec![];
    for input in inputs {
        if let Some(why) = crate::python::too_complex(&input.text) {
            diagnostics.push(format!("Skipped {}: {why}", input.path));
            failed.push(failed_file(b, input.file, &input.path, Lang::Rust, false, why));
            continue;
        }
        let tokens = lex::lex(&input.text);
        let comments = lex::comments(&input.text, &tokens);
        let clean = blank_preamble(&input.text);
        let (file, error) = parse_file(&clean);
        let error = error.map(|(offset, msg)| {
            let line = input.text[..offset.min(input.text.len())].bytes().filter(|c| *c == b'\n').count() + 1;
            format!("{}:{}: {}", input.path, line, msg)
        });
        if let Some(e) = &error {
            let rest = if file.is_none() { "nothing in the file could be read" } else { "the rest of the file is still shown" };
            diagnostics.push(format!("Syntax error in {e}; {rest}."));
        }
        let (body, doc) = match &file {
            Some(f) => {
                let lowerer = lower::Lower { text: &input.text, path: &input.path, resolver, local: lower::top_names(&f.items), self_ty: Default::default(), implicit: Default::default() };
                (lowerer.file(f), lower::doc_text(&f.attrs, true))
            }
            None => (vec![], None),
        };
        drop(file);
        // Spans are only needed while lowering; the parser's record of every
        // file's text is released.
        proc_macro2::extra::invalidate_current_thread_spans();
        let (dotted, package) = match resolver.module(&input.path) {
            Some((c, path)) => {
                let mut all = vec![c.name.clone()];
                all.extend(path.iter().cloned());
                let package = all[..all.len() - 1].join("::");
                (all.join("::"), package)
            }
            None => (input.path.trim_end_matches(".rs").replace('/', "::"), String::new()),
        };
        let first_line = first_code_line(&input.text);
        // `///` and `/** */` document the item below; `//!` the file.
        let docs: Vec<usize> = tokens
            .iter()
            .filter(|t| t.kind == lex::Tok::Comment)
            .map(|t| t.text(&input.text))
            .zip(tokens.iter().filter(|t| t.kind == lex::Tok::Comment))
            .filter(|(text, _)| (text.starts_with("///") && !text.starts_with("////")) || text.starts_with("//!") || (text.starts_with("/**") && !text.starts_with("/***") && *text != "/**/") || text.starts_with("/*!"))
            .map(|(_, t)| t.start as usize)
            .collect();
        // A comment right above the first item describes that item, unless
        // the file opens with imports (`// Helpers for …` above `use`).
        let header_end = adjacent_comment_start(&input.text, &tokens, first_line);
        let mut src = Source::new(input.text, input.file, &comments);
        src.lang = Lang::Rust;
        src.mark_docs(&docs);
        headers.push(doc.unwrap_or_else(|| crate::js::header(&src, header_end.min(src.text.lines().count() as u32 + 1))));
        modules.push(Module { file: input.file, path: input.path, dotted, package, src, ast: body, notebook: false, lang: Lang::Rust, tags: vec![], error });
    }
    (modules, headers, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_by_blanking_the_broken_function() {
        let src = "fn ok() -> u32 { 1 }\nfn broken() { let x = ; }\nstruct S;\n";
        let (f, err) = parse_file(src);
        let f = f.expect("recovered");
        assert_eq!(f.items.len(), 3);
        assert!(err.is_some());
        proc_macro2::extra::invalidate_current_thread_spans();
    }

    #[test]
    fn keeps_offsets_past_a_shebang() {
        let t = blank_preamble("#!/usr/bin/env rust-script\nfn main() {}\n");
        assert!(t.starts_with("   "));
        assert_eq!(t.len(), "#!/usr/bin/env rust-script\nfn main() {}\n".len());
    }
}
