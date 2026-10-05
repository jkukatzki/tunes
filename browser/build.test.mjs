import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, writeFileSync, mkdirSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

for (const threaded of [false, true]) {
test(`failed ${threaded ? 'threaded' : 'ordinary'} app compilation preserves published assets and removes staging`, () => {
  const root = mkdtempSync(join(tmpdir(), 'tunes-build-'));
  try {
    const bin = join(root,'bin'), output = join(root,'static','app'), temporary = join(root, 'tmp');
    mkdirSync(bin); mkdirSync(output,{recursive:true}); mkdirSync(temporary);
    writeFileSync(join(output,'app.wasm'),'previous release');
    for (const name of ['cargo','wasm-bindgen','rsync']) {
      writeFileSync(join(bin,name), `#!/usr/bin/env node
if (process.argv.includes('--version')) process.exit(0);
if (process.argv.includes('metadata')) { console.log(JSON.stringify({target_directory:${JSON.stringify(join(root,'target'))}})); process.exit(0); }
if (process.argv.includes('build')) {
  const fs = require('node:fs');
  fs.writeFileSync(${JSON.stringify(join(root, 'during-build.json'))}, JSON.stringify({
    args: process.argv.slice(2),
    toolchain: process.env.RUSTUP_TOOLCHAIN,
    flags: process.env.RUSTFLAGS ?? '',
    published: fs.readdirSync(${JSON.stringify(join(root, 'static'))}),
    temporary: fs.readdirSync(${JSON.stringify(temporary)})
  }));
}
process.exit(23);
`,{mode:0o755});
    }
    const result = spawnSync(process.execPath,[fileURLToPath(new URL('./build-wasm.mjs',import.meta.url)), join(root,'Cargo.toml'),'app','app',output,join(root,'assets'),'dev'],{env:{...process.env,WASM_THREADS:threaded ? '1' : '0',WASM_THREADS_TOOLCHAIN:'nightly-2026-04-01',TMPDIR:temporary,TMP:temporary,TEMP:temporary,PATH:`${bin}:${process.env.PATH}`},encoding:'utf8'});
    assert.notEqual(result.status,0);
    assert.match(result.stderr,/cargo failed/);
    assert.equal(readFileSync(join(output,'app.wasm'),'utf8'),'previous release');
    assert.deepEqual(readdirSync(join(root,'static')),['app']);
    const duringBuild = JSON.parse(readFileSync(join(root, 'during-build.json'), 'utf8'));
    assert.equal(duringBuild.args.includes('-Zbuild-std=std,panic_abort'), threaded);
    if (threaded) {
      assert.equal(duringBuild.toolchain, 'nightly-2026-04-01');
      assert.match(duringBuild.flags, /\+atomics,\+bulk-memory/);
      for (const flag of ['--shared-memory', '--import-memory', '--max-memory=4294967296', '--export=__wasm_init_tls', '--export=__tls_size', '--export=__tls_align', '--export=__tls_base']) {
        assert.ok(duringBuild.flags.includes(`link-arg=${flag}`), flag);
      }
    }
    assert.deepEqual(duringBuild.published, ['app'], 'staging must stay outside public assets during compilation');
    assert.equal(duringBuild.temporary.length, 1);
    assert.match(duringBuild.temporary[0], /^tunes-wasm-package-/);
    assert.deepEqual(readdirSync(temporary), [], 'failed builds must clean temporary staging');
  } finally { rmSync(root,{recursive:true,force:true}); }
});

}
