//! Every Python release's syntax, 3.8 through 3.15, parses without errors and
//! produces the expected diagrams. Fixtures for ≤3.14 are checked against a real
//! CPython 3.14; the 3.15 forms are taken from CPython 3.15's own test suite.

use blockviz_core::ir::{Kind, Node, Scene};
use blockviz_core::project::InFile;

fn analyze(name: &str, src: &str) -> Scene {
    blockviz_core::analyze("t", vec![InFile { path: name.into(), text: Some(src.into()), bytes: src.len() as u64 }])
}

fn clean(s: &Scene) {
    let errors: Vec<&String> = s.diagnostics.iter().filter(|d| d.contains("Syntax error") || d.contains("Skipped")).collect();
    assert!(errors.is_empty(), "unexpected syntax errors: {errors:?}");
    assert!(!s.nodes.iter().any(|n| n.badges.iter().any(|b| b == "syntax error")));
}

fn find<'a>(s: &'a Scene, title: &str) -> &'a Node {
    s.nodes.iter().find(|n| n.title == title).unwrap_or_else(|| panic!("no block titled {title:?}"))
}

fn descendants<'a>(s: &'a Scene, n: &'a Node) -> Vec<&'a Node> {
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
fn python_3_8_and_3_9() {
    let s = analyze("a.py", include_str!("fixtures/syntax/py38_39.py"));
    clean(&s);
    let f = find(&s, "scale");
    assert_eq!(f.inputs, ["values", "factor", "clamp"]);
    // The relaxed decorator expression `@buttons[0].cache` is functools.cache.
    assert!(f.badges.iter().any(|b| b == "cached"), "{:?}", f.badges);
}

#[test]
fn python_3_10_and_3_11() {
    let s = analyze("a.py", include_str!("fixtures/syntax/py310_311.py"));
    clean(&s);
    let route = find(&s, "route");
    let lanes: Vec<&str> = descendants(&s, route).iter().filter(|n| n.kind == Kind::Lane).map(|n| n.title.as_str()).collect();
    assert!(lanes.iter().any(|l| l.starts_with("case {\"action\": \"copy\"")), "{lanes:?}");
    assert!(lanes.iter().any(|l| l.starts_with("case [first, *others]")));
    let grouped = find(&s, "grouped");
    assert!(descendants(&s, grouped).iter().any(|n| n.title.starts_with("except")));
}

#[test]
fn python_3_12_and_3_13() {
    let s = analyze("a.py", include_str!("fixtures/syntax/py312_313.py"));
    clean(&s);
    find(&s, "Box");
    let describe = find(&s, "describe");
    let call = descendants(&s, describe).into_iter().find(|n| n.title == "first").expect("call to first");
    assert!(call.link.is_some() || call.reference.is_some(), "resolved to the generic function");
    // PEP 701: names inside nested-quote f-strings are still read.
    assert!(describe.edges.iter().any(|e| s.nodes[e.from as usize].title == "songs"));
}

#[test]
fn python_3_14() {
    let s = analyze("a.py", include_str!("fixtures/syntax/py314.py"));
    clean(&s);
    let parse = find(&s, "parse");
    let lanes: Vec<&str> = descendants(&s, parse).iter().filter(|n| n.kind == Kind::Lane).map(|n| n.title.as_str()).collect();
    assert!(lanes.contains(&"except ValueError, TypeError"), "{lanes:?}");
    find(&s, "render");
}

#[test]
fn python_3_15() {
    let s = analyze("a.py", include_str!("fixtures/syntax/py315.py"));
    clean(&s);
    let flatten = find(&s, "flatten");
    assert_eq!(flatten.inputs, ["lists", "dicts"]);
    // Lazily imported modules resolve like ordinary imports.
    let dumps = find(&s, "dumps");
    assert_eq!(dumps.subtitle, "json");
    let sign = find(&s, "sign");
    assert!(descendants(&s, sign).iter().any(|n| n.title == "case +0"));
}

#[test]
fn syntax_errors_keep_the_rest_of_the_file() {
    let s = analyze("a.py", "def broken(:\n    pass\n\ndef fine(x):\n    return helper(x)\n\ndef helper(y):\n    return y * 2\n");
    assert!(s.diagnostics.iter().any(|d| d.contains("Syntax error")));
    find(&s, "fine");
    find(&s, "helper");
}
