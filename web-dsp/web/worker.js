const bindingsUrl = new URL("./tunes_web_dsp.js", import.meta.url);
bindingsUrl.search = new URL(import.meta.url).search;
const rendererUrl = new URL("./render-buffer.mjs", import.meta.url);
rendererUrl.search = new URL(import.meta.url).search;
let renderBlock;
let blockFrames = 512,
  sampleRate;
let initializing = false;
let startupStage = "waiting for init";
function startup(stage) {
  startupStage = stage;
  postMessage({ type: "startup", stage });
}
let dsp, port;
let targetBuffers = 4,
  maxBuffers = 4,
  circulatingBuffers = 0;
const parkedBuffers = [];
function fillBufferPool() {
  while (circulatingBuffers < targetBuffers && parkedBuffers.length) {
    circulatingBuffers++;
    render(parkedBuffers.pop());
  }
}
let session = 0,
  sequence = 0;
let renderCount = 0,
  maxRenderMs = 0,
  totalRenderMs = 0,
  rejected = 0;
let previousRenderStart;
let windowRenders = 0,
  windowRenderMs = 0,
  windowMaxRenderMs = 0;
let maxRenderGapMs = 0,
  maxCallbackMs = 0,
  maxBatchMs = 0;
let blockBudgetMs = 0;
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
  if (previousRenderStart !== undefined)
    maxRenderGapMs = Math.max(maxRenderGapMs, start - previousRenderStart);
  previousRenderStart = start;
  if (packet.samples.length !== blockFrames * 2)
    packet.samples = new Float32Array(blockFrames * 2);
  packet.samples.set(renderBlock());
  const elapsed = performance.now() - start;
  maxRenderMs = Math.max(maxRenderMs, elapsed);
  totalRenderMs += elapsed;
  renderCount++;
  windowRenders++;
  windowRenderMs += elapsed;
  windowMaxRenderMs = Math.max(windowMaxRenderMs, elapsed);
  if (renderCount % 4 === 0 && !statusPending) {
    statusPending = true;
    postMessage(status(monitor ? { samples: packet.samples.slice() } : {}));
  }
  packet.type = "pcm";
  packet.session = session;
  port.postMessage(packet, [packet.samples.buffer]);
  dsp.collect_garbage();
  maxCallbackMs = Math.max(maxCallbackMs, performance.now() - start);
  if (renderCount % 100 === 0 && !reportPending) {
    reportPending = true;
    postMessage({
      type: "timing",
      renderCount,
      maxRenderMs,
      meanRenderMs: totalRenderMs / renderCount,
      rejected,
      windowRenders,
      windowRenderMs,
      windowMaxRenderMs,
      maxRenderGapMs,
      maxCallbackMs,
      maxBatchMs,
      blockBudgetMs,
    });
    windowRenders = windowRenderMs = windowMaxRenderMs = 0;
    maxRenderGapMs = maxCallbackMs = maxBatchMs = 0;
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
    if (data.type === "init" && !dsp && !initializing) {
      const bufferBlocks = data.bufferBlocks ?? 4;
      if (
        !Number.isInteger(bufferBlocks) ||
        bufferBlocks < 4 ||
        bufferBlocks > 32
      )
        throw new Error("Invalid audio buffer count");
      maxBuffers = data.maxBufferBlocks ?? bufferBlocks;
      if (
        !Number.isInteger(maxBuffers) ||
        maxBuffers < bufferBlocks ||
        maxBuffers > 32
      )
        throw new Error("Invalid maximum audio buffer count");
      targetBuffers = bufferBlocks;
      initializing = true;
      sampleRate = data.sampleRate;
      blockBudgetMs = (blockFrames / sampleRate) * 1000;
      // Install the message handler before any asynchronous module loading.
      startup("loading DSP bindings");
      const { default: init, DspWorker } = await import(bindingsUrl.href);
      startup("loading output renderer");
      const { createBlockRenderer } = await import(rendererUrl.href);
      startup("loading and compiling DSP WASM");
      const wasm = await init({ module_or_path: data.wasmUrl });
      startup("creating DSP engine");
      dsp = new DspWorker(data.sampleRate);
      renderBlock = createBlockRenderer(dsp, wasm.memory);
      if (dsp.protocol_version() !== 2)
        throw new Error(
          "DSP protocol mismatch; rebuild the game and audio module together",
        );
      port = data.port;
      port.onmessage = ({ data: packet }) => {
        if (
          packet.type === "recycle" &&
          packet.samples instanceof Float32Array &&
          [1024, 2048, 4096, 8192, 16384].includes(packet.samples.length)
        ) {
          try {
            if (circulatingBuffers > targetBuffers) {
              circulatingBuffers--;
              parkedBuffers.push(packet);
            } else render(packet);
          } catch (error) {
            postMessage({ type: "fatal", message: String(error) });
          }
        }
      };
      port.start();
      startup("rendering initial audio buffers");
      for (let i = 0; i < maxBuffers; i++)
        parkedBuffers.push({ samples: new Float32Array(1024) });
      fillBufferPool();
      postMessage({ type: "ready", protocol: 2 });
    } else if (data.type === "block-frames" && dsp) {
      if (![512, 1024, 2048, 4096, 8192].includes(data.frames))
        throw new Error("Invalid DSP block size");
      if (typeof dsp.set_block_frames !== "function")
        throw new Error("Rebuild DSP WASM to enable variable block sizes");
      dsp.set_block_frames(data.frames);
      blockFrames = data.frames;
      blockBudgetMs = (blockFrames / sampleRate) * 1000;
      postMessage({ type: "block-frames-ready", frames: blockFrames });
    } else if (data.type === "buffer-target" && dsp) {
      if (
        !Number.isInteger(data.bufferBlocks) ||
        data.bufferBlocks < 4 ||
        data.bufferBlocks > maxBuffers
      )
        throw new Error("Invalid audio buffer target");
      targetBuffers = data.bufferBlocks;
      fillBufferPool();
    } else if (data.type === "batch" && dsp) {
      if (!Array.isArray(data.packets) || data.packets.length > 64)
        throw new Error("Invalid audio batch");
      const batchStart = performance.now();
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
      maxBatchMs = Math.max(maxBatchMs, performance.now() - batchStart);
    } else if (data.type === "sample" && dsp) {
      dsp.register_sample(data.id, data.samples, data.sampleRate);
      postMessage({ type: "sample-ready", id: data.id });
    }
  } catch (error) {
    postMessage({
      type: "fatal",
      message: `${startupStage}: ${String(error)}`,
    });
  }
};
