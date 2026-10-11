//! Python expressions described for naming (see `crate::naming`): the shape
//! of step an expression is, where an assignment stores its result, and short
//! renderings of code that elide whole sub-expressions (`f(…)`, `a and …`)
//! instead of cutting text.

use super::source::Source;
use crate::naming::{self, Collection, Dest, Step};
use crate::text::squash;
use ruff_python_ast::{self as ast, Expr, FStringPartRef, InterpolatedStringElement};

pub struct Namer<'a> {
    pub src: &'a Source,
    /// Calls that are blocks of their own (project code, libraries, I/O): a
    /// chain of calls stops at them.
    pub is_block: Box<dyn Fn(&ast::ExprCall) -> bool + 'a>,
}

/// `a.b.c` for a chain of names, else `None`.
pub fn dotted(e: &Expr) -> Option<String> {
    match e {
        Expr::Name(n) => Some(n.id.to_string()),
        Expr::Attribute(a) => Some(format!("{}.{}", dotted(&a.value)?, a.attr)),
        _ => None,
    }
}

/// Builtins that pass their argument's identity through: `len(items)` is
/// about `items`.
const WRAPPERS: &[&str] = &["len", "int", "float", "str", "bool", "list", "tuple", "set", "frozenset", "sorted", "reversed", "repr", "abs", "round", "iter", "next", "enumerate", "any", "all", "type", "id", "bytes"];

fn string_value(e: &Expr) -> Option<String> {
    match e {
        Expr::StringLiteral(s) => Some(s.value.to_str().to_string()),
        Expr::BytesLiteral(b) => Some(String::from_utf8_lossy(&b.value.bytes().collect::<Vec<u8>>()).into_owned()),
        _ => None,
    }
}

impl<'a> Namer<'a> {
    /// A namer that treats no call as a block of its own.
    pub fn plain(src: &'a Source) -> Self {
        Namer { src, is_block: Box::new(|_| false) }
    }

    /// A call's arguments, each shortened, up to `max`: `path, mode, encoding=…`.
    pub fn args(&self, c: &ast::ExprCall, max: usize) -> String {
        let mut parts: Vec<String> = c.arguments.args.iter().map(|a| self.code(a, 20)).collect();
        for k in c.arguments.keywords.iter() {
            parts.push(match &k.arg {
                Some(a) => format!("{a}={}", self.code(&k.value, 16)),
                None => format!("**{}", self.code(&k.value, 16)),
            });
        }
        let mut out = String::new();
        for p in &parts {
            let next = if out.is_empty() { p.clone() } else { format!("{out}, {p}") };
            if next.chars().count() > max {
                return if out.is_empty() { "…".into() } else { format!("{out}, …") };
            }
            out = next;
        }
        out
    }

    fn text(&self, e: &Expr) -> String {
        squash(self.src.of(e))
    }

    /// A word for what an expression is: `items` for `len(items)`,
    /// `listdir` for `os.listdir(d)`, `{http, https}` for a dict display.
    pub fn brief(&self, e: &Expr) -> String {
        match e {
            Expr::Name(n) => n.id.to_string(),
            Expr::Attribute(a) => match dotted(e) {
                Some(d) if d.chars().count() <= 20 => d,
                _ => a.attr.to_string(),
            },
            Expr::Call(c) => match &*c.func {
                Expr::Name(n) if WRAPPERS.contains(&n.id.as_str()) && !c.arguments.args.is_empty() => self.brief(&c.arguments.args[0]),
                Expr::Name(n) => n.id.to_string(),
                Expr::Attribute(a) => a.attr.to_string(),
                f => self.brief(f),
            },
            // `state["epoch"]` is the epoch.
            Expr::Subscript(s) => match string_value(&s.slice) {
                Some(k) if !k.is_empty() && k.chars().all(|c| c.is_alphanumeric() || c == '_') => k,
                _ => self.brief(&s.value),
            },
            Expr::Await(a) => self.brief(&a.value),
            Expr::Starred(s) => self.brief(&s.value),
            Expr::Named(n) => self.brief(&n.target),
            Expr::StringLiteral(_) | Expr::BytesLiteral(_) => {
                let t = self.text(e);
                if t.chars().count() <= 12 {
                    t
                } else {
                    "text".into()
                }
            }
            Expr::NumberLiteral(_) | Expr::BooleanLiteral(_) | Expr::NoneLiteral(_) => self.text(e),
            Expr::Dict(d) => {
                let keys: Vec<String> = d.items.iter().filter_map(|i| i.key.as_ref().and_then(string_value)).collect();
                if keys.is_empty() {
                    "dict".into()
                } else {
                    format!("{{{}}}", naming::list(&keys, 18))
                }
            }
            Expr::List(_) | Expr::ListComp(_) => "list".into(),
            Expr::Tuple(_) => "tuple".into(),
            Expr::Set(_) | Expr::SetComp(_) => "set".into(),
            Expr::FString(_) => "text".into(),
            _ => String::new(),
        }
    }

    /// Source code shortened to about `max` characters by eliding whole
    /// sub-expressions: `settings.SENTRY_DSN and …`, `select(…).where(…)`.
    pub fn code(&self, e: &Expr, max: usize) -> String {
        let text = self.text(e);
        if text.chars().count() <= max {
            return text;
        }
        let len = |s: &str| s.chars().count();
        match e {
            // A name is one token: never cut.
            Expr::Name(_) => text,
            Expr::If(x) => {
                let third = max.saturating_sub(10) / 3;
                format!("{} if {} else {}", self.code(&x.body, third), self.code(&x.test, third), self.code(&x.orelse, third))
            }
            Expr::Slice(_) => "…".into(),
            Expr::BoolOp(b) => {
                let op = format!(" {} ", b.op.as_str());
                let mut out = String::new();
                for v in &b.values {
                    let part = self.code(v, max.saturating_sub(len(&out) + op.len() + 1).max(8));
                    let next = if out.is_empty() { part } else { format!("{out}{op}{part}") };
                    if len(&next) > max {
                        return if out.is_empty() { "…".into() } else { format!("{out}{op}…") };
                    }
                    out = next;
                }
                out
            }
            Expr::UnaryOp(u) => {
                let op = match u.op {
                    ast::UnaryOp::Not => "not ",
                    other => other.as_str(),
                };
                format!("{op}{}", self.code(&u.operand, max.saturating_sub(op.len())))
            }
            Expr::Compare(c) => {
                let half = max.saturating_sub(4) / 2;
                let left = &c.operands[0];
                let mut out = self.code(left, half);
                for (op, r) in c.ops.iter().zip(&c.operands[1..]) {
                    out = format!("{out} {} {}", op.as_str(), self.code(r, half));
                }
                if len(&out) > max + 4 {
                    format!("{} {} …", self.code(left, half), c.ops[0].as_str())
                } else {
                    out
                }
            }
            Expr::BinOp(b) => {
                let left = self.code(&b.left, max.saturating_sub(4) / 2);
                let right = self.code(&b.right, max.saturating_sub(len(&left) + 3));
                format!("{left} {} {right}", b.op.as_str())
            }
            Expr::Call(c) => {
                let func = self.code(&c.func, max.saturating_sub(3));
                if c.arguments.args.is_empty() && c.arguments.keywords.is_empty() {
                    format!("{func}()")
                } else {
                    format!("{func}(…)")
                }
            }
            // A dotted path keeps its last segments: `args.output_file_specified`.
            Expr::Attribute(a) if dotted(e).is_some() => {
                let q = naming::qualifier(&dotted(e).unwrap(), max);
                if q.is_empty() {
                    a.attr.to_string()
                } else {
                    q
                }
            }
            Expr::Attribute(a) => {
                let base = match &*a.value {
                    v @ (Expr::Call(_) | Expr::Subscript(_)) => self.code(v, max.saturating_sub(a.attr.len() + 1)),
                    v => match dotted(v) {
                        Some(d) => naming::qualifier(&d, max.saturating_sub(a.attr.len() + 1)),
                        None => "…".into(),
                    },
                };
                if base.is_empty() {
                    format!("….{}", a.attr)
                } else {
                    format!("{base}.{}", a.attr)
                }
            }
            // A string key is what the lookup is about: keep it, shorten the base.
            Expr::Subscript(s) if string_value(&s.slice).is_some() => {
                let key = self.code(&s.slice, max);
                let base = match dotted(&s.value) {
                    Some(d) => d.rsplit('.').next().unwrap_or(&d).to_string(),
                    None => self.code(&s.value, max.saturating_sub(len(&key) + 2)),
                };
                format!("{base}[{key}]")
            }
            Expr::Subscript(s) => {
                let base = self.code(&s.value, max.saturating_sub(5));
                let index = self.code(&s.slice, max.saturating_sub(len(&base) + 2));
                if len(&base) + len(&index) + 2 <= max {
                    format!("{base}[{index}]")
                } else {
                    format!("{base}[…]")
                }
            }
            // A string without spaces is a key or a name: one token.
            Expr::StringLiteral(_) | Expr::BytesLiteral(_) if !string_value(e).unwrap_or_default().contains(char::is_whitespace) => text,
            Expr::StringLiteral(_) | Expr::BytesLiteral(_) => {
                let quote = if text.starts_with('\'') { '\'' } else { '"' };
                format!("{quote}{}{quote}", naming::shorten(&string_value(e).unwrap_or_default(), max.saturating_sub(2)))
            }
            Expr::FString(_) => match self.template(e) {
                Some(t) => format!("\"{}\"", naming::shorten(&t, max.saturating_sub(2))),
                None => "…".into(),
            },
            Expr::Await(a) => format!("await {}", self.code(&a.value, max.saturating_sub(6))),
            Expr::Dict(_) => self.brief(e),
            Expr::List(_) | Expr::Tuple(_) | Expr::Set(_) => {
                let (open, close) = match e {
                    Expr::List(_) => ('[', ']'),
                    Expr::Set(_) => ('{', '}'),
                    _ => ('(', ')'),
                };
                format!("{open}…{close}")
            }
            Expr::ListComp(_) | Expr::SetComp(_) | Expr::DictComp(_) | Expr::Generator(_) => match self.step(e) {
                Step::Map { over, .. } => format!("each of {over}"),
                _ => "…".into(),
            },
            _ => "…".into(),
        }
    }

    /// Text built by an expression with its holes as `{name}`: f-strings,
    /// `"…".format(…)`, `"…" % x` and concatenations with literal text.
    pub fn template(&self, e: &Expr) -> Option<String> {
        self.template_raw(e).map(|t| squash(&t))
    }

    fn template_raw(&self, e: &Expr) -> Option<String> {
        match e {
            Expr::FString(f) => {
                let mut out = String::new();
                for part in f.value.iter() {
                    match part {
                        FStringPartRef::Literal(l) => out.push_str(&l.value),
                        FStringPartRef::FString(f) => {
                            for el in f.elements.iter() {
                                match el {
                                    InterpolatedStringElement::Literal(l) => out.push_str(&l.value),
                                    InterpolatedStringElement::Interpolation(i) => {
                                        let b = self.brief(&i.expression);
                                        out.push_str(&format!("{{{}}}", if b.is_empty() { "…".into() } else { b }));
                                    }
                                }
                            }
                        }
                    }
                }
                Some(out)
            }
            Expr::Call(c) => match &*c.func {
                Expr::Attribute(a) if a.attr.as_str() == "format" => string_value(&a.value),
                _ => None,
            },
            Expr::BinOp(b) if matches!(b.op, ast::Operator::Mod) => string_value(&b.left),
            Expr::BinOp(b) if matches!(b.op, ast::Operator::Add) => {
                let part = |x: &Expr| self.template_raw(x).or_else(|| string_value(x));
                let (l, r) = (part(&b.left), part(&b.right));
                if l.is_none() && r.is_none() {
                    return None;
                }
                let hole = |x: &Expr| format!("{{{}}}", self.brief(x));
                Some(format!("{}{}", l.unwrap_or_else(|| hole(&b.left)), r.unwrap_or_else(|| hole(&b.right))))
            }
            _ => None,
        }
    }

    /// The values a test or computation is about: `url.scheme`, `items`.
    fn subjects(&self, e: &Expr, out: &mut Vec<String>) {
        match e {
            Expr::BoolOp(b) => b.values.iter().for_each(|v| self.subjects(v, out)),
            Expr::UnaryOp(u) => self.subjects(&u.operand, out),
            Expr::BinOp(b) => {
                self.subjects(&b.left, out);
                self.subjects(&b.right, out);
            }
            Expr::Compare(c) => {
                c.operands.iter().for_each(|r| self.subjects(r, out));
            }
            e if e.is_literal_expr() || matches!(e, Expr::NoneLiteral(_)) => {}
            e => {
                let b = self.brief(e);
                if !b.is_empty() && !out.contains(&b) {
                    out.push(b);
                }
            }
        }
    }

    fn is_test(e: &Expr) -> bool {
        match e {
            Expr::Compare(_) => true,
            Expr::UnaryOp(u) => matches!(u.op, ast::UnaryOp::Not),
            Expr::BoolOp(b) => b.values.iter().all(Self::is_test),
            Expr::Call(c) => matches!(&*c.func, Expr::Name(n) if matches!(n.id.as_str(), "isinstance" | "issubclass" | "hasattr" | "callable" | "any" | "all")),
            _ => false,
        }
    }

    /// What kind of step an expression is.
    pub fn step(&self, e: &Expr) -> Step {
        let mut subjects = vec![];
        if let Some(template) = self.template(e) {
            return Step::Format { template };
        }
        match e {
            Expr::Await(a) => self.step(&a.value),
            Expr::Compare(_) => {
                self.subjects(e, &mut subjects);
                Step::Test { subjects }
            }
            Expr::UnaryOp(u) if matches!(u.op, ast::UnaryOp::Not) => {
                self.subjects(e, &mut subjects);
                Step::Test { subjects }
            }
            // `x or default` picks a value; `a and b` tests.
            Expr::BoolOp(b) if matches!(b.op, ast::BoolOp::Or) && !b.values.last().is_some_and(Self::is_test) => {
                b.values.iter().for_each(|v| self.subjects(v, &mut subjects));
                Step::Choose { subjects }
            }
            Expr::BoolOp(_) => {
                self.subjects(e, &mut subjects);
                Step::Test { subjects }
            }
            Expr::BinOp(_) | Expr::UnaryOp(_) => {
                self.subjects(e, &mut subjects);
                Step::Compute { subjects }
            }
            Expr::If(x) => {
                for v in [&*x.body, &*x.orelse] {
                    self.subjects(v, &mut subjects);
                }
                Step::Choose { subjects }
            }
            Expr::Dict(d) => Step::Pack { kind: Collection::Map, keys: d.items.iter().filter_map(|i| i.key.as_ref().and_then(string_value)).collect() },
            Expr::List(ast::ExprList { elts, .. }) | Expr::Tuple(ast::ExprTuple { elts, .. }) | Expr::Set(ast::ExprSet { elts, .. }) => {
                let kind = match e {
                    Expr::List(_) => Collection::List,
                    Expr::Set(_) => Collection::Set,
                    _ => Collection::Tuple,
                };
                Step::Pack { kind, keys: elts.iter().map(|x| self.brief(x)).filter(|b| !b.is_empty()).collect() }
            }
            Expr::Call(c) if matches!(&*c.func, Expr::Name(n) if n.id.as_str() == "dict") && !c.arguments.keywords.is_empty() => {
                Step::Pack { kind: Collection::Map, keys: c.arguments.keywords.iter().filter_map(|k| k.arg.as_ref().map(|a| a.to_string())).collect() }
            }
            Expr::DictComp(d) => Step::Map { over: d.generators.first().map(|g| self.brief(&g.iter)).unwrap_or_default(), filtered: false },
            Expr::ListComp(ast::ExprListComp { generators, .. }) | Expr::SetComp(ast::ExprSetComp { generators, .. }) | Expr::Generator(ast::ExprGenerator { generators, .. }) => {
                let over = generators.first().map(|g| self.brief(&g.iter)).unwrap_or_default();
                let identity = match e {
                    Expr::ListComp(l) => generators.first().is_some_and(|g| self.text(&l.elt) == self.text(&g.target)),
                    Expr::Generator(l) => generators.first().is_some_and(|g| self.text(&l.elt) == self.text(&g.target)),
                    Expr::SetComp(l) => generators.first().is_some_and(|g| self.text(&l.elt) == self.text(&g.target)),
                    _ => false,
                };
                Step::Map { over, filtered: identity && generators.iter().any(|g| !g.ifs.is_empty()) }
            }
            Expr::Subscript(s) => Step::Pick { from: self.brief(&s.value) },
            Expr::Attribute(a) => Step::Pick { from: self.brief(&a.value) },
            Expr::Call(_) => self.calls(e),
            _ => Step::Other,
        }
    }

    /// A chain of calls applied to one receiver: `x.view(…).transpose(…)`.
    /// The chain stops at a call that is a block of its own.
    fn calls(&self, e: &Expr) -> Step {
        let mut names = vec![];
        let mut cur = e;
        let receiver = loop {
            match cur {
                Expr::Call(c) if std::ptr::eq(cur, e) || !(self.is_block)(c) => match &*c.func {
                    Expr::Attribute(a) => {
                        names.push(a.attr.to_string());
                        cur = &a.value;
                    }
                    Expr::Name(n) => {
                        names.push(n.id.to_string());
                        break None;
                    }
                    f => {
                        names.push(self.brief(f));
                        break None;
                    }
                },
                Expr::Await(a) => cur = &a.value,
                other => break Some(other),
            }
        };
        names.reverse();
        let receiver = receiver.map(|r| dotted(r).unwrap_or_else(|| self.code(r, 20))).unwrap_or_default();
        Step::Calls { names, receiver, reads: super::walk::loads(e) }
    }

    /// Where an assignment target stores its result.
    pub fn dest(&self, t: &Expr) -> Dest {
        match t {
            Expr::Name(n) => Dest::Names(vec![n.id.to_string()]),
            Expr::Tuple(_) | Expr::List(_) => {
                let mut names = vec![];
                super::walk::target_names(t, &mut names);
                Dest::Names(names)
            }
            Expr::Attribute(a) => Dest::Field { owner: self.owner(&a.value), key: a.attr.to_string(), item: false },
            Expr::Subscript(s) => match string_value(&s.slice) {
                Some(k) => Dest::Field { owner: self.owner(&s.value), key: k, item: true },
                // `buf[a:b] = …` writes a slice; `buf[i] = …` an item.
                None if matches!(&*s.slice, Expr::Slice(_)) || matches!(&*s.slice, Expr::Tuple(t) if t.elts.iter().any(|x| matches!(x, Expr::Slice(_)))) => Dest::Item { owner: self.owner(&s.value), index: String::new() },
                None => Dest::Item { owner: self.owner(&s.value), index: self.code(&s.slice, 16) },
            },
            Expr::Starred(s) => self.dest(&s.value),
            _ => Dest::None,
        }
    }

    fn owner(&self, e: &Expr) -> String {
        dotted(e).unwrap_or_else(|| self.code(e, 24))
    }
}

