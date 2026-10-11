//! Python: Ruff's token stream, refined by the syntax tree.
//!
//! Tokens already know keywords (soft keywords such as `match`, `type` and
//! `lazy` only when the parser used them as keywords), strings, f-string parts
//! and comments. The tree adds what tokens cannot: which names are being
//! defined, called, annotated or passed as keyword arguments.

use super::{Hl, Runs};
use ruff_python_ast::token::TokenKind;
use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, Expr, PySourceType, Stmt};
use ruff_text_size::{Ranged, TextRange, TextSize};
use std::collections::HashMap;

const BUILTINS: &[&str] = &[
    "abs", "aiter", "all", "anext", "any", "ascii", "bin", "bool", "breakpoint", "bytearray", "bytes", "callable", "chr", "classmethod", "compile", "complex", "delattr", "dict", "dir", "divmod", "enumerate", "eval", "exec", "filter", "float", "format", "frozenset", "getattr", "globals", "hasattr", "hash", "help", "hex", "id", "input", "int", "isinstance", "issubclass", "iter", "len", "list", "locals", "map", "max", "memoryview", "min", "next", "object", "oct", "open", "ord", "pow", "print", "property", "range", "repr", "reversed", "round", "set", "setattr", "slice", "sorted", "staticmethod", "str", "sum", "super", "tuple", "type", "vars", "zip", "__import__",
];

pub(super) fn highlight(text: &str) -> Vec<u32> {
    let mut runs = Runs::new(text.len());
    // The same guard the analysis uses: pathological generated files stay plain.
    if crate::python::too_complex(text).is_some() {
        return runs.finish();
    }
    let parsed = ruff_python_parser::parse_unchecked_source(text, PySourceType::Python);
    let mut marks = Marks { text, at: HashMap::new() };
    marks.visit_body(&parsed.syntax().body);
    for t in parsed.tokens().iter() {
        let (s, e) = (usize::from(t.start()), usize::from(t.end()));
        let mark = marks.at.get(&(s as u32)).copied();
        let hl = match t.kind() {
            TokenKind::Comment => Hl::Comment,
            TokenKind::String
            | TokenKind::FStringStart
            | TokenKind::FStringMiddle
            | TokenKind::FStringEnd
            | TokenKind::TStringStart
            | TokenKind::TStringMiddle
            | TokenKind::TStringEnd => Hl::String,
            TokenKind::Int | TokenKind::Float | TokenKind::Complex => Hl::Number,
            TokenKind::True | TokenKind::False | TokenKind::None => Hl::Constant,
            TokenKind::IpyEscapeCommand => Hl::Meta,
            k if k.is_keyword() => Hl::Keyword,
            TokenKind::Identifier => name(&text[s..e], mark),
            _ => mark.unwrap_or(Hl::Plain),
        };
        if hl != Hl::Plain {
            runs.mark(s, e, hl);
        }
    }
    runs.finish()
}

fn name(id: &str, mark: Option<Hl>) -> Hl {
    if id == "self" || id == "cls" {
        return Hl::SelfVar;
    }
    match mark {
        Some(h @ (Hl::Function | Hl::Decorator | Hl::Param | Hl::Type)) => return h,
        Some(Hl::Call) if BUILTINS.contains(&id) => return Hl::Builtin,
        _ => {}
    }
    let upper = id.starts_with(|c: char| c.is_ascii_uppercase());
    let has_lower = id.chars().any(|c| c.is_lowercase());
    if upper && !has_lower && id.len() > 1 {
        return Hl::Constant;
    }
    if mark == Some(Hl::Call) {
        return if upper { Hl::Type } else { Hl::Call };
    }
    if upper {
        return Hl::Type;
    }
    if id.len() > 4 && id.starts_with("__") && id.ends_with("__") {
        return Hl::Builtin;
    }
    Hl::Plain
}

/// Classes for name tokens, keyed by token start.
struct Marks<'t> {
    text: &'t str,
    at: HashMap<u32, Hl>,
}

impl Marks<'_> {
    fn set(&mut self, r: TextRange, hl: Hl) {
        self.at.insert(u32::from(r.start()), hl);
    }

    /// Like `set`, but a more specific class already recorded wins.
    fn soft(&mut self, r: TextRange, hl: Hl) {
        self.at.entry(u32::from(r.start())).or_insert(hl);
    }

    /// Names in a type expression: `list[Tensor] | None`, `torch.Tensor`.
    fn types(&mut self, e: &Expr) {
        match e {
            Expr::Name(n) => self.set(n.range, Hl::Type),
            Expr::Attribute(a) => self.set(a.attr.range, Hl::Type),
            Expr::Subscript(s) => {
                self.types(&s.value);
                self.types(&s.slice);
            }
            Expr::Tuple(t) => t.elts.iter().for_each(|e| self.types(e)),
            Expr::List(l) => l.elts.iter().for_each(|e| self.types(e)),
            Expr::BinOp(b) => {
                self.types(&b.left);
                self.types(&b.right);
            }
            _ => {}
        }
    }

    /// Every name in a dotted decorator: `@torch.no_grad()`.
    fn dotted(&mut self, e: &Expr) {
        match e {
            Expr::Name(n) => self.set(n.range, Hl::Decorator),
            Expr::Attribute(a) => {
                self.dotted(&a.value);
                self.set(a.attr.range, Hl::Decorator);
            }
            Expr::Call(c) => self.dotted(&c.func),
            _ => {}
        }
    }

    fn byte_at(&self, at: TextSize) -> Option<u8> {
        self.text.as_bytes().get(usize::from(at)).copied()
    }
}

impl<'a> Visitor<'a> for Marks<'_> {
    fn visit_stmt(&mut self, s: &'a Stmt) {
        match s {
            Stmt::FunctionDef(f) => self.set(f.name.range, Hl::Function),
            Stmt::ClassDef(c) => self.set(c.name.range, Hl::Type),
            Stmt::TypeAlias(t) => self.types(&t.name),
            _ => {}
        }
        visitor::walk_stmt(self, s);
    }

    fn visit_decorator(&mut self, d: &'a ast::Decorator) {
        // `d.range` starts at the `@`.
        self.set(d.range, Hl::Decorator);
        self.dotted(&d.expression);
        visitor::walk_decorator(self, d);
    }

    fn visit_annotation(&mut self, e: &'a Expr) {
        self.types(e);
        visitor::walk_annotation(self, e);
    }

    fn visit_parameter(&mut self, p: &'a ast::Parameter) {
        if !matches!(p.name.id.as_str(), "self" | "cls") {
            self.set(p.name.range, Hl::Param);
        }
        visitor::walk_parameter(self, p);
    }

    fn visit_keyword(&mut self, k: &'a ast::Keyword) {
        if let Some(a) = &k.arg {
            self.set(a.range, Hl::Param);
        }
        visitor::walk_keyword(self, k);
    }

    fn visit_type_param(&mut self, t: &'a ast::TypeParam) {
        let name = match t {
            ast::TypeParam::TypeVar(v) => &v.name,
            ast::TypeParam::TypeVarTuple(v) => &v.name,
            ast::TypeParam::ParamSpec(v) => &v.name,
        };
        self.set(name.range, Hl::Type);
        visitor::walk_type_param(self, t);
    }

    fn visit_expr(&mut self, e: &'a Expr) {
        if let Expr::Call(c) = e {
            match &*c.func {
                Expr::Name(n) => self.soft(n.range, Hl::Call),
                Expr::Attribute(a) => self.soft(a.attr.range, Hl::Call),
                _ => {}
            }
        }
        visitor::walk_expr(self, e);
    }

    fn visit_interpolated_string_element(&mut self, el: &'a ast::InterpolatedStringElement) {
        if let ast::InterpolatedStringElement::Interpolation(i) = el {
            let (open, close) = (i.range.start(), i.range.end() - TextSize::from(1));
            if self.byte_at(open) == Some(b'{') {
                self.set(TextRange::at(open, 1.into()), Hl::Interp);
            }
            if self.byte_at(close) == Some(b'}') {
                self.set(TextRange::at(close, 1.into()), Hl::Interp);
            }
        }
        visitor::walk_interpolated_string_element(self, el);
    }
}
