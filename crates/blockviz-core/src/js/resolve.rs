//! Import specifiers → project files, the way bundlers and `tsc` resolve them:
//! relative paths with or without extensions (`./util` → `util.ts`, and
//! `./util.js` → `util.ts` in TypeScript projects), directory `index` files,
//! `tsconfig.json`/`jsconfig.json` `paths` and `baseUrl` (following
//! `extends`), the common `@/` alias, and workspace packages by their
//! `package.json` name.

use std::collections::{BTreeMap, HashMap, HashSet};

const EXTS: &[&str] = &[".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs", ".d.ts", ".json"];

struct Config {
    /// Directory of the config file.
    dir: String,
    base_url: Option<String>,
    /// (pattern, targets), e.g. ("@/*", ["./src/*"]), relative to the base.
    paths: Vec<(String, Vec<String>)>,
}

pub struct Resolver {
    files: HashSet<String>,
    configs: Vec<Config>,
    /// Workspace package name → its directory and entry file.
    packages: HashMap<String, (String, Option<String>)>,
}

/// JSON with comments and trailing commas (tsconfig) → plain JSON.
pub fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let b = text.as_bytes();
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c as char);
            if c == b'\\' && i + 1 < b.len() {
                out.push(b[i + 1] as char);
                i += 2;
                continue;
            }
            if c == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' => {
                in_str = true;
                out.push('"');
                i += 1;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            b',' => {
                // A trailing comma before `}` or `]`.
                let rest = text[i + 1..].trim_start();
                if !(rest.starts_with('}') || rest.starts_with(']')) {
                    out.push(',');
                }
                i += 1;
            }
            _ => {
                // Multi-byte characters outside strings are copied whole.
                let ch = text[i..].chars().next().unwrap();
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    out
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

/// How many directories deep `dir` is; the repository root is 0.
fn depth(dir: &str) -> usize {
    dir.matches('/').count() + usize::from(!dir.is_empty())
}

/// Joins and normalizes `a/b/../c` style paths; `None` if it leaves the repository.
pub fn join(base: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = if base.is_empty() || rel.starts_with('/') { vec![] } else { base.split('/').filter(|p| !p.is_empty()).collect() };
    for p in rel.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    Some(parts.join("/"))
}

impl Resolver {
    /// `files` are every repository path with its text when it is a config
    /// file the resolver reads (`tsconfig*.json`, `jsconfig.json`, `package.json`).
    pub fn new(files: &[(&str, Option<&str>)]) -> Self {
        let mut r = Resolver { files: files.iter().map(|(p, _)| p.to_string()).collect(), configs: vec![], packages: HashMap::new() };
        // In path order, so that the result does not depend on hashing.
        let texts: BTreeMap<&str, &str> = files.iter().filter_map(|(p, t)| t.map(|t| (*p, t))).collect();
        for (path, text) in &texts {
            let base = path.rsplit('/').next().unwrap_or(path);
            if (base.starts_with("tsconfig") && base.ends_with(".json")) || base == "jsconfig.json" {
                if let Some(c) = config(path, &texts, 0) {
                    r.configs.push(c);
                }
            } else if base == "package.json" {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { continue };
                let Some(name) = v.get("name").and_then(|n| n.as_str()) else { continue };
                let dir = dir_of(path).to_string();
                // Of packages sharing a name the shallowest wins, as a build
                // output inside a package repeats its name; then the first.
                if r.packages.get(name).is_some_and(|(d, _)| depth(d) <= depth(&dir)) {
                    continue;
                }
                let entry = [v.get("source"), v.get("exports").and_then(|e| if e.is_string() { Some(e) } else { e.get(".").and_then(|d| if d.is_string() { Some(d) } else { d.get("import").or(d.get("default")) }) }), v.get("module"), v.get("main"), v.get("types")]
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e.as_str())
                    .find_map(|e| join(&dir, e))
                    .filter(|e| r.files.contains(e) || r.with_ext(e).is_some());
                r.packages.insert(name.to_string(), (dir, entry));
            }
        }
        // The nearest config wins: deeper directories first.
        r.configs.sort_by_key(|c| std::cmp::Reverse(depth(&c.dir)));
        r
    }

    fn with_ext(&self, base: &str) -> Option<String> {
        if self.files.contains(base) && base.rsplit('/').next().is_some_and(|b| b.contains('.')) {
            return Some(base.to_string());
        }
        // `./util.js` written in TypeScript source means `./util.ts`.
        for (js, ts) in [(".js", ".ts"), (".js", ".tsx"), (".jsx", ".tsx"), (".mjs", ".mts"), (".cjs", ".cts")] {
            if let Some(stem) = base.strip_suffix(js) {
                let cand = format!("{stem}{ts}");
                if self.files.contains(&cand) {
                    return Some(cand);
                }
            }
        }
        for ext in EXTS {
            let cand = format!("{base}{ext}");
            if self.files.contains(&cand) {
                return Some(cand);
            }
        }
        for ext in EXTS {
            let cand = if base.is_empty() { format!("index{ext}") } else { format!("{base}/index{ext}") };
            if self.files.contains(&cand) {
                return Some(cand);
            }
        }
        None
    }

    /// The project file `spec` refers to from `from`, if it is one.
    pub fn resolve(&self, from: &str, spec: &str) -> Option<String> {
        if spec.starts_with("./") || spec.starts_with("../") || spec == "." || spec == ".." {
            return self.with_ext(&join(dir_of(from), spec)?);
        }
        if spec.starts_with('/') {
            return self.with_ext(&join("", spec)?);
        }
        // Path aliases and base URLs of the nearest config.
        for c in self.configs.iter().filter(|c| c.dir.is_empty() || from.starts_with(&format!("{}/", c.dir))) {
            let base = c.base_url.clone().unwrap_or_else(|| c.dir.clone());
            for (pattern, targets) in &c.paths {
                let hit = match pattern.strip_suffix('*') {
                    Some(prefix) => spec.strip_prefix(prefix),
                    None => (spec == pattern).then_some(""),
                };
                if let Some(rest) = hit {
                    for t in targets {
                        if let Some(found) = join(&base, &t.replace('*', rest)).and_then(|p| self.with_ext(&p)) {
                            return Some(found);
                        }
                    }
                }
            }
            if c.base_url.is_some() {
                if let Some(found) = join(&base, spec).and_then(|p| self.with_ext(&p)) {
                    return Some(found);
                }
            }
        }
        // Workspace packages by name, and their sub-paths.
        let (name, sub) = package_name(spec);
        if let Some((dir, entry)) = self.packages.get(name) {
            return match sub {
                None => entry.clone().and_then(|e| self.with_ext(&e)).or_else(|| self.with_ext(&format!("{dir}/src/index")).or_else(|| self.with_ext(&format!("{dir}/index")).or_else(|| self.with_ext(dir)))),
                Some(sub) => self.with_ext(&format!("{dir}/src/{sub}")).or_else(|| self.with_ext(&format!("{dir}/{sub}"))),
            };
        }
        // `@/…` and `~/…` usually mean the source root even without a config.
        if let Some(rest) = spec.strip_prefix("@/").or_else(|| spec.strip_prefix("~/")) {
            let from_dir = dir_of(from);
            // The `src` directory above the importer, else the repository's.
            let mut dir = from_dir.to_string();
            loop {
                for root in [format!("{dir}/src"), dir.clone()] {
                    let root = root.trim_start_matches('/').to_string();
                    if let Some(found) = join(&root, rest).and_then(|p| self.with_ext(&p)) {
                        return Some(found);
                    }
                }
                match dir.rsplit_once('/') {
                    Some((d, _)) => dir = d.to_string(),
                    None if !dir.is_empty() => dir = String::new(),
                    None => break,
                }
            }
        }
        None
    }
}

/// `@scope/pkg/sub/path` → (`@scope/pkg`, `sub/path`); `node:fs` → (`fs`, None).
pub fn package_name(spec: &str) -> (&str, Option<&str>) {
    let spec = spec.strip_prefix("node:").unwrap_or(spec);
    let mut cut = spec.match_indices('/');
    let end = if spec.starts_with('@') {
        cut.nth(1).map(|(i, _)| i)
    } else {
        cut.next().map(|(i, _)| i)
    };
    match end {
        Some(i) => (&spec[..i], Some(&spec[i + 1..])),
        None => (spec, None),
    }
}

fn config(path: &str, texts: &BTreeMap<&str, &str>, depth: u32) -> Option<Config> {
    let text = texts.get(path)?;
    let v: serde_json::Value = serde_json::from_str(&strip_jsonc(text)).ok()?;
    let dir = dir_of(path).to_string();
    let opts = v.get("compilerOptions");
    let mut out = Config { dir: dir.clone(), base_url: None, paths: vec![] };
    // `extends` brings in a base config's settings, relative to that file.
    if depth < 5 {
        if let Some(parent) = v.get("extends").and_then(|e| e.as_str()).filter(|e| e.starts_with('.')) {
            let parent = join(&dir, parent).map(|p| if p.ends_with(".json") { p } else { format!("{p}.json") });
            if let Some(base) = parent.and_then(|p| config(&p, texts, depth + 1)) {
                out.base_url = base.base_url;
                out.paths = base.paths;
            }
        }
    }
    if let Some(b) = opts.and_then(|o| o.get("baseUrl")).and_then(|b| b.as_str()) {
        out.base_url = join(&dir, b);
    }
    if let Some(paths) = opts.and_then(|o| o.get("paths")).and_then(|p| p.as_object()) {
        out.paths = paths.iter().map(|(k, v)| (k.clone(), v.as_array().map(|a| a.iter().filter_map(|t| t.as_str().map(str::to_string)).collect()).unwrap_or_default())).collect();
        if out.base_url.is_none() {
            out.base_url = Some(dir);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_like_bundlers() {
        let files = [
            ("tsconfig.json", Some("{ // comment\n \"compilerOptions\": { \"baseUrl\": \".\", \"paths\": { \"@/*\": [\"./src/*\"], }, }, }")),
            ("src/lib/db.ts", None),
            ("src/components/Button.tsx", None),
            ("src/components/index.ts", None),
            ("src/app/page.tsx", None),
            ("src/util.ts", None),
            ("packages/ui/package.json", Some("{\"name\": \"@acme/ui\", \"main\": \"./src/index.ts\"}")),
            ("packages/ui/src/index.ts", None),
            ("packages/ui/src/card.tsx", None),
        ];
        let r = Resolver::new(&files);
        assert_eq!(r.resolve("src/app/page.tsx", "@/lib/db").as_deref(), Some("src/lib/db.ts"));
        assert_eq!(r.resolve("src/app/page.tsx", "../components").as_deref(), Some("src/components/index.ts"));
        assert_eq!(r.resolve("src/app/page.tsx", "../util.js").as_deref(), Some("src/util.ts"));
        assert_eq!(r.resolve("src/app/page.tsx", "@acme/ui").as_deref(), Some("packages/ui/src/index.ts"));
        assert_eq!(r.resolve("src/app/page.tsx", "@acme/ui/card").as_deref(), Some("packages/ui/src/card.tsx"));
        assert_eq!(r.resolve("src/app/page.tsx", "react"), None);
        assert_eq!(package_name("@prisma/client/runtime"), ("@prisma/client", Some("runtime")));
        assert_eq!(package_name("node:fs/promises"), ("fs", Some("promises")));
    }
}
