#!/usr/bin/env python3
"""
analyse.py --- figures and markdown tables for aus_vs_hound.

Usage:
    python analyse.py [--csv results.csv] [--out figures/]

Requires: [uv add /] pip install matplotlib pandas numpy
"""

import argparse
import sys
from pathlib import Path

import matplotlib as mpl
import matplotlib.pyplot as plt
import numpy as np
import pandas as pd
from matplotlib import font_manager as fm
from matplotlib.patches import Patch

HOUND_COLOR = "#E8622A"
AUS_COLOR = "#2A6FBF"
MMAP_COLOR = "#2DB84B"
GRID_COLOR = "#EBEBEB"
TEXT_COLOR = "#222222"

DTYPE_ORDER = ["i16", "i32", "f32"]
DURATION_ORDER = [1, 5, 10, 30, 60, 300, 600]
CHUNK_ORDER = [512, 1024, 4096, 8192, 16384]
CHANNEL_ORDER = [1, 2]
SAMPLE_RATE = 44_100
BPS = {"i8": 1, "i16": 2, "i32": 4, "f32": 4}

BAR_W = 0.34
DPI = 300
FS_BASE = 10
FS_TITLE = 12


def fmt_chunk(c: int) -> str:
    return f"{c // 1024}K" if c >= 1024 else str(c)


def setup_style() -> None:
    poppins_dir = Path.home() / ".local/fonts/Poppins"
    if poppins_dir.exists():
        for ttf in poppins_dir.glob("*.ttf"):
            fm.fontManager.addfont(str(ttf))

    registered = {f.name for f in fm.fontManager.ttflist}
    font_family = "Poppins" if "Poppins" in registered else "sans-serif"
    if font_family == "sans-serif":
        print("  [warn] Poppins not registered --- using sans-serif fallback")

    mpl.rcParams.update(
        {
            "font.family": font_family,
            "font.size": FS_BASE,
            "text.color": TEXT_COLOR,
            "axes.titlesize": FS_BASE,
            "axes.titleweight": "semibold",
            "axes.labelsize": FS_BASE - 1,
            "axes.labelcolor": TEXT_COLOR,
            "xtick.labelsize": FS_BASE - 2,
            "ytick.labelsize": FS_BASE - 2,
            "xtick.color": TEXT_COLOR,
            "ytick.color": TEXT_COLOR,
            "xtick.major.size": 0,
            "ytick.major.size": 3,
            "axes.spines.top": False,
            "axes.spines.right": False,
            "axes.spines.left": True,
            "axes.spines.bottom": True,
            "axes.edgecolor": TEXT_COLOR,
            "axes.grid": True,
            "axes.grid.axis": "y",
            "grid.color": GRID_COLOR,
            "grid.linewidth": 1.0,
            "grid.linestyle": "-",
            "axes.axisbelow": True,
            "axes.facecolor": "white",
            "figure.facecolor": "white",
            "figure.dpi": DPI,
            "savefig.dpi": DPI,
        }
    )


def load(csv_path: Path) -> pd.DataFrame:
    df = pd.read_csv(csv_path)
    # Defensive: add computed columns if older CSV is missing them.
    if "cold_cache" not in df.columns:
        df["cold_cache"] = 0
    if "cv" not in df.columns:
        df["cv"] = df["stddev_ms"] / df["avg_ms"]

    df = df.drop_duplicates(
        subset=[
            "library",
            "operation",
            "dtype",
            "channels",
            "duration_s",
            "chunk_size",
            "cold_cache",
        ],
        keep="last",
    )
    df["dtype"] = pd.Categorical(df["dtype"], categories=DTYPE_ORDER, ordered=True)
    df["duration_s"] = df["duration_s"].astype(int)
    df["chunk_size"] = df["chunk_size"].astype(int)
    df["channels"] = df["channels"].astype(int)
    df["cold_cache"] = df["cold_cache"].astype(int)
    bps = df["dtype"].map(BPS).fillna(4).astype(int)
    df["file_bytes"] = df["duration_s"] * SAMPLE_RATE * df["channels"] * bps
    df["throughput_mbs"] = df["file_bytes"] / (df["avg_ms"] / 1_000.0) / 1e6
    return df


def dtypes_in(df: pd.DataFrame) -> list[str]:
    return [d for d in DTYPE_ORDER if d in df["dtype"].values]


def durations_in(df: pd.DataFrame) -> list[int]:
    return sorted(d for d in df["duration_s"].unique() if d in DURATION_ORDER)


def chunks_in(df: pd.DataFrame) -> list[int]:
    return [c for c in CHUNK_ORDER if c in df["chunk_size"].values]


def channels_in(df: pd.DataFrame) -> list[int]:
    return sorted(c for c in df["channels"].unique() if c in CHANNEL_ORDER)


_LEGEND_HANDLES_2 = [
    Patch(facecolor=HOUND_COLOR, label="hound"),
    Patch(facecolor=AUS_COLOR, label="audio_samples_io"),
]

_LEGEND_HANDLES_3 = [
    Patch(facecolor=HOUND_COLOR, label="hound"),
    Patch(facecolor=AUS_COLOR, label="audio_samples_io"),
    Patch(facecolor=MMAP_COLOR, label="mmap baseline"),
]


def _add_shared_legend(fig: plt.Figure, handles=None, bottom_pad: float = 0.10) -> None:
    if handles is None:
        handles = _LEGEND_HANDLES_2
    fig.legend(
        handles=handles,
        loc="lower center",
        ncol=len(handles),
        frameon=False,
        fontsize=FS_BASE,
        bbox_to_anchor=(0.5, 0.0),
        handlelength=1.4,
        handleheight=0.9,
        borderpad=0,
        columnspacing=1.2,
    )
    fig.subplots_adjust(bottom=bottom_pad)


def _clean_ax(ax: plt.Axes) -> None:
    ax.spines["left"].set_color(TEXT_COLOR)
    ax.spines["bottom"].set_color(TEXT_COLOR)


def _grouped_bars(
    ax: plt.Axes,
    x_labels: list[str],
    h_vals: list[float],
    a_vals: list[float],
    ylabel: str,
    first: bool,
    m_vals: list[float] | None = None,
) -> None:
    x = np.arange(len(x_labels))
    if m_vals is not None:
        w = BAR_W * 0.82  # slightly narrower for 3-bar groups
        ax.bar(x - w, h_vals, w, color=HOUND_COLOR, zorder=3)
        ax.bar(x, a_vals, w, color=AUS_COLOR, zorder=3)
        ax.bar(x + w, m_vals, w, color=MMAP_COLOR, zorder=3)
    else:
        ax.bar(x - BAR_W / 2, h_vals, BAR_W, color=HOUND_COLOR, zorder=3)
        ax.bar(x + BAR_W / 2, a_vals, BAR_W, color=AUS_COLOR, zorder=3)
    ax.set_xticks(x)
    ax.set_xticklabels(x_labels)
    if first:
        ax.set_ylabel(ylabel)
    else:
        ax.set_ylabel("")
    _clean_ax(ax)


def fig_bulk(
    df: pd.DataFrame, operation: str, metric: str, out_dir: Path, ch_label: str
) -> None:
    """
    metric: "avg_ms" | "throughput_mbs"
    X-axis: signal duration  (evenly spaced categories)
    Subplots: one per sample type
    Read benchmarks also include the mmap baseline as a third bar.
    """
    data = df[df["operation"] == operation]
    if data.empty:
        return

    is_read = operation == "read"
    dtypes = dtypes_in(data)
    durations = durations_in(data)
    x_labels = [f"{d}s" for d in durations]
    n = len(dtypes)
    use_tp = metric == "throughput_mbs"
    ylabel = "Throughput (MB/s)" if use_tp else "Time (ms)"

    fig, axes = plt.subplots(1, n, figsize=(3.8 * n, 4.2), sharey=True)
    axes = [axes] if n == 1 else list(axes)

    for i, (ax, dtype) in enumerate(zip(axes, dtypes)):
        sub = data[data["dtype"] == dtype]
        h = sub[sub["library"] == "hound"].sort_values("duration_s")
        a = sub[sub["library"] == "aus"].sort_values("duration_s")

        m_vals = None
        if is_read:
            m = sub[sub["library"] == "mmap"].sort_values("duration_s")
            if not m.empty:
                m_vals = m[metric].tolist()

        _grouped_bars(
            ax,
            x_labels,
            h[metric].tolist(),
            a[metric].tolist(),
            ylabel,
            i == 0,
            m_vals=m_vals,
        )
        ax.set_xlabel("Signal duration")
        ax.set_title(dtype)
        if i > 0:
            ax.tick_params(labelleft=False)

    op_label = "Read" if operation == "read" else "Write"
    met_label = "Throughput" if use_tp else "Time"
    fig.suptitle(
        f"WAV {op_label} --- {met_label} by sample type  ·  {ch_label}",
        fontsize=FS_TITLE,
        fontweight="semibold",
        y=1.02,
    )
    legend_handles = _LEGEND_HANDLES_3 if is_read else _LEGEND_HANDLES_2
    _add_shared_legend(fig, handles=legend_handles, bottom_pad=0.16)

    slug = f"bulk_{operation}_{'throughput' if use_tp else 'time'}_{ch_label}.png"
    fig.savefig(out_dir / slug, bbox_inches="tight")
    plt.close(fig)
    print(f"  {out_dir / slug}")


def fig_streamed(
    df: pd.DataFrame,
    operation: str,
    metric: str,
    duration: int,
    out_dir: Path,
    ch_label: str,
) -> None:
    """
    X-axis: chunk size  (evenly spaced categories, log-scale labels)
    Subplots: one per sample type
    """
    data = df[(df["operation"] == operation) & (df["duration_s"] == duration)]
    if data.empty:
        return

    dtypes = dtypes_in(data)
    chunks = chunks_in(data)
    x_labels = [fmt_chunk(c) for c in chunks]
    n = len(dtypes)
    use_tp = metric == "throughput_mbs"
    ylabel = "Throughput (MB/s)" if use_tp else "Time (ms)"

    fig, axes = plt.subplots(1, n, figsize=(3.8 * n, 4.2), sharey=True)
    axes = [axes] if n == 1 else list(axes)

    for i, (ax, dtype) in enumerate(zip(axes, dtypes)):
        sub = data[data["dtype"] == dtype]
        h = sub[sub["library"] == "hound"].sort_values("chunk_size")
        a = sub[sub["library"] == "aus"].sort_values("chunk_size")

        _grouped_bars(
            ax, x_labels, h[metric].tolist(), a[metric].tolist(), ylabel, i == 0
        )
        ax.set_xlabel("Chunk size (samples)")
        ax.set_title(dtype)
        if i > 0:
            ax.tick_params(labelleft=False)

    op_label = "Streamed Read" if operation == "streamed-read" else "Streamed Write"
    met_label = "Throughput" if use_tp else "Time"
    fig.suptitle(
        f"{op_label} --- {met_label}  ·  {duration}s signal  ·  {ch_label}",
        fontsize=FS_TITLE,
        fontweight="semibold",
        y=1.02,
    )
    _add_shared_legend(fig, bottom_pad=0.16)

    slug = f"{operation.replace('-', '_')}_{'throughput' if use_tp else 'time'}_{duration}s_{ch_label}.png"
    fig.savefig(out_dir / slug, bbox_inches="tight")
    plt.close(fig)
    print(f"  {out_dir / slug}")


def fig_speedup(
    df: pd.DataFrame,
    operation: str,
    pivot_col: str,
    x_labels: list[str],
    xlabel: str,
    title: str,
    out_dir: Path,
    filename: str,
) -> None:
    # Filter to hound vs aus only (excludes mmap library rows).
    data = df[(df["operation"] == operation) & df["library"].isin(["hound", "aus"])]
    if data.empty:
        return

    h_idx = data[data["library"] == "hound"].set_index(["dtype", pivot_col])["avg_ms"]
    a_idx = data[data["library"] == "aus"].set_index(["dtype", pivot_col])["avg_ms"]
    speedup = (h_idx / a_idx).unstack(pivot_col)
    speedup = speedup.reindex([d for d in DTYPE_ORDER if d in speedup.index])

    n_rows, n_cols = speedup.shape
    cell_w = 1.5
    cell_h = 0.85
    cbar_w = 1.0
    fig_w = max(5.0, n_cols * cell_w + cbar_w)
    fig_h = max(2.0, n_rows * cell_h + 1.0)

    fig, ax = plt.subplots(figsize=(fig_w, fig_h), constrained_layout=True)
    fig.patch.set_facecolor("white")
    ax.grid(False)

    vals = speedup.values.astype(float)
    vmin, vmax = 0.0, max(float(np.nanmax(vals)), 3.0)

    im = ax.imshow(vals, cmap="Blues", aspect="auto", vmin=vmin, vmax=vmax)

    cbar = fig.colorbar(im, ax=ax, fraction=0.046, pad=0.02)
    cbar.set_label("speedup (×)", fontsize=FS_BASE - 1, labelpad=6)
    cbar.ax.tick_params(labelsize=FS_BASE - 2)

    ax.set_xticks(range(n_cols))
    ax.set_xticklabels(x_labels, fontsize=FS_BASE - 1)
    ax.set_yticks(range(n_rows))
    ax.set_yticklabels(speedup.index, fontsize=FS_BASE - 1)
    ax.set_xlabel(xlabel, fontsize=FS_BASE - 1)
    ax.tick_params(left=False, bottom=False)
    for spine in ax.spines.values():
        spine.set_visible(False)

    text_thresh = vmin + (vmax - vmin) * 0.6
    for (r, c), val in np.ndenumerate(vals):
        if not np.isnan(val):
            txt_color = "white" if val >= text_thresh else "#222222"
            ax.text(
                c,
                r,
                f"{val:.1f}×",
                ha="center",
                va="center",
                fontsize=FS_BASE - 1,
                fontweight="semibold",
                color=txt_color,
            )

    ax.set_title(title, fontsize=FS_BASE, fontweight="semibold", pad=10)

    fig.savefig(out_dir / filename, bbox_inches="tight")
    plt.close(fig)
    print(f"  {out_dir / filename}")


def _speedup_label(h: float, a: float, σ_h: float = 0.0, σ_a: float = 0.0) -> str:
    r = h / a
    pct = (r - 1) * 100
    if σ_h > 0 or σ_a > 0:
        σ_r = r * np.sqrt((σ_h / h) ** 2 + (σ_a / a) ** 2)
        return f"{r:.2f}× ± {σ_r:.2f}× ({'+' if pct >= 0 else ''}{pct:.0f}%)"
    return f"{r:.2f}× ({'+' if pct >= 0 else ''}{pct:.0f}%)"


def _md_rows_to_table(rows: list[dict]) -> str:
    if not rows:
        return "_no data_\n"
    cols = list(rows[0].keys())
    header = "| " + " | ".join(cols) + " |"
    sep = "|" + "|".join(":---" if i <= 1 else "---:" for i in range(len(cols))) + "|"
    body = ["| " + " | ".join(str(row[c]) for c in cols) + " |" for row in rows]
    return "\n".join([header, sep] + body)


def _bulk_rows(df: pd.DataFrame, operation: str, ch: int) -> list[dict]:
    data = df[(df["operation"] == operation) & (df["channels"] == ch)]
    hi = data[data["library"] == "hound"].set_index(["dtype", "duration_s"])
    ai = data[data["library"] == "aus"].set_index(["dtype", "duration_s"])
    mi = (
        data[data["library"] == "mmap"].set_index(["dtype", "duration_s"])
        if operation == "read"
        else None
    )
    rows = []
    for dtype in dtypes_in(data):
        for dur in durations_in(data):
            try:
                h, a = hi.loc[(dtype, dur)], ai.loc[(dtype, dur)]
            except KeyError:
                continue
            row: dict = {
                "dtype": dtype,
                "duration (s)": dur,
                "hound avg (ms)": f"{h['avg_ms']:.3f}",
                "hound cv": f"{h['cv']:.3f}",
                "aus avg (ms)": f"{a['avg_ms']:.3f}",
                "aus cv": f"{a['cv']:.3f}",
                "speedup (±1σ)": _speedup_label(
                    h["avg_ms"], a["avg_ms"], h["stddev_ms"], a["stddev_ms"]
                ),
            }
            if mi is not None:
                try:
                    m = mi.loc[(dtype, dur)]
                    row["mmap avg (ms)"] = f"{m['avg_ms']:.3f}"
                    row["mmap cv"] = f"{m['cv']:.3f}"
                except KeyError:
                    row["mmap avg (ms)"] = "N/A"
                    row["mmap cv"] = "N/A"
            rows.append(row)
    return rows


def _streamed_rows(
    df: pd.DataFrame, operation: str, duration: int, ch: int
) -> list[dict]:
    data = df[
        (df["operation"] == operation)
        & (df["duration_s"] == duration)
        & (df["channels"] == ch)
    ]
    hi = data[data["library"] == "hound"].set_index(["dtype", "chunk_size"])
    ai = data[data["library"] == "aus"].set_index(["dtype", "chunk_size"])
    rows = []
    for dtype in dtypes_in(data):
        for chunk in chunks_in(data):
            try:
                h, a = hi.loc[(dtype, chunk)], ai.loc[(dtype, chunk)]
            except KeyError:
                continue
            rows.append(
                {
                    "dtype": dtype,
                    "chunk": fmt_chunk(chunk),
                    "hound avg (ms)": f"{h['avg_ms']:.3f}",
                    "hound cv": f"{h['cv']:.3f}",
                    "aus avg (ms)": f"{a['avg_ms']:.3f}",
                    "aus cv": f"{a['cv']:.3f}",
                    "speedup (±1σ)": _speedup_label(
                        h["avg_ms"], a["avg_ms"], h["stddev_ms"], a["stddev_ms"]
                    ),
                }
            )
    return rows


def write_markdown(df: pd.DataFrame, out_dir: Path, cache_label: str = "warm") -> None:
    r0 = df.iloc[0]
    ch_list = channels_in(df)
    ch_str = ", ".join(f"{c}ch" for c in ch_list)
    is_cold = cache_label == "cold"
    cache_note = " · **cold-cache**" if is_cold else ""
    lines = [
        "# WAV Benchmark Results\n",
        "> Generated by `analyse.py`.\n",
        f"- **Sample rate:** {SAMPLE_RATE:,} Hz | **Channels:** {ch_str}{cache_note}",
        f"- **Iterations:** {int(r0['iterations'])} | **Warmup:** {int(r0['warmup'])}\n",
        "> CV = coefficient of variation (σ / mean). Values > 0.5 indicate high variance (⚠).\n",
    ]

    for ch in ch_list:
        ch_label = f"{ch}ch"
        lines += [f"## {ch_label}\n"]

        for op, label in [("read", "Read"), ("write", "Write")]:
            op_df = df[(df["operation"] == op) & (df["channels"] == ch)]
            if not op_df.empty:
                lines += [
                    f"### {label}\n",
                    _md_rows_to_table(_bulk_rows(df, op, ch)),
                    "",
                ]

        if not is_cold:
            for op, label in [
                ("streamed-read", "Streamed Read"),
                ("streamed-write", "Streamed Write"),
            ]:
                op_df = df[(df["operation"] == op) & (df["channels"] == ch)]
                for dur in durations_in(op_df):
                    lines += [
                        f"### {label} --- {dur}s signal\n",
                        _md_rows_to_table(_streamed_rows(df, op, dur, ch)),
                        "",
                    ]

    path = out_dir / f"results_{cache_label}.md"
    path.write_text("\n".join(lines))
    print(f"  {path}")


def _run_warm_figures(warm_df: pd.DataFrame, out_dir: Path) -> None:
    ops = sorted(warm_df["operation"].unique())

    for ch in channels_in(warm_df):
        ch_label = f"{ch}ch"
        ch_df = warm_df[warm_df["channels"] == ch]

        # Bulk
        for op in ("read", "write"):
            if op not in ops:
                continue
            fig_bulk(ch_df, op, "avg_ms", out_dir, ch_label)
            fig_bulk(ch_df, op, "throughput_mbs", out_dir, ch_label)

            op_df = ch_df[
                (ch_df["operation"] == op) & ch_df["library"].isin(["hound", "aus"])
            ]
            dur_labels = [f"{d}s" for d in durations_in(op_df)]
            fig_speedup(
                ch_df,
                op,
                pivot_col="duration_s",
                x_labels=dur_labels,
                xlabel="Signal duration",
                title=f"{'Read' if op == 'read' else 'Write'} speedup  ·  audio_samples_io vs hound  ·  {ch_label}",
                out_dir=out_dir,
                filename=f"speedup_{op}_{ch_label}.png",
            )

        # Streaming
        for op in ("streamed-read", "streamed-write"):
            if op not in ops:
                continue
            op_df = ch_df[ch_df["operation"] == op]

            for dur in durations_in(op_df):
                dur_df = op_df[op_df["duration_s"] == dur]
                fig_streamed(ch_df, op, "avg_ms", dur, out_dir, ch_label)
                fig_streamed(ch_df, op, "throughput_mbs", dur, out_dir, ch_label)

                chunk_labels = [fmt_chunk(c) for c in chunks_in(dur_df)]
                op_label = (
                    "Streamed Read" if op == "streamed-read" else "Streamed Write"
                )
                fig_speedup(
                    dur_df,
                    op,
                    pivot_col="chunk_size",
                    x_labels=chunk_labels,
                    xlabel="Chunk size (samples)",
                    title=f"{op_label} speedup  ·  {dur}s signal  ·  audio_samples_io vs hound  ·  {ch_label}",
                    out_dir=out_dir,
                    filename=f"speedup_{op.replace('-', '_')}_{dur}s_{ch_label}.png",
                )


def _run_cold_figures(cold_df: pd.DataFrame, out_dir: Path) -> None:
    for ch in channels_in(cold_df):
        ch_label = f"{ch}ch_cold"
        ch_df = cold_df[cold_df["channels"] == ch]

        if "read" in cold_df["operation"].unique():
            fig_bulk(ch_df, "read", "avg_ms", out_dir, ch_label)
            fig_bulk(ch_df, "read", "throughput_mbs", out_dir, ch_label)

            op_df = ch_df[
                (ch_df["operation"] == "read") & ch_df["library"].isin(["hound", "aus"])
            ]
            dur_labels = [f"{d}s" for d in durations_in(op_df)]
            fig_speedup(
                ch_df,
                "read",
                pivot_col="duration_s",
                x_labels=dur_labels,
                xlabel="Signal duration",
                title=f"Read speedup (cold cache)  ·  audio_samples_io vs hound  ·  {ch}ch",
                out_dir=out_dir,
                filename=f"speedup_read_{ch_label}.png",
            )


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--csv", default="results/combined.csv")
    ap.add_argument("--out", default="figures")
    args = ap.parse_args()

    csv_path, out_dir = Path(args.csv), Path(args.out)
    if not csv_path.exists():
        print(f"error: {csv_path} not found", file=sys.stderr)
        sys.exit(1)

    out_dir.mkdir(parents=True, exist_ok=True)
    setup_style()

    print(f"Loading {csv_path} ...")
    df = load(csv_path)
    warm_df = df[df["cold_cache"] == 0].copy()
    cold_df = df[df["cold_cache"] == 1].copy()

    ops = sorted(df["operation"].unique())
    chs = channels_in(df)
    print(
        f"  {len(df)} rows | ops: {ops} | dtypes: {list(df['dtype'].unique())} | channels: {chs}"
    )
    print(f"  warm rows: {len(warm_df)} | cold rows: {len(cold_df)}")

    if not warm_df.empty:
        print("\nFigures (warm cache) ...")
        _run_warm_figures(warm_df, out_dir)

    if not cold_df.empty:
        print("\nFigures (cold cache) ...")
        _run_cold_figures(cold_df, out_dir)

    print("\nMarkdown ...")
    if not warm_df.empty:
        write_markdown(warm_df, out_dir, "warm")
    if not cold_df.empty:
        write_markdown(cold_df, out_dir, "cold")

    n = len(list(out_dir.glob("*.png")))
    print(f"\nDone --- {n} figures + markdown → {out_dir}/")


if __name__ == "__main__":
    main()
