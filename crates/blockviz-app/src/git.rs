//! The git repository a folder is in: its branches and recent history, and
//! the folder's files as they were at a revision, to compare the working
//! tree against. Runs the `git` command; without it, or outside a
//! repository, there is nothing to compare against and every answer is empty.

#![allow(dead_code)]

use crate::compare::{FileChange, FileStatus};
use crate::live::{File, Files, MAX_TEXT};
use blockviz_core::project::ignored;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A repository, seen from a folder in it.
#[derive(Clone, Debug)]
pub struct Repo {
    root: PathBuf,
    /// The folder's path within the repository, "" at its root.
    prefix: String,
}

/// A commit, as a list of recent history shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    /// When it was made, in seconds since 1970.
    pub when: i64,
}

impl Repo {
    /// The repository `folder` is in, if any.
    pub fn find(folder: &Path) -> Option<Repo> {
        let out = text(run(folder, &["rev-parse", "--show-toplevel", "--show-prefix"])?)?;
        let mut lines = out.lines();
        let root = PathBuf::from(lines.next()?);
        let prefix = lines.next().unwrap_or("").trim_end_matches('/').to_string();
        Some(Repo { root, prefix })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The branch checked out, or None when HEAD is detached.
    pub fn branch(&self) -> Option<String> {
        self.git(&["symbolic-ref", "--quiet", "--short", "HEAD"])
    }

    /// The branch work merges into: the one origin/HEAD names (as
    /// `origin/main`), else the first of main and master that exists here
    /// or on origin. It may be the branch checked out.
    pub fn default_branch(&self) -> Option<String> {
        let remote = self.git(&["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"]);
        remote.into_iter().chain(["main", "master", "origin/main", "origin/master"].map(String::from)).find(|b| self.resolve(b).is_some())
    }

    /// The commit `a` and `b` last had in common.
    pub fn merge_base(&self, a: &str, b: &str) -> Option<String> {
        self.git(&["merge-base", "--end-of-options", a, b])
    }

    /// The commit `rev` names, as a full sha.
    pub fn resolve(&self, rev: &str) -> Option<String> {
        self.git(&["rev-parse", "--verify", "--quiet", "--end-of-options", &format!("{rev}^{{commit}}")])
    }

    /// The last `n` commits of the branch checked out, newest first.
    pub fn log(&self, n: usize) -> Vec<Commit> {
        let n = n.to_string();
        let format = "--format=%H%x1f%h%x1f%an%x1f%at%x1f%s";
        let Some(out) = self.git(&["log", "--no-color", "--no-show-signature", "-n", &n, format, "HEAD", "--"]) else { return vec![] };
        out.lines()
            .filter_map(|l| {
                let mut f = l.splitn(5, '\x1f');
                let (sha, short, author, when) = (f.next()?.into(), f.next()?.into(), f.next()?.into(), f.next()?.parse().ok()?);
                Some(Commit { sha, short, author, when, subject: f.next().unwrap_or("").into() })
            })
            .collect()
    }

    /// The folder's files as they were at `rev`, by the rules `live::read`
    /// reads them from disk: paths relative to the folder, ignored paths left
    /// out, no text over the size limit or when not UTF-8. Submodules and
    /// symbolic links are not files here.
    pub fn files_at(&self, rev: &str) -> Files {
        let mut files = Files::new();
        let Some(sha) = self.resolve(rev) else { return files };
        let Some(tree) = run(&self.root, &["ls-tree", "-r", "-z", "--long", &format!("{sha}:{}", self.prefix)]) else { return files };
        let mut wanted = vec![];
        for entry in tree.split(|&b| b == 0) {
            let entry = String::from_utf8_lossy(entry);
            // "<mode> <type> <object> <size>\t<path>"
            let Some((meta, path)) = entry.split_once('\t') else { continue };
            let meta: Vec<&str> = meta.split_whitespace().collect();
            let &[mode, kind, oid, size] = meta.as_slice() else { continue };
            if kind != "blob" || mode == "120000" || ignored(path) {
                continue;
            }
            let bytes = size.parse().unwrap_or(0);
            files.insert(path.to_string(), File { text: None, bytes });
            if bytes < MAX_TEXT {
                wanted.push((path.to_string(), oid.to_string()));
            }
        }
        let (paths, oids): (Vec<String>, Vec<String>) = wanted.into_iter().unzip();
        for (path, blob) in paths.iter().zip(self.blobs(oids)) {
            if let Some(f) = files.get_mut(path) {
                f.text = blob.and_then(|b| String::from_utf8(b).ok());
            }
        }
        files
    }

    /// The contents of the objects `oids`, in order, from one `git cat-file`.
    fn blobs(&self, oids: Vec<String>) -> Vec<Option<Vec<u8>>> {
        if oids.is_empty() {
            return vec![];
        }
        let n = oids.len();
        let spawned = Command::new("git").args(["cat-file", "--batch"]).current_dir(&self.root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
        let Ok(mut child) = spawned else { return vec![] };
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return vec![];
        };
        // Asked from a thread of its own: git answers as it reads, and with
        // both pipes full neither side would move.
        let writer = std::thread::spawn(move || {
            let mut w = BufWriter::new(stdin);
            for o in oids {
                if writeln!(w, "{o}").is_err() {
                    break;
                }
            }
        });
        let mut r = BufReader::new(stdout);
        let mut out = Vec::with_capacity(n);
        let mut header = String::new();
        while out.len() < n {
            header.clear();
            if !matches!(r.read_line(&mut header), Ok(1..)) {
                break;
            }
            // "<oid> <type> <size>", or "<oid> missing".
            let Some(size) = header.trim_end().rsplit(' ').next().and_then(|s| s.parse::<usize>().ok()) else {
                out.push(None);
                continue;
            };
            let mut blob = vec![0; size + 1];
            if r.read_exact(&mut blob).is_err() {
                break;
            }
            blob.pop();
            out.push(Some(blob));
        }
        drop(r);
        let _ = child.kill();
        let _ = child.wait();
        let _ = writer.join();
        out
    }

    fn git(&self, args: &[&str]) -> Option<String> {
        text(run(&self.root, args)?)
    }
}

/// How the files of `head` differ from those of `base`, by path. A file
/// removed and one added with the same text are one file renamed, matched
/// first among files of the same name.
pub fn changes(base: &Files, head: &Files) -> Vec<FileChange> {
    let change = |path: &str, status, previous: Option<&str>| FileChange { path: path.to_string(), status, previous: previous.map(String::from) };
    let mut out: Vec<FileChange> = head.iter().filter(|(p, f)| base.get(*p).is_some_and(|b| b.text != f.text || b.bytes != f.bytes)).map(|(p, _)| change(p, FileStatus::Modified, None)).collect();
    let added: Vec<&str> = head.keys().filter(|p| !base.contains_key(*p)).map(String::as_str).collect();
    let mut gone: HashMap<&str, Vec<&str>> = HashMap::new();
    for p in base.keys().filter(|p| !head.contains_key(*p)) {
        if let Some(t) = content(base, p) {
            gone.entry(t).or_default().push(p);
        }
    }
    let name = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
    let mut renamed: HashMap<&str, &str> = HashMap::new();
    for same_name in [true, false] {
        for &a in &added {
            if renamed.contains_key(a) {
                continue;
            }
            let Some(olds) = content(head, a).and_then(|t| gone.get_mut(t)) else { continue };
            if let Some(i) = olds.iter().position(|r| !same_name || name(r) == name(a)) {
                renamed.insert(a, olds.remove(i));
            }
        }
    }
    for &a in &added {
        match renamed.get(a) {
            Some(&r) => out.push(change(a, FileStatus::Renamed, Some(r))),
            None => out.push(change(a, FileStatus::Added, None)),
        }
    }
    for p in base.keys().filter(|p| !head.contains_key(*p) && !renamed.values().any(|r| r == p)) {
        out.push(change(p, FileStatus::Removed, None));
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// A file's text, when it can tell the file apart: text that says nothing
/// (an empty `__init__.py`) does not.
fn content<'a>(files: &'a Files, path: &str) -> Option<&'a str> {
    files[path].text.as_deref().filter(|t| !t.trim().is_empty())
}

fn run(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git").args(args).current_dir(dir).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then_some(out.stdout)
}

/// Output as one value: its text without the final newline, None when empty.
fn text(out: Vec<u8>) -> Option<String> {
    let s = String::from_utf8(out).ok()?;
    let s = s.trim_end_matches('\n');
    (!s.is_empty()).then(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live;

    fn git(dir: &Path, args: &[&str]) {
        let config = ["-c", "user.name=Test", "-c", "user.email=test@example.com", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null"];
        let out = Command::new("git").args(config).args(args).current_dir(dir).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn write(dir: &Path, path: &str, data: impl AsRef<[u8]>) {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
    }

    fn keys(files: &Files) -> Vec<&str> {
        files.keys().map(String::as_str).collect()
    }

    #[test]
    fn reads_a_repository_and_its_files_at_a_revision() {
        let dir = std::env::temp_dir().join(format!("blockviz-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(Repo::find(&dir).is_none_or(|r| r.root() != dir.canonicalize().unwrap()));

        git(&dir, &["init", "-q", "-b", "main"]);
        write(&dir, "a.py", "x = 1\n");
        write(&dir, "pkg/__init__.py", "");
        write(&dir, "pkg/b.py", "def f():\n    return 1\n");
        write(&dir, "pkg/util/old.py", "def helper():\n    return 2\n");
        write(&dir, "node_modules/dep/index.js", "module.exports = 1\n");
        write(&dir, "logo.bin", [0u8, 159, 146, 150, 0]);
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "First"]);

        let repo = Repo::find(&dir).unwrap();
        assert_eq!(repo.root(), dir.canonicalize().unwrap());
        assert_eq!(repo.prefix(), "");
        let first = repo.resolve("HEAD").unwrap();
        assert_eq!(first.len(), 40);
        assert_eq!(repo.resolve("no-such-branch"), None);
        assert_eq!(repo.branch().as_deref(), Some("main"));

        // A branch: a file changed, one moved (with a copy elsewhere under
        // another name), an empty one removed and another added.
        git(&dir, &["checkout", "-q", "-b", "feature"]);
        write(&dir, "a.py", "x = 2\n");
        std::fs::create_dir_all(dir.join("pkg/helpers")).unwrap();
        git(&dir, &["mv", "pkg/util/old.py", "pkg/helpers/old.py"]);
        write(&dir, "other/copy.py", "def helper():\n    return 2\n");
        git(&dir, &["rm", "-q", "pkg/__init__.py"]);
        write(&dir, "lib/__init__.py", "");
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "Second"]);

        assert_eq!(repo.branch().as_deref(), Some("feature"));
        assert_eq!(repo.default_branch().as_deref(), Some("main"));
        assert_eq!(repo.merge_base("HEAD", "main"), Some(first.clone()));
        let log = repo.log(10);
        assert_eq!(log.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["Second", "First"]);
        assert_eq!(log[1].sha, first);
        assert!(first.starts_with(&log[1].short) && log[1].author == "Test" && log[1].when > 1_600_000_000);

        let before = repo.files_at(&first);
        assert_eq!(keys(&before), ["a.py", "logo.bin", "pkg/__init__.py", "pkg/b.py", "pkg/util/old.py"]);
        assert_eq!(before["a.py"].text.as_deref(), Some("x = 1\n"));
        assert_eq!((before["logo.bin"].text.as_deref(), before["logo.bin"].bytes), (None, 5));
        let after = repo.files_at("HEAD");
        assert_eq!(keys(&after), ["a.py", "lib/__init__.py", "logo.bin", "other/copy.py", "pkg/b.py", "pkg/helpers/old.py"]);
        assert!(repo.files_at("no-such-branch").is_empty());

        let change = |path: &str, status, previous: Option<&str>| FileChange { path: path.into(), status, previous: previous.map(String::from) };
        assert_eq!(
            changes(&before, &after),
            [
                change("a.py", FileStatus::Modified, None),
                change("lib/__init__.py", FileStatus::Added, None),
                change("other/copy.py", FileStatus::Added, None),
                change("pkg/__init__.py", FileStatus::Removed, None),
                change("pkg/helpers/old.py", FileStatus::Renamed, Some("pkg/util/old.py")),
            ]
        );

        // The working tree read from disk, against HEAD: only what is not committed.
        write(&dir, "pkg/b.py", "def f():\n    return 3\n");
        assert_eq!(changes(&after, &live::read(&dir)), [change("pkg/b.py", FileStatus::Modified, None)]);

        // A subfolder sees its own files.
        let sub = Repo::find(&dir.join("pkg")).unwrap();
        assert_eq!((sub.root(), sub.prefix()), (repo.root(), "pkg"));
        assert_eq!(keys(&sub.files_at(&first)), ["__init__.py", "b.py", "util/old.py"]);
        assert_eq!(keys(&sub.files_at("HEAD")), ["b.py", "helpers/old.py"]);

        // origin/HEAD names the default branch when there is one.
        git(&dir, &["update-ref", "refs/remotes/origin/trunk", "main"]);
        git(&dir, &["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/trunk"]);
        assert_eq!(repo.default_branch().as_deref(), Some("origin/trunk"));

        git(&dir, &["checkout", "-q", "--detach"]);
        assert_eq!(repo.branch(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn renames_pair_files_of_the_same_text() {
        let files = |list: &[(&str, &str)]| -> Files { list.iter().map(|(p, t)| (p.to_string(), File { text: Some(t.to_string()), bytes: t.len() as u64 })).collect() };
        let base = files(&[("a/x.py", "one"), ("b/y.py", "one"), ("same.py", "s")]);
        let head = files(&[("c/y.py", "one"), ("d/x.py", "one"), ("same.py", "s")]);
        let found: Vec<(String, Option<String>)> = changes(&base, &head).into_iter().map(|c| (c.path, c.previous)).collect();
        assert_eq!(found, [("c/y.py".into(), Some("b/y.py".into())), ("d/x.py".into(), Some("a/x.py".into()))]);
    }
}
