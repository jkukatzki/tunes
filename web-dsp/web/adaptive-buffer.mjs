// Decisions use audio frames, not wall time: background pauses must not earn
// clean-playback credit. The caller also resets observation across visibility changes.
export class AdaptiveBuffer {
  constructor(blocks, sampleRate, policy = "balanced") {
    this.blocks = blocks;
    this.sampleRate = sampleRate;
    this.maxBlocks = 32;
    this.setPolicy(policy);
  }
  setBlockFrames(frames) {
    // Limit adaptive lookahead to roughly 350 ms, subject to the four-block floor.
    this.maxBlocks = Math.max(
      4,
      Math.min(32, Math.ceil((this.sampleRate * 0.35) / frames)),
    );
    this.blocks = Math.min(this.blocks, this.maxBlocks);
    this.reset();
  }
  setPolicy(policy) {
    const profiles = {
      conservative: [60, 1],
      balanced: [30, 2],
      optimistic: [10, 2],
    };
    if (!Object.hasOwn(profiles, policy))
      throw new Error("Unknown buffering policy");
    if (this.policy === policy) return;
    this.policy = policy;
    [this.cleanSeconds, this.shrinkBlocks] = profiles[policy];
    this.reset();
  }
  reset() {
    this.previous = undefined;
    this.grace = this.sampleRate * 3;
    this.clean = 0;
  }
  observe(health, active) {
    if (!active) {
      this.reset();
      return;
    }
    const previous = this.previous;
    this.previous = health;
    if (!previous) return;
    const frames = health.renderedFrames - previous.renderedFrames;
    const missing = health.underrunFrames - previous.underrunFrames;
    if (frames <= 0 || frames > this.sampleRate * 5 || missing < 0) {
      this.reset();
      return;
    }
    if (this.grace > 0) {
      this.grace -= frames;
      return;
    }
    if (missing > 0) {
      this.clean = 0;
      const next = Math.min(
        this.maxBlocks,
        Math.max(this.blocks + 4, Math.ceil(this.blocks * 1.5)),
      );
      if (next === this.blocks) return;
      this.blocks = next;
      this.grace = this.sampleRate * 3;
      return { blocks: next, reason: "output underrun" };
    }
    this.clean += frames;
    if (this.clean >= this.sampleRate * this.cleanSeconds && this.blocks > 4) {
      this.blocks = Math.max(4, this.blocks - this.shrinkBlocks);
      this.clean = 0;
      this.grace = this.sampleRate * 3;
      return {
        blocks: this.blocks,
        reason: `${this.cleanSeconds} seconds of clean playback`,
      };
    }
  }
}
