// Isolate render + maintenance on simultaneous voice retirement. Node CPU only;
// command construction, browser scheduling, and audio hardware are not measured.
import { readFileSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { performance } from 'node:perf_hooks';
import { createHash } from 'node:crypto';
const directory = resolve(process.argv[2]);
const bytes = readFileSync(join(directory, 'tunes_web_dsp_bg.wasm'));
const module = await WebAssembly.compile(bytes);
const results = [];
for (const voices of [12, 48, 96]) {
  const hashes = [];
  for (const mode of ['unbounded', 'incremental']) {
    const bindings = await import(pathToFileURL(join(directory, 'tunes_web_dsp.js')).href + `?${voices}-${mode}`);
    const wasm = bindings.initSync({ module });
    const dsp = new bindings.DspWorker(48000);
    dsp.set_block_frames(128);
    const hash = createHash('sha256');
    const stopTimes = [], followupTimes = [];
    let maxRetired = 0;
    for (let cycle = 0; cycle < 220; cycle++) {
      for (let id = 1; id <= voices; id++) {
        if (!dsp.note(id, 110 * 2 ** ((id % 36) / 12), 2, .08, 0, .01, .2, .7, .5))
          throw new Error('Rejected note');
        // Dispatch bounded command queue before queuing more notes.
        if (id % 32 === 0) dsp.render_buffer();
      }
      dsp.render_buffer();
      if (!dsp.stop_all()) throw new Error('Rejected stop');
      for (let quantum = 0; quantum < 32; quantum++) {
        const start = performance.now();
        const pointer = dsp.render_buffer();
        let retired;
        if (mode === 'incremental') retired = dsp.collect_garbage_budget(4);
        else dsp.collect_garbage();
        const elapsed = performance.now() - start;
        if (retired !== undefined) {
          if (retired > 8) throw new Error('Maintenance budget exceeded');
          maxRetired = Math.max(maxRetired, retired);
        }
        hash.update(new Uint8Array(wasm.memory.buffer, pointer, 256 * 4));
        if (cycle >= 20) (quantum === 0 ? stopTimes : followupTimes).push(elapsed);
      }
    }
    const stats = (values) => {
      const sorted = values.toSorted((a,b) => a-b);
      return {meanMs: values.reduce((a,b) => a+b,0)/values.length,
        p95Ms: sorted[Math.floor(sorted.length*.95)], maxMs: sorted.at(-1)};
    };
    const pcmSha256 = hash.digest('hex');
    hashes.push(pcmSha256);
    results.push({ voices, mode, stop: stats(stopTimes), followup: stats(followupTimes), maxRetired, pcmSha256 });
    dsp.free();
  }
  if (hashes[0] !== hashes[1]) throw new Error('Retirement changes PCM');
}
console.log(JSON.stringify({wasmSha256:createHash('sha256').update(bytes).digest('hex'),
  note:'Node WASM CPU timing, 200 measured chord stops per case. No iPhone performance claim.', results},null,2));
