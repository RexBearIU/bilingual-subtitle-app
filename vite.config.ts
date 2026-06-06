import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

// @tauri-apps/cli sets TAURI_DEV_HOST for mobile; harmless on desktop.
const host = process.env.TAURI_DEV_HOST

// https://vite.dev/config/ — tuned for Tauri (see https://v2.tauri.app/start/frontend/vite/)
export default defineConfig({
  plugins: [svelte()],

  // Tauri expects a fixed port and fails if it is not available.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? { protocol: 'ws', host, port: 1421 }
      : undefined,
    watch: {
      // Don't watch the Rust side; cargo handles that.
      ignored: ['**/src-tauri/**'],
    },
  },

  // Only TAURI_ENV_* and VITE_* vars are exposed to the frontend.
  envPrefix: ['VITE_', 'TAURI_ENV_'],

  build: {
    // Match the webview engine; Tauri uses an up-to-date WebView2 on Windows.
    target: 'esnext',
    minify: process.env.TAURI_ENV_DEBUG ? false : 'esbuild',
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
})
