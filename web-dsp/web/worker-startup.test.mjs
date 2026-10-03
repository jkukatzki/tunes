import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

function worker(loadModule) {
  const messages = [];
  const context = vm.createContext({
    URL,
    Float32Array,
    Set,
    performance,
    loadModule,
    postMessage: (message) => messages.push(message),
  });
  const source = readFileSync(new URL("./worker.js", import.meta.url), "utf8")
    .replaceAll(
      "import.meta.url",
      JSON.stringify("https://game.test/audio/worker.js?v=1"),
    )
    .replaceAll("import(", "loadModule(");
  // Synchronous evaluation must install the handler without waiting for imports.
  vm.runInContext(source, context);
  return { context, messages };
}

test("worker receives init before slow imports finish and then becomes ready", async () => {
  let finishImport;
  const pending = new Promise((resolve) => {
    finishImport = resolve;
  });
  const imports = [];
  let frames = 512;
  const { context, messages } = worker((url) => {
    imports.push(url);
    return imports.length === 1
      ? pending
      : Promise.resolve({
          createBlockRenderer: () => () => new Float32Array(frames * 2),
        });
  });
  assert.equal(typeof context.onmessage, "function");
  assert.equal(imports.length, 0);
  let produced = 0;
  const packets = [];
  const port = {
    start() {},
    postMessage(packet) {
      produced++;
      packets.push(packet);
    },
  };
  const init = context.onmessage({
    data: {
      type: "init",
      sampleRate: 44100,
      bufferBlocks: 24,
      maxBufferBlocks: 32,
      wasmUrl: "dsp.wasm",
      port,
    },
  });
  assert.equal(messages[0].stage, "loading DSP bindings");
  finishImport({
    default: async () => ({ memory: {} }),
    DspWorker: class {
      set_block_frames(value) {
        frames = value;
      }
      protocol_version() {
        return 2;
      }
      playing_ids() {
        return [];
      }
      collect_garbage() {}
    },
  });
  await init;
  assert.equal(messages.at(-1).type, "ready");
  assert.equal(produced, 24);
  await context.onmessage({ data: { type: "buffer-target", bufferBlocks: 8 } });
  // Downsizing does not discard or replace any queued PCM.
  assert.equal(produced, 24);
  const consumed = packets.splice(0, 16);
  for (const packet of consumed) {
    packet.type = "recycle";
    port.onmessage({ data: packet });
  }
  assert.equal(produced, 24);
  const next = packets.shift();
  next.type = "recycle";
  port.onmessage({ data: next });
  assert.equal(produced, 25);
  await context.onmessage({
    data: { type: "buffer-target", bufferBlocks: 32 },
  });
  assert.equal(packets.length, 32);
  assert.equal(new Set(packets).size, 32);
  await context.onmessage({ data: { type: "block-frames", frames: 8192 } });
  assert.equal(messages.at(-1).type, "block-frames-ready");
  const oldPacket = packets.shift();
  assert.equal(oldPacket.samples.length, 1024);
  oldPacket.type = "recycle";
  port.onmessage({ data: oldPacket });
  assert.equal(packets.at(-1).samples.length, 16384);
  assert.equal(packets.length, 32);
  await context.onmessage({
    data: { type: "buffer-target", bufferBlocks: 33 },
  });
  assert.equal(messages.at(-1).type, "fatal");
  assert.ok(imports.every((url) => url.endsWith("?v=1")));
});

test("failed dependency import reports its stage instead of silently timing out", async () => {
  const { context, messages } = worker(async () => {
    throw new Error("HTTP 404");
  });
  await context.onmessage({ data: { type: "init" } });
  assert.equal(messages.at(-1).type, "fatal");
  assert.match(
    messages.at(-1).message,
    /loading DSP bindings: Error: HTTP 404/,
  );
});
