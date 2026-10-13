//! Robustness: unusual Python must never panic and should stay readable.

use blockviz_core::project::InFile;

fn analyze(src: &str) -> blockviz_core::ir::Scene {
    blockviz_core::analyze("t", vec![InFile { path: "t.py".into(), text: Some(src.into()), bytes: src.len() as u64 }])
}

#[test]
fn modern_and_odd_constructs() {
    let src = r#"
import asyncio
from dataclasses import dataclass

def deco(f):
    return f

@deco
async def fetch(session, url, *args, retries: int = 3, **kw):
    async with session.get(url) as resp:
        if (n := len(args)) > 2:
            return await resp.text()
        async for chunk in resp.content:
            yield chunk
    try:
        match url:
            case "a" | "b":
                pass
            case {"k": v}:
                raise ValueError(v)
            case _:
                ...
    except* OSError:
        pass
    finally:
        print("done")

class Outer:
    class Inner:
        x: int = 1
    def method(self, f=lambda y: y + 1):
        global G
        G = [i * j for i in range(3) for j in range(i) if j]
        return {k: v for k, v in zip("ab", G)}, (yield from range(3))

def recurse(n):
    return recurse(n - 1) if n else 0

def star(*a, **k):
    x, *rest = a
    del rest
    return x
"#;
    let s = analyze(src);
    assert!(s.nodes.len() > 5);
    let json = serde_json::to_string(&s).unwrap();
    assert!(json.len() > 100);
}

#[test]
fn recursion_is_bounded() {
    let s = analyze("def f(n):\n    return f(n - 1) + g(n)\n\ndef g(n):\n    return f(n)\n");
    // Mutually recursive calls reference each other without infinite nesting.
    assert!(s.nodes.iter().any(|n| n.title == "f"));
    assert!(s.stats.max_depth < 10);
}

#[test]
fn syntax_errors_are_diagnostics_not_failures() {
    let s = analyze("def broken(:\n    pass\n");
    assert!(s.diagnostics.iter().any(|d| d.contains("Syntax error")));
}

#[test]
fn empty_and_whitespace_projects() {
    let s = analyze("");
    assert!(!s.nodes.is_empty());
    let s = blockviz_core::analyze("empty", vec![]);
    assert_eq!(s.nodes.len(), 1);
}

#[test]
fn deep_nesting_does_not_overflow() {
    let mut src = String::new();
    for i in 0..60 {
        src.push_str(&"    ".repeat(i));
        src.push_str(&format!("if x{i}:\n"));
    }
    src.push_str(&"    ".repeat(60));
    src.push_str("y = f(x)\n");
    let s = analyze(&src);
    assert!(s.nodes.len() > 1);
}

#[test]
fn pathological_nesting_is_skipped_not_crashed() {
    let src = format!("x = {}1{}\n", "(".repeat(400), ")".repeat(400));
    let s = analyze(&src);
    assert!(s.diagnostics.iter().any(|d| d.contains("nest")));
}

#[test]
fn early_return_is_an_if_else() {
    // `if c: return a` then more code: the rest only runs otherwise.
    let s = analyze(
        r#"
import os

def open_resource(root, resource, mode="rb", encoding="utf-8"):
    path = os.path.join(root, resource)
    if mode == "rb":
        return open(path, mode)  # pyright: ignore
    return open(path, mode, encoding=encoding)
"#,
    );
    let f = s.nodes.iter().find(|n| n.title == "open_resource").unwrap();
    let branch = f.children.iter().map(|&c| &s.nodes[c as usize]).find(|n| n.kind == blockviz_core::ir::Kind::Branch).expect("a branch");
    let lanes: Vec<&blockviz_core::ir::Node> = branch.children.iter().map(|&c| &s.nodes[c as usize]).filter(|n| n.kind == blockviz_core::ir::Kind::Lane).collect();
    assert_eq!(lanes.iter().map(|l| l.title.as_str()).collect::<Vec<_>>(), ["if mode == \"rb\"", "else"]);
    let opens: Vec<&str> = lanes.iter().flat_map(|l| l.children.iter().map(|&c| &s.nodes[c as usize])).filter(|n| n.title == "open").map(|n| n.subtitle.as_str()).collect();
    assert_eq!(opens, ["path, mode", "path, mode, encoding=encoding"], "arguments tell the calls apart");
    assert!(s.nodes.iter().all(|n| !n.doc.contains("pyright")), "tool directives are not documentation");
    assert!(s.nodes.iter().any(|n| n.title == "path" && n.subtitle == "os.path.join"), "a value is named by its name, with the call that makes it");
}

fn kids<'a>(s: &'a blockviz_core::ir::Scene, n: &blockviz_core::ir::Node) -> Vec<&'a blockviz_core::ir::Node> {
    n.children.iter().map(|&c| &s.nodes[c as usize]).filter(|c| !c.is_port()).collect()
}

fn descendants<'a>(s: &'a blockviz_core::ir::Scene, n: &'a blockviz_core::ir::Node) -> Vec<&'a blockviz_core::ir::Node> {
    let mut out = vec![];
    let mut stack: Vec<u32> = n.children.clone();
    while let Some(id) = stack.pop() {
        let c = &s.nodes[id as usize];
        out.push(c);
        stack.extend(c.children.iter().copied());
    }
    out
}

#[test]
fn every_return_is_visible() {
    // Flask's `get_send_file_max_age`: three ways to produce a result, none of
    // which calls anything a block would be made for.
    let s = analyze(
        r#"
from datetime import timedelta
from flask import current_app

def get_send_file_max_age(filename):
    value = current_app.config["SEND_FILE_MAX_AGE_DEFAULT"]
    if value is None:
        return None
    if isinstance(value, timedelta):
        return int(value.total_seconds())
    return value  # type: ignore[no-any-return]
"#,
    );
    let f = s.nodes.iter().find(|n| n.title == "get_send_file_max_age").unwrap();
    let steps = kids(&s, f);
    // Guarded exits are steps tagged with their condition, not nested empty lanes.
    assert!(steps.iter().all(|n| n.kind != blockviz_core::ir::Kind::Branch), "{:?}", steps.iter().map(|n| &n.title).collect::<Vec<_>>());
    let exits: Vec<&&blockviz_core::ir::Node> = steps.iter().filter(|n| n.title.starts_with("return")).collect();
    assert_eq!(exits.len(), 2, "{:?}", steps.iter().map(|n| (&n.title, &n.subtitle)).collect::<Vec<_>>());
    assert!(exits.iter().any(|n| n.title == "return None" && n.badges == ["if value is None"]));
    assert!(exits.iter().any(|n| n.subtitle == "int(value.total_seconds())" && n.badges == ["if isinstance(value, timedelta)"]));
    // The config lookup is a step, and every result reaches the return port.
    let lookup = steps.iter().find(|n| n.title == "value").expect("value lookup");
    let port = f.children.iter().map(|&c| &s.nodes[c as usize]).find(|n| n.kind == blockviz_core::ir::Kind::Output).expect("return port");
    let into_port: Vec<u32> = f.edges.iter().filter(|e| e.to == port.id).map(|e| e.from).collect();
    assert!(into_port.contains(&lookup.id));
    for e in &exits {
        assert!(into_port.contains(&e.id), "{} reaches the return port", e.title);
    }
}

#[test]
fn branch_arms_are_never_empty() {
    let s = analyze(
        r#"
def register(username, password):
    error = None
    if not username:
        error = "Username is required."
    elif not password:
        error = "Password is required."
    if error is None:
        save(username, password)
    return error

def sign(x):
    if x > 0:
        return 1
    else:
        return -1
"#,
    );
    for f in ["register", "sign"] {
        let f = s.nodes.iter().find(|n| n.title == f).unwrap();
        for lane in descendants(&s, f).into_iter().filter(|n| n.kind == blockviz_core::ir::Kind::Lane) {
            assert!(!kids(&s, lane).is_empty(), "lane {} in {} is empty", lane.title, f.title);
        }
    }
}

#[test]
fn arrows_say_what_a_value_is_to_the_step() {
    use blockviz_core::ir::EdgeKind;
    let s = analyze(
        r#"
from app.io import load, store, render

def f(path, limit):
    items = load(path)
    n = len(items)
    total = int(items)
    if limit:
        store(items)
    render(n, total)
"#,
    );
    let f = s.nodes.iter().find(|n| n.title == "f").unwrap();
    let kind = |from: &str, to: &str| {
        let id = |t: &str| f.children.iter().copied().find(|&c| s.nodes[c as usize].title == t).unwrap_or_else(|| panic!("no {t}"));
        f.edges.iter().find(|e| e.from == id(from) && e.to == id(to)).map(|e| e.kind)
    };
    // Every value is data, whatever its name suggests (a length, a conversion).
    assert_eq!(kind("n", "render"), Some(EdgeKind::Data));
    assert_eq!(kind("total", "render"), Some(EdgeKind::Data));
    // The condition decides whether `store` runs; `items` is also its data.
    assert_eq!(kind("limit", "store"), Some(EdgeKind::Control));
    assert_eq!(kind("load", "store"), Some(EdgeKind::Data));
}

#[test]
fn calls_through_an_abstract_class_reach_its_subclasses() {
    let s = analyze(
        r#"
from abc import ABC, abstractmethod
import sqlite3


class Store(ABC):
    @abstractmethod
    def get(self, pid: int) -> str:
        """The product's name."""

    def describe(self, pid: int) -> str:
        return f"product {self.get(pid)}"


class SqlStore(Store):
    def __init__(self, path: str):
        self.conn = sqlite3.connect(path)

    def get(self, pid: int) -> str:
        return self.conn.execute("SELECT name FROM products WHERE id = ?", (pid,)).fetchone()[0]


class Cache(ABC):
    @abstractmethod
    def lookup(self, key: str) -> str:
        raise NotImplementedError


class MemoryCache(Cache):
    def __init__(self):
        self.items = {}

    def lookup(self, key: str) -> str:
        return self.items.get(key, "").lower()


class RedisCache(Cache):
    def __init__(self, url: str):
        self.url = url

    def lookup(self, key: str) -> str:
        return fetch(self.url, key)


class FakeCache(Cache):
    def lookup(self, key: str) -> str:
        return key.upper()


def fetch(url, key):
    return url + key


def product(store: Store, cache: Cache, pid: int) -> str:
    hit = cache.lookup(str(pid))
    if hit:
        return hit
    return store.get(pid)


def main():
    print(product(SqlStore("shop.db"), RedisCache("redis://") if fetch("a", "b") else MemoryCache(), 1))
"#,
    );
    use blockviz_core::ir::Kind;
    let node = |parent: &str, title: &str| {
        s.nodes.iter().find(|n| n.kind == Kind::Function && n.title == title && n.parent.is_some_and(|p| s.nodes[p as usize].title == parent)).unwrap_or_else(|| panic!("no {parent}.{title}"))
    };
    let calls_in = |f: &str, title: &str| -> Vec<&blockviz_core::ir::Node> {
        let root = s.nodes.iter().find(|n| n.kind == Kind::Function && n.title == f).unwrap();
        let mut out = vec![];
        let mut stack: Vec<u32> = root.children.clone();
        while let Some(c) = stack.pop() {
            let n = &s.nodes[c as usize];
            if n.kind == Kind::Call && n.title == title {
                out.push(n);
            }
            stack.extend(n.children.iter().copied());
        }
        out
    };
    // The one store there is: the abstract method's subclass, from callers
    // and from the base class's own `describe`.
    for f in ["product", "describe"] {
        let get = calls_in(f, "get");
        assert_eq!((get[0].subtitle.as_str(), get[0].reference), ("SqlStore", Some(node("SqlStore", "get").id)), "{f}");
    }
    // Two caches (the fake aside): named, leading to the abstract method.
    let lookup = calls_in("product", "lookup");
    assert_eq!((lookup[0].subtitle.as_str(), lookup[0].reference), ("MemoryCache or RedisCache", Some(node("Cache", "lookup").id)));
}

#[test]
fn with_as_binds_what_entering_returns() {
    let s = analyze(
        r#"
import tempfile


class Response:
    def json(self):
        return {}


class Session:
    def __exit__(self, *args):
        self.close()

    def request(self, method, url):
        return Response()

    def close(self):
        pass


class Pool:
    def __enter__(self):
        return self.connect()

    def __exit__(self, *args):
        pass

    def connect(self):
        return Session()


class AsyncClient:
    async def __aenter__(self):
        return self

    async def send(self, url):
        return Response()


def request(method, url):
    with Session() as session:
        return session.request(method, url)


def pooled(url):
    with Pool() as conn:
        return conn.request("get", url)


async def fetch(url):
    async with AsyncClient() as client:
        return await client.send(url)


def scratch():
    with tempfile.TemporaryDirectory() as d:
        return d.join("x")


def main():
    r = request("get", "/")
    return r.json()
"#,
    );
    use blockviz_core::ir::Kind;
    let def = |class: &str, title: &str| {
        s.nodes.iter().find(|n| n.kind == Kind::Function && n.title == title && n.parent.is_some_and(|p| s.nodes[p as usize].title == class)).unwrap_or_else(|| panic!("no {class}.{title}")).id
    };
    let call = |f: &str, title: &str| {
        let root = s.nodes.iter().find(|n| n.kind == Kind::Function && n.title == f && n.parent.is_some_and(|p| s.nodes[p as usize].kind == Kind::File)).unwrap();
        let mut stack: Vec<u32> = root.children.clone();
        while let Some(c) = stack.pop() {
            let n = &s.nodes[c as usize];
            if n.title == title && matches!(n.kind, Kind::Call | Kind::Op) {
                return n;
            }
            stack.extend(n.children.iter().copied());
        }
        panic!("no {title} in {f}")
    };
    // A class without its own `__enter__` is entered as itself.
    assert_eq!(call("request", "request").reference, Some(def("Session", "request")));
    // Its `__enter__` (or `__aenter__`) says what is entered.
    assert_eq!(call("pooled", "request").reference, Some(def("Session", "request")));
    assert_eq!(call("fetch", "send").reference, Some(def("AsyncClient", "send")));
    // What comes back out of a `with` is typed too.
    assert_eq!(call("main", "json").reference, Some(def("Response", "json")));
    // A library's context manager is not assumed to enter as itself.
    assert_eq!(call("scratch", "join").reference, None);
}

/// The operators a function's dataflow passes through, in source order.
fn operators(s: &blockviz_core::ir::Scene, f: &str) -> Vec<(String, usize)> {
    let f = s.nodes.iter().find(|n| n.title == f && n.kind == blockviz_core::ir::Kind::Function).unwrap();
    let mut out: Vec<_> = descendants(s, f).into_iter().filter(|n| n.kind == blockviz_core::ir::Kind::Glyph).map(|g| {
        let parent = &s.nodes[g.parent.unwrap() as usize];
        (g.title.clone(), parent.edges.iter().filter(|e| e.to == g.id).count(), g.span.as_ref().map_or(0, |sp| sp.start))
    }).collect();
    out.sort_by_key(|o| o.2);
    out.into_iter().map(|(t, n, _)| (t, n)).collect()
}

#[test]
fn values_combined_by_an_operator_are_the_operator() {
    let s = analyze(
        r#"
from app.io import load

def f(p, q, r, obj):
    a = load(p)
    b = load(q)
    c = load(r)
    s = a + b
    t = a + b + c
    u = a * 2
    v = a % b
    w = a + a
    obj.k = a - b
    x = a @ (b * c)
    return s, t, u, v, w, x
"#,
    );
    // Two values (`a + b`, `a % b`), or a chain of them (`a + b + c`, one sum of
    // three), are an operator; a constant (`a * 2`), a value with itself, a
    // store into a field and a formula operand (`b * c`) are formula steps.
    assert_eq!(operators(&s, "f"), vec![("+".into(), 2), ("+".into(), 3), ("%".into(), 2)]);
}

/// The stops in a function, each as (the names on its line, the block it hangs under).
fn stops<'a>(s: &'a blockviz_core::ir::Scene, f: &'a blockviz_core::ir::Node) -> Vec<(String, &'a blockviz_core::ir::Node)> {
    let mut out = vec![];
    for n in std::iter::once(f).chain(descendants(s, f)) {
        for e in n.edges.iter().filter(|e| s.nodes[e.to as usize].kind == blockviz_core::ir::Kind::Unused) {
            out.push((e.label.clone(), &s.nodes[e.from as usize]));
        }
    }
    out
}

#[test]
fn unread_values_end_in_a_stop() {
    let s = analyze(
        r#"
import requests

COUNT = 0

def params(d_model, vocab_size, n):
    embeddings = d_model * vocab_size
    dense = d_model * vocab_size * n
    return dense

def kept(xs, url, session):
    total = 0
    for x in xs:
        total = total + session.get(x)
    timer = None
    def cancel():
        return timer
    timer = requests.get(url)
    _ignored = requests.get(url)
    shown = requests.head(url)
    print(shown)
    try:
        got = requests.get(url)
    except OSError:
        return None
    else:
        return got, total, cancel

def bump(session, x):
    global COUNT
    rows = session.query(x)
    COUNT = len(rows) * 2
"#,
    );
    let f = s.nodes.iter().find(|n| n.title == "params").unwrap();
    let found = stops(&s, f);
    assert_eq!(found.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(), ["embeddings"]);
    // The line drops straight from the middle of the block to the stop, with no arrowhead.
    let block = found[0].1;
    let e = f.edges.iter().find(|e| e.from == block.id).unwrap();
    let stop = &s.nodes[e.to as usize];
    let x = block.x + block.w / 2.;
    assert!(e.open);
    assert!((e.points[0] - x).abs() < 0.1 && (e.points[1] - (block.y + block.h)).abs() < 0.1, "{:?}", e.points);
    assert!((e.points[2] - x).abs() < 0.1 && (e.points[3] - stop.y).abs() < 0.1, "{:?}", e.points);
    assert!((stop.x + stop.w / 2. - x).abs() < 0.1);
    // Read later (in a loop, by a closure written above, in `else`), left
    // unread on purpose, printed, or a global: none of these is unused.
    for name in ["kept", "bump"] {
        let f = s.nodes.iter().find(|n| n.title == name).unwrap();
        assert!(stops(&s, f).is_empty(), "{name}: {:?}", stops(&s, f).iter().map(|(l, _)| l).collect::<Vec<_>>());
    }
}

#[test]
fn values_read_where_no_arrow_shows_it_are_not_unused() {
    // Each of these values is read, though no later statement of its list
    // reads it: by code that names no locals, in place, by a clause of its
    // own statement that runs after it, or as the value an alias names.
    let s = analyze(
        r#"
import requests

def dynamic(url, render):
    page = requests.get(url)
    return render("t.html", **locals())

def evaluated(url):
    page = requests.get(url)
    return eval("page.status")

def in_place(url, acc):
    items = acc
    items += requests.get(url)

def handler_then_finally(url, log):
    try:
        resp = requests.get(url)
    except OSError:
        fallback = requests.head(url)
    finally:
        log(fallback)
    return resp

def else_then_finally(url, log):
    try:
        resp = requests.get(url)
    except OSError:
        return requests.head(url)
    else:
        parsed = requests.post(resp)
    finally:
        log(parsed)

def for_else(urls, log):
    for u in urls:
        last = requests.get(u)
    else:
        log(last)

def while_else(url, log, n):
    while n:
        last = requests.get(url)
        n -= 1
    else:
        log(last)

def alias_in_loop(xs, session):
    for x in xs:
        s = session
        requests.get(x)
    return xs

def deleted(url):
    page = requests.get(url)
    del page
    return requests.head(url)
"#,
    );
    for name in ["dynamic", "evaluated", "in_place", "handler_then_finally", "else_then_finally", "for_else", "while_else", "alias_in_loop"] {
        let f = s.nodes.iter().find(|n| n.title == name).unwrap();
        assert!(stops(&s, f).is_empty(), "{name}: {:?}", stops(&s, f).iter().map(|(l, b)| (l, &b.title)).collect::<Vec<_>>());
    }
    // Deleting a value is not reading it.
    let f = s.nodes.iter().find(|n| n.title == "deleted").unwrap();
    assert_eq!(stops(&s, f).iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(), ["page"]);
}
