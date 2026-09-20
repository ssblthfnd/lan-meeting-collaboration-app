import { defineConfig } from 'vite';
import { viteSingleFile } from 'vite-plugin-singlefile';

// Remote participant form template.
//
// Hard requirement (architecture rules 6 and 20): the output is ONE HTML file
// that opens from file:// with the machine fully offline. Every asset is
// inlined - no CDN, no external stylesheet, no font fetch, no network call.
//
// `npm run build` runs scripts/check-remote-form-offline.mjs afterwards, which
// fails the build if any external reference survives.
//
// This bundle is plain TypeScript and DOM APIs, with no UI framework
// (ADR-0009). Framework runtimes ship strings and code paths the offline guard
// cannot vouch for, which is why React was removed from this bundle only;
// host-ui and lan-ui still use it.
//
// The Rust crate app-remote later injects the per-participant payload into this
// template; the template itself contains no meeting data.
export default defineConfig({
  plugins: [viteSingleFile()],
  base: './',
  server: {
    host: '127.0.0.1',
    port: 1422,
    strictPort: true,
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    assetsInlineLimit: 100_000_000,
    cssCodeSplit: false,
    sourcemap: false,
    // Vite's module-preload polyfill calls fetch() to warm modulepreload
    // links. In a single-file build there is nothing to preload, so the call
    // is dead code - but it is still a real fetch() in the artefact, and the
    // offline guard rejects it. There is no module graph to preload here.
    modulePreload: false,
    rollupOptions: {
      output: {
        inlineDynamicImports: true,
      },
    },
  },
});
