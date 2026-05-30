#!/usr/bin/env python3
"""
extract_criterion.py — convert criterion benchmark output to the combined.csv
format consumed by analyse.py.

Criterion stores raw per-sample timing in:
  target/criterion/<group>/<function>/<param>/new/sample.json

Each sample.json contains:
  {"iters": [n0, n1, ...], "times": [t0, t1, ...]}   (times in nanoseconds)

The per-iteration time for sample i is: times[i] / iters[i]  nanoseconds.

Benchmark naming convention used in wav_benches.rs:
  group      : bulk_read | bulk_write | streamed_read | streamed_write
  function   : {library}_{dtype}_{channels}ch[_{duration}s]  (duration in fn
               name only for streamed ops)
  param      : {duration}s  (bulk)  |  chunk{N}  (streamed)

Examples:
  bulk_read / hound_i16_1ch / 10s
  streamed_read / hound_i16_1ch_60s / chunk4096

Output CSV columns (same as results/combined.csv):
  library, operation, dtype, channels, duration_s, iterations, warmup,
  chunk_size, cold_cache, avg_ms, stddev_ms, p50_ms, p90_ms, p99_ms, cv

Usage:
  python scripts/extract_criterion.py [--criterion-dir target/criterion]
                                       [--out results/criterion_combined.csv]
"""

import argparse
import json
import re
import sys
from pathlib import Path

import numpy as np


CHUNK_SIZE_DEFAULT = 4096


def parse_function_and_param(group: str, fn_name: str, param: str) -> dict | None:
    """
    Parse benchmark metadata from criterion path components.

    Returns a dict with keys: library, operation, dtype, channels,
    duration_s, chunk_size, or None if parsing fails.
    """
    # Normalise group name to operation slug used in combined.csv.
    op_map = {
        "bulk_read": ("read", 0),
        "bulk_write": ("write", 0),
        "streamed_read": ("streamed-read", 0),
        "streamed_write": ("streamed-write", 0),
        "cold_read": ("read", 1),
        "cold_streamed_read": ("streamed-read", 1),
    }
    if group not in op_map:
        return None
    operation, cold_cache = op_map[group]

    is_streamed = operation.startswith("streamed")

    if is_streamed:
        # fn_name: {library}_{dtype}_{channels}ch_{duration}s
        # param:   chunk{N}
        m = re.fullmatch(
            r"(hound|aus)_(i8|i16|i32|f32)_(\d+)ch_(\d+)s", fn_name
        )
        if not m:
            return None
        library, dtype, channels, dur_str = m.groups()

        cm = re.fullmatch(r"chunk(\d+)", param)
        if not cm:
            return None
        chunk_size = int(cm.group(1))
        duration_s = int(dur_str)
    else:
        # fn_name: {library}_{dtype}_{channels}ch
        # param:   {duration}s
        m = re.fullmatch(r"(hound|aus)_(i8|i16|i32|f32)_(\d+)ch", fn_name)
        if not m:
            return None
        library, dtype, channels = m.groups()

        pm = re.fullmatch(r"(\d+)s", param)
        if not pm:
            return None
        duration_s = int(pm.group(1))
        chunk_size = CHUNK_SIZE_DEFAULT

    return {
        "library": library,
        "operation": operation,
        "dtype": dtype,
        "channels": int(channels),
        "duration_s": duration_s,
        "chunk_size": chunk_size,
        "cold_cache": cold_cache,
    }


def load_sample_json(path: Path) -> list[float] | None:
    """
    Read a criterion sample.json and return per-iteration times in milliseconds.
    """
    try:
        with open(path) as f:
            data = json.load(f)
    except (OSError, json.JSONDecodeError):
        return None

    iters = data.get("iters", [])
    times = data.get("times", [])
    if not iters or not times or len(iters) != len(times):
        return None

    per_iter_ns = [t / n for t, n in zip(times, iters) if n > 0]
    return [ns / 1e6 for ns in per_iter_ns]  # nanoseconds → milliseconds


def stats(values: list[float]) -> dict:
    a = np.array(values)
    avg = float(np.mean(a))
    std = float(np.std(a, ddof=1)) if len(a) > 1 else 0.0
    p50 = float(np.percentile(a, 50))
    p90 = float(np.percentile(a, 90))
    p99 = float(np.percentile(a, 99))
    cv = std / avg if avg > 0 else 0.0
    return {"avg_ms": avg, "stddev_ms": std, "p50_ms": p50, "p90_ms": p90,
            "p99_ms": p99, "cv": cv, "n": len(a)}


def collect(criterion_dir: Path) -> list[dict]:
    rows = []

    # Walk all sample.json files two or three levels deep:
    # criterion_dir / group / function / param / new / sample.json
    for sample_json in sorted(criterion_dir.rglob("new/sample.json")):
        parts = sample_json.relative_to(criterion_dir).parts
        # Expected: (group, function, param, "new", "sample.json")
        if len(parts) != 5 or parts[3] != "new":
            continue

        group, fn_name, param = parts[0], parts[1], parts[2]

        meta = parse_function_and_param(group, fn_name, param)
        if meta is None:
            continue

        timings = load_sample_json(sample_json)
        if not timings:
            continue

        s = stats(timings)
        rows.append({
            "library": meta["library"],
            "operation": meta["operation"],
            "dtype": meta["dtype"],
            "channels": meta["channels"],
            "duration_s": meta["duration_s"],
            "iterations": s["n"],
            "warmup": 0,       # criterion handles warmup internally
            "chunk_size": meta["chunk_size"],
            "cold_cache": meta["cold_cache"],
            "avg_ms": round(s["avg_ms"], 6),
            "stddev_ms": round(s["stddev_ms"], 6),
            "p50_ms": round(s["p50_ms"], 6),
            "p90_ms": round(s["p90_ms"], 6),
            "p99_ms": round(s["p99_ms"], 6),
            "cv": round(s["cv"], 6),
        })

    return rows


COLUMNS = [
    "library", "operation", "dtype", "channels", "duration_s",
    "iterations", "warmup", "chunk_size", "cold_cache",
    "avg_ms", "stddev_ms", "p50_ms", "p90_ms", "p99_ms", "cv",
]


def write_csv(rows: list[dict], out: Path) -> None:
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, "w") as f:
        f.write(",".join(COLUMNS) + "\n")
        for r in rows:
            f.write(",".join(str(r[c]) for c in COLUMNS) + "\n")
    print(f"Wrote {len(rows)} rows → {out}")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--criterion-dir", default="target/criterion",
        help="Root criterion output directory (default: target/criterion)"
    )
    ap.add_argument(
        "--out", default="results/criterion_combined.csv",
        help="Output CSV path (default: results/criterion_combined.csv)"
    )
    args = ap.parse_args()

    criterion_dir = Path(args.criterion_dir)
    if not criterion_dir.exists():
        print(f"error: {criterion_dir} not found — run `cargo bench` first", file=sys.stderr)
        sys.exit(1)

    rows = collect(criterion_dir)
    if not rows:
        print("No benchmark results found. Check that wav_benches ran successfully.", file=sys.stderr)
        sys.exit(1)

    write_csv(rows, Path(args.out))
    print(
        "To regenerate figures from criterion results, run:\n"
        f"  python analyse.py --csv {args.out}"
    )


if __name__ == "__main__":
    main()
