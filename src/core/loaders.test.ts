import { describe, expect, it } from 'vitest';
import { guessFilename, ignoredPath, pastedProject } from './loaders';

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
