//! Ruff's Python syntax tree → the shared syntax tree (`crate::code::syntax`).
//! A direct translation: Python is the language the shared tree was first
//! shaped by, so every construct has a counterpart.

use crate::code::syntax::*;
use ruff_python_ast as py;
use ruff_python_ast::InterpolatedStringElement;

fn decorator_name(d: &py::Expr) -> &str {
    match d {
        py::Expr::Name(n) => n.id.as_str(),
        py::Expr::Attribute(a) => a.attr.as_str(),
        py::Expr::Call(c) => decorator_name(&c.func),
        _ => "",
    }
}

pub fn body(stmts: &[py::Stmt]) -> Vec<Stmt> {
    stmts.iter().map(|s| stmt(s, false)).collect()
}

fn class_body(stmts: &[py::Stmt]) -> Vec<Stmt> {
    stmts.iter().map(|s| stmt(s, true)).collect()
}

fn bx(e: &py::Expr) -> Box<Expr> {
    Box::new(expr(e))
}

fn opt(e: &Option<Box<py::Expr>>) -> Option<Box<Expr>> {
    e.as_deref().map(bx)
}

fn exprs(es: &[py::Expr]) -> Vec<Expr> {
    es.iter().map(expr).collect()
}

fn params(p: &py::Parameters) -> Vec<Param> {
    let mut out = vec![];
    let with_default = |a: &py::ParameterWithDefault, kind: ParamKind| Param {
        name: a.parameter.name.to_string(),
        kind,
        default: a.default.as_deref().map(expr),
        annotation: a.parameter.annotation.as_deref().map(expr),
        range: a.range,
    };
    for a in p.posonlyargs.iter().chain(&p.args) {
        out.push(with_default(a, ParamKind::Positional));
    }
    if let Some(v) = &p.vararg {
        out.push(Param { name: v.name.to_string(), kind: ParamKind::Variadic, default: None, annotation: v.annotation.as_deref().map(expr), range: v.range });
    }
    for a in &p.kwonlyargs {
        out.push(with_default(a, ParamKind::KeywordOnly));
    }
    if let Some(k) = &p.kwarg {
        out.push(Param { name: k.name.to_string(), kind: ParamKind::KeywordVariadic, default: None, annotation: k.annotation.as_deref().map(expr), range: k.range });
    }
    out
}

fn decorators(ds: &[py::Decorator]) -> Vec<Decorator> {
    ds.iter().map(|d| Decorator { expression: expr(&d.expression), range: d.range }).collect()
}

fn keywords(ks: &[py::Keyword]) -> Vec<Keyword> {
    ks.iter().map(|k| Keyword { arg: k.arg.as_ref().map(|a| a.to_string()), value: expr(&k.value), range: k.range }).collect()
}

fn function(f: &py::StmtFunctionDef, in_class: bool) -> FunctionDef {
    let decorated = |name: &str| f.decorator_list.iter().any(|d| decorator_name(&d.expression) == name);
    let kind = if !in_class {
        DefKind::Function
    } else if decorated("staticmethod") {
        DefKind::StaticMethod
    } else if decorated("classmethod") {
        DefKind::ClassMethod
    } else if decorated("property") || decorated("cached_property") {
        DefKind::Getter
    } else if f.name.as_str() == "__init__" {
        DefKind::Constructor
    } else {
        DefKind::Method
    };
    FunctionDef {
        name: f.name.to_string(),
        params: params(&f.parameters),
        body: body(&f.body),
        decorators: decorators(&f.decorator_list),
        returns: opt(&f.returns),
        is_async: f.is_async,
        is_generator: false,
        kind,
        doc: None,
        range: f.range,
    }
}

fn alias(a: &py::Alias) -> Alias {
    Alias { name: a.name.to_string(), asname: a.asname.as_ref().map(|n| n.to_string()), range: a.range }
}

fn stmt(s: &py::Stmt, in_class: bool) -> Stmt {
    match s {
        py::Stmt::FunctionDef(f) => Stmt::FunctionDef(function(f, in_class)),
        py::Stmt::ClassDef(c) => Stmt::ClassDef(ClassDef {
            name: c.name.to_string(),
            bases: c.arguments.as_ref().map(|a| exprs(&a.args)).unwrap_or_default(),
            keywords: c.arguments.as_ref().map(|a| keywords(&a.keywords)).unwrap_or_default(),
            body: class_body(&c.body),
            decorators: decorators(&c.decorator_list),
            kind: ClassKind::Class,
            doc: None,
            range: c.range,
        }),
        py::Stmt::Return(x) => Stmt::Return(StmtReturn { value: opt(&x.value), range: x.range }),
        py::Stmt::Delete(x) => Stmt::Delete(StmtDelete { targets: exprs(&x.targets), range: x.range }),
        py::Stmt::Assign(x) => Stmt::Assign(StmtAssign { targets: exprs(&x.targets), value: bx(&x.value), range: x.range }),
        py::Stmt::AugAssign(x) => Stmt::AugAssign(StmtAugAssign { target: bx(&x.target), op: operator(x.op), value: bx(&x.value), range: x.range }),
        py::Stmt::AnnAssign(x) => Stmt::AnnAssign(StmtAnnAssign { target: bx(&x.target), annotation: bx(&x.annotation), value: opt(&x.value), range: x.range }),
        py::Stmt::TypeAlias(x) => Stmt::TypeAlias(StmtTypeAlias { name: bx(&x.name), value: bx(&x.value), range: x.range }),
        py::Stmt::For(x) => Stmt::For(StmtFor { target: bx(&x.target), iter: bx(&x.iter), body: body(&x.body), orelse: body(&x.orelse), kind: ForKind::In, is_async: x.is_async, range: x.range }),
        py::Stmt::While(x) => Stmt::While(StmtWhile { test: bx(&x.test), body: body(&x.body), orelse: body(&x.orelse), range: x.range }),
        py::Stmt::If(x) => Stmt::If(StmtIf {
            test: bx(&x.test),
            body: body(&x.body),
            elif_else_clauses: x.elif_else_clauses.iter().map(|c| ElifElseClause { test: c.test.as_ref().map(expr), body: body(&c.body), range: c.range }).collect(),
            range: x.range,
        }),
        py::Stmt::With(x) => Stmt::With(StmtWith {
            items: x.items.iter().map(|i| WithItem { context_expr: expr(&i.context_expr), optional_vars: opt(&i.optional_vars), range: i.range }).collect(),
            body: body(&x.body),
            is_async: x.is_async,
            range: x.range,
        }),
        py::Stmt::Match(x) => Stmt::Match(StmtMatch {
            subject: bx(&x.subject),
            cases: x.cases.iter().map(|c| MatchCase { patterns: vec![Ranged::range(&c.pattern)], guard: opt(&c.guard), body: body(&c.body), range: c.range }).collect(),
            range: x.range,
        }),
        py::Stmt::Raise(x) => Stmt::Raise(StmtRaise { exc: opt(&x.exc), cause: opt(&x.cause), range: x.range }),
        py::Stmt::Try(x) => Stmt::Try(StmtTry {
            body: body(&x.body),
            handlers: x
                .handlers
                .iter()
                .map(|py::ExceptHandler::ExceptHandler(h)| ExceptHandler { type_: opt(&h.type_), name: h.name.as_ref().map(|n| n.to_string()), body: body(&h.body), range: h.range })
                .collect(),
            orelse: body(&x.orelse),
            finalbody: body(&x.finalbody),
            range: x.range,
        }),
        py::Stmt::Assert(x) => Stmt::Assert(StmtAssert { test: bx(&x.test), msg: opt(&x.msg), range: x.range }),
        py::Stmt::Import(x) => Stmt::Import(StmtImport { names: x.names.iter().map(alias).collect(), style: ModuleStyle::Dotted, resolved: None, range: x.range }),
        py::Stmt::ImportFrom(x) => Stmt::ImportFrom(StmtImportFrom {
            module: x.module.as_ref().map(|m| m.to_string()),
            names: x.names.iter().map(alias).collect(),
            level: x.level,
            style: ModuleStyle::Dotted,
            resolved: None,
            range: x.range,
        }),
        py::Stmt::Global(x) => Stmt::Global(StmtNames { names: x.names.iter().map(|n| n.to_string()).collect(), range: x.range }),
        py::Stmt::Nonlocal(x) => Stmt::Nonlocal(StmtNames { names: x.names.iter().map(|n| n.to_string()).collect(), range: x.range }),
        py::Stmt::Expr(x) => Stmt::Expr(StmtExpr { value: bx(&x.value), range: x.range }),
        py::Stmt::Pass(x) => Stmt::Pass(Atom { range: x.range }),
        py::Stmt::Break(x) => Stmt::Break(Atom { range: x.range }),
        py::Stmt::Continue(x) => Stmt::Continue(Atom { range: x.range }),
        py::Stmt::IpyEscapeCommand(x) => Stmt::Other(Atom { range: x.range }),
    }
}

fn operator(op: py::Operator) -> Operator {
    match op {
        py::Operator::Add => Operator::Add,
        py::Operator::Sub => Operator::Sub,
        py::Operator::Mult => Operator::Mult,
        py::Operator::MatMult => Operator::MatMult,
        py::Operator::Div => Operator::Div,
        py::Operator::Mod => Operator::Mod,
        py::Operator::Pow => Operator::Pow,
        py::Operator::LShift => Operator::LShift,
        py::Operator::RShift => Operator::RShift,
        py::Operator::BitOr => Operator::BitOr,
        py::Operator::BitXor => Operator::BitXor,
        py::Operator::BitAnd => Operator::BitAnd,
        py::Operator::FloorDiv => Operator::FloorDiv,
    }
}

fn cmp(op: py::CmpOp) -> CmpOp {
    match op {
        py::CmpOp::Eq => CmpOp::Eq,
        py::CmpOp::NotEq => CmpOp::NotEq,
        py::CmpOp::Lt => CmpOp::Lt,
        py::CmpOp::LtE => CmpOp::LtE,
        py::CmpOp::Gt => CmpOp::Gt,
        py::CmpOp::GtE => CmpOp::GtE,
        py::CmpOp::Is => CmpOp::Is,
        py::CmpOp::IsNot => CmpOp::IsNot,
        py::CmpOp::In => CmpOp::In,
        py::CmpOp::NotIn => CmpOp::NotIn,
    }
}

fn ctx(c: py::ExprContext) -> Ctx {
    match c {
        py::ExprContext::Store => Ctx::Store,
        py::ExprContext::Del => Ctx::Del,
        _ => Ctx::Load,
    }
}

fn holes(elements: &py::InterpolatedStringElements, out: &mut Vec<TemplatePart>) {
    for el in elements.iter() {
        match el {
            InterpolatedStringElement::Literal(l) => out.push(TemplatePart::Literal(l.value.to_string())),
            InterpolatedStringElement::Interpolation(i) => {
                let mut spec = vec![];
                if let Some(s) = &i.format_spec {
                    spec_holes(&s.elements, &mut spec);
                }
                out.push(TemplatePart::Hole { expr: expr(&i.expression), spec });
            }
        }
    }
}

fn spec_holes(elements: &py::InterpolatedStringElements, out: &mut Vec<Expr>) {
    for el in elements.iter() {
        if let InterpolatedStringElement::Interpolation(i) = el {
            out.push(expr(&i.expression));
            if let Some(s) = &i.format_spec {
                spec_holes(&s.elements, out);
            }
        }
    }
}

fn comp(kind: CompKind, elt: Option<&py::Expr>, value: Option<&py::Expr>, gens: &[py::Comprehension], range: TextRange) -> Expr {
    // `{**d for d in ds}` has no key: its unpacked mapping is the element.
    let (elt, value) = match (elt, value) {
        (Some(k), v) => (bx(k), v.map(bx)),
        (None, Some(v)) => (bx(v), None),
        (None, None) => (Box::new(Expr::Other(Atom { range })), None),
    };
    Expr::Comp(ExprComp {
        kind,
        elt,
        value,
        generators: gens.iter().map(|g| Comprehension { target: expr(&g.target), iter: expr(&g.iter), ifs: exprs(&g.ifs), range: g.range }).collect(),
        range,
    })
}

pub fn expr(e: &py::Expr) -> Expr {
    match e {
        py::Expr::BoolOp(x) => Expr::BoolOp(ExprBoolOp {
            op: match x.op {
                py::BoolOp::And => BoolOp::And,
                py::BoolOp::Or => BoolOp::Or,
            },
            values: exprs(&x.values),
            range: x.range,
        }),
        py::Expr::Named(x) => Expr::Named(ExprNamed { target: bx(&x.target), value: bx(&x.value), range: x.range }),
        py::Expr::BinOp(x) => Expr::BinOp(ExprBinOp { left: bx(&x.left), op: operator(x.op), right: bx(&x.right), range: x.range }),
        py::Expr::UnaryOp(x) => Expr::UnaryOp(ExprUnaryOp {
            op: match x.op {
                py::UnaryOp::Invert => UnaryOp::Invert,
                py::UnaryOp::Not => UnaryOp::Not,
                py::UnaryOp::UAdd => UnaryOp::UAdd,
                py::UnaryOp::USub => UnaryOp::USub,
            },
            operand: bx(&x.operand),
            range: x.range,
        }),
        py::Expr::Lambda(x) => Expr::Lambda(ExprLambda { params: x.parameters.as_deref().map(params).unwrap_or_default(), body: bx(&x.body), range: x.range }),
        py::Expr::If(x) => Expr::If(ExprIf { test: bx(&x.test), body: bx(&x.body), orelse: bx(&x.orelse), range: x.range }),
        py::Expr::Dict(x) => Expr::Dict(ExprDict { items: x.items.iter().map(|i| DictItem { key: i.key.as_ref().map(expr), value: expr(&i.value) }).collect(), range: x.range }),
        py::Expr::Set(x) => Expr::Set(ExprSeq { elts: exprs(&x.elts), range: x.range }),
        py::Expr::ListComp(x) => comp(CompKind::List, Some(&x.elt), None, &x.generators, x.range),
        py::Expr::SetComp(x) => comp(CompKind::Set, Some(&x.elt), None, &x.generators, x.range),
        py::Expr::Generator(x) => comp(CompKind::Generator, Some(&x.elt), None, &x.generators, x.range),
        py::Expr::DictComp(x) => comp(CompKind::Dict, x.key.as_deref(), Some(&x.value), &x.generators, x.range),
        py::Expr::Await(x) => Expr::Await(ExprInner { value: bx(&x.value), range: x.range }),
        py::Expr::Yield(x) => Expr::Yield(ExprYield { value: opt(&x.value), range: x.range }),
        py::Expr::YieldFrom(x) => Expr::YieldFrom(ExprInner { value: bx(&x.value), range: x.range }),
        py::Expr::Compare(x) => Expr::Compare(ExprCompare { ops: x.ops.iter().map(|o| cmp(*o)).collect(), operands: x.operands.iter().map(expr).collect(), range: x.range }),
        py::Expr::Call(x) => Expr::Call(ExprCall { func: bx(&x.func), arguments: Arguments { args: exprs(&x.arguments.args), keywords: keywords(&x.arguments.keywords) }, kind: CallKind::Call, range: Ranged::range(x) }),
        py::Expr::FString(x) => {
            let mut parts = vec![];
            for part in x.value.iter() {
                match part {
                    py::FStringPartRef::Literal(l) => parts.push(TemplatePart::Literal(l.value.to_string())),
                    py::FStringPartRef::FString(f) => holes(&f.elements, &mut parts),
                }
            }
            Expr::Template(ExprTemplate { kind: TemplateKind::Format, parts, tag: None, range: x.range })
        }
        py::Expr::TString(x) => {
            let mut parts = vec![];
            for t in x.value.iter() {
                holes(&t.elements, &mut parts);
            }
            Expr::Template(ExprTemplate { kind: TemplateKind::Template, parts, tag: None, range: x.range })
        }
        py::Expr::StringLiteral(x) => Expr::StringLiteral(ExprStringLiteral { value: x.value.to_str().to_string(), range: x.range }),
        py::Expr::BytesLiteral(x) => Expr::BytesLiteral(ExprBytesLiteral { value: x.value.bytes().collect(), range: x.range }),
        py::Expr::NumberLiteral(x) => Expr::NumberLiteral(Atom { range: x.range }),
        py::Expr::BooleanLiteral(x) => Expr::BooleanLiteral(ExprBooleanLiteral { value: x.value, range: x.range }),
        py::Expr::NoneLiteral(x) => Expr::NoneLiteral(Atom { range: x.range }),
        py::Expr::EllipsisLiteral(x) => Expr::EllipsisLiteral(Atom { range: x.range }),
        py::Expr::Attribute(x) => Expr::Attribute(ExprAttribute { value: bx(&x.value), attr: x.attr.to_string(), range: x.range }),
        py::Expr::Subscript(x) => Expr::Subscript(ExprSubscript { value: bx(&x.value), slice: bx(&x.slice), range: x.range }),
        py::Expr::Starred(x) => Expr::Starred(ExprInner { value: bx(&x.value), range: x.range }),
        py::Expr::Name(x) => Expr::Name(ExprName { id: x.id.to_string(), ctx: ctx(x.ctx), range: x.range }),
        py::Expr::List(x) => Expr::List(ExprSeq { elts: exprs(&x.elts), range: x.range }),
        py::Expr::Tuple(x) => Expr::Tuple(ExprSeq { elts: exprs(&x.elts), range: x.range }),
        py::Expr::Slice(x) => Expr::Slice(ExprSlice { lower: opt(&x.lower), upper: opt(&x.upper), step: opt(&x.step), range: x.range }),
        py::Expr::IpyEscapeCommand(x) => Expr::Other(Atom { range: x.range }),
    }
}
