//! The Go scanner (spec, "Lexical elements"), a port of go/scanner: tokens
//! with byte ranges, the semicolons Go inserts at line ends, and the
//! scanner's own errors (malformed numbers and escapes, unterminated
//! literals, stray characters). Inserted semicolons sit where go/scanner puts
//! them, at the newline or at the comment that ends the line, because node
//! positions (an implicit `EmptyStmt`) depend on them.

use super::SyntaxError;
use crate::code::syntax::{TextRange, TextSize};

/// Go's token kinds, named after go/token's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Token {
    Illegal,
    Eof,
    Comment,

    Ident,
    Int,
    Float,
    Imag,
    Char,
    String,

    Add,
    Sub,
    Mul,
    Quo,
    Rem,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    AndNot,
    AddAssign,
    SubAssign,
    MulAssign,
    QuoAssign,
    RemAssign,
    AndAssign,
    OrAssign,
    XorAssign,
    ShlAssign,
    ShrAssign,
    AndNotAssign,
    LAnd,
    LOr,
    Arrow,
    Inc,
    Dec,
    Eql,
    Lss,
    Gtr,
    Assign,
    Not,
    Neq,
    Leq,
    Geq,
    Define,
    Ellipsis,
    LParen,
    LBrack,
    LBrace,
    Comma,
    Period,
    RParen,
    RBrack,
    RBrace,
    Semicolon,
    Colon,
    Tilde,

    Break,
    Case,
    Chan,
    Const,
    Continue,
    Default,
    Defer,
    Else,
    Fallthrough,
    For,
    Func,
    Go,
    Goto,
    If,
    Import,
    Interface,
    Map,
    Package,
    Range,
    Return,
    Select,
    Struct,
    Switch,
    Type,
    Var,
}

impl Token {
    /// The operator or keyword as written; the class name for the rest
    /// (`IDENT`, `EOF`), as go/token spells them.
    pub fn as_str(self) -> &'static str {
        use Token::*;
        match self {
            Illegal => "ILLEGAL",
            Eof => "EOF",
            Comment => "COMMENT",
            Ident => "IDENT",
            Int => "INT",
            Float => "FLOAT",
            Imag => "IMAG",
            Char => "CHAR",
            String => "STRING",
            Add => "+",
            Sub => "-",
            Mul => "*",
            Quo => "/",
            Rem => "%",
            And => "&",
            Or => "|",
            Xor => "^",
            Shl => "<<",
            Shr => ">>",
            AndNot => "&^",
            AddAssign => "+=",
            SubAssign => "-=",
            MulAssign => "*=",
            QuoAssign => "/=",
            RemAssign => "%=",
            AndAssign => "&=",
            OrAssign => "|=",
            XorAssign => "^=",
            ShlAssign => "<<=",
            ShrAssign => ">>=",
            AndNotAssign => "&^=",
            LAnd => "&&",
            LOr => "||",
            Arrow => "<-",
            Inc => "++",
            Dec => "--",
            Eql => "==",
            Lss => "<",
            Gtr => ">",
            Assign => "=",
            Not => "!",
            Neq => "!=",
            Leq => "<=",
            Geq => ">=",
            Define => ":=",
            Ellipsis => "...",
            LParen => "(",
            LBrack => "[",
            LBrace => "{",
            Comma => ",",
            Period => ".",
            RParen => ")",
            RBrack => "]",
            RBrace => "}",
            Semicolon => ";",
            Colon => ":",
            Tilde => "~",
            Break => "break",
            Case => "case",
            Chan => "chan",
            Const => "const",
            Continue => "continue",
            Default => "default",
            Defer => "defer",
            Else => "else",
            Fallthrough => "fallthrough",
            For => "for",
            Func => "func",
            Go => "go",
            Goto => "goto",
            If => "if",
            Import => "import",
            Interface => "interface",
            Map => "map",
            Package => "package",
            Range => "range",
            Return => "return",
            Select => "select",
            Struct => "struct",
            Switch => "switch",
            Type => "type",
            Var => "var",
        }
    }

    /// Binary operator precedence, 1 (`||`) to 5 (`*`); 0 for anything else.
    pub fn precedence(self) -> u8 {
        use Token::*;
        match self {
            LOr => 1,
            LAnd => 2,
            Eql | Neq | Lss | Leq | Gtr | Geq => 3,
            Add | Sub | Or | Xor => 4,
            Mul | Quo | Rem | Shl | Shr | And | AndNot => 5,
            _ => 0,
        }
    }

    /// Identifiers and basic literals: the tokens whose text varies.
    pub fn is_literal(self) -> bool {
        matches!(self, Token::Ident | Token::Int | Token::Float | Token::Imag | Token::Char | Token::String)
    }

    pub fn is_keyword(self) -> bool {
        self as u8 >= Token::Break as u8
    }

    pub fn is_operator(self) -> bool {
        (Token::Add as u8..=Token::Tilde as u8).contains(&(self as u8))
    }

    fn keyword(ident: &[u8]) -> Option<Token> {
        use Token::*;
        // By first letter: most identifiers are rejected after one compare.
        let candidates: &[(&[u8], Token)] = match ident.first()? {
            b'b' => &[(b"break", Break)],
            b'c' => &[(b"case", Case), (b"chan", Chan), (b"const", Const), (b"continue", Continue)],
            b'd' => &[(b"default", Default), (b"defer", Defer)],
            b'e' => &[(b"else", Else)],
            b'f' => &[(b"fallthrough", Fallthrough), (b"for", For), (b"func", Func)],
            b'g' => &[(b"go", Go), (b"goto", Goto)],
            b'i' => &[(b"if", If), (b"import", Import), (b"interface", Interface)],
            b'm' => &[(b"map", Map)],
            b'p' => &[(b"package", Package)],
            b'r' => &[(b"range", Range), (b"return", Return)],
            b's' => &[(b"select", Select), (b"struct", Struct), (b"switch", Switch)],
            b't' => &[(b"type", Type)],
            b'v' => &[(b"var", Var)],
            _ => return None,
        };
        candidates.iter().find(|(k, _)| *k == ident).map(|&(_, t)| t)
    }
}

/// A token and the source it covers. Semicolons the scanner inserts cover
/// nothing: their range is empty, at the newline (or comment, or end of file)
/// that ended the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lexeme {
    pub token: Token,
    pub range: TextRange,
}

/// Every token of `src` in order, comments and inserted semicolons included,
/// up to (not including) the end of file. For highlighting: it never fails,
/// and malformed input comes out as `Illegal` tokens or overlong literals.
pub fn tokens(src: &str) -> impl Iterator<Item = Lexeme> + '_ {
    let mut scanner = Scanner::new(src);
    std::iter::from_fn(move || {
        let lexeme = scanner.scan();
        (lexeme.token != Token::Eof).then_some(lexeme)
    })
}

/// Letters per the spec are Unicode letters and `_`; Rust's Alphabetic
/// property is a superset of Go's (it adds letter numbers and some marks).
fn is_letter(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

fn is_digit(c: char) -> bool {
    c.is_numeric()
}

/// The byte with ASCII letters lowered, as go/scanner's `lower`.
fn lower(c: u8) -> u8 {
    c | 0x20
}

fn utf8_len(lead: u8) -> usize {
    match lead {
        0..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

fn litname(prefix: u8) -> &'static str {
    match prefix {
        b'x' => "hexadecimal literal",
        b'o' | b'0' => "octal literal",
        b'b' => "binary literal",
        _ => "decimal literal",
    }
}

/// Where a `_` in a number literal does not separate two digits.
fn invalid_sep(x: &[u8]) -> Option<usize> {
    let mut x1 = b' ';
    let mut d = b'.';
    let mut i = 0;
    if x.len() >= 2 && x[0] == b'0' {
        x1 = lower(x[1]);
        if matches!(x1, b'x' | b'o' | b'b') {
            d = b'0';
            i = 2;
        }
    }
    while i < x.len() {
        let p = d;
        d = x[i];
        if d == b'_' {
            if p != b'0' {
                return Some(i);
            }
        } else if d.is_ascii_digit() || (x1 == b'x' && d.is_ascii_hexdigit()) {
            d = b'0';
        } else {
            if p == b'_' {
                return Some(i - 1);
            }
            d = b'.';
        }
        i += 1;
    }
    (d == b'_').then(|| x.len() - 1)
}

/// The text with carriage returns removed, as go/scanner keeps comments and
/// raw strings. In a block comment `*\r/` keeps its `\r`, which would
/// otherwise close the comment early.
pub(super) fn strip_cr(text: &str, comment: bool) -> String {
    let b = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    for (j, &c) in b.iter().enumerate() {
        if c != b'\r' || (comment && out.len() > 2 && out.last() == Some(&b'*') && b.get(j + 1) == Some(&b'/')) {
            out.push(c);
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

/// Tokens one at a time: `scan` returns `Eof` once the text is done, and
/// `errors` collects what was malformed on the way.
#[derive(Clone)]
pub struct Scanner<'s> {
    src: &'s str,
    pos: usize,
    insert_semi: bool,
    /// Newlines passed so far, plus one.
    line: i64,
    /// What `//line` directives add to `line` from here on.
    line_delta: i64,
    /// The line the last token starts on, numbered as go/token numbers it
    /// (directives included): comments group by these lines.
    pub(super) tok_line: i64,
    pub errors: Vec<SyntaxError>,
}

impl<'s> Scanner<'s> {
    pub fn new(src: &'s str) -> Self {
        // A byte-order mark is only allowed first, and then skipped.
        let pos = if src.starts_with('\u{feff}') { 3 } else { 0 };
        Scanner { src, pos, insert_semi: false, line: 1, line_delta: 0, tok_line: 1, errors: vec![] }
    }

    fn error(&mut self, offset: usize, message: impl Into<String>) {
        self.errors.push(SyntaxError { offset: offset as u32, message: message.into() });
    }

    /// Reports the characters go/scanner rejects wherever they appear: NUL
    /// and a byte-order mark past the start.
    fn check_char(&mut self, i: usize) {
        let b = self.src.as_bytes();
        match b[i] {
            0 => self.error(i, "illegal character NUL"),
            0xef if b[i + 1..].starts_with(&[0xbb, 0xbf]) => self.error(i, "illegal byte order mark"),
            _ => {}
        }
    }

    pub fn scan(&mut self) -> Lexeme {
        let b = self.src.as_bytes();
        while let Some(&c) = b.get(self.pos) {
            match c {
                b' ' | b'\t' | b'\r' => self.pos += 1,
                b'\n' if !self.insert_semi => {
                    self.pos += 1;
                    self.line += 1;
                }
                _ => break,
            }
        }
        let start = self.pos;
        // go/token puts the end of a file that ends in a newline on the
        // newline's line.
        let eof_line = start == b.len() && b.last() == Some(&b'\n');
        self.tok_line = self.line - eof_line as i64 + self.line_delta;
        let Some(&c) = b.get(start) else {
            if self.insert_semi {
                self.insert_semi = false;
                return lexeme(Token::Semicolon, start, start);
            }
            return lexeme(Token::Eof, start, start);
        };

        if c.is_ascii_alphabetic() || c == b'_' || (c >= 0x80 && is_letter(self.char_at(start))) {
            let end = self.scan_identifier(start);
            let tok = if end - start > 1 { Token::keyword(&b[start..end]).unwrap_or(Token::Ident) } else { Token::Ident };
            self.insert_semi = matches!(tok, Token::Ident | Token::Break | Token::Continue | Token::Fallthrough | Token::Return);
            return lexeme(tok, start, end);
        }
        if c.is_ascii_digit() || (c == b'.' && b.get(start + 1).is_some_and(u8::is_ascii_digit)) {
            let (tok, end) = self.scan_number(start);
            self.insert_semi = true;
            return lexeme(tok, start, end);
        }

        let mut insert_semi = false;
        let at = |i: usize| b.get(start + i).copied().unwrap_or(0);
        // The token and its length for operators; the rest return early.
        let (tok, len) = match c {
            b'\n' => {
                // Only reached when a semicolon is due.
                self.pos += 1;
                self.line += 1;
                self.insert_semi = false;
                return lexeme(Token::Semicolon, start, start);
            }
            b'"' => {
                let end = self.scan_string(start);
                self.insert_semi = true;
                return lexeme(Token::String, start, end);
            }
            b'\'' => {
                let end = self.scan_rune(start);
                self.insert_semi = true;
                return lexeme(Token::Char, start, end);
            }
            b'`' => {
                let end = self.scan_raw_string(start);
                self.insert_semi = true;
                return lexeme(Token::String, start, end);
            }
            b'/' if matches!(at(1), b'/' | b'*') => {
                if self.insert_semi && self.find_line_end(start) {
                    // The comment ends the line: the semicolon comes first,
                    // at the comment, which is scanned next.
                    self.insert_semi = false;
                    return lexeme(Token::Semicolon, start, start);
                }
                let end = self.scan_comment(start);
                self.insert_semi = false;
                return lexeme(Token::Comment, start, end);
            }
            b':' if at(1) == b'=' => (Token::Define, 2),
            b':' => (Token::Colon, 1),
            b'.' if at(1) == b'.' && at(2) == b'.' => (Token::Ellipsis, 3),
            b'.' => (Token::Period, 1),
            b',' => (Token::Comma, 1),
            b';' => (Token::Semicolon, 1),
            b'(' => (Token::LParen, 1),
            b')' => {
                insert_semi = true;
                (Token::RParen, 1)
            }
            b'[' => (Token::LBrack, 1),
            b']' => {
                insert_semi = true;
                (Token::RBrack, 1)
            }
            b'{' => (Token::LBrace, 1),
            b'}' => {
                insert_semi = true;
                (Token::RBrace, 1)
            }
            b'+' => match at(1) {
                b'=' => (Token::AddAssign, 2),
                b'+' => {
                    insert_semi = true;
                    (Token::Inc, 2)
                }
                _ => (Token::Add, 1),
            },
            b'-' => match at(1) {
                b'=' => (Token::SubAssign, 2),
                b'-' => {
                    insert_semi = true;
                    (Token::Dec, 2)
                }
                _ => (Token::Sub, 1),
            },
            b'*' => if at(1) == b'=' { (Token::MulAssign, 2) } else { (Token::Mul, 1) },
            b'/' => if at(1) == b'=' { (Token::QuoAssign, 2) } else { (Token::Quo, 1) },
            b'%' => if at(1) == b'=' { (Token::RemAssign, 2) } else { (Token::Rem, 1) },
            b'^' => if at(1) == b'=' { (Token::XorAssign, 2) } else { (Token::Xor, 1) },
            b'<' => match (at(1), at(2)) {
                (b'-', _) => (Token::Arrow, 2),
                (b'=', _) => (Token::Leq, 2),
                (b'<', b'=') => (Token::ShlAssign, 3),
                (b'<', _) => (Token::Shl, 2),
                _ => (Token::Lss, 1),
            },
            b'>' => match (at(1), at(2)) {
                (b'=', _) => (Token::Geq, 2),
                (b'>', b'=') => (Token::ShrAssign, 3),
                (b'>', _) => (Token::Shr, 2),
                _ => (Token::Gtr, 1),
            },
            b'=' => if at(1) == b'=' { (Token::Eql, 2) } else { (Token::Assign, 1) },
            b'!' => if at(1) == b'=' { (Token::Neq, 2) } else { (Token::Not, 1) },
            b'&' => match (at(1), at(2)) {
                (b'^', b'=') => (Token::AndNotAssign, 3),
                (b'^', _) => (Token::AndNot, 2),
                (b'=', _) => (Token::AndAssign, 2),
                (b'&', _) => (Token::LAnd, 2),
                _ => (Token::And, 1),
            },
            b'|' => match at(1) {
                b'=' => (Token::OrAssign, 2),
                b'|' => (Token::LOr, 2),
                _ => (Token::Or, 1),
            },
            b'~' => (Token::Tilde, 1),
            _ => {
                let ch = self.char_at(start);
                match ch {
                    '\u{feff}' => self.error(start, "illegal byte order mark"),
                    '\0' => self.error(start, "illegal character NUL"),
                    _ if ch.is_control() || ch.is_whitespace() => self.error(start, format!("illegal character U+{:04X}", ch as u32)),
                    _ => self.error(start, format!("illegal character U+{:04X} '{ch}'", ch as u32)),
                }
                // An illegal character leaves the semicolon state alone.
                self.pos = start + ch.len_utf8();
                return lexeme(Token::Illegal, start, self.pos);
            }
        };
        self.pos = start + len;
        self.insert_semi = insert_semi;
        lexeme(tok, start, start + len)
    }

    fn char_at(&self, i: usize) -> char {
        self.src[i..].chars().next().unwrap_or('\0')
    }

    fn scan_identifier(&mut self, start: usize) -> usize {
        let b = self.src.as_bytes();
        let mut i = start;
        while let Some(&c) = b.get(i) {
            if c.is_ascii_alphanumeric() || c == b'_' {
                i += 1;
            } else if c >= 0x80 {
                let ch = self.char_at(i);
                if !is_letter(ch) && !is_digit(ch) {
                    break;
                }
                i += ch.len_utf8();
            } else {
                break;
            }
        }
        self.pos = i;
        i
    }

    /// Digits of `base` and `_` from `*i`; bit 0 of the result says a digit
    /// was seen, bit 1 a separator. Below base 10 any decimal digit is taken
    /// and the first one out of range recorded in `invalid`.
    fn digits(&self, i: &mut usize, base: u8, invalid: &mut Option<usize>) -> u8 {
        let b = self.src.as_bytes();
        let mut digsep = 0;
        if base <= 10 {
            while let Some(&c) = b.get(*i) {
                if c == b'_' {
                    digsep |= 2;
                } else if c.is_ascii_digit() {
                    if c >= b'0' + base && invalid.is_none() {
                        *invalid = Some(*i);
                    }
                    digsep |= 1;
                } else {
                    break;
                }
                *i += 1;
            }
        } else {
            while let Some(&c) = b.get(*i) {
                if c == b'_' {
                    digsep |= 2;
                } else if c.is_ascii_hexdigit() {
                    digsep |= 1;
                } else {
                    break;
                }
                *i += 1;
            }
        }
        digsep
    }

    fn scan_number(&mut self, start: usize) -> (Token, usize) {
        let b = self.src.as_bytes();
        let at = |i: usize| b.get(i).copied().unwrap_or(0);
        let mut i = start;
        let mut tok = Token::Illegal;
        let mut base = 10;
        let mut prefix = 0u8;
        let mut digsep = 0;
        let mut invalid = None;

        if at(i) != b'.' {
            tok = Token::Int;
            if at(i) == b'0' {
                i += 1;
                match lower(at(i)) {
                    b'x' => (base, prefix) = (16, b'x'),
                    b'o' => (base, prefix) = (8, b'o'),
                    b'b' => (base, prefix) = (2, b'b'),
                    _ => {
                        (base, prefix) = (8, b'0');
                        digsep = 1;
                    }
                }
                if prefix != b'0' {
                    i += 1;
                }
            }
            digsep |= self.digits(&mut i, base, &mut invalid);
        }
        if at(i) == b'.' {
            tok = Token::Float;
            if prefix == b'o' || prefix == b'b' {
                self.error(i, format!("invalid radix point in {}", litname(prefix)));
            }
            i += 1;
            digsep |= self.digits(&mut i, base, &mut invalid);
        }
        if digsep & 1 == 0 {
            self.error(i, format!("{} has no digits", litname(prefix)));
        }
        let e = lower(at(i));
        if e == b'e' || e == b'p' {
            if e == b'e' && prefix != 0 && prefix != b'0' {
                self.error(i, format!("'{}' exponent requires decimal mantissa", at(i) as char));
            } else if e == b'p' && prefix != b'x' {
                self.error(i, format!("'{}' exponent requires hexadecimal mantissa", at(i) as char));
            }
            i += 1;
            tok = Token::Float;
            if matches!(at(i), b'+' | b'-') {
                i += 1;
            }
            let ds = self.digits(&mut i, 10, &mut None);
            digsep |= ds;
            if ds & 1 == 0 {
                self.error(i, "exponent has no digits");
            }
        } else if prefix == b'x' && tok == Token::Float {
            self.error(i, "hexadecimal mantissa requires a 'p' exponent");
        }
        if at(i) == b'i' {
            tok = Token::Imag;
            i += 1;
        }
        if let (Token::Int, Some(k)) = (tok, invalid) {
            self.error(k, format!("invalid digit '{}' in {}", b[k] as char, litname(prefix)));
        }
        if digsep & 2 != 0 {
            if let Some(k) = invalid_sep(&b[start..i]) {
                self.error(start + k, "'_' must separate successive digits");
            }
        }
        self.pos = i;
        (tok, i)
    }

    /// An escape sequence after its backslash, `i` pointing past the
    /// backslash. On a malformed sequence it reports it and stops at the
    /// offending character, which the literal then continues with.
    fn scan_escape(&mut self, mut i: usize, quote: u8) -> (usize, bool) {
        let b = self.src.as_bytes();
        let offs = i;
        let (mut n, base, max): (u32, u32, u32) = match b.get(i) {
            Some(&c) if matches!(c, b'a' | b'b' | b'f' | b'n' | b'r' | b't' | b'v' | b'\\') || c == quote => return (i + 1, true),
            Some(b'0'..=b'7') => (3, 8, 255),
            Some(b'x') => {
                i += 1;
                (2, 16, 255)
            }
            Some(b'u') => {
                i += 1;
                (4, 16, 0x10ffff)
            }
            Some(b'U') => {
                i += 1;
                (8, 16, 0x10ffff)
            }
            None => {
                self.error(offs, "escape sequence not terminated");
                return (i, false);
            }
            Some(_) => {
                self.error(offs, "unknown escape sequence");
                return (i, false);
            }
        };
        let mut x: u32 = 0;
        while n > 0 {
            let d = match b.get(i) {
                Some(&c) => (c as char).to_digit(16).unwrap_or(16),
                None => 16,
            };
            if d >= base {
                match b.get(i) {
                    None => self.error(i, "escape sequence not terminated"),
                    Some(_) => {
                        let ch = self.char_at(i);
                        self.error(i, format!("illegal character U+{:04X} '{ch}' in escape sequence", ch as u32));
                    }
                }
                return (i, false);
            }
            x = x.wrapping_mul(base).wrapping_add(d);
            i += 1;
            n -= 1;
        }
        if x > max || (0xd800..0xe000).contains(&x) {
            self.error(offs, "escape sequence is invalid Unicode code point");
            return (i, false);
        }
        (i, true)
    }

    fn scan_rune(&mut self, start: usize) -> usize {
        let b = self.src.as_bytes();
        let mut i = start + 1;
        let mut valid = true;
        let mut n = 0;
        loop {
            match b.get(i) {
                None | Some(b'\n') => {
                    if valid {
                        self.error(start, "rune literal not terminated");
                        valid = false;
                    }
                    break;
                }
                Some(b'\'') => {
                    i += 1;
                    break;
                }
                Some(b'\\') => {
                    n += 1;
                    let (next, ok) = self.scan_escape(i + 1, b'\'');
                    i = next;
                    valid &= ok;
                }
                Some(&c) => {
                    n += 1;
                    self.check_char(i);
                    i += utf8_len(c);
                }
            }
        }
        if valid && n != 1 {
            self.error(start, "illegal rune literal");
        }
        self.pos = i;
        i
    }

    fn scan_string(&mut self, start: usize) -> usize {
        let b = self.src.as_bytes();
        let mut i = start + 1;
        loop {
            match b.get(i) {
                None | Some(b'\n') => {
                    self.error(start, "string literal not terminated");
                    break;
                }
                Some(b'"') => {
                    i += 1;
                    break;
                }
                Some(b'\\') => i = self.scan_escape(i + 1, b'"').0,
                Some(&c) => {
                    if c == 0 || c == 0xef {
                        self.check_char(i);
                    }
                    i += 1;
                }
            }
        }
        self.pos = i;
        i
    }

    fn scan_raw_string(&mut self, start: usize) -> usize {
        let b = self.src.as_bytes();
        let mut i = start + 1;
        loop {
            match b.get(i) {
                None => {
                    self.error(start, "raw string literal not terminated");
                    break;
                }
                Some(b'`') => {
                    i += 1;
                    break;
                }
                Some(&c) => {
                    if c == b'\n' {
                        self.line += 1;
                    } else if c == 0 || c == 0xef {
                        self.check_char(i);
                    }
                    i += 1;
                }
            }
        }
        self.pos = i;
        i
    }

    /// Whether the comments at `i` reach the end of the line (or file)
    /// before any other token, in which case a pending semicolon goes first.
    fn find_line_end(&self, mut i: usize) -> bool {
        let b = self.src.as_bytes();
        loop {
            match b.get(i + 1) {
                Some(b'/') => return true,
                Some(b'*') => {}
                _ => return false,
            }
            let mut j = i + 2;
            while let Some(&c) = b.get(j) {
                if c == b'\n' {
                    return true;
                }
                j += 1;
                if c == b'*' && b.get(j) == Some(&b'/') {
                    j += 1;
                    break;
                }
            }
            while matches!(b.get(j), Some(b' ' | b'\t' | b'\r')) {
                j += 1;
            }
            match b.get(j) {
                None | Some(b'\n') => return true,
                Some(b'/') => i = j,
                Some(_) => return false,
            }
        }
    }

    /// A comment at `start`, returning its end. A line comment ends before
    /// its newline (and before the `\r` of a `\r\n`).
    fn scan_comment(&mut self, start: usize) -> usize {
        let b = self.src.as_bytes();
        let mut i = start + 2;
        let (end, next) = if b[start + 1] == b'/' {
            while let Some(&c) = b.get(i) {
                if c == b'\n' {
                    break;
                }
                if c == 0 || c == 0xef {
                    self.check_char(i);
                }
                i += 1;
            }
            let end = if i > start + 2 && b[i - 1] == b'\r' { i - 1 } else { i };
            (end, Some(if i < b.len() { i + 1 } else { i }))
        } else {
            let mut closed = false;
            while let Some(&c) = b.get(i) {
                i += 1;
                if c == b'\n' {
                    self.line += 1;
                } else if c == b'*' && b.get(i) == Some(&b'/') {
                    i += 1;
                    closed = true;
                    break;
                } else if c == 0 || c == 0xef {
                    self.check_char(i - 1);
                }
            }
            if !closed {
                self.error(start, "comment not terminated");
            }
            (i, closed.then_some(i))
        };
        self.pos = i;
        let block = b[start + 1] == b'*';
        if let Some(next) = next {
            if (block || start == 0 || b[start - 1] == b'\n') && b[start + 2..end].starts_with(b"line ") {
                self.line_directive(next, start, end, block);
            }
        }
        end
    }

    /// Checks a `//line file:line[:col]` directive, and renumbers the lines
    /// from `next` (just past the comment) the way go/token does, which
    /// decides how comments group into doc comments.
    fn line_directive(&mut self, next: usize, start: usize, end: usize, block: bool) {
        let text = &self.src[start..if block { end - 2 } else { end }];
        let text = &text[7..];
        let offs = start + 7;
        let trailing = |t: &str| -> Option<(usize, Option<u64>)> {
            let i = t.rfind(':')?;
            let digits = &t[i + 1..];
            let n = (!digits.is_empty() && digits.bytes().all(|c| c.is_ascii_digit())).then(|| digits.parse::<u64>().ok()).flatten();
            Some((i + 1, n))
        };
        let Some((mut i, n)) = trailing(text) else { return };
        let Some(n) = n else {
            self.error(offs + i, format!("invalid line number: {}", &text[i..]));
            return;
        };
        // `file:line:col` or `file:line`.
        let (line, digits) = match trailing(&text[..i - 1]) {
            Some((i2, Some(n2))) => {
                if n == 0 {
                    self.error(offs + i, format!("invalid column number: {}", &text[i..]));
                    return;
                }
                let digits = &text[i2..i - 1];
                i = i2;
                (n2, digits)
            }
            _ => (n, &text[i..]),
        };
        if line == 0 {
            self.error(offs + i, format!("invalid line number: {digits}"));
            return;
        }
        if next < self.src.len() {
            // The line `next` is on, before renumbering.
            let next_line = self.line + (!block) as i64;
            self.line_delta = line as i64 - next_line;
        }
    }
}

fn lexeme(token: Token, start: usize, end: usize) -> Lexeme {
    Lexeme { token, range: TextRange::new(TextSize::from(start as u32), TextSize::from(end as u32)) }
}
