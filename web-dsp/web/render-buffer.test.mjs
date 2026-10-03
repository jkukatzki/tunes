import { test } from "node:test";
import assert from "node:assert/strict";
import { createBlockRenderer } from "./render-buffer.mjs";
test("output views are reused but refreshed after memory growth or relocation", () => {
  const memory = new WebAssembly.Memory({ initial: 1 });
  let pointer = 0;
  const render = createBlockRenderer({ render_buffer: () => pointer }, memory);
  const first = render();
  assert.equal(render(), first);
  memory.grow(1);
  const grown = render();
  assert.notEqual(grown, first);
  assert.equal(grown.buffer, memory.buffer);
  pointer = 4096;
  const moved = render();
  assert.equal(moved.byteOffset, 4096);
  assert.notEqual(moved, grown);
});
test("legacy fallback is bound before callers replace their render method", () => {
  const dsp = {
    count: 0,
    render() {
      return ++this.count;
    },
  };
  dsp.render = createBlockRenderer(dsp);
  assert.equal(dsp.render(), 1);
  assert.equal(dsp.render(), 2);
});

test("view length follows live DSP block changes even without pointer relocation", () => {
  const memory = new WebAssembly.Memory({ initial: 2 });
  let frames = 512;
  const render = createBlockRenderer(
    { render_buffer: () => 0, block_frames: () => frames },
    memory,
  );
  for (const size of [512, 8192, 1024, 4096, 2048, 512]) {
    frames = size;
    assert.equal(render().length, size * 2);
  }
});
