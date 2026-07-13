#!/usr/bin/env bash
#
# compare-generators.sh — generation wall-clock + output-size comparison across
# spargen, progenitor, and openapi-generator on a single OpenAPI spec.
#
# Each tool is run only if it is installed; a missing tool is SKIPPED, never a hard failure.
# A tool that runs but rejects the spec (e.g. progenitor on a 3.1 document) is reported as
# FAILED with its status — that is a real, honest data point, not a script error.
#
# This committed artifact is a script + the methodology in docs/benchmarks.md; it downloads no
# binaries. Read docs/benchmarks.md for the fair-comparison caveats (the three tools have
# different OpenAPI-version scopes and different output contracts, so the numbers are
# illustrative, not a like-for-like race).
#
# Usage:
#   scripts/compare-generators.sh [SPEC] [OUT_DIR]
#
#   SPEC     Path to the OpenAPI document (default: examples/petstore/petstore.yaml)
#   OUT_DIR  Scratch directory for generated output (default: a fresh mktemp -d)
#
# Environment:
#   RUNS         Timed repetitions per tool; the best (min) wall-clock is reported (default: 3)
#   SPARGEN_BIN  Pre-built spargen binary to time (default: build+use target/release/spargen)
#
# Exit status is 0 as long as the harness itself ran; individual tool failures are reported
# in-band and do not fail the script.

set -u

# ---- locate the repo root (this script lives in <root>/scripts) --------------------------------
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

SPEC="${1:-$ROOT_DIR/examples/petstore/petstore.yaml}"
OUT_DIR="${2:-$(mktemp -d)}"
RUNS="${RUNS:-3}"

if [ ! -f "$SPEC" ]; then
  echo "error: spec not found: $SPEC" >&2
  exit 2
fi

mkdir -p "$OUT_DIR"

echo "== compare-generators =="
echo "spec:     $SPEC"
echo "out:      $OUT_DIR"
echo "runs:     $RUNS (best wall-clock reported)"
echo

# ---- helpers ------------------------------------------------------------------------------------

# now_ns: monotonic-ish wall clock in nanoseconds (GNU date; fine on Linux/CI).
now_ns() { date +%s%N; }

# dir_size_kb DIR: total size of DIR in KiB (0 if absent).
dir_size_kb() {
  if [ -d "$1" ]; then du -sk "$1" 2>/dev/null | awk '{print $1}'; else echo 0; fi
}

# run_timed NAME OUTSUBDIR CMD...: run CMD RUNS times into a clean OUTSUBDIR, reporting the best
# wall-clock (ms) and the final output size (KiB). Reports SKIPPED/FAILED without aborting.
run_timed() {
  name="$1"; outsub="$2"; shift 2
  dest="$OUT_DIR/$outsub"
  best_ms=""
  status="ok"
  i=0
  while [ "$i" -lt "$RUNS" ]; do
    rm -rf "$dest"; mkdir -p "$dest"
    start="$(now_ns)"
    if ! "$@" >"$OUT_DIR/$outsub.log" 2>&1; then
      status="failed"
      break
    fi
    end="$(now_ns)"
    ms=$(( (end - start) / 1000000 ))
    if [ -z "$best_ms" ] || [ "$ms" -lt "$best_ms" ]; then best_ms="$ms"; fi
    i=$((i + 1))
  done

  size_kb="$(dir_size_kb "$dest")"
  if [ "$status" = "failed" ]; then
    printf '  %-20s FAILED (see %s)\n' "$name" "$outsub.log"
  else
    printf '  %-20s %6s ms   %8s KiB output\n' "$name" "$best_ms" "$size_kb"
  fi
}

# ---- spargen (always available; built from this workspace) -------------------------------------
echo "-- spargen --"
if [ -n "${SPARGEN_BIN:-}" ] && [ -x "${SPARGEN_BIN:-}" ]; then
  spargen_bin="$SPARGEN_BIN"
else
  echo "  building spargen (release)..."
  if cargo build --release -p spargen --quiet --manifest-path "$ROOT_DIR/Cargo.toml"; then
    spargen_bin="$ROOT_DIR/target/release/spargen"
  else
    spargen_bin=""
  fi
fi
if [ -n "$spargen_bin" ] && [ -x "$spargen_bin" ]; then
  run_timed "spargen" "spargen" \
    "$spargen_bin" generate "$SPEC" --out "$OUT_DIR/spargen/client" --as-crate
else
  echo "  spargen      SKIPPED (build failed)"
fi
echo

# ---- progenitor (cargo-progenitor) -------------------------------------------------------------
echo "-- progenitor --"
if cargo progenitor --help >/dev/null 2>&1; then
  run_timed "progenitor" "progenitor" \
    cargo progenitor --input "$SPEC" --output "$OUT_DIR/progenitor/client" \
    --name compare-client --version 0.0.0
else
  echo "  progenitor   SKIPPED (cargo-progenitor not installed: cargo install cargo-progenitor)"
fi
echo

# ---- openapi-generator (rust generator) --------------------------------------------------------
echo "-- openapi-generator (rust) --"
oag=""
if command -v openapi-generator-cli >/dev/null 2>&1; then
  oag="openapi-generator-cli"
elif command -v openapi-generator >/dev/null 2>&1; then
  oag="openapi-generator"
elif command -v npx >/dev/null 2>&1; then
  # npx resolves @openapitools/openapi-generator-cli (requires a JRE on PATH).
  oag="npx --yes @openapitools/openapi-generator-cli"
fi
if [ -n "$oag" ] && command -v java >/dev/null 2>&1; then
  # shellcheck disable=SC2086
  run_timed "openapi-generator" "openapi-generator" \
    $oag generate -i "$SPEC" -g rust -o "$OUT_DIR/openapi-generator/client"
else
  echo "  openapi-gen  SKIPPED (need openapi-generator-cli or npx + a JRE)"
fi
echo

echo "== done =="
