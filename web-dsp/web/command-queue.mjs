// One in-flight batch. FIFO barriers preserve attack/release ordering.
export class AudioCommandQueue {
  constructor(send, cancelPlay) {
    this.send = send;
    this.cancelPlay = cancelPlay;
    this.pending = [];
    this.busy = false;
    this.bytes = 0;
    this.sequence = 0;
  }
  enqueue(packet, { critical = false, key = null } = {}) {
    packet.seq = ++this.sequence;
    packet.coalesceKey = key;
    if (key) {
      for (let i = this.pending.length - 1; i >= 0; i--) {
        if (!this.pending[i].coalesceKey) break;
        if (this.pending[i].coalesceKey === key) {
          const bytes =
            this.bytes +
            (packet.json?.length ?? 0) * 2 -
            (this.pending[i].json?.length ?? 0) * 2;
          if (bytes > 128 * 1024 * 1024) return false;
          this.bytes = bytes;
          this.pending[i] = packet;
          this.flush();
          return packet.seq;
        }
      }
    }
    const bytes = packet.samples?.byteLength ?? (packet.json?.length ?? 0) * 2;
    if (
      this.pending.length >= (critical ? 384 : 256) ||
      this.bytes + bytes > 128 * 1024 * 1024
    )
      return false;
    this.pending.push(packet);
    this.bytes += bytes;
    this.flush();
    return packet.seq;
  }
  emergencyStop(packet) {
    // Keep registrations and bus lifecycle ordering; cancel attacks and controls.
    this.pending = this.pending.filter((p) => {
      if (p.playId) this.cancelPlay(p.playId);
      return ["pcm", "remove-pcm", "reset"].includes(p.kind) || p.busLifecycle;
    });
    this.bytes = this.pending.reduce(
      (n, p) => n + (p.samples?.byteLength ?? (p.json?.length ?? 0) * 2),
      0,
    );
    // One dedicated emergency slot, even when registrations fill the queue.
    this.pending = this.pending.filter((p) => !p.emergency);
    this.bytes = this.pending.reduce(
      (n, p) => n + (p.samples?.byteLength ?? (p.json?.length ?? 0) * 2),
      0,
    );
    packet.seq = ++this.sequence;
    packet.emergency = true;
    packet.coalesceKey = null;
    this.pending.push(packet);
    this.bytes += (packet.json?.length ?? 0) * 2;
    this.flush();
    return packet.seq;
  }
  flush() {
    if (this.busy || !this.pending.length) return;
    this.busy = true;
    const packets = this.pending.splice(0, 64);
    for (const packet of packets)
      this.bytes -=
        packet.samples?.byteLength ?? (packet.json?.length ?? 0) * 2;
    this.send(packets);
  }
  ack() {
    this.busy = false;
    this.flush();
  }
  clear() {
    this.pending = [];
    this.bytes = 0;
  }
}
