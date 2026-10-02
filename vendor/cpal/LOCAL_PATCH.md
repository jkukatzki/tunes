Vendored from crates.io CPAL 0.15.3 (Apache-2.0; see LICENSE).

Only the WebAudio backend is changed. Its two workers previously kept advancing
an expired playback timestamp after an underrun, scheduling every subsequent
buffer in the past. Observed on iOS Safari: about 16 seconds behind, with only
1–3 ms of callback work per 23.2 ms buffer.

`src/host/webaudio/scheduling.rs` resets an expired cursor to currentTime plus
the backend's existing 25 ms startup headroom. The backend checks both before
rendering and immediately before scheduling, then advances the shared cursor
from the actual start. Future timestamps are preserved so healthy playback
stays contiguous. Native backends are unchanged.

The WebAudio stream also exposes a foreground recovery handle. It retains each
worker's current source and expected end time. If the clock passes that end by
100 ms without the worker receiving `ended`, the handle detaches/stops the old
source and invokes that worker once. Tunes calls this only while visible and
the context is running. This addresses a separate possible failure: a moving
AudioContext clock with no buffer production after returning from background.
That device-specific failure remains to be confirmed by the health logs.

Ended callbacks verify the event target is still the worker's current source.
Queued events from replaced sources and duplicate startup timers are ignored,
preventing recovery from creating extra callback chains. Completed sources are
disconnected. The handle reports rendered-buffer counts for foreground logs.

Stream destruction cancels startup timers, detaches/stops active sources and
breaks self-references in worker closures before closing the context. Callbacks
also refuse to render into a closed context. This permits Tunes to replace a
browser output stream on foreground return without leaving old workers consuming
the shared engine command queue or retaining its state.

The initial cause of the observed backlog is not established. This patch
recovers scheduling; it cannot eliminate gaps caused by sustained overload.

The scheduling regression tests can run without a browser or audio device:

    rustc --edition 2021 --test src/host/webaudio/scheduling.rs -o /tmp/cpal-scheduling-tests
    /tmp/cpal-scheduling-tests

When using Tunes inside another workspace, patch that workspace's crates.io
CPAL dependency to this same directory to keep other CPAL users on one source.
