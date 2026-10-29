import { resolve } from 'node:path';
import { defineConfig } from 'vite';

// The playground is a separate Vite root so the Next dashboard build stays
// untouched. It is emitted into `web/out/playground`, which the daemon already
// serves as part of the dashboard directory.
export default defineConfig({
  root: import.meta.dirname,
  base: '/playground/',
  publicDir: false,
  build: {
    outDir: resolve(import.meta.dirname, '../out/playground'),
    emptyOutDir: true,
    target: 'es2020',
  },
  server: {
    port: 5174,
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:57804',
        changeOrigin: true,
        ws: true,
      },
      '/ws': {
        target: 'http://127.0.0.1:57804',
        changeOrigin: true,
        ws: true,
      },
    },
  },
});
