import { readFile } from 'node:fs/promises';
import { build } from 'esbuild';

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
});
