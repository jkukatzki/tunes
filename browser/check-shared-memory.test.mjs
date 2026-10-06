import { test } from 'node:test';
import assert from 'node:assert/strict';
import { assertSharedMemory } from './check-shared-memory.mjs';

const header = [0, 97, 115, 109, 1, 0, 0, 0];
function fixture(shared, imported) {
  const memory = [shared ? 3 : 1, 1, 2];
  const payload = imported
    ? [1, 3, ...Buffer.from('env'), 6, ...Buffer.from('memory'), 2, ...memory]
    : [1, ...memory];
  return Uint8Array.from([...header, imported ? 2 : 5, payload.length, ...payload]);
}

test('accepts a module that really requires shared imported memory', () => {
  const bytes = fixture(true, true);
  const module = new WebAssembly.Module(bytes);
  const instance = new WebAssembly.Instance(module, {
    env: { memory: new WebAssembly.Memory({ initial: 1, maximum: 2, shared: true }) }
  });
  assert.ok(instance);
  assert.deepEqual(assertSharedMemory(bytes), { initial: 1, maximum: 2, shared: true });
});

test('a smaller imported maximum works and preserves sharing between instances', () => {
  const module = new WebAssembly.Module(fixture(true, true));
  const memory = new WebAssembly.Memory({ initial: 1, maximum: 1, shared: true });
  assert.ok(new WebAssembly.Instance(module, { env: { memory } }));
  assert.ok(new WebAssembly.Instance(module, { env: { memory } }));
  assert.ok(memory.buffer instanceof SharedArrayBuffer);
  assert.throws(() => memory.grow(1), RangeError);
});

for (const [shared, imported] of [[false, false], [false, true], [true, false]]) {
  test(`rejects unsuitable memory: shared=${shared}, imported=${imported}`, () => {
    const bytes = fixture(shared, imported);
    assert.ok(WebAssembly.validate(bytes));
    assert.throws(() => assertSharedMemory(bytes), /must import shared WASM memory/);
  });
}

test('rejects missing memory and truncated sections', () => {
  assert.throws(() => assertSharedMemory(Uint8Array.from(header)), /must import shared/);
  assert.throws(() => assertSharedMemory(fixture(true, true).slice(0, -1)), /Truncated/);
});
