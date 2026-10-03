# Distribution and release checklist

This is the public Git fork of upstream `sqrew/tunes`, retaining upstream's
MIT/Apache-2.0 licensing and attribution. The package name/version identify its
upstream base; they do not claim a new crates.io release. `publish = false`
prevents accidentally publishing the fork as upstream's `tunes` package.

The active dependency is crates.io CPAL 0.15.3. The historical patched copy under
`vendor/cpal` is retained with its Apache-2.0 license and `LOCAL_PATCH.md`, but is
not used by the manifest. CPAL browser output retains AudioContext lifecycle
handling without the patched buffer-chain recovery hook. Worker and direct
AudioWorklet output manage their own lifecycle independently.
A separately named registry release still needs a deliberate package name and
version. Do not remove the publication guard without deciding that identity.
The `web-dsp` companion is also a non-published application crate, distributed as
browser assets. Its lockfile is committed for reproducible tooling versions.

Before tagging a Git release:

1. Review changes and update the changelog, especially compatibility and worker
   protocol changes. Preserve both root licenses and the vendored CPAL license.
2. Run native checks, worker-feature tests, WASM checks and browser transport
   tests (the CI workflow supplies the commands). Check the optional `gpu` feature
   separately if the release changes GPU code.
3. Build the browser distribution using `web-dsp/scripts/package.sh`; use matching
   wasm-bindgen CLI and crate versions. Build the consuming application from the
   same Tunes revision. No game repository is needed to package the worker.
4. Test a native consumer and browser/device playback: instruments, samples,
   sidechains, overload, hide/resume and worker startup failure. Record tested
   devices rather than claiming unmeasured speedups or universal support.
5. Deploy/version application WASM and the complete DSP distribution together.
   Update service-worker inventories. Tag the reviewed revision only when these
   runtime checks pass.

CI checks do not publish packages or create releases automatically. A commit/push
is not a crates.io publication or a device-validation result.
