# Distribution and release checklist

This is the public Git fork of upstream `sqrew/tunes`, retaining upstream's
MIT/Apache-2.0 licensing and attribution. The package name/version identify its
upstream base; they do not claim a new crates.io release. `publish = false`
prevents accidentally publishing the fork as upstream's `tunes` package.

The repository includes a locally patched CPAL 0.15.3 under `vendor/cpal` with
its Apache-2.0 license and `LOCAL_PATCH.md`. Git dependencies preserve that path.
Cargo registry packaging normalizes path dependencies to registry dependencies;
publishing the current manifest would lose required WebAudio methods and fixes.
A separately named registry release therefore needs both a deliberate package
name/version and a published patched backend (or removal/upstreaming of those
backend changes). Do not remove the publication guard without addressing this.
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
