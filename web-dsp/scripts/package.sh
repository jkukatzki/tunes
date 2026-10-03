#!/usr/bin/env bash
set -euo pipefail
# Standalone browser distribution: bash web-dsp/scripts/package.sh [release|dev] [output]
dsp_dir="$(cd "$(dirname "$0")/.." && pwd)"
profile="${1:-release}"
case "$profile" in release) profile_dir=release ;; dev) profile_dir=debug ;; *) echo 'Use release or dev' >&2; exit 1 ;; esac
output_dir="${2:-$dsp_dir/dist}"
mkdir -p "$output_dir"
output_dir="$(cd "$output_dir" && pwd)"
# rsync --delete is restricted to an owned distribution directory.
if [[ -n "$(ls -A "$output_dir")" && ! -f "$output_dir/.tunes-audio-dist" ]]; then
  echo "Refusing to replace non-distribution directory: $output_dir" >&2; exit 1
fi
required=(cargo wasm-bindgen node rsync)
if [[ "$profile" == release ]]; then required+=("${WASM_OPT:-wasm-opt}"); fi
for tool in "${required[@]}"; do command -v "$tool" >/dev/null || { echo "Missing tool: $tool" >&2; exit 1; }; done
if [[ "${WASM_SIMD:-1}" == 1 ]]; then export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+simd128"; fi
target_dir="${CARGO_TARGET_DIR:-$dsp_dir/target}"
stage_dir="$(mktemp -d "$dsp_dir/.package.XXXXXX")"
trap 'rm -rf "$stage_dir"' EXIT
cargo build --locked --manifest-path "$dsp_dir/Cargo.toml" --lib --target wasm32-unknown-unknown --profile "$profile" --target-dir "$target_dir"
wasm-bindgen "$target_dir/wasm32-unknown-unknown/$profile_dir/tunes_web_dsp.wasm" --target web --out-dir "$stage_dir" --out-name tunes_web_dsp --no-typescript
if [[ "$profile" == release ]]; then node "$dsp_dir/scripts/optimize-wasm.mjs" "$stage_dir/tunes_web_dsp_bg.wasm"; fi
for file in bridge.js bridge.d.ts command-queue.mjs adaptive-buffer.mjs render-buffer.mjs worker.js output-worklet.js index.html; do cp "$dsp_dir/web/$file" "$stage_dir/"; done
cp "$dsp_dir/../LICENSE-MIT" "$dsp_dir/../LICENSE-APACHE" "$stage_dir/"
cp "$dsp_dir/../vendor/cpal/LICENSE" "$stage_dir/CPAL-LICENSE"
cp "$dsp_dir/README.md" "$stage_dir/README.md"
touch "$stage_dir/.tunes-audio-dist"
rsync -a --delete "$stage_dir/" "$output_dir/"
echo "Tunes browser audio packaged at $output_dir"
