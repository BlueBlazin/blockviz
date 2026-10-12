/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
// The page is built from src/ (the core as WebAssembly in src/wasm): the Rust
// sources and Cargo's tens of thousands of build files are not watched. The
// Rust crate's fixtures also include other projects' test files.
export default defineConfig({
  base: './',
  worker: { format: 'es' },
  server: { port: 5173, watch: { ignored: ['**/crates/**', '**/target/**'] } },
  test: { include: ['src/**/*.test.ts'] },
});
