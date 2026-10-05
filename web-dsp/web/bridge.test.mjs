import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import { restartableAudio } from "./audio-restart.mjs";
import { AudioCommandQueue } from "./command-queue.mjs";
async function setup(options = {}, globals = {}) {
  const events = new Map(),
    posts = [],
    nodes = [];
  const target = {
    addEventListener: (k, v) => events.set(k, v),
    removeEventListener: (k) => events.delete(k),
  };
  const audio = {
    currentTime: 0,
    sampleRate: 44100,
    state: "running",
    audioWorklet: { async addModule() {} },
    destination: {},
    createGain() { return { gain: { value: 1 }, connect() {}, disconnect() {} }; },
    async resume() {
      this.state = "running";
    },
    async suspend() {
      this.state = "suspended";
    },
    async close() {
      this.state = "closed";
    },
  };
  let worker;
  class Worker {
    constructor() {
      throw new Error("Dedicated Worker must not be constructed");
    }
    postMessage(m) {
      posts.push(m);
      if (m.type === "init")
        queueMicrotask(() =>
          this.onmessage({ data: { type: "ready", protocol: 2 } }),
        );
    }
    terminate() {}
  }
  class AudioWorkletNode {
    constructor(_context, name) {
      this.name = name;
      this.port = {
        postMessage: (message, transfer) => {
          if (message.type === "init") {
            // Model Chrome rejecting compiled modules at the receiving port.
            assert.ok(message.module instanceof ArrayBuffer);
            assert.equal(transfer.length, 1);
            assert.equal(transfer[0], message.module);
          }
          if (name === "tunes-dsp") {
            posts.push(message);
            if (message.type === "batch" && globals.autoAck) {
              this.session ??= 0;
              this.playing ??= new Set();
              for (const packet of message.packets) {
                if (packet.kind === 'playback-snapshot' || packet.kind === 'restore-playback')
                  queueMicrotask(() => this.port.onmessage({data: { type: 'playback-reply', request: packet.request,
                    snapshot: JSON.stringify((packet.ids ?? []).map(id => [id, {elapsed_time:12}])) }}));
                if (packet.kind === "reset") { this.session = packet.session; this.playing.clear(); }
                if (packet.kind === "wire") {
                  const command = JSON.parse(packet.json);
                  if (command.op.kind === "Play") this.playing.add(command.id);
                }
              }
              queueMicrotask(() => this.port.onmessage({ data: {
                type: "ack", session: this.session, sequence: message.packets.at(-1).seq,
                playing: [...this.playing], errors: [],
              } }));
            }
            if (message.type === "init")
              queueMicrotask(() =>
                this.port.onmessage({ data: { type: "ready", protocol: 2 } }),
              );
          }
        },
      };
      nodes.push(this);
    }
    connect() {}
    disconnect() {}
  }
  const context = vm.createContext({
    URL,
    performance,
    Float32Array,
    AudioCommandQueue,
    restartableAudio,
    Worker,
    AudioWorkletNode,
    AudioContext: class {
      constructor(options) { Object.assign(this, audio); this.options = options; this.state = "running"; }
    },
    MessageChannel: class {
      constructor() {
        this.port1 = {};
        this.port2 = {};
      }
    },
    setTimeout,
    clearTimeout,
    console,
    navigator: { audioSession: { type: "auto" } },
    window: target,
    document: { ...target, hidden: false },
    WebAssembly: { compile: async () => ({ compiled: true }) },
    fetch: async () => ({ ok: true, arrayBuffer: async () => new ArrayBuffer(0) }),
    crypto: { getRandomValues: bytes => bytes.fill(1) },
    Uint8Array,
    ...globals,
  });
  let source = readFileSync(new URL("./bridge.js", import.meta.url), "utf8");
  source = source
    .slice(source.indexOf("const releaseNames"))
    .replaceAll(
      "import.meta.url",
      JSON.stringify(
        "https://game.test/game/audio/bridge.js?pibits-game-version=1",
      ),
    )
    .replaceAll("export async function", "async function")
    .replaceAll("export const", "const");
  vm.runInContext(source, context);
  const bridge = await context.createWorkerAudio({
    context: audio,
    ...options,
  });
  return {
    bridge,
    worker,
    nodes,
    audio,
    events,
    posts,
    context,
    ack: (data) =>
      nodes[0].port.onmessage({
        data: { type: "ack", playing: [], errors: [], ...data },
      }),
  };
}
const play = (id) => JSON.stringify({ version: 2, id, op: { kind: "Play" } });
test("pending attacks survive older acknowledgements and obsolete sessions cannot stop new audio", async () => {
  const { bridge, posts, ack } = await setup();
  const s = bridge.attach();
  bridge.wire(s, play("1"));
  assert.equal(bridge.isPlaying(s, "1"), true);
  ack({ session: s, sequence: 1 });
  assert.equal(bridge.isPlaying(s, "1"), true);
  const seq = posts.at(-1).packets.at(-1).seq;
  ack({ session: s, sequence: seq });
  assert.equal(bridge.isPlaying(s, "1"), false);
  const newer = bridge.attach();
  bridge.wire(newer, play("1"));
  bridge.detach(s);
  ack({ session: s, sequence: 999 });
  assert.equal(bridge.isPlaying(newer, "1"), true);
  assert.equal(bridge.wire(s, play("2")), false);
  await bridge.close();
});
test("pagehide suspends even before document.hidden changes; gestures retry foreground resume", async () => {
  const { bridge, audio, events, context } = await setup();
  events.get("pagehide")();
  assert.equal(audio.state, "suspended");
  assert.equal(context.navigator.audioSession.type, "auto");
  events.get("pageshow")();
  assert.equal(audio.state, "running");
  audio.state = "suspended";
  events.get("pointerup")();
  assert.equal(audio.state, "running");
  await bridge.close();
  assert.equal(events.size, 0);
});
test("PCM capacity rejection is synchronous and eviction permits reuse", async () => {
  const { bridge, ack } = await setup();
  const s = bridge.attach();
  ack({ session: s, sequence: 1 });
  for (let i = 0; i < 128; i++)
    assert.equal(bridge.pcm(s, String(i), new Float32Array(2)), true);
  assert.equal(bridge.pcm(s, "overflow", new Float32Array(2)), false);
  assert.equal(bridge.removePcm(s, "0"), true);
  assert.equal(bridge.pcm(s, "replacement", new Float32Array(2)), true);
  await bridge.close();
});

test("installed bridge prevents duplicate ownership and releases globals on close", async () => {
  const { bridge, audio, context } = await setup();
  await bridge.close();
  audio.state = "running";
  const installed = await context.installWorkerAudio({ context: audio });
  assert.equal(typeof context.__tunesWorkerAttach, "function");
  await assert.rejects(
    context.installWorkerAudio({ context: audio }),
    /already installed/,
  );
  await installed.close();
  await installed.close();
  assert.equal(context.__tunesWorkerAttach, undefined);
});

test("direct backend starts without a Worker", async () => {
  const { bridge, worker, nodes, posts, audio } = await setup(
    {},
    {
      WebAssembly: { compile: async () => ({ compiled: true }) },
      fetch: async () => ({
        ok: true,
        arrayBuffer: async () => new ArrayBuffer(0),
      }),
      crypto: { getRandomValues: (bytes) => bytes.fill(1) },
      Uint8Array,
    },
  );
  assert.equal(worker, undefined);
  assert.equal(nodes[0].name, "tunes-dsp");
  assert.equal(bridge.bufferStatus().blockFrames, 128);
  const before = posts.length;
  assert.equal(bridge.setBlockFrames, undefined);
  assert.equal(bridge.setBufferLimit, undefined);
  assert.equal(posts.length, before);
  nodes[0].port.onmessage({ data: { type: "entropy-needed" } });
  assert.equal(posts.at(-1).bytes.length, 65536);
  await bridge.close();
  assert.equal(audio.state, "closed");
  assert.equal(posts.at(-1).type, "close");
});

const flushPromises = async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); };
test("a late hide suspension resumes after returning, without replacing DSP", async () => {
  const { bridge, audio, events, posts } = await setup();
  let finish;
  audio.suspend = () => new Promise(resolve => {
    finish = () => { audio.state = "suspended"; resolve(); };
  });
  events.get("pagehide")();
  events.get("pointerup")(); // document.hidden can still be false during pagehide.
  events.get("pageshow")();
  finish();
  await flushPromises();
  assert.equal(audio.state, "running");
  assert.equal(posts.filter(p => p.type === "init").length, 1);
  await bridge.close();
});
test("a late foreground resume cannot restart hidden audio", async () => {
  const { bridge, audio, events } = await setup();
  let finish;
  audio.resume = () => new Promise(resolve => {
    finish = () => { audio.state = "running"; resolve(); };
  });
  events.get("pageshow")();
  events.get("pagehide")();
  finish();
  await flushPromises();
  assert.equal(audio.state, "suspended");
  await bridge.close();
});
test("frozen running context gets one recovery cycle, then a gesture can retry", async () => {
  const timers = new Map();
  let next = 0;
  const { bridge, audio, events } = await setup({}, {
    setTimeout: cb => { timers.set(++next, cb); return next; },
    clearTimeout: id => timers.delete(id),
  });
  let suspends = 0;
  audio.suspend = async () => { suspends++; audio.state = "suspended"; };
  const tick = async () => {
    const entries = [...timers];
    timers.clear();
    for (const [, cb] of entries) cb();
    await flushPromises();
  };
  events.get("pageshow")();
  await tick();
  assert.equal(suspends, 1);
  await tick();
  assert.equal(suspends, 1);
  assert.equal(timers.size, 0);
  events.get("pointerup")();
  audio.currentTime += 0.6;
  await tick();
  assert.equal(suspends, 1);
  events.get("pointerup")();
  assert.equal(timers.size, 0);
  events.get("pageshow")();
  await bridge.close();
  assert.equal(timers.size, 0);
});

test("removed worker backend is rejected explicitly", async () => {
  await assert.rejects(setup({ backend: "worker" }), /Only AudioWorklet/);
});

test("installed globals survive an automatic context replacement", async () => {
  const { bridge, context, audio, nodes } = await setup({}, { autoAck: true });
  await bridge.close();
  const installed = await context.installWorkerAudio({ context: audio });
  const session = context.__tunesWorkerAttach();
  context.__tunesWorkerPcm(session, "sample", new Float32Array([0.1, 0.2]));
  context.__tunesWorkerSend(session, JSON.stringify({version:2,id:"42",op:{kind:"Play",source:{Sample:{key:"sample"}}}}));
  const send = context.__tunesWorkerSend;
  assert.equal(context.__tunesAudioLatencyHint("balanced"), true);
  for (let i = 0; i < 400 && installed.preferenceStatus().changing; i++)
    await new Promise(resolve => setTimeout(resolve, 5));
  assert.equal(installed.preferenceStatus().applied, "balanced");
  assert.equal(installed.preferenceStatus().error, null);
  assert.equal(context.__tunesWorkerSend, send);
  assert.equal(context.__tunesWorkerPlaying(session, "42"), true);
  assert.equal(nodes.length, 3);
  await installed.close();
  assert.equal(context.__tunesAudioLatencyHint, undefined);
});
