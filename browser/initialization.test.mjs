import { test } from 'node:test';
import assert from 'node:assert/strict';

import { createWasmInitializer } from './index.js';
const context = { createGameInitializer: createWasmInitializer };

test('concurrent and subsequent starts initialize the game only once', async () => {
  const start = context.createGameInitializer();
  let calls = 0;
  let finish;
  const initialize = () => { calls++; return new Promise(resolve => { finish = resolve; }); };
  const first = start(initialize);
  const second = start(initialize);
  assert.equal(first, second);
  await Promise.resolve();
  assert.equal(calls, 1);
  finish();
  await first;
  await start(initialize);
  assert.equal(calls, 1);
});

for (const asynchronous of [false, true]) {
  test(`failed initialization requires reload and cannot falsely succeed (async=${asynchronous})`, async () => {
    const start = context.createGameInitializer();
    const panic = new Error('unreachable');
    let calls = 0;
    const initialize = () => {
      calls++;
      // Mimic wasm-bindgen returning a retained instance on a second call.
      if (calls > 1) return {};
      if (asynchronous) return Promise.reject(panic);
      throw panic;
    };
    for (let attempt = 0; attempt < 2; attempt++) {
      await assert.rejects(start(initialize), error => {
        assert.equal(error.name, 'WasmInitializationError');
        assert.equal(error.cause, panic);
        assert.match(error.message, /Reload the page/);
        return true;
      });
    }
    assert.equal(calls, 1);
  });
}
