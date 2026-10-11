//! Abstraction-quality tests on real repositories of different shapes (trimmed
//! snapshots in fixtures/corpus, see its README): a web app, an async library,
//! a training pipeline, a framework tutorial, a Django site and a CLI. Each test
//! states what a developer reading that codebase should see.

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
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus").join(name);
    let mut files = vec![];
    load(&root, &root, &mut files);
    blockviz_core::analyze(name, files)
}

/// The repository path a node is drawn at (directory titles end in `/`;
/// single-child directories are folded into their child's title).
fn path_of(s: &Scene, n: &Node) -> String {
    let mut parts = vec![n.title.clone()];
    let mut cur = n.parent;
    while let Some(p) = cur {
        let pn = &s.nodes[p as usize];
        if pn.parent.is_some() && pn.title.ends_with('/') {
            parts.push(pn.title.clone());
        }
        cur = pn.parent;
    }
    parts.reverse();
    parts.concat()
}

/// The file drawn at `path`, wherever directories were folded or grouped.
fn file<'a>(s: &'a Scene, path: &str) -> &'a Node {
    let base = path.rsplit('/').next().unwrap();
    s.nodes
        .iter()
        .filter(|n| n.parent.is_some() && n.title.ends_with(base) && matches!(n.kind, Kind::File | Kind::Notebook | Kind::Dir))
        .find(|n| path_of(s, n) == path || path_of(s, n).ends_with(&format!("/{path}")))
        .unwrap_or_else(|| panic!("no file {path}"))
}

fn kids<'a>(s: &'a Scene, n: &Node) -> Vec<&'a Node> {
    n.children.iter().map(|&c| &s.nodes[c as usize]).filter(|c| !matches!(c.kind, Kind::Input | Kind::Output)).collect()
}

fn kid<'a>(s: &'a Scene, n: &Node, title: &str) -> &'a Node {
    kids(s, n).into_iter().find(|c| c.title == title).unwrap_or_else(|| panic!("{} has no {title:?}; has {:?}", n.title, kids(s, n).iter().map(|c| &c.title).collect::<Vec<_>>()))
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

fn reached_by(s: &Scene, n: &Node, kind: EdgeKind) -> Vec<String> {
    s.nodes.iter().flat_map(|p| p.edges.iter()).filter(|e| e.kind == kind && e.to == n.id).map(|e| e.label.clone()).collect()
}

#[test]
fn web_app_routes_read_as_endpoints() {
    let s = scene("fastapi-template");
    let items = file(&s, "backend/app/api/routes/items.py");
    assert_eq!(items.subtitle, "routes");
    let routes: Vec<(&str, &str)> = kids(&s, items).iter().filter(|n| n.kind == Kind::Function).map(|n| (n.title.as_str(), n.subtitle.as_str())).collect();
    for want in [("read_items", "GET /items/"), ("read_item", "GET /items/{id}"), ("create_item", "POST /items/"), ("update_item", "PUT /items/{id}"), ("delete_item", "DELETE /items/{id}")] {
        assert!(routes.contains(&want), "{want:?} not in {routes:?}");
    }
    assert!(kids(&s, items).iter().all(|n| !n.badges.iter().any(|b| b.starts_with('@'))), "no raw decorator text");
    // The router composition is one table, not five identical blocks.
    let api = file(&s, "backend/app/api/main.py");
    let table = kid(&s, api, "api_router.include_router");
    assert_eq!(table.kind, Kind::Table);
    assert!(table.rows.iter().any(|r| r.key == "items.router"));
}

#[test]
fn web_app_entry_points_and_plumbing() {
    let s = scene("fastapi-template");
    let main = file(&s, "backend/app/main.py");
    assert_eq!(main.subtitle, "app");
    assert!(main.badges.contains(&"entry point".into()));
    assert!(!reached_by(&s, main, EdgeKind::Exec).is_empty(), "the Dockerfile's `fastapi run` runs main.py");
    // Package markers with nothing in them are not drawn.
    assert!(s.nodes.iter().filter(|n| n.parent.is_some()).all(|n| !(n.title == "__init__.py" && path_of(&s, n).contains("routes/"))));
    // `from app import crud` imports crud.py, not the package.
    let db = file(&s, "backend/app/core/db.py");
    assert!(db.inputs.iter().any(|i| i.ends_with("app/crud.py")), "{:?}", db.inputs);
    // Alembic revisions are generated: one block, not a diagram per migration.
    let versions = s.nodes.iter().find(|n| n.parent.is_some() && n.title.ends_with("versions/")).expect("versions dir");
    assert!(versions.subtitle.ends_with("generated files"), "{}", versions.subtitle);
    assert!(versions.children.is_empty());
    // Dependency aliases (`SessionDep = Annotated[Session, Depends(get_db)]`) are types, not blocks.
    let deps = file(&s, "backend/app/api/deps.py");
    assert!(kids(&s, deps).iter().all(|n| n.title != "Depends"));
}

#[test]
fn compose_file_is_the_system_diagram() {
    let s = scene("fastapi-template");
    let compose = file(&s, "compose.yml");
    assert_eq!(compose.subtitle, "4 services");
    let backend = kid(&s, compose, "backend");
    assert_eq!(s.nodes[backend.reference.unwrap() as usize].title, "backend/", "a built service zooms into its code");
    assert_eq!(kid(&s, compose, "db").subtitle, "postgres:18");
    let edges: Vec<(&str, &str)> = compose.edges.iter().map(|e| (s.nodes[e.from as usize].title.as_str(), s.nodes[e.to as usize].title.as_str())).collect();
    assert!(edges.contains(&("db", "backend")), "{edges:?}");
    assert!(edges.contains(&("proxy", "backend")), "{edges:?}");
    // Database access and ORM tables are visible.
    let models = file(&s, "backend/app/models.py");
    assert!(kid(&s, models, "Item").badges.contains(&"DB table".into()));
    let crud = file(&s, "backend/app/crud.py");
    assert!(crud.effects.contains(&"database".into()));
}

#[test]
fn library_modules_show_definitions_not_declarations() {
    let s = scene("httpx");
    for n in &s.nodes {
        assert_ne!(n.title, "__all__", "export lists are metadata");
        assert_ne!(n.title, "TypeVar", "type variables are not steps");
        assert!(!n.title.starts_with("getLogger"), "logger setup is not a step");
    }
    // Not a neural network: `head`, `headers` and `normalize` carry no architecture colours.
    let ml = ["attention", "norm", "embed", "activation", "dropout", "mlp", "linear", "loss"];
    assert!(s.nodes.iter().all(|n| !ml.contains(&n.category)), "{:?}", s.nodes.iter().filter(|n| ml.contains(&n.category)).map(|n| &n.title).collect::<Vec<_>>());
    let client = file(&s, "httpx/_client.py");
    assert!(kids(&s, client).iter().all(|n| n.kind != Kind::Section), "a long comment does not section a library module");
    assert!(kids(&s, client).iter().any(|n| n.title == "Client" && n.kind == Kind::Class));
    let decoders = file(&s, "httpx/_decoders.py");
    assert!(kids(&s, decoders).iter().all(|n| !matches!(n.kind, Kind::Branch | Kind::Op)), "optional imports and table tweaks are not blocks: {:?}", kids(&s, decoders).iter().map(|n| &n.title).collect::<Vec<_>>());
    let config = file(&s, "httpx/_config.py");
    assert_eq!(kid(&s, config, "Constants").kind, Kind::Table);
    let init = file(&s, "httpx/__init__.py");
    assert!(init.outputs.contains(&"AsyncClient".to_string()), "exports are listed on the package");
    assert!(kids(&s, init).iter().all(|n| n.kind != Kind::Loop));
}

#[test]
fn training_pipeline_script_shows_its_steps() {
    let s = scene("nanochat");
    let speedrun = file(&s, "runs/speedrun.sh");
    let steps: Vec<&str> = kids(&s, speedrun).iter().map(|n| n.title.as_str()).collect();
    assert_eq!(steps, ["dataset.py", "dataset.py", "tok_train.py", "tok_eval.py", "base_train.py", "base_eval.py", "chat_sft.py", "chat_eval.py"]);
    let train = kid(&s, speedrun, "base_train.py");
    assert_eq!(s.nodes[train.reference.unwrap() as usize].title, "base_train.py", "a step zooms into the program it runs");
    assert!(train.subtitle.starts_with("torchrun"), "{}", train.subtitle);
    let base_train = file(&s, "scripts/base_train.py");
    assert!(base_train.badges.contains(&"entry point".into()));
    assert!(!reached_by(&s, base_train, EdgeKind::Exec).is_empty());
}

#[test]
fn flask_views_and_signals() {
    let s = scene("flask");
    let auth = file(&s, "examples/tutorial/flaskr/auth.py");
    assert_eq!(kid(&s, auth, "register").subtitle, "GET, POST /auth/register");
    assert_eq!(kid(&s, auth, "load_logged_in_user").subtitle, "before app request");
    assert_eq!(file(&s, "examples/tutorial/flaskr/__init__.py").subtitle, "app");
    let blog = file(&s, "examples/tutorial/flaskr/blog.py");
    assert_eq!(kid(&s, blog, "index").subtitle, "GET /");
    // Ten signals declared in a row are one table.
    let signals = file(&s, "src/flask/signals.py");
    let table = kids(&s, signals).into_iter().find(|n| n.kind == Kind::Table && n.title.ends_with("signal")).expect("signal table");
    assert!(table.rows.len() >= 8, "{}", table.rows.len());
    assert!(file(&s, "src/flask/__main__.py").badges.contains(&"entry point".into()));
    // Views are linked to the templates they render; Flask's cookie session is not a database.
    let templates = file(&s, "examples/tutorial/flaskr/templates/");
    let parent = &s.nodes[templates.parent.unwrap() as usize];
    for view in ["auth.py", "blog.py"] {
        assert!(parent.edges.iter().any(|e| e.from == templates.id && s.nodes[e.to as usize].title == view && e.label == "template"), "templates → {view}");
    }
    let load_user = kid(&s, auth, "load_logged_in_user");
    let cookie = kids(&s, load_user).into_iter().find(|n| n.title == "get" && n.subtitle == "session").expect("session.get");
    assert!(!cookie.effects.contains(&"database".into()), "the cookie session is not a database");
    assert!(load_user.effects.contains(&"database".into()), "get_db().execute(…) is");
    assert!(kid(&s, auth, "register").effects.contains(&"database".into()));
}

#[test]
fn django_project_reads_as_routes_settings_and_apps() {
    let s = scene("bakerydemo");
    let urls = file(&s, "bakerydemo/urls.py");
    assert_eq!(urls.subtitle, "routes");
    let table = kid(&s, urls, "urlpatterns");
    for key in ["admin/", "search/", "api/v2/"] {
        assert!(table.rows.iter().any(|r| r.key == key), "{key} not in {:?}", table.rows.iter().map(|r| &r.key).collect::<Vec<_>>());
    }
    assert_eq!(file(&s, "bakerydemo/settings/production.py").subtitle, "config");
    assert_eq!(file(&s, "bakerydemo/wsgi.py").subtitle, "app");
    let migrations = file(&s, "bakerydemo/blog/migrations/");
    assert_eq!(migrations.subtitle, "8 generated files");
    let entry = file(&s, "docker-entrypoint.sh");
    assert!(kids(&s, entry).iter().all(|n| n.title == "manage.py"));
}

#[test]
fn cli_entry_points_come_from_packaging() {
    let s = scene("httpie");
    let main = file(&s, "httpie/__main__.py");
    assert!(main.badges.contains(&"entry point".into()));
    let labels = reached_by(&s, main, EdgeKind::Exec);
    assert!(labels.iter().any(|l| l.contains("installs `http`")), "{labels:?}");
    // Argument definitions repeat one call: they read as tables of options.
    let definition = file(&s, "httpie/cli/definition.py");
    assert!(descendants(&s, definition).iter().any(|n| n.kind == Kind::Table && n.title.ends_with(".add_argument")));
}

#[test]
fn corpus_layouts_are_sound() {
    for name in ["fastapi-template", "httpx", "nanochat", "flask", "bakerydemo", "httpie"] {
        let s = scene(name);
        for n in &s.nodes {
            if n.parent.is_none() && n.id != s.root {
                continue;
            }
            for &c in &n.children {
                let k = &s.nodes[c as usize];
                assert!(k.x >= -0.5 && k.y >= -0.5 && k.x + k.w <= n.cw + 0.5 && k.y + k.h <= n.ch + 0.5, "{name}: {} escapes {}", k.title, n.title);
            }
            for e in &n.edges {
                assert!(e.points.len() >= 4, "{name}: edge without route in {}", n.title);
            }
        }
    }
}

#[test]
fn branch_arms_show_what_they_do() {
    // Every arm of a branch draws at least one step, unless all it does is
    // `pass` or log (shown as the arm's side-effect mark).
    for name in ["fastapi-template", "httpx", "nanochat", "flask", "bakerydemo", "httpie"] {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus").join(name);
        let s = scene(name);
        for lane in s.nodes.iter().filter(|n| n.kind == Kind::Lane) {
            if lane.children.iter().any(|&c| !s.nodes[c as usize].is_port()) {
                continue;
            }
            let sp = lane.span.as_ref().expect("lanes have spans");
            let text = std::fs::read_to_string(root.join(&s.files[sp.file as usize].path)).unwrap();
            let body = text.lines().skip(sp.line as usize - 1).take((sp.end_line - sp.line + 1) as usize).map(|l| l.split('#').next().unwrap().trim()).collect::<Vec<_>>().join(" ");
            let quiet = body.trim() == "pass" || (!lane.effects.is_empty() && ["warn(", "log", "print("].iter().any(|w| body.contains(w)));
            assert!(quiet, "{name}: empty arm {:?} in {} (`{body}`)", lane.title, s.files[sp.file as usize].path);
        }
    }
}

#[test]
fn titles_are_names_not_cut_code() {
    // A block is titled by a name or a short phrase; source code appears in a
    // title only when it fits whole, never cut off mid-expression.
    for name in ["fastapi-template", "httpx", "nanochat", "flask", "bakerydemo", "httpie"] {
        let s = scene(name);
        for n in s.nodes.iter().filter(|n| matches!(n.kind, Kind::Op | Kind::Call | Kind::Table)) {
            let squashed: String = n.code.split_whitespace().collect::<Vec<_>>().join(" ");
            if let Some(cut) = n.title.strip_suffix('…') {
                assert!(cut.len() < 4 || !squashed.starts_with(cut.trim_end()), "{name}: {:?} is cut from `{}`", n.title, n.code);
            }
            for b in &n.badges {
                if let Some(cut) = b.strip_prefix("if ").and_then(|b| b.strip_suffix('…')) {
                    assert!(!cut.ends_with(|c: char| c.is_alphanumeric() || c == '_'), "{name}: badge {b:?} on {} cuts a token", n.title);
                }
            }
        }
    }
}
