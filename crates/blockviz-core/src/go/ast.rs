//! The Go syntax tree, node for node go/ast's.
//!
//! Every node has its `range`: go/ast's `Pos()`..`End()` as byte offsets,
//! with go/ast's conventions kept (a block ends past its `}`, an implicit
//! `EmptyStmt` is empty, a `return` without results spans the keyword).
//! Positions go/ast keeps beside a node's start (closing brackets, operators,
//! `:=`) are fields; positions that always equal `range.start()` (keywords,
//! `*`, `[` of an array type) are not.
//!
//! In code with syntax errors, a missing token is `None` where go/ast has
//! `NoPos`; where go/ast points at whatever token stood there instead (a `)`
//! that is not there), so does this tree, but the node ends before that
//! token rather than one byte into it. A missing name is `_` with an empty
//! range. Every node lies within its parent (doc and line comments aside).
//!
//! Doc and line comments are copies of the groups in `File::comments`.

pub use super::scan::Token;
use crate::code::syntax::{Ranged, TextRange, TextSize};

macro_rules! ranged {
    ($($t:ty),* $(,)?) => {
        $(impl Ranged for $t {
            fn range(&self) -> TextRange {
                self.range
            }
        })*
    };
}

// ----- comments ----------------------------------------------------------------

/// A `//` or `/* */` comment. `text` is the comment as written, markers
/// included, without carriage returns (go/ast's `Text`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub range: TextRange,
    pub text: String,
}

/// Comments with no tokens and no blank line between them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommentGroup {
    pub range: TextRange,
    pub list: Vec<Comment>,
}

impl CommentGroup {
    /// The text of the comments, as go/ast's `CommentGroup.Text`: markers,
    /// the first space of line comments, directives (`//go:generate`,
    /// `//line`) and leading and trailing blank lines removed; runs of blank
    /// lines become one; newline-terminated unless empty.
    pub fn text(&self) -> String {
        let mut lines: Vec<&str> = Vec::with_capacity(10);
        for c in &self.list {
            let t = c.text.as_str();
            let body = match t.as_bytes().get(1) {
                Some(b'/') => {
                    let t = &t[2..];
                    if let Some(rest) = t.strip_prefix(' ') {
                        rest
                    } else if is_directive(t) {
                        continue;
                    } else {
                        t
                    }
                }
                _ => t.get(2..t.len().saturating_sub(2).max(2)).unwrap_or(""),
            };
            lines.extend(body.split('\n').map(|l| l.trim_end_matches([' ', '\t', '\n', '\r'])));
        }
        let mut out: Vec<&str> = Vec::with_capacity(lines.len() + 1);
        for line in lines {
            if !line.is_empty() || out.last().is_some_and(|l| !l.is_empty()) {
                out.push(line);
            }
        }
        if out.last().is_some_and(|l| !l.is_empty()) {
            out.push("");
        }
        out.join("\n")
    }
}

/// `line …` and `name:args` (`go:noinline`, `export:x`), after the `//`.
fn is_directive(c: &str) -> bool {
    if c.starts_with("line ") {
        return true;
    }
    let Some(colon) = c.find(':') else { return false };
    if colon == 0 || colon + 1 >= c.len() {
        return false;
    }
    c.as_bytes()[..=colon + 1].iter().enumerate().all(|(i, &b)| i == colon || b.is_ascii_lowercase() || b.is_ascii_digit())
}

// ----- fields ------------------------------------------------------------------

/// A struct field, an interface method or embedded element, a parameter or
/// result, or a type parameter. `names` is empty for unnamed parameters and
/// embedded fields.
#[derive(Clone, Debug)]
pub struct Field {
    pub range: TextRange,
    pub doc: Option<CommentGroup>,
    pub names: Vec<Ident>,
    pub typ: Expr,
    pub tag: Option<BasicLit>,
    pub comment: Option<CommentGroup>,
}

/// Fields between `(` `)`, `{` `}` or `[` `]`; a result type written without
/// parentheses has neither.
#[derive(Clone, Debug)]
pub struct FieldList {
    pub range: TextRange,
    pub opening: Option<TextSize>,
    pub list: Vec<Field>,
    pub closing: Option<TextSize>,
}

impl FieldList {
    /// Parameters or fields counted by name (`a, b int` is two).
    pub fn num_fields(&self) -> usize {
        self.list.iter().map(|f| f.names.len().max(1)).sum()
    }
}

// ----- expressions and types ---------------------------------------------------

#[derive(Clone, Debug)]
pub enum Expr {
    Bad(BadExpr),
    Ident(Ident),
    Ellipsis(Box<Ellipsis>),
    BasicLit(Box<BasicLit>),
    FuncLit(Box<FuncLit>),
    CompositeLit(Box<CompositeLit>),
    Paren(Box<ParenExpr>),
    Selector(Box<SelectorExpr>),
    Index(Box<IndexExpr>),
    IndexList(Box<IndexListExpr>),
    Slice(Box<SliceExpr>),
    TypeAssert(Box<TypeAssertExpr>),
    Call(Box<CallExpr>),
    Star(Box<StarExpr>),
    Unary(Box<UnaryExpr>),
    Binary(Box<BinaryExpr>),
    KeyValue(Box<KeyValueExpr>),
    ArrayType(Box<ArrayType>),
    StructType(Box<StructType>),
    FuncType(Box<FuncType>),
    InterfaceType(Box<InterfaceType>),
    MapType(Box<MapType>),
    ChanType(Box<ChanType>),
}

impl Ranged for Expr {
    fn range(&self) -> TextRange {
        match self {
            Expr::Bad(x) => x.range,
            Expr::Ident(x) => x.range,
            Expr::Ellipsis(x) => x.range,
            Expr::BasicLit(x) => x.range,
            Expr::FuncLit(x) => x.range,
            Expr::CompositeLit(x) => x.range,
            Expr::Paren(x) => x.range,
            Expr::Selector(x) => x.range,
            Expr::Index(x) => x.range,
            Expr::IndexList(x) => x.range,
            Expr::Slice(x) => x.range,
            Expr::TypeAssert(x) => x.range,
            Expr::Call(x) => x.range,
            Expr::Star(x) => x.range,
            Expr::Unary(x) => x.range,
            Expr::Binary(x) => x.range,
            Expr::KeyValue(x) => x.range,
            Expr::ArrayType(x) => x.range,
            Expr::StructType(x) => x.range,
            Expr::FuncType(x) => x.range,
            Expr::InterfaceType(x) => x.range,
            Expr::MapType(x) => x.range,
            Expr::ChanType(x) => x.range,
        }
    }
}

/// Where an expression could not be parsed.
#[derive(Clone, Debug)]
pub struct BadExpr {
    pub range: TextRange,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ident {
    pub range: TextRange,
    pub name: String,
}

/// `...` as a variadic parameter's type (`...T`) or an array length (`[...]T`).
#[derive(Clone, Debug)]
pub struct Ellipsis {
    pub range: TextRange,
    pub elt: Option<Expr>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LitKind {
    Int,
    Float,
    Imag,
    Char,
    String,
}

/// A number, rune or string literal. `value` is the literal as written
/// (`0x7f`, `'\n'`, `"a\tb"`, `` `raw` ``), quotes included and nothing
/// unescaped; raw strings lose their carriage returns, as in go/ast.
#[derive(Clone, Debug)]
pub struct BasicLit {
    pub range: TextRange,
    pub kind: LitKind,
    pub value: String,
}

#[derive(Clone, Debug)]
pub struct FuncLit {
    pub range: TextRange,
    pub typ: FuncType,
    pub body: BlockStmt,
}

/// `T{…}`, or `{…}` with the type elided inside another composite literal.
#[derive(Clone, Debug)]
pub struct CompositeLit {
    pub range: TextRange,
    pub typ: Option<Expr>,
    pub lbrace: TextSize,
    pub elts: Vec<Expr>,
    pub rbrace: TextSize,
}

#[derive(Clone, Debug)]
pub struct ParenExpr {
    pub range: TextRange,
    pub lparen: TextSize,
    pub x: Expr,
    pub rparen: TextSize,
}

/// `x.sel`: a field or method, or a name qualified by its package.
#[derive(Clone, Debug)]
pub struct SelectorExpr {
    pub range: TextRange,
    pub x: Expr,
    pub sel: Ident,
}

/// `x[index]`: an index, or an instantiation with one type argument.
#[derive(Clone, Debug)]
pub struct IndexExpr {
    pub range: TextRange,
    pub x: Expr,
    pub lbrack: TextSize,
    pub index: Expr,
    pub rbrack: TextSize,
}

/// `x[a, b]`: an instantiation with several type arguments.
#[derive(Clone, Debug)]
pub struct IndexListExpr {
    pub range: TextRange,
    pub x: Expr,
    pub lbrack: TextSize,
    pub indices: Vec<Expr>,
    pub rbrack: TextSize,
}

/// `x[low:high]` or `x[low:high:max]`.
#[derive(Clone, Debug)]
pub struct SliceExpr {
    pub range: TextRange,
    pub x: Expr,
    pub lbrack: TextSize,
    pub low: Option<Expr>,
    pub high: Option<Expr>,
    pub max: Option<Expr>,
    pub slice3: bool,
    pub rbrack: TextSize,
}

/// `x.(T)`, or `x.(type)` (no `typ`) in a type switch.
#[derive(Clone, Debug)]
pub struct TypeAssertExpr {
    pub range: TextRange,
    pub x: Expr,
    pub lparen: TextSize,
    pub typ: Option<Expr>,
    pub rparen: TextSize,
}

/// A call or a conversion; `ellipsis` is the `...` of `f(xs...)`.
#[derive(Clone, Debug)]
pub struct CallExpr {
    pub range: TextRange,
    pub fun: Expr,
    pub lparen: TextSize,
    pub args: Vec<Expr>,
    pub ellipsis: Option<TextSize>,
    pub rparen: TextSize,
}

/// `*x`: a dereference or a pointer type.
#[derive(Clone, Debug)]
pub struct StarExpr {
    pub range: TextRange,
    pub x: Expr,
}

/// A unary operator (`-x`, `!x`, `<-ch`, `&x`, `^x`, `~T`); `*` is `StarExpr`.
#[derive(Clone, Debug)]
pub struct UnaryExpr {
    pub range: TextRange,
    pub op: Token,
    pub x: Expr,
}

/// A binary operator, including `|` between the terms of a type constraint.
#[derive(Clone, Debug)]
pub struct BinaryExpr {
    pub range: TextRange,
    pub x: Expr,
    pub op_pos: TextSize,
    pub op: Token,
    pub y: Expr,
}

/// `key: value` in a composite literal.
#[derive(Clone, Debug)]
pub struct KeyValueExpr {
    pub range: TextRange,
    pub key: Expr,
    pub colon: TextSize,
    pub value: Expr,
}

/// `[len]elt`, `[...]elt` (an `Ellipsis` length) or `[]elt` (no length).
#[derive(Clone, Debug)]
pub struct ArrayType {
    pub range: TextRange,
    pub len: Option<Expr>,
    pub elt: Expr,
}

#[derive(Clone, Debug)]
pub struct StructType {
    pub range: TextRange,
    pub fields: FieldList,
}

/// A signature. `func` is the keyword's position, absent for interface
/// methods; a declared function's type parameters are here too.
#[derive(Clone, Debug)]
pub struct FuncType {
    pub range: TextRange,
    pub func: Option<TextSize>,
    pub type_params: Option<FieldList>,
    pub params: FieldList,
    pub results: Option<FieldList>,
}

/// Methods (named fields with a `FuncType`) and embedded elements
/// (unnamed fields: types, `~T`, unions as `|` binary expressions).
#[derive(Clone, Debug)]
pub struct InterfaceType {
    pub range: TextRange,
    pub methods: FieldList,
}

#[derive(Clone, Debug)]
pub struct MapType {
    pub range: TextRange,
    pub key: Expr,
    pub value: Expr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChanDir {
    /// `chan<- T`
    Send,
    /// `<-chan T`
    Recv,
    /// `chan T`
    Both,
}

#[derive(Clone, Debug)]
pub struct ChanType {
    pub range: TextRange,
    /// The `<-`, if any.
    pub arrow: Option<TextSize>,
    pub dir: ChanDir,
    pub value: Expr,
}

// ----- statements --------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum Stmt {
    Bad(BadStmt),
    Decl(Box<DeclStmt>),
    Empty(EmptyStmt),
    Labeled(Box<LabeledStmt>),
    Expr(Box<ExprStmt>),
    Send(Box<SendStmt>),
    IncDec(Box<IncDecStmt>),
    Assign(Box<AssignStmt>),
    Go(Box<GoStmt>),
    Defer(Box<DeferStmt>),
    Return(Box<ReturnStmt>),
    Branch(Box<BranchStmt>),
    Block(Box<BlockStmt>),
    If(Box<IfStmt>),
    /// Only in the body of a `SwitchStmt` or `TypeSwitchStmt`.
    CaseClause(Box<CaseClause>),
    Switch(Box<SwitchStmt>),
    TypeSwitch(Box<TypeSwitchStmt>),
    /// Only in the body of a `SelectStmt`.
    CommClause(Box<CommClause>),
    Select(Box<SelectStmt>),
    For(Box<ForStmt>),
    Range(Box<RangeStmt>),
}

impl Ranged for Stmt {
    fn range(&self) -> TextRange {
        match self {
            Stmt::Bad(s) => s.range,
            Stmt::Decl(s) => s.range,
            Stmt::Empty(s) => s.range,
            Stmt::Labeled(s) => s.range,
            Stmt::Expr(s) => s.range,
            Stmt::Send(s) => s.range,
            Stmt::IncDec(s) => s.range,
            Stmt::Assign(s) => s.range,
            Stmt::Go(s) => s.range,
            Stmt::Defer(s) => s.range,
            Stmt::Return(s) => s.range,
            Stmt::Branch(s) => s.range,
            Stmt::Block(s) => s.range,
            Stmt::If(s) => s.range,
            Stmt::CaseClause(s) => s.range,
            Stmt::Switch(s) => s.range,
            Stmt::TypeSwitch(s) => s.range,
            Stmt::CommClause(s) => s.range,
            Stmt::Select(s) => s.range,
            Stmt::For(s) => s.range,
            Stmt::Range(s) => s.range,
        }
    }
}

/// Where a statement could not be parsed.
#[derive(Clone, Debug)]
pub struct BadStmt {
    pub range: TextRange,
}

/// A `const`, `type` or `var` declaration inside a function.
#[derive(Clone, Debug)]
pub struct DeclStmt {
    pub range: TextRange,
    pub decl: GenDecl,
}

/// `;` on its own, or (`implicit`) a statement left out before `}` or a
/// newline; then the range is empty, at the `}` or the newline.
#[derive(Clone, Debug)]
pub struct EmptyStmt {
    pub range: TextRange,
    pub implicit: bool,
}

#[derive(Clone, Debug)]
pub struct LabeledStmt {
    pub range: TextRange,
    pub label: Ident,
    pub colon: TextSize,
    pub stmt: Stmt,
}

#[derive(Clone, Debug)]
pub struct ExprStmt {
    pub range: TextRange,
    pub x: Expr,
}

/// `ch <- value`
#[derive(Clone, Debug)]
pub struct SendStmt {
    pub range: TextRange,
    pub chan: Expr,
    pub arrow: TextSize,
    pub value: Expr,
}

/// `x++` or `x--` (`tok` is `Inc` or `Dec`).
#[derive(Clone, Debug)]
pub struct IncDecStmt {
    pub range: TextRange,
    pub x: Expr,
    pub tok_pos: TextSize,
    pub tok: Token,
}

/// `lhs = rhs`, `lhs := rhs` (`Define`) or `lhs op= rhs`.
#[derive(Clone, Debug)]
pub struct AssignStmt {
    pub range: TextRange,
    pub lhs: Vec<Expr>,
    pub tok_pos: TextSize,
    pub tok: Token,
    pub rhs: Vec<Expr>,
}

#[derive(Clone, Debug)]
pub struct GoStmt {
    pub range: TextRange,
    pub call: CallExpr,
}

#[derive(Clone, Debug)]
pub struct DeferStmt {
    pub range: TextRange,
    pub call: CallExpr,
}

#[derive(Clone, Debug)]
pub struct ReturnStmt {
    pub range: TextRange,
    pub results: Vec<Expr>,
}

/// `break`, `continue`, `goto` or `fallthrough`, with its label if any.
#[derive(Clone, Debug)]
pub struct BranchStmt {
    pub range: TextRange,
    pub tok: Token,
    pub label: Option<Ident>,
}

/// `{ … }`. `rbrace` is absent when the block is cut off by a syntax error.
#[derive(Clone, Debug)]
pub struct BlockStmt {
    pub range: TextRange,
    pub list: Vec<Stmt>,
    pub rbrace: Option<TextSize>,
}

/// `if init; cond { body } else …`; `else_` is another `IfStmt` or a block.
#[derive(Clone, Debug)]
pub struct IfStmt {
    pub range: TextRange,
    pub init: Option<Stmt>,
    pub cond: Expr,
    pub body: BlockStmt,
    pub else_: Option<Stmt>,
}

/// `case list:` (expressions, or types in a type switch) or `default:`
/// (empty `list`).
#[derive(Clone, Debug)]
pub struct CaseClause {
    pub range: TextRange,
    pub list: Vec<Expr>,
    pub colon: TextSize,
    pub body: Vec<Stmt>,
}

impl CaseClause {
    pub fn is_default(&self) -> bool {
        self.list.is_empty()
    }
}

/// An expression switch; the body holds only `CaseClause`s.
#[derive(Clone, Debug)]
pub struct SwitchStmt {
    pub range: TextRange,
    pub init: Option<Stmt>,
    pub tag: Option<Expr>,
    pub body: BlockStmt,
}

/// A type switch: `assign` is `x := y.(type)` or `y.(type)`; the body holds
/// only `CaseClause`s.
#[derive(Clone, Debug)]
pub struct TypeSwitchStmt {
    pub range: TextRange,
    pub init: Option<Stmt>,
    pub assign: Stmt,
    pub body: BlockStmt,
}

/// `case comm:` (a send, or a receive possibly assigned) or `default:`
/// (no `comm`).
#[derive(Clone, Debug)]
pub struct CommClause {
    pub range: TextRange,
    pub comm: Option<Stmt>,
    pub colon: TextSize,
    pub body: Vec<Stmt>,
}

/// The body holds only `CommClause`s.
#[derive(Clone, Debug)]
pub struct SelectStmt {
    pub range: TextRange,
    pub body: BlockStmt,
}

#[derive(Clone, Debug)]
pub struct ForStmt {
    pub range: TextRange,
    pub init: Option<Stmt>,
    pub cond: Option<Expr>,
    pub post: Option<Stmt>,
    pub body: BlockStmt,
}

/// `for key, value := range x`. `tok` is `Define` or `Assign`, absent with
/// the key in `for range x`.
#[derive(Clone, Debug)]
pub struct RangeStmt {
    pub range: TextRange,
    pub key: Option<Expr>,
    pub value: Option<Expr>,
    pub tok_pos: Option<TextSize>,
    pub tok: Option<Token>,
    /// The `range` keyword.
    pub range_pos: TextSize,
    pub x: Expr,
    pub body: BlockStmt,
}

// ----- declarations ------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum Spec {
    Import(ImportSpec),
    Value(ValueSpec),
    Type(TypeSpec),
}

impl Ranged for Spec {
    fn range(&self) -> TextRange {
        match self {
            Spec::Import(s) => s.range,
            Spec::Value(s) => s.range,
            Spec::Type(s) => s.range,
        }
    }
}

/// `name "path"`; `name` may be `.` or `_`. A missing path is an empty
/// literal.
#[derive(Clone, Debug)]
pub struct ImportSpec {
    pub range: TextRange,
    pub doc: Option<CommentGroup>,
    pub name: Option<Ident>,
    pub path: BasicLit,
    pub comment: Option<CommentGroup>,
}

/// A `const` or `var` spec: `a, b T = x, y`, any of type and values omitted.
#[derive(Clone, Debug)]
pub struct ValueSpec {
    pub range: TextRange,
    pub doc: Option<CommentGroup>,
    pub names: Vec<Ident>,
    pub typ: Option<Expr>,
    pub values: Vec<Expr>,
    pub comment: Option<CommentGroup>,
}

/// `Name[P any] T`, or an alias `Name = T` (`assign` is the `=`).
#[derive(Clone, Debug)]
pub struct TypeSpec {
    pub range: TextRange,
    pub doc: Option<CommentGroup>,
    pub name: Ident,
    pub type_params: Option<FieldList>,
    pub assign: Option<TextSize>,
    pub typ: Expr,
    pub comment: Option<CommentGroup>,
}

#[derive(Clone, Debug)]
pub enum Decl {
    Bad(BadDecl),
    Gen(GenDecl),
    Func(Box<FuncDecl>),
}

impl Ranged for Decl {
    fn range(&self) -> TextRange {
        match self {
            Decl::Bad(d) => d.range,
            Decl::Gen(d) => d.range,
            Decl::Func(d) => d.range,
        }
    }
}

/// Where a declaration could not be parsed.
#[derive(Clone, Debug)]
pub struct BadDecl {
    pub range: TextRange,
}

/// An `import`, `const`, `type` or `var` declaration (`tok`), grouped in
/// parentheses when `lparen` is set. Specs are `ImportSpec`s for imports,
/// `TypeSpec`s for types and `ValueSpec`s otherwise.
#[derive(Clone, Debug)]
pub struct GenDecl {
    pub range: TextRange,
    pub doc: Option<CommentGroup>,
    pub tok: Token,
    pub lparen: Option<TextSize>,
    pub specs: Vec<Spec>,
    pub rparen: Option<TextSize>,
}

/// A function or (with `recv`) a method; no body for functions implemented
/// elsewhere (assembly).
#[derive(Clone, Debug)]
pub struct FuncDecl {
    pub range: TextRange,
    pub doc: Option<CommentGroup>,
    pub recv: Option<FieldList>,
    pub name: Ident,
    pub typ: FuncType,
    pub body: Option<BlockStmt>,
}

/// A source file. Snippets without a package clause parse too (`package`
/// and `name` absent): then the range runs from the first declaration.
#[derive(Clone, Debug)]
pub struct File {
    pub range: TextRange,
    pub doc: Option<CommentGroup>,
    /// The `package` keyword.
    pub package: Option<TextSize>,
    pub name: Option<Ident>,
    pub decls: Vec<Decl>,
    /// Every comment in the file, in order.
    pub comments: Vec<CommentGroup>,
}

impl File {
    /// The import specs, in order (go/ast's `File.Imports`).
    pub fn imports(&self) -> impl Iterator<Item = &ImportSpec> {
        self.decls.iter().filter_map(|d| match d {
            Decl::Gen(g) if g.tok == Token::Import => Some(g),
            _ => None,
        }).flat_map(|g| g.specs.iter()).filter_map(|s| match s {
            Spec::Import(i) => Some(i),
            _ => None,
        })
    }
}

ranged!(
    Comment, CommentGroup, Field, FieldList, BadExpr, Ident, Ellipsis, BasicLit, FuncLit, CompositeLit, ParenExpr, SelectorExpr, IndexExpr, IndexListExpr, SliceExpr, TypeAssertExpr, CallExpr, StarExpr, UnaryExpr, BinaryExpr, KeyValueExpr, ArrayType, StructType, FuncType, InterfaceType, MapType, ChanType, BadStmt, DeclStmt, EmptyStmt, LabeledStmt, ExprStmt, SendStmt, IncDecStmt, AssignStmt, GoStmt, DeferStmt, ReturnStmt, BranchStmt, BlockStmt, IfStmt, CaseClause, SwitchStmt, TypeSwitchStmt, CommClause, SelectStmt, ForStmt, RangeStmt, ImportSpec, ValueSpec, TypeSpec, BadDecl, GenDecl, FuncDecl, File,
);
