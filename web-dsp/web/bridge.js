const queueUrl = new URL("./command-queue.mjs", import.meta.url);
queueUrl.search = new URL(import.meta.url).search;
const { AudioCommandQueue } = await import(queueUrl.href);
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
  onHealth = () => {},
  context = new AudioContext({ latencyHint: "interactive" }),
} = {}) {
  const asset = (name) => {
    const url = new URL(name, import.meta.url);
    url.search = new URL(import.meta.url).search;
    return url;
  };
  const worker = new Worker(asset("./worker.js"), { type: "module" });
  let node,
    timer,
    closed = false,
    fatal = false,
    session = 0,
    uploadPending = false;
  let monitorSamples = emptySamples;
  const playing = new Map();
  const pcmCache = new Map();
  let pcmBytes = 0;
  const queue = new AudioCommandQueue(
    (packets) => {
      worker.postMessage(
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
    worker.terminate();
    void context.suspend().catch(() => {});
    console.error("[tunes] Audio worker failed:", message);
    onHealth({ type: "error", message });
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
    // Avoid an unhandled rejection while loading the worker module.
    void resumed.catch(() => {});
    if (navigator.audioSession) navigator.audioSession.type = "playback";
    await context.audioWorklet.addModule(asset("./output-worklet.js"));
    node = new AudioWorkletNode(context, "tunes-output", {
      numberOfInputs: 0,
      numberOfOutputs: 1,
      outputChannelCount: [2],
    });
    node.onprocessorerror = () =>
      fail("AudioWorklet processor stopped; reload to restart audio");
    node.port.onmessage = ({ data }) => {
      if (data.type === "health") node.port.postMessage({ type: "health-ack" });
      onHealth(data);
    };
    const channel = new MessageChannel();
    node.port.postMessage({ type: "connect", port: channel.port1 }, [
      channel.port1,
    ]);
    await new Promise((resolve, reject) => {
      timer = setTimeout(
        () => reject(new Error("DSP worker initialization timed out")),
        30000,
      );
      worker.onerror = (event) => reject(new Error(event.message));
      worker.onmessage = ({ data }) => {
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
      worker.postMessage(
        {
          type: "init",
          sampleRate: context.sampleRate,
          wasmUrl: asset("./tunes_web_dsp_bg.wasm").href,
          port: channel.port2,
        },
        [channel.port2],
      );
    });
    clearTimeout(timer);
    worker.onmessage = ({ data }) => {
      if (data.type === "ack") {
        updateStatus(data);
        if (data.errors?.length)
          console.warn("[tunes] Worker command rejected:", data.errors);
        queue.ack();
      }
      if (data.type === "status") {
        updateStatus(data);
        worker.postMessage({ type: "status-ack" });
      }
      if (data.type === "timing") {
        worker.postMessage({ type: "timing-ack" });
        onHealth(data);
      }
      if (data.type === "sample-ready") uploadPending = false;
      if (data.type === "fatal") fail(data.message);
    };
    // Startup reports may already have been delivered to the temporary handler.
    worker.postMessage({ type: "status-ack" });
    worker.postMessage({ type: "timing-ack" });
    worker.onerror = (event) => fail(event.message);
    node.connect(context.destination);
    // Activation may have been lost during download; gestures below retry it.
    void resumed.catch((error) =>
      console.info("[tunes] Audio awaits a gesture:", error),
    );
  } catch (error) {
    clearTimeout(timer);
    worker.terminate();
    node?.disconnect();
    await context.close();
    throw error;
  }
  const valid = (s) => !closed && !fatal && s === session;
  function attach() {
    if (closed || fatal) throw new Error("DSP worker unavailable");
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
  const hide = () => {
    void context.suspend().catch(console.warn);
    if (navigator.audioSession) navigator.audioSession.type = "auto";
  };
  const visibility = () => {
    if (document.hidden) {
      hide();
    } else {
      if (navigator.audioSession) navigator.audioSession.type = "playback";
      if (!fatal) void context.resume().catch(console.warn);
    }
  };
  const gesture = () => {
    if (!document.hidden && !fatal && context.state !== "running")
      void context.resume().catch(() => {});
  };
  document.addEventListener("visibilitychange", visibility, true);
  window.addEventListener("pagehide", hide, true);
  window.addEventListener("pageshow", visibility, true);
  window.addEventListener("pointerup", gesture, true);
  window.addEventListener("touchend", gesture, true);
  window.addEventListener("keydown", gesture, true);
  return {
    sampleRate: context.sampleRate,
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
      worker.postMessage({ type: "sample", id, sampleRate, samples }, [
        samples.buffer,
      ]);
      return true;
    },
    async close() {
      if (closed) return;
      closed = true;
      queue.clear();
      playing.clear();
      document.removeEventListener("visibilitychange", visibility, true);
      window.removeEventListener("pagehide", hide, true);
      window.removeEventListener("pageshow", visibility, true);
      window.removeEventListener("pointerup", gesture, true);
      window.removeEventListener("touchend", gesture, true);
      window.removeEventListener("keydown", gesture, true);
      worker.terminate();
      node.disconnect();
      if (navigator.audioSession) navigator.audioSession.type = "auto";
      await context.close();
    },
  };
}

let installing = false;
/** Install one page-wide producer bridge; await this before starting application WASM. */
export async function installWorkerAudio(options) {
  if (installing || globalThis.__tunesWorkerAttach)
    throw new Error(
      "Tunes worker audio is already installed; close it before installing another",
    );
  installing = true;
  let audio;
  try {
    audio = await createWorkerAudio(options);
  } finally {
    installing = false;
  }
  const bindings = {
    __tunesWorkerAttach: () => audio.attach(),
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
  console.info("[tunes] Worker output ready", {
    protocol: 2,
    sampleRate: audio.sampleRate,
    bufferedFrames: 2048,
  });
  return audio;
}

// Compatibility with launchers written before the standalone browser API.
export const installGameAudio = installWorkerAudio;
