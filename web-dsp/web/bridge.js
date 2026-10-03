const queueUrl = new URL("./command-queue.mjs", import.meta.url);
queueUrl.search = new URL(import.meta.url).search;
const { AudioCommandQueue } = await import(queueUrl.href);
const restartUrl = new URL("./audio-restart.mjs", import.meta.url);
restartUrl.search = new URL(import.meta.url).search;
const { restartableAudio } = await import(restartUrl.href);
const releaseNames = new Set([
  "FadeOut",
  "Stop",
  "StopAll",
  "PauseAll",
  "RemoveEffectBus",
]);
const parameterNames = new Set([
  "SetVolume",
  "SetPan",
  "SetPlaybackRate",
  "SetSoundPosition",
  "SetSoundVelocity",
  "SetSoundOcclusion",
  "SetSoundCone",
  "SetEffectBusMix",
  "SetListenerPosition",
  "SetListenerVelocity",
  "SetListenerForward",
  "SetSpatialParams",
]);
const emptySamples = new Float32Array(0);

export async function createWorkerAudio({
  onHealth,
  backend = "worklet",
  muted = false,
  signal,
  context = new AudioContext({ latencyHint: "interactive" }),
} = {}) {
  if (backend !== "worklet") throw new Error("Only AudioWorklet output is supported");
  const blockFrames = 128;
  const report = data => onHealth?.(data);
  const asset = (name) => {
    const url = new URL(name, import.meta.url);
    url.search = new URL(import.meta.url).search;
    return url;
  };
  const playbackRequests = new Map();
  let playbackRequestId = 0;
  const playbackRequest = (type, fields) => new Promise((resolve, reject) => {
    const request = ++playbackRequestId;
    const timer = setTimeout(() => { playbackRequests.delete(request); reject(new Error("Playback handoff timed out")); }, 3000);
    playbackRequests.set(request, data => { clearTimeout(timer); resolve(data.snapshot); });
    if (queue.enqueue({ kind: type, session, request, ...fields }, { critical: true }) === false) {
      clearTimeout(timer);
      playbackRequests.delete(request);
      reject(new Error("Playback handoff queue full"));
    }
  });
  let endpoint;
  let outputGain;
  let node,
    timer,
    closed = false,
    fatal = false,
    session = 0,
    uploadPending = false;
  let commandError = null;
  let monitorSamples = emptySamples;
  const playing = new Map();
  const pcmCache = new Map();
  let pcmBytes = 0;
  const queue = new AudioCommandQueue(
    (packets) => {
      endpoint.postMessage(
        { type: "batch", packets },
        packets.filter((p) => p.samples).map((p) => p.samples.buffer),
      );
    },
    (id) => playing.delete(id),
  );
  const fail = (message) => {
    fatal = true;
    queue.clear();
    playing.clear();
    endpoint?.terminate();
    void context.suspend().catch(() => {});
    console.error("[tunes] Audio worklet failed:", message);
    report({ type: "error", message });
  };
  function updateStatus(data) {
    if (data.session !== session) return;
    const active = new Set(data.playing);
    for (const [id, seq] of playing)
      if (seq <= data.sequence && !active.has(id)) playing.delete(id);
    if (data.samples) monitorSamples = data.samples;
  }
  try {
    const resumed = context.resume();
    // Avoid an unhandled rejection while loading the audio module.
    void resumed.catch(() => {});
    if (navigator.audioSession) navigator.audioSession.type = "playback";
    const response = await fetch(asset("./tunes_web_dsp_bg.wasm"), { signal });
    if (!response.ok) throw new Error(`DSP WASM download failed (${response.status})`);
    const directModule = await WebAssembly.compile(await response.arrayBuffer());
    if (signal?.aborted) throw new Error("Audio preparation cancelled");
    await context.audioWorklet.addModule(asset("./dsp-worklet.js"));
    if (signal?.aborted) throw new Error("Audio preparation cancelled");
    node = new AudioWorkletNode(context, "tunes-dsp", {
      numberOfInputs: 0,
      numberOfOutputs: 1,
      outputChannelCount: [2],
    });
    await new Promise((resolve, reject) => {
      // Create and attach handlers together so module-load failures cannot be lost.
      {
        // Keep the command bridge unchanged, using the worklet port as its endpoint.
        endpoint = {
          postMessage: (message, transfer = []) =>
            node.port.postMessage(message, transfer),
          terminate: () => {
            node.port.postMessage({ type: "close" });
            node.disconnect();
          },
        };
        node.port.onmessage = (event) => {
          if (event.data.type === "entropy-needed") {
            const bytes = crypto.getRandomValues(new Uint8Array(65536));
            node.port.postMessage({ type: "entropy", bytes }, [bytes.buffer]);
          } else if (event.data.type === "health") {
            node.port.postMessage({ type: "health-ack" });
            report(event.data);
          } else endpoint.onmessage?.(event);
        };
        node.port.onmessageerror = (event) => endpoint.onmessageerror?.(event);
        node.onprocessorerror = () =>
          endpoint.onerror?.({ message: "Direct DSP AudioWorklet stopped" });
      }
      let startupStage = "starting audio worklet";
      timer = setTimeout(
        () =>
          reject(
            new Error(`DSP worklet initialization timed out (${startupStage})`),
          ),
        30000,
      );
      endpoint.onerror = (event) =>
        reject(
          new Error(event.message || `DSP worklet failed (${startupStage})`),
        );
      endpoint.onmessageerror = () =>
        reject(new Error("DSP worklet startup message could not be decoded"));
      endpoint.onmessage = ({ data }) => {
      if (data.type === "playback-reply") {
        playbackRequests.get(data.request)?.(data);
        playbackRequests.delete(data.request);
      }
        if (data.type === "startup") {
          startupStage = data.stage;
          console.info("[tunes] DSP startup:", startupStage);
        }
        if (data.type === "ready") {
          if (data.protocol === 2) resolve();
          else
            reject(
              new Error(
                "DSP protocol mismatch; rebuild game and audio together",
              ),
            );
        }
        if (data.type === "fatal") reject(new Error(data.message));
      };
      endpoint.postMessage(
        {
          type: "init",
          held: muted,
          telemetry: typeof onHealth === "function",
          sampleRate: context.sampleRate,
          module: directModule,
          entropy: crypto.getRandomValues(new Uint8Array(65536)),
        },
        [],
      );
    });
    if (signal?.aborted) throw new Error("Audio preparation cancelled");
    clearTimeout(timer);
    endpoint.onmessage = ({ data }) => {
      if (data.type === "playback-reply") {
        playbackRequests.get(data.request)?.(data);
        playbackRequests.delete(data.request);
      }
      if (data.type === "ack") {
        updateStatus(data);
        if (data.errors?.length) {
          commandError = data.errors.join("; ");
          console.warn("[tunes] Audio command rejected:", data.errors);
        }
        queue.ack();
      }
      if (data.type === "status") {
        updateStatus(data);
        endpoint.postMessage({ type: "status-ack" });
      }
      if (data.type === "timing") {
        endpoint.postMessage({ type: "timing-ack" });
        report(data);
      }
      if (data.type === "sample-ready") uploadPending = false;
      if (data.type === "fatal") fail(data.message);
    };
    // Startup reports may already have been delivered to the temporary handler.
    endpoint.postMessage({ type: "status-ack" });
    endpoint.postMessage({ type: "timing-ack" });
    endpoint.onerror = (event) => fail(event.message);
    endpoint.onmessageerror = () =>
      fail("DSP worklet message could not be decoded");
    if (muted) {
      outputGain = context.createGain();
      outputGain.gain.value = 0;
      node.connect(outputGain);
      outputGain.connect(context.destination);
    } else node.connect(context.destination);
    // Activation may have been lost during download; gestures below retry it.
    void resumed.catch((error) =>
      console.info("[tunes] Audio awaits a gesture:", error),
    );
  } catch (error) {
    clearTimeout(timer);
    endpoint?.terminate();
    node?.disconnect();
    void context.close().catch(console.warn);
    throw error;
  }
  const valid = (s) => !closed && !fatal && s === session;
  function attach() {
    if (closed || fatal) throw new Error("DSP worklet unavailable");
    session++;
    playing.clear();
    pcmCache.clear();
    pcmBytes = 0;
    monitorSamples = emptySamples;
    node.port.postMessage({ type: "generation", session });
    if (!queue.enqueue({ kind: "reset", session }, { critical: true }))
      throw new Error("Audio reset queue full");
    return session;
  }
  function wire(s, json) {
    if (!valid(s) || json.length > 16 * 1024 * 1024) return false;
    const command = JSON.parse(json);
    if (command.version !== 2) return false;
    const id = command.id,
      op = command.op;
    const name = op.kind === "Control" ? op.name : null;
    const packet = {
      kind: "wire",
      session: s,
      json,
      playId: op.kind === "Play" ? id : null,
      busLifecycle: op.kind === "Bus" || name === "RemoveEffectBus",
    };
    const critical = releaseNames.has(name);
    let seq = queue.enqueue(packet, {
      critical,
      key: parameterNames.has(name) ? `${s}:${id}:${name}` : null,
    });
    if (seq === false && critical) {
      seq = queue.emergencyStop({
        kind: "wire",
        session: s,
        json: JSON.stringify({
          version: 2,
          id: "0",
          op: {
            kind: "Control",
            name: "StopAll",
            args: [],
            position: null,
            cone: null,
            spatial: null,
          },
        }),
      });
      // Retain bus retirement even under control overload.
      if (name === "RemoveEffectBus")
        seq = queue.enqueue(packet, { critical: true });
    }
    if (seq !== false && packet.playId) playing.set(id, seq);
    return seq !== false;
  }
  let pageHidden = document.hidden;
  let lifecycleEpoch = 0;
  let recoveryTimer;
  let needsRecovery = false;
  let cycling = false;
  const foreground = () => !closed && !fatal && !pageHidden && !document.hidden;
  const sessionType = (type) => {
    try {
      if (navigator.audioSession) navigator.audioSession.type = type;
    } catch (error) {
      console.warn("[tunes] Audio session update failed", error);
    }
  };
  const suspendHidden = () => {
    void context.suspend().then(() => {
      // A slow suspend may finish after pageshow/resume.
      if (foreground()) resumeForeground();
    }).catch(console.warn);
  };
  const resumeForeground = () => {
    if (!foreground()) return;
    sessionType("playback");
    // Call synchronously so a trusted gesture can unlock Safari audio.
    void context.resume().then(() => {
      // Conversely, an old resume must not restart hidden playback.
      if (!closed && !fatal && !foreground()) suspendHidden();
    }).catch((error) => console.warn("[tunes] Audio resume failed", error));
  };
  const probe = (retry = true) => {
    clearTimeout(recoveryTimer);
    const epoch = lifecycleEpoch;
    const clock = context.currentTime;
    recoveryTimer = setTimeout(() => {
      recoveryTimer = undefined;
      if (epoch !== lifecycleEpoch || !foreground()) return;
      const delta = context.currentTime - clock;
      if (context.state === "running" && delta > 0.01) {
        needsRecovery = false;
        console.info("[tunes] Foreground audio resumed", { backend, clockDelta: delta });
        return;
      }
      needsRecovery = true;
      if (!retry) {
        console.warn("[tunes] Foreground audio stalled; tap to retry", { state: context.state });
        return;
      }
      console.warn("[tunes] Recovering foreground audio", { state: context.state, clockDelta: delta });
      if (context.state === "running") {
        cycling = true;
        void context.suspend().then(() => {
          cycling = false;
          if (!foreground()) return;
          resumeForeground();
          if (epoch === lifecycleEpoch) probe(false);
        }).catch((error) => {
          cycling = false;
          console.warn("[tunes] Foreground audio recovery failed", error);
        });
      } else {
        resumeForeground();
        probe(false);
      }
    }, 600);
  };
  const hide = () => {
    pageHidden = true;
    lifecycleEpoch++;
    needsRecovery = true;
    clearTimeout(recoveryTimer);
    recoveryTimer = undefined;
    suspendHidden();
    sessionType("auto");
  };
  const visibility = () => {
    if (document.hidden) hide();
    else {
      pageHidden = false;
      lifecycleEpoch++;
      needsRecovery = true;
      resumeForeground();
      probe();
    }
  };
  const gesture = () => {
    if (!foreground()) return;
    if (needsRecovery || context.state !== "running") {
      resumeForeground();
      if (recoveryTimer === undefined && !cycling) probe();
    }
  };
  const stateChanged = () => {
    if (closed || fatal) return;
    if (!foreground()) {
      if (context.state === "running") suspendHidden();
    } else if (!cycling && context.state !== "running" && context.state !== "closed") {
      needsRecovery = true;
      resumeForeground();
      if (recoveryTimer === undefined) probe();
    }
  };
  context.addEventListener?.("statechange", stateChanged);
  document.addEventListener("visibilitychange", visibility, true);
  window.addEventListener("pagehide", hide, true);
  window.addEventListener("pageshow", visibility, true);
  window.addEventListener("pointerup", gesture, true);
  window.addEventListener("touchend", gesture, true);
  window.addEventListener("keydown", gesture, true);
  if (pageHidden) hide();
  return {
    sampleRate: context.sampleRate,
    bufferStatus() {
      return {
        backend,
        blockFrames,
        baseLatency: context.baseLatency ?? null,
        outputLatency: context.outputLatency ?? null,
      };
    },
    capturePlayback: ids => playbackRequest("playback-snapshot", { ids }),
    restorePlayback: snapshot => playbackRequest("restore-playback", { snapshot }),
    releasePlayback() { endpoint.postMessage({ type: "release-playback", cancelThrough: playbackRequestId }); },
    deactivate() { node.disconnect(); },
    activate() {
      endpoint.postMessage({ type: "release-playback", cancelThrough: playbackRequestId });
      if (outputGain) outputGain.gain.value = 1;
      if (foreground()) resumeForeground();
    },
    restoreSession(value) {
      session = value - 1;
      attach();
    },
    async drain() {
      const deadline = performance.now() + 10000;
      while (queue.busy || queue.pending.length) {
        if (commandError) throw new Error(commandError);
        if (closed || fatal || performance.now() > deadline)
          throw new Error("Audio restoration timed out or failed");
        await new Promise(resolve => setTimeout(resolve, 10));
      }
      if (commandError || closed || fatal) throw new Error(commandError ?? "Audio unavailable");
    },
    attach,
    wire,
    pcm(s, key, samples) {
      const bytes = pcmBytes - (pcmCache.get(key) ?? 0) + samples.byteLength;
      if (
        !valid(s) ||
        !samples.length ||
        bytes > 128 * 1024 * 1024 ||
        (!pcmCache.has(key) && pcmCache.size >= 128)
      )
        return false;
      if (
        queue.enqueue({
          kind: "pcm",
          session: s,
          key,
          samples: new Float32Array(samples),
        }) === false
      )
        return false;
      pcmCache.set(key, samples.byteLength);
      pcmBytes = bytes;
      return true;
    },
    removePcm(s, key) {
      if (
        !valid(s) ||
        queue.enqueue(
          { kind: "remove-pcm", session: s, key },
          { critical: true },
        ) === false
      )
        return false;
      pcmBytes -= pcmCache.get(key) ?? 0;
      pcmCache.delete(key);
      return true;
    },
    isPlaying(s, id) {
      return valid(s) && playing.has(id);
    },
    monitor(s) {
      if (!valid(s)) return emptySamples;
      const result = monitorSamples;
      monitorSamples = emptySamples;
      return result;
    },
    enableMonitor(s, enabled) {
      if (valid(s)) queue.enqueue({ kind: "monitor", session: s, enabled });
    },
    detach(s) {
      if (valid(s)) {
        attach();
      }
    },
    command(method, ...args) {
      return (
        !closed &&
        !fatal &&
        queue.enqueue(
          { kind: "legacy", session, method, args },
          { critical: method === "release" || method === "stop_all" },
        ) !== false
      );
    },
    upload(id, samples, sampleRate) {
      if (
        closed ||
        uploadPending ||
        !(samples instanceof Float32Array) ||
        samples.byteLength > 64 * 1024 * 1024
      )
        return false;
      uploadPending = true;
      endpoint.postMessage({ type: "sample", id, sampleRate, samples }, [
        samples.buffer,
      ]);
      return true;
    },
    async close({ preserveSession = false } = {}) {
      if (closed) return;
      closed = true;
      lifecycleEpoch++;
      clearTimeout(recoveryTimer);
      context.removeEventListener?.("statechange", stateChanged);
      queue.clear();
      playing.clear();
      document.removeEventListener("visibilitychange", visibility, true);
      window.removeEventListener("pagehide", hide, true);
      window.removeEventListener("pageshow", visibility, true);
      window.removeEventListener("pointerup", gesture, true);
      window.removeEventListener("touchend", gesture, true);
      window.removeEventListener("keydown", gesture, true);
      endpoint?.terminate();
      node.disconnect();
      outputGain?.disconnect();
      if (!preserveSession && navigator.audioSession) navigator.audioSession.type = "auto";
      await context.close();
    },
  };
}

let installing = false;
/** Install one page-wide producer bridge; await this before starting application WASM. */
export async function installWorkerAudio(options) {
  if (installing || globalThis.__tunesWorkerAttach)
    throw new Error(
      "Tunes audio is already installed; close it before installing another",
    );
  installing = true;
  let audio;
  try {
    const initial = await createWorkerAudio({
      ...options,
      context: options?.context ?? new AudioContext({ latencyHint: options?.latencyHint ?? "interactive" }),
    });
    audio = restartableAudio(initial, async (latencyHint, sampleRate, signal) => {
      const context = new AudioContext({ latencyHint, sampleRate });
      const cancel = () => { void context.close().catch(console.warn); };
      signal.addEventListener('abort', cancel, { once: true });
      try { return await createWorkerAudio({ ...options, context, muted: true, signal }); }
      finally { signal.removeEventListener('abort', cancel); }
    }, options?.latencyHint ?? "interactive");
  } finally {
    installing = false;
  }
  const bindings = {
    __tunesAudioLatencyHint: value => audio.setLatencyHint(value),
    __tunesAudioPreferenceStatus: () => audio.preferenceStatus(),
    __tunesWorkerAttach: () => audio.attach(),
    __tunesAudioOutputLatency: () => audio.bufferStatus().outputLatency,
    __tunesWorkerRate: () => audio.sampleRate,
    __tunesWorkerSend: (s, json) => audio.wire(s, json),
    __tunesWorkerPcm: (s, key, pcm) => audio.pcm(s, key, pcm),
    __tunesWorkerRemovePcm: (s, key) => audio.removePcm(s, key),
    __tunesWorkerPlaying: (s, id) => audio.isPlaying(s, id),
    __tunesWorkerMonitor: (s) => audio.monitor(s),
    __tunesWorkerEnableMonitor: (s, enabled) => audio.enableMonitor(s, enabled),
    __tunesWorkerDetach: (s) => audio.detach(s),
  };
  Object.assign(globalThis, bindings);
  const close = audio.close.bind(audio);
  audio.close = async () => {
    try {
      await close();
    } finally {
      for (const [name, fn] of Object.entries(bindings))
        if (globalThis[name] === fn) delete globalThis[name];
    }
  };
  console.info("[tunes] AudioWorklet output ready", {
    protocol: 2,
    sampleRate: audio.sampleRate,
    backend: audio.bufferStatus().backend,
    bufferedFrames:
      audio.bufferStatus().blocks * audio.bufferStatus().blockFrames,
  });
  return audio;
}

// Compatibility with launchers written before the standalone browser API.
export const installGameAudio = installWorkerAudio;
