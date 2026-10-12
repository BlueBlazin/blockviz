import { afterEach, describe, expect, it, vi } from 'vitest';
import { memoryCache } from './cache';
import { githubProject, guessFilename, ignoredPath, pastedProject } from './loaders';

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

describe('GitHub repositories', () => {
  /** A fake GitHub serving `files` (path → text); `sha` names each file's content. */
  function github(files: Record<string, string>) {
    const calls: string[] = [];
    let status = 200;
    const sha = (text: string) => `sha-${text}`;
    const fetch = vi.fn(async (url: string) => {
      calls.push(url);
      if (status !== 200) return new Response('', { status });
      if (url === 'https://api.github.com/repos/o/r') return Response.json({ default_branch: 'main', name: 'r', description: 'A repo' });
      if (url.startsWith('https://api.github.com/repos/o/r/git/trees/main')) {
        return Response.json({ truncated: false, tree: Object.entries(files).map(([path, text]) => ({ path, type: 'blob', size: text.length, sha: sha(text) })) });
      }
      const path = url.replace('https://raw.githubusercontent.com/o/r/main/', '');
      return path in files ? new Response(files[path]) : new Response('', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);
    return { calls, files, fail: (s: number) => { status = s; } };
  }
  const texts = (p: { files: { path: string; text?: string }[] }) => Object.fromEntries(p.files.map(f => [f.path, f.text]));
  const day = 24 * 60 * 60 * 1000;
  afterEach(() => vi.unstubAllGlobals());

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
});
