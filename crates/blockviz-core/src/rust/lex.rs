//! A small Rust tokenizer for what the parser does not keep: comments (syn
//! drops all but doc comments) and `mod` declarations, which the resolver
//! needs for every file before any is lowered. It knows strings, raw strings,
//! char literals and lifetimes, nested block comments; nothing else.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tok {
    Ident,
    Punct,
    Literal,
    Comment,
}

#[derive(Clone, Copy, Debug)]
pub struct Token {
    pub kind: Tok,
    pub start: u32,
    pub end: u32,
}

impl Token {
    pub fn text<'t>(&self, text: &'t str) -> &'t str {
        &text[self.start as usize..self.end as usize]
    }
}

fn ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

fn ident_char(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

pub fn lex(text: &str) -> Vec<Token> {
    let b = text.as_bytes();
    let mut out = vec![];
    let mut i = 0usize;
    let char_at = |i: usize| text[i..].chars().next().unwrap_or('\0');
    while i < b.len() {
        let c = b[i];
        let start = i;
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let kind;
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            kind = Tok::Comment;
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            let mut depth = 0;
            while i < b.len() {
                if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                    depth += 1;
                    i += 2;
                } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            kind = Tok::Comment;
        } else if let Some(len) = raw_string(&b[i..]) {
            i += len;
            kind = Tok::Literal;
        } else if c == b'"' || (matches!(c, b'b' | b'c') && b.get(i + 1) == Some(&b'"')) {
            i += if c == b'"' { 1 } else { 2 };
            while i < b.len() && b[i] != b'"' {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i = (i + 1).min(b.len());
            kind = Tok::Literal;
        } else if c == b'\'' || (c == b'b' && b.get(i + 1) == Some(&b'\'')) {
            // A char literal (`'x'`, `'\n'`, `b'x'`) or a lifetime (`'a`).
            let q = if c == b'\'' { i } else { i + 1 };
            if b.get(q + 1) == Some(&b'\\') {
                // Past the backslash and the escaped character (`'\''`).
                i = q + 3;
                while i < b.len() && b[i] != b'\'' && b[i] != b'\n' {
                    i += 1;
                }
                i = (i + 1).min(b.len());
                kind = Tok::Literal;
            } else if q + 1 < b.len() {
                let ch = char_at(q + 1);
                let after = q + 1 + ch.len_utf8();
                if b.get(after) == Some(&b'\'') {
                    i = after + 1;
                    kind = Tok::Literal;
                } else {
                    i = q + 1;
                    while i < b.len() && ident_char(char_at(i)) {
                        i += char_at(i).len_utf8();
                    }
                    kind = Tok::Punct;
                }
            } else {
                i += 1;
                kind = Tok::Punct;
            }
        } else if c.is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || (b[i] == b'.' && b.get(i + 1).is_some_and(|n| n.is_ascii_digit()))) {
                i += 1;
            }
            kind = Tok::Literal;
        } else if ident_start(char_at(i)) {
            if c == b'r' && b.get(i + 1) == Some(&b'#') {
                i += 2;
            }
            while i < b.len() && ident_char(char_at(i)) {
                i += char_at(i).len_utf8();
            }
            kind = Tok::Ident;
        } else {
            i += char_at(i).len_utf8().max(1);
            kind = Tok::Punct;
        }
        out.push(Token { kind, start: start as u32, end: i.min(b.len()) as u32 });
    }
    out
}

/// The length of a raw string literal at the start of `b` (`r"…"`, `r#"…"#`,
/// `br"…"`, `cr#"…"#`).
fn raw_string(b: &[u8]) -> Option<usize> {
    let mut i = match b {
        [b'r', ..] => 1,
        [b'b' | b'c', b'r', ..] => 2,
        _ => return None,
    };
    let hashes = b[i..].iter().take_while(|&&c| c == b'#').count();
    i += hashes;
    if b.get(i) != Some(&b'"') {
        return None;
    }
    i += 1;
    while i < b.len() {
        if b[i] == b'"' && b[i + 1..].iter().take(hashes).filter(|&&c| c == b'#').count() == hashes && b.len() >= i + 1 + hashes {
            return Some(i + 1 + hashes);
        }
        i += 1;
    }
    Some(b.len())
}

/// Comment lines without their markers (`//`, `///`, `//!`, `/* */` and the
/// leading `*` of continuation lines), for `Source`.
pub fn comments(text: &str, tokens: &[Token]) -> Vec<(ruff_text_size::TextRange, String)> {
    use ruff_text_size::{TextRange, TextSize};
    let mut out = vec![];
    for t in tokens.iter().filter(|t| t.kind == Tok::Comment) {
        let raw = t.text(text);
        if let Some(line) = raw.strip_prefix("//") {
            let line = line.strip_prefix(['/', '!']).unwrap_or(line);
            out.push((TextRange::new(TextSize::from(t.start), TextSize::from(t.end)), line.to_string()));
            continue;
        }
        let body = raw.trim_start_matches("/*").trim_start_matches(['*', '!']).trim_end_matches("*/");
        let mut offset = t.start;
        for (i, line) in raw.split('\n').enumerate() {
            let content = if i == 0 { body.split('\n').next().unwrap_or("") } else { line.trim_end_matches("*/") };
            let cleaned = content.trim().trim_start_matches('*').trim();
            out.push((TextRange::new(TextSize::from(offset), TextSize::from(offset + line.len() as u32)), cleaned.to_string()));
            offset += line.len() as u32 + 1;
        }
    }
    out
}

/// A `mod name;` declaration: its name, its `#[path = "…"]` if any, and the
/// inline modules it is written in (`mod a { mod b; }` declares `a::b`).
#[derive(Debug, Clone)]
pub struct ModDecl {
    pub name: String,
    pub path: Option<String>,
    pub inline: Vec<String>,
}

/// The file-module declarations of a source file.
pub fn mod_decls(text: &str, tokens: &[Token]) -> Vec<ModDecl> {
    let code: Vec<&Token> = tokens.iter().filter(|t| t.kind != Tok::Comment).collect();
    let mut out = vec![];
    // Inline modules open at a brace depth: (name, depth inside it).
    let mut inline: Vec<(String, usize)> = vec![];
    let mut depth = 0usize;
    let mut path_attr: Option<String> = None;
    let mut i = 0;
    let t = |k: usize| code.get(k).map(|t| t.text(text)).unwrap_or("");
    while i < code.len() {
        match t(i) {
            "{" => depth += 1,
            "}" => {
                depth = depth.saturating_sub(1);
                if inline.last().is_some_and(|(_, d)| *d > depth) {
                    inline.pop();
                }
            }
            // `#[path = "x.rs"]`
            "#" if t(i + 1) == "[" && t(i + 2) == "path" && t(i + 3) == "=" => {
                let lit = t(i + 4);
                path_attr = Some(lit.trim_matches('"').to_string());
            }
            "mod" if code[i].kind == Tok::Ident && code.get(i + 1).is_some_and(|n| n.kind == Tok::Ident) => {
                let name = t(i + 1).trim_start_matches("r#").to_string();
                match t(i + 2) {
                    ";" => out.push(ModDecl { name, path: path_attr.take(), inline: inline.iter().map(|(n, _)| n.clone()).collect() }),
                    "{" => {
                        depth += 1;
                        inline.push((name, depth));
                        path_attr = None;
                        i += 3;
                        continue;
                    }
                    _ => {}
                }
            }
            ";" | "fn" | "struct" | "enum" | "impl" | "trait" | "use" | "const" | "static" | "type" => path_attr = None,
            _ => {}
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexes_lifetimes_chars_and_raw_strings() {
        let src = "fn f<'a>(x: &'a str) -> char { let s = r#\"a \" // not\"#; 'x' } // yes";
        let toks = lex(src);
        let comments: Vec<&str> = toks.iter().filter(|t| t.kind == Tok::Comment).map(|t| t.text(src)).collect();
        assert_eq!(comments, ["// yes"]);
        assert!(toks.iter().any(|t| t.kind == Tok::Literal && t.text(src) == "'x'"));
        assert!(toks.iter().any(|t| t.kind == Tok::Punct && t.text(src) == "'a"));
    }

    #[test]
    fn finds_module_declarations() {
        let src = "mod a;\npub(crate) mod b;\n#[path = \"other.rs\"]\nmod c;\nmod inline { mod d; }\nmod e { fn f() {} }\n#[cfg(test)]\nmod tests;\n";
        let decls = mod_decls(src, &lex(src));
        let names: Vec<(&str, Option<&str>, Vec<String>)> = decls.iter().map(|d| (d.name.as_str(), d.path.as_deref(), d.inline.clone())).collect();
        assert_eq!(names, vec![("a", None, vec![]), ("b", None, vec![]), ("c", Some("other.rs"), vec![]), ("d", None, vec!["inline".to_string()]), ("tests", None, vec![])]);
    }
}
