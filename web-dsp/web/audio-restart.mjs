// Keep the game's bridge identity stable while replacing the AudioContext/DSP.
// Restores sample playback and configuration; synthesized voices are discarded.
// Sample transport clocks are transferred at handoff; effect histories reset.
export function restartableAudio(initial, create, initialHint = 'interactive', timeoutMs = 15000) {
  let audio = initial, session = 0, hint = initialHint, desired = hint;
  let pending = null, task = null, closed = false, error = null;
  let monitor = false, order = 0;
  let stage = 'idle';
  const retire = value => {
    // close() has already disconnected output/listeners before its browser promise.
    // Do not let a pending browser close keep the preference UI busy.
    void value.close({ preserveSession: true }).catch(console.warn);
  };
  const pcm = new Map(), buses = new Map(), voices = new Map(), globals = new Map();
  const maxBytes = 128 * 1024 * 1024;
  const keyFor = command => {
    const name = command.op.name;
    return name === 'PauseAll' || name === 'ResumeAll' ? 'pause' : name;
  };
  const prune = () => {
    // Keep IDs until the handoff can explicitly stop any restored samples that finish.
    if (pending) return;
    for (const id of voices.keys()) if (!audio.isPlaying(session, id)) voices.delete(id);
  };
  const describe = () => {
    prune();
    return [
      ...[...pcm].map(([key, samples]) => ['pcm', [session, key, samples]]),
      ...[...buses.values(), ...voices.values(), globals].flatMap(v => [...v.values()])
        .sort((a, b) => a.order - b.order).map(({json}) => ['wire', [session, json]]),
      ['enableMonitor', [session, monitor]],
    ];
  };
  const sampleSource = source => {
    if (source?.Sample) return true;
    // Samples with effects/spatial processing may use a track or mixer wrapper.
    const tracks = source?.Track ? [source.Track]
      : source?.Mixer?.buses?.flatMap(bus => bus.tracks ?? []) ?? [];
    let found = false;
    for (const track of tracks) for (const event of track.events ?? []) {
      if (!event.Sample) return false; // Never restore synthesized notes/drums or mixed graphs.
      found = true;
    }
    return found;
  };
  const persistentControl = new Set([
    'SetListenerPosition', 'SetListenerVelocity', 'SetListenerForward', 'SetSpatialParams',
    'SetEffectBusMix', 'RemoveEffectBus', 'PauseAll', 'ResumeAll', 'StopAll',
  ]);
  const preserve = c => c.op.kind === 'Bus'
    || (c.op.kind === 'Play' && sampleSource(c.op.source))
    || (c.op.kind === 'Control' && (voices.has(c.id) || persistentControl.has(c.op.name)));
  const rememberWire = json => {
    const c = JSON.parse(json), name = c.op.name;
    const record = { json, order: ++order };
    if (c.op.kind === 'Play') voices.set(c.id, new Map([['play', record]]));
    else if (c.op.kind === 'Bus') buses.set(c.id, new Map([['bus', record]]));
    else if (name === 'StopAll') voices.clear();
    else if (name === 'Stop') voices.delete(c.id);
    else if (name === 'RemoveEffectBus') buses.delete(c.id);
    else {
      const target = name === 'SetEffectBusMix' ? buses.get(c.id)
        : name?.startsWith('SetListener') || ['SetSpatialParams', 'PauseAll', 'ResumeAll'].includes(name)
          ? globals : voices.get(c.id);
      if (target) { const key = keyFor(c); target.delete(key); target.set(key, record); }
    }
  };
  const bytes = event => event[0] === 'pcm' ? event[1][2].byteLength
    : event[0] === 'wire' ? event[1][1].length * 2 : 0;
  // Match live transport semantics: replace setters only within a trailing run
  // of parameter updates. Attacks, stops, PCM and resets remain FIFO barriers.
  const replaceable = new Set([
    'SetVolume', 'SetPan', 'SetPlaybackRate', 'SetSoundPosition',
    'SetSoundVelocity', 'SetSoundOcclusion', 'SetSoundCone', 'SetEffectBusMix',
    'SetListenerPosition', 'SetListenerVelocity', 'SetListenerForward', 'SetSpatialParams',
  ]);
  const enqueueChange = event => {
    if (!pending || pending.overflow) return;
    let key = null;
    if (event[0] === 'wire') {
      const c = JSON.parse(event[1][1]);
      if (c.op.kind === 'Control' && replaceable.has(c.op.name))
        key = JSON.stringify([event[1][0], c.id, c.op.name]);
    }
    const index = key === null ? undefined : pending.tail.get(key);
    const size = pending.bytes + bytes(event) - (index === undefined ? 0 : bytes(pending.events[index]));
    if ((index === undefined && pending.events.length >= 512) || size > maxBytes) {
      pending.overflow = true;
      return;
    }
    pending.bytes = size;
    if (index !== undefined) pending.events[index] = event;
    else {
      if (key === null) pending.tail.clear();
      else pending.tail.set(key, pending.events.length);
      pending.events.push(event);
    }
  };
  const replay = async (next, events, wait) => {
    // Preserve FIFO while amortizing acknowledgement latency. The processor still
    // applies only four commands per quantum; this does not increase its budget.
    for (let offset = 0; offset < events.length; offset += 32) {
      if (closed) throw new Error('Audio closed');
      for (const [method, args] of events.slice(offset, offset + 32)) {
        if (next[method](...args) === false) throw new Error(`Audio restore rejected ${method}`);
      }
      await wait(next.drain());
    }
  };
  const run = async () => {
    while (!closed && desired !== hint) {
      const target = desired;
      let next;
      let expired = false;
      const abort = new AbortController();
      const deadline = performance.now() + timeoutMs;
      const wait = async promise => {
        if (performance.now() >= deadline) {
          void Promise.resolve(promise).catch(() => {});
          throw new Error(`Audio preference timed out: ${stage}`);
        }
        let timer;
        try {
          return await Promise.race([promise, new Promise((_, reject) => {
            timer = setTimeout(() => reject(new Error(`Audio preference timed out: ${stage}`)),
              Math.max(0, deadline - performance.now()));
          })]);
        } finally { clearTimeout(timer); }
      };
      try {
        stage = 'preparing context';
        console.info('[tunes] Applying audio preference', { target });
        const events = describe();
        const capturedSession = session;
        pending = { events: [], bytes: 0, overflow: false, tail: new Map() };
        const creation = create(target, audio.sampleRate, abort.signal).then(value => {
          if (expired) { retire(value); throw new Error('Audio restart expired'); }
          return value;
        });
        next = await wait(creation);
        stage = 'restoring audio';
        next.restoreSession(capturedSession);
        await wait(next.drain());
        await replay(next, events, wait);
        stage = "catching up commands";
        for (let pass = 0; pass < 2; pass++) {
          if (pending.overflow) throw new Error('Audio changed too quickly to restart safely');
          const changes = pending.events.splice(0);
          pending.tail.clear();
          pending.bytes = 0;
          await replay(next, changes, wait);
          let stopped = false;
          for (const id of voices.keys()) {
            if (!audio.isPlaying(session, id)) {
              next.wire(session, JSON.stringify({ version: 2, id,
                op: { kind: 'Control', name: 'Stop', args: [], position: null, cone: null, spatial: null } }));
              voices.delete(id);
              stopped = true;
            }
          }
          if (stopped) await wait(next.drain());
          if (!pending.events.length) break;
        }
        stage = 'transferring sample position';
        const capturedIds = [...voices.keys()];
        const beforeSnapshot = pending.events.splice(0);
        pending.tail.clear();
        pending.bytes = 0;
        // Capture is a FIFO barrier on the old bridge. Commands arriving after
        // this point belong after the restored snapshot, not before it.
        const snapshotPromise = audio.capturePlayback(capturedIds);
        // Observe immediately even if replay fails before awaiting the result.
        void snapshotPromise.catch(() => {});
        await replay(next, beforeSnapshot, wait);
        const snapshot = await wait(snapshotPromise);
        await wait(next.restorePlayback(snapshot));
        // A sample may finish between its last status report and the snapshot.
        const alive = new Set(JSON.parse(snapshot).map(([id]) => id));
        for (const id of capturedIds) if (!alive.has(id)) {
          next.wire(session, JSON.stringify({ version: 2, id,
            op: { kind: 'Control', name: 'Stop', args: [], position: null, cone: null, spatial: null } }));
        }
        if (pending.overflow) throw new Error('Audio restart backlog exceeded');
        // Take a finite handoff point. Continuous game updates need not become
        // silent: enqueue the final tail synchronously, then route new commands
        // to the replacement. FIFO keeps registrations before dependent notes.
        for (const [method, args] of pending.events) {
          if (next[method](...args) === false)
            throw new Error(`Audio handoff rejected ${method}`);
        }
        pending.events.length = 0;
        if (closed) throw new Error('Audio closed');
        if (target !== desired) {
          audio.releasePlayback();
          retire(next);
          next = null;
          continue;
        }
        const previous = audio;
        audio = next;
        next = null;
        hint = target;
        error = null;
        pending = null;
        // Both contexts use the same sample rate; Rust engine metadata stays valid.
        previous.deactivate?.();
        audio.activate();
        retire(previous);
        console.info("[tunes] Audio preference applied", { hint, ...audio.bufferStatus() });
      } catch (cause) {
        audio.releasePlayback();
        expired = true;
        abort.abort();
        if (next) retire(next);
        error = String(cause);
        console.warn('[tunes] Audio preference change failed; keeping current audio', cause);
        if (target === desired) desired = hint;
      } finally {
        stage = "idle";
        pending = null;
      }
    }
  };
  const api = {
    get sampleRate() { return audio.sampleRate; },
    bufferStatus: () => audio.bufferStatus(),
    preferenceStatus: () => ({ applied: hint, requested: desired, changing: !!task, stage, error }),
    setLatencyHint(value) {
      if (!['interactive', 'balanced', 'playback'].includes(value) || closed) return false;
      desired = value;
      if (!task && desired !== hint) {
        // A short debounce coalesces rapid clicks without interrupting current output.
        task = new Promise(resolve => setTimeout(resolve, 150)).then(run).finally(() => { task = null; });
      }
      return true;
    },
    attach() {
      session = audio.attach();
      pcm.clear(); buses.clear(); voices.clear(); globals.clear(); monitor = false;
      enqueueChange(['restoreSession', [session]]);
      return session;
    },
    wire(s, json) {
      if (s !== session || closed) return false;
      const command = JSON.parse(json);
      prune();
      // Ordinary synth notes/controls still reach current output, but never enter
      // the restart journal or catch-up queue, including during preparation.
      if (!preserve(command)) return audio.wire(s, json);
      // Bound retained descriptions as well as transient command batches.
      let retained = json.length * 2;
      for (const map of [...buses.values(), ...voices.values(), globals])
        for (const value of map.values()) retained += value.json.length * 2;
      if (retained > maxBytes || !audio.wire(s, json)) return false;
      rememberWire(json);
      enqueueChange(['wire', [s, json]]);
      return true;
    },
    pcm(s, key, samples) {
      if (s !== session || closed || !audio.pcm(s, key, samples)) return false;
      const copy = new Float32Array(samples);
      pcm.set(key, copy);
      enqueueChange(['pcm', [s, key, copy]]);
      return true;
    },
    removePcm(s, key) {
      if (s !== session || !audio.removePcm(s, key)) return false;
      pcm.delete(key);
      enqueueChange(['removePcm', [s, key]]);
      return true;
    },
    isPlaying: (s, id) => audio.isPlaying(s, id),
    monitor: s => audio.monitor(s),
    enableMonitor(s, enabled) {
      if (s !== session) return;
      monitor = enabled;
      audio.enableMonitor(s, enabled);
      enqueueChange(['enableMonitor', [s, enabled]]);
    },
    detach(s) { if (s === session) api.attach(); },
    async close() {
      closed = true;
      await task;
      await audio.close();
      pcm.clear(); buses.clear(); voices.clear(); globals.clear();
    },
  };
  return api;
}
