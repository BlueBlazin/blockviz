//! Robustness: unusual Python must never panic and should stay readable.

use blockviz_core::project::InFile;

fn analyze(src: &str) -> blockviz_core::ir::Scene {
    blockviz_core::analyze("t", vec![InFile { path: "t.py".into(), text: Some(src.into()), bytes: src.len() as u64 }])
}

#[test]
fn modern_and_odd_constructs() {
    let src = r#"
import asyncio
from dataclasses import dataclass

def deco(f):
    return f

@deco
async def fetch(session, url, *args, retries: int = 3, **kw):
    async with session.get(url) as resp:
        if (n := len(args)) > 2:
            return await resp.text()
        async for chunk in resp.content:
            yield chunk
    try:
        match url:
            case "a" | "b":
                pass
            case {"k": v}:
                raise ValueError(v)
            case _:
                ...
    except* OSError:
        pass
    finally:
        print("done")

class Outer:
    class Inner:
        x: int = 1
    def method(self, f=lambda y: y + 1):
        global G
        G = [i * j for i in range(3) for j in range(i) if j]
        return {k: v for k, v in zip("ab", G)}, (yield from range(3))

def recurse(n):
    return recurse(n - 1) if n else 0

def star(*a, **k):
    x, *rest = a
    del rest
    return x
"#;
    let s = analyze(src);
    assert!(s.nodes.len() > 5);
    let json = serde_json::to_string(&s).unwrap();
    assert!(json.len() > 100);
}

#[test]
fn recursion_is_bounded() {
    let s = analyze("def f(n):\n    return f(n - 1) + g(n)\n\ndef g(n):\n    return f(n)\n");
    // Mutually recursive calls reference each other without infinite nesting.
    assert!(s.nodes.iter().any(|n| n.title == "f"));
    assert!(s.stats.max_depth < 10);
}

#[test]
fn syntax_errors_are_diagnostics_not_failures() {
    let s = analyze("def broken(:\n    pass\n");
    assert!(s.diagnostics.iter().any(|d| d.contains("Syntax error")));
}

#[test]
fn empty_and_whitespace_projects() {
    let s = analyze("");
    assert!(!s.nodes.is_empty());
    let s = blockviz_core::analyze("empty", vec![]);
    assert_eq!(s.nodes.len(), 1);
}

#[test]
fn deep_nesting_does_not_overflow() {
    let mut src = String::new();
    for i in 0..60 {
        src.push_str(&"    ".repeat(i));
        src.push_str(&format!("if x{i}:\n"));
    }
    src.push_str(&"    ".repeat(60));
    src.push_str("y = f(x)\n");
    let s = analyze(&src);
    assert!(s.nodes.len() > 1);
}

#[test]
fn pathological_nesting_is_skipped_not_crashed() {
    let src = format!("x = {}1{}\n", "(".repeat(400), ")".repeat(400));
    let s = analyze(&src);
    assert!(s.diagnostics.iter().any(|d| d.contains("nest")));
}

#[test]
fn early_return_is_an_if_else() {
    // `if c: return a` then more code: the rest only runs otherwise.
    let s = analyze(
        r#"
import os

def open_resource(root, resource, mode="rb", encoding="utf-8"):
    path = os.path.join(root, resource)
    if mode == "rb":
        return open(path, mode)  # pyright: ignore
    return open(path, mode, encoding=encoding)
"#,
    );
    let f = s.nodes.iter().find(|n| n.title == "open_resource").unwrap();
    let branch = f.children.iter().map(|&c| &s.nodes[c as usize]).find(|n| n.kind == blockviz_core::ir::Kind::Branch).expect("a branch");
    let lanes: Vec<&blockviz_core::ir::Node> = branch.children.iter().map(|&c| &s.nodes[c as usize]).filter(|n| n.kind == blockviz_core::ir::Kind::Lane).collect();
    assert_eq!(lanes.iter().map(|l| l.title.as_str()).collect::<Vec<_>>(), ["if mode == \"rb\"", "else"]);
    let opens: Vec<&str> = lanes.iter().flat_map(|l| l.children.iter().map(|&c| &s.nodes[c as usize])).filter(|n| n.title == "open").map(|n| n.subtitle.as_str()).collect();
    assert_eq!(opens, ["path, mode", "path, mode, encoding=encoding"], "arguments tell the calls apart");
    assert!(s.nodes.iter().all(|n| !n.doc.contains("pyright")), "tool directives are not documentation");
    assert!(s.nodes.iter().any(|n| n.title == "os.path.join"));
}
