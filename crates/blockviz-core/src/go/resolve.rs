//! Which project directory a Go import path names, the way the go command
//! decides it.
//!
//! A module is a directory with a `go.mod` naming its import path
//! (`module github.com/gin-gonic/gin`). Every directory below it holding
//! `.go` files is a package whose import path is the module's plus the
//! directory, unless a nested `go.mod` starts another module. `replace`
//! directives that point at local directories join modules of one
//! repository, as `go.work` does. Code without a `go.mod` (GOPATH-era
//! repositories, a pasted folder) is matched by the longest directory that
//! ends the import path.

use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Default)]
pub struct GoMod {
    /// The directory holding `go.mod` ("" at the root).
    pub dir: String,
    /// The module's import path.
    pub path: String,
    /// Modules it requires.
    pub requires: Vec<String>,
    /// `replace` directives pointing at directories of this repository:
    /// (import path, directory).
    replaces: Vec<(String, String)>,
}

fn unquote(s: &str) -> &str {
    s.trim().trim_matches('"').trim_matches('`')
}

/// Reads the directives blockviz needs from a `go.mod`.
fn go_mod(dir: &str, text: &str) -> GoMod {
    let mut m = GoMod { dir: dir.to_string(), ..Default::default() };
    let mut block = "";
    for line in text.lines() {
        let l = line.split("//").next().unwrap_or("").trim();
        if l.is_empty() {
            continue;
        }
        if l == ")" {
            block = "";
            continue;
        }
        let (verb, rest) = match l.split_once(char::is_whitespace) {
            Some((v, r)) if block.is_empty() => (v, r.trim()),
            _ if !block.is_empty() => (block, l),
            _ => (l, ""),
        };
        if rest == "(" {
            block = match verb {
                "require" => "require",
                "replace" => "replace",
                _ => "other",
            };
            continue;
        }
        match verb {
            "module" => m.path = unquote(rest).to_string(),
            "require" => {
                if let Some(p) = rest.split_whitespace().next() {
                    m.requires.push(unquote(p).to_string());
                }
            }
            "replace" => {
                let Some((from, to)) = rest.split_once("=>") else { continue };
                let from = unquote(from.split_whitespace().next().unwrap_or(""));
                let to = unquote(to.split_whitespace().next().unwrap_or(""));
                if to.starts_with("./") || to.starts_with("../") || to == "." || to == ".." {
                    m.replaces.push((from.to_string(), join(dir, to)));
                }
            }
            _ => {}
        }
    }
    m
}

/// `use` directories of a `go.work`.
fn go_work(dir: &str, text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut inside = false;
    for line in text.lines() {
        let l = line.split("//").next().unwrap_or("").trim();
        if inside {
            if l == ")" {
                inside = false;
            } else if !l.is_empty() {
                out.push(join(dir, unquote(l)));
            }
        } else if let Some(rest) = l.strip_prefix("use") {
            let rest = rest.trim();
            if rest == "(" {
                inside = true;
            } else if !rest.is_empty() {
                out.push(join(dir, unquote(rest)));
            }
        }
    }
    out
}

fn join(dir: &str, rel: &str) -> String {
    let mut parts: Vec<&str> = if dir.is_empty() { vec![] } else { dir.split('/').collect() };
    for p in rel.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

pub fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

/// Directories whose Go files are not the project's code: `testdata` holds
/// test inputs (often deliberately broken), `vendor` copies of dependencies,
/// and hidden directories tooling. (`_examples` is skipped by the go command
/// but is code people read, so it stays.)
pub fn ignored(path: &str) -> bool {
    let dir = dir_of(path);
    !dir.is_empty() && dir.split('/').any(|d| d == "testdata" || d == "vendor" || d.starts_with('.'))
}

/// The standard library: import paths whose first element has no dot
/// (`fmt`, `net/http`), unless a module of the project claims them.
pub fn is_std(import: &str) -> bool {
    !import.split('/').next().unwrap_or("").contains('.')
}

/// The module an import path is most likely in: hosts that put repositories
/// at `host/owner/repo` keep three elements (`github.com/gin-gonic/gin`),
/// others two (`go.uber.org/zap`, `gopkg.in/yaml.v3`).
pub fn module_path(import: &str) -> String {
    let parts: Vec<&str> = import.split('/').collect();
    let keep = match parts[0] {
        "github.com" | "gitlab.com" | "bitbucket.org" | "codeberg.org" | "git.sr.ht" => 3,
        "golang.org" if parts.get(1) == Some(&"x") => 3,
        // Vanity hosts whose repositories hold several modules.
        "gorm.io" if parts.get(1) == Some(&"driver") => 3,
        _ => 2,
    };
    parts[..keep.min(parts.len())].join("/")
}

/// The package of a library path as the analysis writes it, import path
/// then members: `github.com/jmoiron/sqlx.DB.Select` → `github.com/jmoiron/sqlx`.
pub fn package_of(path: &str) -> &str {
    let tail = path.rfind('/').map(|i| i + 1).unwrap_or(0);
    match path[tail..].find('.') {
        Some(i) => &path[..tail + i],
        None => path,
    }
}

/// The name a package is known by when its own source is not at hand: the
/// last element of its path, past a major-version suffix (`/v2`), without a
/// `go-` prefix, up to the first character that cannot be in an identifier
/// (`gopkg.in/yaml.v3` is `yaml`), as goimports assumes.
pub fn assumed_name(import: &str) -> String {
    let mut parts = import.rsplit('/');
    let mut base = parts.next().unwrap_or(import);
    if base.len() > 1 && base.starts_with('v') && base[1..].bytes().all(|b| b.is_ascii_digit()) {
        if let Some(prev) = parts.next() {
            base = prev;
        }
    }
    let base = base.strip_prefix("go-").unwrap_or(base);
    let end = base.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(base.len());
    base[..end].to_string()
}

pub struct Resolver {
    /// Modules, innermost directory first.
    pub modules: Vec<GoMod>,
    /// Directories holding Go files the go command builds.
    dirs: HashSet<String>,
    /// Workspace modules named by `go.work`, by import path.
    workspace: HashMap<String, String>,
}

impl Resolver {
    /// `files` lists every path, with the text of `go.mod` and `go.work` files.
    pub fn new(files: &[(&str, Option<&str>)]) -> Self {
        let dirs: HashSet<String> = files.iter().filter(|(p, _)| p.ends_with(".go") && !ignored(p)).map(|(p, _)| dir_of(p).to_string()).collect();
        let mut modules: Vec<GoMod> = files.iter().filter(|(p, _)| p.rsplit('/').next() == Some("go.mod") && !ignored(p)).filter_map(|(p, t)| Some(go_mod(dir_of(p), (*t)?))).filter(|m| !m.path.is_empty()).collect();
        modules.sort_by(|a, b| b.dir.len().cmp(&a.dir.len()).then(a.dir.cmp(&b.dir)));
        let mut workspace = HashMap::new();
        for (p, t) in files {
            if p.rsplit('/').next() == Some("go.work") {
                for d in go_work(dir_of(p), t.unwrap_or("")) {
                    if let Some(m) = modules.iter().find(|m| m.dir == d) {
                        workspace.insert(m.path.clone(), m.dir.clone());
                    }
                }
            }
        }
        Resolver { modules, dirs, workspace }
    }

    /// The module holding a file or directory.
    pub fn module_of(&self, path: &str) -> Option<&GoMod> {
        self.modules.iter().find(|m| m.dir.is_empty() || path == m.dir || path.starts_with(&format!("{}/", m.dir)))
    }

    /// A project directory's import path: its module's plus the directory
    /// below it, else the directory itself.
    pub fn import_path(&self, dir: &str) -> String {
        match self.module_of(dir) {
            Some(m) if m.dir.is_empty() => if dir.is_empty() { m.path.clone() } else { format!("{}/{dir}", m.path) },
            Some(m) => match dir.strip_prefix(&m.dir).map(|r| r.trim_start_matches('/')) {
                Some("") => m.path.clone(),
                Some(rest) => format!("{}/{rest}", m.path),
                None => dir.to_string(),
            },
            None => dir.to_string(),
        }
    }

    fn package_dir(&self, dir: String) -> Option<String> {
        self.dirs.contains(&dir).then_some(dir)
    }

    /// The project directory an import in `from` names; `None` for the
    /// standard library and third-party modules.
    pub fn resolve(&self, from: &str, import: &str) -> Option<String> {
        if import.starts_with("./") || import.starts_with("../") {
            return self.package_dir(join(dir_of(from), import));
        }
        let under = |base: &str, dir: &str| -> Option<String> {
            match import.strip_prefix(base) {
                Some("") => Some(dir.to_string()),
                Some(rest) if rest.starts_with('/') => Some(join(dir, &rest[1..])),
                _ => None,
            }
        };
        // The module whose path is the longest prefix of the import, then
        // modules a `replace` or `go.work` points at.
        let mut best: Option<(usize, String)> = None;
        let mut consider = |base: &str, dir: &str| {
            if let Some(d) = under(base, dir) {
                if best.as_ref().is_none_or(|(len, _)| base.len() > *len) && self.dirs.contains(&d) {
                    best = Some((base.len(), d));
                }
            }
        };
        for m in &self.modules {
            consider(&m.path, &m.dir);
        }
        let own = self.module_of(from);
        for (base, dir) in own.iter().flat_map(|m| m.replaces.iter()) {
            consider(base, dir);
        }
        for (base, dir) in &self.workspace {
            consider(base, dir);
        }
        if let Some((_, d)) = best {
            return Some(d);
        }
        // Go's own source tree: the standard library's packages are imported
        // by their directory (`fmt`, `net/http`).
        if let Some(m) = self.modules.iter().find(|m| m.path == "std").filter(|_| is_std(import)) {
            return self.package_dir(join(&m.dir, import));
        }
        if is_std(import) || self.modules.iter().any(|m| import == m.path || import.starts_with(&format!("{}/", m.path))) {
            return None;
        }
        // Without a module that claims it: the longest project directory
        // that ends the import path (`github.com/user/repo/pkg/store` is
        // `pkg/store`), unless the import is another module's.
        if own.is_some_and(|m| m.requires.iter().any(|r| import == r || import.starts_with(&format!("{r}/")))) {
            return None;
        }
        self.dirs.iter().filter(|d| !d.is_empty() && (import == d.as_str() || import.ends_with(&format!("/{d}")))).max_by_key(|d| d.len()).cloned()
    }
}

/// Operating systems and architectures a file name can be constrained to
/// (`poll_linux.go`, `asm_arm64.s`).
const GOOS: &[&str] = &["aix", "android", "darwin", "dragonfly", "freebsd", "hurd", "illumos", "ios", "js", "linux", "nacl", "netbsd", "openbsd", "plan9", "solaris", "wasip1", "windows", "zos"];
const GOARCH: &[&str] = &["386", "amd64", "amd64p32", "arm", "armbe", "arm64", "arm64be", "loong64", "mips", "mipsle", "mips64", "mips64le", "mips64p32", "mips64p32le", "ppc", "ppc64", "ppc64le", "riscv", "riscv64", "s390", "s390x", "sparc", "sparc64", "wasm"];

/// The build constraint a file is under, from its `//go:build` line (or
/// legacy `// +build` lines) above the package clause, else its name
/// (`file_windows.go`, `file_linux_arm64.go`).
pub fn constraint(path: &str, text: &str) -> Option<String> {
    let mut legacy: Vec<String> = vec![];
    for line in text.lines() {
        let l = line.trim();
        if let Some(expr) = l.strip_prefix("//go:build") {
            return Some(expr.trim().to_string()).filter(|e| !e.is_empty());
        }
        if let Some(expr) = l.strip_prefix("// +build") {
            // Spaces are "or", commas "and".
            let alts: Vec<String> = expr.split_whitespace().map(|a| a.split(',').collect::<Vec<_>>().join(" && ")).collect();
            legacy.push(alts.join(" || "));
            continue;
        }
        if l.is_empty() || l.starts_with("//") {
            continue;
        }
        if l.starts_with("/*") || l.starts_with('*') {
            continue;
        }
        break;
    }
    if !legacy.is_empty() {
        return Some(if legacy.len() == 1 { legacy.remove(0) } else { legacy.iter().map(|l| if l.contains("||") { format!("({l})") } else { l.clone() }).collect::<Vec<_>>().join(" && ") });
    }
    let base = path.rsplit('/').next().unwrap_or(path);
    let stem = base.strip_suffix(".go")?;
    let stem = stem.strip_suffix("_test").unwrap_or(stem);
    let parts: Vec<&str> = stem.split('_').collect();
    // The first element is the name proper: `linux.go` is unconstrained.
    let n = parts.len();
    if n >= 3 && GOOS.contains(&parts[n - 2]) && GOARCH.contains(&parts[n - 1]) {
        return Some(format!("{} && {}", parts[n - 2], parts[n - 1]));
    }
    if n >= 2 && (GOOS.contains(&parts[n - 1]) || GOARCH.contains(&parts[n - 1])) {
        return Some(parts[n - 1].to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver(files: &[(&str, &str)]) -> Resolver {
        let listing: Vec<(&str, Option<&str>)> = files.iter().map(|(p, t)| (*p, Some(*t))).collect();
        Resolver::new(&listing)
    }

    #[test]
    fn resolves_imports_through_modules() {
        let r = resolver(&[
            ("go.mod", "module github.com/acme/shop // the shop\n\ngo 1.22\n\nrequire (\n\tgithub.com/gin-gonic/gin v1.10.0\n\tgolang.org/x/sync v0.7.0 // indirect\n)\nreplace github.com/acme/kit => ./kit\n"),
            ("main.go", ""),
            ("internal/store/store.go", ""),
            ("internal/store/sql.go", ""),
            ("api/v2/handlers.go", ""),
            ("kit/go.mod", "module github.com/acme/kit\n"),
            ("kit/log/log.go", ""),
            ("http/client.go", ""),
            ("testdata/broken.go", ""),
        ]);
        assert_eq!(r.resolve("main.go", "github.com/acme/shop/internal/store"), Some("internal/store".into()));
        assert_eq!(r.resolve("main.go", "github.com/acme/shop/api/v2"), Some("api/v2".into()));
        assert_eq!(r.resolve("main.go", "github.com/acme/shop"), Some("".into()));
        assert_eq!(r.resolve("main.go", "github.com/acme/kit/log"), Some("kit/log".into()));
        // The standard library and other modules are not the project's,
        // even when a directory has the same name.
        assert_eq!(r.resolve("main.go", "net/http"), None);
        assert_eq!(r.resolve("main.go", "github.com/gin-gonic/gin"), None);
        assert_eq!(r.resolve("main.go", "github.com/acme/shop/testdata"), None);
        assert_eq!(r.import_path("internal/store"), "github.com/acme/shop/internal/store");
        assert_eq!(r.import_path("kit/log"), "github.com/acme/kit/log");
        assert_eq!(r.module_of("kit/log/log.go").map(|m| m.path.as_str()), Some("github.com/acme/kit"));
        assert_eq!(r.modules.last().unwrap().requires, ["github.com/gin-gonic/gin", "golang.org/x/sync"]);
    }

    #[test]
    fn resolves_without_a_module() {
        let r = resolver(&[("cmd/tool/main.go", ""), ("pkg/store/store.go", ""), ("store/old.go", "")]);
        assert_eq!(r.resolve("cmd/tool/main.go", "github.com/someone/repo/pkg/store"), Some("pkg/store".into()));
        assert_eq!(r.resolve("cmd/tool/main.go", "github.com/someone/repo/store"), Some("store".into()));
        assert_eq!(r.resolve("cmd/tool/main.go", "./../../store"), Some("store".into()));
        assert_eq!(r.resolve("cmd/tool/main.go", "encoding/json"), None);
        // Go's own tree is the standard library.
        let r = resolver(&[("src/go.mod", "module std\n"), ("src/fmt/print.go", ""), ("src/net/http/server.go", "")]);
        assert_eq!(r.resolve("src/net/http/server.go", "fmt"), Some("src/fmt".into()));
    }

    #[test]
    fn names_and_constraints() {
        assert_eq!(assumed_name("gopkg.in/yaml.v3"), "yaml");
        assert_eq!(assumed_name("github.com/redis/go-redis/v9"), "redis");
        assert_eq!(assumed_name("github.com/mattn/go-sqlite3"), "sqlite3");
        assert_eq!(assumed_name("net/http"), "http");
        assert_eq!(constraint("poll_linux.go", "package poll\n"), Some("linux".into()));
        assert_eq!(constraint("asm_linux_arm64.go", ""), Some("linux && arm64".into()));
        assert_eq!(constraint("exec_windows_test.go", ""), Some("windows".into()));
        assert_eq!(constraint("linux.go", ""), None);
        assert_eq!(constraint("x.go", "// Copyright\n\n//go:build unix && !darwin\n\npackage x\n"), Some("unix && !darwin".into()));
        assert_eq!(constraint("x.go", "// +build linux,cgo darwin\n\npackage x\n"), Some("linux && cgo || darwin".into()));
        assert_eq!(constraint("x.go", "package x\n//go:build ignore\n"), None);
        assert!(ignored("pkg/testdata/x.go") && ignored("vendor/github.com/a/b.go") && ignored(".github/x.go") && !ignored("_examples/rest/main.go") && !ignored("main.go"));
    }
}
