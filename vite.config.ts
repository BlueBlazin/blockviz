/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
// The Rust crate's fixtures include other projects' test files.
export default defineConfig({ base: './', worker: { format: 'es' }, server: { port: 5173 }, test: { include: ['src/**/*.test.ts'] } });
