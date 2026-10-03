import { test } from "node:test";
import assert from "node:assert/strict";
import { summarize, validateDuration } from "../scripts/benchmark-utils.mjs";
test("timings count deadline misses without concealing a tail spike", () => {
  const result = summarize([...Array(99).fill(1), 30], 10);
  assert.equal(result.p99Ms, 1);
  assert.equal(result.maxMs, 30);
  assert.equal(result.overBudgetBlocks, 1);
  assert.equal(result.meanMs, 1.29);
  assert.equal(result.p99BudgetRatio, 0.1);
  assert.throws(() => summarize([NaN], 10));
});
test("long benchmarks fail before they measure expired notes as silence", () => {
  assert.doesNotThrow(() => validateDuration(100, 1000, 44100));
  assert.throws(() => validateDuration(100, 2500, 44100), /outlive/);
});
