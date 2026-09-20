import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Host UI is loaded by the Tauri WebView.
// The dev server is bound to localhost only: this bundle is never served to the
// LAN. LAN participants get apps/lan-ui, served by the Rust server.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    host: '127.0.0.1',
    port: 1420,
    strictPort: true,
  },
  envPrefix: ['VITE_', 'TAURI_'],
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'chrome110',
    sourcemap: true,
  },
});
