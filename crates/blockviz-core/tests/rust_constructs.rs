//! Rust constructs read the way a developer would draw them: the same rules as
//! Python and JavaScript (every exit visible, components and repeated blocks,
//! routes and handlers), plus what Rust adds: `impl` blocks and traits,
//! `Result` and `?`, `match` arms that bind, iterator chains, macros, modules
//! resolved through `crate::`, `super::` and re-exports.

use blockviz_core::ir::{EdgeKind, Kind, Node, Scene};
use blockviz_core::project::InFile;
use std::path::Path;

fn analyze(files: &[(&str, &str)]) -> Scene {
    let s = blockviz_core::analyze("t", files.iter().map(|(p, t)| InFile { path: (*p).into(), text: Some((*t).into()), bytes: t.len() as u64 }).collect());
    assert!(!s.diagnostics.iter().any(|d| d.contains("Syntax error")), "{:?}", s.diagnostics);
    s
}

fn find<'a>(s: &'a Scene, title: &str) -> &'a Node {
    s.nodes.iter().find(|n| n.title == title).unwrap_or_else(|| panic!("no block titled {title:?}"))
}

fn kids<'a>(s: &'a Scene, n: &Node) -> Vec<&'a Node> {
    n.children.iter().map(|&c| &s.nodes[c as usize]).filter(|n| !n.is_port()).collect()
}

fn kid<'a>(s: &'a Scene, n: &Node, title: &str) -> &'a Node {
    kids(s, n).into_iter().find(|k| k.title == title).unwrap_or_else(|| panic!("no {title:?} in {}: {:?}", n.title, kids(s, n).iter().map(|k| (&k.title, &k.subtitle)).collect::<Vec<_>>()))
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
    descendants(s, n).into_iter().find(|c| c.title == title).unwrap_or_else(|| panic!("{} has no {title:?}", n.title))
}

fn has_edge(s: &Scene, n: &Node, from: &str, to: &str) -> bool {
    n.edges.iter().any(|e| s.nodes[e.from as usize].title == from && s.nodes[e.to as usize].title == to)
}

fn gpt() -> Scene {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rust-gpt");
    let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap();
    let files = [("Cargo.toml", read("Cargo.toml")), ("src/lib.rs", read("src/lib.rs")), ("src/model.rs", read("src/model.rs"))];
    analyze(&files.iter().map(|(p, t)| (*p, t.as_str())).collect::<Vec<_>>())
}

#[test]
fn a_transformer_reads_like_nanogpt() {
    let s = gpt();
    let model = s.nodes.iter().find(|n| n.kind == Kind::File && n.title == "model.rs").unwrap();
    assert_eq!(titles(&s, model), ["Config", "CausalSelfAttention", "Mlp", "Block", "Gpt"]);
    // Parts are drawn before what they make up, whatever order they are written in.
    assert!(has_edge(&s, model, "Block", "Gpt"));
    assert!(has_edge(&s, model, "CausalSelfAttention", "Block"));
    // `self.ln_1.forward(x)` calls the component, as `self.ln_1(x)` does in PyTorch.
    let block = kid(&s, model, "Block");
    let fwd = kid(&s, block, "forward");
    assert_eq!(titles(&s, fwd), ["ln_1", "attn", "+", "ln_2", "mlp", "+"]);
    assert!(has_edge(&s, fwd, "x", "+"), "residual connection from x");
    let attn = kid(&s, fwd, "attn");
    assert_eq!(attn.subtitle, "CausalSelfAttention");
    assert_eq!(s.nodes[attn.reference.unwrap() as usize].title, "forward", "attn zooms into CausalSelfAttention.forward");
    assert_eq!(attn.category, "attention");
    assert_eq!(kid(&s, fwd, "ln_1").category, "norm");
    // A tensor's activation method is a layer of its own: c_fc → gelu → c_proj.
    let mlp = kid(&s, kid(&s, model, "Mlp"), "forward");
    assert_eq!(titles(&s, mlp), ["c_fc", "gelu", "c_proj"]);
    assert_eq!(kid(&s, mlp, "gelu").category, "activation");
    // The model's components: blocks built by `(0..n_layer).map(…)` repeat n_layer times.
    let gpt = kid(&s, model, "Gpt");
    let comps = kid(&s, gpt, "components");
    assert_eq!(kid(&s, comps, "blocks").subtitle, "Block × n_layer");
    assert_eq!(kid(&s, comps, "wte").category, "embed");
    assert_eq!(kid(&s, comps, "lm_head").category, "linear");
    let forward = kid(&s, gpt, "forward");
    let steps = titles(&s, forward);
    for t in ["wte", "wpe", "ln_f", "lm_head"] {
        assert!(steps.contains(&t), "{t} missing from {steps:?}");
    }
    let lp = kids(&s, forward).into_iter().find(|n| n.kind == Kind::Loop).expect("loop over blocks");
    assert!(lp.badges.iter().any(|b| b == "× n_layer"), "{:?}", lp.badges);
    assert_eq!(kids(&s, lp)[0].title, "block");
    assert_eq!(kids(&s, lp)[0].subtitle, "Block");
}

#[test]
fn a_server_shows_its_state_and_handlers() {
    let s = analyze(&[
        ("Cargo.toml", "[package]\nname = \"kv\"\n[dependencies]\ntokio = \"1\"\n"),
        ("src/lib.rs", "pub mod db;\npub mod server;\npub use db::Db;\n"),
        (
            "src/db.rs",
            r#"
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Holds the shared state.
pub struct DbGuard {
    db: Db,
}

#[derive(Clone)]
pub struct Db {
    shared: Arc<Mutex<State>>,
}

struct State {
    entries: HashMap<String, String>,
}

impl DbGuard {
    pub fn new() -> DbGuard {
        DbGuard { db: Db::new() }
    }
}

impl Db {
    pub fn new() -> Db {
        let state = State { entries: HashMap::new() };
        Db { shared: Arc::new(Mutex::new(state)) }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let state = self.shared.lock().unwrap();
        state.entries.get(key).cloned()
    }

    pub fn set(&self, key: String, value: String) {
        let mut state = self.shared.lock().unwrap();
        state.entries.insert(key, value);
    }
}
"#,
        ),
        (
            "src/server.rs",
            r#"
use crate::Db;
use tokio::net::{TcpListener, TcpStream};

pub struct Listener {
    db: Db,
    listener: TcpListener,
}

impl Listener {
    pub async fn run(&mut self) -> crate::db::Result<()> {
        loop {
            let (socket, _) = self.listener.accept().await?;
            let db = self.db.clone();
            tokio::spawn(async move {
                if let Err(err) = handle(socket, db).await {
                    tracing::error!(cause = %err, "connection error");
                }
            });
        }
    }
}

async fn handle(socket: TcpStream, db: Db) -> std::io::Result<()> {
    let value = db.get("key");
    match value {
        Some(v) => println!("{v}"),
        None => db.set("key".into(), "value".into()),
    }
    Ok(())
}
"#,
        ),
    ]);
    // The guard is drawn below the state it guards; the state below the database.
    let db_rs = s.nodes.iter().find(|n| n.kind == Kind::File && n.title == "db.rs").unwrap();
    assert!(has_edge(&s, db_rs, "Db", "DbGuard"));
    assert!(has_edge(&s, db_rs, "State", "Db"));
    // A component of a project type links to it.
    let listener = find(&s, "Listener");
    let comps = kid(&s, listener, "components");
    assert_eq!(kid(&s, comps, "db").subtitle, "Db");
    assert_eq!(kid(&s, comps, "listener").subtitle, "TcpListener");
    // The accept loop: a network step, and a task spawned per connection that
    // holds its async block.
    let run = kid(&s, listener, "run");
    let lp = kids(&s, run).into_iter().find(|n| n.kind == Kind::Loop).expect("accept loop");
    assert_eq!(lp.title, "repeat");
    let spawn = kid(&s, lp, "spawn");
    assert!(descendants(&s, spawn).iter().any(|n| n.title == "handle"), "the task runs handle");
    // `if let Err(err) = …` binds err; logging is an effect, not a step.
    assert!(find(&s, "handle").effects.contains(&"prints".to_string()));
    // `db.get(…)` on a `Db` parameter is the project method.
    let get = descendants(&s, find(&s, "handle")).into_iter().find(|n| n.title == "get").expect("get call");
    assert_eq!(get.subtitle, "Db");
    assert!(get.reference.is_some());
    // The repo view: `use crate::Db` depends on db.rs, where Db is defined, not on lib.rs.
    let src = s.nodes.iter().find(|n| n.kind == Kind::Dir && n.title == "src/").unwrap();
    assert!(has_edge(&s, src, "db.rs", "server.rs"), "{:?}", src.edges.iter().map(|e| (&s.nodes[e.from as usize].title, &s.nodes[e.to as usize].title)).collect::<Vec<_>>());
    assert!(!has_edge(&s, src, "lib.rs", "server.rs"));
}

#[test]
fn routes_come_from_router_builders() {
    let s = analyze(&[
        ("Cargo.toml", "[package]\nname = \"api\"\n[dependencies]\naxum = \"0.8\"\nactix-web = \"4\"\n"),
        ("src/main.rs", "mod http;\nmod legacy;\n#[tokio::main]\nasync fn main() {\n    let app = http::router();\n    axum::serve(listener, app).await.unwrap();\n}\n"),
        (
            "src/http/mod.rs",
            r#"
use axum::{routing::{get, post}, Router};
mod users;

pub fn router() -> Router {
    Router::new()
        .route("/health", get(health))
        .nest("/api", api())
}

fn api() -> Router {
    Router::new().merge(users::router())
}

async fn health() -> &'static str {
    "ok"
}
"#,
        ),
        (
            "src/http/users.rs",
            r#"
use axum::{extract::Path, routing::get, Json, Router};

pub fn router() -> Router {
    Router::new()
        .route("/users", get(list_users).post(create_user))
        .route("/users/{id}", get(get_user).delete(delete_user))
}

async fn list_users() -> Json<Vec<String>> { Json(vec![]) }
async fn create_user() {}
async fn get_user(Path(id): Path<u64>) -> String { id.to_string() }
async fn delete_user(Path(id): Path<u64>) {}
"#,
        ),
        (
            "src/legacy.rs",
            r#"
use actix_web::{get, web, App, HttpResponse};

#[get("/hello/{name}")]
async fn hello(name: web::Path<String>) -> HttpResponse {
    HttpResponse::Ok().body(format!("Hello {name}"))
}

async fn index() -> HttpResponse { HttpResponse::Ok().finish() }
async fn submit() -> HttpResponse { HttpResponse::Ok().finish() }

pub fn app() -> App {
    App::new()
        .service(hello)
        .route("/", web::get().to(index))
        .service(web::resource("/submit").route(web::post().to(submit)))
}
"#,
        ),
    ]);
    // Handlers are named by the route they serve, nesting prefixes included.
    assert_eq!(find(&s, "health").subtitle, "GET /health");
    assert_eq!(find(&s, "list_users").subtitle, "GET /api/users");
    assert_eq!(find(&s, "create_user").subtitle, "POST /api/users");
    assert_eq!(find(&s, "delete_user").subtitle, "DELETE /api/users/{id}");
    assert_eq!(find(&s, "hello").subtitle, "GET /hello/{name}");
    assert_eq!(find(&s, "index").subtitle, "GET /");
    assert_eq!(find(&s, "submit").subtitle, "POST /submit");
    // A router is a table of its routes.
    let users = s.nodes.iter().find(|n| n.kind == Kind::File && n.title == "users.rs").unwrap();
    assert_eq!(users.subtitle, "routes");
    let table = within(&s, users, "routes");
    assert_eq!(table.kind, Kind::Table);
    let rows: Vec<(&str, &str)> = table.rows.iter().map(|r| (r.key.as_str(), r.value.as_str())).collect();
    assert_eq!(rows, [("GET /api/users", "list_users"), ("POST /api/users", "create_user"), ("GET /api/users/{id}", "get_user"), ("DELETE /api/users/{id}", "delete_user")]);
}

#[test]
fn results_branch_and_errors_are_exits() {
    let s = analyze(&[(
        "src/lib.rs",
        r#"
use std::fs;
use std::path::Path;

pub enum Error {
    Missing,
    Io(std::io::Error),
}

pub fn load(path: &Path) -> Result<String, Error> {
    let Ok(text) = fs::read_to_string(path) else {
        return Err(Error::Missing);
    };
    let trimmed = match text.trim() {
        "" => return Err(Error::Missing),
        t if t.starts_with('#') => t[1..].to_string(),
        t => t.to_string(),
    };
    if trimmed.len() > 100 {
        Ok(trimmed[..100].to_string())
    } else {
        Ok(trimmed)
    }
}

pub fn save(path: &Path, text: &str) -> std::io::Result<()> {
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(path, text)?;
    Ok(())
}
"#,
    )]);
    let load = find(&s, "load");
    // Reading the file is an I/O step; `let … else` guards an error exit.
    let read = kid(&s, load, "read_to_string");
    assert_eq!(read.category, "io");
    assert!(read.effects.contains(&"reads file".to_string()), "{:?}", read.effects);
    let missing = kids(&s, load).into_iter().find(|n| n.title == "error Error::Missing").expect("let-else exit");
    assert_eq!(missing.badges, ["unless Ok(text)"]);
    assert_eq!(missing.category, "error");
    // Match arms are their patterns; a guard tells two arms apart.
    let arms: Vec<&str> = descendants(&s, load).into_iter().filter(|n| n.kind == Kind::Lane).map(|n| n.title.as_str()).collect();
    assert!(arms.starts_with(&["\"\"", "t if t.starts_with('#')", "t"]), "{arms:?}");
    // The final `if` hands back a value from both arms; `Ok(x)` is x.
    let out = load.children.iter().map(|&c| &s.nodes[c as usize]).find(|n| n.kind == Kind::Output).expect("return port");
    assert_eq!(out.title, "trimmed");
    assert_eq!(load.effects, ["reads files", "returns error"]);
    // `?` passes values on; `Ok(())` at the end is no step.
    let save = find(&s, "save");
    assert_eq!(save.effects, ["filesystem", "writes files"]);
    assert!(!kids(&s, save).iter().any(|n| n.title.contains("Ok") || n.title == "return"));
}

#[test]
fn traits_and_impls_belong_to_their_types() {
    let s = analyze(&[
        ("src/lib.rs", "mod frame;\nmod ops;\n"),
        (
            "src/frame.rs",
            r#"
use std::fmt;

/// A protocol frame.
pub struct Frame {
    pub bytes: Vec<u8>,
}

impl Frame {
    pub fn new(bytes: Vec<u8>) -> Frame {
        Frame { bytes }
    }
}

impl fmt::Display for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} bytes", self.bytes.len())
    }
}

impl From<String> for Frame {
    fn from(s: String) -> Frame { Frame::new(s.into_bytes()) }
}

impl From<&str> for Frame {
    fn from(s: &str) -> Frame { Frame::new(s.as_bytes().to_vec()) }
}

pub trait Encode {
    fn encode(&self) -> Vec<u8>;
    fn size(&self) -> usize {
        self.encode().len()
    }
}
"#,
        ),
        (
            "src/ops.rs",
            r#"
use crate::frame::Frame;

impl Frame {
    pub fn checksum(&self) -> u8 {
        self.bytes.iter().fold(0, |a, b| a ^ b)
    }
}

pub fn verify(frame: &Frame) -> bool {
    frame.checksum() == 0
}
"#,
        ),
    ]);
    let frame = find(&s, "Frame");
    assert_eq!(frame.kind, Kind::Class);
    assert_eq!(frame.doc, "A protocol frame.");
    // The traits a type implements are its subtitle; a method says its trait.
    assert_eq!(frame.subtitle, "Display, From");
    assert_eq!(kid(&s, frame, "fmt").subtitle, "Display");
    let froms: Vec<&str> = kids(&s, frame).into_iter().filter(|n| n.title == "from").map(|n| n.subtitle.as_str()).collect();
    assert_eq!(froms, ["From<String>", "From<&str>"]);
    // `Frame::new(…)` makes a Frame.
    let from = kids(&s, frame).into_iter().find(|n| n.title == "from").unwrap();
    assert_eq!(kids(&s, from)[0].title, "Frame");
    // A trait: provided methods are methods.
    let encode = find(&s, "Encode");
    assert_eq!(encode.subtitle, "trait");
    kid(&s, encode, "size");
    // An `impl` in another module extends the type: its methods are found
    // from calls anywhere, and it leads to the type.
    let ext = find(&s, "impl Frame");
    assert_eq!(ext.link, Some(frame.id));
    let call = descendants(&s, find(&s, "verify")).into_iter().find(|n| n.title == "checksum").expect("checksum call");
    assert_eq!(s.nodes[call.reference.unwrap() as usize].title, "checksum");
}

#[test]
fn commands_data_and_queries() {
    let s = analyze(&[
        ("Cargo.toml", "[package]\nname = \"app\"\n[dependencies]\nclap = \"4\"\nserde = \"1\"\nsqlx = \"0.8\"\n"),
        (
            "src/main.rs",
            r#"
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

/// Manages users.
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    #[arg(long, default_value_t = 8080)]
    port: u16,
}

#[derive(Subcommand)]
enum Command {
    Add { name: String },
    List,
}

#[derive(Serialize, Deserialize, sqlx::FromRow)]
struct User {
    id: i64,
    name: String,
}

async fn list(pool: &sqlx::PgPool) -> sqlx::Result<Vec<User>> {
    let users = sqlx::query_as!(User, "SELECT id, name FROM users").fetch_all(pool).await?;
    Ok(users)
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Add { name } => println!("adding {name}"),
        Command::List => {}
    }
}
"#,
        ),
    ]);
    let cli = find(&s, "Cli");
    assert_eq!(cli.badges, ["command line"]);
    let rows: Vec<&str> = cli.rows.iter().map(|r| r.key.as_str()).collect();
    assert_eq!(rows, ["command", "port"]);
    assert_eq!(find(&s, "Command").badges, ["subcommands"]);
    // A row type is the database's.
    assert_eq!(find(&s, "User").badges, ["DB row"]);
    // `sqlx::query_as!(…).fetch_all(pool)` is one database step: the whole
    // function, which does nothing else.
    let list = find(&s, "list");
    assert!(list.effects.contains(&"database".to_string()), "{:?}", list.effects);
    assert!(kids(&s, list).is_empty());
    // The binary runs.
    let main = s.nodes.iter().find(|n| n.kind == Kind::File && n.title.ends_with("main.rs")).unwrap();
    assert_eq!(main.subtitle, "binary");
    assert!(main.badges.iter().any(|b| b == "entry point"));
}

#[test]
fn std_effects_and_glue() {
    let s = analyze(&[(
        "src/main.rs",
        r#"
use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let config = std::fs::read_to_string(&args[1]).expect("config");
    let status = Command::new("git").arg("status").status().unwrap();
    let names: Vec<String> = config.lines().map(|l| l.trim().to_string()).collect();
    let total = names.iter().filter(|n| !n.is_empty()).count();
    eprintln!("{} names, git {:?}", total, status);
    log::info!("done");
    std::process::exit(0);
}
"#,
    )]);
    let main = find(&s, "main");
    for e in ["exits process", "logs", "prints", "reads environment", "reads files", "spawns process"] {
        assert!(main.effects.contains(&e.to_string()), "{e} missing from {:?}", main.effects);
    }
    // Borrowing, cloning and unwrapping are glue: no `.unwrap()`, `.clone()`
    // or `&` in any title.
    for n in descendants(&s, main) {
        assert!(!n.title.contains("unwrap") && !n.title.contains(".to_string") && !n.title.contains(".clone") && !n.title.contains('&'), "{:?}", n.title);
    }
    // `config.lines().map(…).collect()` is a comprehension over lines.
    assert_eq!(kid(&s, main, "names").subtitle, "each of lines");
}

#[test]
fn every_rust_file_is_analyzed_not_outlined() {
    let s = analyze(&[("src/lib.rs", "pub fn a() -> u32 { 1 }\n"), ("src/util.rs", "pub fn b() {}\n")]);
    assert!(!s.nodes.iter().any(|n| n.badges.iter().any(|b| b == "outline")));
    let lib = s.nodes.iter().find(|n| n.kind == Kind::File && n.title == "lib.rs").unwrap();
    assert_eq!(lib.subtitle, "library");
    let _ = EdgeKind::Uses;
}

#[test]
fn workspace_crates_are_the_project() {
    let s = analyze(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n[package]\nname = \"app\"\n[dependencies]\nengine = { path = \"crates/engine\" }\nserde = \"1\"\n"),
        ("src/main.rs", "use engine::search::Searcher;\n\nfn main() {\n    let s = Searcher::new();\n    let hits = engine::count(&s);\n    println!(\"{hits}\");\n}\n"),
        ("crates/engine/Cargo.toml", "[package]\nname = \"engine\"\n"),
        ("crates/engine/src/lib.rs", "pub mod search;\npub extern crate serde as ser;\n\npub fn count(s: &search::Searcher) -> usize {\n    s.len()\n}\n"),
        ("crates/engine/src/search.rs", "pub struct Searcher {\n    items: Vec<String>,\n}\n\nimpl Searcher {\n    pub fn new() -> Searcher {\n        Searcher { items: vec![] }\n    }\n    pub fn len(&self) -> usize {\n        self.items.len()\n    }\n}\n"),
    ]);
    // A crate of the workspace is the project's, though the manifest lists
    // it as a dependency: its types and functions are reached from the binary.
    let main = find(&s, "main");
    let new = kids(&s, main).into_iter().find(|n| n.title == "Searcher").expect("constructor call");
    assert!(new.reference.is_some());
    let count = kid(&s, main, "count");
    assert!(count.reference.is_some(), "engine::count is the project's function");
    let externals = find(&s, "External packages");
    let names: Vec<&str> = externals.rows.iter().map(|r| r.key.as_str()).collect();
    assert!(!names.contains(&"engine"), "{names:?}");
    assert!(names.contains(&"serde"), "{names:?}");
}

#[test]
fn calls_through_trait_objects_reach_their_impls() {
    let s = analyze(&[
        ("Cargo.toml", "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        (
            "src/main.rs",
            "mod repo;\n\nuse repo::{Email, InMemoryRepo, Notifier, Repo, Sms};\nuse std::sync::Arc;\n\nstruct App {\n    repo: Arc<dyn Repo>,\n    notifier: Box<dyn Notifier>,\n}\n\nimpl App {\n    fn register(&self, name: &str) -> u64 {\n        let id = self.repo.save(name);\n        self.notifier.notify(name);\n        id\n    }\n}\n\nfn main() {\n    let notifier: Box<dyn Notifier> = if std::env::args().count() > 1 { Box::new(Email) } else { Box::new(Sms) };\n    let app = App { repo: Arc::new(InMemoryRepo::default()), notifier };\n    app.register(\"ann\");\n}\n",
        ),
        (
            "src/repo.rs",
            "use std::sync::Mutex;\n\npub trait Repo: Send + Sync {\n    fn save(&self, name: &str) -> u64;\n    fn describe(&self, name: &str) -> String {\n        format!(\"{} #{}\", name, self.save(name))\n    }\n}\n\n#[derive(Default)]\npub struct InMemoryRepo {\n    users: Mutex<Vec<String>>,\n}\n\nimpl Repo for InMemoryRepo {\n    fn save(&self, name: &str) -> u64 {\n        let mut users = self.users.lock().unwrap();\n        users.push(name.to_string());\n        users.len() as u64\n    }\n}\n\npub trait Notifier {\n    fn notify(&self, who: &str);\n}\n\npub struct Email;\npub struct Sms;\n\nimpl Notifier for Email {\n    fn notify(&self, who: &str) {\n        println!(\"mail {who}\");\n    }\n}\n\nimpl Notifier for Sms {\n    fn notify(&self, who: &str) {\n        println!(\"sms {who}\");\n    }\n}\n",
        ),
    ]);
    let register = find(&s, "register");
    fn call<'a>(s: &'a Scene, f: &'a Node, title: &str) -> &'a Node {
        descendants(s, f).into_iter().find(|n| n.kind == Kind::Call && n.title == title).unwrap_or_else(|| panic!("no {title} call in {}", f.title))
    }
    let save = within(&s, find(&s, "InMemoryRepo"), "save");
    // `Arc<dyn Repo>`: the one repository there is.
    assert_eq!((call(&s, register, "save").subtitle.as_str(), call(&s, register, "save").reference), ("InMemoryRepo", Some(save.id)));
    // A provided method calling a required one reaches the same impl.
    assert_eq!(call(&s, find(&s, "describe"), "save").reference, Some(save.id));
    // `Box<dyn Notifier>`: either impl, leading to the trait.
    let notify = call(&s, register, "notify");
    assert_eq!((notify.subtitle.as_str(), notify.reference), ("Email or Sms", Some(find(&s, "Notifier").id)));
}

#[test]
fn generic_parameters_are_the_trait_they_are_bounded_by() {
    let s = analyze(&[
        ("Cargo.toml", "[package]\nname = \"grep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        (
            "src/main.rs",
            "use serde::de::DeserializeOwned;\n\npub trait Matcher {\n    fn find(&self, line: &str) -> Option<usize>;\n}\n\npub struct Regex(String);\n\nimpl Matcher for Regex {\n    fn find(&self, line: &str) -> Option<usize> {\n        line.find(self.0.as_str())\n    }\n}\n\n/// Searches with any matcher.\npub struct Searcher<M> {\n    matcher: M,\n}\n\nimpl<T: Matcher> Searcher<T> {\n    fn search(&self, line: &str) -> bool {\n        self.matcher.find(line).is_some()\n    }\n}\n\nfn count<M>(m: &M, lines: &[&str]) -> usize\nwhere\n    M: Matcher + Clone,\n{\n    lines.iter().filter(|l| m.find(l).is_some()).count()\n}\n\n/// A form of any data.\npub struct Form<T: DeserializeOwned>(pub T);\n\nfn main() {\n    let r = Regex(\"a\".into());\n    let s = Searcher { matcher: Regex(\"b\".into()) };\n    println!(\"{} {}\", count(&r, &[\"abc\"]), s.search(\"b\"));\n}\n",
        ),
    ]);
    let regex_find = within(&s, find(&s, "Regex"), "find");
    // `where M: Matcher` and `impl<T: Matcher> Searcher<T>`: both reach the one Matcher.
    for f in ["count", "search"] {
        let call = descendants(&s, find(&s, f)).into_iter().find(|n| n.kind == Kind::Call && n.title == "find").unwrap_or_else(|| panic!("no find call in {f}"));
        assert_eq!(call.reference, Some(regex_find.id), "{f}");
    }
    // The searcher holds a Matcher; a form holds data, not a part.
    let parts = |c: &str| descendants(&s, find(&s, c)).into_iter().filter(|n| n.kind == Kind::Symbol).map(|n| (n.title.clone(), n.subtitle.clone())).collect::<Vec<_>>();
    assert_eq!(parts("Searcher"), [("matcher".to_string(), "Matcher".to_string())]);
    assert!(parts("Form").is_empty(), "{:?}", parts("Form"));
}

#[test]
fn inline_modules_hold_their_types() {
    let s = analyze(&[
        ("Cargo.toml", "[package]\nname = \"geo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        ("src/shape.rs", "pub trait Shape {\n    fn area(&self) -> f64;\n}\n\npub fn total(s: &dyn Shape) -> f64 {\n    s.area() + 1.0\n}\n"),
        ("src/report.rs", "pub fn line(x: f64) -> String {\n    format!(\"{x:.2}\")\n}\n"),
        (
            "src/main.rs",
            "mod report;\nmod shape;\n\nmod shapes {\n    use crate::shape::Shape;\n\n    /// A square.\n    pub struct Square(pub f64);\n\n    impl Shape for Square {\n        fn area(&self) -> f64 {\n            self.0.powi(2)\n        }\n    }\n}\n\nfn main() {\n    let a = shape::total(&shapes::Square(2.0));\n    println!(\"{a}\");\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n    use crate::report::line;\n\n    #[test]\n    fn prints() {\n        assert_eq!(line(shape::total(&shapes::Square(1.0))), \"2.00\");\n    }\n}\n",
        ),
    ]);
    // The module draws its type, and the impl inside it, through the
    // module's own `use`, is a Shape: `total` reaches it.
    let square = within(&s, find(&s, "shapes"), "Square");
    assert_eq!(square.kind, Kind::Class);
    let area = descendants(&s, find(&s, "total")).into_iter().find(|n| n.kind == Kind::Call && n.title == "area").expect("area call");
    assert_eq!(area.reference, Some(within(&s, square, "area").id));
    // The test module uses report.rs; main.rs does not depend on it.
    let edges: Vec<(String, String)> = s.nodes.iter().flat_map(|n| n.edges.iter()).map(|e| (s.nodes[e.from as usize].title.clone(), s.nodes[e.to as usize].title.clone())).collect();
    assert!(!edges.iter().any(|(f, t)| f == "report.rs" && t == "main.rs"), "{edges:?}");
}
