// In-process WASM CPU benchmark. Device scheduling/underruns require the browser harness.
import { readFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { pathToFileURL, fileURLToPath } from "node:url";
import { performance } from "node:perf_hooks";
import { cpus, platform, arch } from "node:os";
import { createHash } from "node:crypto";
import { createBlockRenderer } from "../web/render-buffer.mjs";
import { summarize, validateDuration } from "./benchmark-utils.mjs";
const args = process.argv.slice(2);
const quick = args.includes("--quick");
const focus = args.includes("--focus");
const ownedOutput = args.includes("--owned-output");
if (
  args.some(
    (a) =>
      a.startsWith("--") &&
      a !== "--quick" &&
      a !== "--focus" &&
      a !== "--owned-output",
  )
)
  throw new Error(
    "Usage: benchmark.mjs [distribution-directory] [--quick] [--focus] [--owned-output]",
  );
const directory = resolve(
  args.find((a) => !a.startsWith("--")) ??
    fileURLToPath(new URL("../dist", import.meta.url)),
);
const file = join(directory, "tunes_web_dsp_bg.wasm");
const bytes = readFileSync(file);
const { default: init, DspWorker } = await import(
  pathToFileURL(join(directory, "tunes_web_dsp.js"))
);
const wasm = await init({ module_or_path: bytes });
const frames = 512,
  warmup = quick ? 16 : 100,
  blocks = quick ? 100 : 1000,
  repetitions = quick ? 1 : 3;
const results = [];
const workloads = ["synth", "sample", "mixed", "churn", "controls"];
const requestedVoices = focus
  ? [96]
  : quick
    ? [12, 96, 128]
    : [1, 12, 48, 96, 128];
const rates = quick ? [44100] : [44100, 48000];
for (const rate of rates)
  for (const workload of workloads)
    for (const voices of requestedVoices) {
      validateDuration(warmup, blocks, rate);
      const rounds = [];
      for (let round = 0; round < repetitions; round++) {
        const dsp = new DspWorker(rate);
        const render = createBlockRenderer(
          dsp,
          ownedOutput ? undefined : wasm.memory,
        );
        let rejected = 0,
          nextId = voices + 1,
          peakPlaying = 0,
          energy = 0,
          nonFinite = 0;
        const ids = Array.from({ length: voices }, (_, i) => i + 1);
        const command = (fn) => {
          try {
            if (fn() === false) rejected++;
          } catch {
            rejected++;
          }
        };
        try {
          if (dsp.protocol_version() !== 2)
            throw new Error(
              "Rebuild the DSP distribution: protocol v2 required",
            );
          command(() => dsp.effects(0.12, 0.2));
          if (workload === "sample" || workload === "mixed") {
            const pcm = new Float32Array(rate * 30);
            for (let i = 0; i < pcm.length; i++)
              pcm[i] = 0.2 * Math.sin((2 * Math.PI * 220 * i) / rate);
            dsp.register_sample(1, pcm, rate);
          }
          const attack = (id, i) =>
            command(() =>
              workload === "sample" || (workload === "mixed" && i % 2 === 0)
                ? dsp.play_sample(id, 1, 1, 0.08, ((i % 3) - 1) * 0.4)
                : dsp.note(
                    id,
                    110 * 2 ** ((i % 36) / 12),
                    2,
                    0.08,
                    ((i % 3) - 1) * 0.4,
                    0.01,
                    0.2,
                    0.7,
                    0.05,
                  ),
            );
          ids.forEach(attack);
          for (let i = 0; i < warmup; i++) {
            render();
            dsp.collect_garbage();
          }
          const initialPlaying = dsp.playing_ids().length;
          const renderTimes = [],
            cycleTimes = [],
            controlTimes = [],
            cleanupTimes = [];
          for (let block = 0; block < blocks; block++) {
            const start = performance.now();
            if (workload === "churn" && block % 4 === 0) {
              for (let j = 0; j < Math.min(12, voices); j++) {
                const slot = ((block / 4) * 12 + j) % voices;
                command(() => dsp.release(ids[slot], 0.02));
                ids[slot] = nextId++;
                attack(ids[slot], slot);
              }
            }
            if (workload === "controls") {
              for (let j = 0; j < Math.min(voices, 96); j++) {
                command(() =>
                  dsp.submit(
                    JSON.stringify({
                      version: 2,
                      id: String(ids[j]),
                      op: {
                        kind: "Control",
                        name: "SetPan",
                        args: [Math.sin(block * 0.1 + j)],
                        position: null,
                        cone: null,
                        spatial: null,
                      },
                    }),
                  ),
                );
              }
            }
            const beforeRender = performance.now();
            const output = render();
            const afterRender = performance.now();
            dsp.collect_garbage();
            const end = performance.now();
            renderTimes.push(afterRender - beforeRender);
            controlTimes.push(beforeRender - start);
            cleanupTimes.push(end - afterRender);
            cycleTimes.push(end - start);
            // Verification is outside the measured cycle; never benchmark silence unnoticed.
            for (const sample of output) {
              if (!Number.isFinite(sample)) nonFinite++;
              else energy += sample * sample;
            }
            if (block % 16 === 0)
              peakPlaying = Math.max(peakPlaying, dsp.playing_ids().length);
          }
          const budgetMs = (frames / rate) * 1000;
          rounds.push({
            round,
            initialPlaying,
            peakPlaying,
            finalPlaying: dsp.playing_ids().length,
            rejected,
            rms: Math.sqrt(energy / (blocks * frames * 2)),
            nonFinite,
            render: summarize(renderTimes, budgetMs),
            controls: summarize(controlTimes, budgetMs),
            cleanup: summarize(cleanupTimes, budgetMs),
            cycle: summarize(cycleTimes, budgetMs),
          });
        } finally {
          dsp.free();
        }
      }
      results.push({
        workload,
        requestedVoices: voices,
        sampleRate: rate,
        blockBudgetMs: (frames / rate) * 1000,
        rounds,
      });
      console.error(
        `${workload} ${voices} voices @ ${rate}: worst cycle p99 ${Math.max(...rounds.map((r) => r.cycle.p99Ms)).toFixed(3)}ms; initial playing ${rounds.map((r) => r.initialPlaying).join("/")}`,
      );
    }
const invalid = results.some((r) =>
  r.rounds.some(
    (round) => round.nonFinite || round.rms === 0 || round.initialPlaying === 0,
  ),
);
console.log(
  JSON.stringify(
    {
      schemaVersion: 2,
      kind: "in-process-wasm-dsp",
      outputMode:
        !ownedOutput && typeof DspWorker.prototype.render_buffer === "function"
          ? "borrowed-wasm-buffer"
          : "owned-copy",
      directory,
      wasmBytes: bytes.byteLength,
      sha256: createHash("sha256").update(bytes).digest("hex"),
      runtime: process.version,
      platform: platform(),
      arch: arch(),
      cpu: cpus()[0]?.model,
      configuration: {
        frames,
        warmup,
        blocks,
        repetitions,
        focus,
        sharedDelayMix: 0.12,
        sharedReverbMix: 0.2,
      },
      valid: !invalid,
      limitations: [
        "Excludes worker messaging, AudioWorklet and OS scheduling.",
        "Over-budget CPU blocks are not measured audio underruns.",
        "128 requested voices tests admission/stealing beyond the 96-voice budget.",
        "Compare matching hardware, artifact hashes/build settings and workload configurations.",
      ],
      results,
    },
    null,
    2,
  ),
);
if (invalid) process.exitCode = 1;
