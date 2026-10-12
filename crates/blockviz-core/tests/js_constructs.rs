//! JavaScript and TypeScript constructs read the way a developer would draw
//! them: the same rules as Python (every exit visible, no empty branch arms,
//! arrows that say what a value is to a step), plus what JavaScript adds:
//! promise chains, handlers, routes registered with inline functions.

use blockviz_core::ir::{EdgeKind, Kind, Node, Scene};
use blockviz_core::project::InFile;

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
    kids(s, n).into_iter().find(|k| k.title == title).unwrap_or_else(|| panic!("no {title:?} in {}: {:?}", n.title, kids(s, n).iter().map(|k| &k.title).collect::<Vec<_>>()))
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

fn edge(n: &Node, from: &Node, to: &Node) -> Option<EdgeKind> {
    n.edges.iter().find(|e| e.from == from.id && e.to == to.id).map(|e| e.kind)
}

fn return_port<'a>(s: &'a Scene, f: &Node) -> &'a Node {
    f.children.iter().map(|&c| &s.nodes[c as usize]).find(|n| n.kind == Kind::Output).expect("return port")
}

#[test]
fn every_exit_is_visible() {
    let s = analyze(&[(
        "max_age.js",
        r#"
import { Duration } from 'luxon';

export function maxAge(filename) {
  const value = process.env.MAX_AGE;
  if (value == null) return null;
  if (value instanceof Duration) return Math.floor(value.as('seconds'));
  return value;
}
"#,
    )]);
    let f = find(&s, "maxAge");
    let steps = kids(&s, f);
    assert!(steps.iter().all(|n| n.kind != Kind::Branch), "{:?}", steps.iter().map(|n| &n.title).collect::<Vec<_>>());
    let exits: Vec<&&Node> = steps.iter().filter(|n| n.title.starts_with("return")).collect();
    assert!(exits.iter().any(|n| n.title == "return null" && n.badges == ["if value == null"]), "{exits:?}");
    assert!(exits.iter().any(|n| n.subtitle == "Math.floor(value.as('seconds'))" && n.badges == ["if value instanceof Duration"]), "{exits:?}");
    // Reading the environment is where the value comes from, and every result
    // reaches the return port.
    let lookup = kid(&s, f, "value");
    assert!(lookup.effects.contains(&"reads environment".to_string()));
    let port = return_port(&s, f);
    let into_port: Vec<u32> = f.edges.iter().filter(|e| e.to == port.id).map(|e| e.from).collect();
    assert!(into_port.contains(&lookup.id));
    for e in &exits {
        assert!(into_port.contains(&e.id), "{} reaches the return port", e.title);
    }
}

#[test]
fn branch_arms_are_never_empty() {
    let s = analyze(&[(
        "auth.js",
        r#"
import { save } from './db.js';

export function register(username, password) {
  let error = null;
  if (!username) {
    error = 'Username is required.';
  } else if (!password) {
    error = 'Password is required.';
  }
  if (error === null) {
    save(username, password);
  }
  return error;
}

export const sign = (x) => {
  if (x > 0) {
    return 1;
  } else {
    return -1;
  }
};

export function kind(token) {
  switch (token.type) {
    case 'num':
    case 'int':
      return 'number';
    case 'str':
      return 'string';
    default:
      throw new TypeError(token.type);
  }
}
"#,
    ), ("db.js", "export function save(u, p) { return fetch('/save', { method: 'POST', body: JSON.stringify({ u, p }) }); }\n")]);
    for f in ["register", "sign", "kind"] {
        let f = find(&s, f);
        let lanes: Vec<&Node> = descendants(&s, f).into_iter().filter(|n| n.kind == Kind::Lane).collect();
        assert!(!lanes.is_empty(), "{} has lanes", f.title);
        for lane in lanes {
            assert!(!kids(&s, lane).is_empty(), "lane {} in {} is empty", lane.title, f.title);
        }
    }
    let lanes: Vec<String> = descendants(&s, find(&s, "kind")).into_iter().filter(|n| n.kind == Kind::Lane).map(|n| n.title.clone()).collect();
    assert_eq!(lanes.len(), 3, "{lanes:?}");
    assert!(lanes.contains(&"case 'num', 'int'".to_string()) && lanes.contains(&"default".to_string()), "{lanes:?}");
    let save = descendants(&s, find(&s, "register")).into_iter().find(|n| n.title == "save").expect("save");
    assert_eq!(save.badges, ["if error === null"]);
}

#[test]
fn arrows_say_what_a_value_is_to_the_step() {
    let s = analyze(&[(
        "f.js",
        r#"
import { load, store, render } from './io.js';

export function f(path, limit) {
  const items = load(path);
  const n = items.length;
  const total = Number(items);
  if (limit) store(items);
  render(n, total);
}
"#,
    ), ("io.js", "export function load(p) { return JSON.parse(p); }\nexport function store(x) { console.log(x); }\nexport function render(a, b) { console.log(a, b); }\n")]);
    let f = find(&s, "f");
    let (load, total, store, render) = (kid(&s, f, "load"), kid(&s, f, "total"), kid(&s, f, "store"), kid(&s, f, "render"));
    let limit = f.children.iter().map(|&c| &s.nodes[c as usize]).find(|n| n.title == "limit").unwrap();
    // A size is metadata, drawn lightly downstream; a converted value is still data.
    assert_eq!(edge(f, load, render), Some(EdgeKind::Context));
    assert_eq!(edge(f, total, render), Some(EdgeKind::Data));
    // The condition decides whether `store` runs; `items` is also its data.
    assert_eq!(edge(f, limit, store), Some(EdgeKind::Control));
    assert_eq!(edge(f, load, store), Some(EdgeKind::Data));
}

#[test]
fn promise_chains_pass_values_on() {
    let s = analyze(&[(
        "poll.js",
        r#"
export function poll(url, onData) {
  return fetch(url)
    .then((r) => r.json())
    .then((data) => onData(data))
    .catch((err) => console.error(err));
}
"#,
    )]);
    let f = find(&s, "poll");
    let steps = kids(&s, f);
    // The chain is not a step: fetch's result reaches the callback, whose result is returned.
    assert_eq!(steps.iter().map(|n| n.title.as_str()).collect::<Vec<_>>(), ["fetch", "onData"]);
    let (fetch, on_data) = (kid(&s, f, "fetch"), kid(&s, f, "onData"));
    assert!(fetch.effects.contains(&"network".to_string()));
    assert_eq!(edge(f, fetch, on_data), Some(EdgeKind::Data));
    assert_eq!(edge(f, on_data, return_port(&s, f)), Some(EdgeKind::Data));
}

#[test]
fn handlers_say_when_they_run() {
    let s = analyze(&[(
        "list.jsx",
        r#"
import { useState } from 'react';
import { save, track } from './api';

export function List({ items, onPick }) {
  const [open, setOpen] = useState(false);
  return (
    <ul>
      {items.map((item) => (
        <Row key={item.id} item={item} onClick={() => onPick(item)} />
      ))}
      <button onClick={() => save(items)}>Save</button>
    </ul>
  );
}

function Row({ item, onClick }) {
  return <li onClick={onClick}>{item.name}</li>;
}

export function wire(button) {
  button.addEventListener('click', () => {
    track('clicked');
  });
}
"#,
    ), ("api.js", "export const save = (x) => fetch('/save', { method: 'POST', body: JSON.stringify(x) });\nexport const track = (e) => navigator.sendBeacon('/t', e);\n")]);
    let list = find(&s, "List");
    assert_eq!(list.category, "component");
    let inside = descendants(&s, list);
    let titled = |t: &str| *inside.iter().find(|n| n.title == t).unwrap_or_else(|| panic!("no {t:?} in List"));
    // Per item, or on an event; a handler inside a list still runs on its event.
    assert!(titled("Row").badges.contains(&"each".to_string()));
    assert_eq!(titled("onPick").badges, ["on click"]);
    assert_eq!(titled("save").badges, ["on click"]);
    // A listener registered by name hosts its function.
    let wire = find(&s, "wire");
    let click = kid(&s, wire, "click");
    assert!(descendants(&s, click).iter().any(|n| n.title == "track"), "the listener is drawn inside its registration");
}

#[test]
fn try_catch_finally_lanes() {
    let s = analyze(&[(
        "user.js",
        r#"
export async function loadUser(id, { retries = 3, signal } = {}) {
  try {
    const res = await fetch(`/api/users/${id}`, { signal });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    return await res.json();
  } catch (err) {
    console.error(err);
    return null;
  } finally {
    signal?.removeEventListener?.('abort', onAbort);
  }
}
function onAbort() {}
"#,
    )]);
    let f = find(&s, "loadUser");
    assert_eq!(f.inputs, ["id", "retries", "signal"]);
    assert!(f.badges.contains(&"async".to_string()));
    let lanes: Vec<&Node> = descendants(&s, f).into_iter().filter(|n| n.kind == Kind::Lane).collect();
    let mut titles: Vec<&str> = lanes.iter().map(|n| n.title.as_str()).collect();
    titles.sort();
    assert_eq!(titles, ["catch", "finally", "try"]);
    for lane in lanes {
        assert!(!kids(&s, lane).is_empty(), "lane {} is empty", lane.title);
    }
    let raise = descendants(&s, f).into_iter().find(|n| n.title == "throw Error").expect("throw is a step");
    assert_eq!(raise.badges, ["unless res.ok"]);
}

#[test]
fn services_routes_and_the_database() {
    let s = analyze(&[
        (
            "users.service.ts",
            r#"
import { Injectable } from '@nestjs/common';
import { PrismaService } from './prisma.service';

@Injectable()
export class UsersService {
  private cache = new Map<string, unknown>();
  constructor(private readonly prisma: PrismaService) {}

  async find(id: string) {
    if (this.cache.has(id)) return this.cache.get(id);
    const user = await this.prisma.user.findUnique({ where: { id } });
    this.remember(id, user);
    return user;
  }

  private remember(id: string, user: unknown) {
    this.cache.set(id, user);
  }
}
"#,
        ),
        ("prisma.service.ts", "import { PrismaClient } from '@prisma/client';\nexport class PrismaService extends PrismaClient {}\n"),
        (
            "server.js",
            r#"
const express = require('express');
const { UsersService } = require('./users.service');
const app = express();
app.use(express.json());
app.get('/users/:id', async (req, res) => {
  const user = await users.find(req.params.id);
  if (!user) return res.status(404).end();
  res.json(user);
});
app.post('/users', (req, res) => res.status(201).json(req.body));
const users = new UsersService();
app.listen(process.env.PORT || 3000);
"#,
        ),
    ]);
    // A query through a service built on a database client is a database step.
    let find_fn = s.nodes.iter().find(|n| n.title == "find" && n.kind == Kind::Function).unwrap();
    let query = kid(&s, find_fn, "user");
    assert!(query.effects.contains(&"database".to_string()), "{query:?}");
    // The constructor's parameter property is a component; methods call methods.
    let service = find(&s, "UsersService");
    assert!(descendants(&s, service).iter().any(|n| n.title == "prisma" && n.subtitle == "PrismaService"));
    assert!(descendants(&s, find_fn).iter().any(|n| n.title == "remember" && n.reference.is_some()));
    // Routes registered with inline functions, however short, are endpoints.
    let get = find(&s, "GET /users/:id");
    assert_eq!(get.category, "endpoint");
    assert!(get.badges.contains(&"async".to_string()));
    find(&s, "POST /users");
    // An object created below the route is still known inside it.
    let call = descendants(&s, get).into_iter().find(|n| n.title == "find").expect("users.find");
    assert_eq!(call.reference, Some(find_fn.id));
}

#[test]
fn deep_nesting_does_not_overflow() {
    let mut src = String::from("export const f = (x) => ");
    for _ in 0..400 {
        src.push_str("(x ? ");
    }
    src.push('1');
    for _ in 0..400 {
        src.push_str(" : 2)");
    }
    src.push_str(";\nexport function g() {\n");
    for _ in 0..120 {
        src.push_str("if (a) {\n");
    }
    src.push_str("go();\n");
    for _ in 0..120 {
        src.push_str("}\n");
    }
    src.push_str("}\n");
    let s = blockviz_core::analyze("t", vec![InFile { path: "deep.js".into(), text: Some(src.clone()), bytes: src.len() as u64 }]);
    assert!(!s.nodes.is_empty());
}

#[test]
fn libraries_list_their_constants_once_and_are_not_apps() {
    // Express's own `lib/application.js`: a library that defines `listen`, not an app.
    let s = analyze(&[
        (
            "lib/application.js",
            r#"
'use strict';
var debug = require('debug')('express:application');
var http = require('node:http');
var slice = Array.prototype.slice;
var app = exports = module.exports = {};
var trustProxyDefaultSymbol = '@@symbol:trust_proxy_default';

app.init = function init() {
  this.cache = Object.create(null);
  this.settings[trustProxyDefaultSymbol] = true;
};

app.listen = function listen() {
  var server = http.createServer(this);
  return server.listen.apply(server, slice.call(arguments));
};
"#,
        ),
        ("lib/express.js", "var proto = require('./application');\nexports = module.exports = createApplication;\nfunction createApplication() { var app = {}; Object.assign(app, proto); app.init(); return app; }\n"),
        ("index.js", "'use strict';\n\nmodule.exports = require('./lib/express');\n"),
        ("examples/hello.js", "var express = require('../index');\nvar app = module.exports = express();\napp.get('/', function (req, res) { res.send('Hello World'); });\n"),
    ]);
    let file = s.nodes.iter().find(|n| n.kind == Kind::File && n.title == "application.js").unwrap();
    let keys: Vec<&str> = s.nodes.iter().filter(|n| n.kind == Kind::Table && n.parent == Some(file.id)).flat_map(|n| n.rows.iter().map(|r| r.key.as_str())).collect();
    assert_eq!(keys.iter().filter(|k| **k == "trustProxyDefaultSymbol").count(), 1, "{keys:?}");
    assert!(!file.badges.contains(&"entry point".to_string()), "{:?}", file.badges);
    // `module.exports = require(…)` re-exports: a dependency, not a step.
    let index = s.nodes.iter().find(|n| n.kind == Kind::File && n.title == "index.js").unwrap();
    assert!(index.children.iter().all(|&c| s.nodes[c as usize].is_port()), "{:?}", kids(&s, index).iter().map(|n| &n.title).collect::<Vec<_>>());
    // Calling the package calls what `exports = module.exports = createApplication`
    // exports, through the re-export; the app it makes is not a constant.
    let create = find(&s, "createApplication");
    let hello = s.nodes.iter().find(|n| n.kind == Kind::File && n.title.ends_with("hello.js")).unwrap();
    assert!(descendants(&s, hello).iter().any(|n| n.kind == Kind::Call && n.reference == Some(create.id)), "{:?}", descendants(&s, hello).iter().map(|n| &n.title).collect::<Vec<_>>());
    assert!(descendants(&s, hello).iter().all(|n| n.kind != Kind::Table));
}

#[test]
fn documentation_comes_from_jsdoc_prose() {
    let s = analyze(&[(
        "ref.ts",
        r#"
export const flag = 1;

/**
 * Checks if a value is a ref object.
 *
 * @param r - The value to inspect.
 * @see {@link https://vuejs.org/api/reactivity-utilities.html#isref}
 */
export function isRef<T>(r: T | unknown): r is T
export function isRef(r: any): boolean {
  return r ? r.__v_isRef === true : false
}

/** @internal */
class RefImpl {
  value = 1
  get() { return this.value }
}

export class Store {
  /** Reads a key. */
  get(key: string): string
  get(key: number): number
  get(key: any) { return String(key) }
}
"#,
    )]);
    assert_eq!(find(&s, "isRef").doc, "Checks if a value is a ref object.");
    // A file's description is neither its licence nor its first statement's documentation.
    let s2 = analyze(&[
        ("lib/router.js", "/*!\n * router\n * Copyright(c) 2013 Roman Shtylman\n * MIT Licensed\n */\n\n/**\n * Module dependencies.\n */\nvar debug = require('debug');\nexports.route = function route(path) { debug(path); };\n"),
        ("lib/util.js", "/**\n * @file Helpers shared by the router.\n */\n\nexports.noop = function noop() {};\n"),
        ("lib/first.js", "/** Parses a path. */\nexport function parse(p) { return p.split('/'); }\n"),
    ]);
    let doc = |t: &str| s2.nodes.iter().find(|n| n.kind == Kind::File && n.title.ends_with(t)).unwrap().doc.clone();
    assert_eq!(doc("router.js"), "");
    assert_eq!(doc("util.js"), "Helpers shared by the router.");
    assert_eq!(doc("first.js"), "");
    assert_eq!(find(&s2, "parse").doc, "Parses a path.");
    assert_eq!(find(&s, "RefImpl").doc, "");
    // Overload signatures are one method.
    let store = find(&s, "Store");
    assert_eq!(descendants(&s, store).iter().filter(|n| n.title == "get").count(), 1);
}

#[test]
fn objects_and_prototypes_own_their_functions() {
    let s = analyze(&[
        (
            "app.js",
            r#"
var http = require('node:http');
var View = require('./view');

/** Application prototype. */
var app = exports = module.exports = {};

app.init = function init() {
  this.settings = {};
  this.defaultConfiguration();
};

app.defaultConfiguration = function defaultConfiguration() {
  this.set('etag', 'weak');
};

app.set = function set(setting, val) {
  this.settings[setting] = val;
  return this;
};

app.render = function render(name, options, callback) {
  var view = new View(name, { root: this.get('views') });
  view.render(options, callback);
};

app.listen = function listen() {
  var server = http.createServer(this);
  return server.listen.apply(server, arguments);
};
"#,
        ),
        (
            "view.js",
            r#"
var fs = require('node:fs');
var path = require('node:path');

module.exports = View;

/** A view to render. */
function View(name, options) {
  this.name = name;
  this.root = options.root;
  this.path = this.lookup(name);
}

View.prototype.lookup = function lookup(name) {
  return this.resolve(this.root, name);
};

View.prototype.render = function render(options, callback) {
  fs.readFile(this.path, 'utf8', callback);
};

View.prototype.resolve = function resolve(dir, file) {
  return path.join(dir, file);
};

View.cache = function cache() { return {}; };
"#,
        ),
        (
            "api.ts",
            r#"
export const api = {
  base: '/api',
  async getUser(id: string) { return this.request(`/users/${id}`); },
  saveUser: async (u: { id: string }) => fetch('/users', { method: 'POST', body: JSON.stringify(u) }),
  request(url: string) { return fetch(this.base + url).then((r) => r.json()); },
};
export const style = { color: 'red', width: 3 };
"#,
        ),
        ("server.js", "var app = require('./app');\napp.init();\napp.listen(3000);\n"),
    ]);
    let method = |owner: &str, name: &str| {
        let o = s.nodes.iter().find(|n| n.kind == Kind::Class && n.title == owner).unwrap_or_else(|| panic!("no {owner}"));
        o.children.iter().map(|&c| &s.nodes[c as usize]).find(|n| n.kind == Kind::Function && n.title == name).unwrap_or_else(|| panic!("no {owner}.{name}"))
    };
    // An object with functions attached is one thing; `this` inside them is it.
    let app = find(&s, "app");
    assert_eq!((app.kind, app.subtitle.as_str(), app.doc.as_str()), (Kind::Class, "object", "Application prototype."));
    let init = method("app", "init");
    assert!(descendants(&s, init).iter().any(|n| n.title == "defaultConfiguration" && n.reference == Some(method("app", "defaultConfiguration").id)));
    assert!(descendants(&s, method("app", "defaultConfiguration")).iter().any(|n| n.title == "set" && n.reference == Some(method("app", "set").id)));
    assert!(!method("app", "listen").badges.contains(&"static".to_string()));
    // Defining `listen` does not make the library an app; calling it does.
    let file = |t: &str| s.nodes.iter().find(|n| n.kind == Kind::File && n.title == t).unwrap();
    assert!(!file("app.js").badges.contains(&"entry point".to_string()));
    let server = file("server.js");
    assert!(descendants(&s, server).iter().any(|n| n.title == "listen" && n.reference == Some(method("app", "listen").id)));
    // A constructor function with prototype methods is a class.
    let view = find(&s, "View");
    assert_eq!(view.kind, Kind::Class);
    assert_eq!(view.doc, "A view to render.");
    let ctor = method("View", "constructor");
    assert_eq!((ctor.subtitle.as_str(), ctor.doc.as_str()), ("(name, options)", ""));
    assert!(descendants(&s, ctor).iter().any(|n| n.reference == Some(method("View", "lookup").id)));
    assert!(descendants(&s, method("View", "lookup")).iter().any(|n| n.reference == Some(method("View", "resolve").id)));
    assert!(method("View", "cache").badges.contains(&"static".to_string()));
    let render = method("app", "render");
    assert!(descendants(&s, render).iter().any(|n| n.kind == Kind::Call && n.link == Some(view.id)), "new View(…) constructs the class");
    // An object literal of methods, arrows included; a literal of values stays data.
    assert!(descendants(&s, method("api", "getUser")).iter().any(|n| n.reference == Some(method("api", "request").id)));
    assert_eq!(method("api", "saveUser").inputs, ["u"]);
    assert_eq!(find(&s, "style").kind, Kind::Table);
}

#[test]
fn calls_through_interfaces_and_abstract_classes_reach_implementations() {
    let s = analyze(&[
        (
            "src/repo.ts",
            "/** Where users are kept. */\nexport interface UserRepo {\n  find(id: string): Promise<User | undefined>;\n  save(user: User): Promise<void>;\n}\n\nexport interface User { id: string; name: string }\n\n/** Users in Postgres. */\nexport class PgUserRepo implements UserRepo {\n  constructor(private readonly pool: Pool) {}\n  async find(id: string) {\n    const r = await this.pool.query(\"SELECT * FROM users WHERE id = $1\", [id]);\n    return r.rows[0];\n  }\n  async save(user: User) {\n    await this.pool.query(\"INSERT INTO users VALUES ($1, $2)\", [user.id, user.name]);\n  }\n}\n\n/** A notifier sends one message. */\nexport abstract class Notifier {\n  abstract send(to: string, text: string): Promise<void>;\n  async welcome(user: User) {\n    await this.send(user.id, `Welcome ${user.name}`);\n  }\n}\n\nexport class EmailNotifier extends Notifier {\n  async send(to: string, text: string) { await fetch(`https://mail/${to}`, { method: \"POST\", body: text }); }\n}\n\nexport class SmsNotifier extends Notifier {\n  async send(to: string, text: string) { await fetch(`https://sms/${to}`, { method: \"POST\", body: text }); }\n}\n\ninterface Pool { query(sql: string, args: unknown[]): Promise<{ rows: User[] }> }\n",
        ),
        (
            "src/service.ts",
            "import { UserRepo, Notifier, PgUserRepo, EmailNotifier, SmsNotifier } from \"./repo\";\n\nexport class UserService {\n  constructor(private repo: UserRepo, private notifier: Notifier) {}\n\n  async register(id: string, name: string) {\n    await this.repo.save({ id, name });\n    await this.notifier.send(id, \"hi\");\n    return this.repo.find(id);\n  }\n}\n\nexport const service = new UserService(new PgUserRepo(pool()), Math.random() > 0.5 ? new EmailNotifier() : new SmsNotifier());\ndeclare function pool(): any;\n",
        ),
    ]);
    let method = |class: &str, name: &str| s.nodes.iter().find(|n| n.kind == Kind::Function && n.title == name && n.parent.is_some_and(|p| s.nodes[p as usize].title == class)).unwrap_or_else(|| panic!("no {class}.{name}"));
    fn call<'a>(s: &'a Scene, f: &'a Node, title: &str) -> &'a Node {
        descendants(s, f).into_iter().find(|n| n.kind == Kind::Call && n.title == title).unwrap_or_else(|| panic!("no {title} call in {}", f.title))
    }
    let register = method("UserService", "register");
    // `implements UserRepo`: the one repository, and the class says what it implements.
    assert_eq!(call(&s, register, "save").reference, Some(method("PgUserRepo", "save").id));
    assert_eq!(find(&s, "PgUserRepo").subtitle, "UserRepo");
    // An abstract method: either notifier, from callers and from the base class.
    for f in [register, method("Notifier", "welcome")] {
        let send = call(&s, f, "send");
        assert_eq!((send.subtitle.as_str(), send.reference), ("EmailNotifier or SmsNotifier", Some(method("Notifier", "send").id)));
    }
    assert!(method("Notifier", "send").badges.contains(&"abstract".to_string()));
}
