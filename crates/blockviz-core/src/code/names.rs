//! Python expressions described for naming (see `crate::naming`): the shape
//! of step an expression is, where an assignment stores its result, and short
//! renderings of code that elide whole sub-expressions (`f(…)`, `a and …`)
//! instead of cutting text.

use super::source::Source;
use super::syntax::{self as ast, CompKind, Expr, Ranged, TemplateKind, TemplatePart};
use crate::naming::{self, Collection, Dest, Step};
use crate::text::squash;

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

/// Shortens code known only as text by eliding the contents of bracketed
/// groups, deepest first, then longest, until it fits:
/// `{ [K in keyof T as `get${Capitalize<string & K>}`]: () => T[K] }` →
/// `{ [K in keyof T as `…`]: () => T[K] }`. `None` if no elision fits.
fn elide_groups(text: &str, max: usize) -> Option<String> {
    let mut s: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    loop {
        if s.chars().count() <= max {
            return Some(s);
        }
        let chars: Vec<char> = s.chars().collect();
        // (open, close, depth) of each group whose contents are not yet elided.
        let mut groups: Vec<(usize, usize, usize)> = vec![];
        let mut stack: Vec<(char, usize)> = vec![];
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let prev = if i > 0 { chars[i - 1] } else { ' ' };
            let in_template = stack.last().is_some_and(|t| t.0 == '`');
            match c {
                '\'' | '"' if !in_template => {
                    // Skip a quoted literal.
                    let close = chars[i + 1..].iter().position(|&x| x == c).map(|p| i + 1 + p).unwrap_or(chars.len() - 1);
                    i = close + 1;
                    continue;
                }
                '`' if in_template => {
                    let (_, open) = stack.pop().unwrap();
                    groups.push((open, i, stack.len() + 1));
                }
                '`' => stack.push(('`', i)),
                '$' if in_template && chars.get(i + 1) == Some(&'{') => {
                    stack.push(('{', i + 1));
                    i += 2;
                    continue;
                }
                '(' | '[' | '{' => stack.push((c, i)),
                '<' if prev.is_alphanumeric() || prev == '_' => stack.push(('<', i)),
                ')' | ']' | '}' | '>' => {
                    let opener = match c {
                        ')' => '(',
                        ']' => '[',
                        '}' => '{',
                        _ => '<',
                    };
                    // `=>` is an arrow, not a closing bracket.
                    if c == '>' && (prev == '=' || stack.last().is_none_or(|t| t.0 != '<')) {
                        i += 1;
                        continue;
                    }
                    if let Some(pos) = stack.iter().rposition(|t| t.0 == opener) {
                        stack.truncate(pos + 1);
                        let (_, open) = stack.pop().unwrap();
                        groups.push((open, i, stack.len() + 1));
                    }
                }
                _ => {}
            }
            i += 1;
        }
        // Only groups whose elision shortens the text.
        let best = groups.iter().filter(|g| g.1 - g.0 > 2).max_by_key(|g| (g.2, g.1 - g.0));
        match best {
            Some(&(open, close, _)) => s = chars[..=open].iter().collect::<String>() + "…" + &chars[close..].iter().collect::<String>(),
            // `T | Ref<T> | ShallowRef<T> | …`, `T extends U ? …`: whole leading parts.
            None => return top_level_prefix(&chars, &groups, max),
        }
    }
}

/// The longest run of whole top-level parts (split at ` | `, ` & `, ` ? `,
/// ` : ` or `, `) that fits with a trailing `…`.
fn top_level_prefix(chars: &[char], groups: &[(usize, usize, usize)], max: usize) -> Option<String> {
    let nested = |i: usize| groups.iter().any(|g| g.0 < i && i < g.1);
    let text: String = chars.iter().collect();
    let mut best: Option<String> = None;
    for (i, _) in chars.iter().enumerate() {
        let rest: String = chars[i..].iter().take(3).collect();
        let sep = match rest.as_str() {
            " | " | " & " | " ? " | " : " => rest.trim().to_string(),
            r if r.starts_with(", ") => ",".to_string(),
            _ => continue,
        };
        if nested(i) {
            continue;
        }
        let head: String = text.chars().take(i).collect();
        let candidate = if sep == "," { format!("{head}, …") } else { format!("{head} {sep} …") };
        if candidate.chars().count() <= max {
            best = Some(candidate);
        }
    }
    best
}

fn string_value(e: &Expr) -> Option<String> {
    match e {
        Expr::StringLiteral(s) => Some(s.value.clone()),
        Expr::BytesLiteral(b) => Some(String::from_utf8_lossy(&b.value).into_owned()),
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

    /// `a.b.c` for a chain of names, with Rust's `::` where the code has it
    /// (`Command::Get`, `self.db`).
    pub fn path(&self, e: &Expr) -> Option<String> {
        match e {
            Expr::Name(n) => Some(n.id.to_string()),
            Expr::Attribute(a) => {
                let base = self.path(&a.value)?;
                let gap = self.src.text.get(usize::from(a.value.range().end())..usize::from(a.range.end())).unwrap_or("");
                let sep = if gap.trim_start().starts_with("::") { "::" } else { "." };
                Some(format!("{base}{sep}{}", a.attr))
            }
            _ => None,
        }
    }

    /// A word for what an expression is: `items` for `len(items)`,
    /// `listdir` for `os.listdir(d)`, `{http, https}` for a dict display.
    pub fn brief(&self, e: &Expr) -> String {
        match e {
            // A Rust macro is named without its `!`.
            Expr::Name(n) => n.id.trim_end_matches('!').to_string(),
            Expr::Attribute(a) => match self.path(e) {
                Some(d) if d.chars().count() <= 20 => d,
                _ => a.attr.trim_end_matches('!').to_string(),
            },
            Expr::Call(c) => match &*c.func {
                Expr::Name(n) if WRAPPERS.contains(&n.id.as_str()) && !c.arguments.args.is_empty() => self.brief(&c.arguments.args[0]),
                Expr::Name(n) => n.id.trim_end_matches('!').to_string(),
                Expr::Attribute(a) => a.attr.trim_end_matches('!').to_string(),
                f => self.brief(f),
            },
            // `state["epoch"]` is the epoch.
            Expr::Subscript(s) => match string_value(&s.slice) {
                Some(k) if !k.is_empty() && k.chars().all(|c| c.is_alphanumeric() || c == '_') => k,
                _ => self.brief(&s.value),
            },
            Expr::Await(a) => self.brief(&a.value),
            // A Rust range `0..n`.
            Expr::Slice(_) => {
                let t = self.text(e);
                if t.chars().count() <= 14 {
                    t
                } else {
                    "range".into()
                }
            }
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
            Expr::List(_) => "list".into(),
            Expr::Comp(c) if c.kind == CompKind::List => "list".into(),
            Expr::Tuple(_) => "tuple".into(),
            Expr::Set(_) => "set".into(),
            Expr::Comp(c) if c.kind == CompKind::Set => "set".into(),
            Expr::Template(t) if t.kind == TemplateKind::Format => "text".into(),
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
                if self.src.lang.is_js() {
                    format!("{} ? {} : {}", self.code(&x.test, third), self.code(&x.body, third), self.code(&x.orelse, third))
                } else {
                    format!("{} if {} else {}", self.code(&x.body, third), self.code(&x.test, third), self.code(&x.orelse, third))
                }
            }
            Expr::Slice(_) => "…".into(),
            // `let Some(top) = …` (Rust), `(m := …)` (Python).
            Expr::Named(n) => {
                let target = self.code(&n.target, max / 2);
                if self.src.lang == ast::Lang::Rust {
                    format!("let {target} = {}", self.code(&n.value, max.saturating_sub(len(&target) + 7)))
                } else if self.src.lang.is_js() {
                    format!("{target} = {}", self.code(&n.value, max.saturating_sub(len(&target) + 3)))
                } else {
                    format!("({target} := {})", self.code(&n.value, max.saturating_sub(len(&target) + 6)))
                }
            }
            Expr::BoolOp(b) => {
                let op = format!(" {} ", b.op.as_str());
                let mut out = String::new();
                for v in &b.values {
                    // `a and (b or c)`: a nested test keeps its parentheses
                    // (Rust's `let` in a condition needs none).
                    let nested = matches!(v, Expr::BoolOp(_) | Expr::If(_)) || matches!(v, Expr::Named(_)) && self.src.lang != ast::Lang::Rust;
                    let part = self.code(v, max.saturating_sub(len(&out) + op.len() + 1 + if nested { 2 } else { 0 }).max(8));
                    let part = if nested && !(part.starts_with('(') && part.ends_with(')')) { format!("({part})") } else { part };
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
                    ast::UnaryOp::TypeOf => "typeof ",
                    ast::UnaryOp::Void => "void ",
                    ast::UnaryOp::Delete => "delete ",
                    other => other.as_str(),
                };
                let inner = self.code(&u.operand, max.saturating_sub(op.len()));
                // `!(a && b)`, `not (a or b)`: the operator applies to the whole.
                if matches!(&*u.operand, Expr::BoolOp(_) | Expr::Compare(_) | Expr::BinOp(_) | Expr::If(_)) && !(inner.starts_with('(') && inner.ends_with(')')) {
                    format!("{op}({inner})")
                } else {
                    format!("{op}{inner}")
                }
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
            // `<Spinner …/>`, `<div>…</div>`
            Expr::Call(c) if c.kind == ast::CallKind::Jsx => {
                let tag = match &*c.func {
                    Expr::StringLiteral(s) => s.value.clone(),
                    f => self.path(f).unwrap_or_else(|| self.brief(f)),
                };
                if c.arguments.args.is_empty() {
                    format!("<{tag} …/>")
                } else {
                    format!("<{tag}>…</{tag}>")
                }
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
            Expr::Attribute(a) if self.path(e).is_some() => {
                let q = naming::qualifier(&self.path(e).unwrap(), max);
                if q.is_empty() {
                    a.attr.to_string()
                } else {
                    q
                }
            }
            Expr::Attribute(a) => {
                let base = match &*a.value {
                    v @ (Expr::Call(_) | Expr::Subscript(_)) => self.code(v, max.saturating_sub(a.attr.len() + 1)),
                    v => match self.path(v) {
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
                let base = match self.path(&s.value) {
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
            Expr::Template(t) if t.kind == TemplateKind::Format => match self.template(e) {
                Some(t) => format!("\"{}\"", naming::shorten(&t, max.saturating_sub(2))),
                None => "…".into(),
            },
            // Rust awaits after the value: `read_buf(…).await`.
            Expr::Await(a) if self.src.lang == ast::Lang::Rust => format!("{}.await", self.code(&a.value, max.saturating_sub(6))),
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
            Expr::Comp(_) => match self.step(e) {
                Step::Map { over, .. } => format!("each of {over}"),
                _ => "…".into(),
            },
            // Kept as text (TypeScript types): elide by its brackets.
            Expr::Other(_) => elide_groups(&text, max).unwrap_or_else(|| "…".into()),
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
            Expr::Template(t) if t.kind == TemplateKind::Format && t.tag.is_none() => {
                let mut out = String::new();
                for part in &t.parts {
                    match part {
                        TemplatePart::Literal(l) => out.push_str(l),
                        TemplatePart::Hole { expr, .. } => {
                            let b = self.brief(expr);
                            out.push_str(&format!("{{{}}}", if b.is_empty() { "…".into() } else { b }));
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
            Expr::UnaryOp(u) => u.op.is_not(),
            Expr::BoolOp(b) => b.values.iter().all(Self::is_test),
            Expr::Call(c) => matches!(&*c.func, Expr::Name(n) if matches!(n.id.as_str(), "isinstance" | "issubclass" | "hasattr" | "callable" | "any" | "all")),
            _ => false,
        }
    }

    /// What kind of step an expression is.
    pub fn step(&self, e: &Expr) -> Step {
        let js = self.src.lang.is_js();
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
            Expr::UnaryOp(u) if u.op.is_not() => {
                self.subjects(e, &mut subjects);
                Step::Test { subjects }
            }
            // `x or default` picks a value; `a and b` tests.
            Expr::BoolOp(b) if b.op.is_or() && !b.values.last().is_some_and(Self::is_test) => {
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
            Expr::Dict(d) => Step::Pack { kind: if js { Collection::Object } else { Collection::Map }, keys: d.items.iter().filter_map(|i| i.key.as_ref().and_then(string_value)).collect() },
            Expr::List(ast::ExprSeq { elts, .. }) | Expr::Tuple(ast::ExprSeq { elts, .. }) | Expr::Set(ast::ExprSeq { elts, .. }) => {
                let kind = match e {
                    Expr::List(_) if js => Collection::Array,
                    Expr::List(_) => Collection::List,
                    Expr::Set(_) => Collection::Set,
                    _ => Collection::Tuple,
                };
                Step::Pack { kind, keys: elts.iter().map(|x| self.brief(x)).filter(|b| !b.is_empty()).collect() }
            }
            Expr::Call(c) if matches!(&*c.func, Expr::Name(n) if n.id.as_str() == "dict") && !c.arguments.keywords.is_empty() => {
                Step::Pack { kind: Collection::Map, keys: c.arguments.keywords.iter().filter_map(|k| k.arg.as_ref().map(|a| a.to_string())).collect() }
            }
            Expr::Comp(d) if d.kind == CompKind::Dict => Step::Map { over: d.generators.first().map(|g| self.brief(&g.iter)).unwrap_or_default(), filtered: false },
            Expr::Comp(c) => {
                let generators = &c.generators;
                let over = generators.first().map(|g| self.brief(&g.iter)).unwrap_or_default();
                let identity = generators.first().is_some_and(|g| self.text(&c.elt) == self.text(&g.target));
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
        let mut path = false;
        let receiver = loop {
            match cur {
                // `<section>…</section>`: an element is named by its tag.
                Expr::Call(c) if c.kind == ast::CallKind::Jsx => {
                    let tag = match &*c.func {
                        Expr::StringLiteral(s) => s.value.clone(),
                        f => self.path(f).unwrap_or_else(|| self.brief(f)),
                    };
                    names.push(format!("<{tag}>"));
                    break None;
                }
                Expr::Call(c) if std::ptr::eq(cur, e) || !(self.is_block)(c) => match &*c.func {
                    Expr::Attribute(a) => {
                        names.push(a.attr.trim_end_matches('!').to_string());
                        let gap = self.src.text.get(usize::from(a.value.range().end())..usize::from(a.range.end())).unwrap_or("");
                        path = gap.trim_start().starts_with("::");
                        cur = &a.value;
                    }
                    Expr::Name(n) => {
                        names.push(n.id.trim_end_matches('!').to_string());
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
        let receiver = receiver.map(|r| self.path(r).unwrap_or_else(|| self.code(r, 20))).unwrap_or_default();
        Step::Calls { names, receiver, reads: super::walk::loads(e), path }
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
        self.path(e).unwrap_or_else(|| self.code(e, 24))
    }
}


#[cfg(test)]
mod tests {
    use super::elide_groups;

    #[test]
    fn elides_types_by_their_brackets() {
        let mapped = "{ [K in keyof T as `get${Capitalize<string & K>}`]: () => T[K] }";
        assert_eq!(elide_groups(mapped, 40).as_deref(), Some("{ [K in keyof T as `…`]: () => T[K] }"));
        assert_eq!(elide_groups("Ref<T, S> & { [ShallowRefMarker]?: true }", 30).as_deref(), Some("Ref<T, S> & { […]?: true }"));
        // Arrows and comparisons are not brackets.
        assert_eq!(elide_groups("(value: string) => Promise<Map<string, number>>", 34).as_deref(), Some("(value: string) => Promise<Map<…>>"));
        assert_eq!(elide_groups("a_very_long_identifier_without_any_groups", 10), None);
        // Unions and conditionals keep whole leading parts.
        assert_eq!(elide_groups("T | Ref<T> | ShallowRef<T> | WritableComputedRef<T>", 40).as_deref(), Some("T | Ref<T> | ShallowRef<T> | …"));
        assert_eq!(elide_groups("T extends readonly any[] ? number extends T['length'] ? `${number}` : never : never", 40).as_deref(), Some("T extends readonly any[] ? …"));
    }
}
