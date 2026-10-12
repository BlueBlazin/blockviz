//! Go syntax, Go 1.0 through 1.24, parses and reads the way a developer would
//! describe it. The fixture is a small module (fixtures/go-syntax) with a
//! command, packages spread over several files, tests, files under build
//! constraints, a generated file and test data.

use blockviz_core::ir::{Kind, Node, Scene};
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
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/go-syntax");
    let mut files = vec![];
    load(&root, &root, &mut files);
    let scene = blockviz_core::analyze("go-tour", files);
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

fn lanes<'a>(s: &'a Scene, n: &'a Node) -> Vec<&'a str> {
    descendants(s, n).into_iter().filter(|n| n.kind == Kind::Lane).map(|n| n.title.as_str()).collect()
}

fn titles<'a>(s: &'a Scene, n: &'a Node) -> Vec<&'a str> {
    descendants(s, n).into_iter().map(|n| n.title.as_str()).collect()
}

#[test]
fn every_file_has_its_role() {
    let (s, _) = tour();
    // Only the deliberately broken file reports a syntax error; test data is
    // not the program's code and is not read.
    let errors: Vec<&String> = s.diagnostics.iter().filter(|d| d.contains("Syntax error") || d.contains("Skipped")).collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("broken/broken.go:11"), "{errors:?}");
    let role = |p: &str| file(&s, p).subtitle.clone();
    assert_eq!(role("cmd/tour/main.go"), "binary");
    assert!(has(file(&s, "cmd/tour/main.go"), "entry point"));
    assert_eq!(role("store/store_test.go"), "tests");
    assert_eq!(role("shapes/shape.go"), "module");
    assert_eq!(role("gen/zz_generated.go"), "generated");
    assert_eq!(role("testdata/bad.go"), "Go");
    // Build constraints, from `//go:build`, `// +build` or the file name.
    assert!(has(file(&s, "store/store_linux.go"), "linux"));
    assert!(has(file(&s, "netx/client.go"), "not windows"));
    assert!(has(file(&s, "legacy/old.go"), "linux or darwin"));
    // The package comment describes the file.
    assert_eq!(file(&s, "store/store.go").doc, "Package store keeps shapes in a Postgres database.");
    assert_eq!(file(&s, "cmd/tour/main.go").doc, "Command tour draws shapes and keeps them in a store.");
}

#[test]
fn packages_connect_where_names_are_defined() {
    let (s, _) = tour();
    // Files of a package use each other's names without importing them: the
    // dependency is on the file that defines the name.
    let shapes = s.nodes.iter().find(|n| n.kind == Kind::Dir && n.title == "shapes/").expect("shapes/");
    let shape = within(&s, shapes, "shape.go").id;
    let area = within(&s, shapes, "area.go").id;
    assert!(shapes.edges.iter().any(|e| e.from == shape && e.to == area && e.label.contains("Grade")), "{:?}", shapes.edges);
    // An import of a package depends on the files defining what is used of it.
    let store = file(&s, "store/store.go");
    let edges: Vec<&blockviz_core::ir::Edge> = s.nodes.iter().flat_map(|n| n.edges.iter()).filter(|e| e.from == store.id).collect();
    assert!(edges.iter().any(|e| e.label.contains("Open")), "{edges:?}");
    assert!(!s.nodes.iter().flat_map(|n| n.edges.iter()).any(|e| s.nodes[e.from as usize].title == "doc.go"), "the package comment's file is not a dependency");
    // Methods declared in another file are the type's methods there.
    let circle = within(&s, file(&s, "shapes/shape.go"), "Circle");
    assert!(has(circle, "serializable"), "{:?}", circle.badges);
    let methods = within(&s, file(&s, "shapes/area.go"), "Circle");
    assert_eq!(methods.subtitle, "methods");
    assert_eq!(methods.link, Some(circle.id));
    assert_eq!(kids(&s, methods).iter().map(|n| n.title.as_str()).collect::<Vec<_>>(), ["Area", "String"]);
    // An interface embeds another; a struct embedding a type has it as a base.
    assert_eq!(within(&s, file(&s, "shapes/shape.go"), "Shape").subtitle, "fmt.Stringer");
    assert_eq!(within(&s, file(&s, "shapes/shape.go"), "Named").subtitle, "Shape");
}

#[test]
fn errors_are_passed_on_not_drawn() {
    let (s, _) = tour();
    let store = file(&s, "store/store.go");
    let db = within(&s, store, "DB");
    // `if err != nil { return …, err }` is how Go passes an error on: no branch.
    for n in descendants(&s, store).into_iter().chain(descendants(&s, file(&s, "cmd/tour/main.go"))) {
        assert!(!n.title.contains("err != nil") && n.title != "err", "{:?} in {:?}", n.title, n.subtitle);
    }
    let save = within(&s, db, "Save");
    let closed = within(&s, save, "error ErrClosed");
    assert_eq!(closed.badges, ["if db.closed"]);
    assert_eq!(closed.effects, ["returns error"]);
    assert!(save.effects.contains(&"database".to_string()), "{:?}", save.effects);
    // An error made here, under a condition, is the function's error exit.
    let load = within(&s, db, "Load");
    let missing = within(&s, load, "error NotFoundError");
    assert!(missing.badges.iter().any(|b| b.contains("errors.Is")), "{:?}", missing.badges);
    // Returning what a call returns hands its error back: not an error exit.
    assert!(!titles(&s, within(&s, db, "Close")).iter().any(|t| t.starts_with("error")));
    let write = within(&s, file(&s, "legacy/old.go"), "Write");
    assert!(write.effects.contains(&"writes file".to_string()) && !write.effects.contains(&"returns error".to_string()), "{:?}", write.effects);
    // `db, err := store.Open(…)` then `db.Save(…)`: the value is typed, the method found.
    let main = within(&s, file(&s, "cmd/tour/main.go"), "main");
    assert!(within(&s, main, "Open").reference.is_some());
    assert_eq!(within(&s, main, "Save").reference, Some(save.id));
    // `panic(err)` in a check passes the error on too.
    assert_eq!(within(&s, within(&s, db, "MustLoad"), "Load").reference, Some(load.id));
    // Cleanup deferred, even inside a function literal, is bookkeeping.
    assert!(!titles(&s, within(&s, db, "Export")).iter().any(|t| t.contains("cerr")));
}

#[test]
fn goroutines_channels_and_defer() {
    let (s, _) = tour();
    let shape = file(&s, "shapes/shape.go");
    let areas = within(&s, shape, "Areas");
    let background: Vec<&str> = descendants(&s, areas).into_iter().filter(|n| has(n, "in background")).map(|n| n.title.as_str()).collect();
    assert!(background.contains(&"send"), "{background:?}");
    // `select` is a choice among channel operations.
    let pump = within(&s, file(&s, "netx/client.go"), "Pump");
    assert!(titles(&s, pump).contains(&"select"));
    assert_eq!(lanes(&s, pump), ["case msg, ok := <-in", "case <-time.After(time.Second)", "case <-quit"]);
    // A deferred recovery runs on return.
    let safely = within(&s, file(&s, "store/store.go"), "safely");
    assert!(descendants(&s, safely).iter().any(|n| has(n, "on return")), "{:?}", titles(&s, safely));
    // Network calls through a library object.
    let client = within(&s, file(&s, "netx/client.go"), "Client");
    assert!(within(&s, client, "Fetch").effects.contains(&"network".to_string()));
}

#[test]
fn switches_read_as_written() {
    let (s, _) = tour();
    let shape = file(&s, "shapes/shape.go");
    // A type switch: its cases name types, `default` is Go's word.
    let describe = within(&s, shape, "Describe");
    assert!(titles(&s, describe).contains(&"switch s"));
    assert_eq!(lanes(&s, describe), ["case Circle", "case Square, *Square", "case nil", "default"]);
    // A switch without a tag is an if/else chain; `fallthrough` joins cases.
    let grade = within(&s, shape, "Grade");
    assert_eq!(lanes(&s, grade), ["if area > 100", "else if area > 10", "else if math.IsNaN(area) || area < 0"]);
    // No Python, JavaScript or Rust words.
    for n in s.nodes.iter() {
        assert!(!n.title.starts_with("raise ") && !n.title.starts_with("throw ") && !n.title.starts_with("elif ") && !n.title.starts_with("match ") && n.title != "otherwise", "{:?}", n.title);
        assert!(!n.title.contains('\n'), "{:?}", n.title);
    }
}

#[test]
fn tests_and_legacy_and_modern_syntax() {
    let (s, _) = tour();
    let tests = file(&s, "store/store_test.go");
    assert!(has(within(&s, tests, "TestNotFound"), "test"));
    assert!(has(within(&s, tests, "BenchmarkSafely"), "benchmark"));
    // `if got != want { t.Errorf(…) }` checks, it does not branch.
    assert!(!titles(&s, tests).iter().any(|t| t.contains("Errorf") || t.contains("Fatalf")), "{:?}", titles(&s, tests));
    // Go 1.0 – 1.17.
    let legacy = file(&s, "legacy/old.go");
    assert_eq!(rows(within(&s, legacy, "perm")), [("perm", "0755")]);
    assert!(within(&s, legacy, "ReadAll").effects.contains(&"reads file".to_string()));
    assert_eq!(within(&s, within(&s, legacy, "Tally"), "Inc").reference, Some(within(&s, within(&s, legacy, "Counter"), "Inc").id));
    // Go 1.18 – 1.24: generics, range over functions, generic aliases.
    let modern = file(&s, "modern/new.go");
    let list = within(&s, modern, "List");
    within(&s, within(&s, list, "All"), "returned function");
    assert_eq!(rows(within(&s, modern, "Pair")), [("Key", "K"), ("Val", "V")]);
    within(&s, modern, "Countdown");
    within(&s, modern, "Map");
}

#[test]
fn a_broken_function_leaves_the_rest() {
    let (s, _) = tour();
    let broken = file(&s, "broken/broken.go");
    assert!(has(broken, "syntax error"));
    let titles: Vec<&str> = kids(&s, broken).into_iter().map(|n| n.title.as_str()).collect();
    assert_eq!(titles, ["Fine", "Broken", "StillHere", "AlsoFine"]);
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
    let unicode = file(&s, "unicode/unicode.go");
    assert!(text(within(&s, unicode, "Grüße")).starts_with("func Grüße(名前 string)"));
    assert!(text(within(&s, unicode, "AfterUnicode")).starts_with("func AfterUnicode()"));
    for n in s.nodes.iter().filter(|n| n.span.is_some()) {
        text(n);
    }
}
