// DSP lives in a dedicated Rust/WASM worker. This node only consumes PCM.
class TunesOutput extends AudioWorkletProcessor {
  constructor() {
    super();
    this.session = 0;
    this.blocks = new Array(4).fill(null);
    this.read = 0;
    this.count = 0;
    this.offset = 0;
    this.underrunFrames = 0;
    this.renderedFrames = 0;
    this.reportFrames = 0;
    this.reportPending = false;
    this.port.onmessage = ({ data }) => {
      if (data.type === "generation") {
        this.session = data.session;
        while (this.count) {
          const packet = this.blocks[this.read];
          this.blocks[this.read] = null;
          this.read = (this.read + 1) % 4;
          this.count--;
          packet.type = "recycle";
          this.audioPort.postMessage(packet, [packet.samples.buffer]);
        }
        this.offset = 0;
        return;
      }
      if (data.type === "health-ack") {
        this.reportPending = false;
        return;
      }
      if (data.type !== "connect" || this.audioPort) return;
      this.audioPort = data.port;
      this.audioPort.onmessage = ({ data: packet }) => {
        if (
          packet.type !== "pcm" ||
          !(packet.samples instanceof Float32Array) ||
          packet.samples.length !== 1024
        )
          return;
        if (
          (packet.session ?? 0) !== this.session ||
          this.count === this.blocks.length
        ) {
          // The producer owns exactly four transferable blocks, so overflow
          // indicates a protocol error. Return the buffer instead of growing.
          packet.type = "recycle";
          this.audioPort.postMessage(packet, [packet.samples.buffer]);
          return;
        }
        this.blocks[(this.read + this.count) % this.blocks.length] = packet;
        this.count++;
      };
      this.audioPort.start();
    };
  }
  process(_inputs, outputs) {
    const channels = outputs[0];
    if (!channels?.length) return true;
    for (const channel of channels) channel.fill(0);
    for (let frame = 0; frame < channels[0].length; frame++) {
      if (!this.count) {
        this.underrunFrames++;
        continue;
      }
      const packet = this.blocks[this.read];
      channels[0][frame] = packet.samples[this.offset * 2];
      if (channels.length > 1)
        channels[1][frame] = packet.samples[this.offset * 2 + 1];
      this.offset++;
      if (this.offset === 512) {
        this.blocks[this.read] = null;
        this.read = (this.read + 1) % this.blocks.length;
        this.count--;
        this.offset = 0;
        packet.type = "recycle";
        this.audioPort.postMessage(packet, [packet.samples.buffer]);
      }
    }
    this.renderedFrames += channels[0].length;
    this.reportFrames += channels[0].length;
    if (this.reportFrames >= sampleRate && !this.reportPending) {
      this.reportPending = true;
      this.port.postMessage({
        type: "health",
        renderedFrames: this.renderedFrames,
        underrunFrames: this.underrunFrames,
        queuedBlocks: this.count,
      });
      this.reportFrames = 0;
    }
    return true;
  }
}
registerProcessor("tunes-output", TunesOutput);
