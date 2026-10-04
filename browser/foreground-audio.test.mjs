import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const source = readFileSync(new URL('./index.js', import.meta.url), 'utf8').replaceAll('export ', '');

function setup(session, visibilityState = 'visible') {
  const documentEvents = new Map();
  const windowEvents = new Map();
  const document = { visibilityState, addEventListener(name, handler) {
    assert.equal(documentEvents.has(name), false, 'Listeners installed once');
    documentEvents.set(name, handler);
  } };
  const context = vm.createContext({ document, navigator: { audioSession: session },
    window: { addEventListener(name, handler) {
      assert.equal(windowEvents.has(name), false, 'Listeners installed once');
      windowEvents.set(name, handler);
    } }
  });
  vm.runInContext(source, context);
  return { context, document, documentEvents, windowEvents };
}

test('foreground media priority is released on hide and restored on return', () => {
  const session = { type: 'auto' };
  const { context, document, documentEvents, windowEvents } = setup(session);
  context.foregroundAudioSession();
  assert.equal(session.type, 'playback');
  document.visibilityState = 'hidden';
  documentEvents.get('visibilitychange')();
  assert.equal(session.type, 'auto');
  windowEvents.get('pageshow')();
  assert.equal(session.type, 'auto', 'Restoration while hidden must not reacquire playback');
  document.visibilityState = 'visible';
  documentEvents.get('visibilitychange')();
  assert.equal(session.type, 'playback');
  windowEvents.get('pagehide')();
  assert.equal(session.type, 'auto', 'Release priority even if visibility has not changed yet');
  windowEvents.get('pageshow')();
  assert.equal(session.type, 'playback');
});

test('startup while hidden does not acquire media priority', () => {
  const session = { type: 'auto' };
  setup(session, 'hidden').context.foregroundAudioSession();
  assert.equal(session.type, 'auto');
});

test('unsupported or rejected audio session changes do not prevent startup', () => {
  assert.doesNotThrow(() => setup(undefined).context.foregroundAudioSession());
  const session = { get type() { return 'auto'; }, set type(_value) { throw new Error('unsupported'); } };
  assert.doesNotThrow(() => setup(session).context.foregroundAudioSession());
});
