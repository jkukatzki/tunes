// CPU/PCM verification of the generated worklet with real DSP WASM. This is not
// a browser scheduling or hardware-latency benchmark.
import { readFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { pathToFileURL } from "node:url";
import { createHash, webcrypto } from "node:crypto";
import { performance } from "node:perf_hooks";
import vm from "node:vm";
const directory = resolve(process.argv[2] ?? "web-dsp/dist");
const bytes = readFileSync(join(directory, "tunes_web_dsp_bg.wasm"));
const module = await WebAssembly.compile(bytes);
const referenceArg = process.argv.indexOf('--reference-dir');
const referenceDirectory = referenceArg < 0 ? directory : resolve(process.argv[referenceArg + 1]);
const referenceBytes = readFileSync(join(referenceDirectory, 'tunes_web_dsp_bg.wasm'));
const referenceModule = await WebAssembly.compile(referenceBytes);
const script = readFileSync(
  join(directory, "dsp-worklet.js"),
  "utf8",
).replaceAll(
  "import.meta.url",
  JSON.stringify(pathToFileURL(join(directory, "dsp-worklet.js")).href),
);
const results = [];
const churn = process.argv.includes("--churn");
for (const rate of [44100, 48000])
  for (const voices of [1, 12, 48, 96]) {
    let Processor;
    const messages = [];
    const sandbox = {
      WebAssembly,
      Float32Array,
      Uint8Array,
      Uint32Array,
      Int32Array,
      ArrayBuffer,
      console,
      performance,
      sampleRate: rate,
      AudioWorkletProcessor: class {
        constructor() {
          this.port = { postMessage: (p) => messages.push(p) };
        }
      },
      registerProcessor: (_name, value) => {
        Processor = value;
      },
    };
    vm.runInNewContext(script, sandbox); // Deliberately omit Encoding API globals.
    const processor = new Processor();
    const send = (data) => processor.port.onmessage({ data });
    send({
      type: "init",
      telemetry: true,
      module,
      entropy: webcrypto.getRandomValues(new Uint8Array(65536)),
    });
    if (!messages.some((p) => p.type === "ready"))
      throw new Error(JSON.stringify(messages));
    // Separate bindings/instance provide a scalar protocol reference at the same quantum.
    const bindings = await import(
      pathToFileURL(join(referenceDirectory, "tunes_web_dsp.js")).href +
        `?case=${rate}-${voices}`
    );
    const wasm = bindings.initSync({ module: referenceModule });
    const reference = new bindings.DspWorker(rate);
    reference.set_block_frames(128);
    reference.effects(0.12, 0.2);
    let sequence = 0;
    let batch = [
      { kind: "reset", session: 1, seq: ++sequence },
      {
        kind: "legacy",
        session: 1,
        seq: ++sequence,
        method: "effects",
        args: [0.12, 0.2],
      },
    ];
    for (let i = 0; i < voices; i++) {
      const args = [
        i + 1,
        110 * 2 ** ((i % 36) / 12),
        2,
        0.08,
        0,
        0.01,
        0.2,
        0.7,
        0.5,
      ];
      reference.note(...args);
      batch.push({
        kind: "legacy",
        session: 1,
        seq: ++sequence,
        method: "note",
        args,
      });
      if (batch.length === 64) {
        send({ type: "batch", packets: batch });
        while (processor.pending) processor.commands();
        batch = [];
      }
    }
    if (batch.length) {
      send({ type: "batch", packets: batch });
      while (processor.pending) processor.commands();
    }
    const output = [new Float32Array(128), new Float32Array(128)];
    const times = [];
    const activeIds = Array.from({ length: voices }, (_, i) => i + 1);
    let nextId = voices + 1;
    let maxError = 0,
      energy = 0;
    for (let i = 0; i < 2200; i++) {
      if (churn && i % 16 === 0) {
        const slot = Math.floor(i / 16) % voices;
        const oldId = activeIds[slot],
          id = nextId++;
        activeIds[slot] = id;
        const args = [
          id,
          110 * 2 ** ((slot % 36) / 12),
          2,
          0.08,
          0,
          0.01,
          0.2,
          0.7,
          0.5,
        ];
        reference.release(oldId, 0.004);
        reference.note(...args);
        send({
          type: "batch",
          packets: [
            {
              kind: "legacy",
              session: 1,
              seq: ++sequence,
              method: "release",
              args: [oldId, 0.004],
            },
            {
              kind: "legacy",
              session: 1,
              seq: ++sequence,
              method: "note",
              args,
            },
          ],
        });
      }
      const start = performance.now();
      if (!processor.process([], [output]))
        throw new Error(JSON.stringify(messages.slice(-3)));
      const elapsed = performance.now() - start;
      if (i >= 200) times.push(elapsed);
      const pointer = reference.render_buffer();
      const expected = new Float32Array(wasm.memory.buffer, pointer, 256);
      for (let frame = 0; frame < 128; frame++)
        for (let channel = 0; channel < 2; channel++) {
          const sample = output[channel][frame];
          if (!Number.isFinite(sample)) throw new Error("Non-finite PCM");
          maxError = Math.max(
            maxError,
            Math.abs(sample - expected[frame * 2 + channel]),
          );
          energy += sample * sample;
        }
      reference.collect_garbage();
      if (messages.some((p) => p.type === "fatal"))
        throw new Error(JSON.stringify(messages));
      messages.length = 0;
      send({ type: "health-ack" });
      send({ type: "timing-ack" });
      send({ type: "status-ack" });
    }
    if (maxError > 0.00002 || energy === 0)
      throw new Error(`PCM mismatch: ${maxError}, energy=${energy}`);
    times.sort((a, b) => a - b);
    const budget = (128 / rate) * 1000;
    results.push({
      rate,
      voices,
      workload: churn ? "churn" : "sustained",
      quantum: 128,
      meanMs: times.reduce((a, b) => a + b, 0) / times.length,
      p99Ms: times[Math.floor(times.length * 0.99)],
      maxMs: times.at(-1),
      budgetMs: budget,
      cpuOverruns: times.filter((v) => v > budget).length,
      maxPcmError: maxError,
    });
    send({ type: "close" });
    reference.free();
  }
console.log(
  JSON.stringify(
    {
      runtime: process.version,
      referenceWasmSha256: createHash("sha256").update(referenceBytes).digest("hex"),
      wasmSha256: createHash("sha256").update(bytes).digest("hex"),
      note: "Node CPU simulation; not iPhone audio deadlines",
      results,
    },
    null,
    2,
  ),
);
