/**
 * karpathy/nanoGPT (MIT), vendored as the Rust test fixture and bundled here as
 * one lazily loaded chunk. Images are listed by name only; they are assets,
 * not analyzed.
 */
const PREFIX = '/crates/blockviz-core/tests/fixtures/nanogpt/';

const sources = import.meta.glob(
  ['/crates/blockviz-core/tests/fixtures/nanogpt/**/*.{py,md,ipynb,txt}', '/crates/blockviz-core/tests/fixtures/nanogpt/**/LICENSE'],
  { query: '?raw', import: 'default', eager: true },
) as Record<string, string>;

export const files = [
  ...Object.entries(sources).map(([path, text]) => ({ path: path.slice(PREFIX.length), text, bytes: text.length })),
  { path: 'assets/gpt2_124M_loss.png', bytes: 113_000 },
  { path: 'assets/nanogpt.jpg', bytes: 121_000 },
  { path: '.gitignore', text: '__pycache__/\n.DS_Store\n.idea\n.ipynb_checkpoints/\n.vscode\n*.bin\n*.pkl\n*.pt\n*.pyc\ninput.txt\nenv/\nvenv/\n', bytes: 120 },
];
