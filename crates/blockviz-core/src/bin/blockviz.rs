//! Prints the diagram tree for a directory, for inspecting abstractions
//! without a browser: `cargo run --bin blockviz -- path/to/repo [--depth N] [--json]`.

use blockviz_core::ir::{Node, Scene};
use std::path::Path;

fn walk(dir: &Path, root: &Path, out: &mut Vec<blockviz_core::project::InFile>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.path());
    for e in entries {
        let p = e.path();
        let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        if p.is_dir() {
            if !blockviz_core::project::ignored(&format!("{rel}/x")) {
                walk(&p, root, out);
            }
        } else {
            let bytes = e.metadata().map(|m| m.len()).unwrap_or(0);
            let text = if bytes < 2_000_000 { std::fs::read_to_string(&p).ok() } else { None };
            out.push(blockviz_core::project::InFile { path: rel, text, bytes });
        }
    }
}

fn print(scene: &Scene, id: u32, depth: usize, max: usize) {
    let n: &Node = &scene.nodes[id as usize];
    let pad = "  ".repeat(depth);
    let mut line = format!("{pad}{:?} {}", n.kind, n.title);
    if !n.subtitle.is_empty() {
        line.push_str(&format!(" — {}", n.subtitle));
    }
    if !n.badges.is_empty() {
        line.push_str(&format!(" [{}]", n.badges.join(", ")));
    }
    if let Some(r) = n.reference {
        line.push_str(&format!(" → #{r} {}", scene.nodes[r as usize].title));
    }
    if !n.effects.is_empty() {
        line.push_str(&format!(" {{{}}}", n.effects.join("; ")));
    }
    line.push_str(&format!("  ({:.0}×{:.0}) #{id}", n.w, n.h));
    println!("{line}");
    if !n.doc.is_empty() && depth < max {
        println!("{pad}  “{}”", n.doc.chars().take(90).collect::<String>());
    }
    if depth >= max {
        return;
    }
    for r in n.rows.iter().take(6) {
        println!("{pad}    | {} {}", r.key, r.value);
    }
    for &c in &n.children {
        print(scene, c, depth + 1, max);
    }
    for e in &n.edges {
        let a = &scene.nodes[e.from as usize].title;
        let b = &scene.nodes[e.to as usize].title;
        println!("{pad}  · {a} ─{:?}{}→ {b}", e.kind, if e.label.is_empty() { String::new() } else { format!(" [{}]", e.label) });
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = args.get(1).expect("usage: blockviz <dir> [--depth N] [--json]");
    let depth = args.iter().position(|a| a == "--depth").and_then(|i| args.get(i + 1)).and_then(|d| d.parse().ok()).unwrap_or(4);
    let root = Path::new(dir);
    let mut files = vec![];
    walk(root, root, &mut files);
    let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let start = std::time::Instant::now();
    let scene = blockviz_core::analyze(&name, files);
    let elapsed = start.elapsed();
    if args.iter().any(|a| a == "--json") {
        println!("{}", serde_json::to_string(&scene).unwrap());
        return;
    }
    println!("{} — {} nodes, depth {}, {:?}", scene.title, scene.stats.nodes, scene.stats.max_depth, elapsed);
    for d in &scene.diagnostics {
        println!("! {d}");
    }
    print(&scene, scene.root, 0, depth);
}
