Review of the moni / pushedpeople piano playback path, September 2026.

The reported static has several plausible causes in the code. No device-level
underrun measurement or listening comparison was performed; the changes below
address reproducible lifecycle and rendering defects.

Changes in tunes:

- Fades start from the current gain. Shortening a partially completed fade no
  longer jumps back to its original volume. Repeated fade-outs cannot extend an
  earlier completion deadline.
- Fade-outs retire the sound when they finish, including looping sounds. They
  previously left silent synthesis running until the composition ended. Fades
  and control tweens now use output time, independent of source playback speed
  and looping. Pausing freezes that control clock.
- Sound storage is dense and reused. Its size follows concurrent playback,
  rather than the largest SoundId ever issued. Command lookups are linear in
  current polyphony; mixing no longer scans historical IDs.
- Engine-owned mixers prepare track buffers and event ordering before entering
  the command queue. Live rendering runs sequentially without Rayon dispatch or
  per-block bus-result vectors. Offline rendering keeps its parallel path.
- Live rendering does not synthesize entire notes on a synthesis-cache miss.
  Explicit prerendering can still populate that cache ahead of playback.
- Track sample buffers and drum bookkeeping are reused. Empty stereo effect
  chains and chains containing only linked dynamics process in place; other
  chains reuse channel scratch buffers.
- A linked peak limiter runs after all sounds and streams are summed. The old
  hard clamp ran before streaming was added, while moni's per-note limiters
  could not protect the summed chord. The output ceiling is about -0.3 dB, with
  instant attack and 50 ms gain recovery. Loud passages may sound less loud;
  this limiter is not a lookahead mastering or inter-sample true-peak limiter.
- Event searches use prefix maxima of end times. Binary-searching raw end
  times in a list sorted by starts could discard a long note underneath short
  notes. The search also bounds the future events inspected for each block.
- Samples starting inside a block begin at their correct frame instead of
  waiting for the next callback.
- `playback_duration()` includes the final note release for non-looping engine
  playback and exports. `total_duration()` retains scheduled note lengths for
  repeat spacing. Neither automatically estimates effect tails after the last
  event, and looping playback retains its scheduled loop length.
- Visualization callback registration is checked with `try_lock`, so the
  output callback does not wait for registration to finish.

Changes in moni's shared PianoSequencer (also used by the game piano):

- Retriggers create fresh synth voices. Previous voices enter release, and
  already-releasing voices of the same pitch keep their tails.
- At the normal voice budget, the oldest release is shortened to 5 ms; if
  necessary, the oldest held voice receives the same fade. No hard stop is used
  for stealing or all-notes-off. A bounded extra pool tracks these short fades;
  excessive bursts are refused until room returns. With the default budget of
  12, the bound is 24 voices including stealing fades.
- Finished sounds are pruned before admitting another voice. The unused
  synthesis cache on each 30-second live note was removed.
- Keyboard audio releases existing held notes before same-frame retriggers.
  Unmatched releases are deferred until after attacks so a tap occurring
  wholly in one frame is released. Separate on/off message types still cannot
  represent arbitrary event interleaving exactly; an ordered event stream is
  the longer-term fix. Sample instruments retain their existing one-shot path.

Validation: all 1,592 tunes library tests pass, including 11 new offline
regressions covering fades, voice storage, clipping protection, overlapping
notes, sample onset and rendering parity. The pushedpeople application
workspace passes `cargo check --workspace --offline`, and tunes passes
`cargo check --lib --target wasm32-unknown-unknown --features web`. Existing
warnings remain. No game or audio device was opened.

For a listening comparison, use the same buffer setting and volume: repeat a
single key quickly, alternate large chords, then try long releases with reverb
and delay. Release all keys and confirm both sound and CPU activity settle.
Also compare the synthesized AcousticPiano and sampled piano modes separately.

Further audit targets outside the repaired live-synth path include native
streaming setup/destruction in the callback, fractional-rate mixer resampling
across block boundaries, and stereo effects that reuse one stateful processor
for both channels. The callback still has allocations and synchronization in
some paths; these changes do not establish a fully bounded real-time engine.
Avoiding allocation and blocking work in callbacks follows the guidance in
[PortAudio's callback documentation](https://portaudio.com/docs/v19-doxydocs/writing_a_callback.html).

October 2026 follow-up: shared effects and lighter voice rendering

- The shared moni piano now routes synth voices through one persistent stereo
  delay/reverb bus. Filters and distortion remain per voice. Each bus channel
  has independent effect history, and tails continue after notes retire.
  Global pause freezes the bus; global stop clears its history. Wet/dry changes
  preserve tails, while replacing effect configuration resets them. Keep the
  `EffectBus` handle alive with the instrument; dropping it removes its tails
  and leaves any remaining voices playing dry.
- `AudioEngine::play_track` prepares a direct track voice without constructing
  a composition, mixer, or default bus for each note. The piano also caches
  instrument presets and removes per-note mastering limiters; the existing
  linked engine output limiter still protects the summed output. Shared effects
  and removing per-note limiting can change chord dynamics and release sound.
- Plain sample playback uses a direct sample voice. Requests with filters,
  effects, or spatial positioning retain the composition route. Direct track
  and sample voices render into the engine's shared scratch buffer.
- Notes prepare their frame ranges and rendering choices once per block.
  Drum processing prepares intervals at hit boundaries, preserving same-time
  hits and the resumption of an older long hit after a shorter replacement.
  Cache flags and event scratch storage are reused. This reduces bookkeeping
  in the sample loop; voice creation is not allocation-free.
- Scalar-parity tests exposed a sample interpolation defect: SIMD chunks could
  omit the final frame, and one-frame samples were silent. Both now render
  consistently regardless of chunk alignment.

Validation: all 1,602 tunes library tests pass, including ten added regressions
for direct-source parity, fractional event boundaries, overlapping drums,
sample endings, independent effect channels, shared tails, and playback
controls. Native and WASM `cargo check --package pushedpeople_game --offline`
pass, as does the native moni package check. Existing dependency warnings
remain. No game, audio device, or iOS simulator was opened; device CPU savings
and audible behavior still need a listening comparison, especially chords,
retriggers, and long reverb/delay releases. Shared buses currently continue
processing silence to preserve tails rather than sleeping when inaudible.

October 3 follow-up: overload handling and worker preparation

The command queue now holds at most 384 entries, reserving the final 128 for
release/stop/pause and bus retirement. Parameter updates coalesce on the producer
only within a trailing run of parameter commands; fades, playback and pauses
remain ordering barriers. The callback takes up to 64 commands with `try_lock`.
If even the release reserve fills, pending attacks are canceled and an emergency
global stop is scheduled instead of losing note-offs. A full ordinary queue
returns an error. This bounds control processing, not arbitrary user DSP graphs.

There are 96 normal voice slots and eight extra stealing fades of 5 ms. Priority
protects important attacks from incidental sounds; candidates rank by priority,
release state and estimated gain. Direct-source gain and cached spatial gain
contribute to that estimate; this is not a measured psychoacoustic loudness model.
Incidental samples allow four simultaneous instances per path. The game's local
piano/nonspatial feedback are important, while spatial sample requests are
incidental. Native streams count toward the same budget and have a separate
maximum of 16. There are at most 16 engine effect-bus handles.

Finished sources, replaced buses and finished streams enter fixed retirement
queues. A native maintenance thread or browser timer destroys them outside the
callback. If maintenance cannot keep up, output becomes silent until retirement
capacity returns rather than allocating an unbounded cleanup backlog. Native
stream buffers and decoder threads are prepared on the caller. Device buffers
render in prepared-size chunks; the scratch reserve accommodates rates up to
16x including Doppler. Dry direct samples with effectively zero output gain can
advance without synthesis; stateful effect paths continue processing.

Playback status now uses a fixed atomic registry. Listener/spatial configuration
lives in callback state, so updates neither allocate nor leak old epoch snapshots.
Listener changes invalidate spatial caches. Unconsumed track/bus RMS scans are
skipped, with current sidechain dependencies refreshed each block.

Remaining realtime limitations include user-supplied monitor callbacks, arbitrary
composition complexity, DSP-internal buffer growth, effect-history resets and
cache synchronization. The engine is not a hard realtime guarantee. These changes
leave CPAL's browser renderer on the main thread for applications using
`AudioEngine::with_buffer_size`; the game's worker route below replaces that path.

`web-dsp` is now the game's browser output backend. It runs the same Rust DSP
in a dedicated worker; its AudioWorklet consumes four transferable 512-frame
stereo blocks. The launcher unlocks the context during Play, loads protocol v2,
then starts Bevy. Native output continues to use CPAL. No shared memory or
cross-origin isolation is required.

Protocol v2 sends tracks, standard mixer graphs/effects, shared buses, spatial
parameters and playback controls as owned JSON. Decoded PCM is uploaded separately
once per allocation and cached by ID (128 entries / 128 MiB); channels and loop
metadata accompany each sample reference. Spectral and convolution effects are
not supported and produce explicit errors; current game audio does not use them.
Messages use one in-flight batch of at most 64 commands, a bounded pending queue,
parameter coalescing and reserved release/emergency-stop capacity. Engine resets
change the session generation so stale PCM and playback status cannot affect the
replacement engine. Monitor snapshots are optional, approximate visualisation
updates every four blocks, not a lossless recording API.

Visibility/pagehide suspend audio and release the playback session; foreground
and input gestures retry resume. DSP state remains in the worker. A crashed worker
is reported explicitly and requires reloading the page. Four buffered blocks add
up to 46 ms at 44.1 kHz, plus hardware latency. Heavy graph decoding, first PCM
uploads and arbitrary composition complexity can still cause worker starvation;
this is not a hard realtime guarantee.

`build:game` packages both WASM modules before publishing either. Audio assets are
included in the release inventory and service-worker cache. Rebuild both modules
and advance the release version before device testing/deployment. `build:audio`
remains available for the isolated `/game/audio/index.html` harness.

Validation: 1,619 library tests pass, including protocol render parity for filters,
distortion, sample loops and mixer sidechains. Twelve browser transport/worklet and optimizer tests
cover recycling, underruns, queue overload, session changes, pending playback
status, PCM capacity and foreground gestures. The user confirmed the standalone
harness survives a two-second main-thread stall and minimizing/reopening on iPhone.
Integrated gameplay device validation remains outstanding; no game build or
simulator was run by the agent. A disassembly
of the existing packaged game artifact found zero SIMD instructions. The wrapper
now provides instruction inspection and a repeatable size/speed/SIMD benchmark;
`WASM_SIMD=1` explicitly opts builds into SIMD rather than inferring it from the
dispatcher's lane label.
