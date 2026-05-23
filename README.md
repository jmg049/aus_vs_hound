# aus_vs_hound

WAV file I/O benchmark comparing [`audio_samples_io`](https://crates.io/crates/audio_samples_io) (`aus`) against [`hound`](https://crates.io/crates/hound), with a memory-mapped (`mmap`) read baseline.

Four benchmark modes are covered: bulk read, bulk write, streamed read, and streamed write. Benchmarks are run across multiple sample types (`i16`, `i32`, `f32`), channel counts (mono / stereo), and signal durations. Streamed benchmarks also sweep chunk sizes. A cold-cache mode (Linux only) measures storage-bound performance via `posix_fadvise(DONTNEED)`.

## Prerequisites

- Rust toolchain (`cargo`)
- Python ≥ 3.12

## Setup

Generate the WAV test files used by the benchmarks:

```bash
uv sync
uv run scripts/build_wavs.py
```

This writes sine-wave WAV files into `resources/` for every combination of duration, sample type, and channel count.

## Build

```bash
cargo build --release
```

## Running

### Single run

Run all four benchmark modes with default settings (10 s signal, 1 000 iterations, 100 warmup):

```bash
./target/release/aus_vs_hound
```

Results are written to `results.csv` and `results.md`.

### Full benchmark sweep

`run_all.sh` sweeps all durations (1 s – 600 s), chunk sizes (512 – 16 384), and includes a cold-cache read pass:

```bash
bash run_all.sh
```

Individual CSVs and Markdown tables land in `results/`. A combined CSV is written to `results/combined.csv`.

Environment variables can override the defaults:

| Variable   | Default                        | Description                        |
|:-----------|:-------------------------------|:-----------------------------------|
| `BINARY`   | `./target/release/aus_vs_hound`| Path to the compiled binary        |
| `WARMUP`   | `50`                           | Warmup iterations                  |
| `OUT_DIR`  | `results`                  | Output directory                   |
| `DTYPES`   | `i16,i32,f32`                  | Comma-separated sample types       |
| `CHANNELS` | `1,2`                          | Comma-separated channel counts     |

### CLI options

```text
aus_vs_hound [OPTIONS]

--dtype        <LIST>   Sample types: i16,i32,f32          (default: i16,i32,f32)
--channels     <LIST>   Channel counts                      (default: 1,2)
--duration     <SECS>   Signal duration in seconds          (default: 10)
--iterations   <N>      Measured iterations per benchmark   (default: 1000)
--warmup       <N>      Warmup iterations before measurement(default: 100)
--chunk-size   <N>      Samples per chunk (streamed only)   (default: 4096)
--bench        <LIST>   read,write,streamed-read,streamed-write (default: all four)
--output       <PATH>   Base name for output files          (default: results)
--cold-cache            Drop page cache before each read (Linux only)
--help, -h              Show this help
```

## Analysis

Generate publication-quality figures and summary Markdown tables from the combined CSV:

```bash
python analyse.py --csv results/combined.csv --out figures/
```

Figures are written as 300 DPI PNGs to `figures/`. Summary tables are written to `figures/results_warm.md` and `figures/results_cold.md`.

## Output files

| Path | Contents |
|:-----|:---------|
| `results/*.csv` | Raw per-run timings (avg, σ, p50, p90, p99, CV) |
| `results/*.md` | Markdown tables for each benchmark run |
| `results/combined.csv` | All CSVs merged into one file |
| `figures/*.png` | Time, throughput, and speedup plots |
| `figures/results_warm.md` | Summary tables for warm-cache runs |
| `figures/results_cold.md` | Summary tables for cold-cache runs |
