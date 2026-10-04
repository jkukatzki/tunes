#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';
const [output, profile = 'release'] = process.argv.slice(2);
if (!output || output === '--help') {
  console.log('tunes-build-audio <output-directory> [release|dev]');
  process.exit(output ? 0 : 1);
}
const result = spawnSync('bash', [fileURLToPath(new URL('../web-dsp/scripts/package.sh', import.meta.url)), profile, resolve(output)], { stdio: 'inherit' });
if (result.error) throw result.error;
process.exit(result.status ?? 1);
