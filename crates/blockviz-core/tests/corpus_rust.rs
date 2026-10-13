//! Abstraction-quality tests on Rust repositories of different shapes
//! (trimmed snapshots in fixtures/corpus-rs, see its README): an async
//! network server and client, two command-line tools, a workspace of web
//! services, and a hand-written inference engine. Each test states what a
//! developer reading that codebase should see.

use blockviz_core::ir::{EdgeKind, Kind, Node, Scene};
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

fn scene(name: &str) -> Scene {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus-rs").join(name);
    let mut files = vec![];
    load(&root, &root, &mut files);
    blockviz_core::analyze(name, files)
}

fn file<'a>(s: &'a Scene, path: &str) -> &'a Node {
    let i = s.files.iter().position(|f| f.path == path).unwrap_or_else(|| panic!("no file {path}"));
    s.nodes.iter().find(|n| matches!(n.kind, Kind::File) && n.span.as_ref().is_some_and(|sp| sp.file as usize == i)).unwrap_or_else(|| panic!("no node for {path}"))
}

fn dir<'a>(s: &'a Scene, title: &str) -> &'a Node {
    s.nodes.iter().find(|n| n.kind == Kind::Dir && n.title == title).unwrap_or_else(|| panic!("no directory {title}"))
}

fn kids<'a>(s: &'a Scene, n: &Node) -> Vec<&'a Node> {
    n.children.iter().map(|&c| &s.nodes[c as usize]).filter(|c| !c.is_port()).collect()
}

fn kid<'a>(s: &'a Scene, n: &Node, title: &str) -> &'a Node {
    kids(s, n).into_iter().find(|c| c.title == title).unwrap_or_else(|| panic!("{} has no {title:?}; has {:?}", n.title, kids(s, n).iter().map(|c| (&c.title, &c.subtitle)).collect::<Vec<_>>()))
}

/// Everything inside a node, in drawing order.
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

fn uses(s: &Scene, n: &Node, from: &str, to: &str) -> bool {
    n.edges.iter().any(|e| e.kind == EdgeKind::Uses && s.nodes[e.from as usize].title == from && s.nodes[e.to as usize].title == to)
}

fn lanes<'a>(s: &'a Scene, n: &'a Node) -> Vec<&'a str> {
    descendants(s, n).into_iter().filter(|c| c.kind == Kind::Lane).map(|c| c.title.as_str()).collect()
}

const ALL: &[&str] = &["mini-redis", "fd", "ripgrep-core", "axum-examples", "llama2-rs"];

#[test]
fn mini_redis_reads_as_a_layered_server_and_client() {
    let s = scene("mini-redis");
    // Targets: a library, two binaries, examples and integration tests.
    assert_eq!(file(&s, "src/lib.rs").subtitle, "library");
    for bin in ["src/bin/cli.rs", "src/bin/server.rs"] {
        assert_eq!(file(&s, bin).subtitle, "binary");
        assert!(file(&s, bin).badges.contains(&"entry point".to_string()));
    }
    assert_eq!(file(&s, "examples/hello_world.rs").subtitle, "example");
    assert_eq!(file(&s, "tests/client.rs").subtitle, "tests");
    // Modules depend on where names are defined, not on the crate root that
    // re-exports them: frames → connection → commands → server.
    let src = dir(&s, "src/");
    assert!(uses(&s, src, "frame.rs", "connection.rs"));
    assert!(uses(&s, src, "connection.rs", "cmd/"));
    assert!(uses(&s, src, "cmd/", "server.rs"));
    assert!(!src.edges.iter().any(|e| s.nodes[e.to as usize].title == "lib.rs" && s.nodes[e.from as usize].title.ends_with(".rs")), "nothing feeds the crate root it only re-exports");
    // The server: a listener holding the database and the socket, accepting
    // in a loop and spawning a task per connection.
    let server = file(&s, "src/server.rs");
    let listener = kid(&s, server, "Listener");
    let comps = kid(&s, listener, "components");
    assert_eq!(kid(&s, comps, "db_holder").subtitle, "DbDropGuard");
    assert_eq!(kid(&s, comps, "listener").subtitle, "TcpListener");
    let run = kid(&s, listener, "run");
    let accept = kids(&s, run).into_iter().find(|n| n.kind == Kind::Loop).expect("accept loop");
    assert!(descendants(&s, accept).iter().any(|n| n.title == "spawn" && n.subtitle == "tokio"));
    // Each connection's handler parses commands and applies them.
    let handler = within(&s, kid(&s, server, "Handler"), "run");
    let from_frame = descendants(&s, handler).into_iter().find(|n| n.title == "from_frame").expect("parse the command");
    assert_eq!(from_frame.subtitle, "Command");
    assert!(from_frame.reference.is_some());
    // Commands: an enum whose `from_frame` dispatches on the command name.
    let command = kid(&s, file(&s, "src/cmd/mod.rs"), "Command");
    let arms = lanes(&s, kid(&s, command, "from_frame"));
    assert!(arms.starts_with(&["\"get\"", "\"publish\"", "\"set\""]) && arms.ends_with(&["otherwise"]), "{arms:?}");
    // `apply` dispatches each variant to its command's own `apply`.
    let applied: Vec<&str> = descendants(&s, kid(&s, command, "apply")).into_iter().filter(|n| n.kind == Kind::Call && n.title == "apply").map(|n| n.subtitle.as_str()).collect();
    for c in ["Get", "Publish", "Set", "Subscribe", "Ping", "Unknown"] {
        assert!(applied.contains(&c), "{c} missing from {applied:?}");
    }
    // The database: the guard is drawn below the database it holds.
    assert!(uses(&s, file(&s, "src/db.rs"), "Db", "DbDropGuard"));
    // Writing frames to the socket is network I/O.
    let connection = kid(&s, file(&s, "src/connection.rs"), "Connection");
    assert!(connection.effects.contains(&"network".to_string()), "{:?}", connection.effects);
}

#[test]
fn fd_reads_as_a_command_line_tool() {
    let s = scene("fd");
    let main = file(&s, "src/main.rs");
    assert_eq!(main.subtitle, "binary");
    // Options parsed by clap.
    let opts = kid(&s, file(&s, "src/cli.rs"), "Opts");
    assert!(opts.badges.contains(&"command line".to_string()));
    // `run` parses options, builds the regexes and scans.
    let run = kid(&s, main, "run");
    let steps: Vec<&str> = kids(&s, run).iter().map(|n| n.title.as_str()).collect();
    for t in ["construct_config", "scan"] {
        assert!(steps.contains(&t), "{t} missing from {steps:?}");
    }
    let scan = kid(&s, run, "scan");
    assert!(scan.reference.is_some(), "scan zooms into the walker");
    // A constant is named by itself, and a command in a comment is no heading.
    kid(&s, main, "DEFAULT_LS_COLORS");
    // Platform variants say which platform they are for.
    let fs = file(&s, "src/filesystem.rs");
    let variants: Vec<&Vec<String>> = kids(&s, fs).into_iter().filter(|n| n.title == "is_socket").map(|n| &n.badges).collect();
    assert_eq!(variants.len(), 2);
    assert!(variants.iter().any(|b| b.iter().any(|x| x.contains("unix"))) && variants.iter().any(|b| b.iter().any(|x| x == "windows")), "{variants:?}");
    // Library and project types held by the configuration are its parts;
    // standard types (`PathBuf`, `Duration`) are just state.
    let config = kid(&s, file(&s, "src/config.rs"), "Config");
    let parts: Vec<&str> = kids(&s, kid(&s, config, "components")).into_iter().map(|n| n.subtitle.as_str()).collect();
    assert!(parts.contains(&"FileTypes") && parts.contains(&"CommandSet"), "{parts:?}");
    assert!(!parts.iter().any(|p| matches!(*p, "PathBuf" | "Duration")), "{parts:?}");
}

#[test]
fn ripgrep_reads_as_a_dispatcher_over_search_modes() {
    let s = scene("ripgrep-core");
    let main = file(&s, "crates/core/main.rs");
    assert!(main.badges.contains(&"entry point".to_string()));
    // `run` dispatches on the mode; guards tell the arms for one mode apart.
    let arms = lanes(&s, kid(&s, main, "run"));
    for a in ["Mode::Search(mode) if args.threads() == 1", "Mode::Search(mode)", "Mode::Files if args.threads() == 1", "Mode::Files"] {
        assert!(arms.contains(&a), "{a} missing from {arms:?}");
    }
    let search = descendants(&s, kid(&s, main, "run")).into_iter().find(|n| n.title == "search" && n.kind == Kind::Call).expect("call to search");
    assert_eq!(s.nodes[search.reference.unwrap() as usize].title, "search");
    // Flags are types implementing the `Flag` trait.
    let defs = file(&s, "crates/core/flags/defs.rs");
    let flags = kids(&s, defs).into_iter().filter(|n| n.kind == Kind::Class && n.subtitle == "Flag").count();
    assert!(flags > 50, "{flags} flags");
    // Searching reads files and may run decompression programs.
    let effects = &file(&s, "crates/core/search.rs").effects;
    assert!(effects.contains(&"reads files".to_string()) && effects.contains(&"spawns process".to_string()), "{effects:?}");
}

#[test]
fn axum_examples_read_as_routes_handlers_and_queries() {
    let s = scene("axum-examples");
    // Every example in the workspace is a binary.
    for ex in ["todos", "sqlx-postgres", "jwt", "chat", "key-value-store", "error-handling"] {
        let main = file(&s, &format!("{ex}/src/main.rs"));
        assert_eq!(main.subtitle, "binary", "{ex}");
    }
    // Handlers are named by the routes that reach them, nested prefixes included.
    let todos = file(&s, "todos/src/main.rs");
    assert_eq!(kid(&s, todos, "todos_index").subtitle, "GET /todos");
    assert_eq!(kid(&s, todos, "todos_update").subtitle, "PATCH /todos/{id}");
    let kv = file(&s, "key-value-store/src/main.rs");
    assert_eq!(kid(&s, kv, "kv_get").subtitle, "GET /{key}");
    // The router is a table of routes.
    let table = within(&s, kid(&s, todos, "main"), "routes");
    assert_eq!(table.kind, Kind::Table);
    assert_eq!(table.subtitle, "4 routes");
    // sqlx queries are database steps.
    let sqlx = file(&s, "sqlx-postgres/src/main.rs");
    assert!(kid(&s, sqlx, "using_connection_pool_extractor").effects.contains(&"database".to_string()));
    // An extractor implements axum's trait; its method says so.
    let claims = kid(&s, file(&s, "jwt/src/main.rs"), "Claims");
    assert!(claims.subtitle.contains("FromRequestParts"), "{}", claims.subtitle);
    assert_eq!(kid(&s, claims, "from_request_parts").subtitle, "FromRequestParts");
}

#[test]
fn llama2_reads_as_traits_and_their_implementations() {
    let s = scene("llama2-rs");
    let main = file(&s, "src/main.rs");
    // Traits describe the engine; types implement them.
    assert_eq!(kid(&s, main, "LLamaLayer").subtitle, "trait");
    let layer = kid(&s, main, "LayerWeights");
    assert_eq!(layer.subtitle, "LLamaLayer");
    for m in ["rms_and_qkv", "rope", "attention", "ffn"] {
        assert_eq!(kid(&s, layer, m).subtitle, "LLamaLayer", "{m}");
    }
    // Implementations for standard types are extensions, titled as written.
    kid(&s, main, "impl LinearWeight for Vec");
    // Reading the checkpoint is file I/O.
    assert!(kid(&s, main, "Config").effects.contains(&"reads files".to_string()));
}

#[test]
fn no_rust_syntax_noise_in_titles() {
    for name in ALL {
        let s = scene(name);
        assert!(!s.diagnostics.iter().any(|d| d.contains("Syntax error") || d.contains("Skipped")), "{name}: {:?}", s.diagnostics);
        for n in &s.nodes {
            if !matches!(n.kind, Kind::Op | Kind::Call | Kind::Function | Kind::Class) {
                continue;
            }
            // No glue, no macro bangs, no Python or JavaScript words.
            assert!(!n.title.contains(".unwrap()") && !n.title.contains(".clone()") && !n.title.contains("::<"), "{name}: {:?}", n.title);
            assert!(!(n.title.ends_with('!') && n.title.len() > 1), "{name}: {:?}", n.title);
            assert!(!n.title.starts_with("raise ") && !n.title.starts_with("elif ") && !n.title.starts_with("case "), "{name}: {:?}", n.title);
        }
        // Every lane of a branch draws a step, unless its arm is empty in the
        // code (`EncodingMode::Auto => {}`): an arm that does nothing says so.
        for l in s.nodes.iter().filter(|n| n.kind == Kind::Lane) {
            let empty = l.children.iter().all(|&c| s.nodes[c as usize].is_port());
            if empty {
                assert!(l.span.is_some(), "{name}: empty lane {:?} has no code", l.title);
            }
        }
    }
}

#[test]
fn analysis_is_deterministic() {
    for name in ["mini-redis", "axum-examples"] {
        let a = serde_json::to_string(&scene(name)).unwrap();
        let b = serde_json::to_string(&scene(name)).unwrap();
        assert_eq!(a, b, "{name}");
    }
}

#[test]
fn no_value_is_left_unread() {
    // The compiler rejects or warns of locals never read: what looks unread
    // is a read the shared tree leaves out (an error check, a deferred call,
    // a named result, a write through a reference), so nothing ends in a stop.
    for name in ["axum-examples", "fd", "llama2-rs", "mini-redis", "ripgrep-core"] {
        let s = scene(name);
        let stops: Vec<&str> = s.nodes.iter().filter(|n| n.kind == Kind::Unused).map(|n| n.title.as_str()).collect();
        assert!(stops.is_empty(), "{name}: {} stops", stops.len());
    }
}
