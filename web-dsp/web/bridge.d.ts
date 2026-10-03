/** Diagnostics are sampled; they are not a lossless audio capture interface. */
export type AudioHealth =
  | {
      type: "health";
      renderedFrames: number;
      underrunFrames: number;
      queuedBlocks: number;
    }
  | {
      type: "timing";
      renderCount: number;
      maxRenderMs: number;
      meanRenderMs: number;
      rejected: number;
      windowRenders: number;
      windowRenderMs: number;
      windowMaxRenderMs: number;
      maxRenderGapMs: number;
      maxCallbackMs: number;
      maxBatchMs: number;
      blockBudgetMs: number;
    }
  | { type: "error"; message: string };
export interface WorkerAudioOptions {
  /** Create/resume during a user gesture before awaiting downloads. Ownership transfers to Tunes. */
  context?: AudioContext;
  /** Log aggregated playback diagnostics about every five seconds. */
  diagnostics?: boolean;
  /** Buffer pool, 4–32 blocks, initially 512 frames per block. Default 4. Larger pools add input latency. */
  bufferBlocks?: number;
  /** Adapt the live pool within 4–32 blocks using output underruns. Default false. */
  adaptiveBuffering?: boolean;
  bufferingPolicy?: "conservative" | "balanced" | "optimistic";
  onHealth?: (report: AudioHealth) => void;
}
export interface WorkerAudio {
  readonly sampleRate: number;
  bufferStatus(): {
    blocks: number;
    blockFrames: number;
    adaptive: boolean;
    policy: string;
  };
  setBlockFrames(frames: 512 | 1024 | 2048 | 4096 | 8192): void;
  setBufferingPolicy(policy: "conservative" | "balanced" | "optimistic"): void;
  /** Stop DSP, release listeners and close the supplied/created AudioContext. Safe to call twice. */
  close(): Promise<void>;
}
/** Install the page's single Rust AudioEngine bridge. Await before starting your application's WASM. */
export function installWorkerAudio(
  options?: WorkerAudioOptions,
): Promise<WorkerAudio>;
/** Compatibility name used by older launchers. Prefer installWorkerAudio. */
export const installGameAudio: typeof installWorkerAudio;
