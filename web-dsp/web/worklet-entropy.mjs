// getrandom needs cryptographic seed bytes, but AudioWorkletGlobalScope may not
// provide crypto. Supply bounded, pre-generated Web Crypto entropy from the host.
// Never substitute a predictable PRNG for getrandom.
export class WorkletEntropy {
  constructor() {
    this.current = null;
    this.next = null;
    this.offset = 0;
    this.pending = false;
  }
  add(bytes) {
    if (!(bytes instanceof Uint8Array) || bytes.length !== 65536 || this.next)
      throw new Error("Invalid/excess worklet entropy");
    if (!this.current) this.current = bytes;
    else this.next = bytes;
    this.pending = false;
  }
  fill(target) {
    if (!target.length) return target;
    const available =
      (this.current?.length ?? 0) - this.offset + (this.next?.length ?? 0);
    if (target.length > available)
      throw new Error("Worklet entropy exhausted; host is not responding");
    let written = 0;
    while (written < target.length) {
      if (this.offset === this.current.length) {
        this.current = this.next;
        this.next = null;
        this.offset = 0;
      }
      const count = Math.min(
        target.length - written,
        this.current.length - this.offset,
      );
      target.set(
        this.current.subarray(this.offset, this.offset + count),
        written,
      );
      this.offset += count;
      written += count;
    }
    if (
      !this.next &&
      this.current.length - this.offset < 32768 &&
      !this.pending
    ) {
      this.pending = true;
      this.request?.();
    }
    return target;
  }
}
