//! Language-agnostic outlines for files that are not Python.
//!
//! Real codebases mix languages. Rather than ignoring them, every text file
//! gets a cheap but useful structure: top-level definitions (with call edges
//! between them for brace languages), keys of config files, headings of docs,
//! and references to other files in the repo (`python train.py` in a shell
//! script). Precision is intentionally modest; the UI labels these "outline".

use crate::ir::Row;
use crate::text::{clip, squash, summary};
use regex_lite::Regex;
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    Python,
    Notebook,
    Code,
    Config,
    Doc,
    Asset,
    Meta,
    Other,
}

pub struct OSym {
    pub name: String,
    pub kind: &'static str,
    pub start: usize,
    pub end: usize,
    pub calls: Vec<String>,
}

#[derive(Default)]
pub struct Outline {
    pub symbols: Vec<OSym>,
    pub rows: Vec<Row>,
    pub doc: String,
    pub refs: Vec<String>,
    /// Module specifiers this file imports, as written (`./camera`, `crate::ir`, `util.h`).
    pub imports: Vec<String>,
}

pub fn classify(path: &str) -> (Class, &'static str) {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    let stem = name.split('.').next().unwrap_or("");
    if matches!(name.as_str(), "makefile" | "gnumakefile") || ext == "mk" {
        return (Class::Config, "Makefile");
    }
    if name == "dockerfile" || name.starts_with("dockerfile.") || ext == "dockerfile" {
        return (Class::Config, "Dockerfile");
    }
    if name == "cmakelists.txt" || ext == "cmake" {
        return (Class::Config, "CMake");
    }
    if name.starts_with("requirements") && ext == "txt" {
        return (Class::Config, "Requirements");
    }
    if matches!(name.as_str(), "package-lock.json" | "cargo.lock" | "poetry.lock" | "yarn.lock" | "uv.lock" | "pnpm-lock.yaml" | "pipfile.lock" | "gemfile.lock" | "go.sum") {
        return (Class::Meta, "Lockfile");
    }
    if matches!(stem, "license" | "licence" | "copying" | "notice" | "authors" | "changelog" | "contributing" | "code_of_conduct") {
        return (Class::Doc, "Document");
    }
    if name == ".env" || name.starts_with(".env.") {
        return (Class::Config, "Env");
    }
    let r = match ext {
        "py" | "pyi" | "pyw" => (Class::Python, "Python"),
        "ipynb" => (Class::Notebook, "Notebook"),
        "c" => (Class::Code, "C"),
        "h" => (Class::Code, "C header"),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "ino" => (Class::Code, "C++"),
        "cu" | "cuh" => (Class::Code, "CUDA"),
        "m" | "mm" => (Class::Code, "Objective-C"),
        "metal" => (Class::Code, "Metal"),
        "rs" => (Class::Code, "Rust"),
        "go" => (Class::Code, "Go"),
        "js" | "mjs" | "cjs" | "jsx" => (Class::Code, "JavaScript"),
        "ts" | "tsx" | "mts" | "cts" => (Class::Code, "TypeScript"),
        "vue" => (Class::Code, "Vue"),
        "svelte" => (Class::Code, "Svelte"),
        "java" => (Class::Code, "Java"),
        "kt" | "kts" => (Class::Code, "Kotlin"),
        "swift" => (Class::Code, "Swift"),
        "scala" | "sc" => (Class::Code, "Scala"),
        "cs" => (Class::Code, "C#"),
        "rb" => (Class::Code, "Ruby"),
        "php" => (Class::Code, "PHP"),
        "lua" => (Class::Code, "Lua"),
        "sh" | "bash" | "zsh" | "fish" => (Class::Code, "Shell"),
        "ps1" => (Class::Code, "PowerShell"),
        "r" => (Class::Code, "R"),
        "jl" => (Class::Code, "Julia"),
        "sql" => (Class::Code, "SQL"),
        "dart" => (Class::Code, "Dart"),
        "zig" => (Class::Code, "Zig"),
        "hs" => (Class::Code, "Haskell"),
        "ex" | "exs" => (Class::Code, "Elixir"),
        "proto" => (Class::Code, "Protobuf"),
        "html" | "htm" => (Class::Code, "HTML"),
        "css" | "scss" | "sass" | "less" => (Class::Code, "CSS"),
        "json" | "jsonc" | "json5" => (Class::Config, "JSON"),
        "yaml" | "yml" => (Class::Config, "YAML"),
        "toml" => (Class::Config, "TOML"),
        "ini" | "cfg" | "conf" => (Class::Config, "INI"),
        "xml" => (Class::Config, "XML"),
        "md" | "markdown" | "mdx" => (Class::Doc, "Markdown"),
        "rst" => (Class::Doc, "reStructuredText"),
        "txt" | "adoc" => (Class::Doc, "Text"),
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" | "bmp" | "tiff" => (Class::Asset, "Image"),
        "pdf" => (Class::Asset, "PDF"),
        "ttf" | "otf" | "woff" | "woff2" => (Class::Asset, "Font"),
        "mp3" | "wav" | "flac" | "ogg" | "mp4" | "mov" | "webm" => (Class::Asset, "Media"),
        "zip" | "tar" | "gz" | "tgz" | "7z" | "bz2" | "xz" => (Class::Asset, "Archive"),
        "pt" | "pth" | "bin" | "ckpt" | "safetensors" | "onnx" | "h5" | "pkl" | "npy" | "npz" | "csv" | "tsv" | "parquet" | "jsonl" | "arrow" => (Class::Asset, "Data"),
        _ if name.starts_with('.') => (Class::Meta, "Config"),
        _ => (Class::Other, "File"),
    };
    r
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid outline pattern"))
}

macro_rules! rx {
    ($p:expr) => {{
        static CELL: OnceLock<Regex> = OnceLock::new();
        re(&CELL, $p)
    }};
}

/// (pattern, kind, name group, kind group)
fn definitions(lang: &str) -> Vec<(&'static Regex, &'static str, usize, usize)> {
    match lang {
        "C" | "C header" | "C++" | "CUDA" | "Objective-C" | "Metal" => vec![
            (rx!(r"(?m)^(?:template\s*<[^>\n]*>\s*)?(?:(?:static|inline|extern|virtual|constexpr|__global__|__device__|__host__|__forceinline__|kernel|const|unsigned|signed|struct)\s+)*[A-Za-z_][\w:<>\*&, ]*?[\s\*&]+\**([A-Za-z_][\w:~]*)\s*\("), "function", 1, 0),
            (rx!(r"(?m)^\s*(?:typedef\s+)?(class|struct|union|enum|namespace)\s+([A-Za-z_]\w*)[^;\n]*\{?\s*$"), "", 2, 1),
        ],
        "Rust" => vec![
            (rx!(r#"(?m)^(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?(?:const\s+)?(?:extern\s+"[^"]*"\s+)?fn\s+([A-Za-z_]\w*)"#), "function", 1, 0),
            (rx!(r"(?m)^(?:pub(?:\([^)]*\))?\s+)?(struct|enum|trait|union|mod)\s+([A-Za-z_]\w*)"), "", 2, 1),
            (rx!(r"(?m)^impl(?:<[^>\n]*>)?\s+(?:[\w:<>, &']+?\s+for\s+)?([A-Za-z_][\w:]*)"), "impl", 1, 0),
        ],
        "Go" => vec![
            (rx!(r"(?m)^func\s+(?:\([^)]*\)\s*)?([A-Za-z_]\w*)\s*[\(\[]"), "function", 1, 0),
            (rx!(r"(?m)^type\s+([A-Za-z_]\w*)\s+(struct|interface)"), "", 1, 2),
        ],
        "JavaScript" | "TypeScript" | "Vue" | "Svelte" => vec![
            (rx!(r"(?m)^(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s*\*?\s*([A-Za-z_$][\w$]*)"), "function", 1, 0),
            (rx!(r"(?m)^(?:export\s+)?(?:default\s+)?(?:abstract\s+)?class\s+([A-Za-z_$][\w$]*)"), "class", 1, 0),
            (rx!(r"(?m)^(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*(?::[^=\n]+)?=\s*(?:async\s+)?(?:function\b|\([^)\n]*\)\s*(?::[^=\n]+)?=>|[A-Za-z_$][\w$]*\s*=>)"), "function", 1, 0),
            (rx!(r"(?m)^(?:export\s+)?(?:declare\s+)?(interface|type|enum)\s+([A-Za-z_$][\w$]*)"), "", 2, 1),
        ],
        "Java" | "Kotlin" | "Swift" | "Scala" | "C#" | "Dart" => vec![
            (rx!(r"(?m)^\s{0,4}(?:@\w+\s+)*(?:(?:public|private|protected|internal|abstract|final|static|sealed|open|data|partial|export)\s+)*(class|interface|enum|struct|record|object|trait|protocol|extension)\s+([A-Za-z_]\w*)"), "", 2, 1),
            (rx!(r"(?m)^(?:(?:public|private|internal)\s+)?(?:fun|func|def)\s+([A-Za-z_]\w*)"), "function", 1, 0),
        ],
        "Ruby" => vec![
            (rx!(r"(?m)^\s*def\s+(?:self\.)?([A-Za-z_]\w*[?!]?)"), "function", 1, 0),
            (rx!(r"(?m)^\s*(class|module)\s+([A-Z]\w*(?:::\w+)*)"), "", 2, 1),
        ],
        "PHP" => vec![
            (rx!(r"(?m)^\s*(?:(?:public|private|protected|static|abstract|final)\s+)*function\s+([A-Za-z_]\w*)"), "function", 1, 0),
            (rx!(r"(?m)^\s*(?:abstract\s+|final\s+)?(class|interface|trait)\s+([A-Za-z_]\w*)"), "", 2, 1),
        ],
        "Lua" => vec![(rx!(r"(?m)^\s*(?:local\s+)?function\s+([A-Za-z_][\w.:]*)"), "function", 1, 0)],
        "Shell" | "PowerShell" => vec![
            (rx!(r"(?m)^\s*(?:function\s+)?([A-Za-z_][\w-]*)\s*\(\s*\)"), "function", 1, 0),
            (rx!(r"(?m)^\s*function\s+([A-Za-z_][\w-]*)\s*\{"), "function", 1, 0),
        ],
        "R" => vec![(rx!(r"(?m)^([A-Za-z_.][\w.]*)\s*(?:<-|=)\s*function"), "function", 1, 0)],
        "Julia" => vec![
            (rx!(r"(?m)^(?:function|macro)\s+([A-Za-z_][\w!]*)"), "function", 1, 0),
            (rx!(r"(?m)^(?:mutable\s+)?struct\s+([A-Za-z_]\w*)"), "struct", 1, 0),
        ],
        "SQL" => vec![(rx!(r#"(?im)^\s*create\s+(?:or\s+replace\s+)?(table|view|function|procedure|index|trigger)\s+(?:if\s+not\s+exists\s+)?([\w."]+)"#), "", 2, 1)],
        "Protobuf" => vec![(rx!(r"(?m)^\s*(message|service|enum)\s+([A-Za-z_]\w*)"), "", 2, 1)],
        "Makefile" => vec![(rx!(r"(?m)^([A-Za-z0-9_.%/-]+)\s*:([^=]|$)"), "target", 1, 0)],
        _ => vec![],
    }
}

const KEYWORDS: &[&str] = &["if", "for", "while", "switch", "return", "sizeof", "else", "catch", "do", "case", "new", "delete", "throw", "defined", "elif", "foreach", "using", "typedef", "operator"];

fn brace_language(lang: &str) -> bool {
    matches!(lang, "C" | "C header" | "C++" | "CUDA" | "Objective-C" | "Metal" | "Rust" | "Go" | "JavaScript" | "TypeScript" | "Vue" | "Svelte" | "Java" | "Kotlin" | "Swift" | "Scala" | "C#" | "Dart" | "PHP" | "Shell" | "PowerShell" | "R" | "Zig")
}

/// End of a `{ ... }` body starting at the first brace after `from`, skipping
/// strings and comments. `None` when a `;` comes first (a declaration).
fn body_end(text: &str, from: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => break,
            b';' => return None,
            b'\n' if i > from + 400 => return None,
            _ => i += 1,
        }
    }
    if i >= bytes.len() {
        return None;
    }
    let open = i;
    let mut depth = 0i32;
    let mut in_str: Option<u8> = None;
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(q) = in_str {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                in_str = None;
            }
        } else if c == b'"' || c == b'\'' || c == b'`' {
            in_str = Some(c);
        } else if c == b'/' && bytes.get(i + 1) == Some(&b'/') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        } else if c == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            continue;
        } else if c == b'{' {
            depth += 1;
        } else if c == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some((open, i + 1));
            }
        }
        i += 1;
    }
    Some((open, bytes.len()))
}

fn leading_comment(text: &str) -> String {
    let mut lines = vec![];
    let mut in_block = false;
    for line in text.lines().take(40) {
        let t = line.trim();
        if t.starts_with("#!") || (lines.is_empty() && t.is_empty()) {
            continue;
        }
        if in_block {
            if let Some(end) = t.find("*/") {
                lines.push(t[..end].trim_start_matches('*').trim().to_string());
                break;
            }
            lines.push(t.trim_start_matches('*').trim().to_string());
            continue;
        }
        if let Some(rest) = t.strip_prefix("/*") {
            in_block = !rest.contains("*/");
            lines.push(rest.trim_end_matches("*/").trim_start_matches('*').trim().to_string());
            continue;
        }
        let stripped = ["///", "//!", "//", "#", "--", ";", "%"].iter().find_map(|p| t.strip_prefix(p));
        match stripped {
            Some(c) => lines.push(c.trim().to_string()),
            None => break,
        }
    }
    summary(&lines.into_iter().filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" "))
}

/// First paragraph of a markdown/rst/text document, without markup.
fn doc_summary(text: &str) -> String {
    let mut para: Vec<String> = vec![];
    let mut in_code = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code || t.starts_with('#') || t.starts_with("![") || t.starts_with('<') || t.starts_with("[!") || t.starts_with("===") || t.starts_with("---") {
            if !para.is_empty() && !in_code {
                break;
            }
            continue;
        }
        if t.is_empty() {
            if !para.is_empty() {
                // Announcements ("**Update**: …") come before the real description.
                let first = para[0].trim_start_matches(['*', '_', '>', ' ']).to_ascii_lowercase();
                if ["update", "news", "note", "announcement", "deprecated", "warning", "important"].iter().any(|w| first.starts_with(w)) {
                    para.clear();
                    continue;
                }
                break;
            }
            continue;
        }
        para.push(t.to_string());
    }
    let text = para.join(" ");
    let link = rx!(r"\[([^\]]*)\]\([^)]*\)");
    let text = link.replace_all(&text, "$1");
    summary(&text.replace(['*', '`', '_'], ""))
}

pub fn outline(lang: &str, text: &str) -> Outline {
    let mut out = Outline::default();
    match lang {
        "Markdown" | "reStructuredText" | "Text" | "Document" => {
            out.doc = doc_summary(text);
            let mut in_code = false;
            for line in text.lines() {
                if line.trim_start().starts_with("```") {
                    in_code = !in_code;
                }
                if in_code {
                    continue;
                }
                if let Some(h) = line.strip_prefix('#') {
                    let level = 1 + h.chars().take_while(|c| *c == '#').count();
                    let title = h.trim_start_matches('#').trim();
                    if !title.is_empty() && level <= 3 {
                        out.rows.push(Row { key: clip(title, 48), value: String::new(), note: String::new(), heading: level <= 2 });
                    }
                }
                if out.rows.len() >= 60 {
                    break;
                }
            }
            return out;
        }
        "JSON" => {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                json_rows(&v, "", &mut out.rows, 0);
            }
            out.refs = file_mentions(text);
            return out;
        }
        "YAML" | "TOML" | "INI" | "Env" => {
            let secret = lang == "Env";
            let mut section = String::new();
            for line in text.lines() {
                if out.rows.len() >= 80 {
                    break;
                }
                let t = line.trim_end();
                if t.trim_start().starts_with('#') || t.trim_start().starts_with(';') || t.trim().is_empty() {
                    continue;
                }
                if let Some(c) = rx!(r"^\[\[?([^\]]+)\]\]?\s*$").captures(t.trim()) {
                    section = c[1].to_string();
                    out.rows.push(Row { key: section.clone(), value: String::new(), note: String::new(), heading: true });
                    continue;
                }
                let indent = t.len() - t.trim_start().len();
                if lang == "YAML" && indent > 0 {
                    continue;
                }
                if let Some(c) = rx!(r"^\s*(?:export\s+)?([A-Za-z_][\w.\-]*)\s*[:=]\s*(.*)$").captures(t) {
                    let value = if secret { "•••".to_string() } else { clip(c[2].trim(), 40) };
                    out.rows.push(Row { key: c[1].to_string(), value, note: String::new(), heading: false });
                }
            }
            let _ = section;
            out.refs = file_mentions(text);
            return out;
        }
        "Requirements" => {
            for line in text.lines().take(120) {
                let t = line.split('#').next().unwrap_or("").trim();
                if t.is_empty() || t.starts_with('-') {
                    continue;
                }
                let name_end = t.find(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '[' || c == ']')).unwrap_or(t.len());
                out.rows.push(Row { key: t[..name_end].to_string(), value: t[name_end..].trim().to_string(), note: String::new(), heading: false });
            }
            return out;
        }
        "Dockerfile" => {
            for c in rx!(r"(?m)^\s*(FROM|RUN|COPY|ADD|CMD|ENTRYPOINT|ENV|WORKDIR|EXPOSE|ARG)\s+(.*)$").captures_iter(text).take(40) {
                out.rows.push(Row { key: c[1].to_string(), value: clip(&squash(&c[2]), 44), note: String::new(), heading: false });
            }
            out.refs = file_mentions(text);
            out.doc = leading_comment(text);
            return out;
        }
        _ => {}
    }
    out.doc = leading_comment(text);
    let mut syms: Vec<OSym> = vec![];
    for (re, kind, name_group, kind_group) in definitions(lang) {
        for c in re.captures_iter(text) {
            let Some(name) = c.get(name_group) else { continue };
            let name = name.as_str().trim_matches('"').to_string();
            if KEYWORDS.contains(&name.as_str()) || name.len() > 60 {
                continue;
            }
            let kind = if kind_group > 0 { c.get(kind_group).map(|k| leak_kind(k.as_str())).unwrap_or("type") } else { kind };
            let whole = c.get(0).unwrap();
            let start = whole.start();
            let line_text = &text[start..text[start..].find('\n').map(|e| start + e).unwrap_or(text.len())];
            let kind = if line_text.contains("__global__") || (lang == "Metal" && line_text.contains("kernel")) { "kernel" } else { kind };
            let mut end = whole.end();
            if brace_language(lang) {
                match body_end(text, whole.end()) {
                    Some((_, e)) => end = e,
                    None if kind == "function" && matches!(lang, "C" | "C header" | "C++" | "CUDA" | "Objective-C") => continue,
                    None => {}
                }
            }
            if syms.iter().any(|s| s.start == start) {
                continue;
            }
            syms.push(OSym { name, kind, start, end, calls: vec![] });
            if syms.len() >= 200 {
                break;
            }
        }
    }
    syms.sort_by_key(|s| s.start);
    // Nested definitions (methods inside classes) are folded into their container.
    let mut keep: Vec<OSym> = vec![];
    for s in syms {
        if let Some(last) = keep.last() {
            if s.start < last.end && last.kind != "function" && s.kind == "function" && last.end > last.start + 1 {
                continue;
            }
        }
        keep.push(s);
    }
    // Calls between definitions of the same file (brace languages).
    if brace_language(lang) {
        let names: Vec<String> = keep.iter().map(|s| s.name.clone()).collect();
        let call = rx!(r"\b([A-Za-z_][\w]*)\s*(?:<<<[^>]*>>>)?\s*\(");
        for s in keep.iter_mut() {
            let body = &text[s.start..s.end.min(text.len())];
            let header_end = body.find('{').unwrap_or(0);
            let mut calls = vec![];
            for c in call.captures_iter(&body[header_end..]) {
                let n = c[1].to_string();
                if n != s.name && names.contains(&n) && !calls.contains(&n) {
                    calls.push(n);
                }
            }
            s.calls = calls;
        }
    }
    out.symbols = keep;
    out.refs = file_mentions(text);
    out.imports = imports(lang, text);
    out
}

fn leak_kind(k: &str) -> &'static str {
    match k.to_ascii_lowercase().as_str() {
        "class" => "class",
        "struct" => "struct",
        "union" => "struct",
        "enum" => "enum",
        "namespace" | "mod" | "module" => "module",
        "trait" | "interface" | "protocol" => "interface",
        "type" => "type",
        "record" | "object" => "class",
        "extension" => "extension",
        "table" => "table",
        "view" => "view",
        "function" | "procedure" => "function",
        "message" => "message",
        "service" => "service",
        _ => "type",
    }
}

fn json_rows(v: &serde_json::Value, prefix: &str, rows: &mut Vec<Row>, depth: u32) {
    if let serde_json::Value::Object(map) = v {
        for (k, val) in map.iter() {
            if rows.len() >= 80 {
                return;
            }
            let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
            match val {
                serde_json::Value::Object(inner) if depth == 0 && inner.len() <= 12 && matches!(k.as_str(), "scripts" | "dependencies" | "devDependencies" | "compilerOptions" | "bin") => {
                    rows.push(Row { key: k.clone(), value: String::new(), note: String::new(), heading: true });
                    json_rows(val, "", rows, depth + 1);
                }
                serde_json::Value::Object(inner) => rows.push(Row { key, value: format!("{{{} keys}}", inner.len()), note: String::new(), heading: false }),
                serde_json::Value::Array(items) => rows.push(Row { key, value: format!("[{} items]", items.len()), note: String::new(), heading: false }),
                other => rows.push(Row { key, value: clip(&other.to_string(), 40), note: String::new(), heading: false }),
            }
        }
    }
}

/// Tokens that look like paths to other source/config files.
pub fn file_mentions(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for c in rx!(r"[\w./-]+\.(?:py|sh|bash|js|mjs|ts|yaml|yml|json|toml|cfg|ini|cu|cuh|cpp|cc|c|h|hpp|rs|go|rb|lua|sql|ipynb)\b").find_iter(text) {
        let t = c.as_str().trim_start_matches("./").to_string();
        if !t.contains("://") && !out.contains(&t) {
            out.push(t);
        }
        if out.len() >= 64 {
            break;
        }
    }
    out
}

/// Import specifiers for languages whose module systems map onto files.
pub fn imports(lang: &str, text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut push = |s: &str| {
        if !s.is_empty() && !out.iter().any(|o| o == s) && out.len() < 200 {
            out.push(s.to_string());
        }
    };
    match lang {
        "JavaScript" | "TypeScript" | "Vue" | "Svelte" => {
            for c in rx!(r#"(?m)(?:^\s*import\s[^'"]*?from\s*|^\s*import\s*|^\s*export\s[^'"]*?from\s*|\brequire\(\s*|\bimport\(\s*)['"]([^'"]+)['"]"#).captures_iter(text) {
                push(&c[1]);
            }
        }
        "Rust" => {
            for c in rx!(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_]\w*)\s*;").captures_iter(text) {
                push(&format!("mod:{}", &c[1]));
            }
            for c in rx!(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?use\s+crate::([A-Za-z_]\w*)").captures_iter(text) {
                push(&format!("crate:{}", &c[1]));
            }
        }
        "C" | "C header" | "C++" | "CUDA" | "Objective-C" | "Metal" => {
            for c in rx!(r#"(?m)^\s*#\s*(?:include|import)\s*"([^"]+)""#).captures_iter(text) {
                push(&c[1]);
            }
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn c_functions_and_calls() {
        let src = "#include <stdio.h>\n// Adds things.\nstatic int add(int a, int b) {\n  return a + b;\n}\n\nint main(void) {\n  printf(\"%d\", add(1, 2));\n  return 0;\n}\nint decl(void);\n";
        let o = outline("C", src);
        let names: Vec<&str> = o.symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["add", "main"]);
        assert_eq!(o.symbols[1].calls, ["add"]);
    }
    #[test]
    fn cuda_kernels_are_marked() {
        let o = outline("CUDA", "__global__ void scale(float* x, int n) {\n  x[0] *= 2;\n}\n");
        assert_eq!(o.symbols[0].kind, "kernel");
    }
    #[test]
    fn typescript_and_rust() {
        let ts = outline("TypeScript", "export function a() {}\nexport const b = (x: number) => x;\nclass C {}\ninterface D {}\n");
        assert_eq!(ts.symbols.len(), 4);
        let rs = outline("Rust", "pub fn a() {}\nstruct B;\nimpl B {\n  fn c() {}\n}\n");
        assert!(rs.symbols.iter().any(|s| s.name == "a" && s.kind == "function"));
        assert!(rs.symbols.iter().any(|s| s.name == "B" && s.kind == "struct"));
    }
    #[test]
    fn env_values_are_masked() {
        let o = outline("Env", "API_KEY=secret\n");
        assert_eq!(o.rows[0].value, "•••");
    }
    #[test]
    fn shell_mentions_scripts() {
        let o = outline("Shell", "#!/bin/bash\npython train.py config/train_gpt2.py\n");
        assert!(o.refs.contains(&"train.py".to_string()));
        assert!(o.refs.contains(&"config/train_gpt2.py".to_string()));
    }
}
