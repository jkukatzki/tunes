import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

function setup() {
  let Processor;
  const reports = [];
  const returned = [];
  const context = {
    Float32Array,
    sampleRate: 44100,
    AudioWorkletProcessor: class {
      constructor() {
        this.port = { postMessage: (m) => reports.push(m) };
      }
    },
    registerProcessor: (_name, p) => {
      Processor = p;
    },
  };
  vm.runInNewContext(
    readFileSync(new URL("./output-worklet.js", import.meta.url), "utf8"),
    context,
  );
  const processor = new Processor();
  const port = { start() {}, postMessage: (packet) => returned.push(packet) };
  processor.port.onmessage({ data: { type: "connect", port } });
  return { processor, port, returned, reports };
}
test("PCM is consumed across render quanta and recycled exactly once", () => {
  const { processor, port, returned } = setup();
  const samples = new Float32Array(1024);
  for (let i = 0; i < 512; i++) {
    samples[2 * i] = i / 512;
    samples[2 * i + 1] = -i / 512;
  }
  port.onmessage({ data: { type: "pcm", samples } });
  let total = 0;
  for (const frames of [128, 64, 192, 128]) {
    const output = [new Float32Array(frames), new Float32Array(frames)];
    assert.equal(processor.process([], [output]), true);
    assert.equal(output[0][0], total / 512);
    assert.equal(output[1][frames - 1], -(total + frames - 1) / 512);
    total += frames;
  }
  assert.equal(returned.length, 1);
  assert.equal(returned[0].type, "recycle");
  assert.equal(processor.count, 0);
});
test("underruns output silence and buffering stays bounded", () => {
  const { processor, port, returned } = setup();
  const output = [new Float32Array(128).fill(1), new Float32Array(128).fill(1)];
  processor.process([], [output]);
  assert.ok(output.every((c) => c.every((x) => x === 0)));
  assert.equal(processor.underrunFrames, 128);
  for (let i = 0; i < 5; i++)
    port.onmessage({ data: { type: "pcm", samples: new Float32Array(1024) } });
  assert.equal(processor.count, 4);
  assert.equal(returned.length, 1);
  assert.equal(processor.blocks.length, 4);
});
test("engine generations flush queued audio and reject stale PCM", () => {
  const { processor, port, returned } = setup();
  port.onmessage({
    data: { type: "pcm", session: 0, samples: new Float32Array(1024).fill(1) },
  });
  processor.process([], [[new Float32Array(128), new Float32Array(128)]]);
  processor.port.onmessage({ data: { type: "generation", session: 1 } });
  assert.equal(processor.count, 0);
  assert.equal(processor.offset, 0);
  assert.equal(returned.length, 1);
  port.onmessage({
    data: { type: "pcm", session: 0, samples: new Float32Array(1024).fill(1) },
  });
  assert.equal(processor.count, 0);
  assert.equal(returned.length, 2);
  port.onmessage({
    data: {
      type: "pcm",
      session: 1,
      samples: new Float32Array(1024).fill(0.25),
    },
  });
  const output = [new Float32Array(128), new Float32Array(128)];
  processor.process([], [output]);
  assert.equal(output[0][0], 0.25);
});
