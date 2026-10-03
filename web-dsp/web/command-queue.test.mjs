import { test } from "node:test";
import assert from "node:assert/strict";
import { AudioCommandQueue } from "./command-queue.mjs";
function setup() {
  const sent = [],
    cancelled = [];
  const queue = new AudioCommandQueue(
    (p) => sent.push(p),
    (id) => cancelled.push(id),
  );
  queue.enqueue({ kind: "reset" });
  return { queue, sent, cancelled };
}
test("parameters coalesce only between attack/release barriers", () => {
  const { queue, sent } = setup();
  queue.enqueue({ json: "first" }, { key: "pan" });
  queue.enqueue({ json: "latest" }, { key: "pan" });
  queue.enqueue({ playId: "1" });
  queue.enqueue({ json: "after" }, { key: "pan" });
  queue.ack();
  assert.deepEqual(
    sent[1].map((p) => p.json ?? p.playId),
    ["latest", "1", "after"],
  );
  assert.equal(queue.bytes, 0);
});
test("backpressure reserves release capacity and emergency stop always fits", () => {
  const { queue, cancelled } = setup();
  for (let i = 0; i < 256; i++)
    assert.notEqual(queue.enqueue({ playId: String(i) }), false);
  assert.equal(queue.enqueue({ playId: "rejected" }), false);
  for (let i = 0; i < 128; i++)
    assert.notEqual(
      queue.enqueue({ kind: "remove-pcm" }, { critical: true }),
      false,
    );
  assert.equal(queue.enqueue({ kind: "wire" }, { critical: true }), false);
  assert.notEqual(queue.emergencyStop({ kind: "wire", json: "stop" }), false);
  assert.equal(cancelled.length, 256);
  assert.equal(queue.pending.at(-1).json, "stop");
  const full = setup().queue;
  for (let i = 0; i < 384; i++)
    full.enqueue({ kind: "remove-pcm" }, { critical: true });
  assert.notEqual(full.emergencyStop({ kind: "wire", json: "stop" }), false);
  assert.equal(full.pending.length, 385);
  full.emergencyStop({ kind: "wire", json: "stop again" });
  assert.equal(full.pending.length, 385);
});
test("transfer byte accounting remains finite through reset and PCM packets", () => {
  const { queue } = setup();
  queue.enqueue({ kind: "pcm", samples: new Float32Array(20) });
  queue.enqueue({ kind: "reset" });
  assert.equal(queue.bytes, 80);
  queue.ack();
  assert.equal(queue.bytes, 0);
});
