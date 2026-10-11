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
    assert!(s.nodes.iter().any(|n| n.title == "path" && n.subtitle == "os.path.join"), "a value is named by its name, with the call that makes it");
}

fn kids<'a>(s: &'a blockviz_core::ir::Scene, n: &blockviz_core::ir::Node) -> Vec<&'a blockviz_core::ir::Node> {
    n.children.iter().map(|&c| &s.nodes[c as usize]).filter(|c| !c.is_port()).collect()
}

fn descendants<'a>(s: &'a blockviz_core::ir::Scene, n: &'a blockviz_core::ir::Node) -> Vec<&'a blockviz_core::ir::Node> {
    let mut out = vec![];
    let mut stack: Vec<u32> = n.children.clone();
    while let Some(id) = stack.pop() {
        let c = &s.nodes[id as usize];
        out.push(c);
        stack.extend(c.children.iter().copied());
    }
    out
}

#[test]
fn every_return_is_visible() {
    // Flask's `get_send_file_max_age`: three ways to produce a result, none of
    // which calls anything a block would be made for.
    let s = analyze(
        r#"
from datetime import timedelta
from flask import current_app

def get_send_file_max_age(filename):
    value = current_app.config["SEND_FILE_MAX_AGE_DEFAULT"]
    if value is None:
        return None
    if isinstance(value, timedelta):
        return int(value.total_seconds())
    return value  # type: ignore[no-any-return]
"#,
    );
    let f = s.nodes.iter().find(|n| n.title == "get_send_file_max_age").unwrap();
    let steps = kids(&s, f);
    // Guarded exits are steps tagged with their condition, not nested empty lanes.
    assert!(steps.iter().all(|n| n.kind != blockviz_core::ir::Kind::Branch), "{:?}", steps.iter().map(|n| &n.title).collect::<Vec<_>>());
    let exits: Vec<&&blockviz_core::ir::Node> = steps.iter().filter(|n| n.title.starts_with("return")).collect();
    assert_eq!(exits.len(), 2, "{:?}", steps.iter().map(|n| (&n.title, &n.subtitle)).collect::<Vec<_>>());
    assert!(exits.iter().any(|n| n.title == "return None" && n.badges == ["if value is None"]));
    assert!(exits.iter().any(|n| n.subtitle == "int(value.total_seconds())" && n.badges == ["if isinstance(value, timedelta)"]));
    // The config lookup is a step, and every result reaches the return port.
    let lookup = steps.iter().find(|n| n.title == "value").expect("value lookup");
    let port = f.children.iter().map(|&c| &s.nodes[c as usize]).find(|n| n.kind == blockviz_core::ir::Kind::Output).expect("return port");
    let into_port: Vec<u32> = f.edges.iter().filter(|e| e.to == port.id).map(|e| e.from).collect();
    assert!(into_port.contains(&lookup.id));
    for e in &exits {
        assert!(into_port.contains(&e.id), "{} reaches the return port", e.title);
    }
}

#[test]
fn branch_arms_are_never_empty() {
    let s = analyze(
        r#"
def register(username, password):
    error = None
    if not username:
        error = "Username is required."
    elif not password:
        error = "Password is required."
    if error is None:
        save(username, password)
    return error

def sign(x):
    if x > 0:
        return 1
    else:
        return -1
"#,
    );
    for f in ["register", "sign"] {
        let f = s.nodes.iter().find(|n| n.title == f).unwrap();
        for lane in descendants(&s, f).into_iter().filter(|n| n.kind == blockviz_core::ir::Kind::Lane) {
            assert!(!kids(&s, lane).is_empty(), "lane {} in {} is empty", lane.title, f.title);
        }
    }
}

#[test]
fn arrows_say_what_a_value_is_to_the_step() {
    use blockviz_core::ir::EdgeKind;
    let s = analyze(
        r#"
from app.io import load, store, render

def f(path, limit):
    items = load(path)
    n = len(items)
    total = int(items)
    if limit:
        store(items)
    render(n, total)
"#,
    );
    let f = s.nodes.iter().find(|n| n.title == "f").unwrap();
    let kind = |from: &str, to: &str| {
        let id = |t: &str| f.children.iter().copied().find(|&c| s.nodes[c as usize].title == t).unwrap_or_else(|| panic!("no {t}"));
        f.edges.iter().find(|e| e.from == id(from) && e.to == id(to)).map(|e| e.kind)
    };
    // A size is metadata, drawn lightly downstream; a converted value is still data.
    assert_eq!(kind("n", "render"), Some(EdgeKind::Context));
    assert_eq!(kind("total", "render"), Some(EdgeKind::Data));
    // The condition decides whether `store` runs; `items` is also its data.
    assert_eq!(kind("limit", "store"), Some(EdgeKind::Control));
    assert_eq!(kind("load", "store"), Some(EdgeKind::Data));
}
