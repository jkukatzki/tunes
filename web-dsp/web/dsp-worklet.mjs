// Direct DSP on the audio thread; no PCM transfer round trip.
// Factory dependencies also let the processor run against real WASM in Node tests.
export function createDspProcessor({
  initSync,
  DspWorker,
  createBlockRenderer,
  entropy,
  Base = AudioWorkletProcessor,
  rate = sampleRate,
  now = () => globalThis.performance?.now() ?? Date.now(),
}) {
  return class TunesDsp extends Base {
    constructor() {
      super();
      this.held = false;
      this.cancelledSnapshot = 0;
      this.session = 0;
      this.sequence = 0;
      this.frames = 0;
      this.reports = 0;
      this.failed = false;
      this.pending = null;
      this.pendingIndex = 0;
      this.errors = [];
      this.statusPending = false;
      this.healthPending = false;
      this.timingPending = false;
      this.monitor = false;
      this.offset = 128;
      this.renderCount = 0;
      this.totalRenderCount = 0;
      this.renderMs = 0;
      this.maxRenderMs = 0;
      this.maxCallbackMs = 0;
      this.callbackOverruns = 0;
      this.rejected = 0;
      this.port.onmessageerror = () =>
        this.fail(new Error("DSP worklet could not decode an incoming message"));
      this.port.onmessage = ({ data }) => {
        try {
          if (data.type === "init" && !this.dsp) {
            if (entropy) {
              entropy.request = () =>
                this.port.postMessage({ type: "entropy-needed" });
              entropy.add(data.entropy);
            }
            this.port.postMessage({ type: "startup", stage: "initializing DSP WASM" });
            const wasm = initSync({ module: data.module });
            this.memory = wasm.memory;
            this.dsp = new DspWorker(rate);
            this.held = !!data.held;
            this.telemetry = !!data.telemetry;
            if (
              this.dsp.protocol_version() !== 2 ||
              !this.dsp.set_block_frames ||
              !this.dsp.collect_garbage_budget || !this.dsp.playback_snapshot || !this.dsp.restore_playback || !this.dsp.flush_commands
            )
              throw new Error(
                "Rebuild the DSP module for direct worklet audio",
              );
            this.dsp.set_block_frames(128);
            this.render = createBlockRenderer(this.dsp, wasm.memory);
            this.port.postMessage({ type: "startup", stage: "warming DSP buffers" });
            // Initialize lazy tables/buffers before connecting to the output.
            this.render();
            this.dsp.collect_garbage_budget(4);
            this.port.postMessage({ type: "ready", protocol: 2 });
          } else if (data.type === "release-playback") {
            this.cancelledSnapshot = Math.max(this.cancelledSnapshot, data.cancelThrough ?? 0);
            this.held = false;
          } else if (data.type === "entropy") {
            entropy?.add(data.bytes);
          } else if (data.type === "batch") {
            if (
              this.pending ||
              !Array.isArray(data.packets) ||
              data.packets.length > 64
            )
              throw new Error("Invalid or overlapping DSP batch");
            this.pending = data.packets;
            this.pendingIndex = 0;
            this.errors = [];
          } else if (data.type === "sample") {
            if (this.upload) throw new Error("Sample upload already pending");
            this.upload = data;
          } else if (data.type === "generation") {
            this.offset = 128;
          } else if (data.type === "status-ack") this.statusPending = false;
          else if (data.type === "health-ack") this.healthPending = false;
          else if (data.type === "timing-ack") this.timingPending = false;
          else if (data.type === "close") {
            this.failed = true;
            this.dsp?.free();
          }
        } catch (error) {
          this.fail(error);
        }
      };
    }
    fail(error) {
      this.failed = true;
      this.port.postMessage({ type: "fatal", message: String(error) });
    }
    status(type = "status", extra = {}) {
      this.port.postMessage({
        type,
        session: this.session,
        sequence: this.sequence,
        playing: this.dsp.playing_ids(),
        ...extra,
      });
    }
    commands() {
      if (this.upload) {
        const data = this.upload;
        this.upload = null;
        this.dsp.register_sample(data.id, data.samples, data.sampleRate);
        this.port.postMessage({ type: "sample-ready", id: data.id });
      }
      if (!this.pending) return;
      // Bound command dispatch per audio quantum; the bridge permits one batch in flight.
      const end = Math.min(this.pendingIndex + 4, this.pending.length);
      while (this.pendingIndex < end) {
        const p = this.pending[this.pendingIndex++];
        try {
          if (p.kind === "reset") {
            this.session = p.session;
            this.dsp.reset();
            this.offset = 128;
            this.monitor = false;
          } else if (p.session !== this.session) continue;
          else if (p.kind === "playback-snapshot") {
            if (p.request <= this.cancelledSnapshot) continue;
            this.held = true;
            this.dsp.flush_commands();
            this.port.postMessage({ type: "playback-reply", request: p.request,
              snapshot: this.dsp.playback_snapshot(p.ids) });
          } else if (p.kind === "restore-playback") {
            this.dsp.flush_commands();
            this.dsp.restore_playback(p.snapshot);
            this.port.postMessage({ type: "playback-reply", request: p.request });
          } else if (p.kind === "wire") this.dsp.submit(p.json);
          else if (p.kind === "pcm") this.dsp.register_pcm(p.key, p.samples);
          else if (p.kind === "remove-pcm") this.dsp.remove_pcm(p.key);
          else if (p.kind === "monitor") this.monitor = p.enabled;
          else if (p.kind === "legacy") {
            if (
              ![
                "note",
                "release",
                "stop_all",
                "effects",
                "play_sample",
                "remove_sample",
              ].includes(p.method) ||
              !p.args.every(Number.isFinite)
            )
              throw new Error("Invalid test command");
            if (this.dsp[p.method](...p.args) === false)
              throw new Error("Test command rejected");
          } else throw new Error("Unknown audio packet");
        } catch (error) {
          this.rejected++;
          if (this.errors.length < 8) this.errors.push(String(error));
        }
        this.sequence = Math.max(this.sequence, p.seq);
      }
      if (this.pendingIndex === this.pending.length) {
        this.pending = null;
        this.status("ack", { errors: this.errors });
      }
    }
    process(_inputs, outputs) {
      const channels = outputs[0];
      if (!channels?.length) return !this.failed;
      for (const channel of channels) channel.fill(0);
      if (this.failed || !this.dsp) return !this.failed;
      const start = this.telemetry ? now() : 0;
      try {
        this.commands();
        if (this.held) {
          this.dsp.flush_commands();
          this.dsp.collect_garbage_budget(4);
          return true;
        }
        if (this.offset < 128 && this.samples.buffer !== this.memory.buffer)
          this.samples = new Float32Array(
            this.memory.buffer,
            this.samplePointer,
            256,
          );
        for (let frame = 0; frame < channels[0].length; frame++) {
          if (this.offset === 128) {
            const renderStart = this.telemetry ? now() : 0;
            this.samples = this.render();
            this.samplePointer = this.samples.byteOffset;
            if (this.telemetry) {
              const elapsed = now() - renderStart;
              this.renderMs += elapsed;
              this.maxRenderMs = Math.max(this.maxRenderMs, elapsed);
              this.renderCount++;
              this.totalRenderCount++;
            }
            this.offset = 0;
          }
          channels[0][frame] = this.samples[this.offset * 2];
          if (channels.length > 1)
            channels[1][frame] = this.samples[this.offset * 2 + 1];
          this.offset++;
        }
        // Keep chord releases from draining the entire retirement queue in one quantum.
        this.dsp.collect_garbage_budget(4);
        this.frames += channels[0].length;
        this.reports += channels[0].length;
        // Roughly 20 status updates/s; no per-quantum PCM transfers.
        if (
          Math.floor(this.frames / (rate / 20)) !==
            Math.floor((this.frames - channels[0].length) / (rate / 20)) &&
          !this.statusPending
        ) {
          this.statusPending = true;
          this.status(
            "status",
            this.monitor ? { samples: this.samples.slice() } : {},
          );
        }
        if (!this.telemetry) return true;
        const callbackMs = now() - start;
        this.maxCallbackMs = Math.max(this.maxCallbackMs, callbackMs);
        if (callbackMs > (channels[0].length / rate) * 1000)
          this.callbackOverruns++;
        if (this.reports >= rate) {
          this.reports = 0;
          if (!this.healthPending) {
            this.healthPending = true;
            // There is no software PCM queue to underrun in this backend.
            // Hardware/scheduler dropouts are not measurable from this counter.
            this.port.postMessage({
              type: "health",
              backend: "worklet",
              renderedFrames: this.frames,
              callbackOverruns: this.callbackOverruns,
            });
          }
          if (!this.timingPending) {
            this.timingPending = true;
            this.port.postMessage({
              type: "timing",
              backend: "worklet",
              renderCount: this.totalRenderCount,
              windowRenders: this.renderCount,
              windowRenderMs: this.renderMs,
              windowMaxRenderMs: this.maxRenderMs,
              maxRenderMs: this.maxRenderMs,
              meanRenderMs: this.renderMs / this.renderCount,
              maxCallbackMs: this.maxCallbackMs,
              blockBudgetMs: (128 / rate) * 1000,
              rejected: this.rejected,
            });
            this.renderCount =
              this.renderMs =
              this.maxRenderMs =
              this.maxCallbackMs =
                0;
          }
        }
      } catch (error) {
        for (const channel of channels) channel.fill(0);
        this.fail(error);
      }
      return !this.failed;
    }
  };
}
