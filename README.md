# aus_vs_hound

WAV file I/O benchmark comparing [`audio_samples_io`](https://crates.io/crates/audio_samples_io) (`audio_samples_io`) against [`hound`](https://crates.io/crates/hound).

Six benchmark groups are covered: bulk read, bulk write, streamed read, streamed write, and cold-cache variants of both read modes (Linux only). Benchmarks sweep sample types (`i16`, `i32`, `f32`), channel counts (mono / stereo), signal durations (1 s – 600 s), and chunk sizes (512 – 16 384 samples). All benchmarks are driven by [Criterion](https://github.com/bheisler/criterion.rs).

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

## Running

Run the full benchmark suite:

```bash
cargo bench --bench wav_benches
```

To run a single benchmark group, filter by group name:

```bash
cargo bench --bench wav_benches bulk_read
cargo bench --bench wav_benches bulk_write
cargo bench --bench wav_benches streamed_read
cargo bench --bench wav_benches streamed_write
cargo bench --bench wav_benches cold_read        # Linux only
cargo bench --bench wav_benches cold_streamed_read  # Linux only
```

Criterion writes HTML reports to `target/criterion/`.

> **Cold-cache benchmarks** (`cold_read`, `cold_streamed_read`) use `posix_fadvise(DONTNEED)` to evict the page cache before each iteration and are only supported on Linux.

## Extracting results

After running the benchmarks, convert Criterion's raw output to the CSV format consumed by `analyse.py`:

```bash
python scripts/extract_criterion.py \
    --criterion-dir target/criterion \
    --out results/criterion_combined.csv
```

| Argument | Default | Description |
|:---------|:--------|:------------|
| `--criterion-dir` | `target/criterion` | Criterion output directory |
| `--out` | `results/criterion_combined.csv` | Output CSV path |

## Analysis

Generate figures and summary Markdown tables from the CSV:

```bash
python analyse.py --csv results/criterion_combined.csv --out figures/
```

Figures are written as 300 DPI PNGs to `figures/`. Summary tables are written to `figures/results_warm.md` and `figures/results_cold.md`.

## Output files

| Path | Contents |
|:-----|:---------|
| `target/criterion/` | Criterion HTML reports and raw `sample.json` files |
| `results/criterion_combined.csv` | Extracted timings (avg, σ, CV) used by `analyse.py` |
| `figures/*.png` | Time, throughput, and speedup plots |
| `figures/results_warm.md` | Summary tables for warm-cache runs |
| `figures/results_cold.md` | Summary tables for cold-cache runs |
