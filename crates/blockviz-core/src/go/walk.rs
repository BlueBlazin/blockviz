//! Visiting a Go syntax tree in go/ast's `Walk` order: a node, then its
//! children in source order the way walk.go lists them (doc comments first,
//! line comments last). `File::comments` is not visited, as in go/ast.
//!
//! The walk keeps its own stack, so trees as deep as the parser allows
//! (long `a + b + …` chains included) cost no call stack.

use super::ast::*;
use crate::code::syntax::{Ranged, TextRange};

/// A reference to any node, named after its go/ast type.
#[derive(Clone, Copy, Debug)]
pub enum Node<'a> {
    Comment(&'a Comment),
    CommentGroup(&'a CommentGroup),
    Field(&'a Field),
    FieldList(&'a FieldList),
    BadExpr(&'a BadExpr),
    Ident(&'a Ident),
    Ellipsis(&'a Ellipsis),
    BasicLit(&'a BasicLit),
    FuncLit(&'a FuncLit),
    CompositeLit(&'a CompositeLit),
    ParenExpr(&'a ParenExpr),
    SelectorExpr(&'a SelectorExpr),
    IndexExpr(&'a IndexExpr),
    IndexListExpr(&'a IndexListExpr),
    SliceExpr(&'a SliceExpr),
    TypeAssertExpr(&'a TypeAssertExpr),
    CallExpr(&'a CallExpr),
    StarExpr(&'a StarExpr),
    UnaryExpr(&'a UnaryExpr),
    BinaryExpr(&'a BinaryExpr),
    KeyValueExpr(&'a KeyValueExpr),
    ArrayType(&'a ArrayType),
    StructType(&'a StructType),
    FuncType(&'a FuncType),
    InterfaceType(&'a InterfaceType),
    MapType(&'a MapType),
    ChanType(&'a ChanType),
    BadStmt(&'a BadStmt),
    DeclStmt(&'a DeclStmt),
    EmptyStmt(&'a EmptyStmt),
    LabeledStmt(&'a LabeledStmt),
    ExprStmt(&'a ExprStmt),
    SendStmt(&'a SendStmt),
    IncDecStmt(&'a IncDecStmt),
    AssignStmt(&'a AssignStmt),
    GoStmt(&'a GoStmt),
    DeferStmt(&'a DeferStmt),
    ReturnStmt(&'a ReturnStmt),
    BranchStmt(&'a BranchStmt),
    BlockStmt(&'a BlockStmt),
    IfStmt(&'a IfStmt),
    CaseClause(&'a CaseClause),
    SwitchStmt(&'a SwitchStmt),
    TypeSwitchStmt(&'a TypeSwitchStmt),
    CommClause(&'a CommClause),
    SelectStmt(&'a SelectStmt),
    ForStmt(&'a ForStmt),
    RangeStmt(&'a RangeStmt),
    ImportSpec(&'a ImportSpec),
    ValueSpec(&'a ValueSpec),
    TypeSpec(&'a TypeSpec),
    BadDecl(&'a BadDecl),
    GenDecl(&'a GenDecl),
    FuncDecl(&'a FuncDecl),
    File(&'a File),
}

impl<'a> From<&'a Expr> for Node<'a> {
    fn from(x: &'a Expr) -> Self {
        match x {
            Expr::Bad(x) => Node::BadExpr(x),
            Expr::Ident(x) => Node::Ident(x),
            Expr::Ellipsis(x) => Node::Ellipsis(x),
            Expr::BasicLit(x) => Node::BasicLit(x),
            Expr::FuncLit(x) => Node::FuncLit(x),
            Expr::CompositeLit(x) => Node::CompositeLit(x),
            Expr::Paren(x) => Node::ParenExpr(x),
            Expr::Selector(x) => Node::SelectorExpr(x),
            Expr::Index(x) => Node::IndexExpr(x),
            Expr::IndexList(x) => Node::IndexListExpr(x),
            Expr::Slice(x) => Node::SliceExpr(x),
            Expr::TypeAssert(x) => Node::TypeAssertExpr(x),
            Expr::Call(x) => Node::CallExpr(x),
            Expr::Star(x) => Node::StarExpr(x),
            Expr::Unary(x) => Node::UnaryExpr(x),
            Expr::Binary(x) => Node::BinaryExpr(x),
            Expr::KeyValue(x) => Node::KeyValueExpr(x),
            Expr::ArrayType(x) => Node::ArrayType(x),
            Expr::StructType(x) => Node::StructType(x),
            Expr::FuncType(x) => Node::FuncType(x),
            Expr::InterfaceType(x) => Node::InterfaceType(x),
            Expr::MapType(x) => Node::MapType(x),
            Expr::ChanType(x) => Node::ChanType(x),
        }
    }
}

impl<'a> From<&'a Stmt> for Node<'a> {
    fn from(s: &'a Stmt) -> Self {
        match s {
            Stmt::Bad(s) => Node::BadStmt(s),
            Stmt::Decl(s) => Node::DeclStmt(s),
            Stmt::Empty(s) => Node::EmptyStmt(s),
            Stmt::Labeled(s) => Node::LabeledStmt(s),
            Stmt::Expr(s) => Node::ExprStmt(s),
            Stmt::Send(s) => Node::SendStmt(s),
            Stmt::IncDec(s) => Node::IncDecStmt(s),
            Stmt::Assign(s) => Node::AssignStmt(s),
            Stmt::Go(s) => Node::GoStmt(s),
            Stmt::Defer(s) => Node::DeferStmt(s),
            Stmt::Return(s) => Node::ReturnStmt(s),
            Stmt::Branch(s) => Node::BranchStmt(s),
            Stmt::Block(s) => Node::BlockStmt(s),
            Stmt::If(s) => Node::IfStmt(s),
            Stmt::CaseClause(s) => Node::CaseClause(s),
            Stmt::Switch(s) => Node::SwitchStmt(s),
            Stmt::TypeSwitch(s) => Node::TypeSwitchStmt(s),
            Stmt::CommClause(s) => Node::CommClause(s),
            Stmt::Select(s) => Node::SelectStmt(s),
            Stmt::For(s) => Node::ForStmt(s),
            Stmt::Range(s) => Node::RangeStmt(s),
        }
    }
}

impl<'a> From<&'a Decl> for Node<'a> {
    fn from(d: &'a Decl) -> Self {
        match d {
            Decl::Bad(d) => Node::BadDecl(d),
            Decl::Gen(d) => Node::GenDecl(d),
            Decl::Func(d) => Node::FuncDecl(d),
        }
    }
}

impl<'a> From<&'a Spec> for Node<'a> {
    fn from(s: &'a Spec) -> Self {
        match s {
            Spec::Import(s) => Node::ImportSpec(s),
            Spec::Value(s) => Node::ValueSpec(s),
            Spec::Type(s) => Node::TypeSpec(s),
        }
    }
}

macro_rules! node_from {
    ($($t:ident),* $(,)?) => {
        $(impl<'a> From<&'a $t> for Node<'a> {
            fn from(n: &'a $t) -> Self {
                Node::$t(n)
            }
        })*
    };
}

node_from!(
    Comment, CommentGroup, Field, FieldList, BadExpr, Ident, Ellipsis, BasicLit, FuncLit, CompositeLit, ParenExpr, SelectorExpr, IndexExpr, IndexListExpr, SliceExpr, TypeAssertExpr, CallExpr, StarExpr, UnaryExpr, BinaryExpr, KeyValueExpr, ArrayType, StructType, FuncType, InterfaceType, MapType, ChanType, BadStmt, DeclStmt, EmptyStmt, LabeledStmt, ExprStmt, SendStmt, IncDecStmt, AssignStmt, GoStmt, DeferStmt, ReturnStmt, BranchStmt, BlockStmt, IfStmt, CaseClause, SwitchStmt, TypeSwitchStmt, CommClause, SelectStmt, ForStmt, RangeStmt, ImportSpec, ValueSpec, TypeSpec, BadDecl, GenDecl, FuncDecl, File,
);

impl Node<'_> {
    /// The go/ast type name (`CallExpr`, `IfStmt`, …).
    pub fn kind(&self) -> &'static str {
        match self {
            Node::Comment(_) => "Comment",
            Node::CommentGroup(_) => "CommentGroup",
            Node::Field(_) => "Field",
            Node::FieldList(_) => "FieldList",
            Node::BadExpr(_) => "BadExpr",
            Node::Ident(_) => "Ident",
            Node::Ellipsis(_) => "Ellipsis",
            Node::BasicLit(_) => "BasicLit",
            Node::FuncLit(_) => "FuncLit",
            Node::CompositeLit(_) => "CompositeLit",
            Node::ParenExpr(_) => "ParenExpr",
            Node::SelectorExpr(_) => "SelectorExpr",
            Node::IndexExpr(_) => "IndexExpr",
            Node::IndexListExpr(_) => "IndexListExpr",
            Node::SliceExpr(_) => "SliceExpr",
            Node::TypeAssertExpr(_) => "TypeAssertExpr",
            Node::CallExpr(_) => "CallExpr",
            Node::StarExpr(_) => "StarExpr",
            Node::UnaryExpr(_) => "UnaryExpr",
            Node::BinaryExpr(_) => "BinaryExpr",
            Node::KeyValueExpr(_) => "KeyValueExpr",
            Node::ArrayType(_) => "ArrayType",
            Node::StructType(_) => "StructType",
            Node::FuncType(_) => "FuncType",
            Node::InterfaceType(_) => "InterfaceType",
            Node::MapType(_) => "MapType",
            Node::ChanType(_) => "ChanType",
            Node::BadStmt(_) => "BadStmt",
            Node::DeclStmt(_) => "DeclStmt",
            Node::EmptyStmt(_) => "EmptyStmt",
            Node::LabeledStmt(_) => "LabeledStmt",
            Node::ExprStmt(_) => "ExprStmt",
            Node::SendStmt(_) => "SendStmt",
            Node::IncDecStmt(_) => "IncDecStmt",
            Node::AssignStmt(_) => "AssignStmt",
            Node::GoStmt(_) => "GoStmt",
            Node::DeferStmt(_) => "DeferStmt",
            Node::ReturnStmt(_) => "ReturnStmt",
            Node::BranchStmt(_) => "BranchStmt",
            Node::BlockStmt(_) => "BlockStmt",
            Node::IfStmt(_) => "IfStmt",
            Node::CaseClause(_) => "CaseClause",
            Node::SwitchStmt(_) => "SwitchStmt",
            Node::TypeSwitchStmt(_) => "TypeSwitchStmt",
            Node::CommClause(_) => "CommClause",
            Node::SelectStmt(_) => "SelectStmt",
            Node::ForStmt(_) => "ForStmt",
            Node::RangeStmt(_) => "RangeStmt",
            Node::ImportSpec(_) => "ImportSpec",
            Node::ValueSpec(_) => "ValueSpec",
            Node::TypeSpec(_) => "TypeSpec",
            Node::BadDecl(_) => "BadDecl",
            Node::GenDecl(_) => "GenDecl",
            Node::FuncDecl(_) => "FuncDecl",
            Node::File(_) => "File",
        }
    }
}

impl Ranged for Node<'_> {
    fn range(&self) -> TextRange {
        match self {
            Node::Comment(n) => n.range,
            Node::CommentGroup(n) => n.range,
            Node::Field(n) => n.range,
            Node::FieldList(n) => n.range,
            Node::BadExpr(n) => n.range,
            Node::Ident(n) => n.range,
            Node::Ellipsis(n) => n.range,
            Node::BasicLit(n) => n.range,
            Node::FuncLit(n) => n.range,
            Node::CompositeLit(n) => n.range,
            Node::ParenExpr(n) => n.range,
            Node::SelectorExpr(n) => n.range,
            Node::IndexExpr(n) => n.range,
            Node::IndexListExpr(n) => n.range,
            Node::SliceExpr(n) => n.range,
            Node::TypeAssertExpr(n) => n.range,
            Node::CallExpr(n) => n.range,
            Node::StarExpr(n) => n.range,
            Node::UnaryExpr(n) => n.range,
            Node::BinaryExpr(n) => n.range,
            Node::KeyValueExpr(n) => n.range,
            Node::ArrayType(n) => n.range,
            Node::StructType(n) => n.range,
            Node::FuncType(n) => n.range,
            Node::InterfaceType(n) => n.range,
            Node::MapType(n) => n.range,
            Node::ChanType(n) => n.range,
            Node::BadStmt(n) => n.range,
            Node::DeclStmt(n) => n.range,
            Node::EmptyStmt(n) => n.range,
            Node::LabeledStmt(n) => n.range,
            Node::ExprStmt(n) => n.range,
            Node::SendStmt(n) => n.range,
            Node::IncDecStmt(n) => n.range,
            Node::AssignStmt(n) => n.range,
            Node::GoStmt(n) => n.range,
            Node::DeferStmt(n) => n.range,
            Node::ReturnStmt(n) => n.range,
            Node::BranchStmt(n) => n.range,
            Node::BlockStmt(n) => n.range,
            Node::IfStmt(n) => n.range,
            Node::CaseClause(n) => n.range,
            Node::SwitchStmt(n) => n.range,
            Node::TypeSwitchStmt(n) => n.range,
            Node::CommClause(n) => n.range,
            Node::SelectStmt(n) => n.range,
            Node::ForStmt(n) => n.range,
            Node::RangeStmt(n) => n.range,
            Node::ImportSpec(n) => n.range,
            Node::ValueSpec(n) => n.range,
            Node::TypeSpec(n) => n.range,
            Node::BadDecl(n) => n.range,
            Node::GenDecl(n) => n.range,
            Node::FuncDecl(n) => n.range,
            Node::File(n) => n.range,
        }
    }
}

/// The direct children of `node`, in walk order.
pub fn children<'a>(node: Node<'a>, mut f: impl FnMut(Node<'a>)) {
    macro_rules! one {
        ($x:expr) => {
            f(Node::from($x))
        };
    }
    macro_rules! opt {
        ($x:expr) => {
            if let Some(x) = $x {
                f(Node::from(x))
            }
        };
    }
    macro_rules! all {
        ($xs:expr) => {
            for x in $xs {
                f(Node::from(x))
            }
        };
    }
    match node {
        Node::Comment(_) | Node::BadExpr(_) | Node::Ident(_) | Node::BasicLit(_) | Node::BadStmt(_) | Node::EmptyStmt(_) | Node::BadDecl(_) => {}
        Node::CommentGroup(n) => all!(&n.list),
        Node::Field(n) => {
            opt!(&n.doc);
            all!(&n.names);
            one!(&n.typ);
            opt!(&n.tag);
            opt!(&n.comment);
        }
        Node::FieldList(n) => all!(&n.list),
        Node::Ellipsis(n) => opt!(&n.elt),
        Node::FuncLit(n) => {
            one!(&n.typ);
            one!(&n.body);
        }
        Node::CompositeLit(n) => {
            opt!(&n.typ);
            all!(&n.elts);
        }
        Node::ParenExpr(n) => one!(&n.x),
        Node::SelectorExpr(n) => {
            one!(&n.x);
            one!(&n.sel);
        }
        Node::IndexExpr(n) => {
            one!(&n.x);
            one!(&n.index);
        }
        Node::IndexListExpr(n) => {
            one!(&n.x);
            all!(&n.indices);
        }
        Node::SliceExpr(n) => {
            one!(&n.x);
            opt!(&n.low);
            opt!(&n.high);
            opt!(&n.max);
        }
        Node::TypeAssertExpr(n) => {
            one!(&n.x);
            opt!(&n.typ);
        }
        Node::CallExpr(n) => {
            one!(&n.fun);
            all!(&n.args);
        }
        Node::StarExpr(n) => one!(&n.x),
        Node::UnaryExpr(n) => one!(&n.x),
        Node::BinaryExpr(n) => {
            one!(&n.x);
            one!(&n.y);
        }
        Node::KeyValueExpr(n) => {
            one!(&n.key);
            one!(&n.value);
        }
        Node::ArrayType(n) => {
            opt!(&n.len);
            one!(&n.elt);
        }
        Node::StructType(n) => one!(&n.fields),
        Node::FuncType(n) => {
            opt!(&n.type_params);
            one!(&n.params);
            opt!(&n.results);
        }
        Node::InterfaceType(n) => one!(&n.methods),
        Node::MapType(n) => {
            one!(&n.key);
            one!(&n.value);
        }
        Node::ChanType(n) => one!(&n.value),
        Node::DeclStmt(n) => one!(&n.decl),
        Node::LabeledStmt(n) => {
            one!(&n.label);
            one!(&n.stmt);
        }
        Node::ExprStmt(n) => one!(&n.x),
        Node::SendStmt(n) => {
            one!(&n.chan);
            one!(&n.value);
        }
        Node::IncDecStmt(n) => one!(&n.x),
        Node::AssignStmt(n) => {
            all!(&n.lhs);
            all!(&n.rhs);
        }
        Node::GoStmt(n) => one!(&n.call),
        Node::DeferStmt(n) => one!(&n.call),
        Node::ReturnStmt(n) => all!(&n.results),
        Node::BranchStmt(n) => opt!(&n.label),
        Node::BlockStmt(n) => all!(&n.list),
        Node::IfStmt(n) => {
            opt!(&n.init);
            one!(&n.cond);
            one!(&n.body);
            opt!(&n.else_);
        }
        Node::CaseClause(n) => {
            all!(&n.list);
            all!(&n.body);
        }
        Node::SwitchStmt(n) => {
            opt!(&n.init);
            opt!(&n.tag);
            one!(&n.body);
        }
        Node::TypeSwitchStmt(n) => {
            opt!(&n.init);
            one!(&n.assign);
            one!(&n.body);
        }
        Node::CommClause(n) => {
            opt!(&n.comm);
            all!(&n.body);
        }
        Node::SelectStmt(n) => one!(&n.body),
        Node::ForStmt(n) => {
            opt!(&n.init);
            opt!(&n.cond);
            opt!(&n.post);
            one!(&n.body);
        }
        Node::RangeStmt(n) => {
            opt!(&n.key);
            opt!(&n.value);
            one!(&n.x);
            one!(&n.body);
        }
        Node::ImportSpec(n) => {
            opt!(&n.doc);
            opt!(&n.name);
            one!(&n.path);
            opt!(&n.comment);
        }
        Node::ValueSpec(n) => {
            opt!(&n.doc);
            all!(&n.names);
            opt!(&n.typ);
            all!(&n.values);
            opt!(&n.comment);
        }
        Node::TypeSpec(n) => {
            opt!(&n.doc);
            one!(&n.name);
            opt!(&n.type_params);
            one!(&n.typ);
            opt!(&n.comment);
        }
        Node::GenDecl(n) => {
            opt!(&n.doc);
            all!(&n.specs);
        }
        Node::FuncDecl(n) => {
            opt!(&n.doc);
            opt!(&n.recv);
            one!(&n.name);
            one!(&n.typ);
            opt!(&n.body);
        }
        Node::File(n) => {
            opt!(&n.doc);
            opt!(&n.name);
            all!(&n.decls);
        }
    }
}

/// What `walk` calls for each node.
pub trait Visitor<'a> {
    /// Before the node's children; `false` skips them (and `leave`).
    fn enter(&mut self, node: Node<'a>) -> bool;
    /// After the node's children.
    fn leave(&mut self, _node: Node<'a>) {}
}

/// Visits `root` and everything below it in go/ast's `Walk` order.
pub fn walk<'a>(visitor: &mut impl Visitor<'a>, root: Node<'a>) {
    enum Step<'a> {
        Enter(Node<'a>),
        Leave(Node<'a>),
    }
    let mut stack = vec![Step::Enter(root)];
    let mut kids = vec![];
    while let Some(step) = stack.pop() {
        match step {
            Step::Enter(node) => {
                if visitor.enter(node) {
                    stack.push(Step::Leave(node));
                    children(node, |c| kids.push(c));
                    stack.extend(kids.drain(..).rev().map(Step::Enter));
                }
            }
            Step::Leave(node) => visitor.leave(node),
        }
    }
}

/// Calls `f` on `root` and every node below it, in go/ast's `Inspect`
/// order; when `f` returns `false` the node's children are skipped.
pub fn inspect<'a>(root: Node<'a>, f: impl FnMut(Node<'a>) -> bool) {
    struct Inspector<F>(F);
    impl<'a, F: FnMut(Node<'a>) -> bool> Visitor<'a> for Inspector<F> {
        fn enter(&mut self, node: Node<'a>) -> bool {
            (self.0)(node)
        }
    }
    walk(&mut Inspector(f), root);
}
