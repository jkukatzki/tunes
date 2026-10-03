/** Borrow the DSP output until the next render or WASM memory growth. Copy before transferring it.
 * Refresh the view after memory growth; retain compatibility with older modules.
 */
export function createBlockRenderer(dsp, memory) {
  if (typeof dsp.render_buffer !== "function" || !memory)
    return dsp.render.bind(dsp);
  let view;
  return () => {
    const pointer = dsp.render_buffer();
    const length = (dsp.block_frames?.() ?? 512) * 2;
    if (
      !view ||
      view.buffer !== memory.buffer ||
      view.byteOffset !== pointer ||
      view.length !== length
    ) {
      view = new Float32Array(memory.buffer, pointer, length);
    }
    return view;
  };
}
