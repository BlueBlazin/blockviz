//! What framework code means to a reader: an HTTP route, a CLI command, a
//! background task, an event handler. Recognised by shape (`@<router>.get("/path")`,
//! `@Get(":id")` in a `@Controller("users")`, `router.get("/", …)`, a Next.js
//! `route.ts` exporting `GET`) rather than by import, so one rule covers FastAPI,
//! Flask, Starlette, Litestar, NestJS, Express, Koa, Hono and their routers.

use super::flow::string_literal;
use super::syntax::{self as ast, Expr, Stmt};
use std::collections::HashMap;

/// A function's role in its framework.
pub struct Role {
    /// Replaces the parameter list as the function's subtitle: `GET /items/{id}`.
    pub label: String,
    pub kind: Handler,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handler {
    Route,
    Command,
    Task,
    Hook,
    /// A UI component (React).
    Component,
}

const HTTP: &[&str] = &["get", "post", "put", "patch", "delete", "head", "options", "trace"];

/// Constructors whose instance is the application a server or CLI runs.
pub const APPS: &[&str] = &[
    "FastAPI", "Flask", "Starlette", "Quart", "Sanic", "Litestar", "Celery", "Typer", "get_asgi_application", "get_wsgi_application", "Application", "Bottle", "Falcon", "App", "express", "Hono", "Koa", "Fastify", "fastify", "Elysia", "createServer", "ApolloServer", "createApp", "createRoot", "hydrateRoot",
];

/// Route prefixes of routers and blueprints defined at module level:
/// `router = APIRouter(prefix="/items")`, `bp = Blueprint("auth", __name__, url_prefix="/auth")`.
pub fn prefixes(body: &[Stmt]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for s in body {
        let Stmt::Assign(a) = s else { continue };
        let (Some(Expr::Name(target)), Expr::Call(c)) = (a.targets.first(), &*a.value) else { continue };
        let ctor = last_segment(&c.func);
        if !matches!(ctor, "APIRouter" | "Blueprint" | "Router" | "APIBlueprint" | "Namespace") {
            continue;
        }
        let prefix = c.arguments.keywords.iter().find(|k| matches!(k.arg.as_deref(), Some("prefix" | "url_prefix" | "path"))).and_then(|k| string_literal(&k.value));
        if let Some(p) = prefix {
            out.insert(target.id.to_string(), p.to_string());
        }
    }
    out
}

fn last_segment(e: &Expr) -> &str {
    match e {
        Expr::Name(n) => n.id.as_str(),
        Expr::Attribute(a) => a.attr.as_str(),
        _ => "",
    }
}

fn dotted(e: &Expr) -> Option<String> {
    match e {
        Expr::Name(n) => Some(n.id.to_string()),
        Expr::Attribute(a) => Some(format!("{}.{}", dotted(&a.value)?, a.attr)),
        _ => None,
    }
}

pub fn join_path(prefix: &str, path: &str) -> String {
    let p = format!("{}/{}", prefix.trim_end_matches('/'), path.trim_start_matches('/'));
    if p.len() > 1 && !path.ends_with('/') {
        p.trim_end_matches('/').to_string()
    } else if p.is_empty() {
        "/".into()
    } else {
        p
    }
}

/// The framework role a decorator gives the function it wraps.
pub fn role(d: &Expr, function: &str, prefixes: &HashMap<String, String>, class_prefix: Option<&str>, text: impl Fn(&Expr) -> String) -> Option<Role> {
    let (func, call) = match d {
        Expr::Call(c) => (&*c.func, Some(c)),
        e => (e, None),
    };
    let (recv, attr) = match func {
        Expr::Attribute(a) => (dotted(&a.value), a.attr.as_str()),
        Expr::Name(n) => (None, n.id.as_str()),
        _ => return None,
    };
    let args: &[Expr] = call.map(|c| &c.arguments.args[..]).unwrap_or(&[]);
    let keyword = |k: &str| call.and_then(|c| c.arguments.keywords.iter().find(|x| x.arg.as_deref() == Some(k)).map(|x| &x.value));
    let first = args.first().and_then(string_literal);
    let path = first.or_else(|| keyword("path").and_then(string_literal)).or_else(|| keyword("rule").and_then(string_literal));
    let prefix = recv.as_deref().and_then(|r| prefixes.get(r)).map(String::as_str).unwrap_or("");
    let route = |methods: &str, p: &str| Some(Role { label: format!("{methods} {}", join_path(prefix, p)), kind: Handler::Route });
    let is_path = |p: &str| p.is_empty() || p.starts_with('/');
    let pytest = recv.as_deref().is_some_and(|r| r == "pytest" || r.starts_with("pytest."));
    // NestJS: `@Get(":id")` in a `@Controller("users")` class.
    if recv.is_none() && attr.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        let lower = attr.to_ascii_lowercase();
        if HTTP.contains(&lower.as_str()) || lower == "all" {
            let label = format!("{} {}", attr.to_uppercase(), join_path(&join_path("", class_prefix.unwrap_or("")), path.unwrap_or("")));
            return Some(Role { label, kind: Handler::Route });
        }
        match attr {
            "Cron" | "Interval" | "Timeout" | "Process" => return Some(Role { label: "scheduled task".into(), kind: Handler::Task }),
            "MessagePattern" | "EventPattern" | "OnEvent" | "SubscribeMessage" => return Some(Role { label: format!("on {}", first.unwrap_or("message")), kind: Handler::Hook }),
            "Query" | "Mutation" | "Subscription" if call.is_some() => return Some(Role { label: format!("{} {function}", attr.to_ascii_lowercase()), kind: Handler::Route }),
            _ => {}
        }
    }
    match attr {
        m if HTTP.contains(&m) && path.is_some_and(is_path) => route(&m.to_uppercase(), path.unwrap()),
        "route" | "api_route" if path.is_some_and(is_path) => {
            let methods: Vec<String> = match keyword("methods") {
                Some(Expr::List(l)) => l.elts.iter().filter_map(string_literal).map(|m| m.to_uppercase()).collect(),
                Some(Expr::Tuple(t)) => t.elts.iter().filter_map(string_literal).map(|m| m.to_uppercase()).collect(),
                _ => vec![],
            };
            let methods = if methods.is_empty() { "GET".to_string() } else { methods.join(", ") };
            route(&methods, path.unwrap())
        }
        "websocket" | "websocket_route" | "ws" if path.is_some_and(is_path) => route("WS", path.unwrap()),
        "command" | "group" if !pytest && (recv.is_some() || call.is_some()) => {
            let name = first.map(str::to_string).or_else(|| keyword("name").and_then(string_literal).map(str::to_string)).unwrap_or_else(|| function.replace('_', "-"));
            let what = if attr == "group" { "command group" } else { "command" };
            Some(Role { label: format!("{what} {name}"), kind: Handler::Command })
        }
        "task" | "shared_task" | "actor" | "periodic_task" | "job" | "cron" if !pytest => Some(Role { label: "background task".into(), kind: Handler::Task }),
        "on_event" => Some(Role { label: format!("on {}", first.unwrap_or("event")), kind: Handler::Hook }),
        "middleware" => Some(Role { label: "middleware".into(), kind: Handler::Hook }),
        "exception_handler" | "errorhandler" | "register_error_handler" => {
            let what = args.first().map(&text).unwrap_or_else(|| "errors".into());
            Some(Role { label: format!("handles {what}"), kind: Handler::Hook })
        }
        "before_request" | "before_app_request" | "after_request" | "after_app_request" | "teardown_request" | "teardown_appcontext" | "context_processor" | "app_context_processor" | "url_value_preprocessor" | "template_filter" | "app_template_filter" | "template_global" | "shell_context_processor" | "lifespan" => {
            Some(Role { label: attr.replace('_', " "), kind: Handler::Hook })
        }
        "receiver" | "listens_for" | "listen" if !args.is_empty() => Some(Role { label: format!("on {}", args.iter().map(&text).collect::<Vec<_>>().join(", ")), kind: Handler::Hook }),
        "hookimpl" => Some(Role { label: "plugin hook".into(), kind: Handler::Hook }),
        _ => None,
    }
}

/// The trait a Rust method implements, as written in its `impl` (`Display`,
/// `From<io::Error>`), without the module path.
pub fn implemented_trait(decorators: &[ast::Decorator], text: impl Fn(&Expr) -> String) -> Option<String> {
    decorators.iter().find_map(|d| match &d.expression {
        Expr::Call(c) if matches!(&*c.func, Expr::Name(n) if n.id == "impl") => {
            let t = text(c.arguments.args.first()?);
            let head = t.split('<').next().unwrap_or(&t);
            let cut = head.rfind("::").map(|i| i + 2).unwrap_or(0);
            Some(t[cut..].to_string())
        }
        _ => None,
    })
}

/// What a Rust attribute says about an item: derives that make a type a
/// command-line interface, a database row or an error; tests; tracing.
/// Derives of standard traits and helper attributes (`#[serde(…)]`) say
/// nothing worth a badge.
fn rust_tag(d: &ast::Decorator, func: &Expr) -> Option<String> {
    let name = dotted(func)?;
    let last = name.rsplit('.').next().unwrap_or(&name);
    match last {
        "derive" => {
            let Expr::Call(c) = &d.expression else { return None };
            let derived: Vec<&str> = c.arguments.args.iter().map(last_segment).collect();
            let any = |names: &[&str]| derived.iter().any(|d| names.contains(d));
            if any(&["Parser", "StructOpt"]) {
                Some("command line".into())
            } else if any(&["Subcommand"]) {
                Some("subcommands".into())
            } else if any(&["Args", "ValueEnum"]) {
                Some("arguments".into())
            } else if any(&["FromRow", "Queryable", "Insertable", "Selectable", "AsChangeset", "DeriveEntityModel", "Identifiable", "Model", "SimpleObject"]) {
                Some("DB row".into())
            } else if any(&["Error", "Diagnostic"]) {
                Some("error type".into())
            } else if any(&["Component", "Resource", "Bundle", "Event", "Message", "States"]) {
                Some(derived.iter().find(|d| matches!(**d, "Component" | "Resource" | "Bundle" | "Event" | "Message" | "States")).map(|d| d.to_ascii_lowercase()).unwrap_or_default())
            } else if any(&["Serialize", "Deserialize"]) {
                Some("serializable".into())
            } else {
                None
            }
        }
        // `#[cfg(unix)]`: the platform or feature the item exists for.
        "cfg" => match &d.expression {
            Expr::Call(c) => c.arguments.args.first().and_then(string_literal).map(str::to_string),
            _ => None,
        },
        "test" | "rstest" | "quickcheck" | "proptest" => Some("test".into()),
        "bench" => Some("benchmark".into()),
        "instrument" => Some("traced".into()),
        // The trait a method implements is its subtitle, not a badge; helper
        // attributes of derives (`#[serde(…)]`, `#[arg(…)]`) and anything
        // else unknown say nothing a reader needs on the diagram.
        _ => None,
    }
}

/// `@Controller("users")` → `users`.
pub fn controller_prefix(decorators: &[ast::Decorator]) -> Option<String> {
    decorators.iter().find_map(|d| match &d.expression {
        Expr::Call(c) if matches!(&*c.func, Expr::Name(n) if n.id.as_str() == "Controller") => Some(c.arguments.args.first().and_then(string_literal).unwrap_or("").to_string()),
        _ => None,
    })
}

/// A Next.js App Router handler: `app/api/users/route.ts` exporting `GET` is
/// `GET /api/users` (route groups like `(admin)` are not part of the path).
pub fn file_route(path: &str, function: &str) -> Option<String> {
    if !HTTP.contains(&function.to_ascii_lowercase().as_str()) || function.chars().any(|c| c.is_ascii_lowercase()) {
        return None;
    }
    let (dir, base) = path.rsplit_once('/')?;
    if !matches!(base, "route.ts" | "route.js" | "route.tsx" | "route.jsx" | "route.mjs") {
        return None;
    }
    Some(format!("{function} {}", app_path(dir)?))
}

/// The URL a Next.js App Router directory serves: `src/app/(shop)/items/[id]` → `/items/[id]`.
pub fn app_path(dir: &str) -> Option<String> {
    let parts: Vec<&str> = dir.split('/').collect();
    let at = parts.iter().position(|p| *p == "app")?;
    let route: Vec<&str> = parts[at + 1..].iter().copied().filter(|p| !(p.starts_with('(') && p.ends_with(')')) && !p.starts_with('@')).collect();
    Some(format!("/{}", route.join("/")))
}

/// Where each module's router is mounted: `app.use("/api/users", usersRouter)`
/// in one module gives the routes of the module `usersRouter` comes from that
/// prefix. Mounts nest (`api.use("/users", users)` under `app.use("/api", api)`).
pub fn mounts(modules: &[super::Module], ix: &super::index::Index) -> HashMap<usize, String> {
    let mut found: Vec<(usize, usize, String)> = vec![];
    for (mi, m) in modules.iter().enumerate() {
        if !m.lang.is_js() {
            continue;
        }
        super::walk::stmt_exprs(&m.ast, true, &mut |e| {
            let Expr::Call(c) = e else { return };
            let Expr::Attribute(a) = &*c.func else { return };
            if a.attr != "use" || c.arguments.args.len() < 2 {
                return;
            }
            let Some(prefix) = string_literal(&c.arguments.args[0]) else { return };
            if !prefix.starts_with('/') {
                return;
            }
            for arg in &c.arguments.args[1..] {
                let name = match arg {
                    Expr::Name(n) => n.id.as_str(),
                    Expr::Attribute(x) => x.attr.as_str(),
                    _ => continue,
                };
                let target = match ix.sym(mi, name) {
                    Some(super::index::Sym::Module(t)) | Some(super::index::Sym::Member(t, _)) => Some(*t),
                    _ => None,
                };
                if let Some(t) = target {
                    found.push((mi, t, prefix.to_string()));
                }
            }
        });
    }
    let mut out: HashMap<usize, String> = HashMap::new();
    // Resolve nesting by repeatedly prefixing with the mounting module's own mount.
    for _ in 0..4 {
        for (from, to, prefix) in &found {
            let parent = out.get(from).cloned().unwrap_or_default();
            out.insert(*to, join_path(&parent, prefix));
        }
    }
    out
}

/// A route a router builder registers: the method, the full path and the
/// handler expression.
pub struct Registered<'e> {
    pub verb: String,
    pub path: String,
    pub handler: &'e Expr,
}

/// The routes a router-building expression registers, by shape: axum's
/// `.route("/users", get(list).post(create))` with `.nest("/api", …)` and
/// `.merge(…)`; actix-web's `.route("/x", web::get().to(h))`,
/// `web::resource("/x").route(web::post().to(h))` and `.to(h)`, and
/// `web::scope("/api")`. `nested` collects what is nested under a prefix
/// (`.nest("/api", api::router())`), so routes built elsewhere get it too.
pub fn chain_routes<'e>(e: &'e Expr, prefix: &str, out: &mut Vec<Registered<'e>>, nested: &mut Vec<(String, &'e Expr)>) {
    let Expr::Call(c) = e else { return };
    let Expr::Attribute(a) = &*c.func else { return };
    let args = &c.arguments.args[..];
    chain_routes(&a.value, prefix, out, nested);
    let here = match scope_path(&a.value) {
        Some(p) => join_path(prefix, p),
        None => prefix.to_string(),
    };
    match (a.attr.as_str(), args) {
        ("route" | "at", [path, router]) if string_literal(path).is_some() => method_router(router, &join_path(&here, string_literal(path).unwrap()), out),
        ("route", [router]) => {
            if let Some(p) = resource_path(&a.value) {
                method_router(router, &join_path(&here, p), out);
            }
        }
        ("to", [handler]) => {
            if let Some(p) = resource_path(&a.value) {
                out.push(Registered { verb: "ANY".into(), path: join_path(&here, p), handler });
            }
        }
        ("nest" | "nest_service", [path, inner]) if string_literal(path).is_some() => {
            let p = join_path(&here, string_literal(path).unwrap());
            chain_routes(inner, &p, out, nested);
            nested.push((p, inner));
        }
        ("merge", [inner]) => {
            chain_routes(inner, &here, out, nested);
            nested.push((here.clone(), inner));
        }
        ("service" | "configure", [inner]) => chain_routes(inner, &here, out, nested),
        _ => {}
    }
}

fn verb_name(name: &str) -> Option<String> {
    // `post_service(…)` routes a service the way `post(…)` routes a handler.
    let lower = name.trim_end_matches("_service").to_ascii_lowercase();
    (HTTP.contains(&lower.as_str()) || lower == "any").then(|| lower.to_uppercase())
}

/// The handler a route is given, under the layers wrapped around it
/// (`kv_get.layer(…)`, `handler.with_state(…)`).
pub fn handler_of(e: &Expr) -> &Expr {
    let mut cur = e;
    while let Expr::Call(c) = cur {
        match &*c.func {
            Expr::Attribute(a) if matches!(a.attr.as_str(), "layer" | "with_state" | "route_layer" | "into_service") => cur = &a.value,
            _ => break,
        }
    }
    cur
}

/// `get(list).post(create)`, `web::get().to(h)`, `post(h)`.
fn method_router<'e>(e: &'e Expr, path: &str, out: &mut Vec<Registered<'e>>) {
    let Expr::Call(c) = e else { return };
    let args = &c.arguments.args[..];
    match (&*c.func, args) {
        (Expr::Name(n), [handler]) => {
            if let Some(v) = verb_name(&n.id) {
                out.push(Registered { verb: v, path: path.to_string(), handler });
            }
        }
        (Expr::Attribute(a), [handler]) if a.attr == "to" => {
            let verb = match &*a.value {
                Expr::Call(inner) if inner.arguments.args.is_empty() => verb_name(last_segment(&inner.func)),
                _ => None,
            };
            out.push(Registered { verb: verb.unwrap_or_else(|| "ANY".into()), path: path.to_string(), handler });
        }
        (Expr::Attribute(a), [handler]) => {
            if let Some(v) = verb_name(&a.attr) {
                // `routing::get(h)` is a call of the verb; `get(a).post(b)` chains them.
                if !matches!(&*a.value, Expr::Call(_)) {
                    out.push(Registered { verb: v, path: path.to_string(), handler });
                    return;
                }
                method_router(&a.value, path, out);
                out.push(Registered { verb: v, path: path.to_string(), handler });
            }
        }
        _ => {}
    }
}

/// The path of an actix-web resource a chain starts from: `web::resource("/x")`.
fn resource_path(e: &Expr) -> Option<&str> {
    match e {
        Expr::Call(c) if last_segment(&c.func) == "resource" => c.arguments.args.first().and_then(string_literal),
        Expr::Call(c) => match &*c.func {
            Expr::Attribute(a) => resource_path(&a.value),
            _ => None,
        },
        _ => None,
    }
}

/// The prefix of an actix-web scope a chain starts from: `web::scope("/api")`.
fn scope_path(e: &Expr) -> Option<&str> {
    match e {
        Expr::Call(c) if last_segment(&c.func) == "scope" => c.arguments.args.first().and_then(string_literal),
        Expr::Call(c) => match &*c.func {
            Expr::Attribute(a) => scope_path(&a.value),
            _ => None,
        },
        _ => None,
    }
}

/// Calls that start a method chain, anywhere in a body, closures included
/// (`HttpServer::new(|| App::new().route(…))`).
pub fn chain_tops<'e>(stmts: &'e [Stmt], f: &mut impl FnMut(&'e Expr)) {
    fn visit<'e>(e: &'e Expr, receiver: bool, f: &mut impl FnMut(&'e Expr)) {
        if !receiver && matches!(e, Expr::Call(_)) {
            f(e);
        }
        match e {
            Expr::Call(c) => {
                match &*c.func {
                    Expr::Attribute(a) => visit(&a.value, true, f),
                    other => visit(other, false, f),
                }
                for a in c.arguments.args.iter().chain(c.arguments.keywords.iter().map(|k| &k.value)) {
                    visit(a, false, f);
                }
            }
            Expr::Function(d) => chain_tops(&d.body, f),
            other => {
                let mut kids = vec![];
                super::walk::children(other, &mut kids);
                for k in kids {
                    visit(k, false, f);
                }
            }
        }
    }
    super::walk::stmt_exprs(stmts, true, &mut |root| visit(root, false, f));
}

/// Routes registered by router builders across the project: the label of
/// each handler function (`GET /api/articles`), and the prefix each
/// router-building function is nested under.
pub struct Routes {
    pub handlers: HashMap<usize, String>,
    pub prefixes: HashMap<usize, String>,
}

pub fn routes(modules: &[super::Module], ix: &super::index::Index) -> Routes {
    use super::index::{NoLocals, Ty};
    // (function, nested prefix, nested function) and (function, verb, path, handler).
    let mut nests: Vec<(usize, String, usize)> = vec![];
    let mut regs: Vec<(usize, String, String, usize)> = vec![];
    for (fid, f) in ix.funcs.iter().enumerate() {
        let mi = f.module;
        if modules[mi].lang != ast::Lang::Rust {
            continue;
        }
        let func_of = |e: &Expr| -> Option<usize> {
            let mut target = handler_of(e);
            if let Expr::Call(c) = target {
                target = &c.func;
            }
            match ix.infer(mi, target, &NoLocals) {
                Ty::Func(g) => Some(g),
                _ => None,
            }
        };
        chain_tops(f.def.body, &mut |e| {
            let mut out = vec![];
            let mut nested = vec![];
            chain_routes(e, "", &mut out, &mut nested);
            for r in out {
                if let Some(h) = func_of(r.handler) {
                    // A registration inside `.service(…)` is also met on its own.
                    let reg = (fid, r.verb, r.path, h);
                    if !regs.contains(&reg) {
                        regs.push(reg);
                    }
                }
            }
            for (p, inner) in nested {
                if let Some(g) = func_of(inner) {
                    nests.push((fid, p, g));
                }
            }
        });
    }
    let mut prefixes: HashMap<usize, String> = HashMap::new();
    for _ in 0..4 {
        for (from, p, to) in &nests {
            let parent = prefixes.get(from).cloned().unwrap_or_default();
            prefixes.insert(*to, join_path(&parent, p));
        }
    }
    let mut handlers: HashMap<usize, String> = HashMap::new();
    for (fid, verb, path, h) in regs {
        let full = join_path(prefixes.get(&fid).map(String::as_str).unwrap_or(""), &path);
        let label = format!("{verb} {full}");
        handlers.entry(h).and_modify(|l| *l = format!("{l}, {label}")).or_insert(label);
    }
    Routes { handlers, prefixes }
}

/// A short tag for decorators without a framework role, or `None` for
/// decorators that are plumbing (`@functools.wraps(f)`, `@override`).
pub fn tag(d: &ast::Decorator, lang: ast::Lang, text: impl Fn(&Expr) -> String) -> Option<String> {
    let func = match &d.expression {
        Expr::Call(c) => &*c.func,
        e => e,
    };
    if lang == ast::Lang::Rust {
        return rust_tag(d, func);
    }
    // Any expression is a decorator since 3.9 (`@buttons[0].cache`).
    let name = dotted(func).unwrap_or_else(|| text(func));
    let last = name.rsplit('.').next().unwrap_or(&name);
    Some(
        match last {
            "wraps" | "override" | "final" | "staticmethod" | "classmethod" | "property" | "setter" | "getter" | "deleter" | "no_type_check" | "total_ordering" | "unique" | "runtime_checkable" => return None,
            "contextmanager" | "asynccontextmanager" => "context manager",
            "lru_cache" | "cache" | "cached" | "memoize" => "cached",
            "abstractmethod" | "abstractproperty" => "abstract",
            "overload" => "overload",
            "no_grad" | "inference_mode" => "no grad",
            "compile" | "jit" | "script" | "njit" => "compiled",
            "fixture" => "fixture",
            "parametrize" => "parametrized",
            "dataclass" => "dataclass",
            _ => return Some(format!("@{name}")),
        }
        .to_string(),
    )
}
