//! Rust syntax, editions 2015 through 2024, parses and reads the way a
//! developer would describe it. The fixture is a small crate
//! (fixtures/rust-syntax) with a library, a binary, an example, a test, a
//! benchmark and a build script.

use blockviz_core::ir::{EdgeKind, Kind, Node, Scene};
use blockviz_core::project::InFile;
use std::path::Path;

fn load(dir: &Path, root: &Path, out: &mut Vec<InFile>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
    entries.sort_by_key(|e| e.path());
    for e in entries {
        let p = e.path();
        let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        if p.is_dir() {
            load(&p, root, out);
        } else {
            let bytes = e.metadata().unwrap().len();
            out.push(InFile { path: rel, text: std::fs::read_to_string(&p).ok(), bytes });
        }
    }
}

fn tour() -> (Scene, Vec<InFile>) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rust-syntax");
    let mut files = vec![];
    load(&root, &root, &mut files);
    let scene = blockviz_core::analyze("syntax-tour", files);
    let mut files = vec![];
    load(&root, &root, &mut files);
    (scene, files)
}

fn file<'a>(s: &'a Scene, path: &str) -> &'a Node {
    let i = s.files.iter().position(|f| f.path == path).unwrap_or_else(|| panic!("no file {path}"));
    s.nodes.iter().find(|n| matches!(n.kind, Kind::File) && n.span.as_ref().is_some_and(|sp| sp.file as usize == i)).unwrap_or_else(|| panic!("no node for {path}"))
}

/// Everything inside a node, in drawing order (depth first).
fn descendants<'a>(s: &'a Scene, n: &'a Node) -> Vec<&'a Node> {
    let mut out = vec![];
    for &c in &n.children {
        let c = &s.nodes[c as usize];
        out.push(c);
        out.extend(descendants(s, c));
    }
    out
}

fn within<'a>(s: &'a Scene, n: &'a Node, title: &str) -> &'a Node {
    descendants(s, n).into_iter().find(|c| c.title == title).unwrap_or_else(|| panic!("{} has no {title:?}; has {:?}", n.title, descendants(s, n).iter().map(|c| &c.title).collect::<Vec<_>>()))
}

fn kids<'a>(s: &'a Scene, n: &Node) -> Vec<&'a Node> {
    n.children.iter().map(|&c| &s.nodes[c as usize]).filter(|c| !matches!(c.kind, Kind::Input | Kind::Output)).collect()
}

fn rows(n: &Node) -> Vec<(&str, &str)> {
    n.rows.iter().filter(|r| !r.heading).map(|r| (r.key.as_str(), r.value.as_str())).collect()
}

fn has(n: &Node, badge: &str) -> bool {
    n.badges.iter().any(|b| b == badge)
}

#[test]
fn every_target_has_its_role() {
    let (s, _) = tour();
    // Only the deliberately broken file reports a syntax error.
    let errors: Vec<&String> = s.diagnostics.iter().filter(|d| d.contains("Syntax error") || d.contains("Skipped")).collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("src/broken.rs:7"), "{errors:?}");
    let role = |p: &str| file(&s, p).subtitle.clone();
    assert_eq!(role("src/lib.rs"), "library");
    assert_eq!(role("src/bin/tool.rs"), "binary");
    assert!(has(file(&s, "src/bin/tool.rs"), "entry point"));
    assert_eq!(role("build.rs"), "build script");
    assert_eq!(role("examples/demo.rs"), "example");
    assert_eq!(role("tests/stack.rs"), "tests");
    assert_eq!(role("benches/area.rs"), "benchmarks");
    assert_eq!(role("src/net/mod.rs"), "module");
    // `//!` documents the file.
    assert_eq!(file(&s, "src/net/mod.rs").doc, "Networking: a client and its wire format.");
    // A `#[path]` module is still a module of the crate, and `mod` alone draws no dependency.
    assert_eq!(role("src/elsewhere.rs"), "module");
    assert!(!file(&s, "src/lib.rs").edges.iter().any(|e| e.kind == EdgeKind::Uses));
}

#[test]
fn modern_syntax() {
    let (s, _) = tour();
    let modern = file(&s, "src/modern.rs");
    // An enum with methods is a type with behaviour; trait impls name their trait.
    let shape = within(&s, modern, "Shape");
    assert_eq!(shape.kind, Kind::Class);
    assert_eq!(shape.subtitle, "Display");
    assert_eq!(within(&s, shape, "fmt").subtitle, "Display");
    // Match arms are the patterns as written, guards included; `|` alternatives share an arm.
    let lanes: Vec<&str> = descendants(&s, within(&s, shape, "area")).into_iter().filter(|n| n.kind == Kind::Lane).map(|n| n.title.as_str()).collect();
    assert_eq!(lanes, ["Shape::Circle { radius } if *radius > 0.0", "Shape::Circle { .. }", "Shape::Square(side)", "Shape::Point, Shape::Custom(_)"]);
    // `let … else` is a guard; an `Err` handed back is an error.
    let push = within(&s, within(&s, modern, "Stack"), "push");
    let error = descendants(&s, push).into_iter().find(|n| n.title == "error item").expect("error step");
    assert_eq!(error.badges, ["unless Some(slot)"]);
    assert_eq!(error.effects, ["returns error"]);
    // `if let … && let …` chains read as written.
    let pop = within(&s, within(&s, modern, "Stack"), "pop");
    assert!(descendants(&s, pop).iter().any(|n| n.kind == Kind::Lane && n.title.starts_with("if let Some(top) = ")), "{:?}", descendants(&s, pop).iter().map(|n| &n.title).collect::<Vec<_>>());
    // Traits: required methods are signatures, provided ones are methods.
    let lender = within(&s, modern, "Lender");
    assert_eq!(lender.subtitle, "trait");
    within(&s, lender, "count_all");
    assert_eq!(rows(within(&s, modern, "Service")), [("call", "(…) -> String")]);
    // Iterator chains collected are comprehensions; the entry API is the map's item.
    let counts = within(&s, modern, "counts");
    assert_eq!(within(&s, counts, "lengths").subtitle, "each of words");
    assert!(descendants(&s, counts).iter().any(|n| n.title == "counts" && n.subtitle.ends_with("+= 1") && n.subtitle.starts_with('[')));
    // Tuple structs and unions are data.
    assert_eq!(rows(within(&s, modern, "Wrapper")), [("0", "Vec<u8>")]);
    assert_eq!(rows(within(&s, modern, "Bits")), [("int", "u32"), ("float", "f32")]);
    // A closure returned is a function of its own.
    within(&s, within(&s, modern, "closures"), "returned function");
    // The test module is a namespace of tests.
    let tests = within(&s, modern, "tests");
    assert_eq!(tests.subtitle, "namespace");
    assert!(has(within(&s, tests, "stack_pushes_and_pops"), "test"));
    // No Python or JavaScript words, and no code cut off mid-token.
    for n in descendants(&s, modern) {
        assert!(!n.title.starts_with("raise ") && !n.title.starts_with("throw ") && !n.title.starts_with("elif ") && !n.title.starts_with("case "), "{:?}", n.title);
        assert!(!n.title.contains('\n'), "{:?}", n.title);
    }
}

#[test]
fn macros_are_what_they_stand_for() {
    let (s, _) = tour();
    let macros = file(&s, "src/macros.rs");
    let standard = within(&s, macros, "standard");
    assert!(standard.effects.contains(&"prints".to_string()) && standard.effects.contains(&"panics".to_string()), "{:?}", standard.effects);
    // `thread_local!` and `lazy_static!` declare statics: the module's settings.
    let keys: Vec<&str> = rows(within(&s, macros, "Settings")).into_iter().map(|r| r.0).collect();
    assert_eq!(keys, ["COUNTER", "NAMES"]);
    // A panic in an arm is the arm's step; `todo!()` panics.
    let arm = descendants(&s, within(&s, macros, "unreachable_arm")).into_iter().find(|n| n.kind == Kind::Lane && n.title == "None").expect("None arm");
    assert_eq!(kids(&s, arm)[0].title, "unreachable \"checked by the caller\"");
    assert_eq!(within(&s, macros, "todo_later").effects, ["panics"]);
    // Macro titles never show their `!`.
    for n in descendants(&s, macros) {
        assert!(!n.title.contains('!') || n.title.starts_with('!'), "{:?}", n.title);
    }
}

#[test]
fn legacy_syntax() {
    let (s, _) = tour();
    let legacy = file(&s, "src/legacy.rs");
    // `use std::fs::File; File::open(…)` reads a file, through `try!` as through `?`.
    assert!(within(&s, legacy, "read_all").effects.contains(&"reads files".to_string()));
    let lanes: Vec<&str> = descendants(&s, within(&s, legacy, "classify")).into_iter().filter(|n| n.kind == Kind::Lane).map(|n| n.title.as_str()).collect();
    assert_eq!(lanes, ["b'a'...b'z'", "b'A'...b'Z'", "otherwise"]);
}

#[test]
fn modules_connect_where_names_are_defined() {
    let (s, _) = tour();
    // `client.rs` imports `encode` from its parent, which re-exports it from
    // `wire.rs`: the dependency is on `wire.rs`.
    let net = s.nodes.iter().find(|n| n.kind == Kind::Dir && n.title == "net/").expect("net/");
    let wire = within(&s, net, "wire.rs").id;
    let client = within(&s, net, "client.rs").id;
    assert!(net.edges.iter().any(|e| e.from == wire && e.to == client && e.label == "encode"), "{:?}", net.edges);
    // The client's methods: a constructor and a write to its socket.
    let class = within(&s, &s.nodes[client as usize], "Client");
    assert!(class.effects.contains(&"network".to_string()), "{:?}", class.effects);
}

#[test]
fn a_broken_function_leaves_the_rest() {
    let (s, _) = tour();
    let broken = file(&s, "src/broken.rs");
    assert!(has(broken, "syntax error"));
    let titles: Vec<&str> = kids(&s, broken).into_iter().map(|n| n.title.as_str()).collect();
    assert_eq!(titles, ["fine", "broken", "StillHere"]);
}

#[test]
fn spans_are_exact_in_any_script() {
    let (s, files) = tour();
    let text = |n: &Node| {
        let sp = n.span.as_ref().unwrap();
        let path = &s.files[sp.file as usize].path;
        let t = files.iter().find(|f| &f.path == path).unwrap().text.as_deref().unwrap();
        assert!(t.is_char_boundary(sp.start as usize) && t.is_char_boundary(sp.end as usize), "{} spans {sp:?}", n.title);
        t[sp.start as usize..sp.end as usize].to_string()
    };
    let unicode = file(&s, "src/unicode.rs");
    assert!(text(within(&s, unicode, "grüße")).starts_with("pub fn grüße(名前: &str)"));
    assert!(text(within(&s, unicode, "after_unicode")).starts_with("pub fn after_unicode()"));
    // Every span in the crate is on character boundaries.
    for n in s.nodes.iter().filter(|n| n.span.is_some()) {
        text(n);
    }
    // A shebang does not move offsets.
    assert!(text(within(&s, file(&s, "src/bin/tool.rs"), "main")).starts_with("fn main()"));
}
