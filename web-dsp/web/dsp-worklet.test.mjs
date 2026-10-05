import { test } from "node:test";
import assert from "node:assert/strict";
import { createDspProcessor } from "./dsp-worklet.mjs";
function setup(telemetry = true) {
  const posts = [],
    commands = [];
  let rendered = 0;
  const memory = new WebAssembly.Memory({ initial: 1 });
  class Dsp {
    protocol_version() {
      return 2;
    }
    set_block_frames(frames) {
      assert.equal(frames, 128);
    }
    collect_garbage() {}
    collect_garbage_budget(budget) {
      assert.equal(budget, 4);
      this.maintenanceCalls = (this.maintenanceCalls ?? 0) + 1;
    }
    flush_commands() {}
    playback_snapshot(ids) { return JSON.stringify(ids); }
    restore_playback(json) { commands.push(['restore', json]); }
    playing_ids() {
      return [];
    }
    reset() {
      commands.push("reset");
    }
    submit(json) {
      commands.push(json);
    }
    register_pcm(key) {
      commands.push(key);
    }
    free() {
      commands.push("free");
    }
  }
  const Processor = createDspProcessor({
    Base: class {
      constructor() {
        this.port = { postMessage: (p) => posts.push(p) };
      }
    },
    rate: 48000,
    now: () => 0,
    initSync: () => ({ memory }),
    DspWorker: Dsp,
    createBlockRenderer: () => () => {
      const samples = new Float32Array(memory.buffer, 0, 256);
      for (let i = 0; i < 128; i++)
        samples[i * 2] = samples[i * 2 + 1] = rendered++;
      return samples;
    },
  });
  const processor = new Processor();
  const send = (data) => processor.port.onmessage({ data });
  send({ type: "init", module: {}, telemetry });
  const process = (frames = 128) => {
    const output = [new Float32Array(frames), new Float32Array(frames)];
    processor.process([], [output]);
    return output;
  };
  return { processor, process, send, posts, commands, memory };
}
test("direct DSP renders on demand with no PCM recycle messages", () => {
  const { process, posts } = setup();
  const first = process()[0];
  const second = process()[0];
  assert.equal(first[127] + 1, second[0]);
  for (let i = 0; i < 400; i++) process();
  assert.ok(posts.some((p) => p.type === "health" && p.backend === "worklet"));
  assert.ok(posts.every((p) => !["pcm", "recycle"].includes(p.type)));
});
test("command dispatch is bounded, ordered, and drops stale sessions", () => {
  const { send, process, posts, commands } = setup();
  send({
    type: "batch",
    packets: [
      { kind: "reset", session: 1, seq: 1 },
      ...Array.from({ length: 9 }, (_, i) => ({
        kind: "wire",
        session: 1,
        seq: i + 2,
        json: String(i),
      })),
      { kind: "wire", session: 0, seq: 99, json: "stale" },
    ],
  });
  process();
  assert.equal(commands.length, 4);
  assert.ok(!posts.some((p) => p.type === "ack"));
  process();
  assert.equal(commands.length, 8);
  process();
  assert.equal(commands.length, 10);
  assert.equal(posts.find((p) => p.type === "ack").sequence, 10);
  assert.ok(!commands.includes("stale"));
});
test("partial blocks survive WASM memory growth and close stops rendering", () => {
  const { processor, process, memory, send, commands } = setup();
  const first = process(64)[0];
  memory.grow(1);
  assert.equal(process(64)[0][0], first[63] + 1);
  send({ type: "close" });
  assert.equal(processor.process([], [[new Float32Array(128)]]), false);
  assert.deepEqual(commands, ["free"]);
});
test("overlapping command batches fail explicitly", () => {
  const { send, posts } = setup();
  send({ type: "batch", packets: [] });
  send({ type: "batch", packets: [] });
  assert.equal(posts.at(-1).type, "fatal");
});

test("audio quanta use bounded retirement maintenance", () => {
  const { processor, process } = setup();
  const initial = processor.dsp.maintenanceCalls ?? 0;
  processor.dsp.collect_garbage = () => { throw new Error("unbounded cleanup"); };
  for (let i = 0; i < 100; i++) process();
  assert.equal(processor.failed, false);
  assert.equal(processor.dsp.maintenanceCalls, initial + 100);
});

test("snapshot holds playback until released and restore happens while held", () => {
  const { process, send, posts, commands } = setup();
  process();
  send({type:'batch', packets:[{kind:'playback-snapshot', session:0, seq:1, request:1, ids:['42']}]});
  process();
  assert.equal(posts.find(p => p.type === 'playback-reply').snapshot, '["42"]');
  assert.ok(process()[0].every(v => v === 0));
  send({type:'batch', packets:[{kind:'restore-playback', session:0, seq:2, request:2, snapshot:'[]'}]});
  process();
  assert.deepEqual(commands.at(-1), ['restore', '[]']);
  send({type:'release-playback'});
  assert.ok(process()[0].some(v => v !== 0));
});

test("cancelled late snapshot cannot freeze the surviving context", () => {
  const { process, send } = setup();
  send({type:'release-playback', cancelThrough:7});
  send({type:'batch', packets:[{kind:'playback-snapshot', session:0, seq:1, request:7, ids:[]}]});
  assert.ok(process()[0].some(v => v !== 0));
});

test("normal playback keeps status updates but skips timing and health reports", () => {
  const { process, posts } = setup(false);
  for (let i = 0; i < 400; i++) process();
  assert.ok(posts.some(p => p.type === 'status'));
  assert.ok(!posts.some(p => p.type === 'health' || p.type === 'timing'));
});


test("incoming message decode failures report a fatal error instead of hanging startup", () => {
  const { processor, posts, process } = setup();
  processor.port.onmessageerror();
  assert.equal(posts.at(-1).type, "fatal");
  assert.match(posts.at(-1).message, /could not decode/);
  assert.equal(processor.failed, true);
  assert.ok(process().every(channel => channel.every(sample => sample === 0)));
});
