import { test } from "node:test";
import assert from "node:assert/strict";
import { WorkletTextEncoder, WorkletTextDecoder } from "./worklet-utf8.mjs";
test("worklet UTF-8 matches the native encoder for identifiers and commands", () => {
  const encoder = new WorkletTextEncoder();
  const native = new TextEncoder();
  for (const text of [
    "",
    "piano",
    "é中🎹",
    "\ud800",
    "\ufeffinstrument",
    JSON.stringify({ name: "🎷 Straße" }),
  ]) {
    assert.deepEqual(encoder.encode(text), native.encode(text));
    assert.equal(
      new WorkletTextDecoder("utf-8", { ignoreBOM: true }).decode(
        encoder.encode(text),
      ),
      new TextDecoder("utf-8", { ignoreBOM: true }).decode(native.encode(text)),
    );
    for (let length = 0; length < 20; length++) {
      const a = new Uint8Array(length),
        b = new Uint8Array(length);
      assert.deepEqual(encoder.encodeInto(text, a), native.encodeInto(text, b));
      assert.deepEqual(a, b);
    }
  }
});
test("decoder rejects malformed UTF-8 rather than corrupting command strings", () => {
  for (const bytes of [
    [0xc0, 0x80],
    [0xf4, 0x90, 0x80, 0x80],
    [0xed, 0xa0, 0x80],
    [0xe0, 0x80, 0x80],
    [0xe2, 0x82],
    [0xff],
  ])
    assert.throws(
      () => new WorkletTextDecoder().decode(new Uint8Array(bytes)),
      /Invalid UTF-8/,
    );
});
