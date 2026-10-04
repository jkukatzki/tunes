# @tunes/browser

Framework-independent host utilities and build tools maintained with the Tunes
crate. Both the frontend and u_moni Svelte wrappers consume this package through
`"@tunes/browser": "file:../../tunes"`. Importing the package is SSR-safe.

```js
import { createAudioHost, createWasmInitializer, downloadWasm } from '@tunes/browser';
const once = createWasmInitializer();
let host;
function play() {
  // Call before the first await, inside the user's click handler.
  host ??= createAudioHost({ latencyHint: 'interactive' });
  return once(async () => {
    try {
      const wasm = await downloadWasm('/app/app_bg.wasm', console.log);
      const bindings = await import('/app/app.js');
      await host.install('/app/audio/bridge.js');
      await bindings.default({ module_or_path: wasm });
    } catch (error) {
      await host.close().catch(console.warn);
      throw error;
    }
  });
}
```

`readLatencyHint({storageKey, path})` reads a host-owned JSON settings schema.
The package does not choose storage keys, add a service worker, manage app
releases, or assume a Svelte component layout. Pass versioned URLs to downloads
and bridge installation when the application uses a release cache. The bridge
propagates its query to all DSP modules. One host owns audio per page; close it
when the app is disposed. Failed WASM initialization requires a page reload.

`host.install()` returns the existing Tunes bridge interface, including
`setLatencyHint`, `preferenceStatus` and `bufferStatus`. Foreground media priority
is acquired before download and released on hide or close. Tunes owns subsequent
AudioContext lifecycle and sample-position restoration.

## Build tools

```sh
tunes-build-audio <output-directory> [release|dev]
tunes-build-wasm <Cargo.toml> <package> <binary> <output> <assets> [release|dev|wasm-release]
```

The WASM tool stages app JS/WASM and DSP before copying them into output. App
assets are copied into the adjacent `assets` directory. It uses Cargo metadata
for the target directory, supports `CARGO_TARGET_DIR`, enables SIMD by default,
and runs Binaryen `-O3` for release profiles. `WASM_SIMD=0`, `WASM_OPT`, and
`WASM_OPT_LEVEL` are supported. The installed `wasm-bindgen` CLI must match both
application and DSP Cargo lockfiles. These commands explicitly compile Rust;
`npm install` does not.

The npm distribution includes Rust/DSP sources needed by these build tools.
No app binary, prebuilt WASM, certificate or private key is bundled. A regular
Node/Svelte build imports only the lightweight host module. To build from a
checkout use sibling workspace dependencies; `npm pack --dry-run` reviews the
publishable archive without publishing it.

Run `npm test` from the Tunes root for host, command transport, restart and
worklet tests. Full browser playback still needs a packaged app and device check.
