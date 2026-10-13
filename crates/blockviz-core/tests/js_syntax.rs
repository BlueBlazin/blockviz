//! JavaScript and TypeScript syntax, ES2015 through ES2025 plus TypeScript's
//! own declarations, parses and reads the way a developer would describe it.

use blockviz_core::ir::{Kind, Node, Scene};
use blockviz_core::project::InFile;

fn analyze(files: &[(&str, &str)]) -> Scene {
    blockviz_core::analyze("t", files.iter().map(|(p, t)| InFile { path: (*p).into(), text: Some((*t).into()), bytes: t.len() as u64 }).collect())
}

fn one(name: &str, src: &str) -> Scene {
    let s = analyze(&[(name, src)]);
    let errors: Vec<&String> = s.diagnostics.iter().filter(|d| d.contains("Syntax error") || d.contains("Skipped")).collect();
    assert!(errors.is_empty(), "unexpected syntax errors: {errors:?}");
    assert!(!s.nodes.iter().any(|n| n.badges.iter().any(|b| b == "syntax error")));
    s
}

fn find<'a>(s: &'a Scene, title: &str) -> &'a Node {
    s.nodes.iter().find(|n| n.title == title).unwrap_or_else(|| panic!("no block titled {title:?}"))
}

fn all<'a>(s: &'a Scene, title: &str) -> Vec<&'a Node> {
    s.nodes.iter().filter(|n| n.title == title).collect()
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

fn rows(n: &Node) -> Vec<(&str, &str)> {
    n.rows.iter().filter(|r| !r.heading).map(|r| (r.key.as_str(), r.value.as_str())).collect()
}

fn has(n: &Node, badge: &str) -> bool {
    n.badges.iter().any(|b| b == badge)
}

#[test]
fn modern_javascript() {
    let s = one("modern.mjs", include_str!("fixtures/js-syntax/modern.mjs"));
    let file = s.nodes.iter().find(|n| n.kind == Kind::File).unwrap();
    assert_eq!(file.doc, "ES2015–ES2025 syntax in one module.");
    // The header describes the file; it does not also title the first paragraph.
    assert!(!s.nodes.iter().any(|n| n.kind == Kind::Section && n.title.starts_with("ES2015")));
    // Settings build on one another, logical assignments included: one table,
    // where an update reads as an update.
    let settings = find(&s, "Settings");
    assert_eq!(rows(settings), [("total", "1_000_000n ** 2n"), ("total", "||= 1n"), ("total", "&&= total"), ("maybe", "rest?.deep?.value ?? 'fallback'"), ("maybe", "??= 'x'")]);
    // Classes: accessors, private members, static and generator methods.
    let values = all(&s, "value");
    assert!(values.iter().any(|v| has(v, "getter")), "{values:?}");
    let setter = values.iter().find(|v| has(v, "setter")).expect("setter");
    assert_eq!(setter.effects, ["mutates this.#count"]);
    assert!(has(find(&s, "has"), "static"));
    let stream = find(&s, "stream");
    assert!(has(stream, "async") && has(stream, "generator"), "{:?}", stream.badges);
    find(&s, "#bump");
    // A labeled loop nest keeps its loops and exits.
    let outer = s.nodes.iter().find(|n| n.kind == Kind::Loop && n.title.starts_with("for x of")).expect("outer loop");
    assert!(descendants(&s, outer).iter().any(|n| n.kind == Kind::Loop && n.title == "for i < x"));
    // Code is named, never shown cut off, and never in Python's words.
    for n in &s.nodes {
        assert!(!n.effects.iter().any(|e| e.contains("self.")), "{n:?}");
        assert!(!n.title.contains('\n') && !n.title.contains("=>"), "{:?}", n.title);
    }
}

#[test]
fn typescript_declarations() {
    let s = one("types.ts", include_str!("fixtures/js-syntax/types.ts"));
    // Interfaces and enums are data: their members are rows, values or not.
    assert_eq!(rows(find(&s, "Shape")), [("kind", "'circle' | 'square'"), ("area", "(): number")]);
    assert_eq!(rows(find(&s, "Direction")), [("Up", "1"), ("Down", ""), ("Left", "'LEFT'")]);
    assert_eq!(rows(find(&s, "Color")), [("Red", ""), ("Green", ""), ("Blue", "")]);
    let types = find(&s, "Types");
    assert!(rows(types).iter().any(|r| r.0 == "Point"), "{types:?}");
    // A long type is elided by its brackets, never to nothing.
    assert!(rows(types).contains(&("Mapped", "{ [K in keyof T as `…`]: () => T[K] }")), "{:?}", rows(types));
    // A namespace groups its functions.
    let geometry = find(&s, "Geometry");
    assert_eq!(geometry.subtitle, "namespace");
    assert!(descendants(&s, geometry).iter().any(|n| n.title == "scale"));
    // Ambient declarations describe code elsewhere.
    assert!(s.nodes.iter().all(|n| n.title != "Request" && n.title != "Window" && n.title != "brand"));
    // Parameter properties are the constructor's parameters.
    let ctor = find(&s, "constructor");
    assert_eq!(ctor.subtitle, "(store, id)");
    // A class extending a project class named `Base` is not an ORM table.
    let user = find(&s, "User");
    assert!(has(user, "@Entity") && !has(user, "DB table"), "{:?}", user.badges);
    // Overload signatures are one function.
    assert_eq!(all(&s, "overload").len(), 1);
    // A lone object of settings lists its keys.
    assert_eq!(rows(find(&s, "conf")), [("port", "3000"), ("host", "'localhost'")]);
    let assert_point = find(&s, "assertPoint");
    assert!(descendants(&s, assert_point).iter().any(|n| n.title == "isPoint" && n.reference.is_some()));
}

#[test]
fn react_component() {
    let s = one("component.tsx", include_str!("fixtures/js-syntax/component.tsx"));
    let list = find(&s, "List");
    assert_eq!(list.category, "component");
    assert_eq!(list.inputs, ["items", "render", "onPick"]);
    let inside = descendants(&s, list);
    let titled = |t: &str| *inside.iter().find(|n| n.title == t).unwrap_or_else(|| panic!("no {t:?} in List"));
    // A namespaced element is named by its tag.
    assert_eq!(titled("Dialog.Root").subtitle, "");
    titled("useState");
    // Rendered per item, or run when an event happens.
    assert!(has(titled("render"), "each"));
    let row = titled("Row");
    assert!(has(row, "each") && row.reference.is_some());
    let pick = titled("onPick");
    assert!(has(pick, "on click") && !has(pick, "each"), "{:?}", pick.badges);
    find(&s, "Row");
    assert!(has(find(&s, "Empty"), "@React.memo"));
}

#[test]
fn commonjs_and_umd() {
    let s = analyze(&[("legacy.cjs", include_str!("fixtures/js-syntax/legacy.cjs")), ("helper.js", "module.exports = function helper(args) { return args.length; };\n")]);
    // The UMD wrapper runs in place: its factory is a function, its checks a branch.
    assert!(s.nodes.iter().all(|n| !n.title.starts_with("function")), "an IIFE is not titled by its source");
    let factory = find(&s, "factory");
    assert_eq!(factory.kind, Kind::Function);
    assert!(descendants(&s, factory).iter().any(|n| n.title == "next"));
    // `switch` with fall-through cases.
    let run = find(&s, "run");
    let inside = descendants(&s, run);
    let branch = inside.iter().find(|n| n.kind == Kind::Branch).expect("switch");
    assert_eq!(branch.title, "switch argv[0]");
    let lanes: Vec<&str> = inside.iter().filter(|n| n.kind == Kind::Lane).map(|n| n.title.as_str()).collect();
    assert_eq!(lanes.len(), 2);
    assert!(lanes.contains(&"case 'a', 'b'") && lanes.contains(&"default"), "{lanes:?}");
    // `require` links the files; `exports.run(…)` under the main guard calls `run`.
    let helper = inside.iter().find(|n| n.title == "helper").expect("call to helper");
    assert!(helper.reference.is_some(), "{helper:?}");
    let file = s.nodes.iter().find(|n| n.kind == Kind::File && n.title == "legacy.cjs").unwrap();
    assert!(has(file, "entry point"));
    assert!(file.children.iter().any(|&c| s.nodes[c as usize].title == "run" && s.nodes[c as usize].kind == Kind::Call && s.nodes[c as usize].reference == Some(run.id)));
}

#[test]
fn syntax_errors_keep_the_rest_of_the_file() {
    let s = analyze(&[("broken.ts", include_str!("fixtures/js-syntax/broken.ts"))]);
    assert!(s.diagnostics.iter().any(|d| d.contains("broken.ts:3") && d.contains("rest of the file is still shown")), "{:?}", s.diagnostics);
    let file = s.nodes.iter().find(|n| n.kind == Kind::File).unwrap();
    assert!(has(file, "syntax error"));
    let after = find(&s, "after");
    let fine = find(&s, "fine");
    assert!(descendants(&s, after).iter().any(|n| n.reference == Some(fine.id)));
    // A file oxc cannot read at all still renders, and says so.
    let s = analyze(&[("worse.js", "const = ;\n")]);
    assert!(s.diagnostics.iter().any(|d| d.contains("worse.js:1")), "{:?}", s.diagnostics);
}

#[test]
fn empty_and_comment_only_files() {
    let s = one("empty.js", include_str!("fixtures/js-syntax/empty.js"));
    assert_eq!(s.nodes.iter().filter(|n| n.kind == Kind::File).count(), 1);
    let s = one("comments.js", include_str!("fixtures/js-syntax/comments.js"));
    let file = s.nodes.iter().find(|n| n.kind == Kind::File).unwrap();
    assert_eq!(file.doc, "only a comment and a block");
    assert!(file.children.is_empty());
}

#[test]
fn every_fixture_lays_out() {
    let names = ["modern.mjs", "types.ts", "component.tsx", "legacy.cjs", "broken.ts", "empty.js", "comments.js"];
    let texts = [
        include_str!("fixtures/js-syntax/modern.mjs"),
        include_str!("fixtures/js-syntax/types.ts"),
        include_str!("fixtures/js-syntax/component.tsx"),
        include_str!("fixtures/js-syntax/legacy.cjs"),
        include_str!("fixtures/js-syntax/broken.ts"),
        include_str!("fixtures/js-syntax/empty.js"),
        include_str!("fixtures/js-syntax/comments.js"),
    ];
    let files: Vec<(&str, &str)> = names.iter().copied().zip(texts.iter().copied()).collect();
    let s = analyze(&files);
    for n in &s.nodes {
        assert!(n.w.is_finite() && n.h.is_finite() && n.w >= 0.0 && n.h >= 0.0, "{n:?}");
        if let Some(p) = n.parent {
            let p = &s.nodes[p as usize];
            assert!(n.x >= -0.5 && n.y >= -0.5 && n.x + n.w <= p.cw + 0.5 && n.y + n.h <= p.ch + 0.5, "{} escapes {}", n.title, p.title);
        }
    }
}
