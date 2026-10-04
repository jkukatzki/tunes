export interface AudioPreferenceStorage { storageKey: string; path?: string[] }
export interface Progress { loaded: number; total: number | null }
export interface BrowserAudio {
  close(): Promise<void>;
  setLatencyHint(hint: AudioContextLatencyCategory): boolean;
  preferenceStatus(): { applied: string; requested: string; changing: boolean; stage: string; error: string | null };
  bufferStatus(): { backend: 'worklet'; blockFrames: number; baseLatency: number | null; outputLatency: number | null };
}
export function readLatencyHint(options: AudioPreferenceStorage): AudioContextLatencyCategory;
export function foregroundAudioSession(): () => void;
export function createAudioHost(options?: { latencyHint?: AudioContextLatencyCategory }): {
  readonly context: AudioContext;
  install(bridgeUrl: string): Promise<BrowserAudio>;
  close(): Promise<void>;
};
export class WasmInitializationError extends Error { constructor(cause: unknown); }
export function createWasmInitializer(): (initialize: () => unknown | Promise<unknown>) => Promise<void>;
export function downloadWasm(url: string, onProgress?: (progress: Progress) => void): Promise<WebAssembly.Module>;
