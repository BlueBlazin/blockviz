//! Real Go repositories (trimmed snapshots in fixtures/corpus-go, see its
//! README) read the way their developers would describe them: packages
//! layered by what they use, routes and handlers, implicit interfaces, the
//! commands of a CLI.

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

fn repo(name: &str) -> Scene {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus-go").join(name);
    let mut files = vec![];
    load(&root, &root, &mut files);
    let s = blockviz_core::analyze(name, files);
    assert!(!s.diagnostics.iter().any(|d| d.contains("Syntax error") || d.contains("Skipped")), "{:?}", s.diagnostics);
    s
}

fn file<'a>(s: &'a Scene, path: &str) -> &'a Node {
    let i = s.files.iter().position(|f| f.path == path).unwrap_or_else(|| panic!("no file {path}"));
    s.nodes.iter().find(|n| matches!(n.kind, Kind::File) && n.span.as_ref().is_some_and(|sp| sp.file as usize == i)).unwrap_or_else(|| panic!("no node for {path}"))
}

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
    descendants(s, n).into_iter().find(|c| c.title == title).unwrap_or_else(|| panic!("{} has no {title:?}", n.title))
}

/// A definition directly in a file (not a call of it).
fn def<'a>(s: &'a Scene, n: &'a Node, title: &str) -> &'a Node {
    descendants(s, n).into_iter().find(|c| c.title == title && matches!(c.kind, Kind::Function | Kind::Class)).unwrap_or_else(|| panic!("{} defines no {title:?}", n.title))
}

/// Edges at the repository's top level, as (from, to, label).
fn top_edges(s: &Scene) -> Vec<(String, String, String)> {
    let root = &s.nodes[0];
    root.edges.iter().map(|e| (s.nodes[e.from as usize].title.clone(), s.nodes[e.to as usize].title.clone(), e.label.clone())).collect()
}

#[test]
fn groupcache_layers_its_packages() {
    let s = repo("groupcache");
    // The cache is built on its lru, singleflight and consistenthash packages.
    let edges = top_edges(&s);
    let has = |from: &str, to: &str, label: &str| edges.iter().any(|(f, t, l)| f == from && t == to && l.contains(label));
    assert!(has("lru/", "groupcache.go", "Cache"), "{edges:?}");
    assert!(has("singleflight/", "groupcache.go", "Group"), "{edges:?}");
    assert!(has("consistenthash/", "http.go", "Map"), "{edges:?}");
    assert!(has("peers.go", "groupcache.go", "PeerPicker"), "{edges:?}");
    // Types say which interfaces they satisfy, across packages, by signature.
    let gc = file(&s, "groupcache.go");
    assert_eq!(def(&s, gc, "Group").subtitle, "Getter");
    assert_eq!(def(&s, gc, "GetterFunc").subtitle, "Getter");
    assert_eq!(def(&s, file(&s, "http.go"), "httpGetter").subtitle, "ProtoGetter");
    assert_eq!(def(&s, file(&s, "singleflight/singleflight.go"), "Group").subtitle, "flightGroup");
    assert_eq!(def(&s, file(&s, "consistenthash/consistenthash.go"), "Map").subtitle, "");
    // An interface is its method signatures.
    let picker = def(&s, file(&s, "peers.go"), "PeerPicker");
    assert_eq!(picker.rows[0].key, "PickPeer");
    // Generated protobuf code is shown, not opened.
    assert_eq!(file(&s, "groupcachepb/groupcache.pb.go").subtitle, "generated");
    assert!(file(&s, "http.go").effects.contains(&"network".to_string()));
}

#[test]
fn realworld_serves_its_api() {
    let s = repo("realworld");
    // Routes registered on groups passed from `main` carry their full paths.
    let role = |path: &str, f: &str| def(&s, file(&s, path), f).subtitle.clone();
    assert_eq!(role("articles/routers.go", "ArticleCreate"), "POST /api/articles");
    assert_eq!(role("articles/routers.go", "ArticleList"), "GET /api/articles");
    assert_eq!(role("articles/routers.go", "ArticleCommentDelete"), "DELETE /api/articles/:slug/comments/:id");
    assert_eq!(role("users/routers.go", "UsersLogin"), "POST /api/users/login");
    assert_eq!(role("users/routers.go", "ProfileFollow"), "POST /api/profiles/:username/follow");
    assert_eq!(file(&s, "articles/routers.go").subtitle, "routes");
    let register = def(&s, file(&s, "articles/routers.go"), "ArticlesRegister");
    let table = descendants(&s, register).into_iter().find(|n| n.kind == Kind::Table).expect("route table");
    assert!(table.rows.iter().any(|r| r.key == "GET /api/articles/feed" && r.value == "ArticleFeed"), "{:?}", table.rows);
    // gorm models are table rows; queries are database steps.
    let models = file(&s, "articles/models.go");
    assert!(def(&s, models, "ArticleModel").badges.contains(&"DB row".to_string()));
    // Packages layer: common under users under articles under the program.
    let edges = top_edges(&s);
    let has = |from: &str, to: &str| edges.iter().any(|(f, t, _)| f == from && t == to);
    assert!(has("common/", "users/") && has("users/", "articles/") && has("articles/", "hello.go"), "{edges:?}");
    assert_eq!(file(&s, "hello.go").subtitle, "binary");
}

#[test]
fn redcon_is_a_server_and_its_connections() {
    let s = repo("redcon");
    let lib = file(&s, "redcon.go");
    // The connection interface is a contract; the types that satisfy it say so.
    let conn = def(&s, lib, "Conn");
    assert!(conn.rows.iter().any(|r| r.key == "WriteError" && r.value == "(msg string)"), "{:?}", conn.rows);
    assert_eq!(def(&s, lib, "conn").subtitle, "Conn");
    assert_eq!(def(&s, lib, "TLSServer").subtitle, "Server");
    // The examples are programs serving through the library.
    let clone = file(&s, "example/clone.go");
    assert_eq!(clone.subtitle, "binary");
    let serve = within(&s, def(&s, clone, "main"), "ListenAndServe");
    assert_eq!(serve.reference, Some(def(&s, lib, "ListenAndServe").id));
}

#[test]
fn chi_routes_nest_and_mount() {
    let s = repo("chi");
    let rest = file(&s, "_examples/rest/main.go");
    // `r.Route("/articles", func(r chi.Router) { r.With(paginate).Get("/", …) })`.
    assert_eq!(def(&s, rest, "ListArticles").subtitle, "GET /articles");
    assert_eq!(def(&s, rest, "DeleteArticle").subtitle, "DELETE /articles/{articleID}");
    assert!(def(&s, rest, "GetArticle").subtitle.contains("GET /articles/{articleID}"));
    // `r.Mount("/admin", adminRouter())`: the admin router's routes are under /admin.
    let admin = def(&s, rest, "adminRouter");
    assert!(descendants(&s, admin).iter().any(|n| n.title == "GET /admin/accounts"), "{:?}", descendants(&s, admin).iter().map(|n| &n.title).collect::<Vec<_>>());
    // The router satisfies its own interfaces.
    assert_eq!(def(&s, file(&s, "mux.go"), "Mux").subtitle, "Router, Routes");
}

#[test]
fn glow_is_a_command_line_tool() {
    let s = repo("glow");
    assert_eq!(file(&s, "main.go").subtitle, "binary");
    assert_eq!(file(&s, "config_cmd.go").subtitle, "commands");
    let windows = file(&s, "console_windows.go");
    assert!(windows.badges.contains(&"windows".to_string()), "{:?}", windows.badges);
    assert!(windows.doc.is_empty(), "a build directive is not a description: {:?}", windows.doc);
    assert!(file(&s, "github.go").effects.contains(&"network".to_string()));
}

#[test]
fn golang_examples() {
    let s = repo("example");
    assert_eq!(file(&s, "hello/hello.go").subtitle, "binary");
    assert!(def(&s, file(&s, "hello/reverse/reverse_test.go"), "TestString").badges.contains(&"test".to_string()));
    assert!(def(&s, file(&s, "hello/reverse/example_test.go"), "ExampleString").badges.contains(&"example".to_string()));
    let outyet = file(&s, "outyet/main.go");
    assert!(def(&s, outyet, "main").effects.contains(&"network".to_string()));
    assert!(outyet.doc.starts_with("Outyet is a web server"), "{:?}", outyet.doc);
}

#[test]
fn every_corpus_file_is_analyzed() {
    for name in ["groupcache", "realworld", "redcon", "chi", "glow", "example"] {
        let s = repo(name);
        for f in s.files.iter().filter(|f| f.path.ends_with(".go")) {
            let n = file(&s, &f.path);
            assert!(!n.badges.contains(&"outline".to_string()) && !n.badges.contains(&"too complex".to_string()), "{name}/{}", f.path);
        }
    }
}
