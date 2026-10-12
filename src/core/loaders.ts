import { browserCache, type RepoCache, type SavedFile, type SavedRepo } from './cache';
import type { InputFile, Project } from './types';

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

export function parseGitHub(input: string): { owner: string; repo: string; branch?: string; path?: string } | null {
  const s = input.trim().replace(/^https?:\/\//, '').replace(/^www\./, '').replace(/\.git$/, '').replace(/\/$/, '');
  const m = s.match(/^(?:github\.com\/)?([\w.-]+)\/([\w.-]+)(?:\/tree\/([^/]+)(?:\/(.+))?)?$/);
  return m ? { owner: m[1], repo: m[2], branch: m[3], path: m[4] } : null;
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
}

/**
 * A GitHub repository's files. A reload, a link or Back opens the copy saved
 * in this browser without downloading anything while it is recent; otherwise
 * GitHub lists the files and only those that changed are downloaded. When
 * GitHub cannot be reached (or its rate limit is spent), the saved copy opens.
 */
export async function githubProject(input: string, progress: Progress, { cache = (shared ??= browserCache()), latest = false, now = Date.now }: GitHubOptions = {}): Promise<Project> {
  const ref = parseGitHub(input);
  if (!ref) throw new Error('Enter a GitHub repository like github.com/karpathy/nanoGPT.');
  const key = repoKey(ref);
  const saved = await cache.get(key).catch(() => undefined);
  if (saved && !latest && now() - saved.saved < FRESH_FOR) return fromSaved(saved);
  try {
    const repo = await download(ref, key, saved, progress, now);
    await cache.put(repo).catch(() => undefined);
    return fromSaved(repo);
  } catch (e) {
    if (saved && !(e instanceof Missing)) return fromSaved(saved);
    throw e;
  }
}

/** GitHub says there is no such repository or branch: a saved copy is no answer. */
class Missing extends Error {}

/** Names a repository (and branch and folder) however its URL was written. */
function repoKey(ref: NonNullable<ReturnType<typeof parseGitHub>>): string {
  return `github.com/${ref.owner}/${ref.repo}`.toLowerCase() + (ref.branch ? `/tree/${ref.branch}` : '') + (ref.path ? `/${ref.path.replace(/\/$/, '')}` : '');
}

function fromSaved(r: SavedRepo): Project {
  return { name: r.name, files: r.files.map(({ path, text, bytes }) => (text === undefined ? { path, bytes } : { path, text, bytes })), source: r.source, description: r.description };
}

async function download(ref: NonNullable<ReturnType<typeof parseGitHub>>, key: string, saved: SavedRepo | undefined, progress: Progress, now: () => number): Promise<SavedRepo> {
  const api = `https://api.github.com/repos/${ref.owner}/${ref.repo}`;
  progress('Contacting GitHub…');
  const meta = await fetch(api);
  if (meta.status === 404) throw new Missing(`GitHub has no public repository ${ref.owner}/${ref.repo}.`);
  if (meta.status === 403) throw new Error('GitHub rate limit reached. Try again later, or open a local folder.');
  if (!meta.ok) throw new Error(`GitHub returned ${meta.status}.`);
  const info = await meta.json() as { default_branch: string; name: string; description?: string | null };
  const branch = ref.branch ?? info.default_branch;
  const treeRes = await fetch(`${api}/git/trees/${encodeURIComponent(branch)}?recursive=1`);
  if (treeRes.status === 404) throw new Missing(`Could not list ${ref.owner}/${ref.repo}@${branch} (404).`);
  if (!treeRes.ok) throw new Error(`Could not list ${ref.owner}/${ref.repo}@${branch} (${treeRes.status}).`);
  const tree = await treeRes.json() as { tree: { path: string; type: string; size?: number; sha?: string }[]; truncated: boolean };
  const prefix = ref.path ? `${ref.path.replace(/\/$/, '')}/` : '';
  const blobs = tree.tree
    .filter(t => t.type === 'blob' && t.path.startsWith(prefix) && !ignoredPath(t.path))
    .slice(0, MAX_FILES);
  // Files whose content GitHub names by the same hash as before are not downloaded again.
  const before = new Map((saved?.files ?? []).filter(f => f.sha && f.text !== undefined).map(f => [`${f.path}\n${f.sha}`, f.text!]));
  const files: SavedFile[] = [];
  let total = 0;
  const queue = blobs.map(b => ({ path: b.path.slice(prefix.length), full: b.path, size: b.size ?? 0, sha: b.sha }));
  const textual = queue.filter(f => wantsText(f.path, f.size));
  const missing = textual.filter(f => !before.has(`${f.path}\n${f.sha}`)).length;
  let done = 0;
  const worker = async () => {
    for (;;) {
      const f = textual.shift();
      if (!f) return;
      if (total + f.size > MAX_TOTAL) {
        files.push({ path: f.path, bytes: f.size, sha: f.sha });
        continue;
      }
      total += f.size;
      const kept = before.get(`${f.path}\n${f.sha}`);
      if (kept !== undefined) {
        files.push({ path: f.path, text: kept, bytes: f.size, sha: f.sha });
        continue;
      }
      const url = `https://raw.githubusercontent.com/${ref.owner}/${ref.repo}/${encodeURIComponent(branch)}/${f.full.split('/').map(encodeURIComponent).join('/')}`;
      const res = await fetch(url);
      files.push(res.ok ? { path: f.path, text: await res.text(), bytes: f.size, sha: f.sha } : { path: f.path, bytes: f.size });
      done++;
      if (done % 10 === 0) progress(`Downloading ${done}/${missing} files…`, done / missing);
    }
  };
  if (missing) progress(`Downloading ${missing} files…`, 0);
  await Promise.all(Array.from({ length: 12 }, worker));
  for (const f of queue) if (!wantsText(f.path, f.size)) files.push({ path: f.path, bytes: f.size, sha: f.sha });
  files.sort((a, b) => a.path.localeCompare(b.path));
  const name = ref.path ? ref.path.split('/').pop()! : info.name;
  return { key, name, files, source: `github.com/${ref.owner}/${ref.repo}${ref.path ? `/${ref.path}` : ''}`, description: ref.path ? undefined : info.description ?? undefined, saved: now() };
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
