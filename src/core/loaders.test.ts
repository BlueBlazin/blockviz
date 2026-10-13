import { afterEach, describe, expect, it, vi } from 'vitest';
import { memoryCache } from './cache';
import { fileChanges, firstFiles, githubComparison, githubProject, guessFilename, ignoredPath, isComparison, isTestPath, parseGitHub, pastedProject, readable } from './loaders';

describe('pasted code', () => {
  it('is Python unless it uses JavaScript syntax', () => {
    expect(guessFilename('import torch\n\nclass GPT(nn.Module):\n    def forward(self, x):\n        return x\n')).toBe('main.py');
    expect(guessFilename('from flask import Flask\napp = Flask(__name__)\n')).toBe('main.py');
    expect(guessFilename('x = 1\n')).toBe('main.py');
    expect(guessFilename("const express = require('express');\nconst app = express();\n")).toBe('main.js');
    expect(guessFilename("import { z } from 'zod';\nexport const User = z.object({});\n")).toBe('main.js');
  });

  it('tells TypeScript and JSX apart', () => {
    expect(guessFilename('export interface User { id: string }\nexport function f(u: User): string { return u.id; }\n')).toBe('main.ts');
    expect(guessFilename("export function Card({ title }) {\n  return <div className=\"card\">{title}</div>;\n}\n")).toBe('main.jsx');
    expect(guessFilename('type Props = { title: string };\nexport const Card = ({ title }: Props) => <h1>{title}</h1>;\n')).toBe('main.tsx');
    expect(guessFilename('use std::collections::HashMap;\n\npub struct Db {\n    map: HashMap<String, u32>,\n}\n')).toBe('lib.rs');
    expect(guessFilename('#[tokio::main]\nasync fn main() {\n    let mut x = 1;\n}\n')).toBe('main.rs');
    expect(guessFilename('package main\n\nfunc main() {\n\tx := 1\n}\n')).toBe('main.go');
    expect(guessFilename('package store\n\ntype DB struct {\n\tconn *sql.DB\n}\n')).toBe('store.go');
    expect(guessFilename('func (s *Server) Run(ctx context.Context) error {\n\treturn nil\n}\n')).toBe('main.go');
    expect(guessFilename('import "./styles.css"\nexport const x = 1;\n')).toBe('main.js');
    expect(guessFilename('impl Display for Frame {\n    fn fmt(&self, f: &mut Formatter) -> fmt::Result { Ok(()) }\n}\n')).toBe('lib.rs');
  });

  it('names the project after the file', () => {
    expect(pastedProject('server.ts', 'const a = 1;').name).toBe('server');
    expect(pastedProject('', 'def f():\n    pass\n').files[0].path).toBe('main.py');
  });
});

describe('ignored paths', () => {
  it('skips dependencies and build output of either ecosystem', () => {
    expect(ignoredPath('node_modules/react/index.js')).toBe(true);
    expect(ignoredPath('apps/web/.next/server/page.js')).toBe(true);
    expect(ignoredPath('.turbo/cache/x.json')).toBe(true);
    expect(ignoredPath('src/coverage/report.py')).toBe(false);
    expect(ignoredPath('src/app.ts')).toBe(false);
  });
});

describe('limits', () => {
  it('spend the files on code first, then tests, then documentation', () => {
    const paths = ['docs/a.md', 'tests/test_a.py', 'src/b.py', 'README.md', 'src/a_test.go', 'src/a.py', 'node_modules/x.js'];
    const { first, more } = firstFiles(paths.map(path => ({ path })), 4);
    expect(first.map(f => f.path)).toEqual(['README.md', 'src/a.py', 'src/b.py', 'src/a_test.go']);
    expect(more).toBe(2);
  });

  it('read code before tests and documentation, as far as the text in all allows', () => {
    const files = [{ path: 'src/a.py', size: 6 }, { path: 'src/logo.png', size: 1 }, { path: 'tests/t.py', size: 3 }, { path: 'docs/d.md', size: 3 }];
    const { read, unread } = readable(files, 9);
    expect([...read].map(f => f.path)).toEqual(['src/a.py', 'tests/t.py']);
    expect(unread).toBe(1);
  });

  it('tell test code as the analyzer does', () => {
    for (const p of ['tests/x.py', 'pkg/test_x.py', 'pkg/x_test.go', 'conftest.py', 'src/app.spec.ts', 'web/__tests__/a.js']) expect(isTestPath(p)).toBe(true);
    for (const p of ['src/testing_utils.py', 'src/attest.py', 'src/contest/x.py']) expect(isTestPath(p)).toBe(false);
  });
});

describe('GitHub repositories', () => {
  /** A fake GitHub serving `files` (path → text); `sha` names each file's content.
   * `drops[path]` downloads of a file fail with a network error. */
  function github(files: Record<string, string>) {
    const calls: string[] = [];
    let status = 200;
    const sha = (text: string) => `sha-${text}`;
    const fake = { calls, files, fail: (s: number) => { status = s; }, onFetch: () => {}, drops: {} as Record<string, number> };
    const fetch = vi.fn(async (url: string, init?: RequestInit) => {
      init?.signal?.throwIfAborted();
      calls.push(url);
      fake.onFetch();
      if (status !== 200) return new Response('', { status });
      if (url === 'https://api.github.com/repos/o/r') return Response.json({ default_branch: 'main', name: 'r', description: 'A repo' });
      if (url.startsWith('https://api.github.com/repos/o/r/git/trees/main')) {
        return Response.json({ truncated: false, tree: Object.entries(files).map(([path, text]) => ({ path, type: 'blob', size: text.length, sha: sha(text) })) });
      }
      const path = url.replace('https://raw.githubusercontent.com/o/r/main/', '');
      if (fake.drops[path]-- > 0) throw new TypeError('Failed to fetch');
      return path in files ? new Response(files[path]) : new Response('', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);
    return fake;
  }
  const texts = (p: { files: { path: string; text?: string }[] }) => Object.fromEntries(p.files.map(f => [f.path, f.text]));
  const day = 24 * 60 * 60 * 1000;
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  it('say what a repository too large to read whole leaves out, and keep saying it', async () => {
    const files: Record<string, string> = { 'docs/guide.md': '# Guide', 'tests/test_a.py': 'pass' };
    for (let i = 0; i < 4003; i++) files[`src/m${String(i).padStart(4, '0')}.py`] = `x = ${i}`;
    github(files);
    const cache = memoryCache();
    const p = await githubProject('github.com/o/r', () => {}, { cache, now: () => 0 });
    // The code is read; the three last modules by path, the test and the guide are left out.
    expect(p.files).toHaveLength(4000);
    expect(p.files.every(f => f.path.startsWith('src/') && f.text !== undefined)).toBe(true);
    expect(p.leftOut).toEqual({ files: 5, texts: 0 });
    expect((await githubProject('github.com/o/r', () => {}, { cache, now: () => 1 })).leftOut).toEqual({ files: 5, texts: 0 });
  });

  it('opens again from the saved copy without downloading', async () => {
    const gh = github({ 'a.py': 'x = 1', 'b.py': 'y = 2' });
    const cache = memoryCache();
    const first = await githubProject('github.com/o/r', () => {}, { cache, now: () => 0 });
    expect(texts(first)).toEqual({ 'a.py': 'x = 1', 'b.py': 'y = 2' });
    expect(gh.calls).toHaveLength(4);
    gh.calls.length = 0;
    // A reload or a link: the same files, nothing asked of GitHub.
    const again = await githubProject('https://github.com/O/R/', () => {}, { cache, now: () => day - 1 });
    expect(again).toEqual(first);
    expect(gh.calls).toEqual([]);
  });

  it('downloads only what changed when asked for the latest', async () => {
    const gh = github({ 'a.py': 'x = 1', 'b.py': 'y = 2' });
    const cache = memoryCache();
    await githubProject('o/r', () => {}, { cache, now: () => 0 });
    gh.files['b.py'] = 'y = 3';
    gh.calls.length = 0;
    const latest = await githubProject('o/r', () => {}, { cache, latest: true, now: () => 1 });
    expect(texts(latest)).toEqual({ 'a.py': 'x = 1', 'b.py': 'y = 3' });
    expect(gh.calls.filter(u => u.includes('raw.githubusercontent'))).toEqual(['https://raw.githubusercontent.com/o/r/main/b.py']);
  });

  it('checks an old copy, and opens it when GitHub cannot answer', async () => {
    const gh = github({ 'a.py': 'x = 1' });
    const cache = memoryCache();
    await githubProject('o/r', () => {}, { cache, now: () => 0 });
    gh.calls.length = 0;
    gh.fail(403);
    const old = await githubProject('o/r', () => {}, { cache, now: () => 2 * day });
    expect(texts(old)).toEqual({ 'a.py': 'x = 1' });
    expect(gh.calls.length).toBeGreaterThan(0);
    // A repository GitHub says is gone is not opened from the saved copy.
    gh.fail(404);
    await expect(githubProject('o/r', () => {}, { cache, now: () => 3 * day })).rejects.toThrow('no public repository');
  });

  it('tries a failed download again, and keeps a file it cannot get without its text', async () => {
    vi.useFakeTimers();
    const gh = github(Object.fromEntries(Array.from({ length: 200 }, (_, i) => [`m${i}.py`, `x = ${i}`])));
    gh.drops = { 'm7.py': 1, 'm9.py': Infinity };
    const cache = memoryCache();
    const loading = githubProject('o/r', () => {}, { cache, now: () => 0 });
    await vi.runAllTimersAsync();
    const files = texts(await loading);
    expect(files['m7.py']).toBe('x = 7');
    expect(Object.keys(files).filter(path => files[path] === undefined)).toEqual(['m9.py']);
    expect(gh.calls.filter(u => u.endsWith('/m9.py'))).toHaveLength(3);
    expect(cache.repos.size).toBe(1);
  });

  it('stops downloading when the load is abandoned, and keeps nothing', async () => {
    const gh = github(Object.fromEntries(Array.from({ length: 100 }, (_, i) => [`m${i}.py`, `x = ${i}`])));
    const cache = memoryCache();
    const stop = new AbortController();
    const raw = () => gh.calls.filter(u => u.includes('raw.githubusercontent'));
    gh.onFetch = () => { if (raw().length === 5) stop.abort(); };
    await expect(githubProject('o/r', () => {}, { cache, now: () => 0, signal: stop.signal })).rejects.toMatchObject({ name: 'AbortError' });
    expect(raw()).toHaveLength(5);
    expect(cache.repos.size).toBe(0);
  });
});

describe('GitHub links', () => {
  it('names repositories, pull requests and comparisons', () => {
    expect(parseGitHub('https://github.com/karpathy/nanoGPT')).toEqual({ owner: 'karpathy', repo: 'nanoGPT', branch: undefined, path: undefined });
    expect(parseGitHub('github.com/o/r/tree/dev/src/app')).toEqual({ owner: 'o', repo: 'r', branch: 'dev', path: 'src/app' });
    for (const url of ['https://github.com/o/r/pull/756', 'github.com/o/r/pull/756/files', 'o/r/pull/756?diff=split', 'o/r#756']) {
      expect(parseGitHub(url)).toEqual({ owner: 'o', repo: 'r', pull: 756 });
    }
    expect(parseGitHub('github.com/o/r/compare/main...feature/x')).toEqual({ owner: 'o', repo: 'r', compare: { base: 'main', head: 'feature/x' } });
    expect(parseGitHub('o/r/compare/v1.0..v2.0')).toEqual({ owner: 'o', repo: 'r', compare: { base: 'v1.0', head: 'v2.0' } });
    expect(parseGitHub('o/r/compare/feature')).toEqual({ owner: 'o', repo: 'r', compare: { base: undefined, head: 'feature' } });
    expect(isComparison(parseGitHub('o/r#1'))).toBe(true);
    expect(isComparison(parseGitHub('o/r'))).toBe(false);
  });

  it('says which files were added, removed, changed and renamed', () => {
    const base = [{ path: 'a.py', sha: '1' }, { path: 'b.py', sha: '2' }, { path: 'old.py', sha: '3' }, { path: 'gone.py', sha: '4' }];
    const head = [{ path: 'a.py', sha: '1' }, { path: 'b.py', sha: '5' }, { path: 'new.py', sha: '3' }, { path: 'c.py', sha: '6' }];
    expect(fileChanges(base, head, [{ filename: 'new.py', status: 'renamed', previous_filename: 'old.py' }])).toEqual([
      { path: 'b.py', status: 'modified' },
      { path: 'c.py', status: 'added' },
      { path: 'gone.py', status: 'removed' },
      { path: 'new.py', status: 'renamed', previous: 'old.py' },
    ]);
  });
});

describe('GitHub pull requests', () => {
  /** A fake GitHub with one pull request from a fork: `base` at the merge base, `head` at the fork's commit. */
  function github(base: Record<string, string>, head: Record<string, string>) {
    const calls: string[] = [];
    const tree = (files: Record<string, string>) => Response.json({ truncated: false, tree: Object.entries(files).map(([path, text]) => ({ path, type: 'blob', size: text.length, sha: `sha-${text}` })) });
    vi.stubGlobal('fetch', vi.fn(async (url: string) => {
      calls.push(url);
      const api = 'https://api.github.com/repos/o/r';
      if (url === `${api}/pulls/7`) return Response.json({ number: 7, title: 'Faster loader', html_url: 'https://github.com/o/r/pull/7', state: 'open', merged_at: null, base: { ref: 'main', sha: 'tip', repo: { name: 'r', description: 'A repo' } }, head: { ref: 'fast', label: 'fork:fast', sha: 'H', repo: { full_name: 'fork/r' } } });
      if (url === `${api}/compare/tip...H`) return Response.json({ merge_base_commit: { sha: 'MB' }, total_commits: 1, commits: [{ sha: 'H' }], files: [] });
      if (url === `${api}/git/trees/MB?recursive=1`) return tree(base);
      if (url === `${api}/git/trees/H?recursive=1`) return tree(head);
      const raw = url.match(/^https:\/\/raw\.githubusercontent\.com\/o\/r\/(MB|H)\/(.+)$/);
      const files = raw?.[1] === 'MB' ? base : raw?.[1] === 'H' ? head : null;
      return files && raw![2] in files ? new Response(files[raw![2]]) : new Response('', { status: 404 });
    }));
    return calls;
  }
  afterEach(() => vi.unstubAllGlobals());
  const texts = (p: { files: { path: string; text?: string }[] }) => Object.fromEntries(p.files.map(f => [f.path, f.text]));

  it('loads both versions from where the head branched off, sharing what they share', async () => {
    const calls = github({ 'a.py': 'x = 1', 'b.py': 'y = 2', 'c.py': 'z = 3' }, { 'a.py': 'x = 1', 'b.py': 'y = 20', 'd.py': 'w = 4' });
    const cache = memoryCache();
    const c = await githubComparison('https://github.com/o/r/pull/7/files', () => {}, { cache, now: () => 0 });
    expect(texts(c.base)).toEqual({ 'a.py': 'x = 1', 'b.py': 'y = 2', 'c.py': 'z = 3' });
    expect(texts(c.head)).toEqual({ 'a.py': 'x = 1', 'b.py': 'y = 20', 'd.py': 'w = 4' });
    expect(c.info).toMatchObject({ kind: 'pr', number: 7, title: 'Faster loader', baseRef: 'main', headRef: 'fork:fast', baseSha: 'MB', headSha: 'H' });
    expect(c.info.files).toEqual([{ path: 'b.py', status: 'modified' }, { path: 'c.py', status: 'removed' }, { path: 'd.py', status: 'added' }]);
    // a.py is the same file in both versions: downloaded once.
    expect(calls.filter(u => u.endsWith('/a.py'))).toHaveLength(1);
    expect(calls.filter(u => u.startsWith('https://api.github.com'))).toHaveLength(4);
    // A reload: nothing asked of GitHub.
    calls.length = 0;
    expect(await githubComparison('o/r#7', () => {}, { cache, now: () => 1 })).toEqual(c);
    expect(calls).toEqual([]);
  });

  it('reuses a saved copy of the repository', async () => {
    const calls = github({ 'a.py': 'x = 1', 'b.py': 'y = 2' }, { 'a.py': 'x = 1', 'b.py': 'y = 3' });
    const cache = memoryCache();
    await cache.put({ key: 'github.com/o/r', name: 'r', source: 'github.com/o/r', saved: 0, files: [{ path: 'a.py', text: 'x = 1', bytes: 5, sha: 'sha-x = 1' }, { path: 'b.py', text: 'y = 2', bytes: 5, sha: 'sha-y = 2' }] });
    await githubComparison('o/r/pull/7', () => {}, { cache, now: () => 0 });
    expect(calls.filter(u => u.includes('raw.githubusercontent'))).toEqual(['https://raw.githubusercontent.com/o/r/H/b.py']);
  });
});
