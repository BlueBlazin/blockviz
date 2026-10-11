//! The repository level: directory tree, cross-file relations, artifacts.
//!
//! Files connect through imports, `exec`, string references, and artifacts
//! written by one program and read by another (`prepare.py → train.bin →
//! train.py`). Relations between files in different directories are lifted
//! through directory ports, the same mechanism function diagrams use.

use crate::ir::*;
use crate::outline::{self, Class};
use crate::python::{self, PyInput};
use crate::text::clip;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub struct InFile {
    pub path: String,
    pub text: Option<String>,
    pub bytes: u64,
}

const IGNORED_DIRS: &[&str] = &[".git", "node_modules", "__pycache__", ".venv", "venv", ".mypy_cache", ".pytest_cache", ".ruff_cache", ".tox", ".idea", ".vscode", ".ipynb_checkpoints", "dist", "build", "target", ".next", ".cache", "site-packages", ".eggs"];
const MAX_TEXT: usize = 2_000_000;

pub fn ignored(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    parts[..parts.len().saturating_sub(1)].iter().any(|p| IGNORED_DIRS.contains(p) || p.ends_with(".egg-info"))
        || matches!(parts.last().copied(), Some(".DS_Store") | Some("Thumbs.db"))
        || path.ends_with(".pyc")
}

struct FileEntry {
    path: String,
    class: Class,
    language: &'static str,
    node: Id,
    text: Option<String>,
}

pub fn build(name: &str, files: Vec<InFile>) -> Scene {
    let mut diagnostics = vec![];
    let mut files: Vec<InFile> = files
        .into_iter()
        .map(|mut f| {
            f.path = f.path.trim_start_matches("./").trim_start_matches('/').replace('\\', "/");
            f
        })
        .filter(|f| !f.path.is_empty() && !ignored(&f.path))
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    // A single wrapping directory (an extracted archive) becomes the repo name.
    let mut title = name.to_string();
    if let Some(first) = files.first().and_then(|f| f.path.split_once('/').map(|(d, _)| d.to_string())) {
        if files.len() > 1 && files.iter().all(|f| f.path.starts_with(&format!("{first}/"))) {
            for f in &mut files {
                f.path = f.path[first.len() + 1..].to_string();
            }
            if title.is_empty() {
                title = first;
            }
        }
    }
    if title.is_empty() {
        title = "project".into();
    }

    let mut metas: Vec<FileMeta> = vec![];
    let mut entries: Vec<FileEntry> = vec![];
    let mut py_inputs = vec![];
    for (i, f) in files.into_iter().enumerate() {
        let (class, language) = outline::classify(&f.path);
        let text = f.text.filter(|t| t.len() <= MAX_TEXT && !t.contains('\0'));
        if text.is_none() && matches!(class, Class::Python | Class::Notebook | Class::Code | Class::Config) && f.bytes as usize > MAX_TEXT {
            diagnostics.push(format!("Skipped {} ({} KB is over the per-file limit).", f.path, f.bytes / 1024));
        }
        let mut source = None;
        match (class, &text) {
            (Class::Python, Some(t)) => py_inputs.push(PyInput { file: i as u32, path: f.path.clone(), text: t.clone(), notebook: false }),
            (Class::Notebook, Some(t)) => match crate::notebook::to_python(t) {
                Some(py) => {
                    source = Some(py.clone());
                    py_inputs.push(PyInput { file: i as u32, path: f.path.clone(), text: py, notebook: true });
                }
                None => diagnostics.push(format!("Could not read notebook {}.", f.path)),
            },
            _ => {}
        }
        metas.push(FileMeta {
            path: f.path.clone(),
            language: language.to_string(),
            bytes: f.bytes,
            lines: text.as_ref().map(|t| t.lines().count() as u32).unwrap_or(0),
            analyzed: text.is_some() && class != Class::Asset,
            source,
        });
        entries.push(FileEntry { path: f.path, class, language, node: 0, text });
    }

    let mut b = Builder::default();
    let root = b.add(None, Kind::Repo, Shape::Box, title.clone());
    b.get_mut(root).category = "repo";
    let (b2, py) = python::analyze(b, py_inputs, &mut diagnostics);
    let mut b = b2;
    let mut py_by_file: HashMap<u32, &python::PyFile> = HashMap::new();
    for pf in &py.files {
        py_by_file.insert(pf.file, pf);
    }

    // File nodes.
    let mut refs: Vec<(usize, Vec<String>)> = vec![];
    let mut code_imports: Vec<(usize, Vec<String>)> = vec![];
    for (i, e) in entries.iter_mut().enumerate() {
        let base = e.path.rsplit('/').next().unwrap_or(&e.path).to_string();
        if let Some(pf) = py_by_file.get(&(i as u32)) {
            e.node = pf.node;
            let n = b.get_mut(pf.node);
            n.title = base;
            if n.category == "neutral" {
                n.category = "file";
            }
            continue;
        }
        let text = e.text.as_deref();
        let (kind, category) = match e.class {
            Class::Code | Class::Python | Class::Notebook => (Kind::File, "file"),
            Class::Config => (Kind::File, "config"),
            Class::Doc => (Kind::Doc, "doc"),
            Class::Asset => (Kind::Asset, "asset"),
            Class::Meta | Class::Other => (Kind::Asset, "meta"),
        };
        let id = b.add(None, kind, Shape::Box, base);
        e.node = id;
        {
            let n = b.get_mut(id);
            n.category = category;
            n.subtitle = e.language.to_string();
            n.span = text.map(|t| Span { file: i as u32, start: 0, end: t.len() as u32, line: 1, end_line: t.lines().count().max(1) as u32 });
        }
        let Some(text) = text else {
            let bytes = metas[i].bytes;
            b.get_mut(id).subtitle = format!("{} · {}", e.language, human_size(bytes));
            continue;
        };
        if matches!(e.class, Class::Asset | Class::Meta | Class::Other) {
            continue;
        }
        let o = outline::outline(e.language, text);
        b.get_mut(id).doc = o.doc.clone();
        if !o.imports.is_empty() {
            b.get_mut(id).inputs = o.imports.clone();
            code_imports.push((i, o.imports.clone()));
        }
        let runner = matches!(e.language, "Shell" | "PowerShell" | "Makefile" | "Dockerfile")
            || (e.path.starts_with(".github/workflows/") && matches!(e.language, "YAML"))
            || e.path.ends_with("package.json");
        if !o.refs.is_empty() && runner {
            refs.push((i, o.refs.clone()));
        }
        if !o.symbols.is_empty() {
            b.get_mut(id).badges.push("outline".into());
            let mut by_name: HashMap<String, Id> = HashMap::new();
            for s in &o.symbols {
                let sid = b.add(Some(id), Kind::Symbol, Shape::Box, s.name.clone());
                let line = text[..s.start].matches('\n').count() as u32 + 1;
                let end_line = text[..s.end.min(text.len())].matches('\n').count() as u32 + 1;
                let n = b.get_mut(sid);
                n.subtitle = s.kind.to_string();
                n.category = match s.kind {
                    "function" | "target" => "function",
                    "kernel" => "kernel",
                    _ => "class",
                };
                n.span = Some(Span { file: i as u32, start: s.start as u32, end: s.end.min(text.len()) as u32, line, end_line });
                by_name.entry(s.name.clone()).or_insert(sid);
            }
            for s in &o.symbols {
                let caller = by_name[&s.name];
                for c in &s.calls {
                    if let Some(&callee) = by_name.get(c) {
                        b.connect(callee, caller, EdgeKind::Uses, "");
                    }
                }
            }
        } else if !o.rows.is_empty() && (e.class != Class::Doc || o.rows.len() >= 2) {
            b.get_mut(id).rows = o.rows;
        }
    }
    // Python file roles: an entry point is run (main guard, or top-level
    // actions in a file nobody imports); everything else is a module.
    let imported: HashSet<u32> = py.files.iter().flat_map(|f| f.imports.iter().map(|(t, _)| *t)).collect();
    for pf in &py.files {
        let node = pf.node;
        let kids: Vec<Kind> = b.get(node).children.iter().map(|&c| b.get(c).kind).collect();
        let notebook = b.get(node).kind == Kind::Notebook;
        let only_settings = !kids.is_empty() && kids.iter().all(|k| *k == Kind::Table);
        let acts = b.get(node).effects.iter().any(|e| ["prints", "reads file", "writes file", "network", "executes code", "spawns process", "distributed", "logs"].iter().any(|a| e.starts_with(a)));
        let runs = pf.entry || (acts && !imported.contains(&pf.file) && !kids.iter().all(|k| matches!(k, Kind::Class | Kind::Function | Kind::Table)));
        let role = if pf.error.is_some() {
            "Python"
        } else if notebook {
            "notebook"
        } else if only_settings && !imported.contains(&pf.file) {
            "config"
        } else if runs {
            "script"
        } else {
            "module"
        };
        let n = b.get_mut(node);
        n.subtitle = role.to_string();
        if role == "config" {
            n.category = "config";
        }
        if runs && !notebook {
            n.badges.insert(0, "entry point".into());
        }
    }

    // Directory tree.
    let mut dirs: BTreeMap<String, Id> = BTreeMap::new();
    for e in &entries {
        let parent = match e.path.rsplit_once('/') {
            Some((dir, _)) => ensure_dir(&mut b, &mut dirs, root, dir),
            None => root,
        };
        crate::python::flow::attach(&mut b, parent, e.node);
    }
    // READMEs describe their directory.
    let mut description = String::new();
    let containers: Vec<Id> = std::iter::once(root).chain(dirs.values().copied()).collect();
    for &c in &containers {
        let readme = b.get(c).children.iter().copied().find(|&k| {
            let t = b.get(k).title.to_ascii_lowercase();
            b.get(k).kind == Kind::Doc && t.starts_with("readme")
        });
        if let Some(r) = readme {
            let doc = b.get(r).doc.clone();
            if c == root {
                description = doc;
            } else {
                if b.get(c).doc.is_empty() {
                    b.get_mut(c).doc = doc;
                }
                b.remove(r);
            }
        }
    }
    // Build and tooling configuration gathers into one group per directory.
    for &c in &containers {
        let tooling: Vec<Id> = b
            .get(c)
            .children
            .iter()
            .copied()
            .filter(|&k| {
                let n = b.get(k);
                let t = n.title.to_ascii_lowercase();
                (n.kind == Kind::File && n.category == "config")
                    || matches!(t.as_str(), "setup.py" | "noxfile.py" | "fabfile.py" | "setup.cfg" | "manifest.in" | ".github/" | ".circleci/" | ".devcontainer/")
                    || (n.kind == Kind::Asset && n.category == "meta")
            })
            .collect();
        let has_other = b.get(c).children.len() > tooling.len() + 1;
        if tooling.len() >= 3 && has_other {
            let g = b.add(Some(c), Kind::Dir, Shape::Box, "Build & tooling");
            b.get_mut(g).category = "dir";
            b.get_mut(g).hints.pack = 9;
            for k in tooling {
                crate::python::flow::attach(&mut b, g, k);
            }
        }
    }
    // Docs, assets and repo metadata gather into one group per directory.
    for &c in &containers {
        let others: Vec<Id> = b
            .get(c)
            .children
            .iter()
            .copied()
            .filter(|&k| {
                let n = b.get(k);
                matches!(n.kind, Kind::Doc | Kind::Asset) || (n.kind == Kind::Dir && subtree_is_static(&b, k))
            })
            .collect();
        let has_code = b.get(c).children.iter().any(|&k| !others.contains(&k) && b.get(k).title != "Build & tooling");
        if others.len() >= 2 && has_code {
            let g = b.add(Some(c), Kind::Dir, Shape::Box, "Docs & assets");
            b.get_mut(g).category = "dir";
            b.get_mut(g).hints.pack = 10;
            for k in others {
                crate::python::flow::attach(&mut b, g, k);
            }
        }
    }
    // A directory holding a single item reads better as that item.
    collapse_single_children(&mut b, root);

    // Relations.
    let mut ports: HashMap<(Id, String, bool), Id> = HashMap::new();
    let all_paths: HashMap<&str, usize> = entries.iter().enumerate().map(|(i, e)| (e.path.as_str(), i)).collect();
    let node_of_file = |f: u32| entries[f as usize].node;
    let mut linked: HashSet<(Id, Id)> = HashSet::new();
    for pf in &py.files {
        for (target, names) in &pf.imports {
            let (from, to) = (node_of_file(*target), pf.node);
            let mut names: Vec<String> = names.clone();
            names.dedup();
            let module = b.get(from).title.clone();
            lift_titled(&mut b, from, to, EdgeKind::Uses, &clip(&names.join(", "), 30), &module, &mut ports);
            linked.insert((from, to));
        }
    }
    for (from, to) in &py.execs {
        let (a, c) = (node_of_file(*to), node_of_file(*from));
        if linked.insert((a, c)) {
            lift(&mut b, a, c, EdgeKind::Exec, "exec", &mut ports);
        }
    }
    let path_index: HashMap<&str, usize> = entries.iter().enumerate().map(|(i, e)| (e.path.as_str(), i)).collect();
    let mut by_base: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        by_base.entry(e.path.rsplit('/').next().unwrap_or(&e.path)).or_default().push(i);
    }
    for (i, mentions) in &refs {
        let dir = entries[*i].path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        for m in mentions {
            let candidates = [m.clone(), if dir.is_empty() { m.clone() } else { format!("{dir}/{m}") }];
            let target = candidates.iter().find_map(|c| path_index.get(c.as_str()).copied()).or_else(|| match by_base.get(m.as_str()) {
                Some(v) if v.len() == 1 && !m.contains('/') => Some(v[0]),
                _ => None,
            });
            if let Some(t) = target {
                if t != *i && matches!(entries[t].class, Class::Python | Class::Notebook | Class::Code | Class::Config) {
                    let (a, c) = (entries[t].node, entries[*i].node);
                    if linked.insert((a, c)) {
                        lift(&mut b, a, c, EdgeKind::Uses, "runs", &mut ports);
                    }
                }
            }
        }
    }
    // Imports in other languages, resolved to files.
    for (i, specs) in &code_imports {
        for spec in specs {
            if let Some(t) = resolve_import(&entries[*i].path, entries[*i].language, spec, &all_paths) {
                if t != *i {
                    let (a, c) = (entries[t].node, entries[*i].node);
                    if linked.insert((a, c)) {
                        let module = b.get(a).title.clone();
                        lift_titled(&mut b, a, c, EdgeKind::Uses, "", &module, &mut ports);
                    }
                }
            }
        }
    }
    artifacts(&mut b, &py.artifacts, &entries, &mut ports);
    // Files list what they import, before redundant lines are dropped.
    for pf in &py.files {
        let mut deps: Vec<String> = pf.imports.iter().map(|(t, _)| entries[*t as usize].path.clone()).collect();
        deps.sort();
        if !deps.is_empty() {
            b.get_mut(pf.node).inputs = deps;
        }
    }
    let structural: Vec<Id> = std::iter::once(root).chain(b.nodes.iter().filter(|n| n.kind == Kind::Dir).map(|n| n.id)).collect();
    for &c in &structural {
        transitive_reduction(&mut b, c);
        // A folder exporting to (or importing from) many places says so with
        // its outer edges; per-module port wiring inside would be noise.
        let import_ports: Vec<Id> = b.get(c).children.iter().copied().filter(|&k| b.get(k).is_port() && b.get(c).edges.iter().any(|e| (e.from == k || e.to == k) && e.kind == EdgeKind::Uses)).collect();
        if import_ports.len() > 4 {
            for p in import_ports {
                b.remove(p);
            }
        }
    }

    // External packages, for orientation.
    let mut externals: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    for pf in &py.files {
        for e in &pf.externals {
            if !e.is_empty() && !is_stdlib(e) {
                externals.entry(e.clone()).or_default().insert(pf.file);
            }
        }
    }
    if !externals.is_empty() {
        let t = b.add(Some(root), Kind::Table, Shape::Box, "External packages");
        let n = b.get_mut(t);
        n.category = "library";
        n.hints.pack = 20;
        n.rows = externals
            .iter()
            .map(|(k, files)| Row { key: k.clone(), value: format!("{} file{}", files.len(), if files.len() == 1 { "" } else { "s" }), note: String::new(), heading: false })
            .collect();
        n.doc = "Third-party packages imported by the Python code.".into();
    }

    aggregate_effects(&mut b, root);
    // A one-file project opens directly on that file.
    let mut scene_root = root;
    let top: Vec<Id> = b.get(root).children.clone();
    let code_files: Vec<Id> = top.iter().copied().filter(|&c| matches!(b.get(c).kind, Kind::File | Kind::Notebook)).collect();
    if entries.len() == 1 && code_files.len() == 1 && b.get(code_files[0]).has_interior() {
        scene_root = code_files[0];
        b.get_mut(scene_root).parent = None;
    }
    crate::layout::layout(&mut b, scene_root);
    let python_files = py.files.len() as u32;
    let lines: u32 = metas.iter().map(|m| m.lines).sum();
    let max_depth = b.nodes.iter().map(|n| n.id).filter(|&id| b.get(id).parent.is_some()).map(|id| b.depth(id)).max().unwrap_or(0);
    let (nodes, scene_root) = b.finish(scene_root);
    diagnostics.push("Static analysis: arrows show likely dataflow and dependencies inferred from source, not a runtime trace. Dynamic dispatch and code passed through strings may be missed.".into());
    let stats = Stats { files: metas.len() as u32, python_files, lines, nodes: nodes.len() as u32, max_depth };
    Scene { version: 2, title, description, root: scene_root, nodes, files: metas, diagnostics, stats }
}

/// Drops a dependency A→C when A→…→C already holds through other siblings,
/// so dependency diagrams show their layering instead of a hairball.
fn transitive_reduction(b: &mut Builder, container: Id) {
    let edges = &b.get(container).edges;
    let uses: Vec<(Id, Id)> = edges.iter().filter(|e| e.kind == EdgeKind::Uses && !b.get(e.from).is_port() && !b.get(e.to).is_port()).map(|e| (e.from, e.to)).collect();
    if uses.len() < 3 {
        return;
    }
    let mut adj: HashMap<Id, Vec<Id>> = HashMap::new();
    for &(a, c) in &uses {
        adj.entry(a).or_default().push(c);
    }
    let reaches_without = |from: Id, to: Id| -> bool {
        // Is `to` reachable from `from` by a path of length ≥ 2?
        let mut stack: Vec<Id> = adj.get(&from).map(|v| v.iter().copied().filter(|&n| n != to).collect()).unwrap_or_default();
        let mut seen: HashSet<Id> = stack.iter().copied().collect();
        while let Some(n) = stack.pop() {
            if n == to {
                return true;
            }
            for &m in adj.get(&n).map(|v| v.as_slice()).unwrap_or(&[]) {
                if m == to {
                    return true;
                }
                if seen.insert(m) && seen.len() < 4000 {
                    stack.push(m);
                }
            }
        }
        false
    };
    let redundant: HashSet<(Id, Id)> = uses.iter().copied().filter(|&(a, c)| reaches_without(a, c)).collect();
    if redundant.is_empty() {
        return;
    }
    b.get_mut(container).edges.retain(|e| !(e.kind == EdgeKind::Uses && redundant.contains(&(e.from, e.to))));
}

/// Maps an import specifier to a repo file, per language convention.
fn resolve_import(from: &str, lang: &str, spec: &str, paths: &HashMap<&str, usize>) -> Option<usize> {
    let dir = from.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let join = |base: &str, rel: &str| -> String {
        let mut parts: Vec<&str> = if base.is_empty() { vec![] } else { base.split('/').collect() };
        for p in rel.split('/') {
            match p {
                "." | "" => {}
                ".." => {
                    parts.pop();
                }
                p => parts.push(p),
            }
        }
        parts.join("/")
    };
    let find = |candidates: &[String]| candidates.iter().find_map(|c| paths.get(c.as_str()).copied());
    match lang {
        "JavaScript" | "TypeScript" | "Vue" | "Svelte" => {
            if !spec.starts_with('.') && !spec.starts_with('/') {
                return None;
            }
            let base = join(dir, spec.trim_start_matches('/'));
            let stem = base.trim_end_matches(".js").trim_end_matches(".jsx").to_string();
            let mut c = vec![base.clone()];
            for ext in ["ts", "tsx", "js", "jsx", "mjs", "cjs", "vue", "svelte", "d.ts"] {
                c.push(format!("{stem}.{ext}"));
            }
            for ext in ["ts", "tsx", "js", "jsx"] {
                c.push(format!("{base}/index.{ext}"));
            }
            find(&c)
        }
        "Rust" => {
            let file = from.rsplit('/').next().unwrap_or(from);
            if let Some(m) = spec.strip_prefix("mod:") {
                let here = if matches!(file, "lib.rs" | "main.rs" | "mod.rs") { dir.to_string() } else { join(dir, file.trim_end_matches(".rs")) };
                find(&[join(&here, &format!("{m}.rs")), join(&here, &format!("{m}/mod.rs"))])
            } else if let Some(m) = spec.strip_prefix("crate:") {
                // The crate root is the nearest ancestor `src/`.
                let root = match from.rfind("src/") {
                    Some(i) => &from[..i + 3],
                    None => dir,
                };
                find(&[format!("{root}/{m}.rs"), format!("{root}/{m}/mod.rs")])
            } else {
                None
            }
        }
        _ => {
            let c = vec![join(dir, spec), spec.to_string()];
            find(&c).or_else(|| {
                let base = spec.rsplit('/').next().unwrap_or(spec);
                let hits: Vec<usize> = paths.iter().filter(|(p, _)| p.rsplit('/').next() == Some(base)).map(|(_, &i)| i).collect();
                (hits.len() == 1).then(|| hits[0])
            })
        }
    }
}

fn human_size(bytes: u64) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / (1 << 20) as f64)
    } else if bytes >= 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{bytes} B")
    }
}

fn ensure_dir(b: &mut Builder, dirs: &mut BTreeMap<String, Id>, root: Id, dir: &str) -> Id {
    if let Some(&d) = dirs.get(dir) {
        return d;
    }
    let (parent, name) = match dir.rsplit_once('/') {
        Some((p, n)) => (ensure_dir(b, dirs, root, p), n),
        None => (root, dir),
    };
    let id = b.add(Some(parent), Kind::Dir, Shape::Box, format!("{name}/"));
    b.get_mut(id).category = "dir";
    dirs.insert(dir.to_string(), id);
    id
}

fn subtree_is_static(b: &Builder, id: Id) -> bool {
    b.get(id).children.iter().all(|&c| {
        let n = b.get(c);
        matches!(n.kind, Kind::Doc | Kind::Asset) || (n.kind == Kind::Dir && subtree_is_static(b, c))
    })
}

fn collapse_single_children(b: &mut Builder, id: Id) {
    let kids = b.get(id).children.clone();
    for k in kids {
        collapse_single_children(b, k);
    }
    for k in b.get(id).children.clone() {
        let n = b.get(k);
        if n.kind != Kind::Dir || n.children.len() != 1 || n.title == "Docs & assets" || n.title == "Build & tooling" {
            continue;
        }
        let only = n.children[0];
        let dir_title = n.title.clone();
        let dir_doc = n.doc.clone();
        let pos = b.get(id).children.iter().position(|&c| c == k).unwrap();
        let child = b.get_mut(only);
        child.title = format!("{}{}", dir_title, child.title);
        if child.doc.is_empty() {
            child.doc = dir_doc;
        }
        child.parent = Some(id);
        let order = b.get(k).hints.order;
        b.get_mut(only).hints.order = order;
        b.get_mut(id).children[pos] = only;
        b.get_mut(k).children.clear();
        b.get_mut(k).parent = None;
    }
}

/// Connects two nodes anywhere in the tree, adding ports on every container crossed.
fn lift(b: &mut Builder, from: Id, to: Id, kind: EdgeKind, label: &str, ports: &mut HashMap<(Id, String, bool), Id>) {
    lift_titled(b, from, to, kind, label, "", ports)
}

/// Like `lift`, with an explicit title for the ports it creates.
fn lift_titled(b: &mut Builder, from: Id, to: Id, kind: EdgeKind, label: &str, title: &str, ports: &mut HashMap<(Id, String, bool), Id>) {
    let chain = |b: &Builder, mut x: Id| {
        let mut v = vec![x];
        while let Some(p) = b.get(x).parent {
            v.push(p);
            x = p;
        }
        v
    };
    let ca = chain(b, from);
    let cb = chain(b, to);
    let Some(&lca) = ca.iter().find(|n| cb.contains(n)) else { return };
    if lca == from || lca == to {
        return;
    }
    let ia = ca.iter().position(|&n| n == lca).unwrap();
    let ib = cb.iter().position(|&n| n == lca).unwrap();
    let port_title = if !title.is_empty() {
        title.to_string()
    } else if label.is_empty() || label == "exec" {
        b.get(from).title.clone()
    } else {
        label.to_string()
    };
    let mut src = from;
    for &container in &ca[1..ia] {
        let key = (container, format!("{port_title}|{kind:?}"), true);
        let port = match ports.get(&key) {
            Some(&p) => p,
            None => {
                let p = b.add(Some(container), Kind::Output, Shape::Port, port_title.clone());
                b.get_mut(p).category = "port";
                ports.insert(key, p);
                p
            }
        };
        b.connect(src, port, kind, label);
        src = container;
    }
    let mut dst = to;
    for &container in &cb[1..ib] {
        let key = (container, format!("{port_title}|{kind:?}"), false);
        let port = match ports.get(&key) {
            Some(&p) => p,
            None => {
                let p = b.add(Some(container), Kind::Input, Shape::Port, port_title.clone());
                b.get_mut(p).category = "port";
                ports.insert(key, p);
                p
            }
        };
        b.connect(port, dst, kind, label);
        dst = container;
    }
    b.connect(src, dst, kind, label);
}

fn glob_match(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == name,
        Some((pre, post)) => name.len() >= pre.len() + post.len() && name.starts_with(pre) && name.ends_with(post),
    }
}

/// Files written by one program and read by another become nodes between them.
fn artifacts(b: &mut Builder, uses: &[(u32, String, bool)], entries: &[FileEntry], ports: &mut HashMap<(Id, String, bool), Id>) {
    let concrete: BTreeSet<String> = uses.iter().filter(|(_, n, _)| !n.contains('*')).map(|(_, n, _)| n.clone()).collect();
    let mut writers: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    let mut readers: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    const CODE: &[&str] = &[".py", ".sh", ".ipynb", ".js", ".ts", ".cfg", ".ini", ".toml", ".yaml", ".yml"];
    for (f, name, write) in uses {
        if CODE.iter().any(|ext| name.ends_with(ext)) {
            continue;
        }
        let names: Vec<String> = if name.contains('*') { concrete.iter().filter(|c| glob_match(name, c)).cloned().collect() } else { vec![name.clone()] };
        for n in names {
            if *write {
                writers.entry(n).or_default().insert(*f);
            } else {
                readers.entry(n).or_default().insert(*f);
            }
        }
    }
    // Merge artifacts that connect exactly the same files.
    let mut grouped: BTreeMap<(Vec<u32>, Vec<u32>), Vec<String>> = BTreeMap::new();
    for (name, ws) in &writers {
        let Some(rs) = readers.get(name) else { continue };
        let rs: BTreeSet<u32> = rs.difference(ws).copied().collect();
        if rs.is_empty() {
            continue;
        }
        grouped.entry((ws.iter().copied().collect(), rs.iter().copied().collect())).or_default().push(name.clone());
    }
    for ((ws, rs), names) in grouped {
        // Place the artifact at the deepest container holding all its users.
        let all: Vec<Id> = ws.iter().chain(&rs).map(|&f| entries[f as usize].node).collect();
        let chains: Vec<Vec<Id>> = all
            .iter()
            .map(|&n| {
                let mut v = vec![];
                let mut x = n;
                while let Some(p) = b.get(x).parent {
                    v.push(p);
                    x = p;
                }
                v
            })
            .collect();
        let Some(home) = chains[0].iter().copied().find(|c| chains.iter().all(|ch| ch.contains(c))) else { continue };
        let id = b.add(Some(home), Kind::Artifact, Shape::Box, names.join(", "));
        let n = b.get_mut(id);
        n.category = "data";
        n.subtitle = "file on disk".into();
        n.doc = format!("Written by {} and read by {}.", list(entries, &ws), list(entries, &rs));
        let title = names.join(", ");
        for &w in &ws {
            lift_titled(b, entries[w as usize].node, id, EdgeKind::Artifact, "", &title, ports);
        }
        for &r in &rs {
            lift_titled(b, id, entries[r as usize].node, EdgeKind::Artifact, "", &title, ports);
        }
    }
}

fn list(entries: &[FileEntry], files: &[u32]) -> String {
    files.iter().map(|&f| entries[f as usize].path.clone()).collect::<Vec<_>>().join(", ")
}

/// Every container shows the effects of what it contains.
fn aggregate_effects(b: &mut Builder, id: Id) -> Vec<String> {
    let kids = b.get(id).children.clone();
    let mut all: BTreeSet<String> = b.get(id).effects.iter().cloned().collect();
    for k in kids {
        all.extend(aggregate_effects(b, k));
    }
    let out: Vec<String> = all.into_iter().filter(|e| !e.starts_with("__")).collect();
    let n = b.get_mut(id);
    if n.has_interior() || n.kind == Kind::Repo || n.kind == Kind::Dir {
        // Containers summarise: "reads files" rather than every file name.
        let mut generic: Vec<String> = out.iter().map(|e| generic_effect(e)).collect();
        generic.sort();
        generic.dedup();
        n.effects = generic;
    }
    out
}

fn generic_effect(e: &str) -> String {
    if e.starts_with("reads file") {
        "reads files".into()
    } else if e.starts_with("writes file") {
        "writes files".into()
    } else if e.starts_with("mutates self") {
        "mutates state".into()
    } else if e.starts_with("mutates global") {
        "mutates globals".into()
    } else if e.starts_with("logs metrics") {
        "network".into()
    } else {
        e.to_string()
    }
}

const STDLIB: &[&str] = &[
    "calendar", "filecmp", "netrc", "select", "ssl", "winreg", "_typeshed", "StringIO", "cStringIO", "BaseHTTPServer", "SimpleHTTPServer", "Queue", "urlparse", "urllib2", "cookielib", "httplib", "Cookie", "ConfigParser", "cPickle", "_thread", "__builtin__", "imp", "mimetypes", "binascii", "zlib", "lzma", "bz2", "cmath", "difflib", "getopt", "optparse", "readline", "rlcompleter", "sched", "selectors", "smtplib", "socketserver", "stat", "symtable", "sysconfig", "termios", "tty", "venv", "webbrowser", "wsgiref", "xmlrpc", "zipimport", "posixpath", "ntpath", "genericpath", "encodings", "errno", "fcntl", "grp", "pwd", "resource", "posix", "nt", "msvcrt", "marshal", "ipaddress", "hmac", "string", "graphlib", "tracemalloc", "faulthandler", "trace", "pydoc", "doctest", "configparser", "contextvars", "dataclasses",
    "os", "sys", "re", "math", "json", "time", "datetime", "pickle", "random", "collections", "itertools", "functools", "typing", "dataclasses", "pathlib", "subprocess", "shutil", "logging", "argparse", "inspect", "contextlib", "copy", "abc", "enum", "io", "glob", "tempfile", "threading", "multiprocessing", "asyncio", "unittest", "string", "struct", "hashlib", "base64", "uuid", "csv", "gzip", "zipfile", "tarfile", "urllib", "http", "socket", "signal", "traceback", "warnings", "weakref", "heapq", "bisect", "operator", "statistics", "textwrap", "pprint", "queue", "platform", "getpass", "secrets", "ast", "dis", "importlib", "pkgutil", "types", "decimal", "fractions", "numbers", "array", "codecs", "locale", "unicodedata", "concurrent", "sqlite3", "xml", "html", "email", "ctypes", "gc", "atexit", "builtins", "__future__", "tomllib", "zoneinfo", "shlex", "fnmatch", "timeit", "cProfile", "pdb",
];

fn is_stdlib(name: &str) -> bool {
    STDLIB.contains(&name)
}
