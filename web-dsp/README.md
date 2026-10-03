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
Views are refreshed after WASM memory growth; the worker still copies into its
bounded transferable PCM pool before sending audio to the AudioWorklet.

### Console diagnostics

Pass `diagnostics: true` to `installWorkerAudio` to log `[tunes] Audio diagnostics`
about every five seconds while output health reports arrive. Reports include
interval underrun duration/percentage, output queue depth, audio-clock progress,
and worker render, callback (including cleanup), and command-batch timings.
`maxRenderGapMs` measures time between worker render starts: it includes ordinary
buffer pacing, suspension, and message delivery delays, not just DSP work.
Worker timing windows arrive separately from output health windows; their bounds
are approximate. `timingReports: 0` means no fresh worker timing report arrived,
not that DSP took zero time. Diagnostics do not log individual audio callbacks.
