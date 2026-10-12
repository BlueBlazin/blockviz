//! The parser: a port of go/parser, function for function, so the tree and
//! every position in it match go/ast's. It follows Go 1.18's go/parser (the
//! version the oracle tests run) and takes the grammar to Go 1.24: `~` is a
//! unary operator, and `type T[P *C|~int]` is a type parameter list as
//! go/parser now decides it.
//!
//! Error recovery keeps more than go/parser's: a broken statement skips to
//! its line's end rather than to the next statement keyword, a construct left
//! open stops at a declaration keyword starting a line (`func Name`, `type`),
//! and parsing never stops early. Nesting is capped (`MAX_NESTING`,
//! `MAX_DEPTH`): past it the file is reported and treated as ending there.
//!
//! Parsers of single constructs that recursive paths pass through are kept
//! out of line (`#[inline(never)]`): inlined into the dispatching function,
//! their locals would weigh on every level of nesting.

use super::ast::*;
use super::scan::{strip_cr, Scanner, Token};
use super::SyntaxError;
use crate::code::syntax::{Ranged, TextRange, TextSize};

/// How deeply parentheses, blocks, types and literals may nest. Real code
/// stays under 300; the cap keeps parsing within a WebAssembly stack.
pub const MAX_NESTING: u32 = 1000;
/// How deep the tree may get, counting what the parser builds in loops
/// (`a + b + …`, `x.f().g()…`, `else if …`): the longest chain in the Go
/// distribution is a 7,100-term string concatenation.
pub const MAX_DEPTH: u32 = 10_000;

pub(super) fn parse_file(src: &str) -> (File, Vec<SyntaxError>) {
    let mut p = Parser::new(src);
    let file = p.parse_file();
    let mut errors = p.errors;
    errors.sort_by_key(|e| e.offset);
    (file, errors)
}

fn rng(start: u32, end: u32) -> TextRange {
    TextRange::new(TextSize::from(start), TextSize::from(end.max(start)))
}

fn lo(x: &impl Ranged) -> u32 {
    x.range().start().into()
}

fn hi(x: &impl Ranged) -> u32 {
    x.range().end().into()
}

fn ts(x: u32) -> TextSize {
    TextSize::from(x)
}

fn bad(start: u32, end: u32) -> Expr {
    Expr::Bad(BadExpr { range: rng(start, end) })
}

fn take(x: &mut Expr) -> Expr {
    std::mem::replace(x, Expr::Bad(BadExpr { range: TextRange::default() }))
}

fn is_stmt_start(t: Token) -> bool {
    use Token::*;
    matches!(t, Break | Const | Continue | Defer | Fallthrough | For | Go | Goto | If | Return | Select | Switch | Type | Var)
}

fn is_decl_start(t: Token) -> bool {
    matches!(t, Token::Const | Token::Type | Token::Var | Token::Func | Token::Import)
}

fn is_expr_end(t: Token) -> bool {
    use Token::*;
    matches!(t, Comma | Colon | Semicolon | RParen | RBrack | RBrace)
}

fn unparen(x: &Expr) -> &Expr {
    let mut x = x;
    while let Expr::Paren(p) = x {
        x = &p.x;
    }
    x
}

/// A (possibly parenthesized) type literal or type-set term: what makes
/// `type T[P *X]` a type parameter list rather than an array length. Go 1.18
/// looked through `*`; later versions also take `~T` and unions.
fn is_type_elem(x: &Expr) -> bool {
    let mut x = x;
    loop {
        match x {
            Expr::ArrayType(_) | Expr::StructType(_) | Expr::FuncType(_) | Expr::InterfaceType(_) | Expr::MapType(_) | Expr::ChanType(_) => return true,
            Expr::Star(s) => x = &s.x,
            Expr::Paren(p) => x = &p.x,
            Expr::Unary(u) => return u.op == Token::Tilde,
            Expr::Binary(b) => {
                if is_type_elem(&b.y) {
                    return true;
                }
                x = &b.x;
            }
            _ => return false,
        }
    }
}

fn is_type_switch_assert(x: &Expr) -> bool {
    matches!(x, Expr::TypeAssert(a) if a.typ.is_none())
}

fn pack_index_expr(x: Expr, lbrack: u32, mut exprs: Vec<Expr>, rbrack: u32, end: u32) -> Expr {
    let range = rng(lo(&x), end);
    if exprs.len() == 1 {
        let index = exprs.pop().unwrap_or_else(|| bad(lbrack, rbrack));
        Expr::Index(Box::new(IndexExpr { range, x, lbrack: ts(lbrack), index, rbrack: ts(rbrack) }))
    } else {
        Expr::IndexList(Box::new(IndexListExpr { range, x, lbrack: ts(lbrack), indices: exprs, rbrack: ts(rbrack) }))
    }
}

fn field(names: Vec<Ident>, typ: Expr, tag: Option<BasicLit>) -> Field {
    let start = names.first().map(lo).unwrap_or(lo(&typ));
    let end = tag.as_ref().map(hi).unwrap_or(hi(&typ));
    Field { range: rng(start, end), doc: None, names, typ, tag, comment: None }
}

fn func_type(func: Option<u32>, type_params: Option<FieldList>, params: FieldList, results: Option<FieldList>) -> FuncType {
    let start = func.unwrap_or(lo(&params));
    let end = results.as_ref().map(hi).unwrap_or(hi(&params));
    FuncType { range: rng(start, end), func: func.map(ts), type_params, params, results }
}

fn binary(x: Expr, op_pos: u32, op: Token, y: Expr) -> Expr {
    Expr::Binary(Box::new(BinaryExpr { range: rng(lo(&x), hi(&y)), x, op_pos: ts(op_pos), op, y }))
}

/// Whether an import path is a non-empty string of graphic characters
/// other than spaces and ``!"#$%&'()*,:;<=>?[\]^{|}` ``, as go/parser checks.
fn is_valid_import(lit: &str) -> bool {
    let Some(s) = unquote(lit) else { return false };
    let s = String::from_utf8_lossy(&s);
    !s.is_empty() && s.chars().all(|c| is_graphic(c) && !c.is_whitespace() && !"!\"#$%&'()*,:;<=>?[\\]^{|}`\u{fffd}".contains(c))
}

/// Unicode's graphic characters, approximately: not control, format or
/// private-use characters.
fn is_graphic(c: char) -> bool {
    !(c.is_control()
        || matches!(c, '\u{ad}' | '\u{600}'..='\u{605}' | '\u{61c}' | '\u{6dd}' | '\u{70f}' | '\u{180e}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}' | '\u{fff9}'..='\u{fffb}' | '\u{e000}'..='\u{f8ff}' | '\u{f0000}'..))
}

/// The bytes a string literal denotes, or `None` if it is malformed.
fn unquote(lit: &str) -> Option<Vec<u8>> {
    let b = lit.as_bytes();
    if b.len() < 2 || b[0] != b[b.len() - 1] {
        return None;
    }
    let body = &b[1..b.len() - 1];
    if b[0] == b'`' {
        return (!body.contains(&b'`')).then(|| body.iter().copied().filter(|&c| c != b'\r').collect());
    }
    if b[0] != b'"' {
        return None;
    }
    let mut out = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        let c = body[i];
        if c == b'"' || c == b'\n' {
            return None;
        }
        if c != b'\\' {
            out.push(c);
            i += 1;
            continue;
        }
        let e = *body.get(i + 1)?;
        i += 2;
        let simple = match e {
            b'a' => Some(7),
            b'b' => Some(8),
            b'f' => Some(12),
            b'n' => Some(b'\n'),
            b'r' => Some(b'\r'),
            b't' => Some(b'\t'),
            b'v' => Some(11),
            b'\\' => Some(b'\\'),
            b'"' => Some(b'"'),
            _ => None,
        };
        if let Some(s) = simple {
            out.push(s);
            continue;
        }
        let (n, radix) = match e {
            b'0'..=b'7' => {
                i -= 1;
                (3, 8)
            }
            b'x' => (2, 16),
            b'u' => (4, 16),
            b'U' => (8, 16),
            _ => return None,
        };
        let digits = std::str::from_utf8(body.get(i..i + n)?).ok()?;
        let v = u32::from_str_radix(digits, radix).ok()?;
        i += n;
        match e {
            b'x' => out.push(v as u8),
            b'0'..=b'7' => out.push(u8::try_from(v).ok()?),
            _ => out.extend(char::from_u32(v)?.to_string().bytes()),
        }
    }
    Some(out)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Basic,
    LabelOk,
    RangeOk,
}

/// What `parse_simple_stmt` found: a statement, or the head of a range
/// clause (`k, v := range x`).
enum Simple {
    Stmt(Stmt),
    Range { lhs: Vec<Expr>, tok_pos: u32, tok: Token, range_pos: u32, x: Expr },
}

impl Simple {
    /// The statement; only `for` asks for range clauses, so other callers
    /// never see one.
    fn into_stmt(self) -> Stmt {
        match self {
            Simple::Stmt(s) => s,
            Simple::Range { tok_pos, x, .. } => Stmt::Bad(BadStmt { range: rng(tok_pos, hi(&x)) }),
        }
    }
}

struct Parser<'s> {
    src: &'s str,
    scanner: Scanner<'s>,
    tok: Token,
    pos: u32,
    end: u32,
    /// The current token's line, as go/token numbers lines.
    line: i64,

    comments: Vec<CommentGroup>,
    lead_comment: Option<usize>,
    line_comment: Option<usize>,

    errors: Vec<SyntaxError>,
    /// Line starts, for discarding further errors on a line; built on the
    /// first error.
    line_starts: Vec<u32>,
    sync_pos: u32,
    sync_cnt: u32,

    /// Below 0 in a control clause (`if x {`), where `{` opens the body
    /// rather than a composite literal.
    expr_lev: i32,
    in_rhs: bool,

    nest: u32,
    depth: u32,
    bailed: bool,
}

impl<'s> Parser<'s> {
    fn new(src: &'s str) -> Self {
        let mut p = Parser {
            src,
            scanner: Scanner::new(src),
            tok: Token::Illegal,
            pos: 0,
            end: 0,
            // Before the first token: no line, so a comment opening the
            // file is never a line comment.
            line: 0,
            comments: vec![],
            lead_comment: None,
            line_comment: None,
            errors: vec![],
            line_starts: vec![],
            sync_pos: 0,
            sync_cnt: 0,
            expr_lev: 0,
            in_rhs: false,
            nest: 0,
            depth: 0,
            bailed: false,
        };
        p.next();
        p
    }

    // ----- tokens and comments -------------------------------------------------

    fn next0(&mut self) {
        if self.bailed {
            self.tok = Token::Eof;
            self.end = self.pos;
            return;
        }
        let lx = self.scanner.scan();
        if !self.scanner.errors.is_empty() {
            self.errors.append(&mut self.scanner.errors);
        }
        self.tok = lx.token;
        self.pos = lx.range.start().into();
        self.end = lx.range.end().into();
        self.line = self.scanner.tok_line;
    }

    fn lit(&self) -> &'s str {
        &self.src[self.pos as usize..self.end as usize]
    }

    fn is_implicit_semi(&self) -> bool {
        self.tok == Token::Semicolon && self.end == self.pos
    }

    fn consume_comment(&mut self) -> (Comment, i64) {
        let text = self.lit();
        let mut endline = self.line;
        if text.as_bytes()[1] == b'*' {
            endline += text.bytes().filter(|&c| c == b'\n').count() as i64;
        }
        let text = if text.contains('\r') { strip_cr(text, text.as_bytes()[1] == b'*') } else { text.to_string() };
        let comment = Comment { range: rng(self.pos, self.end), text };
        self.next0();
        (comment, endline)
    }

    /// Comments on consecutive lines (at most `n` lines apart), as one group.
    fn consume_comment_group(&mut self, n: i64) -> (usize, i64) {
        let mut list = vec![];
        let mut endline = self.line;
        while self.tok == Token::Comment && self.line <= endline + n {
            let (c, e) = self.consume_comment();
            list.push(c);
            endline = e;
        }
        let range = rng(lo(&list[0]), hi(&list[list.len() - 1]));
        self.comments.push(CommentGroup { range, list });
        (self.comments.len() - 1, endline)
    }

    /// Advances to the next non-comment token, grouping the comments passed
    /// and noting a line comment (after a token on its line) and a lead
    /// comment (ending on the line before the next token).
    fn next(&mut self) {
        self.lead_comment = None;
        self.line_comment = None;
        let prev = self.line;
        self.next0();
        if self.tok != Token::Comment {
            return;
        }
        let mut comment = None;
        if self.line == prev {
            let (group, endline) = self.consume_comment_group(0);
            comment = Some(group);
            if self.line != endline || self.tok == Token::Eof {
                self.line_comment = Some(group);
            }
        }
        let mut endline = None;
        while self.tok == Token::Comment {
            let (group, e) = self.consume_comment_group(1);
            comment = Some(group);
            endline = Some(e);
        }
        if endline.is_some_and(|e| e + 1 == self.line) {
            self.lead_comment = comment;
        }
    }

    fn group(&self, i: Option<usize>) -> Option<CommentGroup> {
        i.map(|i| self.comments[i].clone())
    }

    // ----- errors and recovery -------------------------------------------------

    fn line_of(&mut self, pos: u32) -> usize {
        if self.line_starts.is_empty() {
            self.line_starts.push(0);
            self.line_starts.extend(self.src.bytes().enumerate().filter(|&(_, c)| c == b'\n').map(|(i, _)| i as u32 + 1));
        }
        self.line_starts.partition_point(|&s| s <= pos)
    }

    /// Records an error, unless one is already recorded on its line (likely
    /// a consequence).
    fn error(&mut self, pos: u32, msg: impl Into<String>) {
        if self.bailed {
            return;
        }
        if let Some(last) = self.errors.last().map(|e| e.offset) {
            if self.line_of(last) == self.line_of(pos) {
                return;
            }
        }
        self.errors.push(SyntaxError { offset: pos, message: msg.into() });
    }

    fn error_expected(&mut self, pos: u32, what: &str) {
        let mut msg = format!("expected {what}");
        if pos == self.pos {
            if self.is_implicit_semi() {
                msg.push_str(", found newline");
            } else if self.tok.is_literal() {
                msg.push_str(", found ");
                msg.push_str(self.lit());
            } else {
                msg.push_str(&format!(", found '{}'", self.tok.as_str()));
            }
        }
        self.error(pos, msg);
    }

    /// A `func`, `type`, `var`, `const` or `import` starting a line: where a
    /// construct left open by an error ends, so the declarations after it
    /// survive.
    fn at_decl_start(&self) -> bool {
        let p = self.pos as usize;
        if p != 0 && self.src.as_bytes()[p - 1] != b'\n' {
            return false;
        }
        match self.tok {
            Token::Import | Token::Type | Token::Var | Token::Const => true,
            Token::Func => self.func_decl_follows(),
            _ => false,
        }
    }

    /// Whether the `func` here declares something (`func Name`, or a method
    /// `func (r T) Name(`), which no statement or expression can do.
    fn func_decl_follows(&self) -> bool {
        let mut scanner = self.scanner.clone();
        let mut budget = 64;
        let mut next = move || loop {
            if budget == 0 {
                return Token::Eof;
            }
            budget -= 1;
            let t = scanner.scan().token;
            if t != Token::Comment {
                return t;
            }
        };
        match next() {
            Token::Ident => true,
            Token::LParen => {
                let mut depth = 1;
                while depth > 0 {
                    match next() {
                        Token::LParen => depth += 1,
                        Token::RParen => depth -= 1,
                        Token::Eof => return false,
                        _ => {}
                    }
                }
                next() == Token::Ident && matches!(next(), Token::LParen | Token::LBrack)
            }
            _ => false,
        }
    }

    /// A token a missing one must not be traded for: it closes an enclosing
    /// construct, ends the file, or starts the next declaration.
    fn keep_for_enclosing(&self) -> bool {
        matches!(self.tok, Token::RParen | Token::RBrack | Token::RBrace | Token::Eof) || self.at_decl_start()
    }

    fn expect(&mut self, tok: Token) -> u32 {
        let pos = self.pos;
        if self.tok != tok {
            self.error_expected(pos, &format!("'{}'", tok.as_str()));
            if self.keep_for_enclosing() {
                return pos;
            }
        }
        self.next();
        pos
    }

    /// `expect` for a one-byte token a construct may end with (a bracket, a
    /// colon): its position, and where such a construct ends. go/ast ends it
    /// past the token; when the token is missing this ends it where the token
    /// should be, short of whatever stands there instead.
    fn close(&mut self, tok: Token) -> (u32, u32) {
        let found = self.tok == tok;
        let pos = self.expect(tok);
        (pos, if found { pos + 1 } else { pos })
    }

    /// `close` for a list's closing token, naming the likelier mistake of a
    /// missing comma before a newline.
    fn close_list(&mut self, tok: Token, context: &str) -> (u32, u32) {
        if self.tok != tok && self.is_implicit_semi() {
            self.error(self.pos, format!("missing ',' before newline in {context}"));
            self.next();
        }
        self.close(tok)
    }

    /// `expect`, with no position when the token is missing.
    fn expect2(&mut self, tok: Token) -> Option<u32> {
        let pos = (self.tok == tok).then_some(self.pos);
        if pos.is_none() {
            self.error_expected(self.pos, &format!("'{}'", tok.as_str()));
            if self.keep_for_enclosing() {
                return None;
            }
        }
        self.next();
        pos
    }

    fn expect_semi(&mut self) {
        if self.tok == Token::RParen || self.tok == Token::RBrace {
            return;
        }
        match self.tok {
            Token::Comma => {
                self.error_expected(self.pos, "';'");
                self.next();
            }
            Token::Semicolon => self.next(),
            _ => {
                self.error_expected(self.pos, "';'");
                self.skip_stmt();
            }
        }
    }

    /// Whether a list goes on past this element: at a comma, or (reporting
    /// the missing comma) at anything but the list's end.
    fn at_comma(&mut self, context: &str, follow: Token) -> bool {
        if self.tok == Token::Comma {
            return true;
        }
        if self.tok != follow {
            let newline = if self.is_implicit_semi() { " before newline" } else { "" };
            self.error(self.pos, format!("missing ','{newline} in {context}"));
            return !self.keep_for_enclosing();
        }
        false
    }

    /// Skips to a token in `to` (or a declaration start), making progress
    /// unless no other recovery has since the last stop here.
    fn advance(&mut self, to: fn(Token) -> bool) {
        while self.tok != Token::Eof {
            if to(self.tok) || self.at_decl_start() {
                if self.pos == self.sync_pos && self.sync_cnt < 10 {
                    self.sync_cnt += 1;
                    return;
                }
                if self.pos > self.sync_pos {
                    self.sync_pos = self.pos;
                    self.sync_cnt = 0;
                    return;
                }
            }
            self.next();
        }
    }

    /// Recovery from a broken statement: skips the rest of it, through the
    /// semicolon (usually a newline) that ends it, but not past a closing
    /// bracket of an enclosing construct, a statement keyword or the next
    /// declaration. Brackets opened on the way are skipped whole.
    fn skip_stmt(&mut self) {
        let mut depth = 0u32;
        loop {
            match self.tok {
                Token::Eof => return,
                Token::LParen | Token::LBrack | Token::LBrace => depth += 1,
                Token::RParen | Token::RBrack | Token::RBrace => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                }
                Token::Semicolon if depth == 0 => {
                    self.next();
                    return;
                }
                t if depth == 0 && is_stmt_start(t) => return,
                _ => {}
            }
            if self.at_decl_start() {
                return;
            }
            self.next();
        }
    }

    /// Recovery from a missing operand: `skip_stmt`, stopping before the `,`
    /// `:` or `;` that ends the expression.
    fn skip_expr(&mut self) {
        let mut depth = 0u32;
        loop {
            match self.tok {
                Token::Eof => return,
                Token::Comma | Token::Colon | Token::Semicolon if depth == 0 => return,
                Token::LParen | Token::LBrack | Token::LBrace => depth += 1,
                Token::RParen | Token::RBrack | Token::RBrace => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                }
                t if depth == 0 && is_stmt_start(t) => return,
                _ => {}
            }
            if self.at_decl_start() {
                return;
            }
            self.next();
        }
    }

    /// After a loop step: whether the loop may go on. A step that consumed
    /// nothing skips the token it stopped at, unless that ends the file or
    /// starts a declaration.
    fn progressed(&mut self, before: u32) -> bool {
        if self.pos != before {
            return true;
        }
        if self.tok == Token::Eof || self.at_decl_start() {
            return false;
        }
        self.next();
        true
    }

    // ----- nesting -------------------------------------------------------------

    /// One level deeper, unless that is past the caps (then the file is cut
    /// short here).
    fn enter(&mut self) -> bool {
        if self.nest >= MAX_NESTING || self.depth >= MAX_DEPTH {
            self.bail();
            return false;
        }
        self.nest += 1;
        self.depth += 1;
        true
    }

    fn leave(&mut self) {
        self.nest -= 1;
        self.depth -= 1;
    }

    /// One more link in a chain built by a loop.
    fn chain(&mut self) {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.bail();
        }
    }

    /// Too deeply nested: reports it and treats the file as ending here.
    fn bail(&mut self) {
        if !self.bailed {
            self.errors.push(SyntaxError { offset: self.pos, message: "exceeded max nesting depth".into() });
            self.bailed = true;
            self.tok = Token::Eof;
            self.end = self.pos;
        }
    }

    // ----- identifiers ---------------------------------------------------------

    fn parse_ident(&mut self) -> Ident {
        let pos = self.pos;
        if self.tok == Token::Ident {
            let ident = Ident { range: rng(pos, self.end), name: self.lit().to_string() };
            self.next();
            ident
        } else {
            self.expect(Token::Ident);
            Ident { range: rng(pos, pos), name: "_".into() }
        }
    }

    fn parse_ident_list(&mut self) -> Vec<Ident> {
        let mut list = vec![self.parse_ident()];
        while self.tok == Token::Comma {
            self.next();
            list.push(self.parse_ident());
        }
        list
    }

    // ----- common productions --------------------------------------------------

    fn parse_expr_list(&mut self) -> Vec<Expr> {
        let x = self.parse_expr();
        let mut list = vec![self.check_expr(x)];
        while self.tok == Token::Comma {
            self.next();
            let x = self.parse_expr();
            list.push(self.check_expr(x));
        }
        list
    }

    fn parse_list(&mut self, in_rhs: bool) -> Vec<Expr> {
        let old = self.in_rhs;
        self.in_rhs = in_rhs;
        let list = self.parse_expr_list();
        self.in_rhs = old;
        list
    }

    // ----- types ---------------------------------------------------------------

    fn parse_type(&mut self) -> Expr {
        if let Some(t) = self.try_ident_or_type() {
            return t;
        }
        let pos = self.pos;
        self.error_expected(pos, "type");
        self.advance(is_expr_end);
        bad(pos, self.pos)
    }

    fn parse_qualified_ident(&mut self, ident: Ident) -> Expr {
        let typ = self.parse_type_name(Some(ident));
        if self.tok == Token::LBrack {
            return self.parse_type_instance(typ);
        }
        typ
    }

    fn parse_type_name(&mut self, ident: Option<Ident>) -> Expr {
        let ident = ident.unwrap_or_else(|| self.parse_ident());
        if self.tok == Token::Period {
            self.next();
            let sel = self.parse_ident();
            return Expr::Selector(Box::new(SelectorExpr { range: rng(lo(&ident), hi(&sel)), x: Expr::Ident(ident), sel }));
        }
        Expr::Ident(ident)
    }

    /// The rest of an array or slice type after `[`; `len` is the length
    /// when already parsed.
    #[inline(never)]
    fn parse_array_type(&mut self, lbrack: u32, len: Option<Expr>) -> Expr {
        let len = match len {
            Some(len) => Some(len),
            None => {
                self.expr_lev += 1;
                let len = if self.tok == Token::Ellipsis {
                    let e = Expr::Ellipsis(Box::new(Ellipsis { range: rng(self.pos, self.pos + 3), elt: None }));
                    self.next();
                    Some(e)
                } else if self.tok != Token::RBrack {
                    Some(self.parse_rhs())
                } else {
                    None
                };
                self.expr_lev -= 1;
                len
            }
        };
        if self.tok == Token::Comma {
            self.error(self.pos, "unexpected comma; expecting ]");
            self.next();
        }
        self.expect(Token::RBrack);
        let elt = self.parse_type();
        Expr::ArrayType(Box::new(ArrayType { range: rng(lbrack, hi(&elt)), len, elt }))
    }

    /// After a name in a field or parameter: `[` starts either the name's
    /// array type (`x [N]T`) or type arguments of an embedded or unnamed type
    /// (`T[P]`), in which case no name is returned.
    fn parse_array_field_or_type_instance(&mut self, x: Ident) -> (Option<Ident>, Expr) {
        let lbrack = self.expect(Token::LBrack);
        let mut args = vec![];
        if self.tok != Token::RBrack {
            self.expr_lev += 1;
            args.push(self.parse_rhs_or_type());
            while self.tok == Token::Comma {
                self.next();
                args.push(self.parse_rhs_or_type());
            }
            self.expr_lev -= 1;
        }
        let (rbrack, end) = self.close(Token::RBrack);
        if args.is_empty() {
            let elt = self.parse_type();
            return (Some(x), Expr::ArrayType(Box::new(ArrayType { range: rng(lbrack, hi(&elt)), len: None, elt })));
        }
        if args.len() == 1 {
            if let Some(elt) = self.try_ident_or_type() {
                let len = args.pop();
                return (Some(x), Expr::ArrayType(Box::new(ArrayType { range: rng(lbrack, hi(&elt)), len, elt })));
            }
        }
        (None, pack_index_expr(Expr::Ident(x), lbrack, args, rbrack, end))
    }

    fn parse_field_decl(&mut self) -> Field {
        let doc = self.group(self.lead_comment);
        let mut names = vec![];
        let typ;
        if self.tok == Token::Ident {
            let name = self.parse_ident();
            if matches!(self.tok, Token::Period | Token::String | Token::Semicolon | Token::RBrace) {
                // An embedded type.
                typ = if self.tok == Token::Period { self.parse_qualified_ident(name) } else { Expr::Ident(name) };
            } else {
                names.push(name);
                while self.tok == Token::Comma {
                    self.next();
                    names.push(self.parse_ident());
                }
                if names.len() == 1 && self.tok == Token::LBrack {
                    let name = names.pop().unwrap();
                    let (name, t) = self.parse_array_field_or_type_instance(name);
                    names.extend(name);
                    typ = t;
                } else {
                    typ = self.parse_type();
                }
            }
        } else {
            typ = self.parse_type();
        }
        let tag = (self.tok == Token::String).then(|| {
            let tag = self.basic_lit();
            self.next();
            tag
        });
        self.expect_semi();
        let mut f = field(names, typ, tag);
        f.doc = doc;
        f.comment = self.group(self.line_comment);
        f
    }

    #[inline(never)]
    fn parse_struct_type(&mut self) -> Expr {
        let pos = self.expect(Token::Struct);
        let lbrace = self.expect(Token::LBrace);
        let mut list = vec![];
        while matches!(self.tok, Token::Ident | Token::Mul | Token::LParen) {
            list.push(self.parse_field_decl());
        }
        let (rbrace, end) = self.close(Token::RBrace);
        let fields = FieldList { range: rng(lbrace, end), opening: Some(ts(lbrace)), list, closing: Some(ts(rbrace)) };
        Expr::StructType(Box::new(StructType { range: rng(pos, end), fields }))
    }

    #[inline(never)]
    fn parse_pointer_type(&mut self) -> Expr {
        let star = self.expect(Token::Mul);
        let x = self.parse_type();
        Expr::Star(Box::new(StarExpr { range: rng(star, hi(&x)), x }))
    }

    fn parse_dots_type(&mut self) -> Expr {
        let pos = self.expect(Token::Ellipsis);
        let elt = self.parse_type();
        Expr::Ellipsis(Box::new(Ellipsis { range: rng(pos, hi(&elt)), elt: Some(elt) }))
    }

    /// One parameter: a name, a type, or both. `name` is a name already
    /// parsed; in type parameter lists (`type_sets`) types may be unions.
    fn parse_param_decl(&mut self, name: Option<Ident>, type_sets: bool) -> (Option<Ident>, Option<Expr>) {
        use Token::*;
        if name.is_none() && type_sets && self.tok == Tilde {
            return (None, Some(self.embedded_elem(None)));
        }
        let mut typ = None;
        let mut name = name;
        if name.is_some() || self.tok == Ident {
            let n = name.take().unwrap_or_else(|| self.parse_ident());
            match self.tok {
                Ident | Mul | Arrow | Func | Chan | Map | Struct | Interface | LParen => {
                    name = Some(n);
                    typ = Some(self.parse_type());
                }
                LBrack => {
                    let (n, t) = self.parse_array_field_or_type_instance(n);
                    name = n;
                    typ = Some(t);
                }
                Ellipsis => return (Some(n), Some(self.parse_dots_type())),
                Period => typ = Some(self.parse_qualified_ident(n)),
                Tilde if type_sets => return (Some(n), Some(self.embedded_elem(None))),
                Or if type_sets => return (None, Some(self.embedded_elem(Some(Expr::Ident(n))))),
                _ => name = Some(n),
            }
        } else {
            match self.tok {
                Mul | Arrow | Func | LBrack | Chan | Map | Struct | Interface | LParen => typ = Some(self.parse_type()),
                Ellipsis => return (None, Some(self.parse_dots_type())),
                _ => {
                    self.error_expected(self.pos, "')'");
                    self.advance(is_expr_end);
                }
            }
        }
        if type_sets && self.tok == Or {
            if let Some(t) = typ.take() {
                typ = Some(self.embedded_elem(Some(t)));
            }
        }
        (name, typ)
    }

    /// Parameters up to `closing` (`)`, or `]` for type parameters). A name
    /// alone may be a type (`func(int, string)`) or share the type after it
    /// (`func(a, b int)`); which, the whole list decides.
    fn parse_parameter_list(&mut self, mut name0: Option<Ident>, mut typ0: Option<Expr>, closing: Token) -> Vec<Field> {
        let tparams = closing == Token::RBrack;
        let pos = name0.as_ref().map(lo).unwrap_or(self.pos);
        let mut list: Vec<(Option<Ident>, Option<Expr>)> = vec![];
        let mut named = 0;
        while name0.is_some() || (self.tok != closing && self.tok != Token::Eof && !self.at_decl_start()) {
            let par = match typ0.take() {
                Some(t) => {
                    let t = if tparams { self.embedded_elem(Some(t)) } else { t };
                    (name0.take(), Some(t))
                }
                None => {
                    let n = name0.take();
                    self.parse_param_decl(n, tparams)
                }
            };
            if par.0.is_some() || par.1.is_some() {
                if par.0.is_some() && par.1.is_some() {
                    named += 1;
                }
                list.push(par);
            }
            if !self.at_comma("parameter list", closing) {
                break;
            }
            self.next();
        }
        if list.is_empty() {
            return vec![];
        }

        // Which parameter's type each one has: its own, or the next typed one.
        let mut source: Vec<usize> = (0..list.len()).collect();
        if named == 0 {
            // All names are types.
            for par in &mut list {
                if let Some(n) = par.0.take() {
                    par.1 = Some(Expr::Ident(n));
                }
            }
            if tparams {
                self.error(pos, "all type parameters must be named");
            }
        } else if named != list.len() {
            let mut ok = true;
            let mut missing = pos;
            let mut typed: Option<usize> = None;
            for i in (0..list.len()).rev() {
                if let Some(t) = &list[i].1 {
                    typed = Some(i);
                    if list[i].0.is_none() {
                        ok = false;
                        missing = lo(t);
                        list[i].0 = Some(Ident { range: rng(missing, missing), name: "_".into() });
                    }
                } else if let Some(t) = typed {
                    source[i] = t;
                } else {
                    ok = false;
                    let start = list[i].0.as_ref().map(lo).unwrap_or(self.pos);
                    missing = start;
                    list[i].1 = Some(bad(start, self.pos));
                }
            }
            if !ok {
                if tparams {
                    self.error(missing, "all type parameters must be named");
                } else {
                    self.error(pos, "mixed named and unnamed parameters");
                }
            }
        }

        if named == 0 {
            return list.into_iter().filter_map(|(_, t)| t).map(|t| field(vec![], t, None)).collect();
        }
        // Consecutive names sharing a type make one field; the type comes
        // with the last of them.
        let mut params = vec![];
        let mut names = vec![];
        let mut typ = None;
        let mut current = usize::MAX;
        for (i, (name, t)) in list.into_iter().enumerate() {
            if source[i] != current {
                if let Some(t) = typ.take() {
                    params.push(field(std::mem::take(&mut names), t, None));
                }
                current = source[i];
            }
            names.extend(name);
            if t.is_some() {
                typ = t;
            }
        }
        if let Some(t) = typ {
            params.push(field(names, t, None));
        }
        params
    }

    /// `[type params](params)`; type parameters only when `accept_tparams`.
    fn parse_parameters(&mut self, accept_tparams: bool) -> (Option<FieldList>, FieldList) {
        let mut tparams = None;
        if accept_tparams && self.tok == Token::LBrack {
            let opening = self.pos;
            self.next();
            let list = self.parse_parameter_list(None, None, Token::RBrack);
            let (rbrack, end) = self.close(Token::RBrack);
            let fl = FieldList { range: rng(opening, end), opening: Some(ts(opening)), list, closing: Some(ts(rbrack)) };
            if fl.num_fields() == 0 {
                self.error(rbrack, "empty type parameter list");
            } else {
                tparams = Some(fl);
            }
        }
        let opening = self.expect(Token::LParen);
        let list = if self.tok != Token::RParen { self.parse_parameter_list(None, None, Token::RParen) } else { vec![] };
        let (rparen, end) = self.close(Token::RParen);
        let params = FieldList { range: rng(opening, end), opening: Some(ts(opening)), list, closing: Some(ts(rparen)) };
        (tparams, params)
    }

    fn parse_result(&mut self) -> Option<FieldList> {
        if self.tok == Token::LParen {
            return Some(self.parse_parameters(false).1);
        }
        let typ = self.try_ident_or_type()?;
        let f = field(vec![], typ, None);
        Some(FieldList { range: f.range, opening: None, list: vec![f], closing: None })
    }

    #[inline(never)]
    fn parse_func_type(&mut self) -> FuncType {
        let pos = self.expect(Token::Func);
        let (tparams, params) = self.parse_parameters(true);
        if let Some(tp) = &tparams {
            self.error(lo(tp), "function type must have no type parameters");
        }
        let results = self.parse_result();
        func_type(Some(pos), None, params, results)
    }

    /// An interface element starting with a name: a method, or an embedded
    /// (possibly instantiated) type.
    fn parse_method_spec(&mut self) -> Field {
        let doc = self.group(self.lead_comment);
        let mut names = vec![];
        let typ;
        match self.parse_type_name(None) {
            Expr::Ident(ident) if self.tok == Token::LBrack => {
                // A generic method (an error) or an embedded instantiated type.
                let lbrack = self.pos;
                self.next();
                self.expr_lev += 1;
                let x = self.parse_expr();
                self.expr_lev -= 1;
                match x {
                    Expr::Ident(name0) if self.tok != Token::Comma && self.tok != Token::RBrack => {
                        let _ = self.parse_parameter_list(Some(name0), None, Token::RBrack);
                        self.expect(Token::RBrack);
                        self.error(lbrack, "interface method must have no type parameters");
                        let (_, params) = self.parse_parameters(false);
                        let results = self.parse_result();
                        names.push(ident);
                        typ = Expr::FuncType(Box::new(func_type(None, None, params, results)));
                    }
                    x => {
                        let mut list = vec![x];
                        if self.at_comma("type argument list", Token::RBrack) {
                            self.expr_lev += 1;
                            self.next();
                            while self.tok != Token::RBrack && self.tok != Token::Eof && !self.at_decl_start() {
                                list.push(self.parse_type());
                                if !self.at_comma("type argument list", Token::RBrack) {
                                    break;
                                }
                                self.next();
                            }
                            self.expr_lev -= 1;
                        }
                        let (rbrack, end) = self.close_list(Token::RBrack, "type argument list");
                        typ = pack_index_expr(Expr::Ident(ident), lbrack, list, rbrack, end);
                    }
                }
            }
            Expr::Ident(ident) if self.tok == Token::LParen => {
                let (_, params) = self.parse_parameters(false);
                let results = self.parse_result();
                names.push(ident);
                typ = Expr::FuncType(Box::new(func_type(None, None, params, results)));
            }
            x @ Expr::Ident(_) => typ = x,
            x => {
                typ = if self.tok == Token::LBrack { self.parse_type_instance(x) } else { x };
            }
        }
        let mut f = field(names, typ, None);
        f.doc = doc;
        f
    }

    /// A type-set element: terms joined by `|`.
    fn embedded_elem(&mut self, x: Option<Expr>) -> Expr {
        let mut x = match x {
            Some(x) => x,
            None => self.embedded_term(),
        };
        let mut n = 0;
        while self.tok == Token::Or {
            let op_pos = self.pos;
            self.next();
            let y = self.embedded_term();
            x = binary(x, op_pos, Token::Or, y);
            self.chain();
            n += 1;
        }
        self.depth -= n;
        x
    }

    fn embedded_term(&mut self) -> Expr {
        if self.tok == Token::Tilde {
            let pos = self.pos;
            self.next();
            let x = self.parse_type();
            return Expr::Unary(Box::new(UnaryExpr { range: rng(pos, hi(&x)), op: Token::Tilde, x }));
        }
        if let Some(t) = self.try_ident_or_type() {
            return t;
        }
        let pos = self.pos;
        self.error_expected(pos, "~ term or type");
        self.advance(is_expr_end);
        bad(pos, self.pos)
    }

    #[inline(never)]
    fn parse_interface_type(&mut self) -> Expr {
        let pos = self.expect(Token::Interface);
        let lbrace = self.expect(Token::LBrace);
        let mut list = vec![];
        loop {
            let mut f = if self.tok == Token::Ident {
                let f = self.parse_method_spec();
                if f.names.is_empty() {
                    let typ = self.embedded_elem(Some(f.typ));
                    let mut g = field(vec![], typ, None);
                    g.doc = f.doc;
                    g
                } else {
                    f
                }
            } else if self.tok == Token::Tilde {
                field(vec![], self.embedded_elem(None), None)
            } else if let Some(t) = self.try_ident_or_type() {
                field(vec![], self.embedded_elem(Some(t)), None)
            } else {
                break;
            };
            self.expect_semi();
            f.comment = self.group(self.line_comment);
            list.push(f);
        }
        let (rbrace, end) = self.close(Token::RBrace);
        let methods = FieldList { range: rng(lbrace, end), opening: Some(ts(lbrace)), list, closing: Some(ts(rbrace)) };
        Expr::InterfaceType(Box::new(InterfaceType { range: rng(pos, end), methods }))
    }

    #[inline(never)]
    fn parse_map_type(&mut self) -> Expr {
        let pos = self.expect(Token::Map);
        self.expect(Token::LBrack);
        let key = self.parse_type();
        self.expect(Token::RBrack);
        let value = self.parse_type();
        Expr::MapType(Box::new(MapType { range: rng(pos, hi(&value)), key, value }))
    }

    #[inline(never)]
    fn parse_chan_type(&mut self) -> Expr {
        let pos = self.pos;
        let (arrow, dir);
        if self.tok == Token::Chan {
            self.next();
            if self.tok == Token::Arrow {
                arrow = Some(ts(self.pos));
                self.next();
                dir = ChanDir::Send;
            } else {
                arrow = None;
                dir = ChanDir::Both;
            }
        } else {
            arrow = Some(ts(self.expect(Token::Arrow)));
            self.expect(Token::Chan);
            dir = ChanDir::Recv;
        }
        let value = self.parse_type();
        Expr::ChanType(Box::new(ChanType { range: rng(pos, hi(&value)), arrow, dir, value }))
    }

    fn parse_type_instance(&mut self, typ: Expr) -> Expr {
        let opening = self.expect(Token::LBrack);
        self.expr_lev += 1;
        let mut list = vec![];
        while self.tok != Token::RBrack && self.tok != Token::Eof && !self.at_decl_start() {
            list.push(self.parse_type());
            if !self.at_comma("type argument list", Token::RBrack) {
                break;
            }
            self.next();
        }
        self.expr_lev -= 1;
        let (closing, end) = self.close_list(Token::RBrack, "type argument list");
        if list.is_empty() {
            self.error_expected(closing, "type argument list");
            let index = bad(opening + 1, closing);
            return Expr::Index(Box::new(IndexExpr { range: rng(lo(&typ), end), x: typ, lbrack: ts(opening), index, rbrack: ts(closing) }));
        }
        pack_index_expr(typ, opening, list, closing, end)
    }

    #[inline(never)]
    fn try_ident_or_type(&mut self) -> Option<Expr> {
        use Token::*;
        if !matches!(self.tok, Ident | LBrack | Struct | Mul | Func | Interface | Map | Chan | Arrow | LParen) || (self.tok == Func && self.at_decl_start()) {
            return None;
        }
        if !self.enter() {
            return Some(bad(self.pos, self.pos));
        }
        let t = match self.tok {
            Ident => {
                let typ = self.parse_type_name(None);
                if self.tok == LBrack {
                    self.parse_type_instance(typ)
                } else {
                    typ
                }
            }
            LBrack => {
                let lbrack = self.expect(LBrack);
                self.parse_array_type(lbrack, None)
            }
            Struct => self.parse_struct_type(),
            Mul => self.parse_pointer_type(),
            Func => Expr::FuncType(Box::new(self.parse_func_type())),
            Interface => self.parse_interface_type(),
            Map => self.parse_map_type(),
            Chan | Arrow => self.parse_chan_type(),
            _ => {
                let lparen = self.pos;
                self.next();
                let x = self.parse_type();
                let (rparen, end) = self.close(RParen);
                Expr::Paren(Box::new(ParenExpr { range: rng(lparen, end), lparen: ts(lparen), x, rparen: ts(rparen) }))
            }
        };
        self.leave();
        Some(t)
    }

    // ----- blocks --------------------------------------------------------------

    fn parse_stmt_list(&mut self) -> Vec<Stmt> {
        let mut list = vec![];
        while !matches!(self.tok, Token::Case | Token::Default | Token::RBrace | Token::Eof) {
            // An import, or a function declared at the start of a line: the
            // block was left open.
            if matches!(self.tok, Token::Import | Token::Func) && self.at_decl_start() {
                break;
            }
            let before = self.pos;
            list.push(self.parse_stmt());
            if !self.progressed(before) {
                break;
            }
        }
        list
    }

    fn parse_block_stmt(&mut self) -> BlockStmt {
        let (lbrace, lbrace_end) = self.close(Token::LBrace);
        let list = self.parse_stmt_list();
        let rbrace = self.expect2(Token::RBrace);
        let end = match rbrace {
            Some(r) => r + 1,
            None => list.last().map(hi).unwrap_or(lbrace_end),
        };
        BlockStmt { range: rng(lbrace, end), list, rbrace: rbrace.map(ts) }
    }

    // ----- expressions ---------------------------------------------------------

    #[inline(never)]
    fn parse_func_type_or_lit(&mut self) -> Expr {
        let typ = self.parse_func_type();
        if self.tok != Token::LBrace {
            return Expr::FuncType(Box::new(typ));
        }
        self.expr_lev += 1;
        let body = self.parse_block_stmt();
        self.expr_lev -= 1;
        Expr::FuncLit(Box::new(FuncLit { range: rng(lo(&typ), hi(&body)), typ, body }))
    }

    fn basic_lit(&self) -> BasicLit {
        let kind = match self.tok {
            Token::Int => LitKind::Int,
            Token::Float => LitKind::Float,
            Token::Imag => LitKind::Imag,
            Token::Char => LitKind::Char,
            _ => LitKind::String,
        };
        let text = self.lit();
        let value = if text.starts_with('`') && text.contains('\r') { strip_cr(text, false) } else { text.to_string() };
        BasicLit { range: rng(self.pos, self.end), kind, value }
    }

    /// An operand, or a type (a conversion or composite literal follows,
    /// or the caller accepts types); callers check which they allow.
    fn parse_operand(&mut self) -> Expr {
        if self.tok == Token::Func && self.at_decl_start() {
            // `x = ` cut off before the next function.
            self.error_expected(self.pos, "operand");
            return bad(self.pos, self.pos);
        }
        match self.tok {
            Token::Ident => return Expr::Ident(self.parse_ident()),
            Token::Int | Token::Float | Token::Imag | Token::Char | Token::String => {
                let x = self.basic_lit();
                self.next();
                return Expr::BasicLit(Box::new(x));
            }
            Token::LParen => {
                let lparen = self.pos;
                self.next();
                self.expr_lev += 1;
                let x = self.parse_rhs_or_type();
                self.expr_lev -= 1;
                let (rparen, end) = self.close(Token::RParen);
                return Expr::Paren(Box::new(ParenExpr { range: rng(lparen, end), lparen: ts(lparen), x, rparen: ts(rparen) }));
            }
            Token::Func => return self.parse_func_type_or_lit(),
            _ => {}
        }
        if let Some(t) = self.try_ident_or_type() {
            return t;
        }
        let pos = self.pos;
        self.error_expected(pos, "operand");
        self.skip_expr();
        bad(pos, self.pos)
    }

    #[inline(never)]
    fn parse_type_assertion(&mut self, x: Expr) -> Expr {
        let lparen = self.expect(Token::LParen);
        let typ = if self.tok == Token::Type {
            self.next();
            None
        } else {
            Some(self.parse_type())
        };
        let (rparen, end) = self.close(Token::RParen);
        let range = rng(lo(&x), end);
        Expr::TypeAssert(Box::new(TypeAssertExpr { range, x, lparen: ts(lparen), typ, rparen: ts(rparen) }))
    }

    #[inline(never)]
    fn parse_index_or_slice_or_instance(&mut self, x: Expr) -> Expr {
        let lbrack = self.expect(Token::LBrack);
        if self.tok == Token::RBrack {
            // `x[]`: accepted, with a complaint.
            self.error_expected(self.pos, "operand");
            let rbrack = self.pos;
            self.next();
            let range = rng(lo(&x), rbrack + 1);
            return Expr::Index(Box::new(IndexExpr { range, x, lbrack: ts(lbrack), index: bad(rbrack, rbrack), rbrack: ts(rbrack) }));
        }
        self.expr_lev += 1;
        let mut index: [Option<Expr>; 3] = [None, None, None];
        let mut colons = [0u32; 2];
        let mut args = vec![];
        if self.tok != Token::Colon {
            // An index or a type argument: both are parsed as expressions.
            index[0] = Some(self.parse_rhs_or_type());
        }
        let mut ncolons = 0;
        match self.tok {
            Token::Colon => {
                while self.tok == Token::Colon && ncolons < 2 {
                    colons[ncolons] = self.pos;
                    ncolons += 1;
                    self.next();
                    if !matches!(self.tok, Token::Colon | Token::RBrack | Token::Eof) {
                        index[ncolons] = Some(self.parse_rhs());
                    }
                }
            }
            Token::Comma => {
                args.extend(index[0].take());
                while self.tok == Token::Comma {
                    self.next();
                    if self.tok != Token::RBrack && self.tok != Token::Eof {
                        args.push(self.parse_type());
                    }
                }
            }
            _ => {}
        }
        self.expr_lev -= 1;
        let (rbrack, end) = self.close(Token::RBrack);
        let range = rng(lo(&x), end);

        if ncolons > 0 {
            let [low, mut high, mut max] = index;
            let slice3 = ncolons == 2;
            if slice3 {
                if high.is_none() {
                    self.error(colons[0], "2nd index required in 3-index slice");
                    high = Some(bad(colons[0] + 1, colons[1]));
                }
                if max.is_none() {
                    self.error(colons[1], "3rd index required in 3-index slice");
                    max = Some(bad(colons[1] + 1, rbrack));
                }
            }
            return Expr::Slice(Box::new(SliceExpr { range, x, lbrack: ts(lbrack), low, high, max, slice3, rbrack: ts(rbrack) }));
        }
        if args.is_empty() {
            let index = index[0].take().unwrap_or_else(|| bad(rbrack, rbrack));
            return Expr::Index(Box::new(IndexExpr { range, x, lbrack: ts(lbrack), index, rbrack: ts(rbrack) }));
        }
        pack_index_expr(x, lbrack, args, rbrack, range.end().into())
    }

    #[inline(never)]
    fn parse_call_or_conversion(&mut self, fun: Expr) -> Expr {
        let lparen = self.expect(Token::LParen);
        self.expr_lev += 1;
        let mut args = vec![];
        let mut ellipsis = None;
        while self.tok != Token::RParen && self.tok != Token::Eof && ellipsis.is_none() && !self.at_decl_start() {
            // Builtins take types too: make([]int, n).
            args.push(self.parse_rhs_or_type());
            if self.tok == Token::Ellipsis {
                ellipsis = Some(ts(self.pos));
                self.next();
            }
            if !self.at_comma("argument list", Token::RParen) {
                break;
            }
            self.next();
        }
        self.expr_lev -= 1;
        let (rparen, end) = self.close_list(Token::RParen, "argument list");
        let range = rng(lo(&fun), end);
        Expr::Call(Box::new(CallExpr { range, fun, lparen: ts(lparen), args, ellipsis, rparen: ts(rparen) }))
    }

    fn parse_value(&mut self) -> Expr {
        if self.tok == Token::LBrace {
            return self.parse_literal_value(None);
        }
        let x = self.parse_expr();
        self.check_expr(x)
    }

    fn parse_element(&mut self) -> Expr {
        let x = self.parse_value();
        if self.tok == Token::Colon {
            let colon = self.pos;
            self.next();
            let value = self.parse_value();
            return Expr::KeyValue(Box::new(KeyValueExpr { range: rng(lo(&x), hi(&value)), key: x, colon: ts(colon), value }));
        }
        x
    }

    #[inline(never)]
    fn parse_literal_value(&mut self, typ: Option<Expr>) -> Expr {
        if !self.enter() {
            return bad(self.pos, self.pos);
        }
        let lbrace = self.expect(Token::LBrace);
        let mut elts = vec![];
        self.expr_lev += 1;
        while self.tok != Token::RBrace && self.tok != Token::Eof && !self.at_decl_start() {
            elts.push(self.parse_element());
            if !self.at_comma("composite literal", Token::RBrace) {
                break;
            }
            self.next();
        }
        self.expr_lev -= 1;
        let (rbrace, end) = self.close_list(Token::RBrace, "composite literal");
        self.leave();
        let start = typ.as_ref().map(lo).unwrap_or(lbrace);
        Expr::CompositeLit(Box::new(CompositeLit { range: rng(start, end), typ, lbrace: ts(lbrace), elts, rbrace: ts(rbrace) }))
    }

    /// `x` if it is an expression; a type where an expression must be is
    /// reported and replaced.
    fn check_expr(&mut self, x: Expr) -> Expr {
        match unparen(&x) {
            Expr::Bad(_)
            | Expr::Ident(_)
            | Expr::BasicLit(_)
            | Expr::FuncLit(_)
            | Expr::CompositeLit(_)
            | Expr::Selector(_)
            | Expr::Index(_)
            | Expr::IndexList(_)
            | Expr::Slice(_)
            | Expr::TypeAssert(_)
            | Expr::Call(_)
            | Expr::Star(_)
            | Expr::Unary(_)
            | Expr::Binary(_)
            | Expr::Paren(_) => x,
            _ => {
                self.error_expected(lo(&x), "expression");
                bad(lo(&x), hi(&x))
            }
        }
    }

    /// `x` unless it is `[...]T`, which only a composite literal may use.
    fn check_expr_or_type(&mut self, x: Expr) -> Expr {
        if let Expr::ArrayType(t) = unparen(&x) {
            if let Some(Expr::Ellipsis(e)) = &t.len {
                self.error(lo(e.as_ref()), "expected array length, found '...'");
                return bad(lo(&x), hi(&x));
            }
        }
        x
    }

    fn parse_primary_expr(&mut self, x: Option<Expr>) -> Expr {
        let mut x = match x {
            Some(x) => x,
            None => self.parse_operand(),
        };
        let mut n = 0;
        loop {
            match self.tok {
                Token::Period => {
                    self.next();
                    match self.tok {
                        Token::Ident => {
                            let x0 = self.check_expr_or_type(x);
                            let sel = self.parse_ident();
                            x = Expr::Selector(Box::new(SelectorExpr { range: rng(lo(&x0), hi(&sel)), x: x0, sel }));
                        }
                        Token::LParen => {
                            let x0 = self.check_expr(x);
                            x = self.parse_type_assertion(x0);
                        }
                        _ => {
                            let pos = self.pos;
                            self.error_expected(pos, "selector or type assertion");
                            if !self.keep_for_enclosing() {
                                self.next();
                            }
                            let sel = Ident { range: rng(pos, pos), name: "_".into() };
                            x = Expr::Selector(Box::new(SelectorExpr { range: rng(lo(&x), hi(&sel)), x, sel }));
                        }
                    }
                }
                Token::LBrack => {
                    let x0 = self.check_expr(x);
                    x = self.parse_index_or_slice_or_instance(x0);
                }
                Token::LParen => {
                    let x0 = self.check_expr_or_type(x);
                    x = self.parse_call_or_conversion(x0);
                }
                Token::LBrace => {
                    // A composite literal, or the block after `if x`.
                    let complit = match unparen(&x) {
                        Expr::Bad(_) | Expr::Ident(_) | Expr::Selector(_) | Expr::Index(_) | Expr::IndexList(_) => self.expr_lev >= 0,
                        Expr::ArrayType(_) | Expr::StructType(_) | Expr::MapType(_) => true,
                        _ => false,
                    };
                    if !complit {
                        break;
                    }
                    if let Expr::Paren(_) = x {
                        let start = lo(unparen(&x));
                        self.error(start, "cannot parenthesize type in composite literal");
                    }
                    x = self.parse_literal_value(Some(x));
                }
                _ => break,
            }
            self.chain();
            n += 1;
        }
        self.depth -= n;
        x
    }

    fn parse_unary_expr(&mut self) -> Expr {
        if !self.enter() {
            return bad(self.pos, self.pos);
        }
        let x = self.unary_expr();
        self.leave();
        x
    }

    fn unary_expr(&mut self) -> Expr {
        match self.tok {
            Token::Add | Token::Sub | Token::Not | Token::Xor | Token::And | Token::Tilde => {
                let (pos, op) = (self.pos, self.tok);
                self.next();
                let x = self.parse_unary_expr();
                let x = self.check_expr(x);
                Expr::Unary(Box::new(UnaryExpr { range: rng(pos, hi(&x)), op, x }))
            }
            Token::Arrow => {
                // A receive, or a receive-only channel type: `<-chan T` parses
                // as `<-` applied to `chan T`, and the arrow then moves into
                // the type, down through send-only channel types
                // (`<-chan<- chan T` is `<-chan (<-chan T)`).
                let mut arrow = self.pos;
                self.next();
                let mut x = self.parse_unary_expr();
                if let Expr::ChanType(_) = x {
                    let mut dir = ChanDir::Send;
                    let mut cur = &mut x;
                    while dir == ChanDir::Send {
                        let Expr::ChanType(t) = cur else { break };
                        if t.dir == ChanDir::Recv {
                            let at = t.arrow.map(u32::from).unwrap_or(lo(t.as_ref()));
                            self.error_expected(at, "'chan'");
                        }
                        let inner = t.arrow;
                        t.range = rng(arrow, hi(t.as_ref()));
                        t.arrow = Some(ts(arrow));
                        arrow = inner.map(u32::from).unwrap_or(arrow);
                        dir = t.dir;
                        t.dir = ChanDir::Recv;
                        cur = &mut t.value;
                    }
                    if dir == ChanDir::Send {
                        self.error_expected(arrow, "channel type");
                    }
                    return x;
                }
                let x = self.check_expr(x);
                Expr::Unary(Box::new(UnaryExpr { range: rng(arrow, hi(&x)), op: Token::Arrow, x }))
            }
            Token::Mul => {
                // A dereference or a pointer type.
                let pos = self.pos;
                self.next();
                let x = self.parse_unary_expr();
                let x = self.check_expr_or_type(x);
                Expr::Star(Box::new(StarExpr { range: rng(pos, hi(&x)), x }))
            }
            _ => self.parse_primary_expr(None),
        }
    }

    fn tok_prec(&self) -> (Token, u8) {
        // `=` in an expression is likely `==` mistyped.
        let tok = if self.in_rhs && self.tok == Token::Assign { Token::Eql } else { self.tok };
        (tok, tok.precedence())
    }

    /// Operators of precedence `prec1` and up, left-associative; `check`
    /// checks the operands are expressions.
    fn parse_binary_expr(&mut self, x: Option<Expr>, prec1: u8, check: bool) -> Expr {
        let mut x = match x {
            Some(x) => x,
            None => self.parse_unary_expr(),
        };
        let mut n = 0;
        loop {
            let (op, oprec) = self.tok_prec();
            if oprec < prec1 {
                break;
            }
            let pos = self.expect(op);
            let y = self.parse_binary_expr(None, oprec + 1, check);
            let (x0, y) = if check { (self.check_expr(x), self.check_expr(y)) } else { (x, y) };
            x = binary(x0, pos, op, y);
            self.chain();
            n += 1;
        }
        self.depth -= n;
        x
    }

    /// The checks `parse_binary_expr` skipped when called unchecked.
    fn check_binary_expr(&mut self, x: &mut Expr) {
        let mut x = x;
        while let Expr::Binary(b) = x {
            let b = b.as_mut();
            b.x = self.check_expr(take(&mut b.x));
            b.y = self.check_expr(take(&mut b.y));
            self.check_binary_expr(&mut b.y);
            x = &mut b.x;
        }
    }

    /// An expression or a type: callers check which they allow.
    fn parse_expr(&mut self) -> Expr {
        self.parse_binary_expr(None, 1, true)
    }

    fn parse_rhs(&mut self) -> Expr {
        let old = self.in_rhs;
        self.in_rhs = true;
        let x = self.parse_expr();
        let x = self.check_expr(x);
        self.in_rhs = old;
        x
    }

    fn parse_rhs_or_type(&mut self) -> Expr {
        let old = self.in_rhs;
        self.in_rhs = true;
        let x = self.parse_expr();
        let x = self.check_expr_or_type(x);
        self.in_rhs = old;
        x
    }

    // ----- statements ----------------------------------------------------------

    fn parse_simple_stmt(&mut self, mode: Mode) -> Simple {
        use Token::*;
        let x = self.parse_list(false);
        if matches!(self.tok, Define | Assign | AddAssign | SubAssign | MulAssign | QuoAssign | RemAssign | AndAssign | OrAssign | XorAssign | ShlAssign | ShrAssign | AndNotAssign) {
            let (pos, tok) = (self.pos, self.tok);
            self.next();
            if mode == Mode::RangeOk && self.tok == Range && (tok == Define || tok == Assign) {
                let range_pos = self.pos;
                self.next();
                let y = self.parse_rhs();
                return Simple::Range { lhs: x, tok_pos: pos, tok, range_pos, x: y };
            }
            let y = self.parse_list(true);
            let range = rng(lo(&x[0]), y.last().map(hi).unwrap_or(pos));
            let s = AssignStmt { range, lhs: x, tok_pos: ts(pos), tok, rhs: y };
            if tok == Define {
                self.check_assign_stmt(&s);
            }
            return Simple::Stmt(Stmt::Assign(Box::new(s)));
        }
        if x.len() > 1 {
            self.error_expected(lo(&x[0]), "1 expression");
        }
        let x = x.into_iter().next().unwrap_or_else(|| bad(self.pos, self.pos));
        match self.tok {
            Colon => {
                let colon = self.pos;
                self.next();
                match x {
                    Expr::Ident(label) if mode == Mode::LabelOk => {
                        let stmt = self.parse_stmt();
                        let range = rng(lo(&label), hi(&stmt));
                        Simple::Stmt(Stmt::Labeled(Box::new(LabeledStmt { range, label, colon: ts(colon), stmt })))
                    }
                    x => {
                        self.error(colon, "illegal label declaration");
                        Simple::Stmt(Stmt::Bad(BadStmt { range: rng(lo(&x), colon + 1) }))
                    }
                }
            }
            Arrow => {
                let arrow = self.pos;
                self.next();
                let value = self.parse_rhs();
                Simple::Stmt(Stmt::Send(Box::new(SendStmt { range: rng(lo(&x), hi(&value)), chan: x, arrow: ts(arrow), value })))
            }
            Inc | Dec => {
                let s = IncDecStmt { range: rng(lo(&x), self.pos + 2), x, tok_pos: ts(self.pos), tok: self.tok };
                self.next();
                Simple::Stmt(Stmt::IncDec(Box::new(s)))
            }
            _ => Simple::Stmt(Stmt::Expr(Box::new(ExprStmt { range: x.range(), x }))),
        }
    }

    fn check_assign_stmt(&mut self, s: &AssignStmt) {
        for x in &s.lhs {
            if !matches!(x, Expr::Ident(_)) {
                self.error_expected(lo(x), "identifier on left side of :=");
            }
        }
    }

    fn parse_call_expr(&mut self, call_type: &str) -> Option<CallExpr> {
        let x = self.parse_rhs_or_type();
        match x {
            Expr::Call(call) => Some(*call),
            Expr::Bad(_) => None,
            x => {
                self.error(hi(&x), format!("function must be invoked in {call_type} statement"));
                None
            }
        }
    }

    fn parse_go_stmt(&mut self) -> Stmt {
        let pos = self.expect(Token::Go);
        let call = self.parse_call_expr("go");
        self.expect_semi();
        match call {
            Some(call) => Stmt::Go(Box::new(GoStmt { range: rng(pos, hi(&call)), call })),
            None => Stmt::Bad(BadStmt { range: rng(pos, pos + 2) }),
        }
    }

    fn parse_defer_stmt(&mut self) -> Stmt {
        let pos = self.expect(Token::Defer);
        let call = self.parse_call_expr("defer");
        self.expect_semi();
        match call {
            Some(call) => Stmt::Defer(Box::new(DeferStmt { range: rng(pos, hi(&call)), call })),
            None => Stmt::Bad(BadStmt { range: rng(pos, pos + 5) }),
        }
    }

    fn parse_return_stmt(&mut self) -> Stmt {
        let pos = self.pos;
        self.expect(Token::Return);
        let results = if self.tok != Token::Semicolon && self.tok != Token::RBrace { self.parse_list(true) } else { vec![] };
        self.expect_semi();
        let end = results.last().map(hi).unwrap_or(pos + 6);
        Stmt::Return(Box::new(ReturnStmt { range: rng(pos, end), results }))
    }

    fn parse_branch_stmt(&mut self, tok: Token) -> Stmt {
        let pos = self.expect(tok);
        let label = (tok != Token::Fallthrough && self.tok == Token::Ident).then(|| self.parse_ident());
        self.expect_semi();
        let end = label.as_ref().map(hi).unwrap_or(pos + tok.as_str().len() as u32);
        Stmt::Branch(Box::new(BranchStmt { range: rng(pos, end), tok, label }))
    }

    /// The expression of an expression statement in a header (`if x`,
    /// `switch x`); other statements are reported.
    fn make_expr(&mut self, s: Option<Stmt>, want: &str) -> Option<Expr> {
        let s = s?;
        if let Stmt::Expr(e) = s {
            return Some(self.check_expr(e.x));
        }
        let found = if matches!(s, Stmt::Assign(_)) { "assignment" } else { "simple statement" };
        self.error(lo(&s), format!("expected {want}, found {found} (missing parentheses around composite literal?)"));
        Some(bad(lo(&s), hi(&s)))
    }

    fn parse_if_header(&mut self) -> (Option<Stmt>, Expr) {
        if self.tok == Token::LBrace {
            self.error(self.pos, "missing condition in if statement");
            return (None, bad(self.pos, self.pos));
        }
        let prev_lev = self.expr_lev;
        self.expr_lev = -1;
        let mut init = None;
        if self.tok != Token::Semicolon {
            // A `var` is accepted, with a complaint.
            if self.tok == Token::Var {
                self.next();
                self.error(self.pos, "var declaration not allowed in 'IF' initializer");
            }
            init = Some(self.parse_simple_stmt(Mode::Basic).into_stmt());
        }
        let mut cond_stmt = None;
        let mut semi = None;
        if self.tok != Token::LBrace {
            if self.tok == Token::Semicolon {
                semi = Some((self.pos, self.is_implicit_semi()));
                self.next();
            } else {
                self.expect(Token::Semicolon);
            }
            if self.tok != Token::LBrace {
                cond_stmt = Some(self.parse_simple_stmt(Mode::Basic).into_stmt());
            }
        } else {
            cond_stmt = init.take();
        }
        let mut cond = None;
        if cond_stmt.is_some() {
            cond = self.make_expr(cond_stmt, "boolean expression");
        } else if let Some((pos, implicit)) = semi {
            if implicit {
                self.error(pos, "unexpected newline, expecting { after if clause");
            } else {
                self.error(pos, "missing condition in if statement");
            }
        }
        let cond = cond.unwrap_or_else(|| bad(self.pos, self.pos));
        self.expr_lev = prev_lev;
        (init, cond)
    }

    /// An `if` and its `else if`s, parsed in a loop and nested afterwards.
    fn parse_if_stmt(&mut self) -> Stmt {
        let mut heads = vec![];
        let mut else_ = None;
        loop {
            let pos = self.expect(Token::If);
            let (init, cond) = self.parse_if_header();
            let body = self.parse_block_stmt();
            heads.push((pos, init, cond, body));
            if self.tok != Token::Else {
                self.expect_semi();
                break;
            }
            self.next();
            match self.tok {
                Token::If => {
                    self.chain();
                    continue;
                }
                Token::LBrace => {
                    let block = self.parse_block_stmt();
                    self.expect_semi();
                    else_ = Some(Stmt::Block(Box::new(block)));
                }
                _ => {
                    self.error_expected(self.pos, "if statement or block");
                    else_ = Some(Stmt::Bad(BadStmt { range: rng(self.pos, self.pos) }));
                }
            }
            break;
        }
        self.depth -= heads.len() as u32 - 1;
        for (pos, init, cond, body) in heads.into_iter().rev() {
            let end = else_.as_ref().map(hi).unwrap_or(hi(&body));
            else_ = Some(Stmt::If(Box::new(IfStmt { range: rng(pos, end), init, cond, body, else_ })));
        }
        else_.unwrap_or_else(|| Stmt::Bad(BadStmt { range: rng(self.pos, self.pos) }))
    }

    fn parse_type_list(&mut self) -> Vec<Expr> {
        let mut list = vec![self.parse_type()];
        while self.tok == Token::Comma {
            self.next();
            list.push(self.parse_type());
        }
        list
    }

    fn parse_case_clause(&mut self, type_switch: bool) -> Stmt {
        let pos = self.pos;
        let list = if self.tok == Token::Case {
            self.next();
            if type_switch {
                self.parse_type_list()
            } else {
                self.parse_list(true)
            }
        } else {
            self.expect(Token::Default);
            vec![]
        };
        let (colon, colon_end) = self.close(Token::Colon);
        let body = self.parse_stmt_list();
        let end = body.last().map(hi).unwrap_or(colon_end);
        Stmt::CaseClause(Box::new(CaseClause { range: rng(pos, end), list, colon: ts(colon), body }))
    }

    fn is_type_switch_guard(&mut self, s: &Option<Stmt>) -> bool {
        match s {
            Some(Stmt::Expr(e)) => is_type_switch_assert(&e.x),
            Some(Stmt::Assign(a)) if a.lhs.len() == 1 && a.rhs.len() == 1 && is_type_switch_assert(&a.rhs[0]) => match a.tok {
                Token::Assign => {
                    // `v = x.(type)` is accepted, with a complaint.
                    self.error(a.tok_pos.into(), "expected ':=', found '='");
                    true
                }
                Token::Define => true,
                _ => false,
            },
            _ => false,
        }
    }

    fn parse_switch_stmt(&mut self) -> Stmt {
        let pos = self.expect(Token::Switch);
        let (mut s1, mut s2) = (None, None);
        if self.tok != Token::LBrace {
            let prev_lev = self.expr_lev;
            self.expr_lev = -1;
            if self.tok != Token::Semicolon {
                s2 = Some(self.parse_simple_stmt(Mode::Basic).into_stmt());
            }
            if self.tok == Token::Semicolon {
                self.next();
                s1 = s2.take();
                if self.tok != Token::LBrace {
                    s2 = Some(self.parse_simple_stmt(Mode::Basic).into_stmt());
                }
            }
            self.expr_lev = prev_lev;
        }
        let type_switch = self.is_type_switch_guard(&s2);
        let lbrace = self.expect(Token::LBrace);
        let mut list = vec![];
        while self.tok == Token::Case || self.tok == Token::Default {
            list.push(self.parse_case_clause(type_switch));
        }
        let (rbrace, end) = self.close(Token::RBrace);
        self.expect_semi();
        let body = BlockStmt { range: rng(lbrace, end), list, rbrace: Some(ts(rbrace)) };
        let range = rng(pos, hi(&body));
        match s2 {
            Some(assign) if type_switch => Stmt::TypeSwitch(Box::new(TypeSwitchStmt { range, init: s1, assign, body })),
            s2 => {
                let tag = self.make_expr(s2, "switch expression");
                Stmt::Switch(Box::new(SwitchStmt { range, init: s1, tag, body }))
            }
        }
    }

    fn parse_comm_clause(&mut self) -> Stmt {
        let pos = self.pos;
        let comm = if self.tok == Token::Case {
            self.next();
            let mut lhs = self.parse_list(false);
            if self.tok == Token::Arrow {
                if lhs.len() > 1 {
                    self.error_expected(lo(&lhs[0]), "1 expression");
                }
                let arrow = self.pos;
                self.next();
                let value = self.parse_rhs();
                let chan = lhs.swap_remove(0);
                Some(Stmt::Send(Box::new(SendStmt { range: rng(lo(&chan), hi(&value)), chan, arrow: ts(arrow), value })))
            } else if matches!(self.tok, Token::Assign | Token::Define) {
                let tok = self.tok;
                if lhs.len() > 2 {
                    self.error_expected(lo(&lhs[0]), "1 or 2 expressions");
                    lhs.truncate(2);
                }
                let tok_pos = self.pos;
                self.next();
                let rhs = self.parse_rhs();
                let s = AssignStmt { range: rng(lo(&lhs[0]), hi(&rhs)), lhs, tok_pos: ts(tok_pos), tok, rhs: vec![rhs] };
                if tok == Token::Define {
                    self.check_assign_stmt(&s);
                }
                Some(Stmt::Assign(Box::new(s)))
            } else {
                if lhs.len() > 1 {
                    self.error_expected(lo(&lhs[0]), "1 expression");
                }
                let x = lhs.swap_remove(0);
                Some(Stmt::Expr(Box::new(ExprStmt { range: x.range(), x })))
            }
        } else {
            self.expect(Token::Default);
            None
        };
        let (colon, colon_end) = self.close(Token::Colon);
        let body = self.parse_stmt_list();
        let end = body.last().map(hi).unwrap_or(colon_end);
        Stmt::CommClause(Box::new(CommClause { range: rng(pos, end), comm, colon: ts(colon), body }))
    }

    fn parse_select_stmt(&mut self) -> Stmt {
        let pos = self.expect(Token::Select);
        let lbrace = self.expect(Token::LBrace);
        let mut list = vec![];
        while self.tok == Token::Case || self.tok == Token::Default {
            list.push(self.parse_comm_clause());
        }
        let (rbrace, end) = self.close(Token::RBrace);
        self.expect_semi();
        let body = BlockStmt { range: rng(lbrace, end), list, rbrace: Some(ts(rbrace)) };
        Stmt::Select(Box::new(SelectStmt { range: rng(pos, hi(&body)), body }))
    }

    fn parse_for_stmt(&mut self) -> Stmt {
        let pos = self.expect(Token::For);
        let (mut s1, mut s2, mut s3) = (None, None, None);
        let mut range = None;
        if self.tok != Token::LBrace {
            let prev_lev = self.expr_lev;
            self.expr_lev = -1;
            if self.tok != Token::Semicolon {
                if self.tok == Token::Range {
                    // `for range x`
                    let range_pos = self.pos;
                    self.next();
                    let x = self.parse_rhs();
                    range = Some((vec![], None, None, range_pos, x));
                } else {
                    match self.parse_simple_stmt(Mode::RangeOk) {
                        Simple::Stmt(s) => s2 = Some(s),
                        Simple::Range { lhs, tok_pos, tok, range_pos, x } => range = Some((lhs, Some(tok_pos), Some(tok), range_pos, x)),
                    }
                }
            }
            if range.is_none() && self.tok == Token::Semicolon {
                self.next();
                s1 = s2.take();
                if self.tok != Token::Semicolon {
                    s2 = Some(self.parse_simple_stmt(Mode::Basic).into_stmt());
                }
                self.expect_semi();
                if self.tok != Token::LBrace {
                    s3 = Some(self.parse_simple_stmt(Mode::Basic).into_stmt());
                }
            }
            self.expr_lev = prev_lev;
        }
        let body = self.parse_block_stmt();
        self.expect_semi();
        let whole = rng(pos, hi(&body));

        if let Some((mut lhs, tok_pos, tok, range_pos, x)) = range {
            if lhs.len() > 2 {
                self.error_expected(lo(&lhs[lhs.len() - 1]), "at most 2 expressions");
                return Stmt::Bad(BadStmt { range: whole });
            }
            let value = (lhs.len() == 2).then(|| lhs.pop()).flatten();
            let key = lhs.pop();
            return Stmt::Range(Box::new(RangeStmt { range: whole, key, value, tok_pos: tok_pos.map(ts), tok, range_pos: ts(range_pos), x, body }));
        }
        let cond = self.make_expr(s2, "boolean or range expression");
        Stmt::For(Box::new(ForStmt { range: whole, init: s1, cond, post: s3, body }))
    }

    fn parse_stmt(&mut self) -> Stmt {
        if !self.enter() {
            return Stmt::Bad(BadStmt { range: rng(self.pos, self.pos) });
        }
        let s = self.stmt();
        self.leave();
        s
    }

    fn stmt(&mut self) -> Stmt {
        use Token::*;
        match self.tok {
            Const | Type | Var => {
                let decl = self.parse_gen_decl(self.tok);
                Stmt::Decl(Box::new(DeclStmt { range: decl.range, decl }))
            }
            // Tokens that can start an expression.
            Ident | Int | Float | Imag | Char | String | Func | LParen | LBrack | Struct | Map | Chan | Interface | Add | Sub | Mul | And | Xor | Arrow | Not => {
                let s = self.parse_simple_stmt(Mode::LabelOk).into_stmt();
                // A labeled statement ends with the statement it labels.
                if !matches!(s, Stmt::Labeled(_)) {
                    self.expect_semi();
                }
                s
            }
            Go => self.parse_go_stmt(),
            Defer => self.parse_defer_stmt(),
            Return => self.parse_return_stmt(),
            Break | Continue | Goto | Fallthrough => self.parse_branch_stmt(self.tok),
            LBrace => {
                let block = self.parse_block_stmt();
                self.expect_semi();
                Stmt::Block(Box::new(block))
            }
            If => self.parse_if_stmt(),
            Switch => self.parse_switch_stmt(),
            Select => self.parse_select_stmt(),
            For => self.parse_for_stmt(),
            Semicolon => {
                let implicit = self.is_implicit_semi();
                let s = EmptyStmt { range: rng(self.pos, if implicit { self.pos } else { self.pos + 1 }), implicit };
                self.next();
                Stmt::Empty(s)
            }
            // A semicolon may be left out before `}`.
            RBrace => Stmt::Empty(EmptyStmt { range: rng(self.pos, self.pos), implicit: true }),
            _ => {
                let pos = self.pos;
                self.error_expected(pos, "statement");
                if !self.at_decl_start() {
                    self.next();
                }
                self.skip_stmt();
                Stmt::Bad(BadStmt { range: rng(pos, self.pos) })
            }
        }
    }

    // ----- declarations --------------------------------------------------------

    fn parse_import_spec(&mut self, doc: Option<CommentGroup>) -> Spec {
        let name = match self.tok {
            Token::Period => {
                let ident = Ident { range: rng(self.pos, self.pos + 1), name: ".".into() };
                self.next();
                Some(ident)
            }
            Token::Ident => Some(self.parse_ident()),
            _ => None,
        };
        let pos = self.pos;
        let path = if self.tok == Token::String {
            let path = self.basic_lit();
            if !is_valid_import(&path.value) {
                self.error(pos, format!("invalid import path: {}", path.value));
            }
            self.next();
            path
        } else {
            self.expect(Token::String);
            BasicLit { range: rng(pos, pos), kind: LitKind::String, value: String::new() }
        };
        self.expect_semi();
        let comment = self.group(self.line_comment);
        let start = name.as_ref().map(lo).unwrap_or(pos);
        Spec::Import(ImportSpec { range: rng(start, hi(&path)), doc, name, path, comment })
    }

    fn parse_value_spec(&mut self, doc: Option<CommentGroup>, keyword: Token, iota: usize) -> Spec {
        let pos = self.pos;
        let names = self.parse_ident_list();
        let typ = self.try_ident_or_type();
        // An initializer is always accepted here, and checked below.
        let values = if self.tok == Token::Assign {
            self.next();
            self.parse_list(true)
        } else {
            vec![]
        };
        self.expect_semi();
        match keyword {
            Token::Var if typ.is_none() && values.is_empty() => self.error(pos, "missing variable type or initialization"),
            Token::Const if values.is_empty() && (iota == 0 || typ.is_some()) => self.error(pos, "missing constant value"),
            _ => {}
        }
        let end = values.last().map(hi).or(typ.as_ref().map(hi)).unwrap_or(hi(&names[names.len() - 1]));
        let comment = self.group(self.line_comment);
        Spec::Value(ValueSpec { range: rng(lo(&names[0]), end), doc, names, typ, values, comment })
    }

    /// `type T[P C, …] …` from the first parameter on, already parsed:
    /// its name, and possibly its constraint.
    fn parse_generic_type(&mut self, open: u32, name0: Ident, typ0: Option<Expr>) -> (FieldList, Option<u32>, Expr) {
        let list = self.parse_parameter_list(Some(name0), typ0, Token::RBrack);
        let (close, end) = self.close(Token::RBrack);
        let tparams = FieldList { range: rng(open, end), opening: Some(ts(open)), list, closing: Some(ts(close)) };
        // Generic aliases are Go 1.24; go/parser has accepted them since 1.18.
        let assign = (self.tok == Token::Assign).then(|| {
            let pos = self.pos;
            self.next();
            pos
        });
        (tparams, assign, self.parse_type())
    }

    fn parse_type_spec(&mut self, doc: Option<CommentGroup>) -> Spec {
        let name = self.parse_ident();
        let mut type_params = None;
        let mut assign = None;
        let typ;
        if self.tok == Token::LBrack {
            let lbrack = self.pos;
            self.next();
            if self.tok == Token::Ident {
                // An array length or a type parameter list. Both start with
                // an expression (`[N]`, `[P any]`, `[P *C]`): parse one and
                // look at it. A name followed by `[` can only be a type
                // parameter with a slice or array constraint.
                let mut x = Expr::Ident(self.parse_ident());
                if self.tok != Token::LBrack {
                    self.expr_lev += 1;
                    let lhs = self.parse_primary_expr(Some(x));
                    x = self.parse_binary_expr(Some(lhs), 1, false);
                    self.expr_lev -= 1;
                }
                // A name followed by a type, or by `,`, is a type parameter;
                // a lone name followed by `]` is an array length.
                match extract_name(x, self.tok == Token::Comma) {
                    Ok((pname, ptype)) if ptype.is_some() || self.tok != Token::RBrack => {
                        let (tp, a, t) = self.parse_generic_type(lbrack, pname, ptype);
                        type_params = Some(tp);
                        assign = a.map(ts);
                        typ = t;
                    }
                    found => {
                        let mut x = match found {
                            Ok((pname, _)) => Expr::Ident(pname),
                            Err(x) => x,
                        };
                        self.check_binary_expr(&mut x);
                        typ = self.parse_array_type(lbrack, Some(x));
                    }
                }
            } else {
                typ = self.parse_array_type(lbrack, None);
            }
        } else {
            if self.tok == Token::Assign {
                assign = Some(ts(self.pos));
                self.next();
            }
            typ = self.parse_type();
        }
        self.expect_semi();
        let comment = self.group(self.line_comment);
        Spec::Type(TypeSpec { range: rng(lo(&name), hi(&typ)), doc, name, type_params, assign, typ, comment })
    }

    fn parse_spec(&mut self, doc: Option<CommentGroup>, keyword: Token, iota: usize) -> Spec {
        match keyword {
            Token::Import => self.parse_import_spec(doc),
            Token::Type => self.parse_type_spec(doc),
            _ => self.parse_value_spec(doc, keyword, iota),
        }
    }

    fn parse_gen_decl(&mut self, keyword: Token) -> GenDecl {
        let doc = self.group(self.lead_comment);
        let pos = self.expect(keyword);
        let mut specs = vec![];
        let (mut lparen, mut rparen) = (None, None);
        if self.tok == Token::LParen {
            lparen = Some(self.pos);
            self.next();
            let mut iota = 0;
            while !matches!(self.tok, Token::RParen | Token::Eof | Token::RBrace | Token::RBrack) && !self.at_decl_start() {
                let before = self.pos;
                let doc = self.group(self.lead_comment);
                specs.push(self.parse_spec(doc, keyword, iota));
                iota += 1;
                if !self.progressed(before) {
                    break;
                }
            }
            let (r, end) = self.close(Token::RParen);
            rparen = Some((r, end));
            self.expect_semi();
        } else {
            specs.push(self.parse_spec(None, keyword, 0));
        }
        let end = match rparen {
            Some((_, end)) => end,
            None => hi(&specs[0]),
        };
        let rparen = rparen.map(|(r, _)| r);
        GenDecl { range: rng(pos, end), doc, tok: keyword, lparen: lparen.map(ts), specs, rparen: rparen.map(ts) }
    }

    fn parse_func_decl(&mut self) -> FuncDecl {
        let doc = self.group(self.lead_comment);
        let pos = self.expect(Token::Func);
        let recv = (self.tok == Token::LParen).then(|| self.parse_parameters(false).1);
        let name = self.parse_ident();
        let (mut tparams, params) = self.parse_parameters(true);
        if let (Some(_), Some(tp)) = (&recv, &tparams) {
            let at = tp.opening.map(u32::from).unwrap_or(lo(tp));
            self.error(at, "method must have no type parameters");
            tparams = None;
        }
        let results = self.parse_result();
        let body = match self.tok {
            Token::LBrace => {
                let body = self.parse_block_stmt();
                self.expect_semi();
                Some(body)
            }
            Token::Semicolon => {
                self.next();
                if self.tok == Token::LBrace {
                    // The brace belongs on the line with the signature.
                    self.error(self.pos, "unexpected semicolon or newline before {");
                    let body = self.parse_block_stmt();
                    self.expect_semi();
                    Some(body)
                } else {
                    None
                }
            }
            _ => {
                self.expect_semi();
                None
            }
        };
        let typ = func_type(Some(pos), tparams, params, results);
        let end = body.as_ref().map(hi).unwrap_or(hi(&typ));
        FuncDecl { range: rng(pos, end), doc, recv, name, typ, body }
    }

    fn parse_decl(&mut self) -> Decl {
        match self.tok {
            Token::Const | Token::Var | Token::Type | Token::Import => Decl::Gen(self.parse_gen_decl(self.tok)),
            Token::Func => Decl::Func(Box::new(self.parse_func_decl())),
            _ => {
                let pos = self.pos;
                self.error_expected(pos, "declaration");
                self.advance(is_decl_start);
                Decl::Bad(BadDecl { range: rng(pos, self.pos) })
            }
        }
    }

    fn parse_file(&mut self) -> File {
        // A snippet without a package clause is still parsed.
        let (doc, package, name) = if self.tok == Token::Package {
            let doc = self.group(self.lead_comment);
            let pos = self.pos;
            self.next();
            let name = self.parse_ident();
            self.expect_semi();
            (doc, Some(pos), Some(name))
        } else {
            (None, None, None)
        };
        let mut decls = vec![];
        while self.tok == Token::Import {
            decls.push(Decl::Gen(self.parse_gen_decl(Token::Import)));
        }
        while self.tok != Token::Eof {
            if self.tok == Token::Import {
                self.error(self.pos, "imports must appear before other declarations");
            }
            let before = self.pos;
            decls.push(self.parse_decl());
            if !self.progressed(before) && self.tok != Token::Eof {
                self.next();
            }
        }
        let start = package.or(decls.first().map(lo)).unwrap_or(0);
        let end = decls.last().map(hi).or(name.as_ref().map(hi)).unwrap_or(start);
        let comments = std::mem::take(&mut self.comments);
        File { range: rng(start, end), doc, package: package.map(ts), name, decls, comments }
    }
}

/// Splits `x` into a type parameter's name and constraint when it can be
/// read as one: `P`, `P *C` (as `P * C`), `P (C)` (as a call), `P *C|D`.
/// A constraint that could also be a value is taken only when `force`d (a
/// comma follows) or when another term of a union shows it is a type.
fn extract_name(x: Expr, force: bool) -> Result<(Ident, Option<Expr>), Expr> {
    // The `|` terms on the left spine, outermost first.
    let mut ors = vec![];
    let mut force = force;
    let mut x = x;
    while let Expr::Binary(b) = x {
        if b.op != Token::Or {
            x = Expr::Binary(b);
            break;
        }
        let b = *b;
        force = force || is_type_elem(&b.y);
        ors.push((b.range, b.op_pos, b.y));
        x = b.x;
    }
    let base = match x {
        Expr::Ident(name) => Ok((name, None)),
        Expr::Binary(b) if b.op == Token::Mul && matches!(b.x, Expr::Ident(_)) && (force || is_type_elem(&b.y)) => {
            let b = *b;
            let Expr::Ident(name) = b.x else { unreachable!() };
            let star = Expr::Star(Box::new(StarExpr { range: rng(b.op_pos.into(), hi(&b.y)), x: b.y }));
            Ok((name, Some(star)))
        }
        Expr::Call(c) if matches!(c.fun, Expr::Ident(_)) && c.args.len() == 1 && c.ellipsis.is_none() && (force || is_type_elem(&c.args[0])) => {
            let mut c = *c;
            let Expr::Ident(name) = c.fun else { unreachable!() };
            // Go 1.18 drops the parentheses: `P (C)` has constraint `C`.
            Ok((name, c.args.pop()))
        }
        x => Err(x),
    };
    match base {
        Ok((name, Some(mut lhs))) => {
            for (_, op_pos, y) in ors.into_iter().rev() {
                lhs = binary(lhs, op_pos.into(), Token::Or, y);
            }
            Ok((name, Some(lhs)))
        }
        Ok((name, None)) if ors.is_empty() => Ok((name, None)),
        other => {
            let mut x = match other {
                Ok((name, _)) => Expr::Ident(name),
                Err(x) => x,
            };
            for (range, op_pos, y) in ors.into_iter().rev() {
                x = Expr::Binary(Box::new(BinaryExpr { range, x, op_pos, op: Token::Or, y }));
            }
            Err(x)
        }
    }
}
