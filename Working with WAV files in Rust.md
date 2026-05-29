---
author: Dr Jack Geraghty
---

# Working with WAV Files in Rust

## A comparison of Hound and audio_samples for reading and writing WAV 

**Disclosure:** I am the developer of the crates evaluated in this article: [`audio_samples`](https://github.com/jmg049/audio_samples), [`audio_samples_io`](https://github.com/jmg049/audio_samples_io), [`wavers`](https://github.com/jmg049/wavers), [`i24`](https://github.com/jmg049/i24), and [`spectrograms`](https://github.com/jmg049/spectrograms). To support independent verification, the benchmark harness, raw timing data, and analysis scripts are published alongside the article.

---

<figure style="margin: 2em 0; text-align: center;">
<img src="figures/speedup_read_1ch_cold.png" alt="Heatmap showing bulk read speedup of audio_samples_io over hound across signal durations and sample types, mono, approximately cold cache" style="max-width: 100%; height: auto;">
<figcaption style="font-size: 0.85em; color: #555; margin-top: 0.6em; text-align: left; max-width: 680px; margin-left: auto; margin-right: auto; line-height: 1.5;">
Bulk read speedup of <code>audio_samples_io</code> over <code>hound</code> (speedup = <code>hound</code> avg / <code>aus</code> avg), mono, approximately cold cache. Sample types: <code>i16</code>, <code>i32</code>, <code>f32</code> at 44,100 Hz; 30 iterations per cell. This represents storage-bound single-pass performance — the practical floor for most workloads. Warm-cache and repeated-access results are in the sections below.
</figcaption>
</figure>

> **Quick reference**
> 
> | Use `hound` when | Use `audio_samples_io` when |
> |:---|:---|
> | Zero transitive dependencies is a hard requirement (embedded, WASM, audited supply chains) | Performance matters — faster reads at every cache regime, faster `i32`/`f32` writes |
> | Integrating with an existing `hound`-based codebase | You want a typed, channel-aware API that encodes format invariants at compile time |
> | Write-only workload with `i16` at chunk sizes ≤ 512 samples (the one remaining parity case) | Your audio pipeline continues beyond I/O — the same `AudioSamples<T>` type carries through resampling, filtering, and spectral analysis |

WAV files (`.wav`) are among the most common formats for storing sampled audio data. The format is uncompressed by default, preserving quality at the cost of file size, though compressed variants exist.

In Rust, the de facto library for reading and writing `.wav` files is [`hound` by Ruuda](https://github.com/ruuda/hound), with over 600 stars on GitHub at the time of writing.

[`audio_samples`](https://github.com/jmg049/audio_samples) is a crate I have developed over the past two years, building on earlier work in the now-archived [`wavers`](https://github.com/jmg049/wavers) crate. It provides a unified, channel-aware representation of sampled audio data with a broad range of optional processing capabilities: statistical analysis, resampling, editing, filtering, spectral transforms, parametric EQ, and voice activity detection (VAD), among others. Spectral analysis is implemented via the [`spectrograms`](https://github.com/jmg049/spectrograms) crate, which was spun out of `audio_samples` to stand on its own; `audio_samples` integrates it fully through wrapper functions, passing the internal `ndarray` representation through to the underlying spectrogram routines.



`audio_samples` itself does not handle file I/O. That responsibility belongs to the companion crate [`audio_samples_io`](https://github.com/jmg049/audio_samples_io), which adds `.wav` and `.flac` read/write support (FLAC is a story for another day). The separation is intentional: `audio_samples` operates on audio already in memory, while `audio_samples_io` manages the disk-facing layer.

This article compares both crates for reading and writing `.wav` files: their APIs, their design philosophies, and their measured performance. The comparison is most directly applicable to general-purpose Rust audio work on standard targets; where zero-dependency constraints apply, `hound`'s position is largely uncontested and the performance comparison is secondary.

The article is structured in four parts: a side-by-side API walkthrough with working code examples, a benchmark methodology section, results across four conditions (bulk and streamed read/write), and a discussion covering implementation-level causes, practical implications, and broader ecosystem context. The results sections are intentionally detailed, with numbers that require careful qualification by cache regime and signal duration.[^repo] Readers primarily interested in the practical recommendation can read the API sections and skip directly to the Conclusion.

[^repo]: The benchmark harness, raw timing data, and analysis scripts are published at [github.com/jmg049/aus_vs_hound](https://github.com/jmg049/aus_vs_hound).

### A Look at Both Crates

#### hound

`hound` models a WAV file as a **spec** (the header metadata) paired with a typed sample iterator or writer.
You describe the file upfront with a `WavSpec`, then stream samples in or out one value at a time.
There is no built-in concept of frames or channels; interleaved samples are your responsibility to interpret.

##### Reading

```rust
use hound::WavReader;

fn main() {
    let mut reader = WavReader::open("audio.wav").unwrap();
    let spec = reader.spec();

    println!(
        "channels: {}, sample rate: {}, bit depth: {}",
        spec.channels, spec.sample_rate, spec.bits_per_sample
    );

    // Collect every sample into memory at once.
    let samples: Vec<i16> = reader
        .samples::<i16>()
        .map(|s| s.unwrap())
        .collect();

    println!("read {} samples", samples.len());
}
```

The type parameter on `.samples::<T>()` must match the file's bit depth and sample format; `hound` will return an error at runtime otherwise.
All samples land in a flat `Vec`; interleaving across channels is not unwrapped for you.

##### Writing

```rust
use hound::{WavSpec, WavWriter, SampleFormat};

fn main() {
    let spec = WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };

    let mut writer = WavWriter::create("output.wav", spec).unwrap();

    // Write a 1-second 440 Hz sine wave.
    for n in 0..44_100_u32 {
        let t = n as f32 / 44_100.0;
        let sample = (t * 440.0 * 2.0 * std::f32::consts::PI).sin();
        writer.write_sample((sample * i16::MAX as f32) as i16).unwrap();
    }

    writer.finalize().unwrap();
}
```

The `WavSpec` must be fully specified before writing starts; there is no way to change it mid-stream.
Calling `.finalize()` explicitly is strongly recommended: although `WavWriter`'s `Drop` implementation attempts to finalise the file, any error encountered during drop-time finalisation is silently discarded, so an explicit call is the only way to surface a finalisation failure to the caller.

##### Streamed Reading

`hound` does not have a dedicated chunked-read API; the `.samples()` iterator *is* the streaming primitive.
You can impose your own chunk window by driving the iterator manually.
The idiomatic first pass is to `.collect()` each chunk, but that allocates a fresh `Vec` on every iteration.
To avoid that, pre-allocate with `Vec::with_capacity` and `clear()` + `push()` the buffer each time:

```rust
use hound::WavReader;

fn main() {
    let mut reader = WavReader::open("audio.wav").unwrap();
    let mut iter = reader.samples::<i16>();
    let chunk_size = 1024;
    let mut buf: Vec<i16> = Vec::with_capacity(chunk_size);

    loop {
        buf.clear();
        for s in iter.by_ref().take(chunk_size) {
            buf.push(s.unwrap());
        }

        if buf.is_empty() {
            break;
        }

        // Process this chunk without holding the full file in memory.
        let _ = &buf;
    }
}
```

Because `.samples()` wraps `BufReader` internally, each call advances the read position; no seeking or manual offset tracking is needed.
The pre-allocated `buf` is reused across every chunk, so there is no heap allocation in the hot loop.

##### Streamed Writing

Writing is inherently streamed in `hound`: `write_sample` appends one value at a time.
The pattern for chunk-based writing is simply grouping your calls:

```rust
use hound::{WavSpec, WavWriter, SampleFormat};

fn main() {
    let spec = WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };

    let mut writer = WavWriter::create("output.wav", spec).unwrap();
    let chunk_size = 1024;

    // Simulate arriving chunks of audio data.
    let total_samples = 44_100_u32;
    let mut written = 0;

    while written < total_samples {
        let end = (written + chunk_size as u32).min(total_samples);
        for n in written..end {
            let t = n as f32 / 44_100.0;
            let sample = (t * 440.0 * 2.0 * std::f32::consts::PI).sin();
            writer.write_sample((sample * i16::MAX as f32) as i16).unwrap();
        }
        written = end;
    }

    writer.finalize().unwrap();
}
```

Each `write_sample` call goes through `hound`'s internal `BufWriter`, so the OS-level writes are already batched.
The chunk loop here reflects a realistic producer pattern where audio arrives in blocks rather than all at once.

#### audio_samples

`audio_samples` takes a different philosophy: audio is represented as a typed, channel-aware struct (`AudioSamples<T>`) rather than a flat iterator of interleaved values.
The I/O layer lives in the companion crate `audio_samples_io`, which exposes both one-shot and streamed read/write paths.
The library owns the concept of frames vs.
samples, so you get that structure for free rather than having to reconstruct it yourself.

##### Reading

```rust
use audio_samples_io;

fn main() {
    // Reads the file and returns an AudioSamples<i16>.
    // Channel count, sample rate, and frame count are embedded in the struct.
    let signal = audio_samples_io::read::<_, i16>("audio.wav").unwrap();
    println!("{:#}", signal);
}
```

The type parameter selects the in-memory representation you want; if the file's native sample type differs, `audio_samples_io` handles the conversion automatically.
`hound` requires the type parameter to match the file's bit depth exactly and will error at runtime if it does not.
The returned `AudioSamples<T>` knows its own shape (channel count, sample rate, and frame count are all embedded), so there is no flat interleaved `Vec` to decode manually.

> **Note:** 8-bit WAV samples are stored as unsigned bytes in the WAV spec. `audio_samples_io` follows this convention, so the correct Rust type for 8-bit audio is `u8`, not `i8`.

##### Writing

```rust
use audio_samples::{AudioSamples, sample_rate};
use audio_samples_io;
use std::time::Duration;

fn main() {
    // Build a 1-second 440 Hz sine wave; channel count and sample rate
    // are baked into the AudioSamples struct, so write() needs no spec.
    let signal = audio_samples::sine_wave::<i16>(
        440.0,
        Duration::from_secs(1),
        sample_rate!(44100),
        1.0,
    );

    audio_samples_io::write::<_, i16>("output.wav", &signal).unwrap();
}
```

A core design principle of `audio_samples` is that several classes of invalid audio should be *unrepresentable*. The `sample_rate!` macro produces a `NonZeroU32` at compile time: a zero sample rate is a build error, not a runtime panic. Channel count is enforced by `NonZeroU16`, making a zero-channel signal impossible to construct. Audio length uses `NonZeroUsize`, so empty audio cannot exist. These constraints are not just documentation conventions; they are part of the type signatures. Code that compiles is guaranteed to carry a non-zero sample rate, at least one channel, and at least one sample.

Because all the metadata travels with the `AudioSamples` value, there is no separate spec struct to fill out; the writer derives everything it needs from the signal itself.
For the one-shot `write()` path, there is no manual `finalize()` step — the write is completed before `write()` returns. The streamed write path does retain an explicit `finalize()` call so that errors during chunk flushing can be surfaced to the caller.

##### Streamed Reading

`audio_samples_io` has a dedicated streaming API.
You open a `StreamedReader`, read metadata from it directly, allocate a buffer sized to match, then pull frames in a loop — all from the same open handle:

```rust
use audio_samples::AudioSamples;
use audio_samples_io::open_streamed;
use std::num::{NonZeroU32, NonZeroUsize};

fn main() {
    let chunk_size = NonZeroUsize::new(1024).unwrap();

    // Open once: channel count and sample rate are available on the live reader.
    let mut streamed = open_streamed("audio.wav").unwrap();
    let sr = NonZeroU32::new(streamed.sample_rate()).unwrap();

    // Allocate once; read_frames_into reuses this allocation every iteration.
    let mut buffer = AudioSamples::<i16>::zeros_mono(chunk_size, sr);

    while streamed.remaining_frames() > 0 {
        streamed.read_frames_into(&mut buffer, chunk_size).unwrap();
        // Process this chunk without holding the full file in memory.
        let _ = &buffer;
    }
}
```

The notable design difference from `hound` here is that `read_frames_into` *requires* you to hand it a buffer; the API makes reuse the only option.
With `hound` the natural idiom is `.collect()`, which allocates a fresh `Vec` each iteration; you can avoid that by using `Vec::with_capacity(chunk_size)` and `clear()` + `extend()` in the loop, but the API does not push you there.
The `remaining_frames()` counter also gives an explicit termination condition rather than relying on an empty read.
The tradeoff is some call-site ceremony: constructing the buffer requires explicit `NonZeroUsize` and `NonZeroU32` values, obtained via `.unwrap()` in the example above; `hound`'s equivalent parameters are plain integers.

For multi-channel files, replace `zeros_mono` with `zeros_multi`:

```rust
use std::num::NonZeroU32;

let nz_ch = NonZeroU32::new(channels as u32).unwrap();
let mut buffer = AudioSamples::<i16>::zeros_multi(nz_ch, chunk_size, sr);
```

##### Streamed Writing

The streamed write path mirrors the read path.
You create a `StreamedWriter` with `create_streamed`, push `AudioSamples` chunks through `write_frames`, then call `finalize`:

```rust
use audio_samples::{AudioSamples, sample_rate};
use audio_samples_io::{create_streamed, traits::AudioStreamWrite};
use std::time::Duration;

fn main() {
    let sr = sample_rate!(44100);
    let total_frames: usize = 44_100; // 1 second

    // Pre-build one chunk of audio to push repeatedly.
    let chunk_dur = Duration::from_secs_f64(1024.0 / 44_100.0);
    let chunk = audio_samples::sine_wave::<i16>(440.0, chunk_dur, sr, 1.0);
    let chunk_frames = chunk.len().get();

    // The type parameter T fixes the on-disk sample format at creation time.
    let mut writer = create_streamed::<_, i16>("output.wav", 1, 44_100).unwrap();

    let mut written = 0;
    while written < total_frames {
        writer.write_frames(&chunk).unwrap();
        written += chunk_frames;
    }

    writer.finalize().unwrap();
}
```

The type parameter `T` on `create_streamed` fixes the on-disk sample format at the call site, playing the same role as `WavSpec` in `hound` but encoded in the type rather than a separate struct.
Unlike `hound`'s `write_sample`, `write_frames` accepts a whole `AudioSamples` chunk at once, so the chunking granularity is explicit in the call rather than implied by how many times you call the API.

### Methodology

All benchmarks were run on a single machine in release mode with link-time optimisation enabled (`lto = true`, `codegen-units = 1`, `opt-level = 3`).
The timing harness is a hand-written Rust program rather than a framework such as `criterion` or `divan`; this keeps the control flow and usage code for both libraries identical and directly readable.
The trade-off is that there is no automatic outlier rejection or throughput normalisation, so the raw numbers should be read in conjunction with the standard deviation and percentile columns rather than the mean alone.

The benchmark harness (`src/main.rs`), raw per-iteration results (`results/`), and analysis and plotting scripts (`analyse.py`) are all available in the same repository as this article. All reported figures can be reproduced by running `cargo run --release` followed by `python analyse.py`.

#### Crate versions

| Crate | Version |
|:------|:--------|
| `hound` | 3.5.1 |
| `audio_samples` | 1.0.9 (`bare-bones` feature) |
| `audio_samples_io` | 0.3.0 (`wav` feature) |

#### Test environment

| Property | Value |
|:---------|:------|
| CPU | AMD EPYC-Genoa (virtual) |
| vCPUs | 2 (1 thread per core, SMT disabled) |
| L2 cache | 2 × 1 MiB |
| L3 cache | 32 MiB (shared) |
| RAM | ≈ 3.8 GiB |
| Storage | QEMU virtual disk (`/dev/sda`, ROTA=0, `mq-deadline` scheduler) |
| OS | Ubuntu 26.04 LTS (Resolute Raccoon) |
| Kernel | 7.0.0-15-generic |
| CPU frequency governor | N/A (no cpufreq scaling reported) |
| SMT | disabled |
| CPU affinity | none (affinity list: CPUs 0,1) |
| ASLR | enabled (level 2, full) |
| Filesystem | ext4 — `rw,relatime` (no compression, no CoW) |

Several settings are worth noting as potential confounds.

The machine is a virtual machine (QEMU/KVM), so CPU clock characteristics and storage latency are subject to hypervisor scheduling.
No CPU-affinity pinning was applied; the OS scheduler was free to migrate the benchmark process between the two vCPUs.
The ext4 filesystem has no compression or copy-on-write overhead, so write benchmarks measure library serialisation and raw I/O time without the additional zstd cost present on btrfs+compress setups.
The 32 MiB L3 cache is the key architectural feature driving the speedup discontinuity in the results: files up to ≈ 30 MiB fit in the LLC across thousands of iterations, yielding LLC-bandwidth-limited reads for `audio_samples_io`; larger files spill to DRAM.

#### Signal

Each benchmark operates on a 440 Hz sine wave sampled at 44,100 Hz.
The signal is pre-generated once, before the timed loop begins, so what is measured is the I/O operation alone and not sample generation.
Write benchmarks measure creating a file, flushing all data, and finalising the header; read benchmarks measure opening a file and reading all of its data into memory.

Signal durations of 1, 5, 10, 30, 60, 300, and 600 seconds were tested, giving file sizes from ~88 KB (1 s, `i16`, mono) to ~212 MB (600 s, `i32`/`f32`, stereo).
Three sample types were benchmarked: `i16`, `i32`, and `f32`. Although `audio_samples` and `audio_samples_io` also support `i24` (24-bit integers), `hound` v3.5.1 does not implement the `i24` sample type; a cross-library comparison for that dtype is therefore not possible and `i24` is absent from the sweep.
Both mono (1-channel) and stereo (2-channel) signals were benchmarked to verify behaviour under interleaved multi-channel data.

#### Benchmark conditions

Four conditions were measured for each library, dtype, and duration:

- **Bulk read**: open the file and read all samples into a single in-memory allocation in one call.
- **Bulk write**: create the file, write all pre-generated samples, and finalise.
- **Streamed read**: open the file and pull samples into a fixed-size buffer chunk by chunk.
Both libraries use an allocation-free pattern here: `hound` uses `Vec::with_capacity(chunk_size)` with `clear()` + `push()` on each iteration; `audio_samples_io` uses `read_frames_into` with a pre-allocated `AudioSamples<T>` buffer.
Neither library allocates inside the hot loop.
- **Streamed write**: create the file, push a pre-generated chunk repeatedly until the full signal duration has been written, and finalise.

For the streaming conditions, chunk sizes of 512, 1024, 4096, 8192, and 16384 samples were swept.

#### Measurement

Iteration counts were scaled by signal duration to keep total benchmark time tractable while retaining statistical depth at shorter durations: **10,000 measured iterations** for signals of 1–10 s, **5,000** for 30–60 s, and **1,000** for 300–600 s.
All configurations used **50 warmup iterations**.
Warmup iterations are timed the same way but their results are discarded; they exist to bring file-system caches to a steady state and allow the allocator to settle before measurement begins.
Each iteration opens or creates the file from scratch; no file handle is reused across iterations.

`std::hint::black_box` is applied to the result of every read to prevent the compiler from eliminating the work as dead code.

The reported statistics are:

| Statistic | Meaning |
|:----------|:--------|
| avg | arithmetic mean across all measured iterations (10,000 for 1–10 s; 5,000 for 30–60 s; 1,000 for 300–600 s; warmup iterations excluded) |
| σ | standard deviation |
| p50 | median |
| p90 | 90th percentile |
| p99 | 99th percentile |

All times are in milliseconds.
Where σ is high or there is a large gap between p50 and p99, OS scheduling or filesystem jitter is likely a contributing factor.

### Benchmark Results

#### Bulk Read

<figure style="margin: 2em 0; text-align: center;">
<img src="figures/bulk_read_throughput_1ch.png" alt="Line chart: bulk read throughput in MB/s vs signal duration for hound and audio_samples_io, mono, warm cache" style="max-width: 100%; height: auto;">
<figcaption style="font-size: 0.85em; color: #555; margin-top: 0.6em; text-align: left; max-width: 680px; margin-left: auto; margin-right: auto; line-height: 1.5;">
<strong>Figure 1 — Bulk read throughput, mono (warm cache).</strong> <em>Y-axis:</em> throughput (MB/s), computed as logical WAV file size divided by mean read time per iteration. <em>X-axis:</em> signal duration (seconds, log scale, 1–600 s). Sample types: <code>i16</code>, <code>i32</code>, <code>f32</code> at 44,100 Hz; mono (1-channel). Iteration counts: 10,000 (1–10 s), 5,000 (30–60 s), 1,000 (300–600 s); 50 warmup iterations excluded. Shaded bands: ±1σ. Cache condition: warm — files are page-cache resident after warmup completes.
</figcaption>
</figure>

<figure style="margin: 2em 0; text-align: center;">
<img src="figures/speedup_read_1ch.png" alt="Line chart: bulk read speedup ratio (hound avg / aus avg) vs signal duration, mono, warm cache" style="max-width: 100%; height: auto;">
<figcaption style="font-size: 0.85em; color: #555; margin-top: 0.6em; text-align: left; max-width: 680px; margin-left: auto; margin-right: auto; line-height: 1.5;">
<strong>Figure 2 — Bulk read speedup over <code>hound</code>, mono (warm cache).</strong> <em>Y-axis:</em> speedup ratio (hound avg ÷ aus avg); values above 1 indicate <code>audio_samples_io</code> is faster. <em>X-axis:</em> signal duration (seconds, log scale). Sample types: <code>i16</code>, <code>i32</code>, <code>f32</code>; mono (1-channel). Same iteration counts as Figure 1. Error bars: propagated ±1σ uncertainty, σ<sub>R</sub> = R√((σ<sub>H</sub>/H)² + (σ<sub>A</sub>/A)²); full per-condition values are in <code>results/</code>.
</figcaption>
</figure>

`audio_samples_io` is substantially faster than `hound` for bulk reads across every dtype and duration tested.
The speedup is highest at intermediate durations and shows a pronounced discontinuity around the processor's last-level cache (LLC) capacity.
For `i16`, the speedup peaks above 150× for files that fit in the LLC (up to ≈ 300 s mono at 44,100 Hz on this machine), then drops to ≈ 11× at 600 s once the file exceeds the LLC and reads are bounded by DRAM bandwidth.
For `i32` and `f32` (twice as many bytes per sample), the LLC boundary falls earlier — between 60 s and 300 s — and the speedup there is 4–7×; at shorter durations where both sample types still fit in cache it is 34–119× depending on duration.
In absolute terms, `hound` reads a 600 s `i16` mono file (≈ 50 MB) in 305 ms on average; `audio_samples_io` reads the same file in 28.8 ms.
The spread across iterations is also much tighter for `audio_samples_io`: its standard deviation is consistently an order of magnitude smaller than `hound`'s at short and medium durations, and the gap between p50 and p99 is narrow.
`hound`'s p99 climbs noticeably above its mean at longer durations, reflecting the accumulation of per-sample overhead over many thousands of loop iterations.

The table below shows the `audio_samples_io` speedup over `hound` across all tested durations for mono signals. The figures above show the underlying throughput curves.

**Bulk read speedup (`hound` avg / `aus` avg), mono, warm cache**

| Duration | `i16` speedup | `i32` speedup | `f32` speedup |
|:---------|-------------:|-------------:|-------------:|
| 1 s  | 23× | 12× | 26× |
| 5 s  | 66× | 28× | 60× |
| 10 s | 86× | 34× | 71× |
| 30 s | 117× | 49× | 94× |
| 60 s | 153× | 54× | 119× |
| 300 s | 154× | 4× | 7× |
| 600 s | 11× | 4× | 7× |

> Speedup = `hound` avg / `aus` avg (values > 1 mean `audio_samples_io` is faster). Propagated 1σ uncertainty (σ_R = R√((σ_H/H)² + (σ_A/A)²)) is included in the full per-condition tables in `results/`.

The step-change in speedup between 60 s / 300 s and 600 s reflects the LLC capacity of this machine.
For `i16` mono, the file at 300 s is ≈ 26 MB and fits in the LLC; with thousands of warmup-plus-measured iterations the working set becomes LLC-resident, consistent with `audio_samples_io`'s read-into-Vec path reading at L3 bandwidth and producing the high 154× figure.
At 600 s the `i16` file is ≈ 50 MB and spills to DRAM, halving throughput; `hound`'s per-sample cost is unchanged, so the speedup collapses to 11×.
For `i32` and `f32` (twice as many bytes per sample) the LLC boundary falls a duration earlier — already at 300 s the 50 MB file is DRAM-bound — so the transition appears between 60 s (34–119×) and 300 s (4–7×).

A raw memory-map baseline (opening the file, mapping it read-only with a sequential-access hint, and casting the byte slice directly) was benchmarked alongside both libraries.
At short and medium durations where the file is LLC-resident, `audio_samples_io` is measurably faster than the mmap baseline (the `BufReader::read_to_end` path, once the allocation is warmed, outpaces mmap's per-page-fault cost).
At 600 s, where the file exceeds the LLC and reads are DRAM-bound, `audio_samples_io` (28.8 ms) is noticeably slower than mmap (8.7 ms), which reflects the difference between a kernel copy-to-userspace and a direct mapping; the mmap path avoids one copy.
`hound`'s throughput for `i16` warm-cache reads is approximately 166 MB/s, around 35× below the mmap ceiling at the same duration.

#### Stereo (2-channel) Bulk Read

<figure style="margin: 2em 0; text-align: center;">
<img src="figures/bulk_read_throughput_2ch.png" alt="Line chart: bulk read throughput in MB/s vs signal duration for hound and audio_samples_io, stereo, warm cache" style="max-width: 100%; height: auto;">
<figcaption style="font-size: 0.85em; color: #555; margin-top: 0.6em; text-align: left; max-width: 680px; margin-left: auto; margin-right: auto; line-height: 1.5;">
<strong>Figure 3 — Bulk read throughput, stereo (warm cache).</strong> Same axes, iteration counts, and cache condition as Figure 1. Sample types: <code>i16</code>, <code>i32</code>, <code>f32</code> at 44,100 Hz; stereo (2-channel, interleaved). Shaded bands: ±1σ. Stereo results closely mirror mono; deinterleaving overhead is negligible at these file sizes.
</figcaption>
</figure>

<figure style="margin: 2em 0; text-align: center;">
<img src="figures/speedup_read_2ch.png" alt="Line chart: bulk read speedup ratio (hound avg / aus avg) vs signal duration, stereo, warm cache" style="max-width: 100%; height: auto;">
<figcaption style="font-size: 0.85em; color: #555; margin-top: 0.6em; text-align: left; max-width: 680px; margin-left: auto; margin-right: auto; line-height: 1.5;">
<strong>Figure 4 — Bulk read speedup over <code>hound</code>, stereo (warm cache).</strong> Same axes and methodology as Figure 2. Sample types: <code>i16</code>, <code>i32</code>, <code>f32</code>; stereo (2-channel). Error bars: propagated ±1σ. Speedup profiles are essentially identical to mono (Figure 2), confirming that channel count does not affect the relative advantage at these signal lengths.
</figcaption>
</figure>

Stereo results are almost identical to mono across every dtype and duration.
The speedup ranges for stereo `i16` mirror those for mono (≈ 10× at 600 s; ≈ 60× at 5 s), and `i32`/`f32` stereo speedups are within 1–2× of their mono counterparts.
The deinterleaving step required for multi-channel reads adds negligible overhead at 44,100 Hz stereo relative to the bulk read gain, so channel count neither boosts nor penalises the result at these signal lengths.

#### Bulk Write

<figure style="margin: 2em 0; text-align: center;">
<img src="figures/bulk_write_throughput_1ch.png" alt="Line chart: bulk write throughput in MB/s vs signal duration for hound and audio_samples_io, mono" style="max-width: 100%; height: auto;">
<figcaption style="font-size: 0.85em; color: #555; margin-top: 0.6em; text-align: left; max-width: 680px; margin-left: auto; margin-right: auto; line-height: 1.5;">
<strong>Figure 5 — Bulk write throughput, mono.</strong> <em>Y-axis:</em> throughput (MB/s), computed as logical WAV file size divided by mean write time per iteration. <em>X-axis:</em> signal duration (seconds, log scale, 1–600 s). Sample types: <code>i16</code>, <code>i32</code>, <code>f32</code> at 44,100 Hz; mono (1-channel). Iteration counts: 10,000 (1–10 s), 5,000 (30–60 s), 1,000 (300–600 s); 50 warmup iterations excluded. Shaded bands: ±1σ. Write benchmarks run on ext4 (no compression); absolute MB/s figures reflect library serialisation and raw I/O time. Both libraries pass through the same filesystem path, so the speedup ratio is unaffected.
</figcaption>
</figure>

<figure style="margin: 2em 0; text-align: center;">
<img src="figures/speedup_write_1ch.png" alt="Line chart: bulk write speedup ratio (hound avg / aus avg) vs signal duration, mono" style="max-width: 100%; height: auto;">
<figcaption style="font-size: 0.85em; color: #555; margin-top: 0.6em; text-align: left; max-width: 680px; margin-left: auto; margin-right: auto; line-height: 1.5;">
<strong>Figure 6 — Bulk write speedup over <code>hound</code>, mono.</strong> <em>Y-axis:</em> speedup ratio (hound avg ÷ aus avg); values above 1 indicate <code>audio_samples_io</code> is faster. <em>X-axis:</em> signal duration (seconds, log scale). Sample types: <code>i16</code>, <code>i32</code>, <code>f32</code>; mono (1-channel). Error bars: propagated ±1σ. The <code>i16</code> series compares against <code>hound</code>'s optimised <code>SampleWriter16</code> path — already <code>hound</code>'s fastest available write API for that dtype.
</figcaption>
</figure>

`audio_samples_io` is consistently faster across all dtypes and durations tested.
For `i16` the margin is small (≈ 1.1× at 60 s), but for `i32` and `f32` it is substantially larger: approximately 3× for `i32` and 2.4× for `f32` at 60 s.
The table below shows this at 60 s, where variance is low enough to make the comparison meaningful.

**60 s · mono · write** (5,000 iterations)

| Benchmark | avg (ms) | σ (ms) | p50 (ms) | p90 (ms) | p99 (ms) |
|:----------|--------:|-------:|--------:|--------:|--------:|
| `hound` · `i16` | 3.603 | 0.575 | 3.393 | 4.482 | 5.492 |
| `aus` · `i16` | 3.374 | 0.520 | 3.202 | 3.974 | 5.200 |
| | | | | | |
| `hound` · `i32` | 19.263 | 2.413 | 18.662 | 20.472 | 30.074 |
| `aus` · `i32` | 6.313 | 0.808 | 6.077 | 7.086 | 9.389 |
| | | | | | |
| `hound` · `f32` | 15.940 | 1.662 | 15.558 | 17.308 | 21.527 |
| `aus` · `f32` | 6.648 | 0.795 | 6.357 | 7.575 | 9.464 |

Write variance is higher than read variance.
CV values at 60 s are in the 10–16% range for both libraries; p99 is 1.5–1.6× the mean, with no multi-second tail events.
At 600 s, p99 climbs to 40–210 ms for both libraries (from a mean of 29–175 ms), reflecting occasional OS scheduling pressure, but no ⚠ rows appear in the raw results (`results/bulk_600s.csv`).

#### Streamed Read

<figure style="margin: 2em 0; text-align: center;">
<img src="figures/streamed_read_throughput_60s_1ch.png" alt="Line chart: streamed read throughput in MB/s vs chunk size for hound and audio_samples_io, 60 s signal, mono, warm cache" style="max-width: 100%; height: auto;">
<figcaption style="font-size: 0.85em; color: #555; margin-top: 0.6em; text-align: left; max-width: 680px; margin-left: auto; margin-right: auto; line-height: 1.5;">
<strong>Figure 7 — Streamed read throughput, 60 s, mono (warm cache).</strong> <em>Y-axis:</em> throughput (MB/s), averaged over the full 60 s stream per iteration. <em>X-axis:</em> chunk size in samples (512, 1,024, 4,096, 8,192, 16,384). Fixed signal duration: 60 s at 44,100 Hz; mono (1-channel). Sample types: <code>i16</code>, <code>i32</code>, <code>f32</code>. Iteration count: 5,000; 50 warmup iterations excluded. Shaded bands: ±1σ. Cache condition: warm. Both libraries use allocation-free chunk patterns; neither allocates inside the hot loop.
</figcaption>
</figure>

<figure style="margin: 2em 0; text-align: center;">
<img src="figures/speedup_streamed_read_60s_1ch.png" alt="Line chart: streamed read speedup ratio (hound avg / aus avg) vs chunk size, 60 s signal, mono, warm cache" style="max-width: 100%; height: auto;">
<figcaption style="font-size: 0.85em; color: #555; margin-top: 0.6em; text-align: left; max-width: 680px; margin-left: auto; margin-right: auto; line-height: 1.5;">
<strong>Figure 8 — Streamed read speedup over <code>hound</code>, 60 s, mono (warm cache).</strong> <em>Y-axis:</em> speedup ratio (hound avg ÷ aus avg); values above 1 indicate <code>audio_samples_io</code> is faster. <em>X-axis:</em> chunk size in samples (512–16,384). Fixed duration: 60 s; mono (1-channel). Sample types: <code>i16</code>, <code>i32</code>, <code>f32</code>. Iteration count: 5,000. Error bars: propagated ±1σ. <code>hound</code>'s streamed-read time is flat across chunk sizes because its iterator advances one sample at a time regardless of how the caller groups reads.
</figcaption>
</figure>

The read advantage for `audio_samples_io` carries over to the streamed path and is stable across all chunk sizes.
At 60 s with a 4096-sample chunk: 43× for `i16`, 19× for `i32`, 11× for `f32`.
At 600 s (table below) the speedups are lower due to the LLC spill effect described above.
Within each duration, larger chunk sizes (4 K–16 K) tend to favour `audio_samples_io` slightly more than smaller ones.
`hound`'s streamed-read time is essentially flat across chunk sizes at a given duration, as expected: the iterator advances one sample at a time regardless of how the caller groups the results.
Standard deviation for `audio_samples_io` remains tight across the board; `hound`'s deviation is proportionally lower here than in bulk mode, but its absolute times are still much higher.

The table below shows absolute timings at 600 s with a 4096-sample chunk.

**600 s · mono · streamed read · chunk 4096** (1,000 iterations)

| Benchmark | avg (ms) | σ (ms) | p50 (ms) | p90 (ms) | p99 (ms) |
|:----------|--------:|-------:|--------:|--------:|--------:|
| `hound` · `i16` | 309.334 | 10.001 | 307.148 | 324.046 | 339.445 |
| `aus` · `i16` | 10.313 | 1.035 | 9.935 | 12.264 | 12.996 |
| | | | | | |
| `hound` · `i32` | 197.424 | 16.299 | 192.188 | 218.779 | 257.232 |
| `aus` · `i32` | 12.879 | 1.016 | 12.554 | 13.783 | 17.099 |
| | | | | | |
| `hound` · `f32` | 129.905 | 16.972 | 124.444 | 150.917 | 196.076 |
| `aus` · `f32` | 13.475 | 1.077 | 13.063 | 14.961 | 17.719 |

Speedups here are 30× for `i16`, 15× for `i32`, and 9.6× for `f32`.

`hound · i32` reads measurably slower than `hound · f32` despite identical on-disk byte counts.
The cause is in hound's `Sample::read()` dispatch: the `i32` implementation matches against six arms (`(1,8)`, `(2,16)`, `(3,24)`, `(4,24)`, `(4,32)`, and a wildcard) before reaching the 32-bit integer case, while the `f32` implementation reaches its only relevant arm `(4,32)` immediately.
Over 26.5 million samples in a 600 s file, this extra per-sample branch evaluation compounds to approximately 67 ms on this machine.

#### Streamed Write

<figure style="margin: 2em 0; text-align: center;">
<img src="figures/speedup_streamed_write_60s_1ch.png" alt="Line chart: streamed write speedup ratio (hound avg / aus avg) vs chunk size, 60 s signal, mono" style="max-width: 100%; height: auto;">
<figcaption style="font-size: 0.85em; color: #555; margin-top: 0.6em; text-align: left; max-width: 680px; margin-left: auto; margin-right: auto; line-height: 1.5;">
<strong>Figure 9 — Streamed write speedup over <code>hound</code>, 60 s, mono.</strong> <em>Y-axis:</em> speedup ratio (hound avg ÷ aus avg); values above 1 indicate <code>audio_samples_io</code> is faster; linear scale. <em>X-axis:</em> chunk size in samples (512, 1,024, 4,096, 8,192, 16,384). Fixed duration: 60 s at 44,100 Hz; mono (1-channel). Sample types: <code>i16</code>, <code>i32</code>, <code>f32</code>. Iteration count: 5,000; 50 warmup iterations excluded. Error bars: propagated ±1σ. Write benchmarks run on ext4 (no compression); both libraries are affected equally so the speedup ratio is unaffected by the filesystem setting.
</figcaption>
</figure>

Streamed write is the most variable condition of the four.
At the smallest chunk size (512 samples), both libraries are within a few percent of each other across all durations and dtypes, and in several cases `hound` is marginally ahead.
As chunk size increases the gap widens in favour of `audio_samples_io`; the speedup figure above shows this progression across all chunk sizes at 60 s (speedups of 6×, 4.9×, and 4.2× for `i16`, `i32`, and `f32` respectively at 4096 samples).
The 600 s results show moderate variance (CV 7–11%).
For short durations (1 s, 5 s), results are noisier in general and show less consistent ordering.

The table below shows the raw timing at 600 s with a 4096-sample chunk.

**600 s · mono · streamed write · chunk 4096** (1,000 iterations)

| Benchmark | avg (ms) | σ (ms) | p50 (ms) | p90 (ms) | p99 (ms) |
|:----------|--------:|-------:|--------:|--------:|--------:|
| `hound` · `i16` | 303.039 | 32.451 | 291.732 | 344.967 | 431.922 |
| `aus` · `i16` | 44.815 | 4.613 | 43.575 | 51.656 | 61.114 |
| | | | | | |
| `hound` · `i32` | 406.957 | 32.078 | 398.165 | 445.534 | 524.178 |
| `aus` · `i32` | 75.110 | 7.064 | 73.529 | 85.605 | 94.275 |
| | | | | | |
| `hound` · `f32` | 346.299 | 25.743 | 338.368 | 379.322 | 440.017 |
| `aus` · `f32` | 70.463 | 6.007 | 69.151 | 78.059 | 90.013 |

Speedups at this chunk size are 6.8× (`i16`), 5.4× (`i32`), and 4.9× (`f32`).
No ⚠ rows appear; all CV values are under 0.15, and p99 is within 2× the mean.

#### Cold-cache Bulk Read

<figure style="margin: 2em 0; text-align: center;">
<img src="figures/bulk_read_throughput_1ch_cold.png" alt="Line chart: bulk read throughput in MB/s vs signal duration for hound, audio_samples_io, and mmap baseline, mono, approximately cold cache" style="max-width: 100%; height: auto;">
<figcaption style="font-size: 0.85em; color: #555; margin-top: 0.6em; text-align: left; max-width: 680px; margin-left: auto; margin-right: auto; line-height: 1.5;">
<strong>Figure 10 — Bulk read throughput, mono (approximately cold cache).</strong> <em>Y-axis:</em> throughput (MB/s), computed as logical WAV file size divided by mean read time per iteration. <em>X-axis:</em> signal duration (seconds, log scale). Implementations: <code>hound</code>, <code>audio_samples_io</code>, and a raw <code>mmap</code> baseline (sequential read-ahead hint). Sample types: <code>i16</code>, <code>i32</code>, <code>f32</code>; mono (1-channel). <strong>Iteration count: 30</strong>. Cache eviction: <code>POSIX_FADV_DONTNEED</code> called before each iteration (advisory; eviction not independently verified). Results should be treated as approximately storage-bound rather than guaranteed cold. Error bars: ±1σ.
</figcaption>
</figure>

To characterise storage-bound performance, bulk read benchmarks were repeated with `posix_fadvise(POSIX_FADV_DONTNEED)` called before each measured iteration to advise the kernel to evict the file from the page cache.
`POSIX_FADV_DONTNEED` is advisory: the kernel may ignore it, and eviction was not independently verified (e.g. via `vmtouch`); results should be treated as approximately storage-bound rather than guaranteed cold.
The storage device here appears to be significantly faster than typical SATA storage, and `hound`'s per-sample CPU overhead is still clearly visible as a bottleneck even under cold-read conditions.

**600 s · mono · cold-cache read** (30 iterations)

| Benchmark | avg (ms) | σ (ms) | p50 (ms) |
|:----------|--------:|-------:|--------:|
| `hound` · `i16` | 314.222 | 6.187 | 312.140 |
| `aus` · `i16` | 60.786 | 3.769 | 60.504 |
| `mmap` · `i16` | 50.430 | 3.792 | 49.780 |
| | | | |
| `hound` · `i32` | 280.091 | 14.192 | 275.427 |
| `aus` · `i32` | 125.025 | 14.016 | 121.075 |
| `mmap` · `i32` | 98.319 | 9.555 | 96.485 |
| | | | |
| `hound` · `f32` | 457.221 | 8.162 | 455.917 |
| `aus` · `f32` | 127.315 | 6.608 | 125.241 |
| `mmap` · `f32` | 103.243 | 12.192 | 99.447 |

Even cold, the three implementations do **not** converge for any dtype at 600 s.
For `i32` and `f32`, `audio_samples_io` (125–127 ms) is ≈ 1.25× slower than mmap (98–103 ms), while `hound` (280–457 ms) is 2.2–3.6× slower than `audio_samples_io`.
The storage device is fast enough that `hound`'s per-sample CPU loop remains the binding constraint even in cold-read conditions — storage bandwidth is no longer the equaliser.

`i16` continues to show the largest `hound` disadvantage: 314 ms for `hound` vs 60.8 ms for `audio_samples_io` (5.2×), with `audio_samples_io` tracking closely behind mmap (50.4 ms).
The structural reason is unchanged from the warm-cache case: `hound`'s per-sample loop cannot keep up with the storage interface's delivery rate for the narrow `i16` type.

At short durations (1–5 s), both libraries are operating in the low-millisecond range where file-open latency and OS scheduling noise dominate, and results are noisier for all dtypes.

### Discussion

#### Why reads diverge: implementation-level analysis

The large read speedup for `audio_samples_io` is a direct consequence of how each library moves bytes from disk into memory, and examining the source code of both confirms this precisely.

`hound`'s `.samples::<T>()` iterator calls `Sample::read()` once per sample ([`read.rs` L733–736, hound v3.5.1](https://github.com/ruuda/hound/blob/v3.5.1/src/read.rs#L733-L736)).
Each `Sample::read()` call dispatches through the `ReadExt` trait to a type-specific function (`read_le_i16()`, `read_le_i32()`, and so on), which in turn calls `read_into()` in a per-byte loop ([`read.rs` L119–193, hound v3.5.1](https://github.com/ruuda/hound/blob/v3.5.1/src/read.rs#L119-L193)).
Even with `BufReader` absorbing the underlying syscall overhead, every sample still requires multiple function-call layers, a bitwise assembly step, and a type conversion.
Over a 600 s `i16` file (approximately 26.5 million samples), this amounts to 26.5 million iterations of that stack.

`audio_samples_io`'s non-streaming path takes a fundamentally different approach.
The entire file is loaded into memory via `BufReader::read_to_end()` or memory-mapped with a sequential read-ahead hint (`wav_file.rs`, lines 324–339, `audio_samples_io` v0.3.0).
For aligned data, samples are then made available via an unsafe `core::slice::from_raw_parts` reinterpretation of the raw byte slice (`data.rs`, lines 85–87, `audio_samples_io` v0.3.0): no copy, no per-sample conversion, just a type-level assertion that the bytes are already in the right format.
When the in-file type matches the requested output type (`S == T`), an additional `unsafe mem::transmute` skips even the `Vec` conversion step (`wav_file.rs`, line 203, `audio_samples_io` v0.3.0).
In the common case the entire "decode" step reduces to a single pointer cast followed by a bounds check.

The streaming path retains most of this advantage.
`audio_samples_io`'s `read_frames_into` issues a single `read()` call for all bytes of the requested chunk (`streaming.rs`, line 467, `audio_samples_io` v0.3.0), writes them into a reused internal `Vec<u8>` buffer, and then converts in bulk using `chunks_exact(2).map(...)` or `chunks_exact(4).map(...)`: one pass through the byte slice at the cost of a single iterator chain.
`hound`'s streamed path is identical to its bulk path from an internal standpoint: the same per-sample iterator, called the same number of times regardless of what chunk size the caller imposes.


#### Why writes diverge for i32 and f32

Both libraries ultimately write through a `BufWriter`, and the `BufWriter` coalesces per-sample calls before they reach the OS.
`hound`'s `write_sample()` dispatches through `Sample::write_padded()` on every call ([`write.rs` L429–437, hound v3.5.1](https://github.com/ruuda/hound/blob/v3.5.1/src/write.rs#L429-L437)), but each call writes only a small number of bytes to the internal write buffer; the kernel only sees the batched flushes.
`audio_samples_io` serialises a whole `AudioSamples` chunk in one pass and writes it with a single `write_all()`.
The difference is the number of function-call layers per byte written, not the number of syscalls.
On a faster storage path — where flush latency is lower and the CPU-side serialisation dominates a larger fraction of total time — this difference becomes more visible.
The 3× advantage for `i32` and 2.4× for `f32` at 60 s reflects exactly this: the server's storage can drain the `BufWriter` quickly enough that the per-sample dispatch in `hound` becomes the bottleneck, while `audio_samples_io`'s bulk `write_all` amortises that cost.

For `i16`, `hound`'s benchmark uses `get_i16_writer` (the `SampleWriter16` path), which pre-allocates an internal buffer and flushes with a single `write_all()`, closely mirroring what `audio_samples_io` does.
For `i32` and `f32`, `hound` has no equivalent optimised writer, so those types use `write_sample` per sample.
The `i16` write results therefore compare each library's best available path (≈ parity at 60 s); the `i32`/`f32` results compare `audio_samples_io`'s bulk serialisation against `hound`'s only available path for those types (3× and 2.4× respectively).

#### Predictability as an independent result

Beyond mean latency, the σ and percentile data carry their own information.
`audio_samples_io`'s read times have standard deviations consistently much smaller than `hound`'s at short and medium durations (e.g. 76× smaller at 60 s for `i16`); at 600 s, where both implementations are DRAM-bound, the ratio narrows to ≈ 9×.
This follows from the implementation: a path that performs one bulk read and one pointer cast has far fewer opportunities to accumulate scheduling jitter than one that iterates millions of times.
`hound`'s p99 values climb above the mean at longer durations (e.g. 15.5 ms p99 vs 14.0 ms mean for `i16` at 30 s), which reflects the probabilistic accumulation of interrupt-induced pauses over a long iteration loop.

In latency-sensitive applications (real-time transcription, live DSP, audio pipeline inference), worst-case latency is often the binding constraint rather than average throughput. In audio, "real-time" typically means completing processing within a fixed buffer period — for example, a 1,024-sample buffer at 44,100 Hz gives approximately 23 ms per callback, and a 256-sample buffer gives 5.8 ms. Under any such budget, the p99 is the relevant figure, not the mean; `audio_samples_io`'s tighter tail distribution means it is consistently closer to the available headroom, regardless of which buffer size defines the constraint.
A library whose p99 is 1.5× its p50 is easier to reason about under real-time deadlines than one whose tail is less bounded.

#### Streamed write chunk size as an ablation of call overhead

The chunk-size sweep in the streamed write condition is effectively an ablation of `write_frames` per-call cost.
At 512 samples, `write_frames` is called thousands of times per file: at 44,100 Hz, a 600 s file requires roughly 51,600 calls at this chunk size.
The per-call cost of each `write_frames` invocation (serialisation pass plus a `write_all`) is then comparable in aggregate to `hound`'s per-sample calls, and the two libraries converge.
At 16,384 samples per chunk, the call count drops to around 1,600 for the same file.
Each `write_frames` call now amortises its fixed overhead over 32× more data, while `hound`'s per-sample count is unchanged.
The monotone relationship between chunk size and speedup ratio, visible across all durations and dtypes, is consistent with this model.

The implication for practitioners is that `audio_samples_io`'s write advantage is not fixed: it grows with processing block size, and applications already operating on large audio buffers will benefit more.

#### The small-signal noise floor

Results at 1 s and 5 s durations, particularly for streamed writes, exhibit qualitatively different behaviour from longer durations.
Mean times fall in the sub-millisecond to low-millisecond range, where the resolution of `std::time::Instant` on Linux, OS scheduling quanta, and file-open/close overhead each constitute a non-negligible fraction of the measured interval.
Several 1 s streamed write configurations show σ values of 50–200% of the mean, and the p99 is occasionally an order of magnitude above the p50.
At these scales the benchmark is as much a measurement of OS scheduler behaviour as of library throughput.
The results demonstrate that both libraries can handle short files quickly, but any ranking at this granularity would require pinned CPU affinity, disabled frequency scaling, and clock sources with sub-microsecond resolution to be reliable.

#### Limitations

**Single machine, single filesystem.** All measurements were taken on the server described in the Test Environment section.
Because 50 warmup iterations precede measurement, read benchmarks reflect page-cache-warm performance at short and medium durations, and are DRAM-bandwidth-bound at long durations where the file exceeds the LLC.
Write results should be interpreted in light of the server's filesystem and storage configuration (ext4, `rw,relatime`, QEMU virtual disk; see Test Environment); absolute write times will differ on different filesystems or compression settings, though the relative ordering between libraries should be more stable.
Whether the relative ordering between libraries changes on spinning disk or network-attached storage is not tested here.

**Page-cache warmth and LLC effects.** The 50-iteration warmup brings file-sized working sets into the page cache before measurement begins.
For files smaller than the server's LLC, further iterations warm the LLC itself, and `audio_samples_io`'s throughput at those sizes reflects L3 bandwidth rather than DRAM bandwidth.
The headline speedup figures (up to 154× for `i16`) should therefore be understood as LLC-warm figures for appropriately-sized files.
For files larger than the LLC, performance is DRAM-bound and speedup figures are lower (11× for `i16` at 600 s, 4–7× for `i32`/`f32`).
The cold-cache benchmarks measure the storage-bound case directly; on this server, storage is fast enough that `hound`'s CPU bottleneck remains the dominant constraint even cold, so the convergence to near-parity observed on slower SATA storage does not occur here.

**Mono and stereo, but not surround.** Both mono (1-channel) and stereo (2-channel) signals were benchmarked.
Speedup profiles are essentially identical across the two channel counts, suggesting `audio_samples_io`'s SIMD deinterleaving path adds negligible overhead at these file sizes.
Whether the same holds for surround formats (5.1, 7.1), where the interleave stride is larger, remains untested.

**Reduced iteration counts at long durations.** Iteration counts were scaled to keep total benchmark time tractable: 1,000 iterations at 300–600 s.
The 600 s write results (CV 7–11%, p99 within 2× mean) are based on a relatively small number of observations and should be treated as indicative; a dedicated long-duration write experiment with tighter OS noise control would be needed to draw strong conclusions at that scale.

**`hound`'s write path varies by dtype.** For `i16`, the benchmark uses `hound`'s `SampleWriter16` path (`get_i16_writer`), which pre-allocates an internal buffer and flushes with a single `write_all` (hound's most efficient write API).
For `i32` and `f32`, no equivalent optimised writer exists in `hound`, so those dtypes use the standard `write_sample` per-sample path.
The `i16` write comparison is therefore already between each library's best available path; the `i32`/`f32` results reflect `hound`'s only available path for those types.

#### Contextualising the read numbers

The warm-cache read speedup (4–154× depending on dtype, duration, and LLC fit) is a real measurement, but it measures specific scenarios: either the file fits in the LLC (very high speedup) or the file is larger than the LLC but still DRAM-resident (moderate speedup).
Both are relevant for repeated-access workloads — multi-epoch training loops, hot file caches, or any pipeline that reads the same files many times.
For those workloads the advantage is large and architecturally significant: 10,000 ten-second `i16` files read from a warm cache take roughly 46 seconds with `hound` and under 0.6 seconds with `audio_samples_io` at the 86× speedup measured at 10 s.

For workloads that read each file once from a large dataset, the cold-cache numbers are the honest baseline.
On this server, storage is fast enough that `hound`'s per-sample CPU loop is the bottleneck even cold: `audio_samples_io` is 2.2–5.2× faster across all dtypes at 600 s cold.
Unlike on slower SATA storage, the gap does not close at longer durations for `i32` and `f32`.
Any benchmark that cites only LLC-warm figures without qualifying the LLC size is overstating the practical advantage for single-pass dataset work.

#### Practical implications

**Reads.** For workloads where files are read repeatedly or the working set fits in the LLC, `audio_samples_io`'s warm-cache advantage is large enough to matter and should be the deciding factor.
For cold, single-pass reads, `audio_samples_io` remains faster across all dtypes on this server (5.2× for `i16`, 2.2× for `i32`, 3.6× for `f32` at 600 s cold) — storage is no longer the equaliser here, because the server's storage is fast enough that `hound`'s CPU bottleneck is still exposed.

**Streaming reads.** The advantage transfers fully to the streaming path at every chunk size tested.
Applications can choose processing block sizes based entirely on algorithmic or latency requirements without any I/O throughput penalty.

**Writes.** The advantage is significant for `i32` and `f32` (3× and 2.4× at 60 s), and near-parity for `i16` (1.1×) where both libraries use an equivalent bulk-write path.
At the smallest chunk size (512 samples) both libraries are within a few percent; as chunk size grows, `audio_samples_io` pulls ahead.
For most applications the write difference does not dominate the decision, but `audio_samples_io` has no write-path caveat remaining and is the faster library at all chunk sizes of 1,024 and above across all dtypes.

#### API and ergonomics

Performance aside, the two libraries have meaningfully different APIs, and the ergonomics argument deserves equal weight in a practical decision.

`hound` models a WAV file as a spec plus a flat iterator of interleaved samples.
To read a file you must inspect the header, dispatch on bit depth and sample format, and collect into a `Vec<T>` with no channel structure.
Channel interleaving is your responsibility to untangle.
On the write side, `WavSpec` must be fully specified before writing starts, and `finalize()` should be called explicitly: although `WavWriter`'s `Drop` implementation attempts finalisation on drop, any I/O error during that drop-time finalisation is silently swallowed, so explicit `finalize()` is the only way to surface a failure to the caller.

`audio_samples_io` inverts this.
`read::<_, i16>(path)` is a single call that returns an `AudioSamples<i16>`, a channel-aware struct that carries its own sample rate, frame count, and channel count.
There is no manual header dispatch, no interleaved `Vec` to reinterpret, and for the one-shot write path no `finalize()` step — the write is completed before `write()` returns.
The `sample_rate!` macro produces a `NonZeroU32` at compile time, making a zero sample rate a build error rather than a runtime panic.
Even before reaching for advanced features (resampling, filtering, spectral transforms), the core API is genuinely less error-prone for audio work.

The argument that `hound` is "simpler" requires qualification.
It has a smaller surface area, but smaller surface area is not the same as simpler to use correctly.
The user is responsible for more decisions and more invariants.
`audio_samples_io` encodes more of those invariants in the type system and handles more of the bookkeeping automatically.

#### The broader ecosystem

Loading and saving audio is rarely the end goal. In practice, audio data is read, processed, and written, and the gap between `hound` and `audio_samples_io` widens considerably once you account for what happens in between.

`hound` stops at the file boundary. Once you have a `Vec<i16>`, any further processing requires separate crates: resampling, channel mixing, filtering, windowing, spectral analysis, and so on. Each of those crates has its own types, its own channel and sample-rate conventions, and its own error surface. The user is responsible for keeping everything consistent.

`audio_samples` is designed to be the representation you carry through the whole pipeline. The same `AudioSamples<T>` struct that `audio_samples_io` returns can be passed directly to the library's resampling, filtering, spectral transforms, parametric EQ, and VAD routines without any conversion or unwrapping. Spectral analysis is handled through wrapper functions that extract the internal `ndarray` representation and pass it to the [`spectrograms`](https://github.com/jmg049/spectrograms) crate, which was spun out of `audio_samples` because it could stand alone as a general-purpose library. From the caller's perspective it is still a single API; the split is an implementation detail. Sample rate and channel count travel with the signal throughout, so there is no opportunity for a mismatch between what a processing function receives and what the format implies.

For projects that will do any meaningful audio work, this matters more than the I/O benchmark numbers. It is not just that `audio_samples_io` reads faster; it is that the loaded audio arrives in the right type to continue working with, across a growing ecosystem of crates that all share the same representation, under the same type-safety guarantees, without any conversion layer between them. The dependency count discussed above should be read in this light: crates like `rayon` and `ndarray` are there to support the processing capabilities, not the I/O path, and a project that would pull them in independently for its own work gets them for free.

#### Dependencies

`hound` v3.5.1 has zero production dependencies. The `cpal` entry in its dependency tree is a dev-dependency used for playback examples only; nothing in it reaches any downstream consumer of the library. Any project that depends on `hound` compiles exactly `hound` and nothing else.

`audio_samples_io` v0.3.0 (with the `bare-bones` feature on `audio_samples` v1.0.9 and the `wav` feature on `audio_samples_io`) brings a moderate but well-motivated tree. The direct production dependencies of `audio_samples_io` itself are: `bytemuck` (zero-copy type casting), `memmap2` (memory-mapped files, which requires `libc`), `ndarray`, `non-empty-iter`, `non-empty-slice`, and `thiserror`. The `audio_samples` crate adds: `bytemuck`, `i24` (24-bit integer type, which transitively requires `ndarray` and `num-traits`), `ndarray`, `non-empty-iter`, `non-empty-slice`, `num-complex`, `num-traits`, and `thiserror`.

Every dependency in this list has a clear role. `bytemuck` and `memmap2` back the fast zero-copy I/O path. `ndarray` is the storage layer for `AudioSamples<T>`. `thiserror` handles error types. `num-traits` arrives transitively via `i24`, which enables 24-bit integer support. `non-empty-iter` and `non-empty-slice` are what enforce the non-empty audio and non-zero channel guarantees at the type level. None of these are incidental. Notably, the heavier optional capabilities of `audio_samples` (parallel processing via `rayon`, plotting support) are not present under `bare-bones`; they are behind separate feature flags and incur no compile cost unless explicitly enabled. When the spectral transform feature is enabled, [`spectrograms`](https://github.com/jmg049/spectrograms) enters the graph as an additional dependency; it is optional and incurs no cost when not used.

All of the crates listed are among the most-downloaded and actively maintained in the Rust ecosystem. `bytemuck`, `ndarray`, and `thiserror` in particular are widely used in Rust numerical computing, so the marginal cost of adding `audio_samples_io` is often lower than the raw count implies.

`hound`'s zero-dependency position remains a genuine advantage in specific contexts: embedded or `no_std` targets, environments with strict supply-chain audit requirements, or projects where compile time is tightly budgeted. For general-purpose audio work on a standard target, the trade-off is whether the listed crates are acceptable given the performance and ergonomic gains on the other side, and for Rust projects already doing numerical or audio processing work, several will already be in the graph.

#### Future work

Several extensions would strengthen or broaden these findings.

A surround-format sweep (5.1, 7.1) would test whether `audio_samples_io`'s SIMD deinterleaving path remains cost-free as the channel count grows, and would also exercise the `audio_samples_io` streamed multi-channel path which is not covered here.

Profiling `hound`'s read path under a sampling profiler (e.g. `perf record` with `--call-graph dwarf`) would confirm whether the bottleneck is the trait-dispatch iteration, the per-byte `ReadExt` calls, or the `BufReader` refill logic, and would guide any targeted optimisation of `hound` itself.
The cold-cache `i16` result (where `hound` cannot saturate the storage interface despite the CPU being nominally idle) makes the CPU-side bottleneck hypothesis particularly worth verifying.

Finally, repeating the cold-cache read benchmarks on NVMe storage (where sequential read bandwidth exceeds 3,000 MB/s) would test whether `hound`'s CPU bottleneck on `i16` persists at higher bus speeds, or whether the gap closes once the storage layer is fast enough to keep the per-sample pipeline continuously fed.

### Conclusion

Both libraries solve the same problem (reading and writing WAV files in Rust), but they do so from fundamentally different positions.

`hound` is a well-established library with a long track record in the Rust ecosystem. Its API is minimal and its behaviour is predictable, but "minimal" is not the same as simple to use correctly: the user is responsible for manual header dispatch, interpreting flat interleaved output, and a `finalize()` call whose omission causes any finalisation error to be silently swallowed by the `Drop` implementation rather than returned to the caller. Its one unambiguous advantage is zero transitive dependencies, which carries real weight in environments where the dependency graph is audited or constrained.

`audio_samples_io` takes a structurally different position. Its read path (a bulk memory load followed by a pointer cast) is inherently faster than any per-sample iterator, and the benchmarks confirm it: up to 154× faster on LLC-warm reads, 4–119× for DRAM-warm reads at intermediate durations, and 2.2–5.2× faster on cold reads — because on this server's fast storage, `hound`'s per-sample CPU loop remains the bottleneck even without a page-cache advantage. The write advantage is substantial for `i32` and `f32` (3× and 2.4× respectively at 60 s) and near-parity for `i16`, where both libraries use an equivalent bulk-write path. Beyond raw performance, the API is easier to use correctly: a single read call returns a typed, channel-aware struct with sample rate and frame count embedded; writes complete atomically; and invalid audio is structurally harder to construct by accident. With the `bare-bones` feature, the dependency tree is moderate and every crate in it has a clear, specific purpose; the heavier optional capabilities (parallel processing, plotting) are behind separate feature flags and compile only when requested.

The warm-cache speedup figures deserve context. LLC-warm figures (up to 154×) apply when the file fits in the processor's LLC — typical for repeated access to smaller files. DRAM-warm figures (4–119×) apply when the file is larger than the LLC but still in-memory. Cold figures (2–5×) apply to single-pass reads on fast storage. A benchmark that cites only the highest figures without qualifying the cache regime overstates the practical advantage for large single-pass datasets — though on NVMe-class storage, the cold figures would likely remain meaningful.

For new Rust projects doing audio work, `audio_samples_io` is the stronger default: faster across reads at every measured cache regime, meaningfully faster for `i32`/`f32` writes, less error-prone by construction, and the dependency overhead is manageable with feature flags. `hound` remains the right choice when zero transitive dependencies is a hard requirement, when integrating with an existing `hound`-based codebase, or when the workload is write-only with `i16` at small chunk sizes — the one remaining scenario where performance is near-parity.