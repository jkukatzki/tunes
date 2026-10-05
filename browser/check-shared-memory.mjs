import { readFileSync } from 'node:fs';

// Inspect only import/memory sections; never instantiate or execute the game.
export function assertSharedMemory(bytes) {
  let offset = 0;
  const byte = () => {
    if (offset >= bytes.length) throw new Error('Truncated WASM');
    return bytes[offset++];
  };
  const uint = () => {
    let value = 0, shift = 0, part;
    do {
      part = byte();
      value += (part & 127) * 2 ** shift;
      shift += 7;
      if (shift > 35) throw new Error('Expected wasm32 integer');
    } while (part & 128);
    return value;
  };
  const name = () => { const length = uint(); offset += length; };
  const limits = () => {
    const flags = uint();
    if (flags & ~3) throw new Error('Unsupported WASM memory/table limits');
    uint();
    if (flags & 1) uint();
    return flags;
  };
  const reference = () => {
    const type = byte();
    if (type === 0x63 || type === 0x64) uint(); // typed reference heap type
  };
  const memories = [];
  for (const expected of [0, 97, 115, 109, 1, 0, 0, 0]) {
    if (byte() !== expected) throw new Error('Invalid WASM header');
  }
  while (offset < bytes.length) {
    const section = byte(), size = uint(), end = offset + size;
    if (end > bytes.length) throw new Error('Truncated WASM section');
    if (section === 2) {
      const count = uint();
      for (let i = 0; i < count; i++) {
        name(); name();
        switch (byte()) {
          case 0: uint(); break; // function
          case 1: reference(); limits(); break; // table
          case 2: memories.push({ imported: true, flags: limits() }); break;
          case 3: reference(); byte(); break; // global
          case 4: byte(); uint(); break; // exception tag
          default: throw new Error('Unsupported WASM import kind');
        }
      }
    } else if (section === 5) {
      const count = uint();
      for (let i = 0; i < count; i++) memories.push({ imported: false, flags: limits() });
    }
    if (offset > end) throw new Error('Invalid WASM section length');
    offset = end;
  }
  if (memories.length !== 1 || !memories[0].imported || memories[0].flags !== 3) {
    throw new Error('Threaded game must import shared WASM memory with a maximum; check --shared-memory, --import-memory and --max-memory linker flags');
  }
}

export function checkSharedMemoryFile(path) {
  assertSharedMemory(readFileSync(path));
  console.info(`[WASM] Verified imported shared memory: ${path}`);
}
