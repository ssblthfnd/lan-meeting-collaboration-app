/**
 * Structural properties of the package itself.
 *
 * These read the sources rather than calling them, because the subject is the
 * shape of the package: "no framework", "no HTML sink", "no absolute URL" are
 * facts about files, not about values. The same properties are asserted again
 * from Rust in `src-tauri/tests/boundaries.rs`, which is the copy that runs in
 * the Rust validation flow; this copy fails faster, in the suite a change to
 * this package would be run against first.
 */

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { existsSync } from 'node:fs';

import { describe, expect, it } from 'vitest';

/**
 * The file with whole-line comments removed.
 *
 * The same conservative rule `src-tauri/tests/boundaries.rs` uses, and for the
 * same reason: documentation that *states* a rule - "nothing here assigns
 * innerHTML" - must not be mistaken for breaking it. Only a line whose first
 * non-whitespace is a comment marker is dropped, so a trailing comment never
 * takes code with it.
 */
function codeOnly(contents: string): string {
  const kept: string[] = [];
  let inBlock = false;

  for (const line of contents.split('\n')) {
    const trimmed = line.trimStart();

    if (inBlock) {
      if (trimmed.includes('*/')) {
        inBlock = false;
      }
      continue;
    }
    if (trimmed.startsWith('/*')) {
      inBlock = !trimmed.includes('*/');
      continue;
    }
    if (trimmed.startsWith('//') || trimmed.startsWith('*')) {
      continue;
    }
    kept.push(line);
  }

  return kept.join('\n');
}

/** Every `.ts` file under `packages/editor/src`, comments stripped. */
function sources(): { path: string; text: string }[] {
  const root = packageRoot();
  const found: { path: string; text: string }[] = [];

  const walk = (directory: string): void => {
    for (const entry of readdirSync(directory)) {
      const full = join(directory, entry);
      if (statSync(full).isDirectory()) {
        walk(full);
      } else if (full.endsWith('.ts')) {
        found.push({ path: full, text: codeOnly(readFileSync(full, 'utf8')) });
      }
    }
  };

  walk(join(root, 'src'));
  expect(found.length).toBeGreaterThan(0);
  return found;
}

function packageRoot(): string {
  let directory = process.cwd();
  for (;;) {
    const candidate = resolve(directory, 'packages/editor');
    if (existsSync(join(candidate, 'package.json'))) {
      return candidate;
    }
    const parent = dirname(directory);
    if (parent === directory) {
      throw new Error('could not locate packages/editor');
    }
    directory = parent;
  }
}

describe('packages/editor stays bundleable into the offline remote form', () => {
  it('contains no absolute URL literal', () => {
    // `scripts/check-remote-form-offline.mjs` reads the built bytes of the
    // remote form and refuses any absolute URL in them. This package is
    // bundled into that file, so one literal here becomes a failed build two
    // steps from now - and the guard must not be relaxed to accommodate it
    // (ADR-0009). Caught here instead.
    for (const file of sources()) {
      expect(file.text, file.path).not.toMatch(/https?:\/\//i);
    }
  });

  it('makes no network call', () => {
    for (const file of sources()) {
      for (const forbidden of [
        'fetch(',
        'XMLHttpRequest',
        'WebSocket',
        'EventSource',
        'sendBeacon',
        'import(',
      ]) {
        expect(file.text, `${file.path} mentions ${forbidden}`).not.toContain(forbidden);
      }
    }
  });

  it('imports no UI framework', () => {
    // ADR-0009: the remote form has no framework, and whatever this package
    // exports has to be usable without one.
    for (const file of sources()) {
      for (const forbidden of ['react', 'react-dom', 'preact', 'vue', 'svelte']) {
        expect(file.text, `${file.path} imports ${forbidden}`).not.toMatch(
          new RegExp(`from\\s+['"]${forbidden}`),
        );
      }
    }
  });

  it('declares no runtime dependency', () => {
    const manifest = JSON.parse(
      readFileSync(join(packageRoot(), 'package.json'), 'utf8'),
    ) as { dependencies?: Record<string, string> };
    expect(manifest.dependencies ?? {}).toEqual({});
  });
});

describe('packages/editor has no HTML sink', () => {
  it('assigns no HTML anywhere', () => {
    // The renderer's safety is structural: text reaches the DOM through
    // `textContent` and nodes are built with `createElement`. A sink would
    // reintroduce the sanitiser problem the design exists to avoid.
    for (const file of sources()) {
      for (const forbidden of [
        'innerHTML',
        'outerHTML',
        'dangerouslySetInnerHTML',
        'insertAdjacentHTML',
        'document.write',
        'createContextualFragment',
        'eval(',
        'Function(',
      ]) {
        expect(file.text, `${file.path} uses ${forbidden}`).not.toContain(forbidden);
      }
    }
  });

  it('creates only allowlisted elements', () => {
    // Every `createElement` call in the package, with its literal argument.
    const created = new Set<string>();
    for (const file of sources()) {
      for (const match of file.text.matchAll(/createElement\(\s*'([a-z0-9]+)'/g)) {
        created.add(match[1] ?? '');
      }
    }

    const allowed = new Set([
      'p',
      'ul',
      'ol',
      'li',
      'table',
      'thead',
      'tbody',
      'tr',
      'th',
      'td',
      'pre',
      'code',
      'em',
      'strong',
      'a',
      'br',
    ]);

    for (const tag of created) {
      expect(allowed, `createElement('${tag}')`).toContain(tag);
    }
  });
});
