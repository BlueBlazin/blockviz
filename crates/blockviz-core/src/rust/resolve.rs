//! Which file holds a Rust module path, the way Cargo and rustc decide it.
//!
//! Each Cargo package has crate roots (`src/lib.rs`, `src/main.rs`,
//! `src/bin/*.rs`, `examples/`, `tests/`, `benches/`, `build.rs`, or the paths
//! its manifest names). From a root, `mod name;` declarations name child
//! modules in `name.rs` or `name/mod.rs` (or a `#[path]`), so walking them
//! gives every file its crate and module path. A path in code then resolves
//! by its first segment (`crate`, `self`, `super`, a child module, a
//! workspace crate) and descends through child modules; what is left names
//! items in the last module.

use super::lex::{self, ModDecl};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CrateKind {
    Lib,
    Bin,
    Example,
    Test,
    Bench,
    Build,
}

#[derive(Debug, Clone)]
pub struct Crate {
    pub root: String,
    /// The name code uses for it (`mini_redis`): the package or target name
    /// with `-` as `_`.
    pub name: String,
    pub kind: CrateKind,
    /// The library crate of the same package, which its other targets use by name.
    pub lib: Option<usize>,
}

/// What a path's leading segments name.
#[derive(Debug, PartialEq, Eq)]
pub enum Resolution {
    /// A project module: its file, and how many segments named modules
    /// (the rest name items in it).
    Module { file: String, used: usize },
    /// Another crate (`std`, `tokio`): its name.
    External(String),
    /// Not a module path from here: a type, a variable, an imported name.
    Local,
}

#[derive(Default)]
struct Manifest {
    package: bool,
    name: Option<String>,
    lib_name: Option<String>,
    lib_path: Option<String>,
    bins: Vec<(Option<String>, Option<String>)>,
    build: Option<String>,
    dependencies: Vec<String>,
}

fn manifest(text: &str) -> Manifest {
    let mut m = Manifest::default();
    let mut section = String::new();
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('#') || l.is_empty() {
            continue;
        }
        if let Some(rest) = l.strip_prefix("[[") {
            section = rest.trim_end_matches("]]").trim().to_string();
            if section == "bin" {
                m.bins.push((None, None));
            }
            continue;
        }
        if let Some(rest) = l.strip_prefix('[') {
            section = rest.split(']').next().unwrap_or("").trim().to_string();
            if section == "package" {
                m.package = true;
            }
            continue;
        }
        let Some((k, v)) = l.split_once('=') else { continue };
        let key = k.trim().trim_matches('"');
        let value = v.split(" #").next().unwrap_or("").trim().trim_matches('"').to_string();
        match (section.as_str(), key) {
            ("package", "name") => m.name = Some(value),
            ("package", "build") => m.build = Some(value),
            ("lib", "name") => m.lib_name = Some(value),
            ("lib", "path") => m.lib_path = Some(value),
            ("bin", "name") => {
                if let Some(b) = m.bins.last_mut() {
                    b.0 = Some(value);
                }
            }
            ("bin", "path") => {
                if let Some(b) = m.bins.last_mut() {
                    b.1 = Some(value);
                }
            }
            (s, k) if s.ends_with("dependencies") => m.dependencies.push(k.split('.').next().unwrap_or(k).replace('-', "_")),
            _ => {}
        }
    }
    m
}

fn join(dir: &str, rel: &str) -> String {
    let mut parts: Vec<&str> = if dir.is_empty() { vec![] } else { dir.split('/').collect() };
    for p in rel.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

fn stem(path: &str) -> &str {
    let base = path.rsplit('/').next().unwrap_or(path);
    base.strip_suffix(".rs").unwrap_or(base)
}

pub struct Resolver {
    pub crates: Vec<Crate>,
    /// Module file → (crate, module path).
    module_of: HashMap<String, (usize, Vec<String>)>,
    /// (crate, module path joined by `::`) → file.
    file_of: HashMap<(usize, String), String>,
    /// Library crates by the name code uses.
    libs: HashMap<String, usize>,
    /// Dependencies named in manifests (`tokio`, `serde_json`).
    pub dependencies: HashSet<String>,
}

impl Resolver {
    /// A digest of everything lowering a file reads from the resolver: the
    /// crates and where their modules are. Unchanged by edits that add no
    /// module or crate, so those leave other files' lowering valid.
    pub fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for c in &self.crates {
            (&c.root, &c.name, c.kind, c.lib).hash(&mut h);
        }
        let mut modules: Vec<_> = self.module_of.iter().collect();
        modules.sort();
        modules.hash(&mut h);
        let mut files: Vec<_> = self.file_of.iter().collect();
        files.sort();
        files.hash(&mut h);
        let mut libs: Vec<_> = self.libs.iter().collect();
        libs.sort();
        libs.hash(&mut h);
        let mut deps: Vec<_> = self.dependencies.iter().collect();
        deps.sort();
        deps.hash(&mut h);
        h.finish()
    }

    /// `files` lists every path with the text of Rust files and manifests.
    pub fn new(files: &[(&str, Option<&str>)]) -> Self {
        let rs: HashSet<&str> = files.iter().filter(|(p, _)| p.ends_with(".rs")).map(|(p, _)| *p).collect();
        let texts: HashMap<&str, &str> = files.iter().filter_map(|(p, t)| t.map(|t| (*p, t))).collect();
        let mut r = Resolver { crates: vec![], module_of: HashMap::new(), file_of: HashMap::new(), libs: HashMap::new(), dependencies: HashSet::new() };
        if rs.is_empty() {
            return r;
        }
        let exists = |p: &str| rs.contains(p);
        let mut manifests: Vec<(&str, Manifest)> = files.iter().filter(|(p, _)| p.rsplit('/').next() == Some("Cargo.toml")).filter_map(|(p, _)| Some((dir_of(p), manifest(texts.get(p)?)))).collect();
        manifests.sort_by_key(|(d, _)| std::cmp::Reverse(d.len()));
        for (dir, m) in &manifests {
            r.dependencies.extend(m.dependencies.iter().cloned());
            if !m.package {
                continue;
            }
            let package = m.name.clone().unwrap_or_else(|| dir.rsplit('/').next().unwrap_or("crate").to_string()).replace('-', "_");
            let at = |rel: &str| join(dir, rel);
            let lib_root = m.lib_path.as_deref().map(at).or_else(|| Some(at("src/lib.rs")).filter(|p| exists(p)));
            let lib = lib_root.filter(|p| exists(p)).map(|root| {
                r.crates.push(Crate { root, name: m.lib_name.clone().unwrap_or_else(|| package.clone()).replace('-', "_"), kind: CrateKind::Lib, lib: None });
                r.crates.len() - 1
            });
            let mut roots: Vec<(String, String, CrateKind)> = vec![];
            for (name, path) in &m.bins {
                let name = name.clone().unwrap_or_else(|| package.clone());
                let path = path.as_deref().map(at).unwrap_or_else(|| at(&format!("src/bin/{name}.rs")));
                roots.push((path, name, CrateKind::Bin));
            }
            roots.push((at("src/main.rs"), package.clone(), CrateKind::Bin));
            for (sub, kind) in [("src/bin", CrateKind::Bin), ("examples", CrateKind::Example), ("tests", CrateKind::Test), ("benches", CrateKind::Bench)] {
                let base = at(sub);
                let prefix = format!("{base}/");
                let mut found: Vec<&str> = rs.iter().copied().filter(|p| p.starts_with(&prefix)).collect();
                found.sort();
                for p in found {
                    let rest = &p[prefix.len()..];
                    match rest.split('/').collect::<Vec<_>>()[..] {
                        [file] => roots.push((p.to_string(), stem(file).to_string(), kind)),
                        [d, "main.rs"] => roots.push((p.to_string(), d.to_string(), kind)),
                        _ => {}
                    }
                }
            }
            if m.build.as_deref() != Some("false") {
                roots.push((m.build.as_deref().map(at).unwrap_or_else(|| at("build.rs")), "build_script".into(), CrateKind::Build));
            }
            for (root, name, kind) in roots {
                if exists(&root) && !r.crates.iter().any(|c| c.root == root) {
                    r.crates.push(Crate { root, name: name.replace('-', "_"), kind, lib });
                }
            }
        }
        // Without manifests (or for files no package claims), `lib.rs` and
        // `main.rs` are crate roots.
        let mut loose: Vec<&str> = rs.iter().copied().filter(|p| matches!(p.rsplit('/').next(), Some("lib.rs" | "main.rs"))).collect();
        loose.sort();
        for p in loose {
            if r.crates.iter().any(|c| c.root == p) {
                continue;
            }
            let dir = dir_of(p);
            let name = if dir.rsplit('/').next() == Some("src") { dir_of(dir) } else { dir };
            let name = name.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("crate").replace('-', "_");
            r.crates.push(Crate { root: p.to_string(), name, kind: if p.ends_with("lib.rs") { CrateKind::Lib } else { CrateKind::Bin }, lib: None });
        }
        // A package's binaries use its library by name.
        for c in 0..r.crates.len() {
            if r.crates[c].kind == CrateKind::Lib {
                r.libs.entry(r.crates[c].name.clone()).or_insert(c);
            }
        }
        // Libraries first, so a module shared through `#[path]` belongs to the library.
        let mut order: Vec<usize> = (0..r.crates.len()).collect();
        order.sort_by_key(|&c| r.crates[c].kind != CrateKind::Lib);
        let decls = |p: &str| -> Vec<ModDecl> { texts.get(p).map(|t| lex::mod_decls(t, &lex::lex(t))).unwrap_or_default() };
        for c in order {
            let root = r.crates[c].root.clone();
            let mut stack = vec![(root, vec![])];
            while let Some((file, path)) = stack.pop() {
                if r.module_of.contains_key(&file) {
                    continue;
                }
                r.file_of.insert((c, path.join("::")), file.clone());
                r.module_of.insert(file.clone(), (c, path.clone()));
                let owns_dir = path.is_empty() || file.ends_with("/mod.rs") || file == "mod.rs";
                let dir = dir_of(&file);
                let base = if owns_dir { dir.to_string() } else { join(dir, stem(&file)) };
                for d in decls(&file) {
                    let nested = join(&base, &d.inline.join("/"));
                    let candidates = match &d.path {
                        Some(p) if d.inline.is_empty() => vec![join(dir, p)],
                        Some(p) => vec![join(&nested, p)],
                        None => vec![join(&nested, &format!("{}.rs", d.name)), join(&nested, &format!("{}/mod.rs", d.name))],
                    };
                    if let Some(child) = candidates.into_iter().find(|p| exists(p)) {
                        let mut sub = path.clone();
                        sub.extend(d.inline.iter().cloned());
                        sub.push(d.name.clone());
                        stack.push((child, sub));
                    }
                }
            }
        }
        // Files no root reaches (an incomplete upload): placed by their
        // directory under the nearest crate root above them.
        let mut orphans: Vec<&str> = rs.iter().copied().filter(|p| !r.module_of.contains_key(*p)).collect();
        orphans.sort();
        for p in orphans {
            let owner = r.crates.iter().enumerate().filter(|(_, c)| c.kind == CrateKind::Lib || c.kind == CrateKind::Bin).filter(|(_, c)| {
                let d = dir_of(&c.root);
                d.is_empty() || p.starts_with(&format!("{d}/"))
            });
            let best = owner.max_by_key(|(_, c)| dir_of(&c.root).len()).map(|(i, c)| (i, dir_of(&c.root).to_string()));
            let (c, path) = match best {
                Some((c, d)) => {
                    let rel = if d.is_empty() { p } else { &p[d.len() + 1..] };
                    let mut segs: Vec<String> = rel.trim_end_matches(".rs").split('/').map(str::to_string).collect();
                    if segs.last().map(String::as_str) == Some("mod") {
                        segs.pop();
                    }
                    (c, segs)
                }
                None => {
                    r.crates.push(Crate { root: p.to_string(), name: stem(p).to_string(), kind: CrateKind::Bin, lib: None });
                    (r.crates.len() - 1, vec![])
                }
            };
            r.file_of.entry((c, path.join("::"))).or_insert_with(|| p.to_string());
            r.module_of.insert(p.to_string(), (c, path));
        }
        r
    }

    /// The crate a file belongs to and its module path in it.
    pub fn module(&self, file: &str) -> Option<(&Crate, &[String])> {
        self.module_of.get(file).map(|(c, p)| (&self.crates[*c], &p[..]))
    }

    /// Whether a name is a library crate of the project (`mini_redis`).
    pub fn is_crate(&self, name: &str) -> bool {
        self.libs.contains_key(name)
    }

    /// The root file of a library crate of the project.
    pub fn crate_root(&self, name: &str) -> Option<&str> {
        self.libs.get(name).map(|&c| self.crates[c].root.as_str())
    }

    /// Whether `file` is the root of its crate.
    pub fn is_root(&self, file: &str) -> bool {
        self.crates.iter().any(|c| c.root == file)
    }

    fn child(&self, c: usize, path: &[String], name: &str) -> Option<&String> {
        let key = if path.is_empty() { name.to_string() } else { format!("{}::{name}", path.join("::")) };
        self.file_of.get(&(c, key))
    }

    /// Resolves a path written in `from`. `local` says whether a first
    /// segment is a name the file defines or imports (a type, an alias).
    pub fn resolve(&self, from: &str, segs: &[&str], local: impl Fn(&str) -> bool) -> Resolution {
        let Some((krate, here)) = self.module_of.get(from).cloned() else { return Resolution::Local };
        let Some(&first) = segs.first() else { return Resolution::Local };
        let (c, mut path, mut i): (usize, Vec<String>, usize) = match first {
            "crate" => (krate, vec![], 1),
            "self" => (krate, here.clone(), 1),
            "super" => {
                let mut p = here.clone();
                let mut i = 0;
                while segs.get(i) == Some(&"super") {
                    p.pop();
                    i += 1;
                }
                (krate, p, i)
            }
            "Self" => return Resolution::Local,
            name if self.child(krate, &here, name).is_some() => (krate, [here.clone(), vec![name.to_string()]].concat(), 1),
            name if local(name) => return Resolution::Local,
            name => {
                let own = self.crates[krate].lib.filter(|&l| self.crates[l].name == name);
                match own.or_else(|| self.libs.get(name).copied()) {
                    Some(l) => (l, vec![], 1),
                    None => return Resolution::External(name.to_string()),
                }
            }
        };
        while i < segs.len() {
            if self.child(c, &path, segs[i]).is_none() {
                break;
            }
            path.push(segs[i].to_string());
            i += 1;
        }
        match self.file_of.get(&(c, path.join("::"))) {
            Some(f) => Resolution::Module { file: f.clone(), used: i },
            None => Resolution::Local,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver(files: &[(&str, &str)]) -> Resolver {
        let listing: Vec<(&str, Option<&str>)> = files.iter().map(|(p, t)| (*p, Some(*t))).collect();
        Resolver::new(&listing)
    }

    #[test]
    fn walks_modules_from_crate_roots() {
        let r = resolver(&[
            ("Cargo.toml", "[package]\nname = \"mini-redis\"\n[dependencies]\ntokio = \"1\"\n"),
            ("src/lib.rs", "pub mod cmd;\nmod db;\npub use db::Db;\n"),
            ("src/main.rs", "fn main() {}\n"),
            ("src/cmd/mod.rs", "mod get;\npub use get::Get;\n"),
            ("src/cmd/get.rs", "use crate::Db;\nuse super::Get;\n"),
            ("src/db.rs", "#[path = \"shards.rs\"]\nmod sharded;\n"),
            ("src/shards.rs", ""),
            ("src/bin/cli.rs", "use mini_redis::cmd::Get;\n"),
        ]);
        let m = |f: &str| r.module(f).map(|(c, p)| (c.name.clone(), p.join("::")));
        assert_eq!(m("src/cmd/get.rs"), Some(("mini_redis".into(), "cmd::get".into())));
        assert_eq!(m("src/shards.rs"), Some(("mini_redis".into(), "db::sharded".into())));
        let none = |_: &str| false;
        assert_eq!(r.resolve("src/cmd/get.rs", &["crate", "Db"], none), Resolution::Module { file: "src/lib.rs".into(), used: 1 });
        assert_eq!(r.resolve("src/cmd/get.rs", &["super", "Get"], none), Resolution::Module { file: "src/cmd/mod.rs".into(), used: 1 });
        assert_eq!(r.resolve("src/bin/cli.rs", &["mini_redis", "cmd", "Get"], none), Resolution::Module { file: "src/cmd/mod.rs".into(), used: 2 });
        assert_eq!(r.resolve("src/lib.rs", &["db", "Db"], none), Resolution::Module { file: "src/db.rs".into(), used: 1 });
        assert_eq!(r.resolve("src/lib.rs", &["tokio", "net"], none), Resolution::External("tokio".into()));
        assert_eq!(r.resolve("src/lib.rs", &["Db", "new"], |n| n == "Db"), Resolution::Local);
    }
}
