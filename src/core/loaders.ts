import { browserCache, type RepoCache, type SavedFile, type SavedRepo } from './cache';
import type { CompareInfo, Comparison, FileChange, InputFile, Project } from './types';

// Keep in step with `IGNORED_DIRS` in crates/blockviz-core/src/project.rs.
const IGNORED = new Set(['.git', 'node_modules', '__pycache__', '.venv', 'venv', '.mypy_cache', '.pytest_cache', '.ruff_cache', '.tox', '.idea', '.vscode', '.ipynb_checkpoints', 'dist', 'build', 'target', '.next', '.cache', 'site-packages', '.eggs', '.turbo', '.nuxt', '.svelte-kit', '.output', '.vercel', '.parcel-cache', '.yarn', 'bower_components', 'storybook-static']);
const BINARY = /\.(png|jpe?g|gif|webp|ico|bmp|tiff?|pdf|zip|tar|gz|tgz|7z|bz2|xz|pt|pth|bin|ckpt|safetensors|onnx|h5|pkl|npy|npz|parquet|arrow|mp3|wav|flac|ogg|mp4|mov|webm|ttf|otf|woff2?|so|dylib|dll|exe|o|a|class|jar|pyc|db|sqlite)$/i;
const MAX_FILE = 2_000_000;
const MAX_FILES = 4000;
const MAX_TOTAL = 40_000_000;

export function ignoredPath(path: string): boolean {
  const parts = path.split('/');
  return parts.slice(0, -1).some(p => IGNORED.has(p) || p.endsWith('.egg-info')) || parts[parts.length - 1] === '.DS_Store';
}

const wantsText = (path: string, size: number) => size <= MAX_FILE && !BINARY.test(path);

export type Progress = (message: string, fraction?: number) => void;

/** karpathy/nanoGPT, bundled as an example. */
export async function exampleProject(): Promise<Project> {
  const { files } = await import('./example');
  return { name: 'nanoGPT', files: [...files].sort((a, b) => a.path.localeCompare(b.path)), source: 'github.com/karpathy/nanoGPT' };
}

async function readFiles(entries: { path: string; file: File }[], progress: Progress): Promise<InputFile[]> {
  const out: InputFile[] = [];
  let total = 0;
  const kept = entries.filter(e => !ignoredPath(e.path)).slice(0, MAX_FILES);
  for (let i = 0; i < kept.length; i++) {
    const { path, file } = kept[i];
    if (i % 25 === 0) progress(`Reading files… ${i}/${kept.length}`, i / kept.length);
    if (wantsText(path, file.size) && total + file.size <= MAX_TOTAL) {
      total += file.size;
      out.push({ path, text: await file.text(), bytes: file.size });
    } else {
      out.push({ path, bytes: file.size });
    }
  }
  return out;
}

/** From `<input type="file" webkitdirectory>`. */
export async function folderProject(list: FileList, progress: Progress): Promise<Project> {
  const entries = Array.from(list).map(file => ({ path: (file as File & { webkitRelativePath: string }).webkitRelativePath || file.name, file }));
  // Files picked without their folder (phones can't pick folders) have bare names.
  const first = entries[0]?.path ?? '';
  const name = first.includes('/') ? first.split('/')[0] : entries.length === 1 ? first : 'files';
  return { name, files: await readFiles(entries, progress), source: 'local folder' };
}

/** From a drag-and-drop of files and/or folders. */
export async function droppedProject(items: DataTransferItemList, progress: Progress): Promise<Project> {
  const entries: { path: string; file: File }[] = [];
  const walk = async (entry: FileSystemEntry, prefix: string): Promise<void> => {
    if (entries.length >= MAX_FILES) return;
    const path = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isFile) {
      const file = await new Promise<File>((res, rej) => (entry as FileSystemFileEntry).file(res, rej));
      entries.push({ path, file });
    } else if (entry.isDirectory && !IGNORED.has(entry.name)) {
      const reader = (entry as FileSystemDirectoryEntry).createReader();
      for (;;) {
        const batch = await new Promise<FileSystemEntry[]>((res, rej) => reader.readEntries(res, rej));
        if (!batch.length) break;
        for (const e of batch) await walk(e, path);
      }
    }
  };
  const roots = Array.from(items).map(i => i.webkitGetAsEntry()).filter((e): e is FileSystemEntry => !!e);
  for (const r of roots) await walk(r, '');
  progress(`Reading ${entries.length} files…`);
  const name = roots.length === 1 && roots[0].isDirectory ? roots[0].name : 'dropped files';
  return { name, files: await readFiles(entries, progress), source: 'dropped files' };
}

/** A GitHub location: a repository (a branch, a folder), a pull request, or two versions compared. */
export interface GitHubRef {
  owner: string;
  repo: string;
  branch?: string;
  path?: string;
  /** A pull request's number. */
  pull?: number;
  /** Two branches (or commits) compared, `base...head`; no base means the default branch. */
  compare?: { base?: string; head: string };
}

export function parseGitHub(input: string): GitHubRef | null {
  let s = input.trim().replace(/^https?:\/\//, '').replace(/^www\./, '');
  // `owner/repo#123` names a pull request.
  const short = s.match(/^(?:github\.com\/)?([\w.-]+)\/([\w.-]+?)(?:\.git)?#(\d+)$/);
  if (short) return { owner: short[1], repo: short[2], pull: Number(short[3]) };
  s = s.replace(/[?#].*$/, '').replace(/\/$/, '');
  const pull = s.match(/^(?:github\.com\/)?([\w.-]+)\/([\w.-]+)\/pulls?\/(\d+)(?:\/.*)?$/);
  if (pull) return { owner: pull[1], repo: pull[2], pull: Number(pull[3]) };
  const cmp = s.match(/^(?:github\.com\/)?([\w.-]+)\/([\w.-]+)\/compare\/(?:(.+?)\.\.\.?)?([^.].*)$/);
  if (cmp) return { owner: cmp[1], repo: cmp[2], compare: { base: cmp[3], head: cmp[4] } };
  const m = s.replace(/\.git$/, '').match(/^(?:github\.com\/)?([\w.-]+)\/([\w.-]+)(?:\/tree\/([^/]+)(?:\/(.+))?)?$/);
  return m ? { owner: m[1], repo: m[2], branch: m[3], path: m[4] } : null;
}

/** Whether a GitHub link names two versions (a pull request or a comparison) rather than one. */
export function isComparison(ref: GitHubRef | null): boolean {
  return !!ref && (ref.pull !== undefined || !!ref.compare);
}

/** How long a saved repository opens without asking GitHub whether it changed. */
const FRESH_FOR = 24 * 60 * 60 * 1000;
let shared: RepoCache | undefined;

export interface GitHubOptions {
  /** Where downloaded repositories are kept. */
  cache?: RepoCache;
  /** Ask GitHub for the latest commit even if a saved copy is recent
   * (opening a repository on purpose, not reloading or following a link). */
  latest?: boolean;
  now?: () => number;
  /** Stops downloading; what was downloaded is not saved. */
  signal?: AbortSignal;
}

/**
 * A GitHub repository's files. A reload, a link or Back opens the copy saved
 * in this browser without downloading anything while it is recent; otherwise
 * GitHub lists the files and only those that changed are downloaded. When
 * GitHub cannot be reached (or its rate limit is spent), the saved copy opens.
 */
export async function githubProject(input: string, progress: Progress, { cache = (shared ??= browserCache()), latest = false, now = Date.now, signal }: GitHubOptions = {}): Promise<Project> {
  const ref = parseGitHub(input);
  if (!ref) throw new Error('Enter a GitHub repository like github.com/karpathy/nanoGPT.');
  const key = repoKey(ref);
  const saved = await cache.get(key).catch(() => undefined);
  if (saved && !latest && now() - saved.saved < FRESH_FOR) return fromSaved(saved);
  try {
    const repo = await download(ref, key, known(saved), progress, now, signal);
    await cache.put(repo).catch(() => undefined);
    return fromSaved(repo);
  } catch (e) {
    if (saved && !(e instanceof Missing)) return fromSaved(saved);
    throw e;
  }
}

/**
 * Both versions of a pull request (the head, and the commit it branched off
 * its base, as GitHub's "Files changed" compares them) or of two branches
 * compared. Saved like a repository; files the two share, or that a saved
 * copy of the repository already has, are downloaded once.
 */
export async function githubComparison(input: string, progress: Progress, { cache = (shared ??= browserCache()), latest = false, now = Date.now, signal }: GitHubOptions = {}): Promise<Comparison> {
  const ref = parseGitHub(input);
  if (!ref || !isComparison(ref)) throw new Error('Enter a pull request like github.com/karpathy/nanoGPT/pull/756.');
  const key = repoKey(ref);
  const saved = await cache.get(key).catch(() => undefined);
  const usable = saved?.compare && saved.base ? saved : undefined;
  if (usable && !latest && now() - usable.saved < FRESH_FOR) return comparisonOf(usable);
  try {
    const plain = await cache.get(repoKey({ owner: ref.owner, repo: ref.repo })).catch(() => undefined);
    const repo = await downloadComparison(ref, key, known(plain, usable), progress, now, signal);
    await cache.put(repo).catch(() => undefined);
    return comparisonOf(repo);
  } catch (e) {
    if (usable && !(e instanceof Missing)) return comparisonOf(usable);
    throw e;
  }
}

/** GitHub says there is no such repository, branch or pull request: a saved copy is no answer. */
class Missing extends Error {}

/** Names a repository (and branch and folder), pull request or comparison however its URL was written. */
function repoKey(ref: GitHubRef): string {
  const repo = `github.com/${ref.owner}/${ref.repo}`.toLowerCase();
  if (ref.pull !== undefined) return `${repo}/pull/${ref.pull}`;
  if (ref.compare) return `${repo}/compare/${ref.compare.base ?? ''}...${ref.compare.head}`;
  return repo + (ref.branch ? `/tree/${ref.branch}` : '') + (ref.path ? `/${ref.path.replace(/\/$/, '')}` : '');
}

const plainFiles = (files: SavedFile[]): InputFile[] => files.map(({ path, text, bytes }) => (text === undefined ? { path, bytes } : { path, text, bytes }));

function fromSaved(r: SavedRepo): Project {
  return { name: r.name, files: plainFiles(r.files), source: r.source, description: r.description };
}

function comparisonOf(r: SavedRepo): Comparison {
  const head = fromSaved(r);
  return { head, base: { ...head, files: plainFiles(r.base ?? []) }, info: r.compare! };
}

/** Texts already downloaded, by the hash GitHub names their content by. */
function known(...saved: (SavedRepo | undefined)[]): Map<string, string> {
  const out = new Map<string, string>();
  for (const r of saved) for (const f of [...(r?.files ?? []), ...(r?.base ?? [])]) if (f.sha && f.text !== undefined) out.set(f.sha, f.text);
  return out;
}

/** A GitHub API answer; a missing thing is `Missing`, a spent rate limit says so. */
async function gh<T>(url: string, what: string, signal?: AbortSignal): Promise<T> {
  const res = await fetch(url, { signal });
  if (res.status === 404 || res.status === 422) throw new Missing(`GitHub has no ${what}.`);
  if (res.status === 403 || res.status === 429) throw new Error('GitHub rate limit reached. Try again later, or open a local folder.');
  if (!res.ok) throw new Error(`GitHub returned ${res.status} for ${what}.`);
  return res.json() as Promise<T>;
}

/** A branch or tag name in a URL path, its slashes kept. */
const refPath = (ref: string) => ref.split('/').map(encodeURIComponent).join('/');

interface Blob { path: string; full: string; size: number; sha?: string }

/** The files of a commit, branch or tree, under `prefix`, that are the project's own. */
async function listTree(api: string, rev: string, prefix: string, what: string, signal?: AbortSignal): Promise<Blob[]> {
  const tree = await gh<{ tree: { path: string; type: string; size?: number; sha?: string }[]; truncated: boolean }>(`${api}/git/trees/${refPath(rev)}?recursive=1`, what, signal);
  return tree.tree
    .filter(t => t.type === 'blob' && t.path.startsWith(prefix) && !ignoredPath(t.path))
    .slice(0, MAX_FILES)
    .map(t => ({ path: t.path.slice(prefix.length), full: t.path, size: t.size ?? 0, sha: t.sha }));
}

/** Texts not yet `known` among `blobs` (what downloading them will fetch). */
const unknown = (blobs: Blob[], known: Map<string, string>) => blobs.filter(b => wantsText(b.path, b.size) && !(b.sha && known.has(b.sha)));

/** How long to wait before trying a failed download again (twice). */
const RETRY_MS = [250, 1000];

/** A file's text, or undefined if `url` has none. A network or server
 * error is tried again; one that persists leaves the file without text. */
async function fetchText(url: string, signal?: AbortSignal): Promise<string | undefined> {
  for (const wait of [0, ...RETRY_MS]) {
    if (wait) await new Promise(resolve => setTimeout(resolve, wait));
    try {
      const res = await fetch(url, { signal });
      if (res.ok) return await res.text();
      if (res.status < 500) return undefined;
    } catch (e) {
      if (signal?.aborted) throw e;
    }
  }
  return undefined;
}

/**
 * The files of `blobs` from the first of `raws` (raw.githubusercontent.com
 * prefixes) that has each; texts already `known` by their hash are reused,
 * and what is downloaded becomes known. `tick` counts each download.
 */
async function fetchFiles(raws: string[], blobs: Blob[], known: Map<string, string>, tick: () => void, signal?: AbortSignal): Promise<SavedFile[]> {
  const files: SavedFile[] = [];
  let total = 0;
  const textual = blobs.filter(f => wantsText(f.path, f.size));
  const worker = async () => {
    for (;;) {
      const f = textual.shift();
      if (!f) return;
      if (total + f.size > MAX_TOTAL) {
        files.push({ path: f.path, bytes: f.size, sha: f.sha });
        continue;
      }
      total += f.size;
      const kept = f.sha ? known.get(f.sha) : undefined;
      if (kept !== undefined) {
        files.push({ path: f.path, text: kept, bytes: f.size, sha: f.sha });
        continue;
      }
      let text: string | undefined;
      for (const raw of raws) {
        text = await fetchText(`${raw}/${f.full.split('/').map(encodeURIComponent).join('/')}`, signal);
        if (text !== undefined) break;
      }
      files.push(text === undefined ? { path: f.path, bytes: f.size } : { path: f.path, text, bytes: f.size, sha: f.sha });
      if (text !== undefined && f.sha) known.set(f.sha, text);
      tick();
    }
  };
  // A worker that fails (or is aborted) leaves the others nothing more to fetch.
  await Promise.all(Array.from({ length: 12 }, () => worker().catch(e => { textual.length = 0; throw e; })));
  for (const f of blobs) if (!wantsText(f.path, f.size)) files.push({ path: f.path, bytes: f.size, sha: f.sha });
  return files.sort((a, b) => a.path.localeCompare(b.path));
}

/** Reports downloads of `missing` files as they finish. */
function counter(missing: number, progress: Progress): () => void {
  let done = 0;
  if (missing) progress(`Downloading ${missing} files…`, 0);
  return () => {
    done++;
    if (done % 10 === 0) progress(`Downloading ${done}/${missing} files…`, done / missing);
  };
}

async function download(ref: GitHubRef, key: string, known: Map<string, string>, progress: Progress, now: () => number, signal?: AbortSignal): Promise<SavedRepo> {
  const repo = `${ref.owner}/${ref.repo}`;
  const api = `https://api.github.com/repos/${repo}`;
  progress('Contacting GitHub…');
  const info = await gh<{ default_branch: string; name: string; description?: string | null }>(api, `public repository ${repo}`, signal);
  const branch = ref.branch ?? info.default_branch;
  const prefix = ref.path ? `${ref.path.replace(/\/$/, '')}/` : '';
  const blobs = await listTree(api, branch, prefix, `branch ${branch} in ${repo}`, signal);
  const files = await fetchFiles([`https://raw.githubusercontent.com/${repo}/${refPath(branch)}`], blobs, known, counter(unknown(blobs, known).length, progress), signal);
  const name = ref.path ? ref.path.split('/').pop()! : info.name;
  return { key, name, files, source: `github.com/${repo}${ref.path ? `/${ref.path}` : ''}`, description: ref.path ? undefined : info.description ?? undefined, saved: now() };
}

interface PullJson {
  number: number;
  title: string;
  html_url: string;
  state: string;
  merged_at?: string | null;
  base: { ref: string; sha: string; repo?: { name: string; description?: string | null } | null };
  head: { ref: string; label?: string; sha: string; repo?: { full_name: string } | null };
}

interface CompareJson {
  merge_base_commit: { sha: string };
  total_commits: number;
  commits: { sha: string }[];
  files?: { filename: string; status: string; previous_filename?: string | null }[];
}

async function downloadComparison(ref: GitHubRef, key: string, known: Map<string, string>, progress: Progress, now: () => number, signal?: AbortSignal): Promise<SavedRepo> {
  const repo = `${ref.owner}/${ref.repo}`;
  const api = `https://api.github.com/repos/${repo}`;
  progress('Contacting GitHub…');
  let info: CompareInfo;
  let name = ref.repo;
  let description: string | undefined;
  let baseRev: string;
  let headRev: string;
  // A pull request from a fork has its commits in the base repository too; its own is a fallback.
  const raws = (sha: string, fork?: string | null) => [`https://raw.githubusercontent.com/${repo}/${sha}`, ...(fork && fork !== repo ? [`https://raw.githubusercontent.com/${fork}/${sha}`] : [])];
  let fork: string | null | undefined;
  if (ref.pull !== undefined) {
    const pr = await gh<PullJson>(`${api}/pulls/${ref.pull}`, `pull request #${ref.pull} in ${repo}`, signal);
    baseRev = pr.base.sha;
    headRev = pr.head.sha;
    fork = pr.head.repo?.full_name;
    name = pr.base.repo?.name ?? ref.repo;
    description = pr.base.repo?.description ?? undefined;
    info = { kind: 'pr', repo, number: pr.number, title: pr.title, url: pr.html_url, state: pr.merged_at ? 'merged' : pr.state, baseRef: pr.base.ref, headRef: pr.head.label ?? pr.head.ref, baseSha: '', headSha: pr.head.sha, files: [] };
  } else {
    let base = ref.compare!.base;
    if (!base) {
      const meta = await gh<{ default_branch: string; description?: string | null }>(api, `public repository ${repo}`, signal);
      base = meta.default_branch;
      description = meta.description ?? undefined;
    }
    baseRev = base;
    headRev = ref.compare!.head;
    info = { kind: 'compare', repo, title: `${base}...${headRev}`, url: `https://github.com/${repo}/compare/${base}...${headRev}`, baseRef: base, headRef: headRev, baseSha: '', headSha: '', files: [] };
  }
  // Compared from where the head branched off, as GitHub's "Files changed" is.
  const cmp = await gh<CompareJson>(`${api}/compare/${refPath(baseRev)}...${refPath(headRev)}`, `comparison ${baseRev}...${headRev} in ${repo}`, signal);
  info.baseSha = cmp.merge_base_commit.sha;
  if (!info.headSha) {
    info.headSha = cmp.total_commits === 0 ? info.baseSha
      : cmp.commits.length >= cmp.total_commits ? cmp.commits[cmp.commits.length - 1].sha
      : (await gh<{ sha: string }>(`${api}/commits/${refPath(headRev)}`, `commit ${headRev} in ${repo}`, signal)).sha;
  }
  const [baseBlobs, headBlobs] = await Promise.all([
    listTree(api, info.baseSha, '', `commit ${info.baseSha.slice(0, 7)} in ${repo}`, signal),
    listTree(api, info.headSha, '', `commit ${info.headSha.slice(0, 7)} in ${repo}`, signal),
  ]);
  const headShas = new Set(headBlobs.map(b => b.sha));
  const missing = unknown(headBlobs, known).length + unknown(baseBlobs, known).filter(b => !headShas.has(b.sha)).length;
  const tick = counter(missing, progress);
  const head = await fetchFiles(raws(info.headSha, fork), headBlobs, known, tick, signal);
  const base = await fetchFiles(raws(info.baseSha), baseBlobs, known, tick, signal);
  info.files = fileChanges(baseBlobs, headBlobs, cmp.files ?? []);
  const source = ref.pull !== undefined ? `github.com/${repo}/pull/${ref.pull}` : `github.com/${repo}/compare/${info.title}`;
  return { key, name, description, source, files: head, base, compare: info, saved: now() };
}

/** How each file differs between the two versions, by content hash; GitHub's
 * own list (which stops at 300 files) says which were renamed. */
export function fileChanges(base: { path: string; sha?: string }[], head: { path: string; sha?: string }[], listed: { filename: string; status: string; previous_filename?: string | null }[]): FileChange[] {
  const before = new Map(base.map(b => [b.path, b.sha]));
  const after = new Map(head.map(h => [h.path, h.sha]));
  const out = new Map<string, FileChange>();
  for (const [path, sha] of after) {
    if (!before.has(path)) out.set(path, { path, status: 'added' });
    else if (before.get(path) !== sha) out.set(path, { path, status: 'modified' });
  }
  for (const path of before.keys()) if (!after.has(path)) out.set(path, { path, status: 'removed' });
  for (const f of listed) {
    if (f.status !== 'renamed' || !f.previous_filename || !after.has(f.filename) || !before.has(f.previous_filename) || after.has(f.previous_filename)) continue;
    out.set(f.filename, { path: f.filename, status: 'renamed', previous: f.previous_filename });
    out.delete(f.previous_filename);
  }
  return [...out.values()].sort((a, b) => a.path.localeCompare(b.path));
}

export function pastedProject(filename: string, text: string): Project {
  const name = filename.trim() || guessFilename(text);
  return { name: name.replace(/\.[^./]+$/, ''), files: [{ path: name, text, bytes: text.length }], source: 'pasted code' };
}

/** A file name for pasted code, by what the code looks like: Go or Rust by
 *  their keywords, else Python unless it uses JavaScript's syntax, TypeScript if it has type annotations, JSX if it
 *  returns elements. */
export function guessFilename(text: string): string {
  // Go: a package clause, `func` declarations, an import block, `type T struct {`.
  const go = /^\s*package\s+\w+\s*$|^\s*func\s+(\([^)]*\)\s*)?\w+(\[[^\]]*\])?\s*\(|^\s*import\s*\(\s*$|^\s*type\s+\w+(\[[^\]]*\])?\s+(struct|interface)\s*\{/m.test(text);
  if (go) {
    const pkg = /^\s*package\s+(\w+)/m.exec(text)?.[1];
    return !pkg || pkg === 'main' || /^\s*func\s+main\s*\(/m.test(text) ? 'main.go' : `${pkg}.go`;
  }
  // Rust: items with Rust's keywords (`fn`, `impl`, `use x::y`, `pub struct`, `let mut`).
  const rust = /^\s*(pub(\([\w:]+\))?\s+)?(async\s+)?(fn\s+\w+\s*[<(]|impl(<[^>]*>)?\s+\w|mod\s+\w+\s*[;{]|use\s+[\w:{}*, ]+::|(struct|enum|trait)\s+\w+)|\blet\s+mut\s|#\[derive\(|^\s*#!\[/m.test(text);
  if (rust) return /^\s*(pub\s+)?(async\s+)?fn\s+main\s*\(/m.test(text) ? 'main.rs' : 'lib.rs';
  const python = /^\s*(def \w+\(|class \w+(\(.*\))?:\s*$|from [\w.]+ import |import [\w.]+(\s+as \w+)?\s*$|if __name__ ==)/m.test(text);
  const js = /^\s*(import\s[^\n]*from\s*['"]|import\s+['"]|export\s|const\s|let\s|var\s|function\*?\s|module\.exports|(async\s+)?function\s)|=>|\brequire\(['"]/m.test(text);
  if (!js || (python && !/[;{}]\s*$/m.test(text))) return 'main.py';
  const ts = /^\s*(export\s+)?(interface|type|enum|namespace)\s+\w+|\)\s*:\s*[\w<[{]|\b(let|const)\s+\w+\s*:\s*\w|\bas const\b|\bimplements\s/m.test(text);
  const jsx = /return\s*\(?\s*<[A-Za-z>]|=>\s*\(?\s*<[A-Za-z>]|<\/[A-Za-z][\w.]*>|<[A-Z][\w.]*[^>]*\/>/.test(text);
  return `main.${ts ? 'ts' : 'js'}${jsx ? 'x' : ''}`;
}
