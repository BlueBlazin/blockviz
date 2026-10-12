//! A report of the abstractions most likely to read badly, over the Python,
//! JavaScript, Rust and Go corpora.
//! Run with `cargo test --test audit -- --ignored --nocapture`.

use blockviz_core::ir::{EdgeKind, Kind, Scene};
use std::path::Path;

fn load(dir: &Path, root: &Path, out: &mut Vec<blockviz_core::project::InFile>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
    entries.sort_by_key(|e| e.path());
    for e in entries {
        let p = e.path();
        let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        if p.is_dir() {
            load(&p, root, out);
        } else {
            let bytes = e.metadata().unwrap().len();
            out.push(blockviz_core::project::InFile { path: rel, text: std::fs::read_to_string(&p).ok(), bytes });
        }
    }
}

fn scene(name: &str) -> (Scene, Vec<blockviz_core::project::InFile>) {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let root = ["corpus", "corpus-js", "corpus-rs", "corpus-go"].iter().map(|c| fixtures.join(c).join(name)).find(|p| p.exists()).unwrap();
    let mut files = vec![];
    load(&root, &root, &mut files);
    let s = blockviz_core::analyze(name, files);
    let mut files = vec![];
    load(&root, &root, &mut files);
    (s, files)
}

/// The first source line of a node, trimmed.
fn line(s: &Scene, files: &[blockviz_core::project::InFile], n: &blockviz_core::ir::Node) -> String {
    let Some(sp) = &n.span else { return String::new() };
    let path = &s.files[sp.file as usize].path;
    let Some(text) = files.iter().find(|f| &f.path == path).and_then(|f| f.text.as_deref()) else { return String::new() };
    text.lines().nth(sp.line.saturating_sub(1) as usize).unwrap_or("").trim().chars().take(100).collect()
}

fn codeish(t: &str) -> bool {
    // `<div>`, `<Dialog.Root>`: an element is named by its tag.
    let tag = t.starts_with('<') && t.ends_with('>') && t[1..t.len() - 1].chars().all(|c| c.is_alphanumeric() || ".:-_".contains(c));
    !tag && (t.contains('…') || t.chars().any(|c| "()[]{}=<>+*/%\"'!".contains(c)) || t.contains(" and ") || t.contains(" or ") || t.contains(" if ") || t.contains(" not "))
}

#[test]
#[ignore]
fn audit() {
    let only = std::env::var("AUDIT").ok();
    let python = ["fastapi-template", "httpx", "nanochat", "flask", "bakerydemo", "httpie"];
    let javascript = ["express-api", "nextjs-app", "nestjs-cats", "react-vite", "ky", "serve", "turborepo-basic"];
    let rust = ["mini-redis", "fd", "ripgrep-core", "axum-examples", "llama2-rs"];
    let go = ["groupcache", "realworld", "redcon", "chi", "glow", "example"];
    for name in python.into_iter().chain(javascript).chain(rust).chain(go) {
        if only.as_deref().is_some_and(|o| o != name) {
            continue;
        }
        let (s, files) = scene(name);
        let blocks: Vec<_> = s.nodes.iter().filter(|n| matches!(n.kind, Kind::Op | Kind::Call)).collect();
        let code: Vec<_> = blocks.iter().filter(|n| codeish(&n.title)).collect();
        let guards: Vec<_> = s.nodes.iter().filter(|n| n.badges.iter().any(|b| b.starts_with("if ") || b.starts_with("unless "))).collect();
        let ctx: usize = s.nodes.iter().flat_map(|n| &n.edges).filter(|e| e.kind == EdgeKind::Context).count();
        let data: usize = s.nodes.iter().flat_map(|n| &n.edges).filter(|e| e.kind == EdgeKind::Data).count();
        println!("== {name}: {} blocks, {} code titles, {} guard tags, edges data {data} / context {ctx}", blocks.len(), code.len(), guards.len());
        let n = std::env::var("AUDIT_N").ok().and_then(|v| v.parse().ok()).unwrap_or(25);
        for b in code.iter().take(n) {
            println!("  code  {:<30} {:<22} | {}", b.title, b.subtitle.chars().take(22).collect::<String>(), line(&s, &files, b));
        }
        let empty: Vec<_> = s.nodes.iter().filter(|l| l.kind == Kind::Lane && l.children.iter().all(|&c| s.nodes[c as usize].is_port())).collect();
        println!("  {} empty lanes", empty.len());
        for l in empty.iter().take(n) {
            println!("  empty {:<30} | {}", l.title, line(&s, &files, l));
        }
        if std::env::var("AUDIT_ALL").is_ok() {
            let ops: Vec<_> = blocks.iter().filter(|b| b.kind == Kind::Op).collect();
            let step = (ops.len() / n.max(1)).max(1);
            for b in ops.iter().step_by(step) {
                println!("  op    {:<30} {:<26} | {}", b.title, b.subtitle.chars().take(26).collect::<String>(), line(&s, &files, b));
            }
        }
        for g in guards.iter().take(n / 2) {
            println!("  guard {:<24} {:?} | {}", g.title, g.badges, line(&s, &files, g));
        }
    }
}
