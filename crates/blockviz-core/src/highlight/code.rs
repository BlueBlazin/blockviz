//! A table-driven lexer for brace and script languages: comments, strings,
//! numbers, keywords, and names classified by what surrounds them (`fn name`,
//! `name(`, `Name`). Per-language quirks are flags rather than separate lexers.

use super::{Hl, Runs};

pub(super) struct Spec {
    line: &'static [&'static str],
    block: &'static [(&'static str, &'static str)],
    quotes: &'static str,
    keywords: &'static str,
    types: &'static str,
    constants: &'static str,
    builtins: &'static str,
    /// Keywords after which a name is a function being defined.
    defs: &'static str,
    /// Keywords after which a name is a type being defined.
    type_defs: &'static str,
    flags: u32,
}

/// `#include` and other preprocessor lines.
const PREPROC: u32 = 1;
/// Backtick strings with `${expr}` interpolation.
const TEMPLATE: u32 = 1 << 1;
/// `/.../flags` literals.
const REGEX: u32 = 1 << 2;
/// Raw strings, lifetimes, `#[attributes]`, `macro!`.
const RUST: u32 = 1 << 3;
/// `$var`, `${var}`, `$(VAR)`.
const DOLLAR: u32 = 1 << 4;
/// `@Name` decorators and annotations.
const AT: u32 = 1 << 5;
/// Case-insensitive keywords.
const NOCASE: u32 = 1 << 6;
/// Shell rules: `#` comments only at a word boundary, literal single quotes,
/// strings spanning lines.
const SHELL: u32 = 1 << 7;
/// Ordinary strings may span lines.
const MULTILINE: u32 = 1 << 8;
/// `"""text blocks"""`.
const TRIPLE: u32 = 1 << 9;
/// Capitalized names are not types (Go exports, shell variables).
const PLAIN_CAPS: u32 = 1 << 10;
/// `$` is an identifier character.
const IDENT_DOLLAR: u32 = 1 << 11;

const C_KEYWORDS: &str = "alignas alignof asm break case catch class const consteval constexpr constinit const_cast continue co_await co_return co_yield decltype default delete do dynamic_cast else enum explicit export extern for friend goto if inline mutable namespace new noexcept operator private protected public register reinterpret_cast requires return sizeof static static_assert static_cast struct switch template thread_local throw try typedef typeid typename union using virtual volatile while __global__ __device__ __host__ __shared__ __constant__ __restrict__ restrict kernel device threadgroup";
const C_TYPES: &str = "void bool char short int long float double signed unsigned auto size_t ssize_t ptrdiff_t int8_t int16_t int32_t int64_t uint8_t uint16_t uint32_t uint64_t wchar_t char16_t char32_t half float2 float3 float4 half2 dim3 FILE";

static C: Spec = Spec {
    line: &["//"],
    block: &[("/*", "*/")],
    quotes: "\"'",
    keywords: C_KEYWORDS,
    types: C_TYPES,
    constants: "true false NULL nullptr",
    builtins: "printf fprintf sprintf snprintf puts malloc calloc realloc free memcpy memmove memset strlen strcmp strcpy assert exit",
    defs: "",
    type_defs: "class struct union enum namespace",
    flags: PREPROC,
};

static OBJC: Spec = Spec { flags: PREPROC | AT, ..C };

static RUST_SPEC: Spec = Spec {
    line: &["//"],
    block: &[("/*", "*/")],
    quotes: "\"",
    keywords: "as async await break const continue crate dyn else enum extern fn for if impl in let loop match mod move mut pub ref return static struct super trait type union unsafe use where while yield macro_rules",
    types: "bool char str u8 u16 u32 u64 u128 usize i8 i16 i32 i64 i128 isize f32 f64",
    constants: "true false",
    builtins: "",
    defs: "fn",
    type_defs: "struct enum trait type union",
    flags: RUST | MULTILINE,
};

static GO: Spec = Spec {
    line: &["//"],
    block: &[("/*", "*/")],
    quotes: "\"'`",
    keywords: "break case chan const continue default defer else fallthrough for func go goto if import interface map package range return select struct switch type var",
    types: "bool byte complex64 complex128 error float32 float64 int int8 int16 int32 int64 rune string uint uint8 uint16 uint32 uint64 uintptr any",
    constants: "true false nil iota",
    builtins: "append cap clear close complex copy delete imag len make max min new panic print println real recover",
    defs: "func",
    type_defs: "type",
    flags: PLAIN_CAPS,
};

static TS: Spec = Spec {
    line: &["//"],
    block: &[("/*", "*/")],
    quotes: "\"'`",
    keywords: "abstract as async await break case catch class const continue debugger declare default delete do else enum export extends finally for from function get if implements import in infer instanceof interface is keyof let namespace new of override private protected public readonly return satisfies set static super switch throw try type typeof var void while with yield",
    types: "any boolean bigint never number object string symbol unknown",
    constants: "true false null undefined NaN Infinity",
    builtins: "require",
    defs: "function",
    type_defs: "class interface type enum namespace",
    flags: TEMPLATE | REGEX | AT | IDENT_DOLLAR,
};

static JAVA: Spec = Spec {
    line: &["//"],
    block: &[("/*", "*/")],
    quotes: "\"'",
    keywords: "abstract assert break case catch class continue default do else enum extends final finally for goto if implements import instanceof interface native new package permits private protected public record return sealed static strictfp super switch synchronized throw throws transient try var volatile while yield",
    types: "boolean byte char double float int long short void",
    constants: "true false null",
    builtins: "",
    defs: "",
    type_defs: "class interface enum record",
    flags: AT | TRIPLE,
};

static KOTLIN: Spec = Spec {
    line: &["//"],
    block: &[("/*", "*/")],
    quotes: "\"'",
    keywords: "abstract actual annotation as break by catch class companion const constructor continue crossinline data do else enum expect external final finally for fun if import in infix init inline inner interface internal is lateinit noinline object open operator out override package private protected public reified return sealed super suspend tailrec throw try typealias val value var vararg when where while",
    types: "",
    constants: "true false null",
    builtins: "println print listOf mutableListOf mapOf mutableMapOf setOf require check error",
    defs: "fun",
    type_defs: "class interface object typealias",
    flags: AT | TRIPLE,
};

static SWIFT: Spec = Spec {
    line: &["//"],
    block: &[("/*", "*/")],
    quotes: "\"",
    keywords: "actor any associatedtype async await break case catch class continue convenience default defer deinit do dynamic else enum extension fallthrough fileprivate final for func guard if import in indirect init inout internal is lazy let mutating nonmutating open operator override private protocol public repeat required rethrows return some static struct subscript super switch throw throws try typealias unowned var weak where while",
    types: "",
    constants: "true false nil",
    builtins: "print",
    defs: "func",
    type_defs: "class struct enum protocol extension actor typealias",
    flags: AT | TRIPLE,
};

static SCALA: Spec = Spec {
    line: &["//"],
    block: &[("/*", "*/")],
    quotes: "\"'",
    keywords: "abstract case catch class def do else enum export extends final finally for given if implicit import lazy match new object override package private protected return sealed super then throw trait try type using val var while with yield",
    types: "",
    constants: "true false null",
    builtins: "println",
    defs: "def",
    type_defs: "class object trait type enum",
    flags: AT | TRIPLE,
};

static CSHARP: Spec = Spec {
    line: &["//"],
    block: &[("/*", "*/")],
    quotes: "\"'",
    keywords: "abstract as async await base break case catch checked class const continue default delegate do else enum event explicit extern finally fixed for foreach get goto if implicit in init interface internal is lock namespace new operator out override params private protected public readonly record ref return sealed set sizeof stackalloc static struct switch throw try typeof unchecked unsafe using var virtual volatile when where while yield",
    types: "bool byte char decimal double dynamic float int long object sbyte short string uint ulong ushort void",
    constants: "true false null",
    builtins: "",
    defs: "",
    type_defs: "class struct interface enum record namespace",
    flags: TRIPLE,
};

static DART: Spec = Spec {
    line: &["//"],
    block: &[("/*", "*/")],
    quotes: "\"'",
    keywords: "abstract as assert async await break case catch class const continue covariant default deferred do else enum export extends extension external factory final finally for get if implements import in interface is late library mixin new on operator part required rethrow return sealed set show static super switch sync throw try typedef var while with yield",
    types: "int double num bool dynamic void",
    constants: "true false null",
    builtins: "print",
    defs: "",
    type_defs: "class mixin enum extension typedef",
    flags: AT | TRIPLE,
};

static RUBY: Spec = Spec {
    line: &["#"],
    block: &[("=begin", "=end")],
    quotes: "\"'`",
    keywords: "BEGIN END alias and begin break case class def defined? do else elsif end ensure for if in module next not or redo rescue retry return super then undef unless until when while yield require require_relative attr_accessor attr_reader attr_writer private protected public",
    types: "",
    constants: "true false nil",
    builtins: "puts print p raise",
    defs: "def",
    type_defs: "class module",
    flags: MULTILINE,
};

static PHP: Spec = Spec {
    line: &["//", "#"],
    block: &[("/*", "*/")],
    quotes: "\"'`",
    keywords: "abstract and array as break callable case catch class clone const continue declare default do echo else elseif empty enddeclare endfor endforeach endif endswitch endwhile enum extends final finally fn for foreach function global goto if implements include include_once instanceof insteadof interface isset list match namespace new or print private protected public readonly require require_once return static switch throw trait try unset use var while xor yield",
    types: "int float bool string void mixed",
    constants: "true false null TRUE FALSE NULL",
    builtins: "",
    defs: "function",
    type_defs: "class interface trait enum",
    flags: DOLLAR | MULTILINE,
};

static LUA: Spec = Spec {
    line: &["--"],
    block: &[("--[[", "]]")],
    quotes: "\"'",
    keywords: "and break do else elseif end for function goto if in local not or repeat return then until while",
    types: "",
    constants: "true false nil",
    builtins: "print require pairs ipairs tostring tonumber type error assert pcall setmetatable",
    defs: "function",
    type_defs: "",
    flags: 0,
};

static SHELL_SPEC: Spec = Spec {
    line: &["#"],
    block: &[],
    quotes: "\"'`",
    keywords: "if then else elif fi for while until do done case esac in function select time return exit break continue local export readonly declare typeset unset shift source alias set trap eval exec",
    types: "",
    constants: "true false",
    builtins: "",
    defs: "function",
    type_defs: "",
    flags: SHELL | DOLLAR | PLAIN_CAPS,
};

static POWERSHELL: Spec = Spec {
    line: &["#"],
    block: &[("<#", "#>")],
    quotes: "\"'",
    keywords: "begin break catch class continue data define do dynamicparam else elseif end enum exit filter finally for foreach from function hidden if in param process return static switch throw trap try until using var while",
    types: "",
    constants: "$true $false $null",
    builtins: "",
    defs: "function",
    type_defs: "class enum",
    flags: NOCASE | DOLLAR | MULTILINE,
};

static R: Spec = Spec {
    line: &["#"],
    block: &[],
    quotes: "\"'`",
    keywords: "if else repeat while function for in next break return library require",
    types: "",
    constants: "TRUE FALSE NULL NA NaN Inf",
    builtins: "c list print cat paste",
    defs: "",
    type_defs: "",
    flags: MULTILINE | PLAIN_CAPS,
};

static JULIA: Spec = Spec {
    line: &["#"],
    block: &[("#=", "=#")],
    quotes: "\"",
    keywords: "abstract baremodule begin break catch const continue do else elseif end export finally for function global if import let local macro module mutable primitive quote return struct try type using where while",
    types: "",
    constants: "true false nothing missing",
    builtins: "println print",
    defs: "function macro",
    type_defs: "struct type module",
    flags: TRIPLE | AT,
};

static SQL: Spec = Spec {
    line: &["--"],
    block: &[("/*", "*/")],
    quotes: "'\"`",
    keywords: "add all alter and as asc begin between by cascade case check column commit constraint create database declare default delete desc distinct drop else end exists foreign from full function grant group having if in index inner insert into is join key left like limit not null offset on or order outer primary procedure references returning returns revoke right rollback schema select set table then transaction trigger union unique update using values view when where with",
    types: "int integer bigint smallint serial text varchar char boolean bool date timestamp timestamptz time float real double numeric decimal json jsonb uuid blob",
    constants: "true false",
    builtins: "count sum avg min max coalesce now",
    defs: "",
    type_defs: "",
    flags: NOCASE | MULTILINE | PLAIN_CAPS,
};

static ZIG: Spec = Spec {
    line: &["//"],
    block: &[],
    quotes: "\"'",
    keywords: "addrspace align allowzero and anyframe anytype asm async await break callconv catch comptime const continue defer else enum errdefer error export extern fn for if inline linksection noalias noinline nosuspend opaque or orelse packed pub resume return struct suspend switch test threadlocal try union unreachable usingnamespace var volatile while",
    types: "i8 u8 i16 u16 i32 u32 i64 u64 i128 u128 isize usize f16 f32 f64 f128 bool void type anyerror noreturn comptime_int comptime_float",
    constants: "true false null undefined",
    builtins: "",
    defs: "fn",
    type_defs: "",
    flags: AT,
};

static HASKELL: Spec = Spec {
    line: &["--"],
    block: &[("{-", "-}")],
    quotes: "\"",
    keywords: "case class data default deriving do else forall foreign if import in infix infixl infixr instance let module newtype of qualified then type where",
    types: "",
    constants: "True False",
    builtins: "",
    defs: "",
    type_defs: "data newtype type class",
    flags: 0,
};

static ELIXIR: Spec = Spec {
    line: &["#"],
    block: &[],
    quotes: "\"'",
    keywords: "after alias and catch cond def defimpl defmacro defmodule defp defprotocol defstruct do else end fn for if import in not or quote raise receive require rescue try unless unquote use when with",
    types: "",
    constants: "true false nil",
    builtins: "",
    defs: "def defp defmacro",
    type_defs: "defmodule",
    flags: AT | TRIPLE | MULTILINE,
};

static PROTO: Spec = Spec {
    line: &["//"],
    block: &[("/*", "*/")],
    quotes: "\"'",
    keywords: "syntax edition package import option message enum service rpc returns stream oneof map repeated optional required reserved extend extensions to max public weak",
    types: "double float int32 int64 uint32 uint64 sint32 sint64 fixed32 fixed64 sfixed32 sfixed64 bool string bytes",
    constants: "true false",
    builtins: "",
    defs: "rpc",
    type_defs: "message enum service",
    flags: 0,
};

static CMAKE: Spec = Spec {
    line: &["#"],
    block: &[],
    quotes: "\"",
    keywords: "if elseif else endif foreach endforeach while endwhile function endfunction macro endmacro return",
    types: "",
    constants: "ON OFF TRUE FALSE",
    builtins: "",
    defs: "function macro",
    type_defs: "",
    flags: NOCASE | DOLLAR | MULTILINE | PLAIN_CAPS,
};

pub(super) fn spec(lang: &str) -> Option<&'static Spec> {
    Some(match lang {
        "C" | "C header" | "C++" | "CUDA" | "Metal" => &C,
        "Objective-C" => &OBJC,
        "Rust" => &RUST_SPEC,
        "Go" => &GO,
        "JavaScript" | "TypeScript" => &TS,
        "Java" => &JAVA,
        "Kotlin" => &KOTLIN,
        "Swift" => &SWIFT,
        "Scala" => &SCALA,
        "C#" => &CSHARP,
        "Dart" => &DART,
        "Ruby" => &RUBY,
        "PHP" => &PHP,
        "Lua" => &LUA,
        "Shell" => &SHELL_SPEC,
        "PowerShell" => &POWERSHELL,
        "R" => &R,
        "Julia" => &JULIA,
        "SQL" => &SQL,
        "Zig" => &ZIG,
        "Haskell" => &HASKELL,
        "Elixir" => &ELIXIR,
        "Protobuf" => &PROTO,
        "CMake" => &CMAKE,
        _ => return None,
    })
}

pub(super) fn shell() -> &'static Spec {
    &SHELL_SPEC
}

pub(super) fn script() -> &'static Spec {
    &TS
}

fn listed(list: &str, w: &str, nocase: bool) -> bool {
    list.split_ascii_whitespace().any(|k| if nocase { k.eq_ignore_ascii_case(w) } else { k == w })
}

/// Keywords after which `/` starts a regular expression rather than dividing.
const REGEX_AFTER: &str = "return typeof instanceof in of new delete void throw case do else yield await";

fn ident_start(c: u8, spec: &Spec) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 || (c == b'$' && spec.flags & IDENT_DOLLAR != 0)
}

fn ident_end(b: &[u8], mut i: usize, end: usize, spec: &Spec) -> usize {
    while i < end && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] >= 0x80 || (b[i] == b'$' && spec.flags & IDENT_DOLLAR != 0)) {
        i += 1;
    }
    i
}

pub(super) fn find(b: &[u8], from: usize, end: usize, pat: &[u8]) -> Option<usize> {
    if pat.is_empty() || from >= end {
        return None;
    }
    b[from..end].windows(pat.len()).position(|w| w == pat).map(|k| from + k)
}

pub(super) fn line_end(b: &[u8], i: usize, end: usize) -> usize {
    b[i..end].iter().position(|&c| c == b'\n').map_or(end, |k| i + k)
}

/// End of a quoted string starting at `i` (one past the closing quote).
pub(super) fn string_end(b: &[u8], i: usize, end: usize, q: u8, escapes: bool, multiline: bool) -> usize {
    let mut j = i + 1;
    while j < end {
        let c = b[j];
        if c == b'\\' && escapes {
            j += 2;
            continue;
        }
        if c == q {
            return j + 1;
        }
        if c == b'\n' && !multiline {
            return j;
        }
        j += 1;
    }
    end
}

/// End of a number: digits, radix prefixes, separators, exponents, suffixes.
pub(super) fn number_end(b: &[u8], mut i: usize, end: usize) -> usize {
    let hex = i + 1 < end && b[i] == b'0' && matches!(b[i + 1], b'x' | b'X');
    while i < end {
        let c = b[i];
        if c.is_ascii_alphanumeric() || c == b'_' {
            i += 1;
            if !hex && matches!(c, b'e' | b'E') && i < end && matches!(b[i], b'+' | b'-') {
                i += 1;
            }
        } else if c == b'.' && i + 1 < end && b[i + 1].is_ascii_digit() {
            i += 1;
        } else {
            break;
        }
    }
    i
}

/// From an opening bracket at `i`, one past its match (or `end`), skipping
/// quoted strings.
fn matching(b: &[u8], i: usize, end: usize, open: u8, close: u8) -> usize {
    let mut depth = 0usize;
    let mut j = i;
    while j < end {
        let c = b[j];
        if c == b'"' || c == b'\'' {
            j = string_end(b, j, end, c, true, false);
            continue;
        }
        if c == open {
            depth += 1;
        } else if c == close {
            if depth <= 1 {
                return j + 1;
            }
            depth -= 1;
        }
        j += 1;
    }
    end
}

/// `$name`, `${...}`, `$(NAME)`, `$1`, `$@` → one past its end.
fn dollar_var(b: &[u8], i: usize, end: usize) -> Option<usize> {
    let n = *b.get(i + 1)?;
    if i + 1 >= end {
        return None;
    }
    match n {
        b'{' => {
            let j = matching(b, i + 1, end, b'{', b'}');
            Some(j)
        }
        b'(' => {
            let j = matching(b, i + 1, end, b'(', b')');
            let inner = &b[i + 2..j.saturating_sub(1).max(i + 2)];
            // `$(NAME)` is a Make variable; `$(cmd args)` is a command substitution.
            (!inner.is_empty() && inner.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')).then_some(j)
        }
        c if c.is_ascii_alphabetic() || c == b'_' => {
            let mut j = i + 1;
            while j < end && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            Some(j)
        }
        c if c.is_ascii_digit() || b"@?#$!*-".contains(&c) => Some(i + 2),
        _ => None,
    }
}

/// A string whose `$var`s are interpolated (shell, PHP, Make).
fn dollar_string(b: &[u8], i: usize, j: usize, runs: &mut Runs) {
    let mut seg = i;
    let mut k = i + 1;
    let close = j.saturating_sub(1).max(k);
    while k < close {
        if b[k] == b'\\' {
            k += 2;
            continue;
        }
        if b[k] == b'$' {
            if let Some(e) = dollar_var(b, k, close) {
                runs.mark(seg, k, Hl::String);
                runs.mark(k, e, Hl::Param);
                seg = e;
                k = e;
                continue;
            }
        }
        k += 1;
    }
    runs.mark(seg, j, Hl::String);
}

/// A backtick template literal with `${expr}` parts, lexed recursively.
fn template(text: &str, i: usize, end: usize, spec: &Spec, runs: &mut Runs) -> usize {
    let b = text.as_bytes();
    let mut j = i + 1;
    let mut seg = i;
    while j < end {
        match b[j] {
            b'\\' => j += 2,
            b'`' => {
                runs.mark(seg, j + 1, Hl::String);
                return j + 1;
            }
            b'$' if j + 1 < end && b[j + 1] == b'{' => {
                runs.mark(seg, j, Hl::String);
                runs.mark(j, j + 2, Hl::Interp);
                let close = matching(b, j + 1, end, b'{', b'}');
                let inner_end = if close <= end && close > j + 2 && b[close - 1] == b'}' { close - 1 } else { close };
                lex(text, j + 2, inner_end, spec, runs);
                if inner_end < close {
                    runs.mark(inner_end, close, Hl::Interp);
                }
                j = close;
                seg = j;
            }
            _ => j += 1,
        }
    }
    runs.mark(seg, end, Hl::String);
    end
}

fn regex_end(b: &[u8], i: usize, end: usize) -> Option<usize> {
    let mut j = i + 1;
    let mut class = false;
    while j < end {
        match b[j] {
            b'\\' => j += 1,
            b'[' => class = true,
            b']' => class = false,
            b'/' if !class => {
                j += 1;
                while j < end && b[j].is_ascii_alphabetic() {
                    j += 1;
                }
                return Some(j);
            }
            b'\n' => return None,
            _ => {}
        }
        j += 1;
    }
    None
}

/// `r"..."`, `r#"..."#`, `b"..."`, `br"..."`, `c"..."` starting at `i`.
fn rust_string(b: &[u8], i: usize, end: usize) -> Option<usize> {
    let mut j = i;
    if matches!(b[j], b'b' | b'c') {
        j += 1;
    }
    let raw = j < end && b[j] == b'r';
    if raw {
        j += 1;
    }
    let mut hashes = 0;
    while raw && j < end && b[j] == b'#' {
        hashes += 1;
        j += 1;
    }
    if j == i || j >= end || b[j] != b'"' {
        return None;
    }
    if !raw {
        return Some(string_end(b, j, end, b'"', true, true));
    }
    let mut k = j + 1;
    while k < end {
        if b[k] == b'"' && b[k + 1..end].iter().take(hashes).filter(|&&c| c == b'#').count() == hashes && k + hashes < end {
            return Some(k + 1 + hashes);
        }
        k += 1;
    }
    Some(end)
}

/// A Rust char literal (`'x'`, `'\n'`, `'é'`) rather than a lifetime.
fn rust_char(b: &[u8], i: usize, end: usize) -> Option<usize> {
    if i + 1 >= end {
        return None;
    }
    let n = b[i + 1];
    if n == b'\\' {
        if i + 3 > end {
            return None;
        }
        let close = b[i + 3..end.min(i + 14)].iter().position(|&c| c == b'\'')?;
        return Some(i + 4 + close);
    }
    let len = match n {
        0..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    };
    (b.get(i + 1 + len) == Some(&b'\'')).then_some(i + 2 + len)
}

fn skip_inline_space(b: &[u8], mut i: usize, end: usize) -> usize {
    while i < end && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    i
}

/// Lexes `text[start..end]` into `runs`.
pub(super) fn lex(text: &str, start: usize, end: usize, spec: &Spec, runs: &mut Runs) {
    let b = text.as_bytes();
    let has = |f: u32| spec.flags & f != 0;
    let nocase = has(NOCASE);
    let mut i = start;
    let mut bol = true;
    // The last significant byte, and the keyword just before a name.
    let mut prev = b'\n';
    let mut prev_kw: &str = "";
    while i < end {
        let c = b[i];
        if c == b'\n' {
            bol = true;
            i += 1;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let at_bol = std::mem::replace(&mut bol, false);
        let rest = &b[i..end];

        if let Some((open, close)) = spec.block.iter().find(|(o, _)| rest.starts_with(o.as_bytes())) {
            let j = find(b, i + open.len(), end, close.as_bytes()).map_or(end, |k| k + close.len());
            runs.mark(i, j, Hl::Comment);
            i = j;
            continue;
        }
        let shell_word = !has(SHELL) || i == start || b[i - 1].is_ascii_whitespace() || b[i - 1] == b';';
        if c == b'#' && has(PREPROC) && at_bol {
            let w0 = skip_inline_space(b, i + 1, end);
            let w = ident_end(b, w0, end, spec);
            runs.mark(i, w, Hl::Meta);
            let k = skip_inline_space(b, w, end);
            if matches!(&text[w0..w], "include" | "import") && k < end && b[k] == b'<' {
                let close = b[k..line_end(b, k, end)].iter().position(|&c| c == b'>').map_or(line_end(b, k, end), |p| k + p + 1);
                runs.mark(k, close, Hl::String);
                i = close;
            } else {
                i = w;
            }
            prev = b'#';
            continue;
        }
        if c == b'#' && has(RUST) && (rest.starts_with(b"#[") || rest.starts_with(b"#![")) {
            let open = i + if rest[1] == b'!' { 2 } else { 1 };
            let j = matching(b, open, end, b'[', b']');
            runs.mark(i, j, Hl::Meta);
            i = j;
            prev = b']';
            continue;
        }
        if spec.line.iter().any(|l| rest.starts_with(l.as_bytes())) && (c != b'#' || shell_word) {
            let j = line_end(b, i, end);
            runs.mark(i, j, Hl::Comment);
            i = j;
            continue;
        }
        if has(RUST) && matches!(c, b'r' | b'b' | b'c') && (i == start || !b[i - 1].is_ascii_alphanumeric()) {
            if let Some(j) = rust_string(b, i, end) {
                runs.mark(i, j, Hl::String);
                i = j;
                prev = b'"';
                continue;
            }
        }
        if has(RUST) && c == b'\'' {
            if let Some(j) = rust_char(b, i, end) {
                runs.mark(i, j, Hl::String);
                i = j;
            } else {
                let j = ident_end(b, i + 1, end, spec);
                runs.mark(i, j.max(i + 1), Hl::Type);
                i = j.max(i + 1);
            }
            prev = b'\'';
            continue;
        }
        if spec.quotes.as_bytes().contains(&c) {
            if c == b'`' && has(TEMPLATE) {
                i = template(text, i, end, spec, runs);
            } else if has(TRIPLE) && rest.starts_with(&[c, c, c]) {
                let j = find(b, i + 3, end, &[c, c, c]).map_or(end, |k| k + 3);
                runs.mark(i, j, Hl::String);
                i = j;
            } else {
                let escapes = !(has(SHELL) && c == b'\'');
                let j = string_end(b, i, end, c, escapes, has(MULTILINE) || has(SHELL) || c == b'`');
                if has(DOLLAR) && c == b'"' {
                    dollar_string(b, i, j, runs);
                } else {
                    runs.mark(i, j, Hl::String);
                }
                i = j;
            }
            prev = b'"';
            prev_kw = "";
            continue;
        }
        if c == b'$' && has(DOLLAR) {
            if let Some(j) = dollar_var(b, i, end) {
                runs.mark(i, j, Hl::Param);
                i = j;
                prev = b'a';
                continue;
            }
        }
        if c == b'@' && has(AT) && i + 1 < end && ident_start(b[i + 1], spec) {
            let mut j = ident_end(b, i + 1, end, spec);
            while j + 1 < end && b[j] == b'.' && ident_start(b[j + 1], spec) {
                j = ident_end(b, j + 1, end, spec);
            }
            runs.mark(i, j, Hl::Decorator);
            i = j;
            prev = b'a';
            continue;
        }
        if c == b'/' && has(REGEX) && b"(,=:[!&|?{};+-*%<>~^\n".contains(&prev) {
            if let Some(j) = regex_end(b, i, end) {
                runs.mark(i, j, Hl::String);
                i = j;
                prev = b'"';
                continue;
            }
        }
        let ident_like = prev.is_ascii_alphanumeric() || prev == b'_';
        if c.is_ascii_digit() || (c == b'.' && !ident_like && i + 1 < end && b[i + 1].is_ascii_digit()) {
            let j = number_end(b, i, end);
            runs.mark(i, j, Hl::Number);
            i = j;
            prev = b'0';
            continue;
        }
        if ident_start(c, spec) {
            let j = ident_end(b, i, end, spec);
            let w = &text[i..j];
            let after_dot = prev == b'.';
            let mut stop = j;
            let k = skip_inline_space(b, j, end);
            let next = (k < end).then(|| b[k]);
            let upper = w.starts_with(|c: char| c.is_ascii_uppercase()) && !has(PLAIN_CAPS);
            let hl = if has(RUST) && j < end && b[j] == b'!' && b.get(j + 1) != Some(&b'=') {
                stop = j + 1;
                Hl::Call
            } else if !after_dot && listed(spec.constants, w, nocase) {
                Hl::Constant
            } else if !after_dot && listed(spec.keywords, w, nocase) {
                Hl::Keyword
            } else if !after_dot && matches!(w, "self" | "this") {
                Hl::SelfVar
            } else if !prev_kw.is_empty() && listed(spec.defs, prev_kw, nocase) {
                Hl::Function
            } else if !prev_kw.is_empty() && listed(spec.type_defs, prev_kw, nocase) {
                Hl::Type
            } else if next == Some(b'(') {
                if listed(spec.builtins, w, false) {
                    Hl::Builtin
                } else if upper && w.chars().any(|c| c.is_lowercase()) {
                    Hl::Type
                } else {
                    Hl::Call
                }
            } else if listed(spec.types, w, nocase) {
                Hl::Type
            } else if upper && w.len() > 1 && !w.chars().any(|c| c.is_lowercase()) {
                Hl::Constant
            } else if upper {
                Hl::Type
            } else {
                Hl::Plain
            };
            if hl != Hl::Plain {
                runs.mark(i, stop, hl);
            }
            prev_kw = if hl == Hl::Keyword { w } else { "" };
            prev = if hl == Hl::Keyword && listed(REGEX_AFTER, w, false) { b'(' } else { b'a' };
            i = stop;
            continue;
        }
        prev = c;
        prev_kw = "";
        i += 1;
    }
}
