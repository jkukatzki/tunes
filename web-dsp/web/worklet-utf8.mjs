// AudioWorkletGlobalScope does not consistently expose the Encoding API.
// The generated wasm-bindgen glue needs non-streaming UTF-8 strings only.
export class WorkletTextEncoder {
  encodeInto(text, output) {
    let read = 0,
      written = 0;
    for (const char of text) {
      let code = char.codePointAt(0);
      if (code >= 0xd800 && code <= 0xdfff) code = 0xfffd;
      const size = code < 0x80 ? 1 : code < 0x800 ? 2 : code < 0x10000 ? 3 : 4;
      if (written + size > output.length) break;
      if (size === 1) output[written++] = code;
      else {
        output[written++] =
          (size === 2 ? 0xc0 : size === 3 ? 0xe0 : 0xf0) |
          (code >> (6 * (size - 1)));
        for (let shift = 6 * (size - 2); shift >= 0; shift -= 6)
          output[written++] = 0x80 | ((code >> shift) & 63);
      }
      read += char.length;
    }
    return { read, written };
  }
  encode(text = "") {
    const bytes = new Uint8Array(text.length * 3);
    return bytes.subarray(0, this.encodeInto(text, bytes).written);
  }
}
export class WorkletTextDecoder {
  constructor(_encoding, options = {}) {
    this.ignoreBOM = options.ignoreBOM;
  }
  decode(input = new Uint8Array()) {
    const bytes =
      input instanceof ArrayBuffer
        ? new Uint8Array(input)
        : new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
    let result = "";
    for (let i = 0; i < bytes.length; ) {
      const first = bytes[i++];
      const count =
        first < 0x80
          ? 0
          : first >= 0xc2 && first <= 0xdf
            ? 1
            : first >= 0xe0 && first <= 0xef
              ? 2
              : first >= 0xf0 && first <= 0xf4
                ? 3
                : -1;
      if (count < 0 || i + count > bytes.length)
        throw new TypeError("Invalid UTF-8");
      let code = first & (count === 0 ? 0x7f : (1 << (6 - count)) - 1);
      for (let j = 0; j < count; j++) {
        const next = bytes[i++];
        if ((next & 0xc0) !== 0x80) throw new TypeError("Invalid UTF-8");
        code = (code << 6) | (next & 63);
      }
      if (
        (count && code < [0, 0x80, 0x800, 0x10000][count]) ||
        code > 0x10ffff ||
        (code >= 0xd800 && code <= 0xdfff)
      )
        throw new TypeError("Invalid UTF-8");
      if (code !== 0xfeff || i !== 3 || this.ignoreBOM)
        result += String.fromCodePoint(code);
    }
    return result;
  }
}
