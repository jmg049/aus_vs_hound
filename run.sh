#!/usr/bin/env bash
set -euo pipefail

# Targeted benchmark runner for quickly re-running specific conditions.
#
# Examples:
#   bash run.sh --bench read --dtype i16 --channels 2 --duration 10 --cold-cache
#   bash run.sh --bench streamed-read,streamed-write --chunk-size 8192 --duration 30
#   bash run.sh --profile quick --bench write --dtype f32 --channels 1

BINARY="${BINARY:-./target/release/aus_vs_hound}"
OUT_DIR="${OUT_DIR:-results}"
PROFILE="${PROFILE:-standard}"   # quick | standard | heavy
BENCH="${BENCH:-read}"
DTYPES="${DTYPES:-i16}"
CHANNELS="${CHANNELS:-2}"
DURATION="${DURATION:-10}"
CHUNK_SIZE="${CHUNK_SIZE:-4096}"
SWEEP_DURATIONS="${SWEEP_DURATIONS:-1,5,10,30,60,300,600}"
SWEEP_CHUNKS="${SWEEP_CHUNKS:-512,1024,4096,8192,16384}"
WARMUP="${WARMUP:-50}"
ITERATIONS="${ITERATIONS:-}"
ITER_MAP="${ITER_MAP:-}"
COLD_CACHE=0
APPEND_COMBINED=1
DRY_RUN=0
SWEEP=0
NAME=""

usage() {
    cat <<'EOF'
run.sh --- targeted aus_vs_hound benchmark runner

USAGE:
  bash run.sh [OPTIONS]

OPTIONS:
  --bench <LIST>         Bench list (default: read)
                         valid: read,write,streamed-read,streamed-write
  --dtype <LIST>         Sample dtype list (default: i16)
  --channels <LIST>      Channel list (default: 2)
  --duration <SECS>      Duration in seconds (default: 10)
  --chunk-size <N>       Chunk size for streamed benches (default: 4096)
    --sweep                Sweep durations/chunks for selected benches
    --sweep-durations <L>  Duration list for --sweep (default: 1,5,10,30,60,300,600)
    --sweep-chunks <L>     Chunk list for --sweep (default: 512,1024,4096,8192,16384)
  --iterations <N>       Iterations (default: based on --profile and duration)
    --iter-map <MAP>       Per-duration iterations, e.g. 1:100,5:100,10:100,30:50,60:30,300:10,600:5
  --warmup <N>           Warmup iterations (default: 50)
  --profile <NAME>       quick | standard | heavy (default: standard)
  --cold-cache           Enable --cold-cache
  --name <BASE>          Explicit output base name (without extension)
  --out-dir <DIR>        Output directory (default: results)
  --binary <PATH>        Path to compiled binary
  --no-combined          Do not merge into <out-dir>/combined.csv
  --dry-run              Print command but do not run it
  -h, --help             Show this help

ENV OVERRIDES:
    BINARY OUT_DIR PROFILE BENCH DTYPES CHANNELS DURATION CHUNK_SIZE SWEEP_DURATIONS SWEEP_CHUNKS ITERATIONS ITER_MAP WARMUP

NOTES:
  * This script is for focused, single-condition reruns.
  * Use --sweep for a one-command all-conditions pass.
  * If --name is omitted, a descriptive one is auto-generated.
EOF
}

is_streamed_bench_list() {
    local benches="$1"
    case ",$benches," in
        *,streamed-read,*|*,streamed-write,*) return 0 ;;
        *) return 1 ;;
    esac
}

has_bench() {
    local list="$1"
    local needle="$2"
    case ",$list," in
        *",$needle,"*) return 0 ;;
        *) return 1 ;;
    esac
}

append_combined() {
    local out_dir="$1"
    local new_csv="$2"

    [[ "$APPEND_COMBINED" -eq 1 ]] || return 0

    local combined="${out_dir}/combined.csv"
    if [[ -f "$new_csv" ]]; then
        if [[ ! -f "$combined" ]]; then
            cat "$new_csv" > "$combined"
        else
            tail -n +2 "$new_csv" >> "$combined"
        fi
        local rows
        rows=$(( $(wc -l < "$combined") - 1 ))
        echo "Updated combined CSV: $combined (rows: $rows)"
    else
        echo "Warning: expected CSV not found at $new_csv" >&2
    fi
}

run_one() {
    local bench="$1"
    local duration="$2"
    local chunk="$3"
    local name="$4"
    local iters="$5"

    local out_base="${OUT_DIR}/${name}"
    local cmd=(
        "$BINARY"
        --bench "$bench"
        --dtype "$DTYPES"
        --channels "$CHANNELS"
        --duration "$duration"
        --iterations "$iters"
        --warmup "$WARMUP"
        --output "$out_base"
    )

    if is_streamed_bench_list "$bench"; then
        cmd+=(--chunk-size "$chunk")
    fi

    if [[ "$COLD_CACHE" -eq 1 ]]; then
        cmd+=(--cold-cache)
    fi

    echo ""
    echo "→ bench=$bench dur=${duration}s iters=$iters warmup=$WARMUP dtype=$DTYPES ch=$CHANNELS$(is_streamed_bench_list "$bench" && echo " chunk=$chunk")$( [[ "$COLD_CACHE" -eq 1 ]] && echo " cold" )"
    echo "  output=${out_base}.{csv,md}"

    if [[ "$DRY_RUN" -eq 1 ]]; then
        printf '  cmd:'
        printf ' %q' "${cmd[@]}"
        echo ""
        return 0
    fi

    "${cmd[@]}"
    append_combined "$OUT_DIR" "${out_base}.csv"
}

default_iterations() {
    local profile="$1"
    local dur="$2"

    case "$profile" in
        quick)
            case "$dur" in
                1) echo 250 ;;
                5) echo 200 ;;
                10) echo 150 ;;
                30) echo 100 ;;
                60) echo 60 ;;
                300) echo 20 ;;
                600) echo 10 ;;
                *) echo 50 ;;
            esac
            ;;
        heavy)
            case "$dur" in
                1|5|10) echo 20000 ;;
                30|60) echo 10000 ;;
                300|600) echo 2000 ;;
                *) echo 1000 ;;
            esac
            ;;
        standard|*)
            case "$dur" in
                1|5|10) echo 10000 ;;
                30|60) echo 5000 ;;
                300|600) echo 1000 ;;
                *) echo 1000 ;;
            esac
            ;;
    esac
}

iter_for_duration() {
    local dur="$1"

    if [[ -n "$ITERATIONS" ]]; then
        echo "$ITERATIONS"
        return 0
    fi

    if [[ -n "$ITER_MAP" ]]; then
        local pair k v
        IFS=',' read -r -a pairs <<< "$ITER_MAP"
        for pair in "${pairs[@]}"; do
            pair="$(echo "$pair" | xargs)"
            [[ -n "$pair" ]] || continue
            if [[ "$pair" != *:* ]]; then
                echo "Invalid --iter-map entry: '$pair' (expected DURATION:ITERATIONS)" >&2
                exit 1
            fi
            k="${pair%%:*}"
            v="${pair#*:}"
            k="$(echo "$k" | xargs)"
            v="$(echo "$v" | xargs)"
            if [[ "$k" == "$dur" ]]; then
                if ! [[ "$v" =~ ^[0-9]+$ ]]; then
                    echo "Invalid iteration value in --iter-map for duration '$k': '$v'" >&2
                    exit 1
                fi
                echo "$v"
                return 0
            fi
        done
    fi

    default_iterations "$PROFILE" "$dur"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --bench) BENCH="$2"; shift 2 ;;
        --dtype) DTYPES="$2"; shift 2 ;;
        --channels) CHANNELS="$2"; shift 2 ;;
        --duration) DURATION="$2"; shift 2 ;;
        --chunk-size) CHUNK_SIZE="$2"; shift 2 ;;
        --sweep) SWEEP=1; shift ;;
        --sweep-durations) SWEEP_DURATIONS="$2"; shift 2 ;;
        --sweep-chunks) SWEEP_CHUNKS="$2"; shift 2 ;;
        --iterations) ITERATIONS="$2"; shift 2 ;;
        --iter-map) ITER_MAP="$2"; shift 2 ;;
        --warmup) WARMUP="$2"; shift 2 ;;
        --profile) PROFILE="$2"; shift 2 ;;
        --cold-cache) COLD_CACHE=1; shift ;;
        --name) NAME="$2"; shift 2 ;;
        --out-dir) OUT_DIR="$2"; shift 2 ;;
        --binary) BINARY="$2"; shift 2 ;;
        --no-combined) APPEND_COMBINED=0; shift ;;
        --dry-run) DRY_RUN=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *)
            echo "Unknown option: $1" >&2
            usage
            exit 1
            ;;
    esac
done

case "$PROFILE" in
    quick|standard|heavy) ;;
    *)
        echo "Invalid profile: $PROFILE (expected quick|standard|heavy)" >&2
        exit 1
        ;;
esac

if [[ ! -x "$BINARY" ]]; then
    echo "Binary not found or not executable: $BINARY" >&2
    echo "Attempting to build with cargo..." >&2
    cargo build --release > /dev/null || {
        echo "Failed to build the binary. Please ensure you have Rust installed and run: cargo build --release" >&2
        exit 1
    }
    exit 1
fi

mkdir -p "$OUT_DIR"

if [[ "$SWEEP" -eq 0 ]]; then
    ITERATIONS="$(iter_for_duration "$DURATION")"

    echo "Running targeted benchmark:"
    echo "  bench      : $BENCH"
    echo "  dtype      : $DTYPES"
    echo "  channels   : $CHANNELS"
    echo "  duration   : ${DURATION}s"
    echo "  iterations : $ITERATIONS (profile=$PROFILE)"
    echo "  warmup     : $WARMUP"
    if is_streamed_bench_list "$BENCH"; then
        echo "  chunk-size : $CHUNK_SIZE"
    fi
    if [[ "$COLD_CACHE" -eq 1 ]]; then
        echo "  cache mode : cold"
    else
        echo "  cache mode : warm"
    fi

    if [[ -z "$NAME" ]]; then
        base="run_${BENCH//,/+}_${DTYPES//,/+}_${CHANNELS//,/+}ch_${DURATION}s"
        if is_streamed_bench_list "$BENCH"; then
            base+="_chunk${CHUNK_SIZE}"
        fi
        if [[ "$COLD_CACHE" -eq 1 ]]; then
            base+="_cold"
        fi
        NAME="$base"
    fi

    run_one "$BENCH" "$DURATION" "$CHUNK_SIZE" "$NAME" "$ITERATIONS"
else
    IFS=',' read -r -a durs <<< "$SWEEP_DURATIONS"
    IFS=',' read -r -a chunks <<< "$SWEEP_CHUNKS"

    bulk_bench=""
    streamed_bench=""
    if has_bench "$BENCH" "read"; then
        bulk_bench="read"
    fi
    if has_bench "$BENCH" "write"; then
        if [[ -n "$bulk_bench" ]]; then bulk_bench+=","; fi
        bulk_bench+="write"
    fi
    if has_bench "$BENCH" "streamed-read"; then
        streamed_bench="streamed-read"
    fi
    if has_bench "$BENCH" "streamed-write"; then
        if [[ -n "$streamed_bench" ]]; then streamed_bench+=","; fi
        streamed_bench+="streamed-write"
    fi

    total=0
    if [[ -n "$bulk_bench" ]]; then
        total=$(( total + ${#durs[@]} ))
    fi
    if [[ -n "$streamed_bench" ]]; then
        total=$(( total + (${#durs[@]} * ${#chunks[@]}) ))
    fi

    if [[ "$total" -eq 0 ]]; then
        echo "No valid benches selected for --sweep: $BENCH" >&2
        exit 1
    fi

    echo "Running sweep:"
    echo "  benches    : $BENCH"
    echo "  dtype      : $DTYPES"
    echo "  channels   : $CHANNELS"
    echo "  durations  : $SWEEP_DURATIONS"
    if [[ -n "$streamed_bench" ]]; then
        echo "  chunks     : $SWEEP_CHUNKS"
    fi
    echo "  iterations : ${ITERATIONS:-per-duration default} (profile=$PROFILE)"
    if [[ -n "$ITER_MAP" ]]; then
        echo "  iter-map   : $ITER_MAP"
    fi
    echo "  warmup     : $WARMUP"
    echo "  cache mode : $( [[ "$COLD_CACHE" -eq 1 ]] && echo cold || echo warm )"
    echo "  runs       : $total"

    run_idx=0
    for dur in "${durs[@]}"; do
        d="$(echo "$dur" | xargs)"
        [[ -n "$d" ]] || continue
        iters="$(iter_for_duration "$d")"

        if [[ -n "$bulk_bench" ]]; then
            run_idx=$(( run_idx + 1 ))
            slug="${bulk_bench//,/+}"
            name="sweep_${slug}_${d}s"
            if [[ "$COLD_CACHE" -eq 1 ]]; then
                name+="_cold"
            fi
            echo ""
            echo "━━━ [$run_idx/$total] bulk duration=${d}s ━━━"
            run_one "$bulk_bench" "$d" "$CHUNK_SIZE" "$name" "$iters"
        fi

        if [[ -n "$streamed_bench" ]]; then
            for chunk in "${chunks[@]}"; do
                c="$(echo "$chunk" | xargs)"
                [[ -n "$c" ]] || continue
                run_idx=$(( run_idx + 1 ))
                slug="${streamed_bench//,/+}"
                name="sweep_${slug}_${d}s_chunk${c}"
                if [[ "$COLD_CACHE" -eq 1 ]]; then
                    name+="_cold"
                fi
                echo ""
                echo "━━━ [$run_idx/$total] streamed duration=${d}s chunk=${c} ━━━"
                run_one "$streamed_bench" "$d" "$c" "$name" "$iters"
            done
        fi
    done
fi

echo ""
echo "Done."
