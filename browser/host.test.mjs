import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createAudioHost, downloadWasm, readLatencyHint } from './index.js';
function environment(t) {
  const events = new Map(), calls = [], session = { type: 'auto' };
  const target = prefix => ({
    addEventListener(n, fn) { events.set(prefix+n, fn); },
    removeEventListener(n) { events.delete(prefix+n); }
  });
  const replacements = {
    window: { ...target('window:'), localStorage: { getItem: () => null } },
    document: { ...target('document:'), visibilityState: 'visible' },
    navigator: { audioSession: session },
    AudioContext: class {
      state = 'suspended';
      constructor(options) { calls.push(['create', options.latencyHint]); }
      resume() { calls.push(['resume']); this.state = 'running'; return Promise.resolve(); }
      close() { calls.push(['close']); this.state = 'closed'; return Promise.resolve(); }
    },
  };
  for (const [name, value] of Object.entries(replacements)) {
    const previous = Object.getOwnPropertyDescriptor(globalThis, name);
    Object.defineProperty(globalThis, name, { configurable: true, value });
    t.after(() => previous ? Object.defineProperty(globalThis, name, previous) : delete globalThis[name]);
  }
  return { calls, events, session };
}
test('gesture unlock precedes async installation and close releases resources once', async t => {
  const { calls, events, session } = environment(t);
  const host = createAudioHost({ latencyHint: 'balanced' });
  assert.deepEqual(calls, [['create','balanced'],['resume']]);
  assert.equal(session.type, 'playback');
  const url = 'data:text/javascript,export async function installWorkerAudio({context}) { return { close: () => context.close() }; }';
  const first = host.install(url);
  assert.equal(host.install(url), first);
  await first;
  await host.close(); await host.close();
  assert.equal(calls.filter(c => c[0] === 'close').length, 1);
  assert.equal(events.size, 0);
  assert.equal(session.type, 'auto');
});
test('failed bridge import can still release unlocked context and listeners', async t => {
  const { events, session } = environment(t);
  const host = createAudioHost();
  await assert.rejects(host.install('data:text/javascript,throw new Error("failed")'));
  await host.close().catch(() => {});
  assert.equal(host.context.state, 'closed');
  assert.equal(events.size, 0); assert.equal(session.type, 'auto');
});
test('invalid storage schemas do not prevent startup', t => {
  environment(t);
  for (const saved of ['bad', 'null', '{}', '{"hint":"invalid"}']) {
    window.localStorage.getItem = () => saved;
    assert.equal(readLatencyHint({storageKey:'app',path:['hint']}), 'interactive');
  }
});
test('WASM HTTP errors fail before compilation', async t => {
  const fetch = globalThis.fetch;
  t.after(() => globalThis.fetch = fetch);
  globalThis.fetch = async () => new Response('missing', {status:404});
  await assert.rejects(downloadWasm('/missing'), /404/);
  globalThis.fetch = async () => new Response(new Uint8Array([0,97,115,109,1,0,0,0]));
  const updates = [];
  assert.ok(await downloadWasm('/app.wasm', p => updates.push(p)) instanceof WebAssembly.Module);
  assert.equal(updates.at(-1).loaded, 8);
});
