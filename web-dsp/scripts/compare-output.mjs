// Compare two actual WASM artifacts, including note envelopes, effects and controls.
import { createBlockRenderer } from "../web/render-buffer.mjs";
import { readFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { pathToFileURL } from "node:url";
if (process.argv.length !== 4)
  throw new Error("Usage: compare-output.mjs <baseline-dir> <candidate-dir>");
async function load(directory) {
  const path = resolve(directory);
  const module = await import(pathToFileURL(join(path, "tunes_web_dsp.js")));
  const wasm = await module.default({
    module_or_path: readFileSync(join(path, "tunes_web_dsp_bg.wasm")),
  });
  return class {
    constructor(rate) {
      const dsp = new module.DspWorker(rate);
      dsp.render = createBlockRenderer(dsp, wasm.memory);
      return dsp;
    }
  };
}
const A = await load(process.argv[2]),
  B = await load(process.argv[3]);
let maxError = 0,
  squaredError = 0,
  comparedSamples = 0;
for (const rate of [44100, 48000])
  for (const voices of [1, 12, 96, 128])
    for (const wave of [0, 1, 2, 3]) {
      const pair = [new A(rate), new B(rate)];
      try {
        for (const dsp of pair) {
          dsp.effects(0.12, 0.2);
          for (let i = 0; i < voices; i++)
            dsp.note(
              i + 1,
              110 * 2 ** ((i % 36) / 12),
              wave,
              0.08,
              ((i % 3) - 1) * 0.4,
              0.01,
              0.2,
              0.7,
              0.05,
            );
        }
        for (let block = 0; block < 192; block++) {
          if (block === 70)
            for (const dsp of pair)
              for (let i = 1; i <= voices; i++)
                dsp.submit(
                  JSON.stringify({
                    version: 2,
                    id: String(i),
                    op: {
                      kind: "Control",
                      name: "SetPan",
                      args: [0.3],
                      position: null,
                      cone: null,
                      spatial: null,
                    },
                  }),
                );
          if (block === 120)
            for (const dsp of pair)
              for (let i = 1; i <= voices; i++) dsp.release(i, 0.02);
          const a = pair[0].render(),
            b = pair[1].render();
          if (a.length !== b.length) throw new Error("Output length changed");
          for (let i = 0; i < a.length; i++) {
            if (!Number.isFinite(a[i]) || !Number.isFinite(b[i]))
              throw new Error("Nonfinite audio");
            const error = Math.abs(a[i] - b[i]);
            maxError = Math.max(maxError, error);
            squaredError += error * error;
            comparedSamples++;
          }
          for (const dsp of pair) dsp.collect_garbage();
        }
      } finally {
        for (const dsp of pair) dsp.free();
      }
    }
// Exercise PCM resampling and end-of-sample boundaries as well as synthesis.
for (const rate of [44100, 48000])
  for (const speed of [0.5, 1, 1.5, 2])
    for (const length of [1, 67, 12000]) {
      const pair = [new A(rate), new B(rate)];
      try {
        const pcm = Float32Array.from(
          { length },
          (_, i) => 0.2 * Math.sin(i * 0.03),
        );
        for (const dsp of pair) {
          dsp.effects(0.12, 0.2);
          dsp.register_sample(1, pcm, 22050);
          dsp.play_sample(1, 1, speed, 0.4, 0.3);
        }
        for (let block = 0; block < 64; block++) {
          const a = pair[0].render(),
            b = pair[1].render();
          if (a.length !== b.length) throw new Error("Output length changed");
          for (let i = 0; i < a.length; i++) {
            if (!Number.isFinite(a[i]) || !Number.isFinite(b[i]))
              throw new Error("Nonfinite audio");
            const error = Math.abs(a[i] - b[i]);
            maxError = Math.max(maxError, error);
            squaredError += error * error;
            comparedSamples++;
          }
          for (const dsp of pair) dsp.collect_garbage();
        }
      } finally {
        for (const dsp of pair) dsp.free();
      }
    }
console.log(
  JSON.stringify(
    {
      comparedSamples,
      maxError,
      rmsError: Math.sqrt(squaredError / comparedSamples),
      pass: maxError <= 2e-5,
    },
    null,
    2,
  ),
);
if (maxError > 2e-5) process.exitCode = 1;
