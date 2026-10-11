//! Syntax classes for the inspector's source view.
//!
//! Python is classified from Ruff's tokens, refined by the syntax tree, so it
//! follows exactly the grammar the analysis parses. Other languages get a
//! table-driven lexer; config and document formats get small line-oriented
//! ones. The goal is code a reader can scan, not a full grammar per language.
//!
//! Output is run-length over the file's UTF-8 bytes, `[len, class, len, class,
//! ...]`, covering every byte and breaking only on character boundaries, so the
//! page can slice any span out of it.

mod code;
mod python;
mod text;

use crate::outline::classify;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Hl {
    Plain = 0,
    Keyword,
    String,
    Comment,
    Number,
    Constant,
    Builtin,
    /// A name being defined: `def name`, `fn name`.
    Function,
    Call,
    Type,
    Decorator,
    Param,
    /// `self`, `cls`, `this`.
    SelfVar,
    /// Delimiters of an interpolation inside a string: `{` `}` of an f-string.
    Interp,
    /// Keys of config and data files.
    Key,
    Tag,
    Attr,
    Heading,
    Strong,
    Link,
    /// Inline and fenced code in documents.
    Code,
    /// Preprocessor lines, attributes, shell escapes, document markers.
    Meta,
}

/// Highlights `text` as the language its `path` implies.
pub fn highlight(path: &str, text: &str) -> Vec<u32> {
    let (_, lang) = classify(path);
    highlight_lang(lang, path, text)
}

fn highlight_lang(lang: &str, path: &str, text: &str) -> Vec<u32> {
    let mut runs = Runs::new(text.len());
    match lang {
        // Notebooks are shown as the Python extracted from their code cells.
        "Python" | "Notebook" => return python::highlight(text),
        "JSON" => text::json(text, 0, text.len(), &mut runs),
        "YAML" => text::yaml(text, &mut runs),
        "TOML" => text::toml(text, &mut runs),
        "INI" | "Env" => text::ini(text, &mut runs),
        "Requirements" => text::requirements(text, &mut runs),
        "Markdown" => text::markdown(text, &mut runs),
        "HTML" | "XML" | "Vue" | "Svelte" => text::markup(text, 0, text.len(), &mut runs),
        "CSS" => text::css(text, 0, text.len(), &mut runs),
        "Makefile" => text::makefile(text, &mut runs),
        "Dockerfile" => text::dockerfile(text, &mut runs),
        "Lockfile" => {
            let ext = path.rsplit('.').next().unwrap_or("");
            let lang = match ext {
                "json" => "JSON",
                "yaml" => "YAML",
                "lock" | "toml" => "TOML",
                _ => return runs.finish(),
            };
            return highlight_lang(lang, "", text);
        }
        _ => match code::spec(lang) {
            Some(spec) => code::lex(text, 0, text.len(), spec, &mut runs),
            None => {}
        },
    }
    runs.finish()
}

/// Highlights an embedded block (a fenced code block, a `<script>`) named by a
/// language tag such as `python` or `ts`, writing into `runs` at `start`.
fn embed(tag: &str, text: &str, start: usize, end: usize, runs: &mut Runs) -> bool {
    let lang = match tag.to_ascii_lowercase().as_str() {
        "py" | "python" | "python3" | "py3" | "ipython" | "pycon" => "Python",
        "js" | "javascript" | "jsx" | "mjs" | "node" => "JavaScript",
        "ts" | "typescript" | "tsx" => "TypeScript",
        "sh" | "bash" | "shell" | "zsh" | "console" | "shell-session" => "Shell",
        "rs" | "rust" => "Rust",
        "c" | "h" => "C",
        "cpp" | "c++" | "cc" | "cuda" | "cu" => "C++",
        "go" | "golang" => "Go",
        "java" => "Java",
        "kotlin" | "kt" => "Kotlin",
        "swift" => "Swift",
        "ruby" | "rb" => "Ruby",
        "sql" => "SQL",
        "lua" => "Lua",
        "json" | "jsonc" => "JSON",
        "yaml" | "yml" => "YAML",
        "toml" => "TOML",
        "ini" | "cfg" | "dotenv" => "INI",
        "html" | "xml" | "svg" | "vue" => "HTML",
        "css" | "scss" | "less" => "CSS",
        "dockerfile" | "docker" => "Dockerfile",
        "makefile" | "make" => "Makefile",
        "md" | "markdown" => "Markdown",
        _ => return false,
    };
    let inner = highlight_lang(lang, "", &text[start..end]);
    runs.splice(start, &inner);
    true
}

/// Run-length classes, built left to right. Bytes not marked are plain.
pub(crate) struct Runs {
    out: Vec<u32>,
    pos: usize,
    len: usize,
}

impl Runs {
    fn new(len: usize) -> Self {
        Runs { out: Vec::new(), pos: 0, len }
    }

    /// Classifies `[start, end)`. Ranges must arrive in order; anything
    /// overlapping what is already classified is clipped.
    fn mark(&mut self, start: usize, end: usize, hl: Hl) {
        let start = start.max(self.pos);
        let end = end.min(self.len);
        if end <= start {
            return;
        }
        if start > self.pos {
            self.push(start - self.pos, Hl::Plain);
        }
        self.push(end - start, hl);
        self.pos = end;
    }

    /// Copies runs computed for a sub-text that starts at `at`.
    fn splice(&mut self, at: usize, inner: &[u32]) {
        let mut p = at;
        for pair in inner.chunks_exact(2) {
            let (n, c) = (pair[0] as usize, pair[1]);
            if c != Hl::Plain as u32 {
                self.mark_raw(p, p + n, c);
            }
            p += n;
        }
    }

    fn mark_raw(&mut self, start: usize, end: usize, class: u32) {
        let start = start.max(self.pos);
        if end <= start {
            return;
        }
        if start > self.pos {
            self.push(start - self.pos, Hl::Plain);
        }
        self.push_raw((end - start) as u32, class);
        self.pos = end;
    }

    fn push(&mut self, n: usize, hl: Hl) {
        self.push_raw(n as u32, hl as u32);
    }

    fn push_raw(&mut self, n: u32, class: u32) {
        let k = self.out.len();
        if k >= 2 && self.out[k - 1] == class {
            self.out[k - 2] += n;
        } else {
            self.out.extend([n, class]);
        }
    }

    fn finish(mut self) -> Vec<u32> {
        if self.pos < self.len {
            let n = self.len - self.pos;
            self.push(n, Hl::Plain);
        }
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(text, class)` for every non-plain run.
    pub(crate) fn classes(path: &str, src: &str) -> Vec<(String, Hl)> {
        let runs = highlight(path, src);
        let total: u32 = runs.chunks_exact(2).map(|p| p[0]).sum();
        assert_eq!(total as usize, src.len(), "runs must cover the text");
        let mut out = Vec::new();
        let mut p = 0usize;
        for pair in runs.chunks_exact(2) {
            let n = pair[0] as usize;
            assert!(src.is_char_boundary(p) && src.is_char_boundary(p + n), "runs must break on char boundaries");
            if pair[1] != 0 {
                out.push((src[p..p + n].to_string(), unsafe { std::mem::transmute::<u8, Hl>(pair[1] as u8) }));
            }
            p += n;
        }
        out
    }

    pub(crate) fn has(path: &str, src: &str, text: &str, hl: Hl) -> bool {
        classes(path, src).iter().any(|(t, h)| t == text && *h == hl)
    }

    #[test]
    fn python_definitions_calls_and_strings() {
        let src = "@dataclass\nclass GPTConfig:\n    n_layer: int = 12\n\ndef forward(self, idx, targets=None):\n    # embed\n    x = self.wte(idx)\n    print(f\"{x[\"a\"]!r:>{width}}\")\n    return GPTConfig(n_layer=2)\n";
        let c = classes("model.py", src);
        let find = |t: &str| c.iter().find(|(s, _)| s == t).map(|(_, h)| *h);
        assert_eq!(find("@dataclass"), Some(Hl::Decorator));
        assert_eq!(find("class"), Some(Hl::Keyword));
        assert_eq!(find("GPTConfig"), Some(Hl::Type));
        assert_eq!(find("int"), Some(Hl::Type));
        assert_eq!(find("12"), Some(Hl::Number));
        assert_eq!(find("forward"), Some(Hl::Function));
        assert_eq!(find("self"), Some(Hl::SelfVar));
        assert_eq!(find("idx"), Some(Hl::Param));
        assert_eq!(find("None"), Some(Hl::Constant));
        assert_eq!(find("# embed"), Some(Hl::Comment));
        assert_eq!(find("wte"), Some(Hl::Call));
        assert_eq!(find("print"), Some(Hl::Builtin));
        assert_eq!(find("n_layer"), Some(Hl::Param), "keyword argument");
        // PEP 701: the nested quotes stay inside the interpolation.
        assert!(c.iter().any(|(s, h)| s == "\"a\"" && *h == Hl::String));
        assert!(c.iter().any(|(s, h)| s == "{" && *h == Hl::Interp));
    }

    #[test]
    fn python_soft_keywords_follow_the_parser() {
        let src = "match = 1\nmatch command:\n    case [x]:\n        pass\ntype Point = tuple[float, float]\nlazy import json\n";
        let c = classes("a.py", src);
        let kinds: Vec<_> = c.iter().filter(|(s, _)| s == "match").map(|(_, h)| *h).collect();
        assert_eq!(kinds, vec![Hl::Keyword], "`match` as a variable is not a keyword");
        assert!(has("a.py", src, "case", Hl::Keyword));
        assert!(has("a.py", src, "type", Hl::Keyword));
        assert!(has("a.py", src, "Point", Hl::Type));
        assert!(has("a.py", src, "lazy", Hl::Keyword));
    }

    #[test]
    fn non_ascii_text_keeps_char_boundaries() {
        for (path, src) in [
            ("a.py", "s = 'héllo ✓'  # naïve\nπ = 3.14\n"),
            ("a.rs", "let π = \"日本\"; // ✓\nfn ünï() {}\n"),
            ("a.md", "# Tïtle ✓\n`cödé` and **bölđ**\n"),
            ("a.yaml", "ключ: значение # ✓\n"),
            ("a.js", "const s = `a ${ \"é\" } ✓`; /é+/.test(s)\n"),
        ] {
            classes(path, src);
        }
    }

    #[test]
    fn brace_languages() {
        let rs = "#[derive(Debug)]\npub struct Point<'a> { x: &'a str }\nfn main() {\n    let s = r#\"raw \"quoted\"\"#;\n    println!(\"{}\", s); // done\n}\n";
        assert!(has("a.rs", rs, "#[derive(Debug)]", Hl::Meta));
        assert!(has("a.rs", rs, "Point", Hl::Type));
        assert!(has("a.rs", rs, "main", Hl::Function));
        assert!(has("a.rs", rs, "r#\"raw \"quoted\"\"#", Hl::String));
        assert!(has("a.rs", rs, "println!", Hl::Call));
        assert!(has("a.rs", rs, "// done", Hl::Comment));
        let ts = "import { x } from './x';\nexport function f(a: number): string {\n  const re = /a\\/b/g;\n  return `n=${a + 1}`; /* c */\n}\n";
        assert!(has("a.ts", ts, "'./x'", Hl::String));
        assert!(has("a.ts", ts, "f", Hl::Function));
        assert!(has("a.ts", ts, "number", Hl::Type));
        assert!(has("a.ts", ts, "/a\\/b/g", Hl::String));
        assert!(has("a.ts", ts, "${", Hl::Interp));
        assert!(has("a.ts", ts, "1", Hl::Number));
        let c = "#include <stdio.h>\nint main(void) { printf(\"%d\\n\", 1); }\n";
        assert!(has("a.c", c, "#include", Hl::Meta));
        assert!(has("a.c", c, "<stdio.h>", Hl::String));
        assert!(has("a.c", c, "int", Hl::Type));
        let sh = "#!/bin/sh\nexport A=\"$HOME/x\" # c\nif [ -n \"${B}\" ]; then echo $1; fi\n";
        assert!(has("a.sh", sh, "#!/bin/sh", Hl::Comment));
        assert!(has("a.sh", sh, "$HOME", Hl::Param));
        assert!(has("a.sh", sh, "fi", Hl::Keyword));
    }

    #[test]
    fn config_and_documents() {
        let toml = "[package]\nname = \"blockviz\" # c\nversion = 1\n";
        assert!(has("Cargo.toml", toml, "[package]", Hl::Type));
        assert!(has("Cargo.toml", toml, "name", Hl::Key));
        assert!(has("Cargo.toml", toml, "\"blockviz\"", Hl::String));
        let yaml = "on:\n  push:\n    branches: [main] # c\nrun: |\n  echo: hi\n";
        assert!(has("ci.yml", yaml, "branches", Hl::Key));
        assert!(has("ci.yml", yaml, "# c", Hl::Comment));
        assert!(!has("ci.yml", yaml, "echo", Hl::Key), "block scalar content is not a key");
        let json = "{\"a\": [1, true, \"s\"]}";
        assert!(has("a.json", json, "\"a\"", Hl::Key));
        assert!(has("a.json", json, "\"s\"", Hl::String));
        assert!(has("a.json", json, "true", Hl::Constant));
        let md = "# Title\n\nSee [docs](http://x) and `code`.\n\n```python\ndef f(): pass\n```\n";
        assert!(has("README.md", md, "# Title", Hl::Heading));
        assert!(has("README.md", md, "[docs](http://x)", Hl::Link));
        assert!(has("README.md", md, "`code`", Hl::Code));
        assert!(has("README.md", md, "def", Hl::Keyword), "fenced Python is highlighted as Python");
        let html = "<div class=\"a\"><script>let x = 1;</script><!-- c --></div>";
        assert!(has("a.html", html, "<div", Hl::Tag));
        assert!(has("a.html", html, "class", Hl::Attr));
        assert!(has("a.html", html, "let", Hl::Keyword));
        assert!(has("a.html", html, "<!-- c -->", Hl::Comment));
        let mk = "CC ?= gcc\nall: main.o\n\t$(CC) -o app main.o # link\n";
        assert!(has("Makefile", mk, "CC", Hl::Key));
        assert!(has("Makefile", mk, "all", Hl::Function));
        assert!(has("Makefile", mk, "$(CC)", Hl::Param));
        let df = "FROM python:3.15\nRUN pip install -r requirements.txt\n";
        assert!(has("Dockerfile", df, "FROM", Hl::Keyword));
        assert!(has("Dockerfile", df, "RUN", Hl::Keyword));
    }
}
