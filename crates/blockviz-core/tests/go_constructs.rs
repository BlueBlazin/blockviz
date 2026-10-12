//! Go constructs read the way a developer would draw them: the same rules as
//! the other languages (every exit visible, components, routes and handlers,
//! dataflow), plus what Go adds: packages of several files, methods declared
//! apart from their type, interfaces satisfied without saying so, errors
//! passed on by `if err != nil`, goroutines, channels and `select`.

use blockviz_core::ir::{Kind, Node, Scene};
use blockviz_core::project::InFile;
use std::path::Path;

fn analyze(files: &[(&str, &str)]) -> Scene {
    let s = blockviz_core::analyze("t", files.iter().map(|(p, t)| InFile { path: (*p).into(), text: Some((*t).into()), bytes: t.len() as u64 }).collect());
    assert!(!s.diagnostics.iter().any(|d| d.contains("Syntax error")), "{:?}", s.diagnostics);
    s
}

fn shop() -> Scene {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/go-shop");
    let files: Vec<(String, String)> = ["go.mod", "api/handlers.go", "cmd/server/main.go", "worker/pool.go", "cli/root.go"].iter().map(|p| (p.to_string(), std::fs::read_to_string(root.join(p)).unwrap())).collect();
    analyze(&files.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect::<Vec<_>>())
}

fn find<'a>(s: &'a Scene, title: &str) -> &'a Node {
    s.nodes.iter().find(|n| n.title == title).unwrap_or_else(|| panic!("no block titled {title:?}"))
}

fn kids<'a>(s: &'a Scene, n: &Node) -> Vec<&'a Node> {
    n.children.iter().map(|&c| &s.nodes[c as usize]).filter(|n| !n.is_port()).collect()
}

fn titles<'a>(s: &'a Scene, n: &Node) -> Vec<&'a str> {
    kids(s, n).into_iter().map(|k| k.title.as_str()).collect()
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
    descendants(s, n).into_iter().find(|c| c.title == title).unwrap_or_else(|| panic!("{} has no {title:?}; has {:?}", n.title, descendants(s, n).iter().map(|c| &c.title).collect::<Vec<_>>()))
}

fn file<'a>(s: &'a Scene, path: &str) -> &'a Node {
    let i = s.files.iter().position(|f| f.path == path).unwrap_or_else(|| panic!("no file {path}"));
    s.nodes.iter().find(|n| matches!(n.kind, Kind::File) && n.span.as_ref().is_some_and(|sp| sp.file as usize == i)).unwrap()
}

#[test]
fn a_web_service_shows_its_routes_and_handlers() {
    let s = shop();
    let api = file(&s, "api/handlers.go");
    // Routes registered on groups are tabled with their full paths.
    let new = within(&s, api, "New");
    let table = kids(&s, new).into_iter().find(|n| n.kind == Kind::Table).expect("routes table");
    assert_eq!(table.subtitle, "3 routes");
    let keys: Vec<&str> = table.rows.iter().map(|r| r.key.as_str()).collect();
    assert_eq!(keys, ["GET /api/v1/products", "GET /api/v1/products/:id", "POST /api/v1/products"]);
    // Handlers, given as method values, say what they serve.
    assert_eq!(within(&s, api, "listProducts").subtitle, "GET /api/v1/products");
    assert_eq!(within(&s, api, "createProduct").subtitle, "POST /api/v1/products");
    // Queries through an sqlx handle are database steps; `db:` tags make a row.
    assert!(within(&s, api, "listProducts").effects.contains(&"database".to_string()));
    assert!(within(&s, api, "Product").badges.contains(&"DB row".to_string()));
    // The server holds its database and router as components.
    let server = within(&s, api, "Server");
    let parts: Vec<(&str, &str)> = descendants(&s, server).into_iter().filter(|n| n.kind == Kind::Symbol).map(|n| (n.title.as_str(), n.subtitle.as_str())).collect();
    assert_eq!(parts, [("db", "DB"), ("router", "Engine")]);
    assert!(within(&s, server, "Run").effects.contains(&"network".to_string()));
    // `gin.H{…}` is a map, not a call.
    assert!(!s.nodes.iter().any(|n| n.title == "H"));
    // net/http: Go 1.22 patterns, and the handler roles.
    let main = within(&s, file(&s, "cmd/server/main.go"), "main");
    let mux = kids(&s, main).into_iter().find(|n| n.kind == Kind::Table).expect("mux routes");
    assert_eq!(mux.rows.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(), ["/healthz", "GET /items/{id}"]);
    assert_eq!(find(&s, "item").subtitle, "GET /items/{id}");
    let listen = within(&s, main, "ListenAndServe");
    assert!(listen.badges.contains(&"in background".to_string()) && listen.effects.contains(&"network".to_string()));
    // `api.New(db).Run(":8080")`: into the project's code.
    assert!(within(&s, main, "New").reference.is_some());
    assert!(within(&s, main, "Run").reference.is_some());
}

#[test]
fn a_worker_pool_reads_as_goroutines_and_channels() {
    let s = shop();
    let pool = within(&s, file(&s, "worker/pool.go"), "Pool");
    // Launching workers: the call runs in the background; WaitGroup and
    // mutex bookkeeping are not steps.
    let start = within(&s, pool, "Start");
    let work = within(&s, start, "work");
    assert!(work.badges.contains(&"in background".to_string()), "{:?}", work.badges);
    assert!(!descendants(&s, start).iter().any(|n| n.title == "Add" || n.title == "Done"));
    // A worker waits on whichever happens first.
    let w = kids(&s, pool).into_iter().find(|n| n.title == "work" && n.kind == Kind::Function).expect("work method");
    let lanes: Vec<&str> = descendants(&s, w).into_iter().filter(|n| n.kind == Kind::Lane).map(|n| n.title.as_str()).collect();
    assert_eq!(lanes, ["case <-ctx.Done()", "case job, ok := <-p.jobs"]);
    assert!(descendants(&s, w).iter().any(|n| n.title == "send" && n.subtitle == "results"));
}

#[test]
fn command_line_commands_are_named() {
    let s = shop();
    let cli = file(&s, "cli/root.go");
    assert_eq!(cli.subtitle, "commands");
    assert_eq!(within(&s, cli, "shop").subtitle, "command");
    assert_eq!(within(&s, cli, "serve").subtitle, "command");
}

#[test]
fn methods_and_interfaces_across_files() {
    let s = analyze(&[
        ("go.mod", "module example.com/zoo\n"),
        ("animal.go", "package zoo\n\n// Animal makes a sound.\ntype Animal interface {\n\tSound() string\n\tName() string\n}\n\n// Dog is a good boy.\ntype Dog struct{ name string }\n\nfunc (d Dog) Name() string { return d.name }\n"),
        ("sounds.go", "package zoo\n\nimport \"strings\"\n\nfunc (d Dog) Sound() string { return strings.ToUpper(\"woof\") }\n\n// Chorus has every animal speak.\nfunc Chorus(all []Animal) []string {\n\tvar out []string\n\tfor _, a := range all {\n\t\tout = append(out, describe(a))\n\t}\n\treturn out\n}\n"),
        ("describe.go", "package zoo\n\nfunc describe(a Animal) string { return a.Name() + \" says \" + a.Sound() }\n"),
    ]);
    // Dog has Sound in another file, and so satisfies Animal.
    let dog = file(&s, "animal.go");
    let dog = within(&s, dog, "Dog");
    assert_eq!(dog.subtitle, "Animal");
    let extension = within(&s, file(&s, "sounds.go"), "Dog");
    assert_eq!(extension.subtitle, "methods");
    assert_eq!(extension.link, Some(dog.id));
    // `describe` is defined in a sibling file: the call reaches it.
    let chorus = within(&s, file(&s, "sounds.go"), "Chorus");
    let call = descendants(&s, chorus).into_iter().find(|n| n.title == "describe").expect("describe call");
    assert_eq!(call.reference, Some(within(&s, file(&s, "describe.go"), "describe").id));
    // `out = append(out, …)` adds to the list.
    assert!(!titles(&s, chorus).contains(&"append"));
}

#[test]
fn errors_wrapped_or_not_are_passed_on() {
    let s = analyze(&[(
        "load.go",
        "package load\n\nimport (\n\t\"encoding/json\"\n\t\"errors\"\n\t\"fmt\"\n\t\"os\"\n)\n\nvar ErrEmpty = errors.New(\"empty config\")\n\ntype Config struct{ Name string }\n\n// Load reads a config file.\nfunc Load(path string) (*Config, error) {\n\tdata, err := os.ReadFile(path)\n\tif err != nil {\n\t\treturn nil, fmt.Errorf(\"read %s: %w\", path, err)\n\t}\n\tif len(data) == 0 {\n\t\treturn nil, ErrEmpty\n\t}\n\tvar c Config\n\tif err := json.Unmarshal(data, &c); err != nil {\n\t\treturn nil, err\n\t}\n\treturn &c, nil\n}\n\n// MustLoad exits when the config cannot be read.\nfunc MustLoad(path string) *Config {\n\tc, err := Load(path)\n\tif err != nil {\n\t\tfmt.Fprintln(os.Stderr, err)\n\t\tos.Exit(1)\n\t}\n\treturn c\n}\n\n// Retry tries again once: a check that does something else stays.\nfunc Retry(path string) (*Config, error) {\n\tc, err := Load(path)\n\tif err != nil {\n\t\tc, err = Load(path + \".bak\")\n\t}\n\treturn c, err\n}\n",
    )]);
    let load = find(&s, "Load");
    assert_eq!(titles(&s, load), ["ReadFile", "error ErrEmpty", "Unmarshal"]);
    assert!(load.effects.contains(&"reads files".to_string()), "{:?}", load.effects);
    let empty = within(&s, load, "error ErrEmpty");
    assert_eq!(empty.badges, ["if len(data) == 0"]);
    // Printing the error and exiting is passing it on, the command-line way.
    let must = find(&s, "MustLoad");
    assert!(!descendants(&s, must).iter().any(|n| n.title.contains("err")), "{:?}", titles(&s, must));
    // A retry is a branch of its own.
    let retry = find(&s, "Retry");
    assert!(descendants(&s, retry).iter().any(|n| n.badges.iter().any(|b| b == "if err != nil")), "{:?}", descendants(&s, retry).iter().map(|n| (&n.title, &n.badges)).collect::<Vec<_>>());
}

#[test]
fn a_package_imported_twice_is_reached_either_way() {
    // `models` and `. models` in one file (gorm's tests do this). Each run
    // hashes differently, so a few runs catch an answer that depends on it.
    for _ in 0..8 {
        let s = analyze(&[
            ("go.mod", "module example.com/m\n"),
            ("models/models.go", "package models\n\n// User is a person.\ntype User struct{ Name string }\n\n// NewUser makes one.\nfunc NewUser(n string) *User { return &User{Name: n} }\n"),
            ("app/app_test.go", "package app_test\n\nimport (\n\t\"testing\"\n\n\t\"example.com/m/models\"\n\t. \"example.com/m/models\"\n)\n\nfunc TestUsers(t *testing.T) {\n\tu := models.NewUser(\"a\")\n\tv := NewUser(\"b\")\n\tt.Log(u, v)\n}\n"),
        ]);
        let test = find(&s, "TestUsers");
        let calls: Vec<&Node> = descendants(&s, test).into_iter().filter(|n| n.title == "NewUser" || n.title == "User").collect();
        assert_eq!(calls.len(), 2, "{:?}", descendants(&s, test).iter().map(|n| &n.title).collect::<Vec<_>>());
        assert!(calls.iter().all(|c| c.reference.is_some()), "{:?}", calls.iter().map(|c| (&c.title, c.reference)).collect::<Vec<_>>());
    }
}

#[test]
fn every_go_file_is_analyzed_not_outlined() {
    let s = shop();
    for f in ["api/handlers.go", "cmd/server/main.go", "worker/pool.go", "cli/root.go"] {
        assert!(!file(&s, f).badges.contains(&"outline".to_string()), "{f}");
    }
    // Third-party modules by module path; the standard library is not listed.
    let ext = s.nodes.iter().find(|n| n.title == "External packages").expect("externals");
    let keys: Vec<&str> = ext.rows.iter().map(|r| r.key.as_str()).collect();
    assert_eq!(keys, ["github.com/gin-gonic/gin", "github.com/jmoiron/sqlx", "github.com/spf13/cobra"]);
}
