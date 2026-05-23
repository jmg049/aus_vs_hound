#!/usr/bin/env bash
set -euo pipefail

BINARY="${BINARY:-./target/release/aus_vs_hound}"
WARMUP="${WARMUP:-50}"
OUT_DIR="${OUT_DIR:-results}"
DTYPES="${DTYPES:-i16,i32,f32}"
CHANNELS="${CHANNELS:-1,2}"

# Durations (seconds) swept for every benchmark kind
DURATIONS=(1 5 10 30 60 300 600)

# Iterations per duration — scale down for longer runs
declare -A ITER_FOR_DUR=( [1]=10000 [5]=10000 [10]=10000 [30]=5000 [60]=5000 [300]=1000 [600]=1000 )

# Chunk sizes swept only for streaming benchmarks
CHUNK_SIZES=(512 1024 4096 8192 16384)

if [[ ! -x "$BINARY" ]]; then
    echo "Binary not found or not executable: $BINARY" >&2
    echo "Run: cargo build --release" >&2
    exit 1
fi

mkdir -p "$OUT_DIR"

total_runs=$(( ${#DURATIONS[@]} * 2 + ${#DURATIONS[@]} * ${#CHUNK_SIZES[@]} ))
run=0

for dur in "${DURATIONS[@]}"; do
    run=$(( run + 1 ))
    iters="${ITER_FOR_DUR[$dur]:-100}"
    name="${OUT_DIR}/bulk_${dur}s"
    echo ""
    echo "━━━ [$run/$total_runs] Read + Write │ duration=${dur}s  iterations=${iters} ━━━"
    "$BINARY" \
        --bench read,write \
        --dtype "$DTYPES" \
        --channels "$CHANNELS" \
        --duration "$dur" \
        --iterations "$iters" \
        --warmup "$WARMUP" \
        --output "$name"
done

for dur in "${DURATIONS[@]}"; do
    for chunk in "${CHUNK_SIZES[@]}"; do
        run=$(( run + 1 ))
        iters="${ITER_FOR_DUR[$dur]:-100}"
        name="${OUT_DIR}/streamed_${dur}s_chunk${chunk}"
        echo ""
        echo "━━━ [$run/$total_runs] Streamed Read + Write │ duration=${dur}s  chunk=${chunk}  iterations=${iters} ━━━"
        "$BINARY" \
            --bench streamed-read,streamed-write \
            --dtype "$DTYPES" \
            --channels "$CHANNELS" \
            --duration "$dur" \
            --iterations "$iters" \
            --warmup "$WARMUP" \
            --chunk-size "$chunk" \
            --output "$name"
    done
done
# Iteration counts are lower: each iteration forces a disk read via posix_fadvise(DONTNEED).
declare -A COLD_ITER_FOR_DUR=( [1]=1000 [5]=1000 [10]=1000 [30]=500 [60]=300 [300]=30 [600]=30 )

for dur in "${DURATIONS[@]}"; do
    run=$(( run + 1 ))
    iters="${COLD_ITER_FOR_DUR[$dur]:-5}"
    name="${OUT_DIR}/cold_read_${dur}s"
    echo ""
    echo "━━━ [$run/$total_runs] Cold Read │ duration=${dur}s  iterations=${iters} ━━━"
    "$BINARY" \
        --bench read \
        --dtype "$DTYPES" \
        --channels "$CHANNELS" \
        --duration "$dur" \
        --iterations "$iters" \
        --warmup "$WARMUP" \
        --output "$name" \
        --cold-cache
done

COMBINED="${OUT_DIR}/combined.csv"
header_written=0
for f in "$OUT_DIR"/*.csv; do
    [[ "$f" == "$COMBINED" ]] && continue
    if [[ "$header_written" -eq 0 ]]; then
        cat "$f" > "$COMBINED"
        header_written=1
    else
        tail -n +2 "$f" >> "$COMBINED"
    fi
done

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "All done."
echo "  Individual results : $OUT_DIR/"
echo "  Combined CSV       : $COMBINED"
echo "  Rows               : $(( $(wc -l < "$COMBINED") - 1 ))"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
