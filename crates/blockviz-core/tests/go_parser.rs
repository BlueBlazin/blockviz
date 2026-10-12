//! The Go parser (src/go) against Go's own.
//!
//! `tests/go-oracle/dump.go` prints go/parser's tree in a canonical form, and
//! `dump` below prints ours the same way. The fixtures in
//! fixtures/go-parser cover the grammar, with go/parser's dumps committed
//! next to them (regenerate with tests/go-oracle/regen.sh), so CI compares
//! without Go. The ignored tests need a Go installation (`GOROOT`, by
//! default /usr/local/go) or take minutes:
//!
//!     cargo test --release --test go_parser go_distribution -- --ignored --exact --nocapture
//!
//! compares with go/parser live over every file of the installation;
//! `go_distribution_speed` times the parser on it, and the `damaged_*` tests
//! fuzz at length.

use blockviz_core::code::syntax::{Ranged, TextRange, TextSize};
use blockviz_core::go::ast::*;
use blockviz_core::go::walk::{self, Node, Visitor};
use blockviz_core::go::{self, scan};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

// ----- the dump ----------------------------------------------------------------

/// Control characters and backslashes escaped, as dump.go escapes them.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn pos(p: Option<TextSize>) -> i64 {
    p.map(|p| u32::from(p) as i64).unwrap_or(-1)
}

fn at(p: TextSize) -> u32 {
    p.into()
}

fn y(b: bool) -> u8 {
    b as u8
}

fn lit_kind(k: LitKind) -> &'static str {
    match k {
        LitKind::Int => "INT",
        LitKind::Float => "FLOAT",
        LitKind::Imag => "IMAG",
        LitKind::Char => "CHAR",
        LitKind::String => "STRING",
    }
}

/// One node as dump.go prints it.
fn node_line(node: Node) -> String {
    let attrs = match node {
        Node::Comment(c) => format!(" {}", escape(&c.text)),
        Node::Field(f) => format!(" names={} tag={} doc={} comment={}", f.names.len(), y(f.tag.is_some()), y(f.doc.is_some()), y(f.comment.is_some())),
        Node::FieldList(l) => format!(" open={} close={}", pos(l.opening), pos(l.closing)),
        Node::Ident(i) => format!(" {}", i.name),
        Node::Ellipsis(e) => format!(" elt={}", y(e.elt.is_some())),
        Node::BasicLit(b) => format!(" {} {}", lit_kind(b.kind), escape(&b.value)),
        Node::CompositeLit(c) => format!(" type={} lbrace={} rbrace={} elts={}", y(c.typ.is_some()), at(c.lbrace), at(c.rbrace), c.elts.len()),
        Node::ParenExpr(p) => format!(" lparen={} rparen={}", at(p.lparen), at(p.rparen)),
        Node::IndexExpr(i) => format!(" lbrack={} rbrack={}", at(i.lbrack), at(i.rbrack)),
        Node::IndexListExpr(i) => format!(" lbrack={} rbrack={} indices={}", at(i.lbrack), at(i.rbrack), i.indices.len()),
        Node::SliceExpr(s) => format!(" lbrack={} rbrack={} low={} high={} max={} slice3={}", at(s.lbrack), at(s.rbrack), y(s.low.is_some()), y(s.high.is_some()), y(s.max.is_some()), y(s.slice3)),
        Node::TypeAssertExpr(t) => format!(" lparen={} rparen={} type={}", at(t.lparen), at(t.rparen), y(t.typ.is_some())),
        Node::CallExpr(c) => format!(" lparen={} ellipsis={} rparen={} args={}", at(c.lparen), pos(c.ellipsis), at(c.rparen), c.args.len()),
        Node::UnaryExpr(u) => format!(" {}", u.op.as_str()),
        Node::BinaryExpr(b) => format!(" {} op={}", b.op.as_str(), at(b.op_pos)),
        Node::KeyValueExpr(k) => format!(" colon={}", at(k.colon)),
        Node::ArrayType(a) => format!(" len={}", y(a.len.is_some())),
        Node::FuncType(f) => format!(" func={} tparams={} results={}", pos(f.func), y(f.type_params.is_some()), y(f.results.is_some())),
        Node::ChanType(c) => {
            let dir = match c.dir {
                ChanDir::Send => "send",
                ChanDir::Recv => "recv",
                ChanDir::Both => "both",
            };
            format!(" {dir} arrow={}", pos(c.arrow))
        }
        Node::EmptyStmt(e) => format!(" implicit={}", y(e.implicit)),
        Node::LabeledStmt(l) => format!(" colon={}", at(l.colon)),
        Node::SendStmt(s) => format!(" arrow={}", at(s.arrow)),
        Node::IncDecStmt(s) => format!(" {} tok={}", s.tok.as_str(), at(s.tok_pos)),
        Node::AssignStmt(a) => format!(" {} tok={} lhs={} rhs={}", a.tok.as_str(), at(a.tok_pos), a.lhs.len(), a.rhs.len()),
        Node::ReturnStmt(r) => format!(" results={}", r.results.len()),
        Node::BranchStmt(b) => format!(" {} label={}", b.tok.as_str(), y(b.label.is_some())),
        Node::BlockStmt(b) => format!(" rbrace={}", pos(b.rbrace)),
        Node::IfStmt(i) => format!(" init={} else={}", y(i.init.is_some()), y(i.else_.is_some())),
        Node::CaseClause(c) => {
            let list = if c.is_default() { "default".to_string() } else { format!("list={}", c.list.len()) };
            format!(" {list} colon={}", at(c.colon))
        }
        Node::SwitchStmt(s) => format!(" init={} tag={}", y(s.init.is_some()), y(s.tag.is_some())),
        Node::TypeSwitchStmt(s) => format!(" init={}", y(s.init.is_some())),
        Node::CommClause(c) => format!(" comm={} colon={}", y(c.comm.is_some()), at(c.colon)),
        Node::ForStmt(f) => format!(" init={} cond={} post={}", y(f.init.is_some()), y(f.cond.is_some()), y(f.post.is_some())),
        Node::RangeStmt(r) => format!(" key={} value={} {} tok={}", y(r.key.is_some()), y(r.value.is_some()), r.tok.map(|t| t.as_str()).unwrap_or("none"), pos(r.tok_pos)),
        Node::ImportSpec(i) => format!(" name={} doc={} comment={}", y(i.name.is_some()), y(i.doc.is_some()), y(i.comment.is_some())),
        Node::ValueSpec(v) => format!(" names={} type={} values={} doc={} comment={}", v.names.len(), y(v.typ.is_some()), v.values.len(), y(v.doc.is_some()), y(v.comment.is_some())),
        Node::TypeSpec(t) => format!(" tparams={} assign={} doc={} comment={}", y(t.type_params.is_some()), pos(t.assign), y(t.doc.is_some()), y(t.comment.is_some())),
        Node::GenDecl(g) => format!(" {} lparen={} rparen={} specs={} doc={}", g.tok.as_str(), pos(g.lparen), pos(g.rparen), g.specs.len(), y(g.doc.is_some())),
        Node::FuncDecl(f) => format!(" recv={} body={} doc={}", y(f.recv.is_some()), y(f.body.is_some()), y(f.doc.is_some())),
        Node::File(f) => format!(" package={} doc={}", pos(f.package), y(f.doc.is_some())),
        _ => String::new(),
    };
    let r = node.range();
    format!("{} {} {}{}", node.kind(), at(r.start()), at(r.end()), attrs)
}

/// The tree as dump.go prints go/parser's: "ok", the nodes in walk order
/// indented by depth, then the comment groups.
fn dump(file: &File) -> String {
    struct Dumper {
        out: String,
        depth: usize,
    }
    impl<'a> Visitor<'a> for Dumper {
        fn enter(&mut self, node: Node<'a>) -> bool {
            for _ in 0..self.depth {
                self.out.push_str("  ");
            }
            self.out.push_str(&node_line(node));
            self.out.push('\n');
            self.depth += 1;
            true
        }
        fn leave(&mut self, _: Node<'a>) {
            self.depth -= 1;
        }
    }
    let mut d = Dumper { out: String::from("ok\n"), depth: 0 };
    walk::walk(&mut d, Node::File(file));
    d.out.push_str("comments\n");
    for g in &file.comments {
        let _ = writeln!(d.out, "  {}", node_line(Node::CommentGroup(g)));
        for c in &g.list {
            let _ = writeln!(d.out, "    {}", node_line(Node::Comment(c)));
        }
    }
    d.out
}

/// The first line where two dumps differ, with context.
fn first_difference(want: &str, got: &str) -> String {
    let (w, g): (Vec<&str>, Vec<&str>) = (want.lines().collect(), got.lines().collect());
    let i = w.iter().zip(&g).position(|(a, b)| a != b).unwrap_or(w.len().min(g.len()));
    let from = i.saturating_sub(3);
    let show = |lines: &[&str]| lines[from.min(lines.len())..(i + 3).min(lines.len())].join("\n");
    format!("line {}:\n--- go/parser\n{}\n--- ours\n{}", i + 1, show(&w), show(&g))
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/go-parser")
}

// ----- go/parser's dumps, committed --------------------------------------------

#[test]
fn fixtures_match_go_parser() {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(fixture_dir()).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "go")).collect();
    entries.sort();
    assert!(entries.len() >= 10, "fixtures missing");
    let mut checked = 0;
    let mut kinds = std::collections::BTreeSet::new();
    for path in entries {
        let golden = path.with_extension("golden");
        let Ok(want) = std::fs::read_to_string(&golden) else { continue };
        let src = std::fs::read_to_string(&path).unwrap();
        let parsed = go::parse(&src);
        assert!(parsed.errors.is_empty(), "{}: {:?}", path.display(), parsed.errors);
        let got = dump(&parsed.file);
        assert!(want == got, "{} differs from go/parser at {}", path.display(), first_difference(&want, &got));
        walk::inspect(Node::File(&parsed.file), |n| {
            kinds.insert(n.kind());
            true
        });
        checked += 1;
    }
    assert!(checked >= 10, "goldens missing: run tests/go-oracle/regen.sh");
    // Every node type but the Bad* ones, which only errors make.
    let missing: Vec<&str> = ALL_KINDS.iter().copied().filter(|k| !k.starts_with("Bad") && !kinds.contains(k)).collect();
    assert!(missing.is_empty(), "no fixture has {missing:?}");
}

const ALL_KINDS: &[&str] = &[
    "Comment", "CommentGroup", "Field", "FieldList", "BadExpr", "Ident", "Ellipsis", "BasicLit", "FuncLit", "CompositeLit", "ParenExpr", "SelectorExpr", "IndexExpr", "IndexListExpr", "SliceExpr", "TypeAssertExpr", "CallExpr", "StarExpr", "UnaryExpr", "BinaryExpr", "KeyValueExpr", "ArrayType", "StructType", "FuncType", "InterfaceType", "MapType", "ChanType", "BadStmt", "DeclStmt", "EmptyStmt", "LabeledStmt", "ExprStmt", "SendStmt", "IncDecStmt", "AssignStmt", "GoStmt", "DeferStmt", "ReturnStmt", "BranchStmt", "BlockStmt", "IfStmt", "CaseClause", "SwitchStmt", "TypeSwitchStmt", "CommClause", "SelectStmt", "ForStmt", "RangeStmt", "ImportSpec", "ValueSpec", "TypeSpec", "BadDecl", "GenDecl", "FuncDecl", "File",
];

// ----- go/parser live, over a Go installation ----------------------------------

fn go_root() -> Option<PathBuf> {
    let root = std::env::var_os("GOROOT").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/usr/local/go"));
    root.join("src").is_dir().then_some(root)
}

fn go_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            go_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "go") {
            out.push(p);
        }
    }
}

/// Builds tests/go-oracle/dump.go.
fn oracle() -> PathBuf {
    let bin = Path::new(env!("CARGO_TARGET_TMPDIR")).join("go-oracle-dump");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/go-oracle/dump.go");
    let status = std::process::Command::new("go").arg("build").arg("-o").arg(&bin).arg(&src).status().expect("go is not installed");
    assert!(status.success(), "building the oracle failed");
    bin
}

/// Differences from go/parser kept on purpose, by file: none so far.
const KNOWN_DIFFERENCES: &[(&str, &str)] = &[];

/// Every `.go` file under `$GOROOT/src` and `$GOROOT/test` (`GOROOT`
/// defaults to /usr/local/go), parsed by go/parser and by us: where
/// go/parser accepts a file our tree must be identical and error-free; where
/// it rejects one we must report an error too. Files without a package
/// clause are the exception: we parse them as snippets.
#[test]
#[ignore]
fn go_distribution() {
    use std::io::{BufRead, Write};
    let Some(root) = go_root() else {
        eprintln!("no Go installation; set GOROOT");
        return;
    };
    let mut files = vec![];
    go_files(&root.join("src"), &mut files);
    go_files(&root.join("test"), &mut files);
    let bin = oracle();
    let mut child = std::process::Command::new(bin).arg("-stdin").stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let list: String = files.iter().map(|f| format!("{}\n", f.display())).collect();
    let feeder = std::thread::spawn(move || stdin.write_all(list.as_bytes()));
    let stdout = std::io::BufReader::with_capacity(1 << 20, child.stdout.take().unwrap());

    // Our side runs on a large stack: this test compares trees, and
    // `deeply_nested_input_is_an_error` checks the stack.
    let report = std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(move || {
            let mut lines = stdout.lines().map(|l| l.unwrap()).peekable();
            let (mut identical, mut both_rejected, mut snippets, mut known) = (0, 0, 0, 0);
            let mut failures = vec![];
            let mut ours = std::time::Duration::ZERO;
            let mut bytes = 0usize;
            while let Some(header) = lines.next() {
                let path = header.strip_prefix("### ").expect("record header").to_string();
                let status = lines.next().unwrap();
                let mut want = String::new();
                if status == "ok" {
                    want.push_str("ok\n");
                    while lines.peek().is_some_and(|l| !l.starts_with("### ")) {
                        want.push_str(&lines.next().unwrap());
                        want.push('\n');
                    }
                }
                let Ok(src) = std::fs::read_to_string(&path) else {
                    failures.push(format!("{path}: not UTF-8"));
                    continue;
                };
                bytes += src.len();
                let start = std::time::Instant::now();
                let parsed = std::panic::catch_unwind(|| go::parse(&src));
                ours += start.elapsed();
                let Ok(parsed) = parsed else {
                    failures.push(format!("{path}: panicked"));
                    continue;
                };
                if KNOWN_DIFFERENCES.iter().any(|(p, _)| path.ends_with(p)) {
                    known += 1;
                    continue;
                }
                if status == "ok" {
                    if !parsed.errors.is_empty() {
                        failures.push(format!("{path}: go/parser accepts it, we report {:?}", &parsed.errors[..parsed.errors.len().min(3)]));
                        continue;
                    }
                    let got = dump(&parsed.file);
                    if got == want {
                        identical += 1;
                    } else {
                        failures.push(format!("{path}: {}", first_difference(&want, &got)));
                    }
                } else if !parsed.errors.is_empty() {
                    both_rejected += 1;
                } else if status.contains("expected 'package'") {
                    snippets += 1;
                } else {
                    failures.push(format!("{path}: go/parser rejects it ({status}), we accept it"));
                }
            }
            let total = identical + both_rejected + snippets + known + failures.len();
            let report = format!(
                "{total} files: {identical} identical, {both_rejected} rejected by both, {snippets} without a package clause, {known} known differences, {} failures; our parse time {:.0} ms for {:.1} MB",
                failures.len(),
                ours.as_secs_f64() * 1000.0,
                bytes as f64 / 1e6
            );
            (report, failures)
        })
        .unwrap();
    let (report, failures) = report.join().unwrap();
    feeder.join().unwrap().unwrap();
    assert!(child.wait().unwrap().success());
    eprintln!("{report}");
    for f in failures.iter().take(30) {
        eprintln!("\n{f}");
    }
    assert!(failures.is_empty(), "{report}");
}

/// Parse time for the distribution (`src`, then `src` and `test`), without
/// the oracle; the best of three runs.
#[test]
#[ignore]
fn go_distribution_speed() {
    let Some(root) = go_root() else { return };
    for dirs in [&["src"][..], &["src", "test"]] {
        let mut files = vec![];
        for d in dirs {
            go_files(&root.join(d), &mut files);
        }
        let sources: Vec<String> = files.iter().filter_map(|f| std::fs::read_to_string(f).ok()).collect();
        let bytes: usize = sources.iter().map(|s| s.len()).sum();
        let lines: usize = sources.iter().map(|s| s.lines().count()).sum();
        let (parse, drop) = (0..3)
            .map(|_| {
                let start = std::time::Instant::now();
                let trees: Vec<go::Parsed> = sources.iter().map(|s| go::parse(s)).collect();
                let parsed = start.elapsed();
                std::mem::drop(trees);
                (parsed, start.elapsed() - parsed)
            })
            .min()
            .unwrap();
        eprintln!("{dirs:?}: {} files, {lines} lines, {:.1} MB: parsed in {:.0} ms, trees dropped in {:.0} ms", sources.len(), bytes as f64 / 1e6, parse.as_secs_f64() * 1000.0, drop.as_secs_f64() * 1000.0);
    }
}

// ----- what the tree promises on any input ---------------------------------------

/// Ranges inside the text and on character boundaries; every node inside
/// its parent, except doc and line comments, which sit before and after it;
/// comments in order; errors inside the text.
fn check_tree(src: &str, parsed: &go::Parsed) -> Result<(), String> {
    struct Checker<'s> {
        src: &'s str,
        parents: Vec<TextRange>,
        problem: Option<String>,
    }
    impl<'a> Visitor<'a> for Checker<'_> {
        fn enter(&mut self, node: Node<'a>) -> bool {
            let r = node.range();
            let (s, e) = (at(r.start()) as usize, at(r.end()) as usize);
            if self.problem.is_none() {
                if e > self.src.len() || !self.src.is_char_boundary(s) || !self.src.is_char_boundary(e) {
                    self.problem = Some(format!("{} {s}..{e} is outside the text or splits a character", node.kind()));
                } else if let Some(p) = self.parents.last() {
                    if !matches!(node, Node::CommentGroup(_)) && !p.contains_range(r) {
                        self.problem = Some(format!("{} {s}..{e} is outside its parent {p:?}", node.kind()));
                    }
                }
            }
            self.parents.push(r);
            true
        }
        fn leave(&mut self, _: Node<'a>) {
            self.parents.pop();
        }
    }
    let mut checker = Checker { src, parents: vec![], problem: None };
    walk::walk(&mut checker, Node::File(&parsed.file));
    if let Some(p) = checker.problem {
        return Err(p);
    }
    let mut last = 0;
    for c in parsed.file.comments.iter().flat_map(|g| &g.list) {
        let (s, e) = (at(c.range.start()) as usize, at(c.range.end()) as usize);
        if s < last || e > src.len() || !src[s..e].starts_with('/') {
            return Err(format!("comment {s}..{e} out of order or not a comment"));
        }
        last = e;
    }
    match parsed.errors.iter().find(|e| e.offset as usize > src.len() || !src.is_char_boundary(e.offset as usize)) {
        Some(e) => Err(format!("error {e:?} is outside the text")),
        None => Ok(()),
    }
}

fn parse_checked(src: &str) -> go::Parsed {
    let parsed = go::parse(src);
    if let Err(problem) = check_tree(src, &parsed) {
        panic!("{problem}\n--- input:\n{src}");
    }
    parsed
}

/// SplitMix64: deterministic, so failures reproduce.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Fragments that break Go in interesting ways.
const DAMAGE: &[&str] = &[
    "{", "}", "(", ")", "[", "]", ",", ";", ":", ":=", "=", ".", "...", "<-", "~", "|", "*", "&", "\n", " ", "func", "func f() {", "func (r T) m(", "type", "type T[", "var", "const", "import", "package", "if", "else", "for", "range", "switch", "case", "default:", "select", "go", "defer", "return", "break", "goto L", "L:", "struct{", "interface{", "map[", "chan", "x", "_", "0x", "1e", "0b2", "1__0", "0x1.p", "'", "'ab'", "'\\", "\"", "\"\\q\"", "`", "/*", "*/", "//", "//line x:0\n", "\u{feff}", "\0", "é", "日本", "\r", "\t", "@", "#", "?", "[T any]", "x.(type)", ")(", "}{", "...)", "<-chan", "chan<-", "[]", "[...]", "T{", "~int|", "\n}\n", "\nfunc g() {}\n",
];

/// One to three random edits: deleting or inserting text, or deleting,
/// duplicating, swapping or replacing tokens.
fn damage(src: &str, rng: &mut Rng) -> String {
    let mut s = src.to_string();
    for _ in 0..1 + rng.below(3) {
        let tokens: Vec<TextRange> = scan::tokens(&s).map(|l| l.range).filter(|r| !r.is_empty()).collect();
        let tok = |rng: &mut Rng| tokens.get(rng.below(tokens.len())).map(|r| (at(r.start()) as usize, at(r.end()) as usize));
        match rng.below(7) {
            0 => {
                let a = floor_boundary(&s, rng.below(s.len() + 1));
                let b = floor_boundary(&s, a + 1 + rng.below(8));
                s.replace_range(a..b, "");
            }
            1 => {
                let a = floor_boundary(&s, rng.below(s.len() + 1));
                s.insert_str(a, DAMAGE[rng.below(DAMAGE.len())]);
            }
            2 => {
                if let Some((a, b)) = tok(rng) {
                    s.replace_range(a..b, "");
                }
            }
            3 => {
                if let Some((a, b)) = tok(rng) {
                    let t = s[a..b].to_string();
                    s.insert_str(b, &t);
                }
            }
            4 => {
                if let Some((a, b)) = tok(rng) {
                    s.replace_range(a..b, DAMAGE[rng.below(DAMAGE.len())]);
                }
            }
            _ => {
                let i = rng.below(tokens.len().saturating_sub(1));
                if let (Some(x), Some(y)) = (tokens.get(i), tokens.get(i + 1)) {
                    let (xa, xb, ya, yb) = (at(x.start()) as usize, at(x.end()) as usize, at(y.start()) as usize, at(y.end()) as usize);
                    let swapped = format!("{}{}{}{}{}", &s[..xa], &s[ya..yb], &s[xb..ya], &s[xa..xb], &s[yb..]);
                    s = swapped;
                }
            }
        }
    }
    s
}

fn fixture_sources() -> Vec<String> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(fixture_dir()).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "go")).collect();
    paths.sort();
    paths.iter().map(|p| std::fs::read_to_string(p).unwrap()).collect()
}

#[test]
fn every_prefix_of_the_fixtures_parses_to_a_sound_tree() {
    for src in fixture_sources() {
        let step = (src.len() / 150).max(1);
        for cut in (0..=src.len()).step_by(step) {
            parse_checked(&src[..floor_boundary(&src, cut)]);
        }
    }
}

#[test]
fn damaged_fixtures_parse_to_sound_trees() {
    let mut rng = Rng(0x5eed_0001);
    for src in fixture_sources() {
        for _ in 0..300 {
            parse_checked(&damage(&src, &mut rng));
        }
    }
}

/// The same, at length: `GO_FUZZ_ROUNDS` (default 20,000) damaged inputs per
/// fixture, from seed `GO_FUZZ_SEED`.
#[test]
#[ignore]
fn damaged_fixtures_at_length() {
    let rounds: usize = std::env::var("GO_FUZZ_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(20_000);
    let seed: u64 = std::env::var("GO_FUZZ_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let mut rng = Rng(seed);
    for src in fixture_sources() {
        for _ in 0..rounds {
            parse_checked(&damage(&src, &mut rng));
        }
    }
}

/// Every file of a Go installation, damaged `GO_FUZZ_ROUNDS` (default 20)
/// times each, on a large stack (the distribution nests deeply).
#[test]
#[ignore]
fn damaged_go_distribution() {
    let Some(root) = go_root() else { return };
    let rounds: usize = std::env::var("GO_FUZZ_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
    let mut files = vec![];
    go_files(&root.join("src"), &mut files);
    go_files(&root.join("test"), &mut files);
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(move || {
            let mut rng = Rng(3);
            for path in files {
                let Ok(src) = std::fs::read_to_string(&path) else { continue };
                for _ in 0..rounds {
                    let damaged = damage(&src, &mut rng);
                    let parsed = go::parse(&damaged);
                    if let Err(problem) = check_tree(&damaged, &parsed) {
                        let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("go-fuzz-failure.go");
                        std::fs::write(&out, &damaged).unwrap();
                        panic!("{}: {problem} (input in {})", path.display(), out.display());
                    }
                }
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn random_token_soup_parses_in_linear_time() {
    let mut rng = Rng(7);
    let mut soup = String::from("package p\n");
    while soup.len() < 300_000 {
        soup.push_str(DAMAGE[rng.below(DAMAGE.len())]);
        soup.push(' ');
    }
    let start = std::time::Instant::now();
    let parsed = parse_checked(&soup);
    assert!(!parsed.errors.is_empty());
    assert!(start.elapsed().as_secs() < 10, "{:?} for 300 KB", start.elapsed());
}

#[test]
fn odd_files() {
    for src in [
        "", "\n", "\u{feff}", "\u{feff}package p", "// only a comment", "/* unterminated", "/* a */ /* b */", "package", "package p", "package p;", "\r\n\r\n", "package p\r\nfunc f() {}\r\n", "\0", "`", "\"", "'", "func", "}", ")))", "{{{", "package p\nfunc", "package p\nfunc f(", "package p\nimport", "package p\nimport (", "package p\ntype T[", "package p\nvar x = []int{", "package p\nfunc f() { L: }", "package p\nfunc f() { for range", "package p\nfunc f() { switch x.(type) { case", "package p\nvar x = <-chan<-", "package p\ntype T[P *C", "package p\n//line :0\n", "package p\n/*line x.go:1:0*/\n", "x := 1", "{ x }", "package p\nfunc (", "package p\nfunc (r T", "\u{feff}\u{feff}",
    ] {
        parse_checked(src);
    }

    let p = go::parse("");
    assert!(p.errors.is_empty() && p.file.decls.is_empty() && p.file.name.is_none());
    let p = go::parse("// only a comment\n");
    assert!(p.errors.is_empty());
    assert_eq!(p.file.comments.len(), 1);
    let p = go::parse("\u{feff}package p\n");
    assert!(p.errors.is_empty());
    assert_eq!(p.file.package, Some(TextSize::from(3)));
    let p = go::parse("package p\n\u{feff}");
    assert_eq!(p.errors[0].message, "illegal byte order mark");
    let p = go::parse("package p\r\n\r\n// Doc.\r\nfunc f() {}\r\n");
    assert!(p.errors.is_empty());
    let Decl::Func(f) = &p.file.decls[0] else { panic!() };
    assert_eq!(f.doc.as_ref().unwrap().text(), "Doc.\n");
}

#[test]
fn long_lines() {
    let s = format!("package p\nvar s = \"{}\"\n", "x".repeat(1 << 20));
    let p = parse_checked(&s);
    assert!(p.errors.is_empty());
    let s = format!("package p\nvar a = []int{{{}}}\n", "1, ".repeat(100_000));
    let p = parse_checked(&s);
    assert!(p.errors.is_empty());
    let Decl::Gen(g) = &p.file.decls[0] else { panic!() };
    let Spec::Value(v) = &g.specs[0] else { panic!() };
    let Expr::CompositeLit(c) = &v.values[0] else { panic!() };
    assert_eq!(c.elts.len(), 100_000);
    let s = format!("package p\nfunc f() {{ {} }}\n", "a(); ".repeat(100_000));
    let p = parse_checked(&s);
    assert!(p.errors.is_empty());
    let s = format!("package p\nvar {} int\n", "x".repeat(1 << 20));
    assert!(parse_checked(&s).errors.is_empty());
}

// ----- nesting -------------------------------------------------------------------

/// Runs `f` on a thread with the WebAssembly build's 8 MB stack.
fn on_wasm_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new().stack_size(8 << 20).spawn(f).unwrap().join().unwrap()
}

fn nested(n: usize) -> Vec<(&'static str, String)> {
    vec![
        ("parentheses", format!("package p\nvar x = {}1{}", "(".repeat(n), ")".repeat(n))),
        ("calls", format!("package p\nvar x = {}y{}", "f(".repeat(n), ")".repeat(n))),
        ("unary operators", format!("package p\nvar x = {}y", "- ".repeat(n))),
        ("receives", format!("package p\nvar x = {}c", "<-".repeat(n))),
        ("pointers", format!("package p\nvar x {}int", "*".repeat(n))),
        ("slices", format!("package p\nvar x {}int", "[]".repeat(n))),
        ("maps", format!("package p\nvar x {}int", "map[int]".repeat(n))),
        ("channels", format!("package p\nvar x {}int", "chan ".repeat(n))),
        ("structs", format!("package p\ntype T {}int{}", "struct { a ".repeat(n), "}".repeat(n))),
        ("func results", format!("package p\nvar x {}int", "func() ".repeat(n))),
        ("composite literals", format!("package p\nvar x = T{}{}", "{".repeat(n), "}".repeat(n))),
        ("func literals", format!("package p\nvar x = {}{}", "func() {".repeat(n), "}".repeat(n))),
        ("blocks", format!("package p\nfunc f() {}{}", "{".repeat(n), "}".repeat(n))),
        ("ifs", format!("package p\nfunc f() {{ {}{} }}", "if x { ".repeat(n), "}".repeat(n))),
        ("labels", format!("package p\nfunc f() {{ {} }}", "L: ".repeat(n))),
        ("binary chain", format!("package p\nvar x = {}1", "1 + ".repeat(n))),
        ("selector chain", format!("package p\nvar x = a{}", ".b".repeat(n))),
        ("call chain", format!("package p\nvar x = f{}", "()".repeat(n))),
        ("index chain", format!("package p\nvar x = a{}", "[0]".repeat(n))),
        ("else-if chain", format!("package p\nfunc f() {{ if a {{}} {} }}", "else if a {} ".repeat(n))),
        ("union chain", format!("package p\ntype T interface {{ int{} }}", " | int".repeat(n))),
        ("type parameter union", format!("package p\ntype T[P int{}] int", " | int".repeat(n))),
    ]
}

#[test]
fn deeply_nested_input_is_an_error_not_a_crash() {
    for (name, src) in nested(100_000) {
        let (errors, sound) = on_wasm_stack(move || {
            let p = go::parse(&src);
            let sound = check_tree(&src, &p);
            (p.errors, sound)
        });
        assert!(errors.iter().any(|e| e.message == "exceeded max nesting depth"), "{name}: {errors:?}");
        sound.unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

#[test]
fn nesting_real_code_reaches_parses() {
    // Go's own code nests at most 260 deep, and time/tzdata concatenates
    // 7,100 strings in one expression. Nested function literals count twice
    // (an expression and a statement each).
    for (name, src) in nested(400).into_iter().chain([("long concatenation", format!("package p\nvar x = {}\"\"", "\"x\" + ".repeat(7_500)))]) {
        let errors = on_wasm_stack(move || go::parse(&src).errors);
        assert!(errors.is_empty(), "{name}: {errors:?}");
    }
}

// ----- recovery ------------------------------------------------------------------

/// The names of the functions called by expression statements.
fn calls(stmts: &[Stmt]) -> Vec<&str> {
    stmts
        .iter()
        .filter_map(|s| match s {
            Stmt::Expr(e) => match &e.x {
                Expr::Call(c) => match &c.fun {
                    Expr::Ident(i) => Some(i.name.as_str()),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn decl_names(file: &File) -> Vec<&str> {
    file.decls
        .iter()
        .filter_map(|d| match d {
            Decl::Func(f) => Some(f.name.name.as_str()),
            Decl::Gen(g) => g.specs.first().map(|s| match s {
                Spec::Type(t) => t.name.name.as_str(),
                Spec::Value(v) => v.names[0].name.as_str(),
                Spec::Import(i) => i.path.value.as_str(),
            }),
            Decl::Bad(_) => Some("<bad>"),
        })
        .collect()
}

#[test]
fn a_broken_statement_keeps_its_siblings() {
    let src = "package p\n\nfunc f() {\n\ta()\n\tx := ;\n\tb()\n\ty := )\n\tc()\n\tif {\n\t}\n\td()\n\tz = = 1\n\te()\n\tfoo bar baz\n\tg()\n\th(1, 2\n\ti()\n}\n\nfunc after() {}\n";
    let p = parse_checked(src);
    let Decl::Func(f) = &p.file.decls[0] else { panic!() };
    // As in go/parser, a newline in an open argument list is taken for a
    // missing comma: `i()` becomes an argument of `h`.
    assert_eq!(calls(&f.body.as_ref().unwrap().list), ["a", "b", "c", "d", "e", "g", "h"]);
    assert_eq!(decl_names(&p.file), ["f", "after"]);
    let messages: Vec<(usize, &str)> = p.errors.iter().map(|e| (e.offset as usize, e.message.as_str())).collect();
    assert_eq!(messages[0], (src.find("x := ;").unwrap() + 5, "expected operand, found ';'"));
    assert_eq!(messages[1], (src.find("y := )").unwrap() + 5, "expected operand, found ')'"));
    assert_eq!(messages[2], (src.find("if {").unwrap() + 3, "missing condition in if statement"));
    assert_eq!(messages[3], (src.find("= = 1").unwrap() + 2, "expected operand, found '='"));
    assert_eq!(messages[4], (src.find("bar").unwrap(), "expected ';', found bar"));
    assert_eq!(messages[5], (src.find("h(1, 2").unwrap() + 6, "missing ',' before newline in argument list"));
    assert!(messages.len() >= 6, "{messages:?}");
    // What could not be read is kept as Bad* nodes covering it.
    let mut bad = vec![];
    walk::inspect(Node::File(&p.file), |n| {
        if n.kind().starts_with("Bad") {
            bad.push((n.kind(), text(src, &n)));
        }
        true
    });
    assert_eq!(bad, [("BadExpr", ""), ("BadExpr", ""), ("BadStmt", ")\n\t"), ("BadExpr", ""), ("BadExpr", "= 1"), ("BadExpr", "")]);
}

#[test]
fn a_broken_declaration_keeps_the_ones_after_it() {
    let src = "package p\n\nfunc a() {\n\tif x {\n\t\tb()\n}\n\nfunc c() {\n\td(\n}\n\nfunc (r *T) e() {\n\tx := []int{1, 2\n}\n\nfunc g() {}\n\nvar v = 1 +\n\nfunc h() {}\n\nconst k =\n\nfunc i() {}\n\ntype T struct {\n\tf int\n\nfunc j() {}\n\nx := 1\n\nfunc k() {}\n\nvar w = foo(\n\nfunc l() {}\n\nimport \"late\"\n\nfunc m() {}\n";
    let p = parse_checked(src);
    assert_eq!(decl_names(&p.file), ["a", "c", "e", "g", "v", "h", "k", "i", "T", "j", "<bad>", "k", "w", "l", "\"late\"", "m"]);
    let at_line = |line: usize| src.split_inclusive('\n').take(line - 1).map(str::len).sum::<usize>();
    assert_eq!(p.errors[0].offset as usize, at_line(8), "{:?}", p.errors);
    assert_eq!(p.errors[0].message, "expected '}', found 'func'");
    assert!(p.errors.iter().any(|e| e.message == "imports must appear before other declarations"));
}

#[test]
fn broken_code_inside_expressions() {
    for src in [
        "package p\nvar x = []int{1, 2, @, 4}\nfunc f() {}\n",
        "package p\nvar x = map[string]int{\"a\": }\nfunc f() {}\n",
        "package p\nvar x = f(a b c)\nfunc f() {}\n",
        "package p\nvar x = a.\nfunc f() {}\n",
        "package p\nvar x = s[1:2:]\nfunc f() {}\n",
        "package p\nvar x = T[]\nfunc f() {}\n",
        "package p\ntype T[P any,,] struct{}\nfunc f() {}\n",
        "package p\ntype T[P any struct{}\nfunc f() {}\n",
        "package p\nfunc g[T any](x T) {}\nfunc (r R) m[T any]() {}\nfunc f() {}\n",
        "package p\ntype I interface { m[T any]() }\nfunc f() {}\n",
        "package p\nfunc g(a int, string) {}\nfunc f() {}\n",
        "package p\nvar x = <- <-chan int\nfunc f() {}\n",
    ] {
        let p = parse_checked(src);
        assert!(!p.errors.is_empty(), "{src}");
        assert_eq!(decl_names(&p.file).last(), Some(&"f"), "{src}: {:?}", p.errors);
    }
}

#[test]
fn snippets_without_a_package_clause() {
    let src = "import \"fmt\"\n\n// Greet says hello.\nfunc Greet() { fmt.Println(\"hi\") }\n";
    let p = parse_checked(src);
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    assert!(p.file.name.is_none() && p.file.package.is_none() && p.file.doc.is_none());
    assert_eq!(p.file.imports().map(|i| i.path.value.as_str()).collect::<Vec<_>>(), ["\"fmt\""]);
    let Decl::Func(f) = &p.file.decls[1] else { panic!() };
    assert_eq!(f.doc.as_ref().unwrap().text(), "Greet says hello.\n");
    assert_eq!(p.file.range, TextRange::new(0.into(), (src.len() as u32 - 1).into()));
}

// ----- Go 1.24 syntax the oracle predates ------------------------------------------

fn type_spec<'a>(file: &'a File, name: &str) -> &'a TypeSpec {
    file.decls
        .iter()
        .find_map(|d| match d {
            Decl::Gen(g) => g.specs.iter().find_map(|s| match s {
                Spec::Type(t) if t.name.name == name => Some(t),
                _ => None,
            }),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no type {name}"))
}

fn func<'a>(file: &'a File, name: &str) -> &'a FuncDecl {
    file.decls
        .iter()
        .find_map(|d| match d {
            Decl::Func(f) if f.name.name == name => Some(f),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no func {name}"))
}

fn text<'s>(src: &'s str, node: &impl Ranged) -> &'s str {
    &src[node.range()]
}

#[test]
fn go_1_24_syntax() {
    let src = std::fs::read_to_string(fixture_dir().join("go124.go")).unwrap();
    let p = parse_checked(&src);
    assert!(p.errors.is_empty(), "{:?}", p.errors);

    // `[P *C | ~int]` is one type parameter whose constraint is a union.
    let t = type_spec(&p.file, "PtrUnion");
    let tp = t.type_params.as_ref().unwrap();
    assert_eq!(tp.list.len(), 1);
    assert_eq!(tp.list[0].names[0].name, "P");
    let Expr::Binary(union) = &tp.list[0].typ else { panic!("{:?}", tp.list[0].typ) };
    assert_eq!(union.op, Token::Or);
    assert!(matches!(&union.x, Expr::Star(s) if matches!(&s.x, Expr::Ident(c) if c.name == "C")));
    assert!(matches!(&union.y, Expr::Unary(u) if u.op == Token::Tilde));
    assert_eq!(text(&src, union.as_ref()), "*C | ~int");
    assert_eq!(text(&src, &tp.list[0]), "P *C | ~int");
    assert!(matches!(t.typ, Expr::StructType(_)));

    let t = type_spec(&p.file, "Set");
    let tp = t.type_params.as_ref().unwrap();
    let fields: Vec<(&str, &str)> = tp.list.iter().map(|f| (f.names[0].name.as_str(), text(&src, &f.typ))).collect();
    assert_eq!(fields, [("T", "~int | ~string"), ("PT", "*T")]);

    // A generic alias.
    let t = type_spec(&p.file, "Vec");
    assert!(t.type_params.is_some());
    assert_eq!(t.assign.map(|a| &src[at(a) as usize..at(a) as usize + 1]), Some("="));
    assert_eq!(text(&src, &t.typ), "[]T");
    // The doc of an ungrouped declaration is the declaration's.
    assert!(t.doc.is_none());
    let docs: Vec<String> = p.file.decls.iter().filter_map(|d| match d {
        Decl::Gen(g) => g.doc.as_ref().map(|d| d.text()),
        _ => None,
    }).collect();
    assert!(docs.contains(&"A generic alias (Go 1.24).\n".to_string()), "{docs:?}");

    // `~x` as an expression.
    let f = func(&p.file, "neg");
    let Stmt::Return(r) = &f.body.as_ref().unwrap().list[0] else { panic!() };
    assert!(matches!(&r.results[0], Expr::Unary(u) if u.op == Token::Tilde && text(&src, u.as_ref()) == "~x"));

    // Range over an integer and over a function: plain range clauses.
    let Stmt::Range(r) = &func(&p.file, "iter").body.as_ref().unwrap().list[0] else { panic!() };
    assert_eq!(text(&src, r.key.as_ref().unwrap()), "i");
    assert_eq!(r.tok, Some(Token::Define));
    assert_eq!(&src[at(r.range_pos) as usize..][..5], "range");
    assert!(matches!(&r.x, Expr::BasicLit(b) if b.kind == LitKind::Int && b.value == "3"));
    let Stmt::Range(r) = &func(&p.file, "use").body.as_ref().unwrap().list[0] else { panic!() };
    assert_eq!(text(&src, &r.x), "iter");
}

// ----- the scanner, the walk and the conveniences -----------------------------------

#[test]
fn tokens_include_comments_and_inserted_semicolons() {
    use scan::Token::*;
    let src = "x := 1 // one\ny /* two */ ++\n'a'";
    let toks: Vec<(scan::Token, &str, u32)> = scan::tokens(src).map(|l| (l.token, &src[l.range], at(l.range.start()))).collect();
    assert_eq!(
        toks,
        [(Ident, "x", 0), (Define, ":=", 2), (Int, "1", 5), (Semicolon, "", 7), (Comment, "// one", 7), (Ident, "y", 14), (Comment, "/* two */", 16), (Inc, "++", 26), (Semicolon, "", 28), (Char, "'a'", 29), (Semicolon, "", 32)]
    );
}

#[test]
fn scanner_errors_point_at_the_problem() {
    for (src, at, message) in [
        ("var s = \"a\\qb\"", 11, "unknown escape sequence"),
        ("var s = \"abc", 8, "string literal not terminated"),
        ("var s = `abc", 8, "raw string literal not terminated"),
        ("var c = 'ab'", 8, "illegal rune literal"),
        ("var c = ''", 8, "illegal rune literal"),
        ("var c = '\\400'", 10, "escape sequence is invalid Unicode code point"),
        ("var c = '\\uD800'", 10, "escape sequence is invalid Unicode code point"),
        ("var n = 0x", 10, "hexadecimal literal has no digits"),
        ("var n = 09", 9, "invalid digit '9' in octal literal"),
        ("var n = 1__0", 10, "'_' must separate successive digits"),
        ("var n = 0b12", 11, "invalid digit '2' in binary literal"),
        ("var n = 0x1.0", 13, "hexadecimal mantissa requires a 'p' exponent"),
        ("var n = 1e", 10, "exponent has no digits"),
        ("var n = 0b1.0", 11, "invalid radix point in binary literal"),
        ("var x = 1 @ 2", 10, "illegal character U+0040 '@'"),
        ("/* open", 0, "comment not terminated"),
        ("//line foo.go:x\nvar x int", 14, "invalid line number: x"),
    ] {
        let p = go::parse(src);
        let e = p.errors.first().unwrap_or_else(|| panic!("{src}: no error"));
        assert_eq!((e.offset, e.message.as_str()), (at, message), "{src}");
    }
}

#[test]
fn walk_visits_in_go_ast_order() {
    let src = "package p\n\n// F doc.\nfunc F(a int) (b int) { return a + 1 }\n";
    let p = go::parse(src);
    let mut kinds = vec![];
    walk::inspect(Node::File(&p.file), |n| {
        kinds.push(n.kind());
        true
    });
    assert_eq!(
        kinds,
        ["File", "Ident", "FuncDecl", "CommentGroup", "Comment", "Ident", "FuncType", "FieldList", "Field", "Ident", "Ident", "FieldList", "Field", "Ident", "Ident", "BlockStmt", "ReturnStmt", "BinaryExpr", "Ident", "BasicLit"]
    );
    let mut kinds = vec![];
    walk::inspect(Node::File(&p.file), |n| {
        kinds.push(n.kind());
        !matches!(n, Node::FuncType(_) | Node::BlockStmt(_))
    });
    assert_eq!(kinds, ["File", "Ident", "FuncDecl", "CommentGroup", "Comment", "Ident", "FuncType", "BlockStmt"]);

    struct Depth(usize, usize);
    impl<'a> Visitor<'a> for Depth {
        fn enter(&mut self, _: Node<'a>) -> bool {
            self.0 += 1;
            self.1 = self.1.max(self.0);
            true
        }
        fn leave(&mut self, _: Node<'a>) {
            self.0 -= 1;
        }
    }
    let mut d = Depth(0, 0);
    walk::walk(&mut d, Node::File(&p.file));
    assert_eq!((d.0, d.1), (0, 6));
}

#[test]
fn doc_comments_and_literal_text() {
    let src = "// Package p.\npackage p\n\nimport (\n\t// Doc.\n\t\"fmt\" // Line.\n)\n\n// T is a type.\n//\n// It has two paragraphs.\n//go:generate stringer\ntype T struct {\n\t// X doc.\n\tX int `json:\"x\"` // X line.\n}\n\n/*\nBlock doc.\n*/\nvar v = 0x1F + 'a' + 1.5i\n";
    let p = parse_checked(src);
    assert!(p.errors.is_empty());
    assert_eq!(p.file.doc.as_ref().unwrap().text(), "Package p.\n");
    let i = p.file.imports().next().unwrap();
    assert_eq!((i.doc.as_ref().unwrap().text(), i.comment.as_ref().unwrap().text()), ("Doc.\n".into(), "Line.\n".into()));
    let Decl::Gen(g) = &p.file.decls[1] else { panic!() };
    assert_eq!(g.doc.as_ref().unwrap().text(), "T is a type.\n\nIt has two paragraphs.\n");
    let Spec::Type(t) = &g.specs[0] else { panic!() };
    let Expr::StructType(s) = &t.typ else { panic!() };
    let x = &s.fields.list[0];
    assert_eq!((x.doc.as_ref().unwrap().text(), x.comment.as_ref().unwrap().text()), ("X doc.\n".into(), "X line.\n".into()));
    assert_eq!(x.tag.as_ref().unwrap().value, "`json:\"x\"`");
    let Decl::Gen(g) = &p.file.decls[2] else { panic!() };
    assert_eq!(g.doc.as_ref().unwrap().text(), "Block doc.\n");
    let Spec::Value(v) = &g.specs[0] else { panic!() };
    let mut lits = vec![];
    walk::inspect(Node::from(&v.values[0]), |n| {
        if let Node::BasicLit(b) = n {
            lits.push((b.kind, b.value.as_str()));
        }
        true
    });
    assert_eq!(lits, [(LitKind::Int, "0x1F"), (LitKind::Char, "'a'"), (LitKind::Imag, "1.5i")]);
}
