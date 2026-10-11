#!/usr/bin/env node
// ES2015–ES2025 syntax in one module.
import data from './data.json' with { type: 'json' };
import * as path from 'node:path';
export * as helpers from './helpers.js';

const config = await import('./config.js');
const { a = 1, b: { c } = {}, ...rest } = config;
let total = 1_000_000n ** 2n;
total ||= 1n;
total &&= total;
let maybe = rest?.deep?.value ?? 'fallback';
maybe ??= 'x';

class Counter {
  static #instances = 0;
  #count = 0;
  accessor label = 'counter';
  static {
    Counter.#instances = 0;
  }
  get value() { return this.#count; }
  set value(v) { this.#count = v; }
  #bump() { this.#count++; }
  static has(o) { return #count in o; }
  *[Symbol.iterator]() { yield this.#count; }
  async *stream() { for await (const x of source()) yield x; }
}

outer: for (const x of [1, 2, 3]) {
  for (let i = 0; i < x; i++) {
    if (i === 2) continue outer;
    if (i > 5) break outer;
  }
}

const re = /(?<year>\d{4})/dv;
const tag = String.raw`C:\path\${maybe}`;
const grouped = Object.groupBy([1, 2, 3], (n) => (n % 2 ? 'odd' : 'even'));
{
  using res = getResource();
  await using conn = await connect();
  res.use(conn);
}
label: {
  break label;
}
export default function main() {
  const p = path.join(import.meta.dirname, 'out');
  return [data, p, re, tag, grouped, total];
}
