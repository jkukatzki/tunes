import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import { AdaptiveBuffer } from "./adaptive-buffer.mjs";
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
    sampleRate: 44100,
    state: "running",
    audioWorklet: { async addModule() {} },
    destination: {},
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
      worker = this;
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
    constructor() {
      this.port = { postMessage() {} };
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
    AdaptiveBuffer,
    Worker,
    AudioWorkletNode,
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
      worker.onmessage({
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

test("diagnostics aggregate intervals and rate-limit console output", async () => {
  let now = 0;
  const logs = [];
  const { bridge, worker, nodes, audio } = await setup(
    { diagnostics: true },
    {
      performance: { now: () => now },
      console: {
        info: (...args) => logs.push(args),
        warn: (...args) => logs.push(args),
      },
    },
  );
  audio.currentTime = 5;
  worker.onmessage({
    data: {
      type: "timing",
      windowRenders: 100,
      windowRenderMs: 50,
      windowMaxRenderMs: 2,
      maxRenderGapMs: 80,
      maxCallbackMs: 3,
      maxBatchMs: 4,
      rejected: 2,
    },
  });
  const health = (frames, underruns) =>
    nodes[0].port.onmessage({
      data: {
        type: "health",
        renderedFrames: frames,
        underrunFrames: underruns,
        queuedBlocks: 1,
      },
    });
  now = 1000;
  health(44100, 441);
  assert.equal(logs.length, 0);
  now = 5000;
  health(220500, 441);
  const first = JSON.parse(
    logs[0][0].slice("[tunes] Audio diagnostics ".length),
  );
  assert.equal(first.underrunMs, 10);
  assert.equal(first.meanRenderMs, 0.5);
  assert.equal(first.maxRenderGapMs, 80);
  now = 10000;
  health(441000, 441);
  const second = JSON.parse(
    logs[1][0].slice("[tunes] Audio diagnostics ".length),
  );
  assert.equal(second.underrunMs, 0);
  assert.equal(second.timingReports, 0);
  assert.equal(second.maxRenderGapMs, 0);
  assert.equal(second.rejectedCommands, 0);
  await bridge.close();
});

test("adaptive mode sends bounded live targets without recreating the audio graph", async () => {
  const { bridge, nodes, posts } = await setup({
    bufferBlocks: 24,
    adaptiveBuffering: true,
  });
  assert.equal(posts.find((p) => p.type === "init").maxBufferBlocks, 32);
  for (let i = 1; i <= 5; i++)
    nodes[0].port.onmessage({
      data: {
        type: "health",
        renderedFrames: i * 44100,
        underrunFrames: i * 128,
        queuedBlocks: 0,
      },
    });
  assert.equal(posts.find((p) => p.type === "buffer-target").bufferBlocks, 32);
  assert.equal(nodes.length, 1);
  await bridge.close();
});

test("live block size waits for worker acknowledgement and clamps adaptive target", async () => {
  const { bridge, worker, posts } = await setup({
    bufferBlocks: 24,
    adaptiveBuffering: true,
  });
  bridge.setBlockFrames(8192);
  assert.equal(posts.at(-1).type, "block-frames");
  assert.equal(bridge.bufferStatus().blockFrames, 512);
  worker.onmessage({ data: { type: "block-frames-ready", frames: 8192 } });
  assert.equal(bridge.bufferStatus().blockFrames, 8192);
  assert.equal(bridge.bufferStatus().blocks, 4);
  assert.equal(posts.at(-1).type, "buffer-target");
  assert.throws(() => bridge.setBlockFrames(999), /Invalid DSP block size/);
  await bridge.close();
});
