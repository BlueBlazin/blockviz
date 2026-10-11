//! What framework decorators mean to a reader: an HTTP route, a CLI command, a
//! background task, an event handler. Recognised by shape (`@<router>.get("/path")`)
//! rather than by import, so one rule covers FastAPI, Flask, Starlette, Quart,
//! Sanic, Litestar, blueprints and routers alike.

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
}

const HTTP: &[&str] = &["get", "post", "put", "patch", "delete", "head", "options", "trace"];

/// Constructors whose instance is the application a server or CLI runs.
pub const APPS: &[&str] = &["FastAPI", "Flask", "Starlette", "Quart", "Sanic", "Litestar", "Celery", "Typer", "get_asgi_application", "get_wsgi_application", "Application", "Bottle", "Falcon", "App"];

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

fn join_path(prefix: &str, path: &str) -> String {
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
pub fn role(d: &Expr, function: &str, prefixes: &HashMap<String, String>, text: impl Fn(&Expr) -> String) -> Option<Role> {
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

/// A short tag for decorators without a framework role, or `None` for
/// decorators that are plumbing (`@functools.wraps(f)`, `@override`).
pub fn tag(d: &ast::Decorator, text: impl Fn(&Expr) -> String) -> Option<String> {
    let func = match &d.expression {
        Expr::Call(c) => &*c.func,
        e => e,
    };
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
