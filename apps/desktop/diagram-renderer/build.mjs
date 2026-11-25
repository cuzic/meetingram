import { readFile } from 'node:fs/promises';
import { build } from 'esbuild';

// TEMPORARY: until svg-diagram is released with ADR 0081 (mindmap edge and tags),
// SVG_DIAGRAM_SRC points at a checkout of svg-diagram and its browser entry is
// bundled from source. After the release, drop this and use the npm package again.
const source = process.env.SVG_DIAGRAM_SRC;
const alias = source ? { 'svg-diagram/browser': `${source}/lib/web.ts` } : undefined;

const license = await readFile('LICENSE.svg-diagram', 'utf8');
await build({
  entryPoints: ['renderer.ts'],
  bundle: true,
  platform: 'browser',
  format: 'iife',
  minify: true,
  legalComments: 'eof',
  banner: { js: `/*! svg-diagram\n${license}*/` },
  outfile: 'renderer.js',
  ...(alias ? { alias } : {}),
});
