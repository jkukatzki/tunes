import { test } from 'node:test';
import assert from 'node:assert/strict';
import { restartableAudio } from './audio-restart.mjs';
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
async function settled(audio) {
  for (let i = 0; i < 600 && audio.preferenceStatus().changing; i++) await pause(5);
  assert.equal(audio.preferenceStatus().changing, false);
}
function fake() {
  let session = 0;
  const calls = [], playing = new Set();
  return {
    calls, playing, closed: false, active: false, sampleRate: 48000,
    attach() { playing.clear(); return ++session; },
    restoreSession(s) { session = s; playing.clear(); calls.push(['reset', s]); },
    wire(s, json) {
      assert.equal(s, session);
      const c = JSON.parse(json); calls.push(['wire', c]);
      if (c.op.kind === 'Play') playing.add(c.id);
      if (c.op.name === 'Stop') playing.delete(c.id);
      return true;
    },
    pcm(s, key, samples) { assert.equal(s, session); calls.push(['pcm', key, [...samples]]); return true; },
    removePcm(s, key) { calls.push(['remove-pcm', key]); return true; },
    enableMonitor(s, value) { calls.push(['monitor', value]); },
    isPlaying(s, id) { return session === s && playing.has(id); },
    monitor() { return new Float32Array(); },
    bufferStatus() { return { outputLatency: this.active ? .02 : .01 }; },
    async capturePlayback(ids) { this.held = true; return JSON.stringify(ids.map(id => [id, { elapsed_time: 12.5, paused: false }])); },
    async restorePlayback(snapshot) { calls.push(['position', JSON.parse(snapshot)]); },
    releasePlayback() { this.held = false; },
    async drain() {},
    activate() { this.active = true; },
    async close() { this.closed = true; },
  };
}
const wire = (id, op) => JSON.stringify({ version: 2, id, op: op.kind === 'Play'
  ? { source: { Sample: { key: 'sample' } }, ...op } : op });
test('changes coalesce and restore PCM, buses, voice controls and stable session IDs', async () => {
  const old = fake(), next = fake(), requested = [];
  const audio = restartableAudio(old, async (hint, rate) => { requested.push([hint, rate]); return next; });
  const session = audio.attach();
  const samples = new Float32Array([.1, .2]);
  audio.pcm(session, 'sample', samples);
  samples.fill(0); // Retained PCM must own its storage.
  audio.wire(session, wire('100', {kind:'Bus', effects:{}}));
  audio.wire(session, wire('1', {kind:'Play', bus:100}));
  audio.wire(session, wire('1', {kind:'Control', name:'SetVolume', args:[.5]}));
  audio.setLatencyHint('balanced'); audio.setLatencyHint('playback');
  await settled(audio);
  assert.deepEqual(requested, [['playback', 48000]]);
  assert.equal(audio.isPlaying(session, '1'), true);
  assert.equal(next.calls[1][0], 'pcm');
  assert.ok(next.calls[1][2][0] > 0);
  assert.deepEqual(next.calls.filter(c => c[0]==='wire').map(c=>c[1].op.kind), ['Bus','Play','Control']);
  assert.equal(old.closed, true);
  assert.equal(next.active, true);
  assert.equal(audio.bufferStatus().outputLatency, .02);
  await audio.close();
});
test('commands arriving while the replacement loads are replayed before committing', async () => {
  const old = fake(), next = fake();
  let ready;
  const gate = new Promise(resolve => ready = resolve);
  const audio = restartableAudio(old, async () => { await gate; return next; });
  const s = audio.attach();
  audio.wire(s, wire('1', {kind:'Play'}));
  audio.setLatencyHint('balanced');
  await pause(170);
  audio.wire(s, wire('1', {kind:'Control', name:'Stop'}));
  audio.wire(s, wire('2', {kind:'Play'}));
  assert.equal(old.closed, false);
  ready(); await settled(audio);
  assert.equal(audio.isPlaying(s,'1'), false);
  assert.equal(audio.isPlaying(s,'2'), true);
  assert.equal(audio.preferenceStatus().applied, 'balanced');
  await audio.close();
});
test('failed restoration keeps old audio and permits retrying the same preference', async () => {
  const old = fake(), failed = fake(), good = fake();
  failed.drain = async () => { throw new Error('restore rejected'); };
  let attempts = 0;
  const audio = restartableAudio(old, async () => ++attempts === 1 ? failed : good);
  audio.attach();
  audio.setLatencyHint('balanced'); await settled(audio);
  assert.equal(old.closed, false);
  assert.equal(failed.closed, true);
  assert.match(audio.preferenceStatus().error, /restore rejected/);
  audio.setLatencyHint('balanced'); await settled(audio);
  assert.equal(audio.preferenceStatus().applied, 'balanced');
  assert.equal(old.closed, true);
  await audio.close();
});
test('session reset while loading discards the prior session before replaying new sounds', async () => {
  const old = fake(), next = fake();
  let ready;
  const gate = new Promise(resolve => ready = resolve);
  const audio = restartableAudio(old, async () => { await gate; return next; });
  audio.attach(); audio.setLatencyHint('playback'); await pause(170);
  const session = audio.attach();
  audio.wire(session, wire('2', {kind:'Play'}));
  ready(); await settled(audio);
  assert.equal(audio.isPlaying(session, '2'), true);
  await audio.close();
});
test('a superseded context never becomes audible and only the latest mode commits', async () => {
  const old = fake(), superseded = fake(), latest = fake();
  let ready;
  const gate = new Promise(resolve => ready = resolve);
  const targets = [];
  const audio = restartableAudio(old, async hint => {
    targets.push(hint);
    if (hint === 'balanced') { await gate; return superseded; }
    return latest;
  });
  audio.attach(); audio.setLatencyHint('balanced'); await pause(170);
  audio.setLatencyHint('playback'); ready(); await settled(audio);
  assert.deepEqual(targets, ['balanced', 'playback']);
  assert.equal(superseded.closed, true);
  assert.equal(superseded.active, false);
  assert.equal(latest.active, true);
  assert.equal(audio.preferenceStatus().applied, 'playback');
  await audio.close();
});
test('sounds that finish during preparation are not resurrected at the switch', async () => {
  const old = fake(), next = fake();
  let ready;
  const gate = new Promise(resolve => ready = resolve);
  const audio = restartableAudio(old, async () => { await gate; return next; });
  const session = audio.attach();
  audio.wire(session, wire('1', {kind:'Play'}));
  audio.setLatencyHint('balanced'); await pause(170);
  old.playing.clear();
  ready(); await settled(audio);
  assert.equal(audio.preferenceStatus().applied, 'balanced');
  assert.equal(audio.isPlaying(session, '1'), false);
  await audio.close();
});
test('continuous listener updates cannot keep the handoff chasing acknowledgements', async () => {
  const old = fake(), next = fake();
  let audio, session, drains = 0;
  next.drain = async () => {
    drains++;
    audio.wire(session, wire('0', {kind:'Control', name:'SetListenerPosition', args:[drains,0,0]}));
  };
  audio = restartableAudio(old, async () => next);
  session = audio.attach();
  audio.setLatencyHint('balanced'); await settled(audio);
  assert.equal(audio.preferenceStatus().applied, 'balanced');
  assert.ok(drains < 10);
  const last = next.calls.filter(c => c[0] === 'wire').at(-1)[1];
  assert.equal(last.op.args[0], drains);
  await audio.close();
});
test('a stalled factory times out and disposes a replacement arriving late', async () => {
  const old = fake(), late = fake();
  let finish, signal;
  const audio = restartableAudio(old, (_hint, _rate, abort) => {
    signal = abort;
    return new Promise(resolve => finish = resolve);
  }, 'interactive', 30);
  audio.attach(); audio.setLatencyHint('balanced'); await settled(audio);
  assert.match(audio.preferenceStatus().error, /timed out: preparing context/);
  assert.equal(signal.aborted, true);
  assert.equal(old.closed, false);
  finish(late); await pause(0);
  assert.equal(late.closed, true);
  await audio.close();
});
test('a pending browser close cannot keep a successful switch applying', async () => {
  const old = fake(), next = fake();
  old.close = () => new Promise(() => {});
  const audio = restartableAudio(old, async () => next);
  audio.attach(); audio.setLatencyHint('playback'); await settled(audio);
  assert.equal(audio.preferenceStatus().applied, 'playback');
  await audio.close();
});
test('slow startup coalesces 20000 spatial updates without crossing note or PCM barriers', async () => {
  const old = fake(), next = fake();
  let ready, started;
  const began = new Promise(resolve => started = resolve);
  const gate = new Promise(resolve => ready = resolve);
  const audio = restartableAudio(old, async () => { started(); await gate; return next; });
  const session = audio.attach();
  audio.setLatencyHint('balanced'); await began;
  for (let frame = 0; frame < 10000; frame++) {
    if (frame === 5000) {
      audio.pcm(session, 'sample', new Float32Array([.1]));
      audio.wire(session, wire('7', {kind:'Play'}));
    }
    audio.wire(session, wire('0', {kind:'Control', name:'SetListenerPosition', args:[frame,0,0]}));
    audio.wire(session, wire('0', {kind:'Control', name:'SetListenerForward', args:[frame,0,1]}));
  }
  audio.wire(session, wire('7', {kind:'Control', name:'Stop'}));
  ready(); await settled(audio);
  assert.equal(audio.preferenceStatus().applied, 'balanced');
  assert.equal(audio.preferenceStatus().error, null);
  const messages = next.calls.filter(c => c[0] === 'wire');
  assert.deepEqual(messages.map(c => c[1].op.name ?? c[1].op.kind), [
    'SetListenerPosition','SetListenerForward','Play','SetListenerPosition','SetListenerForward','Stop',
  ]);
  assert.deepEqual(messages.filter(c => c[1].op.name === 'SetListenerPosition').map(c => c[1].op.args[0]), [4999,9999]);
  assert.ok(next.calls.findIndex(c => c[0] === 'pcm') < next.calls.findIndex(c => c[0] === 'wire' && c[1].op.kind === 'Play'));
  await audio.close();
});
test('a non-coalescible command flood still fails safely without dropping stop barriers', async () => {
  const old = fake(), next = fake();
  let ready, started;
  const began = new Promise(resolve => started = resolve);
  const gate = new Promise(resolve => ready = resolve);
  const audio = restartableAudio(old, async () => { started(); await gate; return next; });
  const session = audio.attach();
  audio.setLatencyHint('playback'); await began;
  for (let i = 0; i < 600; i++) audio.wire(session, wire('0', {kind:'Control', name:'StopAll'}));
  ready(); await settled(audio);
  assert.match(audio.preferenceStatus().error, /changed too quickly/);
  assert.equal(old.closed, false);
  assert.equal(next.closed, true);
  await audio.close();
});
test('synth voices and controls are dropped, including heavy note traffic while preparing', async () => {
  const old = fake(), next = fake();
  let ready, started;
  const began = new Promise(resolve => started = resolve);
  const gate = new Promise(resolve => ready = resolve);
  const audio = restartableAudio(old, async () => { started(); await gate; return next; });
  const session = audio.attach();
  const synth = {kind:'Play', source:{Track:{events:[{Note:{}}]}}};
  audio.wire(session, wire('synth', synth));
  audio.wire(session, wire('music', {kind:'Play'}));
  audio.setLatencyHint('balanced'); await began;
  for (let i = 0; i < 1500; i++) {
    audio.wire(session, wire(String(i), synth));
    audio.wire(session, wire(String(i), {kind:'Control',name:'SetVolume',args:[.5]}));
    audio.wire(session, wire(String(i), {kind:'Control',name:'Stop'}));
  }
  ready(); await settled(audio);
  assert.equal(audio.preferenceStatus().applied, 'balanced');
  assert.equal(audio.isPlaying(session, 'synth'), false);
  assert.equal(audio.isPlaying(session, 'music'), true);
  assert.deepEqual(next.calls.filter(c=>c[0]==='wire').map(c=>c[1].id), ['music']);
  assert.equal(audio.wire(session, wire('fresh', synth)), true);
  assert.equal(audio.isPlaying(session,'fresh'), true);
  await audio.close();
});
test('effected sample-only tracks/mixers survive, mixed synth/sample graphs do not', async () => {
  const old = fake(), next = fake();
  const audio = restartableAudio(old, async () => next);
  const session = audio.attach();
  audio.wire(session, wire('track', {kind:'Play',source:{Track:{events:[{Sample:{key:'sample'}}]}}}));
  audio.wire(session, wire('mixer', {kind:'Play',source:{Mixer:{buses:[{tracks:[{events:[{Sample:{key:'sample'}}]}]}]}}}));
  audio.wire(session, wire('mixed', {kind:'Play',source:{Track:{events:[{Sample:{key:'sample'}},{Note:{}}]}}}));
  audio.setLatencyHint('playback'); await settled(audio);
  assert.equal(audio.isPlaying(session,'track'),true);
  assert.equal(audio.isPlaying(session,'mixer'),true);
  assert.equal(audio.isPlaying(session,'mixed'),false);
  await audio.close();
});

test('transfers current sample position before activation, and resumes old audio if transfer fails', async () => {
  const old = fake(), next = fake();
  old.capturePlayback = async ids => { old.held = true; assert.deepEqual(ids, ['1']); return '[["1",{"elapsed_time":37.25,"paused":true}]]'; };
  next.activate = () => { assert.deepEqual(next.calls.find(c => c[0] === 'position')[1], [['1', {elapsed_time:37.25, paused:true}]]); next.active = true; };
  const audio = restartableAudio(old, async () => next);
  const s = audio.attach(); audio.wire(s, wire('1', {kind:'Play'}));
  audio.setLatencyHint('balanced'); await settled(audio);
  assert.equal(next.active, true); await audio.close();
  const previous = fake(), broken = fake();
  broken.restorePlayback = async () => { throw new Error('transfer failed'); };
  const retry = restartableAudio(previous, async () => broken);
  retry.attach(); retry.setLatencyHint('playback'); await settled(retry);
  assert.equal(previous.held, false); assert.equal(previous.closed, false);
  assert.equal(retry.preferenceStatus().applied, 'interactive'); await retry.close();
});

test('late sample starts precede snapshot restoration; post-snapshot controls follow it', async () => {
  const old = fake(), next = fake();
  const audio = restartableAudio(old, async () => next);
  const s = audio.attach();
  let injected = false;
  next.drain = async () => {
    if (!injected) { injected = true; audio.wire(s, wire('late', {kind:'Play'})); }
  };
  old.capturePlayback = async ids => {
    assert.ok(ids.includes('late'));
    audio.wire(s, wire('late', {kind:'Control', name:'Pause', args:[]}));
    return '[["late",{"elapsed_time":0.2}]]';
  };
  next.restorePlayback = async snapshot => {
    assert.ok(next.playing.has('late'));
    next.calls.push(['position', JSON.parse(snapshot)]);
  };
  audio.setLatencyHint('playback'); await settled(audio);
  const position = next.calls.findIndex(c => c[0] === 'position');
  const paused = next.calls.findIndex(c => c[0] === 'wire' && c[1].op.name === 'Pause');
  assert.ok(paused > position);
  await audio.close();
});
