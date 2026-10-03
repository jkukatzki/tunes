/** Diagnostics are sampled; they are not a lossless audio capture interface. */
export type AudioHealth =
  | {
      type: "health";
      renderedFrames: number;
      backend?: "worklet";
      /** CPU callback overruns; hardware dropouts are not observable here. */
      callbackOverruns?: number;
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
      maxCallbackMs: number;
      blockBudgetMs: number;
    }
  | { type: "error"; message: string };
export interface WorkerAudioOptions {
  /** Create/resume during a user gesture before awaiting downloads. Ownership transfers to Tunes. */
  context?: AudioContext;
  /** Requested hint of a supplied context; used to avoid unnecessary restarts. */
  latencyHint?: AudioContextLatencyCategory;
  /** Compatibility option; AudioWorklet is the only browser backend. */
  backend?: "worklet";
  /** Opt in to worklet timing/health measurements; disabled in normal playback. */
  onHealth?: (report: AudioHealth) => void;
}
export interface WorkerAudio {
  readonly sampleRate: number;
  setLatencyHint(hint: AudioContextLatencyCategory): boolean;
  preferenceStatus(): { applied: string; requested: string; changing: boolean; stage: string; error: string | null };
  bufferStatus(): {
    backend: "worklet";
    blockFrames: number;
    baseLatency: number | null;
    outputLatency: number | null;
  };
  /** Stop DSP, release listeners and close the supplied/created AudioContext. Safe to call twice. */
  close(): Promise<void>;
}
/** Install the page's single Rust AudioEngine bridge. Await before starting your application's WASM. */
export function installWorkerAudio(
  options?: WorkerAudioOptions,
): Promise<WorkerAudio>;
/** Compatibility name used by older launchers. Prefer installWorkerAudio. */
export const installGameAudio: typeof installWorkerAudio;
