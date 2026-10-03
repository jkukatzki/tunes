const bindingsUrl = new URL("./tunes_web_dsp.js", import.meta.url);
bindingsUrl.search = new URL(import.meta.url).search;
const { default: init, DspWorker } = await import(bindingsUrl.href);
let dsp, port;
let session = 0,
  sequence = 0;
let renderCount = 0,
  maxRenderMs = 0,
  totalRenderMs = 0,
  rejected = 0;
let reportPending = false,
  statusPending = false,
  monitor = false;
function status(extra = {}) {
  return {
    type: "status",
    session,
    sequence,
    playing: dsp.playing_ids(),
    ...extra,
  };
}
function render(packet) {
  const start = performance.now();
  packet.samples.set(dsp.render());
  const elapsed = performance.now() - start;
  maxRenderMs = Math.max(maxRenderMs, elapsed);
  totalRenderMs += elapsed;
  renderCount++;
  if (renderCount % 4 === 0 && !statusPending) {
    statusPending = true;
    postMessage(status(monitor ? { samples: packet.samples.slice() } : {}));
  }
  packet.type = "pcm";
  packet.session = session;
  port.postMessage(packet, [packet.samples.buffer]);
  dsp.collect_garbage();
  if (renderCount % 100 === 0 && !reportPending) {
    reportPending = true;
    postMessage({
      type: "timing",
      renderCount,
      maxRenderMs,
      meanRenderMs: totalRenderMs / renderCount,
      rejected,
    });
  }
}
const methods = new Set([
  "note",
  "release",
  "stop_all",
  "effects",
  "play_sample",
  "remove_sample",
]);
onmessage = async ({ data }) => {
  try {
    if (data.type === "timing-ack") {
      reportPending = false;
      return;
    }
    if (data.type === "status-ack") {
      statusPending = false;
      return;
    }
    if (data.type === "init" && !dsp) {
      await init({ module_or_path: data.wasmUrl });
      dsp = new DspWorker(data.sampleRate);
      if (dsp.protocol_version() !== 2)
        throw new Error(
          "DSP protocol mismatch; rebuild the game and audio module together",
        );
      port = data.port;
      port.onmessage = ({ data: packet }) => {
        if (
          packet.type === "recycle" &&
          packet.samples instanceof Float32Array &&
          packet.samples.length === 1024
        ) {
          try {
            render(packet);
          } catch (error) {
            postMessage({ type: "fatal", message: String(error) });
          }
        }
      };
      port.start();
      for (let i = 0; i < 4; i++) render({ samples: new Float32Array(1024) });
      postMessage({ type: "ready", protocol: 2 });
    } else if (data.type === "batch" && dsp) {
      if (!Array.isArray(data.packets) || data.packets.length > 64)
        throw new Error("Invalid audio batch");
      const errors = [];
      for (const p of data.packets) {
        try {
          if (p.kind === "reset") {
            session = p.session;
            dsp.reset();
            sequence = p.seq;
            monitor = false;
          } else if (p.session !== session) continue;
          else if (p.kind === "wire") dsp.submit(p.json);
          else if (p.kind === "pcm") dsp.register_pcm(p.key, p.samples);
          else if (p.kind === "remove-pcm") dsp.remove_pcm(p.key);
          else if (p.kind === "monitor") monitor = p.enabled;
          else if (p.kind === "legacy") {
            if (!methods.has(p.method) || !p.args.every(Number.isFinite))
              throw new Error("Invalid test command");
            if (dsp[p.method](...p.args) === false)
              throw new Error("Test command rejected");
          } else throw new Error("Unknown audio packet");
        } catch (error) {
          rejected++;
          if (errors.length < 8) errors.push(String(error));
        }
        sequence = Math.max(sequence, p.seq);
      }
      postMessage({ ...status(), type: "ack", errors });
    } else if (data.type === "sample" && dsp) {
      dsp.register_sample(data.id, data.samples, data.sampleRate);
      postMessage({ type: "sample-ready", id: data.id });
    }
  } catch (error) {
    postMessage({ type: "fatal", message: String(error) });
  }
};
