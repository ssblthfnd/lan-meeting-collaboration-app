import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// LAN participant UI.
//
// This bundle is served to participant browsers by the Rust HTTP server, which
// embeds `dist/` at compile time. It has no Tauri API access: everything it
// needs comes over HTTP/WebSocket, and the backend decides what it may see.
//
// The dev server binds 0.0.0.0 so the page can be opened from a phone on the
// same network while developing. Production serving is done by Rust, not Vite.
export default defineConfig({
  plugins: [react()],
  base: '/',
  server: {
    host: '0.0.0.0',
    port: 1421,
    strictPort: true,
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    sourcemap: true,
  },
});
