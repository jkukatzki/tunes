#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { resolve, join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
// Explicit paths keep the package independent of any application's directory layout.
const [manifest, pkg, binary, output, assets, profile = 'release'] = process.argv.slice(2);
if (!assets || manifest === '--help') {
  console.log('tunes-build-wasm <Cargo.toml> <package> <binary> <output> <assets> [release|dev|wasm-release]');
  process.exit(manifest === '--help' ? 0 : 1);
}
if (!['release', 'dev', 'wasm-release'].includes(profile)) throw new Error('Invalid profile');
const env = { ...process.env };
if ((env.WASM_SIMD ?? '1') === '1') env.RUSTFLAGS = `${env.RUSTFLAGS ?? ''} -C target-feature=+simd128`;
const run = (command, args) => {
  const result = spawnSync(command, args, { stdio: 'inherit', env });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} failed (${result.status})`);
};
for (const command of ['cargo', 'wasm-bindgen', 'rsync', ...(profile === 'dev' ? [] : [env.WASM_OPT || 'wasm-opt'])]) run(command, ['--version']);
const metadata = spawnSync('cargo', ['metadata', '--no-deps', '--format-version', '1', '--manifest-path', resolve(manifest)], { encoding: 'utf8', env });
if (metadata.status !== 0) throw new Error(metadata.stderr);
const target = JSON.parse(metadata.stdout).target_directory;
const destination = resolve(output);
mkdirSync(dirname(destination), { recursive: true });
const stage = mkdtempSync(join(dirname(destination), '.wasm-package-'));
try {
  run('cargo', ['build', '--locked', '--manifest-path', resolve(manifest), '-p', pkg, '--bin', binary, '--target', 'wasm32-unknown-unknown', '--profile', profile]);
  run('wasm-bindgen', [join(target, 'wasm32-unknown-unknown', profile === 'dev' ? 'debug' : profile, `${binary}.wasm`), '--target', 'web', '--out-dir', stage, '--out-name', binary, '--no-typescript']);
  if (profile !== 'dev') run(process.execPath, [fileURLToPath(new URL('./optimize-wasm.mjs', import.meta.url)), join(stage, `${binary}_bg.wasm`)]);
  run(process.execPath, [fileURLToPath(new URL('./build-audio.mjs', import.meta.url)), join(stage, 'audio'), profile === 'dev' ? 'dev' : 'release']);
  mkdirSync(destination, { recursive: true });
  run('rsync', ['-a', `${stage}/`, `${destination}/`]);
  run('rsync', ['-a', '--delete', `${stage}/audio/`, `${destination}/audio/`]);
  const assetOutput = join(dirname(destination), 'assets');
  mkdirSync(assetOutput, { recursive: true });
  run('rsync', ['-a', '--exclude=.DS_Store', `${resolve(assets)}/`, `${assetOutput}/`]);
} finally { rmSync(stage, { recursive: true, force: true }); }
