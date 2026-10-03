import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  mkdtempSync,
  writeFileSync,
  readFileSync,
  readdirSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(
  new URL("../scripts/optimize-wasm.mjs", import.meta.url),
);

for (const fails of [false, true]) {
  test(
    fails
      ? "failed optimization preserves original and cleans staging"
      : "O3 keeps larger optimized output",
    () => {
      const dir = mkdtempSync(join(tmpdir(), "wasm-opt-test-"));
      try {
        const file = join(dir, "game.wasm");
        const optimizer = join(dir, "optimizer");
        const original = Buffer.from([0, 97, 115, 109, 1, 0, 0, 0]);
        writeFileSync(file, original);
        writeFileSync(
          optimizer,
          `#!/usr/bin/env node
const fs = require('node:fs');
const args = process.argv.slice(2);
if (!args.includes('-O3')) process.exit(4);
fs.writeFileSync(args[args.indexOf('-o') + 1], Buffer.from([0,97,115,109,1,0,0,0,0,2,1,120]));
process.exit(${fails ? 2 : 0});
`,
          { mode: 0o755 },
        );
        const result = spawnSync(process.execPath, [script, file], {
          env: {
            ...process.env,
            WASM_OPT: optimizer,
            WASM_OPT_LEVEL: "",
            WASM_OPT_CONVERGE: "",
          },
          encoding: "utf8",
        });
        if (fails) {
          assert.notEqual(result.status, 0);
          assert.deepEqual(readFileSync(file), original);
        } else {
          assert.equal(result.status, 0, result.stderr);
          assert.equal(readFileSync(file).length, 12);
          assert.ok(WebAssembly.validate(readFileSync(file)));
        }
        assert.deepEqual(readdirSync(dir).sort(), ["game.wasm", "optimizer"]);
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    },
  );
}
