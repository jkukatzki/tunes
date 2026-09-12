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
