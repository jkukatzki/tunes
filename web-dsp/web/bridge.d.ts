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
    }
  | { type: "error"; message: string };
export interface WorkerAudioOptions {
  /** Create/resume during a user gesture before awaiting downloads. Ownership transfers to Tunes. */
  context?: AudioContext;
  onHealth?: (report: AudioHealth) => void;
}
export interface WorkerAudio {
  readonly sampleRate: number;
  /** Stop DSP, release listeners and close the supplied/created AudioContext. Safe to call twice. */
  close(): Promise<void>;
}
/** Install the page's single Rust AudioEngine bridge. Await before starting your application's WASM. */
export function installWorkerAudio(
  options?: WorkerAudioOptions,
): Promise<WorkerAudio>;
/** Compatibility name used by older launchers. Prefer installWorkerAudio. */
export const installGameAudio: typeof installWorkerAudio;
