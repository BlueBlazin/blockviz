//! A syntax tree shared by every language front end.
//!
//! Front ends (Python via Ruff, JavaScript/TypeScript via oxc, Rust via syn) lower their own
//! trees into this one, and everything downstream (the symbol index, the flow
//! builder, naming) reads only this. It keeps what diagrams need: statements,
//! expressions, definitions and imports, with byte ranges into the original
//! source so titles and the inspector show the code as written. Constructs a
//! language adds are lowered to the nearest shared shape: JSX elements are
//! calls, `switch` is a match, `this` is the receiver like `self`.

pub use ruff_text_size::{Ranged, TextRange, TextSize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    Python,
    JavaScript,
    TypeScript,
    Rust,
    Go,
}

impl Lang {
    pub fn is_js(self) -> bool {
        matches!(self, Lang::JavaScript | Lang::TypeScript)
    }
    pub fn name(self) -> &'static str {
        match self {
            Lang::Python => "Python",
            Lang::JavaScript => "JavaScript",
            Lang::TypeScript => "TypeScript",
            Lang::Rust => "Rust",
            Lang::Go => "Go",
        }
    }
    /// Classes declare their fields in their body (`users = new Map()`,
    /// `struct S { users: Users }`) rather than only assigning them in methods.
    pub fn declares_fields(self) -> bool {
        self != Lang::Python
    }
    /// Front ends resolve imports to files themselves (JavaScript specifiers,
    /// Rust module paths), and modules re-export names wholesale (`export *`,
    /// `pub use x::*`).
    pub fn resolves_paths(self) -> bool {
        self != Lang::Python
    }
    /// An assignment in a function makes a local name, unless the function
    /// declares it `global` or `nonlocal`. Elsewhere `x = …` may write a
    /// variable of the code around the function (`let` and `:=` declare,
    /// but the shared tree does not keep them apart from assignments).
    pub fn assignment_declares(self) -> bool {
        self == Lang::Python
    }
    /// The compiler rejects a local that is never used (Go) or warns of
    /// one, and of a value assigned and never read (Rust). A value that
    /// looks unread there is one whose reads the shared tree leaves out:
    /// an error check, a deferred call, a named result handed back by a
    /// bare `return`, a write through a reference (`*x = v`).
    pub fn warns_of_unused(self) -> bool {
        matches!(self, Lang::Go | Lang::Rust)
    }
    /// The receiver of a method: `self` or `this`.
    pub fn receiver(self) -> &'static str {
        if self.is_js() {
            "this"
        } else {
            "self"
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ctx {
    Load,
    Store,
    Del,
}

macro_rules! ranged {
    ($($t:ty),* $(,)?) => {
        $(impl Ranged for $t {
            fn range(&self) -> TextRange {
                self.range
            }
        })*
    };
}

// ----- expressions -------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum Expr {
    Name(ExprName),
    Attribute(ExprAttribute),
    Subscript(ExprSubscript),
    Call(ExprCall),
    StringLiteral(ExprStringLiteral),
    BytesLiteral(ExprBytesLiteral),
    NumberLiteral(Atom),
    BooleanLiteral(ExprBooleanLiteral),
    /// `None`, `null`, `undefined`.
    NoneLiteral(Atom),
    EllipsisLiteral(Atom),
    /// An f-string, t-string or template literal.
    Template(ExprTemplate),
    BinOp(ExprBinOp),
    UnaryOp(ExprUnaryOp),
    BoolOp(ExprBoolOp),
    Compare(ExprCompare),
    If(ExprIf),
    Lambda(ExprLambda),
    /// A function with statements used as a value: a callback, an event handler.
    Function(Box<FunctionDef>),
    Dict(ExprDict),
    List(ExprSeq),
    Tuple(ExprSeq),
    Set(ExprSeq),
    Comp(ExprComp),
    Await(ExprInner),
    Yield(ExprYield),
    YieldFrom(ExprInner),
    /// `*x` in Python, `...x` in JavaScript.
    Starred(ExprInner),
    /// `(x := f())`, or an assignment used as a value in JavaScript.
    Named(ExprNamed),
    Slice(ExprSlice),
    /// Syntax the analysis does not look into (regular expressions, class
    /// expressions, IPython escapes).
    Other(Atom),
}

#[derive(Clone, Debug)]
pub struct Atom {
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct ExprName {
    pub id: String,
    pub ctx: Ctx,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct ExprAttribute {
    pub value: Box<Expr>,
    pub attr: String,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct ExprSubscript {
    pub value: Box<Expr>,
    pub slice: Box<Expr>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct Keyword {
    /// `None` for `**kwargs` (Python) or `{...props}` (JSX).
    pub arg: Option<String>,
    pub value: Expr,
    pub range: TextRange,
}

#[derive(Clone, Debug, Default)]
pub struct Arguments {
    pub args: Vec<Expr>,
    pub keywords: Vec<Keyword>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallKind {
    Call,
    /// `new X(…)`.
    New,
    /// A JSX element: `func` is the component (or a string for an intrinsic
    /// element like `div`), props are keywords and children are arguments.
    Jsx,
}

#[derive(Clone, Debug)]
pub struct ExprCall {
    pub func: Box<Expr>,
    pub arguments: Arguments,
    pub kind: CallKind,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct ExprStringLiteral {
    pub value: String,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct ExprBytesLiteral {
    pub value: Vec<u8>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct ExprBooleanLiteral {
    pub value: bool,
    pub range: TextRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemplateKind {
    /// Python f-string, or JavaScript template literal: builds text.
    Format,
    /// Python t-string: builds a template object.
    Template,
}

#[derive(Clone, Debug)]
pub enum TemplatePart {
    Literal(String),
    /// An interpolated value; `spec` holds values inside its format spec.
    Hole { expr: Expr, spec: Vec<Expr> },
}

#[derive(Clone, Debug)]
pub struct ExprTemplate {
    pub kind: TemplateKind,
    pub parts: Vec<TemplatePart>,
    /// A tagged template's tag: `sql\`…\``, `styled.div\`…\``.
    pub tag: Option<Box<Expr>>,
    pub range: TextRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operator {
    Add,
    Sub,
    Mult,
    MatMult,
    Div,
    Mod,
    Pow,
    LShift,
    RShift,
    /// `>>>`
    URShift,
    BitOr,
    BitXor,
    BitAnd,
    FloorDiv,
}

impl Operator {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Operator::Add => "+",
            Operator::Sub => "-",
            Operator::Mult => "*",
            Operator::MatMult => "@",
            Operator::Div => "/",
            Operator::Mod => "%",
            Operator::Pow => "**",
            Operator::LShift => "<<",
            Operator::RShift => ">>",
            Operator::URShift => ">>>",
            Operator::BitOr => "|",
            Operator::BitXor => "^",
            Operator::BitAnd => "&",
            Operator::FloorDiv => "//",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExprBinOp {
    pub left: Box<Expr>,
    pub op: Operator,
    pub right: Box<Expr>,
    pub range: TextRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    /// Python `not`.
    Not,
    /// JavaScript `!`.
    Bang,
    Invert,
    UAdd,
    USub,
    TypeOf,
    Void,
    Delete,
}

impl UnaryOp {
    pub const fn as_str(&self) -> &'static str {
        match self {
            UnaryOp::Not => "not",
            UnaryOp::Bang => "!",
            UnaryOp::Invert => "~",
            UnaryOp::UAdd => "+",
            UnaryOp::USub => "-",
            UnaryOp::TypeOf => "typeof",
            UnaryOp::Void => "void",
            UnaryOp::Delete => "delete",
        }
    }
    /// Logical negation in either language.
    pub fn is_not(&self) -> bool {
        matches!(self, UnaryOp::Not | UnaryOp::Bang)
    }
}

#[derive(Clone, Debug)]
pub struct ExprUnaryOp {
    pub op: UnaryOp,
    pub operand: Box<Expr>,
    pub range: TextRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoolOp {
    /// Python `and` / `or`.
    And,
    Or,
    /// JavaScript `&&`, `||`, `??`.
    AndAnd,
    OrOr,
    Coalesce,
}

impl BoolOp {
    pub const fn as_str(&self) -> &'static str {
        match self {
            BoolOp::And => "and",
            BoolOp::Or => "or",
            BoolOp::AndAnd => "&&",
            BoolOp::OrOr => "||",
            BoolOp::Coalesce => "??",
        }
    }
    /// Picks the first value that is set (`a or b`, `a ?? b`).
    pub fn is_or(&self) -> bool {
        matches!(self, BoolOp::Or | BoolOp::OrOr | BoolOp::Coalesce)
    }
}

#[derive(Clone, Debug)]
pub struct ExprBoolOp {
    pub op: BoolOp,
    pub values: Vec<Expr>,
    pub range: TextRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    NotEq,
    Lt,
    LtE,
    Gt,
    GtE,
    Is,
    IsNot,
    In,
    NotIn,
    /// JavaScript `===`, `!==`, `instanceof`.
    StrictEq,
    StrictNotEq,
    InstanceOf,
}

impl CmpOp {
    pub const fn as_str(&self) -> &'static str {
        match self {
            CmpOp::Eq => "==",
            CmpOp::NotEq => "!=",
            CmpOp::Lt => "<",
            CmpOp::LtE => "<=",
            CmpOp::Gt => ">",
            CmpOp::GtE => ">=",
            CmpOp::Is => "is",
            CmpOp::IsNot => "is not",
            CmpOp::In => "in",
            CmpOp::NotIn => "not in",
            CmpOp::StrictEq => "===",
            CmpOp::StrictNotEq => "!==",
            CmpOp::InstanceOf => "instanceof",
        }
    }
}

/// `a < b < c`: `operands` are a, b, c and `ops` the comparisons between them.
#[derive(Clone, Debug)]
pub struct ExprCompare {
    pub ops: Vec<CmpOp>,
    pub operands: Vec<Expr>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct ExprIf {
    pub test: Box<Expr>,
    pub body: Box<Expr>,
    pub orelse: Box<Expr>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct ExprLambda {
    pub params: Vec<Param>,
    pub body: Box<Expr>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct DictItem {
    /// `None` for `**d` (Python) or `...d` (JavaScript).
    pub key: Option<Expr>,
    pub value: Expr,
}

#[derive(Clone, Debug)]
pub struct ExprDict {
    pub items: Vec<DictItem>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct ExprSeq {
    pub elts: Vec<Expr>,
    pub range: TextRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompKind {
    List,
    Set,
    Generator,
    Dict,
}

#[derive(Clone, Debug)]
pub struct Comprehension {
    pub target: Expr,
    pub iter: Expr,
    pub ifs: Vec<Expr>,
    pub range: TextRange,
}

/// A comprehension: `[elt for … in …]`, `{key: value for …}`.
#[derive(Clone, Debug)]
pub struct ExprComp {
    pub kind: CompKind,
    pub elt: Box<Expr>,
    /// The value of a dict comprehension (`elt` is its key); `None` for
    /// `{**d for d in ds}`, whose `elt` is the unpacked mapping.
    pub value: Option<Box<Expr>>,
    pub generators: Vec<Comprehension>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct ExprInner {
    pub value: Box<Expr>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct ExprYield {
    pub value: Option<Box<Expr>>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct ExprNamed {
    pub target: Box<Expr>,
    pub value: Box<Expr>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct ExprSlice {
    pub lower: Option<Box<Expr>>,
    pub upper: Option<Box<Expr>>,
    pub step: Option<Box<Expr>>,
    pub range: TextRange,
}

impl Ranged for Expr {
    fn range(&self) -> TextRange {
        match self {
            Expr::Name(x) => x.range,
            Expr::Attribute(x) => x.range,
            Expr::Subscript(x) => x.range,
            Expr::Call(x) => x.range,
            Expr::StringLiteral(x) => x.range,
            Expr::BytesLiteral(x) => x.range,
            Expr::NumberLiteral(x) | Expr::NoneLiteral(x) | Expr::EllipsisLiteral(x) | Expr::Other(x) => x.range,
            Expr::BooleanLiteral(x) => x.range,
            Expr::Template(x) => x.range,
            Expr::BinOp(x) => x.range,
            Expr::UnaryOp(x) => x.range,
            Expr::BoolOp(x) => x.range,
            Expr::Compare(x) => x.range,
            Expr::If(x) => x.range,
            Expr::Lambda(x) => x.range,
            Expr::Function(x) => x.range,
            Expr::Dict(x) => x.range,
            Expr::List(x) | Expr::Tuple(x) | Expr::Set(x) => x.range,
            Expr::Comp(x) => x.range,
            Expr::Await(x) | Expr::YieldFrom(x) | Expr::Starred(x) => x.range,
            Expr::Yield(x) => x.range,
            Expr::Named(x) => x.range,
            Expr::Slice(x) => x.range,
        }
    }
}

impl Expr {
    /// Strings, bytes, numbers, booleans, `None` and `...`.
    pub fn is_literal_expr(&self) -> bool {
        matches!(self, Expr::StringLiteral(_) | Expr::BytesLiteral(_) | Expr::NumberLiteral(_) | Expr::BooleanLiteral(_) | Expr::NoneLiteral(_) | Expr::EllipsisLiteral(_))
    }
}

ranged!(Atom, ExprName, ExprAttribute, ExprSubscript, ExprCall, ExprStringLiteral, ExprBytesLiteral, ExprBooleanLiteral, ExprTemplate, ExprBinOp, ExprUnaryOp, ExprBoolOp, ExprCompare, ExprIf, ExprLambda, ExprDict, ExprSeq, ExprComp, ExprInner, ExprYield, ExprNamed, ExprSlice, Keyword, Comprehension);

// ----- definitions ---------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    Positional,
    /// `*args` / `...rest`.
    Variadic,
    KeywordOnly,
    /// `**kwargs`.
    KeywordVariadic,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub kind: ParamKind,
    pub default: Option<Expr>,
    pub annotation: Option<Expr>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct Decorator {
    pub expression: Expr,
    pub range: TextRange,
}

/// What a function is to its class, when the syntax says so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefKind {
    Function,
    Method,
    Constructor,
    StaticMethod,
    ClassMethod,
    Getter,
    Setter,
}

#[derive(Clone, Debug)]
pub struct FunctionDef {
    /// Empty for an anonymous function expression.
    pub name: String,
    /// Parameters in declaration order. A method's receiver comes first
    /// (`self`, or `this` for JavaScript, where it is implicit).
    pub params: Vec<Param>,
    pub body: Vec<Stmt>,
    pub decorators: Vec<Decorator>,
    pub returns: Option<Box<Expr>>,
    pub is_async: bool,
    pub is_generator: bool,
    pub kind: DefKind,
    /// Documentation written outside the body (JSDoc); Python docstrings stay
    /// the body's first statement.
    pub doc: Option<String>,
    pub range: TextRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassKind {
    Class,
    /// A TypeScript interface or object type: fields only.
    Interface,
    Enum,
    /// A TypeScript namespace: functions and constants grouped under a name.
    Namespace,
    /// A JavaScript object that owns functions (`var app = {}; app.listen = …`).
    Object,
    /// A Rust trait: required methods are signatures, provided ones methods.
    Trait,
    /// Methods added to a type defined in another module (a Rust `impl`
    /// block): the type is the first base, an implemented trait the second.
    Extension,
}

#[derive(Clone, Debug)]
pub struct ClassDef {
    pub name: String,
    pub bases: Vec<Expr>,
    pub keywords: Vec<Keyword>,
    pub body: Vec<Stmt>,
    pub decorators: Vec<Decorator>,
    pub kind: ClassKind,
    pub doc: Option<String>,
    pub range: TextRange,
}

ranged!(Param, Decorator, FunctionDef, ClassDef);

// ----- statements -------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum Stmt {
    Expr(StmtExpr),
    Assign(StmtAssign),
    AnnAssign(StmtAnnAssign),
    AugAssign(StmtAugAssign),
    Return(StmtReturn),
    Raise(StmtRaise),
    Delete(StmtDelete),
    Assert(StmtAssert),
    Pass(Atom),
    Break(Atom),
    Continue(Atom),
    Global(StmtNames),
    Nonlocal(StmtNames),
    If(StmtIf),
    For(StmtFor),
    While(StmtWhile),
    With(StmtWith),
    Try(StmtTry),
    Match(StmtMatch),
    FunctionDef(FunctionDef),
    ClassDef(ClassDef),
    Import(StmtImport),
    ImportFrom(StmtImportFrom),
    TypeAlias(StmtTypeAlias),
    Export(StmtExport),
    Other(Atom),
}

#[derive(Clone, Debug)]
pub struct StmtExpr {
    pub value: Box<Expr>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct StmtAssign {
    pub targets: Vec<Expr>,
    pub value: Box<Expr>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct StmtAnnAssign {
    pub target: Box<Expr>,
    pub annotation: Box<Expr>,
    pub value: Option<Box<Expr>>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct StmtAugAssign {
    pub target: Box<Expr>,
    pub op: Operator,
    pub value: Box<Expr>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct StmtReturn {
    pub value: Option<Box<Expr>>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct StmtRaise {
    pub exc: Option<Box<Expr>>,
    pub cause: Option<Box<Expr>>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct StmtDelete {
    pub targets: Vec<Expr>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct StmtAssert {
    pub test: Box<Expr>,
    pub msg: Option<Box<Expr>>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct StmtNames {
    pub names: Vec<String>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct ElifElseClause {
    /// `None` for `else`.
    pub test: Option<Expr>,
    pub body: Vec<Stmt>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct StmtIf {
    pub test: Box<Expr>,
    pub body: Vec<Stmt>,
    /// `elif`/`else if` and `else` arms, flattened.
    pub elif_else_clauses: Vec<ElifElseClause>,
    pub range: TextRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForKind {
    /// Python `for x in xs`.
    In,
    /// JavaScript `for (const x of xs)`.
    Of,
    /// JavaScript `for (const k in obj)`: over keys.
    Keys,
    /// A counting loop, `for (let i = 0; i < n; i++)`: `target` is the counter
    /// and `iter` the condition.
    Count,
}

#[derive(Clone, Debug)]
pub struct StmtFor {
    pub target: Box<Expr>,
    pub iter: Box<Expr>,
    pub body: Vec<Stmt>,
    pub orelse: Vec<Stmt>,
    pub kind: ForKind,
    pub is_async: bool,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct StmtWhile {
    pub test: Box<Expr>,
    pub body: Vec<Stmt>,
    pub orelse: Vec<Stmt>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct WithItem {
    pub context_expr: Expr,
    pub optional_vars: Option<Box<Expr>>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct StmtWith {
    pub items: Vec<WithItem>,
    pub body: Vec<Stmt>,
    pub is_async: bool,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct ExceptHandler {
    pub type_: Option<Box<Expr>>,
    pub name: Option<String>,
    pub body: Vec<Stmt>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct StmtTry {
    pub body: Vec<Stmt>,
    pub handlers: Vec<ExceptHandler>,
    pub orelse: Vec<Stmt>,
    pub finalbody: Vec<Stmt>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct MatchCase {
    /// The patterns as written (`case {"k": v}`; `case 'GET': case 'HEAD':`
    /// sharing one body); empty for a JavaScript `default:`.
    pub patterns: Vec<TextRange>,
    /// What the pattern binds from the subject, as an assignment target
    /// (`Some(user)` binds `user`; `Point { x, y }` binds `(x, y)`).
    pub binds: Option<Box<Expr>>,
    /// The variant a pattern unpacks, when it binds one value from it
    /// (`Command::Get(cmd)` → `Get`): with the subject's type, says what the
    /// value is.
    pub variant: Option<String>,
    pub guard: Option<Box<Expr>>,
    pub body: Vec<Stmt>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct StmtMatch {
    pub subject: Box<Expr>,
    pub cases: Vec<MatchCase>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct Alias {
    pub name: String,
    pub asname: Option<String>,
    pub range: TextRange,
}

/// How an import names its module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleStyle {
    /// Python's dotted names, relative by `level`.
    Dotted,
    /// A path or package specifier (`./util`, `react`, `@/lib/db`).
    Path,
}

/// Imports a whole module: Python `import a.b as c`; JavaScript
/// `import * as c from "m"` and `const c = require("m")` (`name` is the module);
/// Rust `use crate::db;` and `mod db;`.
#[derive(Clone, Debug)]
pub struct StmtImport {
    pub names: Vec<Alias>,
    pub style: ModuleStyle,
    /// The project file the module resolves to, when the front end resolved it.
    pub resolved: Option<String>,
    /// Declares a child module (Rust `mod db;`): binds its name without
    /// depending on it.
    pub declaration: bool,
    pub range: TextRange,
}

/// Imports names from a module: Python `from m import x`; JavaScript
/// `import x, { y as z } from "m"` (a default import is the name `default`).
#[derive(Clone, Debug)]
pub struct StmtImportFrom {
    pub module: Option<String>,
    pub names: Vec<Alias>,
    pub level: u32,
    pub style: ModuleStyle,
    /// The project file the module resolves to, when the front end resolved it.
    pub resolved: Option<String>,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct StmtTypeAlias {
    pub name: Box<Expr>,
    pub value: Box<Expr>,
    pub range: TextRange,
}

/// Names a module exports: `(local name, exported name)`, the exported name
/// `default` for a default export.
#[derive(Clone, Debug)]
pub struct StmtExport {
    pub names: Vec<(String, String)>,
    pub range: TextRange,
}

impl Ranged for Stmt {
    fn range(&self) -> TextRange {
        match self {
            Stmt::Expr(x) => x.range,
            Stmt::Assign(x) => x.range,
            Stmt::AnnAssign(x) => x.range,
            Stmt::AugAssign(x) => x.range,
            Stmt::Return(x) => x.range,
            Stmt::Raise(x) => x.range,
            Stmt::Delete(x) => x.range,
            Stmt::Assert(x) => x.range,
            Stmt::Pass(x) | Stmt::Break(x) | Stmt::Continue(x) | Stmt::Other(x) => x.range,
            Stmt::Global(x) | Stmt::Nonlocal(x) => x.range,
            Stmt::If(x) => x.range,
            Stmt::For(x) => x.range,
            Stmt::While(x) => x.range,
            Stmt::With(x) => x.range,
            Stmt::Try(x) => x.range,
            Stmt::Match(x) => x.range,
            Stmt::FunctionDef(x) => x.range,
            Stmt::ClassDef(x) => x.range,
            Stmt::Import(x) => x.range,
            Stmt::ImportFrom(x) => x.range,
            Stmt::TypeAlias(x) => x.range,
            Stmt::Export(x) => x.range,
        }
    }
}

ranged!(StmtExpr, StmtAssign, StmtAnnAssign, StmtAugAssign, StmtReturn, StmtRaise, StmtDelete, StmtAssert, StmtNames, ElifElseClause, StmtIf, StmtFor, StmtWhile, WithItem, StmtWith, ExceptHandler, StmtTry, MatchCase, StmtMatch, Alias, StmtImport, StmtImportFrom, StmtTypeAlias, StmtExport);
