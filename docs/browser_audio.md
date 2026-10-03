# Browser audio

This fork supports two browser outputs:

| Feature / constructor | DSP execution | Host setup |
| --- | --- | --- |
| `web` / `AudioEngine::with_buffer_size` | Browser main thread (CPAL) | Normal wasm-bindgen application |
| `worker` / `AudioEngine::with_worker_output` | Separate DSP WASM in a dedicated worker | Install the JavaScript bridge first |

Native applications continue to use `AudioEngine::new()` or `with_buffer_size()`.
The worker backend has no dependency on Bevy, Svelte, or a particular game.

## Install this fork

```toml
[dependencies]
tunes = { git = "https://github.com/jkukatzki/tunes", branch = "pushas-tunes-compat", features = ["worker"] }
```

Pin a tested Git revision for production and build the DSP module from that same
checkout. Upstream crates.io `tunes` does not contain this fork's changes.
See [release notes](releasing.md) before considering registry publication.

## Package the DSP module

Install the Rust `wasm32-unknown-unknown` target, Node.js 22+, rsync, Binaryen
(`wasm-opt`), and `wasm-bindgen-cli` matching the `wasm-bindgen` version in
`web-dsp/Cargo.lock`. Then, from a checkout of this repository:

```sh
rustup target add wasm32-unknown-unknown
bash web-dsp/scripts/package.sh release
```

This builds the separate DSP crate and creates `web-dsp/dist/`. Copy the whole
directory to your web server, for example `/audio/`. The optional second argument
selects an output directory. Existing nonempty directories are accepted only if
they contain the distribution's `.tunes-audio-dist` marker, to avoid deleting
unrelated files. Use `dev` instead of `release` to skip wasm-opt.

Rust and wasm-opt default to speed-oriented optimization (`3` / `-O3`), even if
that increases download size. `WASM_OPT` selects a Binaryen executable;
`WASM_OPT_LEVEL` accepts `-O2`, `-O3`, `-Os`, or `-Oz`. `WASM_SIMD=1` explicitly
opts into SIMD and requires a compatible browser. Do not infer SIMD support from
the library's internal lane dispatcher. Measure performance on your devices.

## Start from a user gesture

Serve over HTTPS (or localhost). AudioWorklet requires a secure context. Serve
JavaScript with a JavaScript MIME type and WASM as `application/wasm`. The worker
uses its own memory; cross-origin isolation and SharedArrayBuffer are not needed.

```js
// Run this handler once. Disable the Play button while startup is in progress.
playButton.addEventListener('click', async () => {
  const context = new AudioContext({ latencyHint: 'interactive' });
  // Start activation synchronously, before awaiting imports/downloads.
  void context.resume().catch(() => {});
  const { installWorkerAudio } = await import('/audio/bridge.js');
  const audio = await installWorkerAudio({ context, onHealth: console.log });
  try {
    const { default: startApplication } = await import('/app.js');
    await startApplication();
  } catch (error) {
    await audio.close();
    throw error;
  }
}, { once: true });
```

In your application's Rust initialization:

```rust,ignore
let engine = tunes::engine::AudioEngine::with_worker_output()?;
// Use the normal play_track, play_sample, play_mixer and playback controls.
```

Keep the engine alive for playback. Only one installed bridge/active worker
engine is supported per page. Replacing the engine resets its session and voices;
old handles and queued PCM cannot control the replacement. `audio.close()` releases
the worker, DOM listeners, global bindings and the AudioContext, including a
context supplied by your application. It is safe to close twice. The older
`installGameAudio` export remains an alias for compatibility.

## Lifecycle, capabilities and limits

The bridge suspends audio when hidden or on `pagehide`, releases the playback
session on browsers supporting Audio Session, and resumes on foreground return.
Pointer, touch and keyboard gestures retry activation if the browser requires it.
Foreground playback uses the `playback` session where available for iPhone Silent
Mode support. This is browser-dependent. Fatal worker/processor errors are logged
and reported through `onHealth`; reload to recover.

The protocol supports DSP tracks, mono/stereo sample metadata and loop points,
standard mixer effects and sidechains, shared delay/reverb buses, spatial controls,
volume/pan/rate automation, fades, pause/resume and stop. Spectral/convolution
effects and native file streams are explicitly unsupported. The private JSON
wire format is not a general RPC API; version and package both modules together.

DSP output uses four transferable 512-frame stereo blocks (about 46 ms buffered
at 44.1 kHz, plus device latency). Buffer size is fixed. Commands are bounded and
coalesced with reserved release capacity and an emergency stop under overload.
The renderer admits 96 voices with eight additional stealing fades. The game PCM
cache is bounded at 128 allocations / 128 MiB; registration/admission may fail and
should be handled. Arbitrarily complex graphs, decoding and uploads can still
starve the worker. This is not a hard realtime guarantee.

`recover_web_output_on_foreground()` should be called from your frame loop if you
use a monitor callback: it delivers optional worker snapshots every four rendered
blocks, subject to backpressure. These are visualisation samples, not continuous
recording. Background lifecycle handling itself is in the JavaScript bridge.

Keep the entire distribution and application WASM on the same release. The bridge
propagates its URL query to sibling modules for versioned caches. Precache all
files if offline operation is required. See the application's service-worker
policy rather than assuming browser caches update every sibling automatically.

## Verify on a device

`web-dsp/dist/index.html` is an optional diagnostic harness. Serve it through your
existing HTTPS development server (including Vite); opening it as `file://` will
not work. For a desktop-only local check, you can run
`python3 -m http.server 8080 --bind 127.0.0.1 --directory web-dsp/dist` and open
`http://localhost:8080`; phone testing requires your HTTPS server. Test sustained chords, main-thread stalls, background/foreground,
sample loading, overload and user-gesture recovery. Monitor underruns and command
rejections. The harness has passed an iPhone stall/resume check; this is not a
cross-browser compatibility certification.
