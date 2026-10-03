import { test } from "node:test";
import assert from "node:assert/strict";
import { AdaptiveBuffer } from "./adaptive-buffer.mjs";
function simulation(blocks = 24) {
  const controller = new AdaptiveBuffer(blocks, 48000);
  let renderedFrames = 0,
    underrunFrames = 0;
  return {
    controller,
    step(missing = 0, active = true) {
      renderedFrames += 48000;
      underrunFrames += missing;
      return controller.observe({ renderedFrames, underrunFrames }, active);
    },
  };
}
test("grows after grace, never exceeds 32, and shrinks only after sustained clean audio", () => {
  const { controller, step } = simulation();
  for (let i = 0; i < 4; i++) assert.equal(step(4800), undefined);
  assert.deepEqual(step(128), { blocks: 32, reason: "output underrun" });
  for (let i = 0; i < 10; i++) step(128);
  assert.equal(controller.blocks, 32);
  for (let i = 0; i < 29; i++) assert.equal(step(), undefined);
  assert.equal(step().blocks, 30);
});
test("background and resume reset clean-playback credit and ignore resume underruns", () => {
  const { controller, step } = simulation();
  for (let i = 0; i < 30; i++) step();
  step(48000, false);
  for (let i = 0; i < 4; i++) assert.equal(step(48000), undefined);
  assert.equal(controller.blocks, 24);
  for (let i = 0; i < 29; i++) assert.equal(step(), undefined);
  assert.equal(step().blocks, 22);
});
test("stable playback never shrinks below four blocks", () => {
  const { controller, step } = simulation(4);
  for (let i = 0; i < 100; i++) assert.equal(step(), undefined);
  assert.equal(controller.blocks, 4);
});

test("each policy uses its configured clean interval and reduction", () => {
  for (const [policy, seconds, reduction] of [
    ["conservative", 60, 1],
    ["balanced", 30, 2],
    ["optimistic", 10, 2],
  ]) {
    const controller = new AdaptiveBuffer(8, 48000, policy);
    let frames = 0;
    const step = () =>
      controller.observe(
        { renderedFrames: (frames += 48000), underrunFrames: 0 },
        true,
      );
    for (let i = 0; i < 4 + seconds - 1; i++) assert.equal(step(), undefined);
    assert.equal(step().blocks, 8 - reduction);
  }
});
test("changing policy preserves target and resets accumulated shrink credit", () => {
  const controller = new AdaptiveBuffer(8, 48000);
  controller.clean = 29 * 48000;
  controller.setPolicy("optimistic");
  assert.equal(controller.blocks, 8);
  assert.equal(controller.clean, 0);
  assert.equal(controller.cleanSeconds, 10);
  controller.clean = 48000;
  controller.setPolicy("optimistic");
  assert.equal(controller.clean, 48000);
  assert.throws(
    () => controller.setPolicy("invalid"),
    /Unknown buffering policy/,
  );
});

test("large blocks constrain adaptive lookahead and reset shrink observations", () => {
  const controller = new AdaptiveBuffer(24, 48000);
  controller.setBlockFrames(2048);
  assert.equal(controller.maxBlocks, 9);
  assert.equal(controller.blocks, 9);
  controller.setBlockFrames(8192);
  assert.equal(controller.blocks, 4);
  controller.setBlockFrames(512);
  assert.equal(controller.maxBlocks, 32);
  assert.equal(controller.blocks, 4);
});
