import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, renameSync, rmSync, statSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { brotliCompressSync, constants, gzipSync } from 'node:zlib';

if (!process.argv[2]) throw new Error('Expected WASM file path');
const file = resolve(process.argv[2]);
const optimizer = process.env.WASM_OPT || 'wasm-opt';
const level = process.env.WASM_OPT_LEVEL || '-O3';
if (!['-Os', '-Oz', '-O2', '-O3'].includes(level)) {
	throw new Error('WASM_OPT_LEVEL must be -Os, -Oz, -O2, or -O3');
}

const before = statSync(file).size;
const temporary = mkdtempSync(join(dirname(file), '.wasm-opt-'));
const output = join(temporary, 'optimized.wasm');
try {
	const args = [file, level, '--enable-reference-types', '--strip-debug', '--strip-producers', '-o', output];
	if ((process.env.WASM_SIMD || '1') === '1') args.push('--enable-simd');
	if (process.env.WASM_OPT_CONVERGE === '1') args.push('--converge');
	console.log(`Optimizing WASM with ${optimizer} ${level} ...`);
	const result = spawnSync(optimizer, args, { stdio: 'inherit' });
	if (result.error) throw new Error(`Cannot run ${optimizer}; install Binaryen (macOS: brew install binaryen).`, { cause: result.error });
	if (result.status !== 0) throw new Error(`wasm-opt failed (${result.signal ?? result.status}); original WASM preserved.`);
	// Runtime optimizations may increase size (for example through inlining).
	// Keep successful output instead of silently discarding those optimizations.
	renameSync(output, file);
	const bytes = readFileSync(file);
	const mib = (size) => `${(size / 1024 / 1024).toFixed(2)} MiB`;
	console.log(`WASM: ${mib(before)} -> ${mib(bytes.length)} (${((bytes.length / before - 1) * 100).toFixed(1)}% size change)`);
	console.log(`Estimated transfer: gzip-9 ${mib(gzipSync(bytes, { level: 9 }).length)}, Brotli-6 ${mib(brotliCompressSync(bytes, { params: { [constants.BROTLI_PARAM_QUALITY]: 6 } }).length)}`);
	console.log('Compression estimates only: configure HTTP compression on your server/CDN.');
} finally {
	rmSync(temporary, { recursive: true, force: true });
}
