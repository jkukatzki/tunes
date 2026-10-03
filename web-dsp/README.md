# Tunes browser DSP

A separate Rust/WASM DSP module and AudioWorklet output for the Tunes audio
library. No Bevy, Svelte or shared-memory toolchain is required.

Build from the repository root with:

```sh
bash web-dsp/scripts/package.sh release
```

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
