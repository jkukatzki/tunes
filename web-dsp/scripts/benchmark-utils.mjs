export function summarize(values, budgetMs) {
  if (!values.length || values.some((v) => !Number.isFinite(v) || v < 0))
    throw new Error("Invalid timing samples");
  const sorted = [...values].sort((a, b) => a - b);
  const percentile = (p) =>
    sorted[Math.max(0, Math.ceil(p * sorted.length) - 1)];
  return {
    count: values.length,
    meanMs: values.reduce((a, b) => a + b, 0) / values.length,
    p50Ms: percentile(0.5),
    p95Ms: percentile(0.95),
    p99Ms: percentile(0.99),
    maxMs: sorted.at(-1),
    overBudgetBlocks: values.filter((v) => v > budgetMs).length,
    p99BudgetRatio: percentile(0.99) / budgetMs,
  };
}
export function validateDuration(warmup, blocks, rate, frames = 512) {
  // Diagnostic notes last 30 seconds. Leave room for envelope release and startup.
  if (((warmup + blocks) * frames) / rate >= 28)
    throw new Error(
      "Benchmark would outlive sustained voices; reduce block count",
    );
}
