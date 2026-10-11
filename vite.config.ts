import { defineConfig } from 'vite';
export default defineConfig({ base: './', worker: { format: 'es' }, server: { port: 5173 } });
