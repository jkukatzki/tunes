# Browser audio

The standalone browser bridge uses AudioWorklet only. A legacy main-thread CPAL
constructor remains available to direct library consumers; it is not an automatic fallback:

| Feature / constructor | DSP execution | Host setup |
| --- | --- | --- |
| `web` / `AudioEngine::with_buffer_size` | Browser main thread (CPAL) | Normal wasm-bindgen application |
| `worker` / `AudioEngine::with_worker_output` | Separate DSP WASM on the AudioWorklet thread | Install the JavaScript bridge first |

Native applications continue to use `AudioEngine::new()` or `with_buffer_size()`.
The AudioWorklet backend has no dependency on Bevy, Svelte, or a particular game.

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
`WASM_OPT_LEVEL` accepts `-O2`, `-O3`, `-Os`, or `-Oz`. SIMD is enabled by
default and requires a compatible browser (Safari/iOS 16.4+). Set `WASM_SIMD=0`
to build without enabling SIMD. Do not infer SIMD support from
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
old handles and queued commands cannot control the replacement. `audio.close()` releases
the worker, DOM listeners, global bindings and the AudioContext, including a
context supplied by your application. It is safe to close twice. The older
`installGameAudio` export remains an alias for compatibility.

## Lifecycle, capabilities and limits

The bridge suspends audio when hidden or on `pagehide`, releases the playback
session on browsers supporting Audio Session, and resumes on foreground return.
Pointer, touch and keyboard gestures retry activation if the browser requires it.
Foreground playback uses the `playback` session where available for iPhone Silent
Mode support. This is browser-dependent. Fatal processor errors are logged
and reported through `onHealth`; reload to recover.

The protocol supports DSP tracks, mono/stereo sample metadata and loop points,
standard mixer effects and sidechains, shared delay/reverb buses, spatial controls,
volume/pan/rate automation, fades, pause/resume and stop. Spectral/convolution
effects and native file streams are explicitly unsupported. The private JSON
wire format is not a general RPC API; version and package both modules together.

DSP runs directly on the AudioWorklet thread in 128-frame blocks. There is no
transferable PCM output queue or browser buffer-size setting. Commands are bounded
and coalesced with reserved release capacity and an emergency stop under overload.
The renderer admits 96 voices with eight additional stealing fades. The game PCM
cache is bounded at 128 allocations / 128 MiB; registration/admission may fail and
should be handled. Arbitrarily complex graphs, decoding and uploads can still
miss audio-thread deadlines. This is not a hard realtime guarantee.

`recover_web_output_on_foreground()` should be called from your frame loop if you
use a monitor callback: it delivers optional audio snapshots at roughly 20 Hz, subject to backpressure. These are visualisation samples, not continuous
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

## Direct DSP on the audio thread (default)

`installWorkerAudio()` runs the same
DSP WASM directly inside an AudioWorklet. The host compiles the module before
installation; the worklet instantiates it synchronously. Packaging generates a
single `dsp-worklet.js` containing the binding glue, UTF-8 support and processor,
so dependencies share the distribution's release URL. No shared memory or
cross-origin isolation is required.

This mode renders 128 frames on demand. Browser settings expose latency/power
preferences; device-buffer presets are native-only. It removes the worker scheduling and PCM-transfer round
trip, but does not guarantee glitch-free playback: the DSP now has to meet the
audio thread's deadline. Four commands at most are dispatched per quantum, in
order, with one bounded batch in flight. Large PCM registrations, effect changes,
WASM memory growth and cleanup can still cause expensive work on that thread.
Main-thread stalls can still delay new user input or commands.

The generated binding uses bounded Web Crypto entropy supplied by the host for
Rust RNG seeding; it never substitutes predictable randomness. The host refills
this small pool on request. Status messages are sampled, not per-quantum PCM
transfers. An optional `onHealth` callback enables health/timing reports.
`callbackOverruns` measures CPU time exceeding a quantum's budget, not hardware
dropouts. Timer resolution varies. Normal playback skips these measurements.

Direct output is the only browser backend. The former dedicated-worker fallback
and adaptive buffering controls have been removed. The historical `worker` Cargo
feature and `installWorkerAudio` API names remain for compatibility.
Foreground recovery checks audio-clock progress, retries once, and permits a
user-gesture retry if still stalled. Hide/resume promise races are reconciled
without replacing the DSP or losing loaded samples. Device verification of this
recovery remains necessary. Use `node web-dsp/scripts/benchmark-worklet.mjs
web-dsp/dist` for CPU and PCM-parity verification against the same DSP renderer.
The desktop report covers 32,000 measured callbacks (sustained and repeated attacks/releases,
1/12/48/96 voices, 44.1/48 kHz), with zero CPU-budget overruns and bit-identical output for 9,011,200
compared samples. It does not measure Safari scheduling or physical output latency.

## Changing latency preference without reloading

The installed bridge exposes `setLatencyHint("interactive" | "balanced" | "playback")`.
Changes are debounced, then a new context and muted DSP are prepared. Registered
PCM, buses, listener controls, monitoring, and active sample playback descriptions are
replayed before output switches. Concurrent commands are retained in a bounded
backlog. Replay uses batches of at most 32 commands and a finite handoff point,
so continuous listener updates cannot prevent completion. The whole attempt has
a 15-second deadline; context closure does not delay the preference status.
On failure, old audio remains available; `preferenceStatus()` reports
the error and requested/applied preference. This cannot detect whether a browser
honors the hint; `bufferStatus()` exposes reported base/output latency instead.

Synthesized voices and their per-voice controls are not replayed, including notes
played during preparation. Direct samples and sample-only tracks/mixers are
restored automatically; mixed sample/synth graphs are dropped. No music flag is
needed. Sample playback positions, pause state and rate/fade timelines transfer
from the old DSP at handoff. The replacement is held during preparation so samples
cannot finish silently. A brief handoff gap is possible; effect tails reset. This is
not sample-exact state migration. PCM replay retains up to 128 MiB of host data
in addition to DSP storage, and preparing the replacement temporarily uses two
DSP instances. Sound descriptions and the change backlog are separately bounded
to 128 MiB, with at most 512 commands in the backlog. Registration still occurs
on the new audio thread; the old context remains the output until restoration
finishes. No dedicated-worker output backend is involved.

The raw `createWorkerAudio` diagnostic harness does not install this restart
controller. Use `installWorkerAudio` for application ownership and live preferences.

## Reusable browser host package

The Tunes checkout also provides the npm package `@tunes/browser`. It owns
user-gesture audio unlock, foreground media-session lifetime, latency preference
parsing, one-time WASM initialization and shared build commands. The frontend and
u_moni Svelte wrappers use the same host package and the same DSP distribution.
See [the package guide](../browser/README.md) for API and packaging commands.
App-specific release caches, settings keys and UI stay in the consuming app.
