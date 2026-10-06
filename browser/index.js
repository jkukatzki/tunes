/** Safe to import during SSR. Browser globals are accessed only when called. */
export function readLatencyHint({ storageKey, path = [] } = {}) {
  try {
    let value = JSON.parse(window.localStorage.getItem(storageKey) ?? 'null');
    for (const key of path) value = value?.[key];
    if (['interactive', 'balanced', 'playback'].includes(value)) return value;
  } catch { /* Storage may be unavailable or contain an older settings schema. */ }
  return 'interactive';
}

/** Acquire media priority only in the foreground, including during downloads. */
export function foregroundAudioSession() {
  const set = type => { try { if (navigator.audioSession) navigator.audioSession.type = type; } catch {} };
  const sync = () => set(document.visibilityState === 'visible' ? 'playback' : 'auto');
  const hide = () => set('auto');
  document.addEventListener('visibilitychange', sync, true);
  window.addEventListener('pageshow', sync, true);
  window.addEventListener('pagehide', hide, true);
  sync();
  return () => {
    document.removeEventListener('visibilitychange', sync, true);
    window.removeEventListener('pageshow', sync, true);
    window.removeEventListener('pagehide', hide, true);
    hide();
  };
}

/** Call synchronously from Play, before any await, to unlock Safari audio. */
export function createAudioHost({ latencyHint = 'interactive' } = {}) {
  const releaseSession = foregroundAudioSession();
  let context;
  try { context = new AudioContext({ latencyHint }); }
  catch (error) { releaseSession(); throw error; }
  void context.resume().catch(() => {});
  let installed, closed = false;
  return {
    get context() { return context; },
    install(bridgeUrl) {
      if (closed) return Promise.reject(new Error('Audio host is closed'));
      return installed ??= (async () => {
        const bridge = await import(/* @vite-ignore */ bridgeUrl);
        if (closed) throw new Error('Audio host is closed');
        return bridge.installWorkerAudio({ context, latencyHint });
      })();
    },
    async close() {
      if (closed) return;
      closed = true;
      releaseSession();
      try { const audio = await installed; if (audio) await audio.close(); }
      finally { if (context.state !== 'closed') await context.close(); }
    },
  };
}

export class WasmInitializationError extends Error {
  constructor(cause) {
    const detail = cause instanceof Error ? cause.message : String(cause);
    const recovery = /out of memory|memory allocation failed/i.test(detail)
      ? 'Close the app or browser completely and reopen it; reloading may not release enough memory.'
      : 'Reload the page to try again.';
    super(`Application initialization failed: ${detail}. ${recovery}`, { cause });
    this.name = 'WasmInitializationError';
  }
}
/** wasm-bindgen retains failed instances too: never retry initialization in-page. */
export function createWasmInitializer() {
  let initialization;
  return initialize => initialization ??= Promise.resolve().then(async () => {
    try { await initialize(); } catch (cause) { throw new WasmInitializationError(cause); }
  });
}

export async function downloadWasm(url, onProgress = () => {}) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`Could not download WASM (${response.status} ${response.statusText})`);
  const declared = Number(response.headers.get('content-length'));
  const total = declared > 0 ? declared : null;
  const reader = response.body?.getReader();
  if (!reader) {
    const bytes = await response.arrayBuffer();
    onProgress({ loaded: bytes.byteLength, total: bytes.byteLength });
    return WebAssembly.compile(bytes);
  }
  // Feed the compiler with backpressure instead of retaining every chunk and
  // allocating a second complete copy of a potentially 100+ MB module.
  if (typeof WebAssembly.compileStreaming === 'function') {
    let loaded = 0;
    let completed = false;
    const body = new ReadableStream({
      async pull(controller) {
        const { done, value } = await reader.read();
        if (done) {
          completed = true;
          controller.close();
        } else {
          loaded += value.byteLength;
          onProgress({ loaded, total });
          controller.enqueue(value);
        }
      },
      cancel(reason) { return reader.cancel(reason); },
    });
    try {
      // The bytes are already decoded by fetch. Normalize the MIME type for
      // cached responses and servers serving WASM as application/octet-stream.
      return await WebAssembly.compileStreaming(new Response(body, {
        headers: { 'Content-Type': 'application/wasm' },
      }));
    } finally {
      if (!completed) await reader.cancel().catch(() => {});
      reader.releaseLock();
    }
  }
  const chunks = [];
  let loaded = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      chunks.push(value); loaded += value.byteLength;
      onProgress({ loaded, total });
    }
  } finally { reader.releaseLock(); }
  const bytes = new Uint8Array(loaded);
  let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
  chunks.length = 0;
  return WebAssembly.compile(bytes);
}
