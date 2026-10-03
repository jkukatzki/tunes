# Tunes browser DSP

A separate Rust/WASM DSP module and AudioWorklet output for the Tunes audio
library. No Bevy, Svelte or shared-memory toolchain is required.

Build from the repository root with:

```sh
bash web-dsp/scripts/package.sh release
```

Builds enable WASM SIMD by default and require a compatible browser (Safari/iOS
16.4+). Use `WASM_SIMD=0 bash web-dsp/scripts/package.sh release` for a build
without enabling SIMD.

The resulting `web-dsp/dist` directory is a complete browser distribution. Serve
it over HTTPS, import `installWorkerAudio` from `bridge.js`, and await it before
starting the application's WASM with `AudioEngine::with_worker_output()`.
`bridge.d.ts` describes the supported host API. `index.html` is an optional
standalone diagnostic harness.

See [browser setup](https://github.com/jkukatzki/tunes/blob/pushas-tunes-compat/docs/browser_audio.md)
for prerequisites, activation, lifecycle, caching and limitations. The private
protocol is versioned; rebuild both WASM modules together. Browser assets and
application code must use the same Tunes revision.

Licensed MIT OR Apache-2.0; see the included license files. This companion crate
is not published separately to crates.io.

## Stress benchmarks

```sh
node web-dsp/scripts/benchmark.mjs web-dsp/dist --quick > quick.json
node web-dsp/scripts/benchmark.mjs web-dsp/dist > full.json
```

The full suite tests 1/12/48/96/128 requested voices at 44.1/48 kHz across sustained
synthesis, samples, mixed sources, rapid attack/release churn, and parameter floods.
Each case runs three fresh instances with warmup. Shared delay/reverb remains
configured in every case. It reports actual playing counts, command rejections,
output RMS/nonfinite checks, and p50/p95/p99/max timings for render, controls,
cleanup and their combined cycle, with deadline exceedances. The artifact hash,
CPU, runtime and configuration are recorded for repeatable comparisons.

128 requested voices deliberately exceeds the engine's 96-voice admission limit;
it does not prove 128 simultaneous normal voices. CPU deadline exceedances are
not AudioWorklet underruns: use the HTTPS harness on your phone to measure real
scheduling. Validation and playing-status polling occur outside timed cycles.
The suite measures the existing WASM artifact and never builds it implicitly.


For focused optimization comparisons, use `--focus` (96 voices, both sample rates,
three repetitions) and alternate baseline/candidate runs on the same idle machine.
`node web-dsp/scripts/compare-output.mjs <baseline-dir> <candidate-dir>` checks
rendered sample parity across synthesis, controls, voice overload and PCM cases.
The [optimization report](benchmarks/2026-10-03-optimization.json) records fresh
O3 builds with and without the hot-path changes, plus a separate SIMD experiment.

The runner uses the reusable WASM output view when supported, matching the browser
worker. `--owned-output` forces the older copying API for a same-artifact comparison.
Views are refreshed after WASM memory growth. The AudioWorklet reads the DSP
output directly; no transferable PCM pool is involved.

### Optional performance reports

Pass an `onHealth` callback to opt in to worklet health and timing reports, as
used by the standalone harness. Normal game playback does not collect timings
or print periodic diagnostics. Reports expose CPU callback overruns; hardware
playback dropouts cannot be measured by these counters.

### Direct audio-thread output

`installWorkerAudio()` defaults to direct 128-frame synthesis
inside the AudioWorklet. The distribution now includes a generated
`dsp-worklet.js`; rebuild the DSP module when updating it. There is no dedicated-worker fallback. See `docs/browser_audio.md` for limitations and diagnostics.
Run `node web-dsp/scripts/benchmark-worklet.mjs web-dsp/dist` to verify real-WASM
PCM parity and desktop CPU time. This does not measure phone audio scheduling.

### Incremental maintenance and interleaved mixing

Direct AudioWorklet output retires at most four voices and four effect buses per
callback instead of destroying every completed object at once. The separate
budgets prevent starvation. Offline renderers can still drain all retired objects between jobs.
This bounds destructor count, not execution time: an individual composition or
last sample reference can still be expensive to free. Voice construction, PCM
registration, and session reset also remain potential audio-thread spikes.

The engine mixes ordinary stereo voices directly in interleaved SIMD lanes,
without splitting and rejoining channels. Multiplication order is preserved;
fading and resampling retain their existing paths.

To compare against a previously packaged DSP (including PCM equivalence):

```sh
node web-dsp/scripts/benchmark-worklet.mjs /path/to/new-dist --churn --reference-dir /path/to/old-dist
node web-dsp/scripts/benchmark-retirement.mjs /path/to/new-dist
```

These are Node CPU experiments, not measurements of browser scheduling or iPhone
latency. The retirement benchmark isolates chord stops; it excludes command
construction and measures silent output after stopping. The worklet benchmark
checks active synthesis against the reference WASM.

Recorded [development-Mac results](benchmarks/2026-10-03-interleaved-maintenance.json)
use three alternating runs per mixer implementation. At 48 kHz, median mean
callback time improved approximately 4% / 7% / 9% for 12 / 48 / 96 voices with
note churn; all compared PCM samples were identical. Absolute savings are small,
and these results do not establish iPhone improvement. The cleanup comparison
moves work into subsequent callbacks rather than eliminating it.
