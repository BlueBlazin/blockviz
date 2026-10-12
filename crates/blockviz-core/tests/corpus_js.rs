//! Abstraction-quality tests on JavaScript and TypeScript repositories of
//! different shapes (trimmed snapshots in fixtures/corpus-js, see its README):
//! an Express API, a Next.js app, a NestJS service, a React SPA, a TypeScript
//! library, a CLI and a workspace monorepo. Each test states what a developer
//! reading that codebase should see.

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

fn root(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus-js").join(name)
}

/// A node's code: its own, or the source at its span, which is all the scene
/// keeps when the two are the same.
fn code_of(s: &Scene, root: &Path, n: &Node) -> String {
    match &n.span {
        Some(sp) if n.code.is_empty() => {
            let f = &s.files[sp.file as usize];
            let text = f.source.clone().unwrap_or_else(|| std::fs::read_to_string(root.join(&f.path)).unwrap());
            text[sp.start as usize..sp.end as usize].to_string()
        }
        _ => n.code.clone(),
    }
}

fn scene(name: &str) -> Scene {
    let root = root(name);
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

fn file<'a>(s: &'a Scene, path: &str) -> &'a Node {
    let base = path.rsplit('/').next().unwrap();
    s.nodes
        .iter()
        .filter(|n| n.parent.is_some() && n.title.ends_with(base) && matches!(n.kind, Kind::File | Kind::Dir))
        .find(|n| path_of(s, n) == path || path_of(s, n).ends_with(&format!("/{path}")))
        .unwrap_or_else(|| panic!("no file {path}"))
}

fn kids<'a>(s: &'a Scene, n: &Node) -> Vec<&'a Node> {
    n.children.iter().map(|&c| &s.nodes[c as usize]).filter(|c| !matches!(c.kind, Kind::Input | Kind::Output)).collect()
}

fn kid<'a>(s: &'a Scene, n: &Node, title: &str) -> &'a Node {
    kids(s, n).into_iter().find(|c| c.title == title).unwrap_or_else(|| panic!("{} has no {title:?}; has {:?}", n.title, kids(s, n).iter().map(|c| (&c.title, &c.subtitle)).collect::<Vec<_>>()))
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

/// Whether `n` reaches `target`'s file through a `Uses` edge at some level.
fn imports(s: &Scene, n: &Node, target: &Node) -> bool {
    let inputs = &n.inputs;
    inputs.iter().any(|i| path_of(s, target).ends_with(i.as_str()) || i.ends_with(&target.title))
}

const ALL: &[&str] = &["express-api", "nextjs-app", "nestjs-cats", "react-vite", "ky", "serve", "turborepo-basic"];

#[test]
fn express_api_reads_as_routes_controllers_and_services() {
    let s = scene("express-api");
    let app = file(&s, "src/app.js");
    assert_eq!(app.subtitle, "app");
    // Middleware set up one after another is one ordered list.
    let middleware = kids(&s, app).into_iter().flat_map(|k| std::iter::once(k).chain(descendants(&s, k))).find(|n| n.rows.iter().any(|r| r.key.starts_with("use(helmet")));
    assert!(middleware.is_some(), "middleware list");
    // `nodemon src/index.js` in package.json makes index.js the entry point.
    assert!(file(&s, "src/index.js").badges.contains(&"entry point".into()));
    // A router's routes are a table of method, path and handler.
    let auth = file(&s, "src/routes/v1/auth.route.js");
    assert_eq!(auth.subtitle, "routes");
    let table = kids(&s, auth).into_iter().find(|n| n.kind == Kind::Table && n.category == "endpoint").expect("route table");
    assert!(table.rows.iter().any(|r| r.key == "POST /register" && r.value == "authController.register"), "{:?}", table.rows.iter().map(|r| (&r.key, &r.value)).collect::<Vec<_>>());
    let users = file(&s, "src/routes/v1/user.route.js");
    let titles: Vec<&str> = kids(&s, users).iter().map(|n| n.title.as_str()).collect();
    assert!(titles.contains(&"GET, PATCH, DELETE /:userId"), "{titles:?}");
    // `const createUser = catchAsync(async (req, res) => …)` is a function, decorated.
    let controller = file(&s, "src/controllers/user.controller.js");
    let create = kid(&s, controller, "createUser");
    assert_eq!(create.kind, Kind::Function);
    assert!(create.badges.contains(&"@catchAsync".into()), "{:?}", create.badges);
    // It calls the service, which writes to the database through the Mongoose model.
    let call = kids(&s, create).into_iter().find(|n| n.title == "createUser" && n.kind == Kind::Call).expect("call to the service");
    let service = &s.nodes[call.reference.expect("zooms into the service") as usize];
    assert!(service.effects.contains(&"database".into()), "{:?}", service.effects);
    // A Mongoose schema is a table of fields.
    let model = file(&s, "src/models/user.model.js");
    let schema = kid(&s, model, "userSchema");
    assert_eq!(schema.kind, Kind::Table);
    assert!(schema.rows.iter().any(|r| r.key == "email"));
    // Node's own modules are not third-party packages.
    let externals = s.nodes.iter().find(|n| n.title == "External packages").unwrap();
    let names: Vec<&str> = externals.rows.iter().map(|r| r.key.as_str()).collect();
    assert!(names.contains(&"express") && names.contains(&"mongoose"), "{names:?}");
    assert!(!names.iter().any(|n| matches!(*n, "fs" | "path" | "http" | "child_process" | "util")), "{names:?}");
}

#[test]
fn nextjs_app_reads_as_pages_routes_and_components() {
    let s = scene("nextjs-app");
    assert_eq!(file(&s, "app/(dashboard)/dashboard/page.tsx").subtitle, "page /dashboard");
    assert!(file(&s, "app/(dashboard)/dashboard/layout.tsx").subtitle.starts_with("layout"));
    // Route handlers are named by method and URL; their input schema is a table.
    let posts = file(&s, "app/api/posts/route.ts");
    assert!(posts.subtitle.starts_with("routes"), "{}", posts.subtitle);
    assert_eq!(kid(&s, posts, "GET").subtitle, "GET /api/posts");
    assert_eq!(kid(&s, posts, "POST").subtitle, "POST /api/posts");
    let schema = kid(&s, posts, "postCreateSchema");
    assert_eq!(schema.kind, Kind::Table);
    assert!(schema.rows.iter().any(|r| r.key == "title"));
    // Prisma through `@/lib/db` is database access.
    let post = file(&s, "app/api/posts/[postId]/route.ts");
    assert!(kid(&s, post, "DELETE").effects.contains(&"database".into()));
    // A client component renders the components it is built from.
    let billing = file(&s, "components/billing-form.tsx");
    assert!(billing.badges.contains(&"client".into()), "{:?}", billing.badges);
    let form = kid(&s, billing, "BillingForm");
    assert_eq!(form.category, "component");
    let inside: Vec<&str> = kids(&s, form).iter().map(|n| n.title.as_str()).collect();
    for t in ["Card", "CardHeader", "onSubmit", "<form>"] {
        assert!(inside.contains(&t), "{t} not in {inside:?}");
    }
    // `@/lib/utils` resolves through tsconfig paths.
    assert!(imports(&s, billing, file(&s, "lib/utils.ts")), "{:?}", billing.inputs);
}

#[test]
fn nestjs_controllers_services_and_dtos() {
    let s = scene("nestjs-cats");
    let controller = file(&s, "src/cats/cats.controller.ts");
    assert_eq!(controller.subtitle, "routes");
    let class = kid(&s, controller, "CatsController");
    let routes: Vec<(&str, &str)> = kids(&s, class).iter().filter(|n| n.kind == Kind::Function).map(|n| (n.title.as_str(), n.subtitle.as_str())).collect();
    for want in [("create", "POST /cats"), ("findAll", "GET /cats"), ("findOne", "GET /cats/:id")] {
        assert!(routes.contains(&want), "{want:?} not in {routes:?}");
    }
    // The injected service is a component, and calls on it zoom into it.
    let comps = kid(&s, class, "components");
    assert_eq!(kid(&s, comps, "catsService").subtitle, "CatsService");
    let create = kid(&s, class, "create");
    let call = kids(&s, create).into_iter().find(|n| n.kind == Kind::Call && n.title == "create").expect("service call");
    assert_eq!(s.nodes[call.reference.unwrap() as usize].title, "create");
    // A DTO is a table of its fields.
    let dto = file(&s, "src/cats/dto/create-cat.dto.ts");
    let fields: Vec<&str> = kid(&s, dto, "CreateCatDto").rows.iter().map(|r| r.key.as_str()).collect();
    assert_eq!(fields, ["name", "age", "breed"]);
    assert_eq!(file(&s, "e2e/cats/cats.e2e-spec.ts").subtitle, "tests");
    // Test suites are named by what they describe.
    let spec = file(&s, "src/cats/cats.service.spec.ts");
    assert!(descendants(&s, spec).iter().any(|n| n.title == "should add a new cat" && n.subtitle == "it"));
}

#[test]
fn react_app_reads_as_components_hooks_and_queries() {
    let s = scene("react-vite");
    let get = file(&s, "src/features/discussions/api/get-discussion.ts");
    let hook = kid(&s, get, "useDiscussion");
    assert!(hook.badges.contains(&"hook".into()));
    let calls: Vec<&str> = kids(&s, hook).iter().map(|n| n.title.as_str()).collect();
    assert!(calls.contains(&"getDiscussionQueryOptions") && calls.contains(&"useQuery"), "{calls:?}");
    let view = file(&s, "src/features/discussions/components/discussion-view.tsx");
    assert_eq!(view.subtitle, "components");
    let component = kid(&s, view, "DiscussionView");
    assert_eq!(component.category, "component");
    // The component's data comes from the hook defined in the feature.
    let uses = descendants(&s, component).into_iter().find(|n| n.title == "useDiscussion").expect("hook call");
    assert_eq!(s.nodes[uses.reference.unwrap() as usize].title, "useDiscussion");
}

#[test]
fn library_shows_its_classes_and_types() {
    let s = scene("ky");
    let ky = file(&s, "source/core/Ky.ts");
    let class = kid(&s, ky, "Ky");
    assert_eq!(class.kind, Kind::Class);
    assert!(kids(&s, class).iter().filter(|n| n.kind == Kind::Function).count() >= 5);
    // Interfaces describe data: tables of their fields.
    let options = file(&s, "source/types/options.ts");
    assert!(kids(&s, options).iter().any(|n| n.kind == Kind::Class && !n.rows.is_empty()));
    // Errors extend a base error defined in the project.
    let http = file(&s, "source/errors/HTTPError.ts");
    assert!(kids(&s, http).iter().any(|n| n.kind == Kind::Class && n.subtitle.contains("KyError")), "{:?}", kids(&s, http).iter().map(|n| (&n.title, &n.subtitle)).collect::<Vec<_>>());
}

#[test]
fn cli_reads_as_a_script_with_sections() {
    let s = scene("serve");
    let main = file(&s, "source/main.ts");
    assert!(main.badges.contains(&"entry point".into()), "the package's bin runs it: {:?}", main.badges);
    let sections: Vec<&str> = kids(&s, main).iter().filter(|n| n.kind == Kind::Section).map(|n| n.title.as_str()).collect();
    assert!(sections.iter().any(|t| t.starts_with("Parse the options")), "{sections:?}");
}

#[test]
fn workspace_packages_resolve_by_name() {
    let s = scene("turborepo-basic");
    let page = file(&s, "apps/web/app/page.tsx");
    assert_eq!(page.subtitle, "page /");
    // `@repo/ui/button` is the workspace package's file.
    assert!(imports(&s, page, file(&s, "packages/ui/src/button.tsx")), "{:?}", page.inputs);
    // A CSS module the page imports is linked to it.
    let parent = &s.nodes[page.parent.unwrap() as usize];
    let css = kids(&s, parent).into_iter().find(|n| n.title == "page.module.css").expect("css module");
    assert!(parent.edges.iter().any(|e| e.from == css.id && e.to == page.id && e.kind == EdgeKind::Uses));
}

#[test]
fn titles_are_names_and_branch_arms_are_never_empty() {
    for name in ALL {
        let s = scene(name);
        let root = root(name);
        for n in s.nodes.iter().filter(|n| matches!(n.kind, Kind::Op | Kind::Call | Kind::Table)) {
            if let Some(cut) = n.title.strip_suffix('…') {
                let code = code_of(&s, &root, n);
                let squashed: String = code.split_whitespace().collect::<Vec<_>>().join(" ");
                assert!(cut.len() < 4 || !squashed.starts_with(cut.trim_end()), "{name}: {:?} is cut from `{code}`", n.title);
            }
        }
        for lane in s.nodes.iter().filter(|n| n.kind == Kind::Lane) {
            if lane.children.iter().any(|&c| !s.nodes[c as usize].is_port()) {
                continue;
            }
            let sp = lane.span.as_ref().expect("lanes have spans");
            let text = std::fs::read_to_string(root.join(&s.files[sp.file as usize].path)).unwrap();
            let body = text.lines().skip(sp.line as usize - 1).take((sp.end_line - sp.line + 1) as usize).map(|l| l.split("//").next().unwrap().trim()).collect::<Vec<_>>().join(" ");
            let quiet = body.trim().is_empty() || body.trim() == "{}" || (!lane.effects.is_empty() && ["console.", "warn(", "log"].iter().any(|w| body.contains(w)));
            assert!(quiet, "{name}: empty arm {:?} in {} (`{body}`)", lane.title, s.files[sp.file as usize].path);
        }
    }
}

#[test]
fn layouts_are_sound_and_analysis_deterministic() {
    for name in ALL {
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
        assert!(s.diagnostics.iter().all(|d| !d.starts_with("Skipped")), "{name}: {:?}", s.diagnostics);
    }
    let a = serde_json::to_string(&scene("nextjs-app")).unwrap();
    let b = serde_json::to_string(&scene("nextjs-app")).unwrap();
    assert_eq!(a, b);
}
