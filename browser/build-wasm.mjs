#!/usr/bin/env node
import { checkSharedMemoryFile } from './check-shared-memory.mjs';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { resolve, join, dirname } from 'node:path';
import { tmpdir } from 'node:os';
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
const threaded = env.WASM_THREADS === '1';
if (threaded && !env.WASM_THREADS_TOOLCHAIN) throw new Error('WASM_THREADS_TOOLCHAIN must pin the threaded web toolchain');
const gameEnv = threaded ? {
  ...env,
  RUSTUP_TOOLCHAIN: env.WASM_THREADS_TOOLCHAIN,
  RUSTFLAGS: `${env.RUSTFLAGS ?? ''} -C target-feature=+atomics,+bulk-memory -C link-arg=--shared-memory -C link-arg=--max-memory=4294967296 -C link-arg=--import-memory -C link-arg=--export=__wasm_init_tls -C link-arg=--export=__tls_size -C link-arg=--export=__tls_align -C link-arg=--export=__tls_base`,
} : env;
const run = (command, args, environment = env) => {
  const result = spawnSync(command, args, { stdio: 'inherit', env: environment });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} failed (${result.status})`);
};
for (const command of ['cargo', 'wasm-bindgen', 'rsync', ...(profile === 'dev' ? [] : [env.WASM_OPT || 'wasm-opt'])]) run(command, ['--version']);
const metadata = spawnSync('cargo', ['metadata', '--no-deps', '--format-version', '1', '--manifest-path', resolve(manifest)], { encoding: 'utf8', env });
if (metadata.status !== 0) throw new Error(metadata.stderr);
const target = JSON.parse(metadata.stdout).target_directory;
const destination = resolve(output);
mkdirSync(dirname(destination), { recursive: true });
// Keep staging outside the public asset tree so concurrent/interrupted builds
// cannot publish temporary WASM files or include them in the service worker.
const stage = mkdtempSync(join(tmpdir(), 'tunes-wasm-package-'));
try {
  run('cargo', ['build', ...(threaded ? ['-Zbuild-std=std,panic_abort'] : []), '--locked', '--manifest-path', resolve(manifest), '-p', pkg, '--bin', binary, '--target', 'wasm32-unknown-unknown', '--profile', profile], gameEnv);
  const rawWasm = join(target, 'wasm32-unknown-unknown', profile === 'dev' ? 'debug' : profile, `${binary}.wasm`);
  if (threaded) checkSharedMemoryFile(rawWasm);
  run('wasm-bindgen', [join(target, 'wasm32-unknown-unknown', profile === 'dev' ? 'debug' : profile, `${binary}.wasm`), '--target', 'web', '--out-dir', stage, '--out-name', binary, '--no-typescript']);
  if (profile !== 'dev') run(process.execPath, [fileURLToPath(new URL('./optimize-wasm.mjs', import.meta.url)), join(stage, `${binary}_bg.wasm`)]);
  if (threaded) checkSharedMemoryFile(join(stage, `${binary}_bg.wasm`));
  run(process.execPath, [fileURLToPath(new URL('./build-audio.mjs', import.meta.url)), join(stage, 'audio'), profile === 'dev' ? 'dev' : 'release']);
  mkdirSync(destination, { recursive: true });
  run('rsync', ['-a', `${stage}/`, `${destination}/`]);
  run('rsync', ['-a', '--delete', `${stage}/audio/`, `${destination}/audio/`]);
  const assetOutput = join(dirname(destination), 'assets');
  mkdirSync(assetOutput, { recursive: true });
  run('rsync', ['-a', '--exclude=.DS_Store', `${resolve(assets)}/`, `${assetOutput}/`]);
} finally { rmSync(stage, { recursive: true, force: true }); }
