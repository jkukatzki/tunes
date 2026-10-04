import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, writeFileSync, mkdirSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

test('failed app compilation preserves published assets and removes staging', () => {
  const root = mkdtempSync(join(tmpdir(), 'tunes-build-'));
  try {
    const bin = join(root,'bin'), output = join(root,'static','app');
    mkdirSync(bin); mkdirSync(output,{recursive:true});
    writeFileSync(join(output,'app.wasm'),'previous release');
    for (const name of ['cargo','wasm-bindgen','rsync']) {
      writeFileSync(join(bin,name), `#!/usr/bin/env node
if (process.argv.includes('--version')) process.exit(0);
if (process.argv.includes('metadata')) { console.log(JSON.stringify({target_directory:${JSON.stringify(join(root,'target'))}})); process.exit(0); }
process.exit(23);
`,{mode:0o755});
    }
    const result = spawnSync(process.execPath,[fileURLToPath(new URL('./build-wasm.mjs',import.meta.url)), join(root,'Cargo.toml'),'app','app',output,join(root,'assets'),'dev'],{env:{...process.env,PATH:`${bin}:${process.env.PATH}`},encoding:'utf8'});
    assert.notEqual(result.status,0);
    assert.match(result.stderr,/cargo failed/);
    assert.equal(readFileSync(join(output,'app.wasm'),'utf8'),'previous release');
    assert.deepEqual(readdirSync(join(root,'static')),['app']);
  } finally { rmSync(root,{recursive:true,force:true}); }
});
