#!/usr/bin/env node
/**
 * Build guard for the remote participant form.
 *
 * Architecture rules 6 and 20 require the generated form to be a single,
 * fully self-contained HTML file that opens offline from a file:// URL. That
 * property is easy to break by accident (a font import, a sourcemap comment, a
 * stray CDN script), and the breakage only shows up on a participant machine
 * with no internet - which is exactly where it must not.
 *
 * This script fails the build if the built file is not standalone.
 *
 * No dependencies: plain Node, so it runs anywhere the repo builds.
 */

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const distDir = resolve(here, '..', 'apps', 'remote-form', 'dist');

/** Maximum acceptable size of the generated form, in bytes. */
const MAX_BYTES = 1_500_000;

const FORBIDDEN = [
  { name: 'external script', re: /<script[^>]+src\s*=\s*["']?(https?:)?\/\//i },
  { name: 'external stylesheet', re: /<link[^>]+href\s*=\s*["']?(https?:)?\/\//i },
  { name: 'CSS @import of a remote resource', re: /@import\s+(url\()?["']?(https?:)?\/\//i },
  { name: 'absolute http(s) URL', re: /https?:\/\/(?!www\.w3\.org\/)/i },
  { name: 'fetch() call', re: /\bfetch\s*\(/ },
  { name: 'XMLHttpRequest', re: /\bXMLHttpRequest\b/ },
  { name: 'WebSocket', re: /\bnew\s+WebSocket\b/ },
  { name: 'navigator.sendBeacon', re: /\bsendBeacon\b/ },
  { name: 'EventSource', re: /\bnew\s+EventSource\b/ },
];

function listFiles(dir) {
  return readdirSync(dir).flatMap((entry) => {
    const full = join(dir, entry);
    return statSync(full).isDirectory() ? listFiles(full) : [full];
  });
}

let files;
try {
  files = listFiles(distDir);
} catch {
  console.error(`[remote-form] build output not found: ${distDir}`);
  console.error('[remote-form] run the build before this check.');
  process.exit(1);
}

const problems = [];

const htmlFiles = files.filter((f) => f.endsWith('.html'));
const sidecars = files.filter((f) => !f.endsWith('.html'));

if (htmlFiles.length !== 1) {
  problems.push(`expected exactly 1 HTML file in dist, found ${htmlFiles.length}`);
}

if (sidecars.length > 0) {
  problems.push(
    `dist must contain only the single HTML file, found extra asset(s): ${sidecars
      .map((f) => f.slice(distDir.length + 1))
      .join(', ')}`,
  );
}

for (const file of htmlFiles) {
  const html = readFileSync(file, 'utf8');
  const bytes = Buffer.byteLength(html);

  if (bytes > MAX_BYTES) {
    problems.push(`${file} is ${bytes} bytes, over the ${MAX_BYTES} byte budget`);
  }

  for (const { name, re } of FORBIDDEN) {
    if (re.test(html)) {
      problems.push(`${file} contains a forbidden pattern: ${name}`);
    }
  }
}

if (problems.length > 0) {
  console.error('[remote-form] offline self-containment check FAILED:');
  for (const p of problems) {
    console.error(`  - ${p}`);
  }
  console.error('\nThe remote form must open offline from file:// with no external resource.');
  process.exit(1);
}

console.log('[remote-form] offline self-containment check passed.');
