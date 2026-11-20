import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  build: { assetsInlineLimit: 0 },
  server: { host: '127.0.0.1', port: 17322, strictPort: true }
});
