use std::{
    fmt::Display,
    io::Write as IoWrite,
    num::{NonZeroU32, NonZeroUsize},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use audio_samples::{AudioSamples, StandardSample, operations::AudioChannelOps, sample_rate};
use audio_samples_io::{
    create_streamed, open_streamed,
    traits::{AudioStreamWrite, AudioStreamWriter},
};

pub struct BenchResult {
    name: String,
    #[allow(dead_code)]
    dtype: String,
    #[allow(dead_code)]
    duration: Duration,
    timings: Vec<f64>,
}

impl BenchResult {
    pub fn new(name: &str, dtype: &str, duration: Duration) -> Self {
        Self {
            name: name.to_string(),
            dtype: dtype.to_string(),
            duration,
            timings: Vec::new(),
        }
    }

    pub fn add_timing(&mut self, timing: f64) {
        self.timings.push(timing);
    }

    pub fn average(&self) -> f64 {
        self.timings.iter().sum::<f64>() / self.timings.len() as f64
    }

    pub fn variance(&self) -> f64 {
        let mean = self.average();
        self.timings.iter().map(|t| (t - mean).powi(2)).sum::<f64>() / self.timings.len() as f64
    }

    pub fn std_dev(&self) -> f64 {
        self.variance().sqrt()
    }

    /// Coefficient of variation: σ / mean. Values > 0.5 indicate high variance.
    pub fn cv(&self) -> f64 {
        self.std_dev() / self.average()
    }

    pub fn p50(&self) -> f64 {
        let mut s = self.timings.clone();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mid = s.len() / 2;
        if s.len() % 2 == 0 { (s[mid - 1] + s[mid]) / 2.0 } else { s[mid] }
    }

    pub fn p90(&self) -> f64 {
        let mut s = self.timings.clone();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        s[(s.len() as f64 * 0.9).ceil() as usize - 1]
    }

    pub fn p99(&self) -> f64 {
        let mut s = self.timings.clone();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        s[(s.len() as f64 * 0.99).ceil() as usize - 1]
    }
}

impl Display for BenchResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: avg={:.3}ms σ={:.3}ms p50={:.3}ms p90={:.3}ms p99={:.3}ms",
            self.name,
            self.average(),
            self.std_dev(),
            self.p50(),
            self.p90(),
            self.p99(),
        )
    }
}

pub struct BenchConfig {
    pub iterations: usize,
    pub warmup_iterations: usize,
    pub signal_duration: Duration,
    pub signal_dtype: String,
    pub chunk_size: usize,
    pub channels: u32,
    /// Drop the page cache before each measured read iteration (Linux only).
    pub cold_cache: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchKind {
    Read,
    Write,
    StreamedRead,
    StreamedWrite,
}

impl BenchKind {
    fn label(self) -> &'static str {
        match self {
            BenchKind::Read => "Read",
            BenchKind::Write => "Write",
            BenchKind::StreamedRead => "Streamed Read",
            BenchKind::StreamedWrite => "Streamed Write",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            BenchKind::Read => "read",
            BenchKind::Write => "write",
            BenchKind::StreamedRead => "streamed-read",
            BenchKind::StreamedWrite => "streamed-write",
        }
    }

    fn short(self) -> &'static str {
        match self {
            BenchKind::Read => "read   ",
            BenchKind::Write => "write  ",
            BenchKind::StreamedRead => "s-read ",
            BenchKind::StreamedWrite => "s-write",
        }
    }
}

/// Advise the OS to evict `fp` from the page cache (Linux: posix_fadvise DONTNEED).
/// Call before each cold-cache read iteration.
fn drop_page_cache(fp: &Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::AsRawFd;
        let f = std::fs::File::open(fp)
            .map_err(|e| format!("cold-cache open '{}' failed: {e}", fp.display()))?;
        let ret = unsafe {
            libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED)
        };
        if ret != 0 {
            return Err(format!("posix_fadvise(DONTNEED) returned {ret}"));
        }
        return Ok(());
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = fp;
        Err("--cold-cache requires Linux (posix_fadvise DONTNEED)".to_string())
    }
}

fn bench_hound_read(bench_cfg: &BenchConfig) -> Result<BenchResult, String> {
    let fp = PathBuf::from(format!(
        "resources/sine_{}s_{}_{}ch.wav",
        bench_cfg.signal_duration.as_secs(),
        bench_cfg.signal_dtype,
        bench_cfg.channels,
    ));

    if !fp.exists() {
        return Err(format!("File does not exist: {}", fp.display()));
    }

    let spec = hound::WavReader::open(&fp)
        .map_err(|e| format!("Failed to probe {}: {}", fp.display(), e))?
        .spec();

    fn run_for_type<T: hound::Sample>(fp: &Path, cfg: &BenchConfig) -> Result<BenchResult, String> {
        for _ in 0..cfg.warmup_iterations {
            let mut reader = hound::WavReader::open(fp)
                .map_err(|e| format!("Warmup open failed: {}", e))?;
            let _s: Vec<T> = reader.samples::<T>()
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Warmup read failed: {}", e))?;
            std::hint::black_box(&_s);
        }

        let mut result = BenchResult::new(
            &format!("hound · {} · {}ch", cfg.signal_dtype, cfg.channels),
            &cfg.signal_dtype,
            cfg.signal_duration,
        );

        for _ in 0..cfg.iterations {
            if cfg.cold_cache { drop_page_cache(fp)?; }
            let start = Instant::now();
            let mut reader = hound::WavReader::open(fp)
                .map_err(|e| format!("Measured open failed: {}", e))?;
            let _s: Vec<T> = reader.samples::<T>()
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Measured read failed: {}", e))?;
            std::hint::black_box(&_s);
            result.add_timing(start.elapsed().as_secs_f64() * 1000.0);
        }

        Ok(result)
    }

    match (spec.bits_per_sample, spec.sample_format) {
        (8,  hound::SampleFormat::Int)   => run_for_type::<i8>(&fp, bench_cfg),
        (16, hound::SampleFormat::Int)   => run_for_type::<i16>(&fp, bench_cfg),
        (32, hound::SampleFormat::Int)   => run_for_type::<i32>(&fp, bench_cfg),
        (32, hound::SampleFormat::Float) => run_for_type::<f32>(&fp, bench_cfg),
        _ => Err(format!(
            "Unsupported WAV format in {}: {}‑bit {:?}",
            fp.display(), spec.bits_per_sample, spec.sample_format
        )),
    }
}

fn bench_hound_streamed_read(bench_cfg: &BenchConfig) -> Result<BenchResult, String> {
    let fp = PathBuf::from(format!(
        "resources/sine_{}s_{}_{}ch.wav",
        bench_cfg.signal_duration.as_secs(),
        bench_cfg.signal_dtype,
        bench_cfg.channels,
    ));

    if !fp.exists() {
        return Err(format!("File does not exist: {}", fp.display()));
    }

    let spec = hound::WavReader::open(&fp)
        .map_err(|e| format!("Failed to probe {}: {}", fp.display(), e))?
        .spec();

    fn run_for_type<T: hound::Sample>(fp: &Path, cfg: &BenchConfig) -> Result<BenchResult, String> {
        let chunk = cfg.chunk_size;
        let mut buf: Vec<T> = Vec::with_capacity(chunk);

        for _ in 0..cfg.warmup_iterations {
            let mut reader = hound::WavReader::open(fp)
                .map_err(|e| format!("Warmup open failed: {}", e))?;
            let mut iter = reader.samples::<T>();
            loop {
                buf.clear();
                for s in iter.by_ref().take(chunk) {
                    buf.push(s.map_err(|e| format!("Warmup read failed: {}", e))?);
                }
                if buf.is_empty() { break; }
                std::hint::black_box(&buf);
            }
        }

        let mut result = BenchResult::new(
            &format!("hound · {} · {}ch", cfg.signal_dtype, cfg.channels),
            &cfg.signal_dtype,
            cfg.signal_duration,
        );

        for _ in 0..cfg.iterations {
            if cfg.cold_cache { drop_page_cache(fp)?; }
            let start = Instant::now();
            let mut reader = hound::WavReader::open(fp)
                .map_err(|e| format!("Measured open failed: {}", e))?;
            let mut iter = reader.samples::<T>();
            loop {
                buf.clear();
                for s in iter.by_ref().take(chunk) {
                    buf.push(s.map_err(|e| format!("Measured read failed: {}", e))?);
                }
                if buf.is_empty() { break; }
                std::hint::black_box(&buf);
            }
            result.add_timing(start.elapsed().as_secs_f64() * 1000.0);
        }

        Ok(result)
    }

    match (spec.bits_per_sample, spec.sample_format) {
        (8,  hound::SampleFormat::Int)   => run_for_type::<i8>(&fp, bench_cfg),
        (16, hound::SampleFormat::Int)   => run_for_type::<i16>(&fp, bench_cfg),
        (32, hound::SampleFormat::Int)   => run_for_type::<i32>(&fp, bench_cfg),
        (32, hound::SampleFormat::Float) => run_for_type::<f32>(&fp, bench_cfg),
        _ => Err(format!(
            "Unsupported WAV format in {}: {}‑bit {:?}",
            fp.display(), spec.bits_per_sample, spec.sample_format
        )),
    }
}

// Bulk write using SampleWriter16 — i16 only.
fn run_hound_write_bulk_i16(
    fp: &Path,
    n_frames: usize,
    spec: hound::WavSpec,
    cfg: &BenchConfig,
) -> Result<BenchResult, String> {
    let channels = spec.channels as usize;
    let sr = spec.sample_rate;
    let tau = 2.0 * std::f32::consts::PI * 440.0;
    let total_samples = (n_frames * channels) as u32;

    let samples: Vec<i16> = (0..n_frames)
        .flat_map(|frame| {
            let val = ((frame as f32 / sr as f32 * tau).sin() * i16::MAX as f32) as i16;
            std::iter::repeat(val).take(channels)
        })
        .collect();

    for _ in 0..cfg.warmup_iterations {
        let mut w = hound::WavWriter::create(fp, spec)
            .map_err(|e| format!("Warmup create failed: {}", e))?;
        {
            let mut sw = w.get_i16_writer(total_samples);
            for &s in &samples { sw.write_sample(s); }
            sw.flush().map_err(|e| format!("Warmup flush failed: {}", e))?;
        }
        w.finalize().map_err(|e| format!("Warmup finalize failed: {}", e))?;
    }

    let mut result = BenchResult::new(
        &format!("hound · {} · {}ch", cfg.signal_dtype, channels),
        &cfg.signal_dtype,
        cfg.signal_duration,
    );

    for _ in 0..cfg.iterations {
        let start = Instant::now();
        let mut w = hound::WavWriter::create(fp, spec)
            .map_err(|e| format!("Measured create failed: {}", e))?;
        {
            let mut sw = w.get_i16_writer(total_samples);
            for &s in &samples { sw.write_sample(s); }
            sw.flush().map_err(|e| format!("Measured flush failed: {}", e))?;
        }
        w.finalize().map_err(|e| format!("Measured finalize failed: {}", e))?;
        result.add_timing(start.elapsed().as_secs_f64() * 1000.0);
    }

    std::fs::remove_file(fp).map_err(|io| io.to_string())?;
    Ok(result)
}

// Pre-generates interleaved samples then writes them all at once.
fn run_hound_write_bulk<T>(
    fp: &Path,
    samples: &[T],
    spec: hound::WavSpec,
    cfg: &BenchConfig,
) -> Result<BenchResult, String>
where
    T: hound::Sample + Copy,
{
    for _ in 0..cfg.warmup_iterations {
        let mut w = hound::WavWriter::create(fp, spec)
            .map_err(|e| format!("Warmup create failed: {}", e))?;
        for &s in samples {
            w.write_sample(s).map_err(|e| format!("Warmup write failed: {}", e))?;
        }
        w.finalize().map_err(|e| format!("Warmup finalize failed: {}", e))?;
    }

    let mut result = BenchResult::new(
        &format!("hound · {} · {}ch", cfg.signal_dtype, spec.channels),
        &cfg.signal_dtype,
        cfg.signal_duration,
    );

    for _ in 0..cfg.iterations {
        let start = Instant::now();
        let mut w = hound::WavWriter::create(fp, spec)
            .map_err(|e| format!("Measured create failed: {}", e))?;
        for &s in samples {
            w.write_sample(s).map_err(|e| format!("Measured write failed: {}", e))?;
        }
        w.finalize().map_err(|e| format!("Measured finalize failed: {}", e))?;
        result.add_timing(start.elapsed().as_secs_f64() * 1000.0);
    }

    std::fs::remove_file(fp).map_err(|io| io.to_string())?;
    Ok(result)
}

// Streamed write using SampleWriter16 per chunk — i16 only.
fn run_hound_write_streamed_i16(
    fp: &Path,
    spec: hound::WavSpec,
    n_frames: usize,
    chunk_frames: usize,
    channels: usize,
    cfg: &BenchConfig,
) -> Result<BenchResult, String> {
    let sr = spec.sample_rate;
    let tau = 2.0 * std::f32::consts::PI * 440.0;

    for _ in 0..cfg.warmup_iterations {
        let mut w = hound::WavWriter::create(fp, spec)
            .map_err(|e| format!("Warmup create failed: {}", e))?;
        let mut i = 0;
        while i < n_frames {
            let end = (i + chunk_frames).min(n_frames);
            let chunk_samples = ((end - i) * channels) as u32;
            {
                let mut sw = w.get_i16_writer(chunk_samples);
                for frame in i..end {
                    let val = ((frame as f32 / sr as f32 * tau).sin() * i16::MAX as f32) as i16;
                    for _ in 0..channels { sw.write_sample(val); }
                }
                sw.flush().map_err(|e| format!("Warmup flush failed: {}", e))?;
            }
            i = end;
        }
        w.finalize().map_err(|e| format!("Warmup finalize failed: {}", e))?;
    }

    let mut result = BenchResult::new(
        &format!("hound · {} · {}ch", cfg.signal_dtype, channels),
        &cfg.signal_dtype,
        cfg.signal_duration,
    );

    for _ in 0..cfg.iterations {
        let start = Instant::now();
        let mut w = hound::WavWriter::create(fp, spec)
            .map_err(|e| format!("Measured create failed: {}", e))?;
        let mut i = 0;
        while i < n_frames {
            let end = (i + chunk_frames).min(n_frames);
            let chunk_samples = ((end - i) * channels) as u32;
            {
                let mut sw = w.get_i16_writer(chunk_samples);
                for frame in i..end {
                    let val = ((frame as f32 / sr as f32 * tau).sin() * i16::MAX as f32) as i16;
                    for _ in 0..channels { sw.write_sample(val); }
                }
                sw.flush().map_err(|e| format!("Measured flush failed: {}", e))?;
            }
            i = end;
        }
        w.finalize().map_err(|e| format!("Measured finalize failed: {}", e))?;
        result.add_timing(start.elapsed().as_secs_f64() * 1000.0);
    }

    std::fs::remove_file(fp).map_err(|io| io.to_string())?;
    Ok(result)
}

// Generates and writes in frame-indexed chunks; frame_gen produces one sample
// value per frame which is written for every channel (interleaved).
fn run_hound_write_streamed<T, F>(
    fp: &Path,
    spec: hound::WavSpec,
    n_frames: usize,
    chunk_frames: usize,
    channels: usize,
    frame_gen: F,
    cfg: &BenchConfig,
) -> Result<BenchResult, String>
where
    T: hound::Sample + Copy,
    F: Fn(usize) -> T,
{
    for _ in 0..cfg.warmup_iterations {
        let mut w = hound::WavWriter::create(fp, spec)
            .map_err(|e| format!("Warmup create failed: {}", e))?;
        let mut i = 0;
        while i < n_frames {
            let end = (i + chunk_frames).min(n_frames);
            for frame in i..end {
                let val = frame_gen(frame);
                for _ in 0..channels {
                    w.write_sample(val).map_err(|e| format!("Warmup write failed: {}", e))?;
                }
            }
            i = end;
        }
        w.finalize().map_err(|e| format!("Warmup finalize failed: {}", e))?;
    }

    let mut result = BenchResult::new(
        &format!("hound · {} · {}ch", cfg.signal_dtype, channels),
        &cfg.signal_dtype,
        cfg.signal_duration,
    );

    for _ in 0..cfg.iterations {
        let start = Instant::now();
        let mut w = hound::WavWriter::create(fp, spec)
            .map_err(|e| format!("Measured create failed: {}", e))?;
        let mut i = 0;
        while i < n_frames {
            let end = (i + chunk_frames).min(n_frames);
            for frame in i..end {
                let val = frame_gen(frame);
                for _ in 0..channels {
                    w.write_sample(val).map_err(|e| format!("Measured write failed: {}", e))?;
                }
            }
            i = end;
        }
        w.finalize().map_err(|e| format!("Measured finalize failed: {}", e))?;
        result.add_timing(start.elapsed().as_secs_f64() * 1000.0);
    }

    std::fs::remove_file(fp).map_err(|io| io.to_string())?;
    Ok(result)
}

fn bench_hound_write(bench_cfg: &BenchConfig) -> Result<BenchResult, String> {
    let fp = PathBuf::from(format!(
        "resources/tmp/hound_sine_{}s_{}_{}ch.wav",
        bench_cfg.signal_duration.as_secs(),
        bench_cfg.signal_dtype,
        bench_cfg.channels,
    ));
    if let Some(p) = fp.parent() { std::fs::create_dir_all(p).map_err(|e| e.to_string())?; }

    let sr = 44_100u32;
    let n = (bench_cfg.signal_duration.as_secs_f64() * sr as f64) as usize;
    let ch = bench_cfg.channels as usize;
    let tau = 2.0 * std::f32::consts::PI * 440.0;

    match bench_cfg.signal_dtype.as_str() {
        "i8" => {
            let samples: Vec<i8> = (0..n)
                .flat_map(|frame| {
                    let v = ((frame as f32 / sr as f32 * tau).sin() * i8::MAX as f32) as i8;
                    std::iter::repeat(v).take(ch)
                })
                .collect();
            run_hound_write_bulk(&fp, &samples, hound::WavSpec {
                channels: bench_cfg.channels as u16, sample_rate: sr,
                bits_per_sample: 8, sample_format: hound::SampleFormat::Int,
            }, bench_cfg)
        }
        "i16" => {
            run_hound_write_bulk_i16(&fp, n, hound::WavSpec {
                channels: bench_cfg.channels as u16, sample_rate: sr,
                bits_per_sample: 16, sample_format: hound::SampleFormat::Int,
            }, bench_cfg)
        }
        "i32" => {
            let samples: Vec<i32> = (0..n)
                .flat_map(|frame| {
                    let v = ((frame as f32 / sr as f32 * tau).sin() * i32::MAX as f32) as i32;
                    std::iter::repeat(v).take(ch)
                })
                .collect();
            run_hound_write_bulk(&fp, &samples, hound::WavSpec {
                channels: bench_cfg.channels as u16, sample_rate: sr,
                bits_per_sample: 32, sample_format: hound::SampleFormat::Int,
            }, bench_cfg)
        }
        "f32" => {
            let samples: Vec<f32> = (0..n)
                .flat_map(|frame| {
                    let v = (frame as f32 / sr as f32 * tau).sin();
                    std::iter::repeat(v).take(ch)
                })
                .collect();
            run_hound_write_bulk(&fp, &samples, hound::WavSpec {
                channels: bench_cfg.channels as u16, sample_rate: sr,
                bits_per_sample: 32, sample_format: hound::SampleFormat::Float,
            }, bench_cfg)
        }
        _ => Err(format!("Unsupported signal dtype: {}", bench_cfg.signal_dtype)),
    }
}

fn bench_hound_streamed_write(bench_cfg: &BenchConfig) -> Result<BenchResult, String> {
    let fp = PathBuf::from(format!(
        "resources/tmp/hound_streamed_sine_{}s_{}_{}ch.wav",
        bench_cfg.signal_duration.as_secs(),
        bench_cfg.signal_dtype,
        bench_cfg.channels,
    ));
    if let Some(p) = fp.parent() { std::fs::create_dir_all(p).map_err(|e| e.to_string())?; }

    let sr = 44_100u32;
    let n = (bench_cfg.signal_duration.as_secs_f64() * sr as f64) as usize;
    let chunk = bench_cfg.chunk_size;
    let ch = bench_cfg.channels;
    let tau = 2.0 * std::f32::consts::PI * 440.0;

    match bench_cfg.signal_dtype.as_str() {
        "i8" => run_hound_write_streamed(
            &fp,
            hound::WavSpec { channels: ch as u16, sample_rate: sr, bits_per_sample: 8,
                             sample_format: hound::SampleFormat::Int },
            n, chunk, ch as usize,
            |frame| ((frame as f32 / sr as f32 * tau).sin() * i8::MAX as f32) as i8,
            bench_cfg,
        ),
        "i16" => run_hound_write_streamed_i16(
            &fp,
            hound::WavSpec { channels: ch as u16, sample_rate: sr, bits_per_sample: 16,
                             sample_format: hound::SampleFormat::Int },
            n, chunk, ch as usize, bench_cfg,
        ),
        "i32" => run_hound_write_streamed(
            &fp,
            hound::WavSpec { channels: ch as u16, sample_rate: sr, bits_per_sample: 32,
                             sample_format: hound::SampleFormat::Int },
            n, chunk, ch as usize,
            |frame| ((frame as f32 / sr as f32 * tau).sin() * i32::MAX as f32) as i32,
            bench_cfg,
        ),
        "f32" => run_hound_write_streamed(
            &fp,
            hound::WavSpec { channels: ch as u16, sample_rate: sr, bits_per_sample: 32,
                             sample_format: hound::SampleFormat::Float },
            n, chunk, ch as usize,
            |frame| (frame as f32 / sr as f32 * tau).sin(),
            bench_cfg,
        ),
        _ => Err(format!("Unsupported signal dtype: {}", bench_cfg.signal_dtype)),
    }
}

/// Memory-mapped read baseline: establishes the upper bound on read throughput
/// by accessing all sample bytes with zero user-space copy.
///
/// The data offset is derived from the WAV header once before the timed loops.
/// Each iteration maps the file, reads every sample byte, then unmaps. With a
/// warm page cache this reflects pure memory-bandwidth cost; with --cold-cache
/// it reflects storage I/O throughput.
fn bench_mmap_read(bench_cfg: &BenchConfig) -> Result<BenchResult, String> {
    let fp = PathBuf::from(format!(
        "resources/sine_{}s_{}_{}ch.wav",
        bench_cfg.signal_duration.as_secs(),
        bench_cfg.signal_dtype,
        bench_cfg.channels,
    ));

    if !fp.exists() {
        return Err(format!("File does not exist: {}", fp.display()));
    }

    // Probe the header once to find where sample data begins.
    let data_offset = {
        let probe = hound::WavReader::open(&fp)
            .map_err(|e| format!("Failed to probe {}: {}", fp.display(), e))?;
        let spec = probe.spec();
        let n_samples = probe.len() as usize;
        let bytes_per_sample = (spec.bits_per_sample / 8) as usize;
        let data_bytes = n_samples * bytes_per_sample;
        let file_size = std::fs::metadata(&fp)
            .map_err(|e| e.to_string())?.len() as usize;
        file_size.saturating_sub(data_bytes)
    };

    for _ in 0..bench_cfg.warmup_iterations {
        let file = std::fs::File::open(&fp).map_err(|e| e.to_string())?;
        let mmap = unsafe {
            memmap2::MmapOptions::new().map(&file).map_err(|e| e.to_string())?
        };
        let sum: u64 = mmap[data_offset..].iter().copied().map(u64::from).sum();
        std::hint::black_box(sum);
    }

    let mut result = BenchResult::new(
        &format!("mmap · {} · {}ch", bench_cfg.signal_dtype, bench_cfg.channels),
        &bench_cfg.signal_dtype,
        bench_cfg.signal_duration,
    );

    for _ in 0..bench_cfg.iterations {
        if bench_cfg.cold_cache { drop_page_cache(&fp)?; }
        let start = Instant::now();
        let file = std::fs::File::open(&fp).map_err(|e| e.to_string())?;
        let mmap = unsafe {
            memmap2::MmapOptions::new().map(&file).map_err(|e| e.to_string())?
        };
        let sum: u64 = mmap[data_offset..].iter().copied().map(u64::from).sum();
        std::hint::black_box(sum);
        result.add_timing(start.elapsed().as_secs_f64() * 1000.0);
    }

    Ok(result)
}

fn bench_aus_read(bench_cfg: &BenchConfig) -> Result<BenchResult, String> {
    let fp = PathBuf::from(format!(
        "resources/sine_{}s_{}_{}ch.wav",
        bench_cfg.signal_duration.as_secs(),
        bench_cfg.signal_dtype,
        bench_cfg.channels,
    ));

    if !fp.exists() {
        return Err(format!("File does not exist: {}", fp.display()));
    }

    fn run_for_type<T: StandardSample>(fp: &Path, cfg: &BenchConfig) -> Result<BenchResult, String> {
        for _ in 0..cfg.warmup_iterations {
            let _s = audio_samples_io::read::<_, T>(fp)
                .map_err(|e| format!("Warmup read failed: {}", e))?;
            std::hint::black_box(&_s);
        }

        let mut result = BenchResult::new(
            &format!("aus · {} · {}ch", cfg.signal_dtype, cfg.channels),
            &cfg.signal_dtype,
            cfg.signal_duration,
        );

        for _ in 0..cfg.iterations {
            if cfg.cold_cache { drop_page_cache(fp)?; }
            let start = Instant::now();
            let _s = audio_samples_io::read::<_, T>(fp)
                .map_err(|e| format!("Measured read failed: {}", e))?;
            std::hint::black_box(&_s);
            result.add_timing(start.elapsed().as_secs_f64() * 1000.0);
        }

        Ok(result)
    }

    match bench_cfg.signal_dtype.as_str() {
        "i8"  => run_for_type::<u8>(&fp, bench_cfg),
        "i16" => run_for_type::<i16>(&fp, bench_cfg),
        "i32" => run_for_type::<i32>(&fp, bench_cfg),
        "f32" => run_for_type::<f32>(&fp, bench_cfg),
        _ => Err(format!("Unsupported signal dtype: {}", bench_cfg.signal_dtype)),
    }
}

fn bench_aus_streamed_read(bench_cfg: &BenchConfig) -> Result<BenchResult, String> {
    let fp = PathBuf::from(format!(
        "resources/sine_{}s_{}_{}ch.wav",
        bench_cfg.signal_duration.as_secs(),
        bench_cfg.signal_dtype,
        bench_cfg.channels,
    ));

    if !fp.exists() {
        return Err(format!("File does not exist: {}", fp.display()));
    }

    fn run_for_type<T: StandardSample + 'static>(fp: &Path, cfg: &BenchConfig) -> Result<BenchResult, String> {
        let nz_chunk = NonZeroUsize::new(cfg.chunk_size)
            .ok_or_else(|| "chunk_size must be > 0".to_string())?;

        // Open once: metadata is available on the live reader; no probe → drop → reopen needed.
        let mut streamed = open_streamed(fp)
            .map_err(|e| format!("Failed to open {}: {}", fp.display(), e))?;
        let channels = streamed.num_channels();
        let nz_sr = NonZeroU32::new(streamed.sample_rate())
            .ok_or_else(|| "sample_rate is zero".to_string())?;

        let mut buffer = if channels == 1 {
            AudioSamples::<T>::zeros_mono(nz_chunk, nz_sr)
        } else {
            let nz_ch = NonZeroU32::new(channels as u32)
                .ok_or_else(|| "channels is zero".to_string())?;
            AudioSamples::<T>::zeros_multi(nz_ch, nz_chunk, nz_sr)
        };

        // First warmup pass reuses the already-open reader; subsequent passes reopen.
        for i in 0..cfg.warmup_iterations {
            if i > 0 {
                streamed = open_streamed(fp)
                    .map_err(|e| format!("Warmup open failed: {}", e))?;
            }
            while streamed.remaining_frames() > 0 {
                streamed.read_frames_into(&mut buffer, nz_chunk)
                    .map_err(|e| format!("Warmup read failed: {}", e))?;
                std::hint::black_box(&buffer);
            }
        }
        drop(streamed);

        let mut result = BenchResult::new(
            &format!("aus · {} · {}ch", cfg.signal_dtype, cfg.channels),
            &cfg.signal_dtype,
            cfg.signal_duration,
        );

        for _ in 0..cfg.iterations {
            if cfg.cold_cache { drop_page_cache(fp)?; }
            let start = Instant::now();
            let mut streamed = open_streamed(fp)
                .map_err(|e| format!("Measured open failed: {}", e))?;
            while streamed.remaining_frames() > 0 {
                streamed.read_frames_into(&mut buffer, nz_chunk)
                    .map_err(|e| format!("Measured read failed: {}", e))?;
                std::hint::black_box(&buffer);
            }
            result.add_timing(start.elapsed().as_secs_f64() * 1000.0);
        }

        Ok(result)
    }

    match bench_cfg.signal_dtype.as_str() {
        "i8"  => run_for_type::<u8>(&fp, bench_cfg),
        "i16" => run_for_type::<i16>(&fp, bench_cfg),
        "i32" => run_for_type::<i32>(&fp, bench_cfg),
        "f32" => run_for_type::<f32>(&fp, bench_cfg),
        _ => Err(format!("Unsupported signal dtype: {}", bench_cfg.signal_dtype)),
    }
}

fn bench_aus_write(bench_cfg: &BenchConfig) -> Result<BenchResult, String> {
    let fp = PathBuf::from(format!(
        "resources/tmp/aus_sine_{}s_{}_{}ch.wav",
        bench_cfg.signal_duration.as_secs(),
        bench_cfg.signal_dtype,
        bench_cfg.channels,
    ));
    if let Some(p) = fp.parent() { std::fs::create_dir_all(p).map_err(|e| e.to_string())?; }

    fn run_for_type<T: StandardSample>(fp: &Path, cfg: &BenchConfig) -> Result<BenchResult, String> {
        let mono = audio_samples::sine_wave::<T>(440.0, cfg.signal_duration, sample_rate!(44100), 1.0);
        let signal = if cfg.channels == 1 {
            mono
        } else {
            mono.duplicate_to_channels(cfg.channels as usize)
                .map_err(|e| format!("duplicate_to_channels failed: {}", e))?
        };

        for _ in 0..cfg.warmup_iterations {
            audio_samples_io::write::<_, T>(fp, &signal)
                .map_err(|e| format!("Warmup write failed: {}", e))?;
        }

        let mut result = BenchResult::new(
            &format!("aus · {} · {}ch", cfg.signal_dtype, cfg.channels),
            &cfg.signal_dtype,
            cfg.signal_duration,
        );

        for _ in 0..cfg.iterations {
            let start = Instant::now();
            audio_samples_io::write::<_, T>(fp, &signal)
                .map_err(|e| format!("Measured write failed: {}", e))?;
            result.add_timing(start.elapsed().as_secs_f64() * 1000.0);
        }

        std::fs::remove_file(fp).map_err(|io| io.to_string())?;
        Ok(result)
    }

    match bench_cfg.signal_dtype.as_str() {
        "i8"  => run_for_type::<u8>(&fp, bench_cfg),
        "i16" => run_for_type::<i16>(&fp, bench_cfg),
        "i32" => run_for_type::<i32>(&fp, bench_cfg),
        "f32" => run_for_type::<f32>(&fp, bench_cfg),
        _ => Err(format!("Unsupported signal dtype: {}", bench_cfg.signal_dtype)),
    }
}

fn bench_aus_streamed_write(bench_cfg: &BenchConfig) -> Result<BenchResult, String> {
    let fp = PathBuf::from(format!(
        "resources/tmp/aus_streamed_sine_{}s_{}_{}ch.wav",
        bench_cfg.signal_duration.as_secs(),
        bench_cfg.signal_dtype,
        bench_cfg.channels,
    ));
    if let Some(p) = fp.parent() { std::fs::create_dir_all(p).map_err(|e| e.to_string())?; }

    fn run_for_type<T: StandardSample + 'static>(
        fp: &Path,
        cfg: &BenchConfig,
    ) -> Result<BenchResult, String> {
        let sr = sample_rate!(44100);
        let total_frames = (cfg.signal_duration.as_secs_f64() * 44100.0) as usize;
        let chunk_dur = Duration::from_secs_f64(cfg.chunk_size as f64 / 44100.0);

        let mono_chunk = audio_samples::sine_wave::<T>(440.0, chunk_dur, sr, 1.0);
        let chunk = if cfg.channels == 1 {
            mono_chunk
        } else {
            mono_chunk.duplicate_to_channels(cfg.channels as usize)
                .map_err(|e| format!("duplicate_to_channels failed: {}", e))?
        };
        let chunk_frames = chunk.samples_per_channel().get();

        for _ in 0..cfg.warmup_iterations {
            let mut writer = create_streamed::<&Path, T>(fp, cfg.channels as u16, 44100)
                .map_err(|e| format!("Warmup create failed: {}", e))?;
            let mut written = 0;
            while written < total_frames {
                writer.write_frames(&chunk)
                    .map_err(|e| format!("Warmup write failed: {}", e))?;
                written += chunk_frames;
            }
            writer.finalize()
                .map_err(|e| format!("Warmup finalize failed: {}", e))?;
        }

        let mut result = BenchResult::new(
            &format!("aus · {} · {}ch", cfg.signal_dtype, cfg.channels),
            &cfg.signal_dtype,
            cfg.signal_duration,
        );

        for _ in 0..cfg.iterations {
            let start = Instant::now();
            let mut writer = create_streamed::<&Path, T>(fp, cfg.channels as u16, 44100)
                .map_err(|e| format!("Measured create failed: {}", e))?;
            let mut written = 0;
            while written < total_frames {
                writer.write_frames(&chunk)
                    .map_err(|e| format!("Measured write failed: {}", e))?;
                written += chunk_frames;
            }
            writer.finalize()
                .map_err(|e| format!("Measured finalize failed: {}", e))?;
            result.add_timing(start.elapsed().as_secs_f64() * 1000.0);
        }

        std::fs::remove_file(fp).map_err(|io| io.to_string())?;
        Ok(result)
    }

    match bench_cfg.signal_dtype.as_str() {
        "i8"  => run_for_type::<u8>(&fp, bench_cfg),
        "i16" => run_for_type::<i16>(&fp, bench_cfg),
        "i32" => run_for_type::<i32>(&fp, bench_cfg),
        "f32" => run_for_type::<f32>(&fp, bench_cfg),
        _ => Err(format!("Unsupported signal dtype: {}", bench_cfg.signal_dtype)),
    }
}

pub struct BenchPair {
    pub dtype: String,
    pub channels: u32,
    pub hound: Option<BenchResult>,
    pub aus: Option<BenchResult>,
    /// Memory-mapped read baseline; populated only for BenchKind::Read.
    pub mmap: Option<BenchResult>,
}

pub struct BenchGroup {
    pub kind: BenchKind,
    pub pairs: Vec<BenchPair>,
}

pub struct BenchSuite {
    pub groups: Vec<BenchGroup>,
    pub duration_secs: u64,
    pub iterations: usize,
    pub warmup: usize,
    pub chunk_size: usize,
    pub cold_cache: bool,
}

pub fn run_suite(
    dtypes: &[String],
    channels: &[u32],
    kinds: &[BenchKind],
    iterations: usize,
    warmup: usize,
    duration_secs: u64,
    chunk_size: usize,
    cold_cache: bool,
) -> BenchSuite {
    // mmap runs only for the Read kind.
    let has_read = kinds.contains(&BenchKind::Read);
    let mmap_count = if has_read { dtypes.len() * channels.len() } else { 0 };
    let total = kinds.len() * dtypes.len() * channels.len() * 2 + mmap_count;
    let mut current = 0usize;
    let mut groups = Vec::new();

    for &kind in kinds {
        let mut pairs = Vec::new();

        for dtype in dtypes {
            for &ch in channels {
                let cfg = BenchConfig {
                    iterations,
                    warmup_iterations: warmup,
                    signal_duration: Duration::from_secs(duration_secs),
                    signal_dtype: dtype.clone(),
                    chunk_size,
                    channels: ch,
                    cold_cache,
                };

                current += 1;
                print!("[{current:>3}/{total}] hound {} {dtype:<3} {ch}ch ... ", kind.short());
                let _ = std::io::stdout().flush();
                let hound_result = match kind {
                    BenchKind::Read         => bench_hound_read(&cfg),
                    BenchKind::Write        => bench_hound_write(&cfg),
                    BenchKind::StreamedRead  => bench_hound_streamed_read(&cfg),
                    BenchKind::StreamedWrite => bench_hound_streamed_write(&cfg),
                };
                match &hound_result {
                    Ok(r)  => println!("done  ({:.3} ms avg)", r.average()),
                    Err(e) => println!("FAILED: {e}"),
                }

                current += 1;
                print!("[{current:>3}/{total}] aus   {} {dtype:<3} {ch}ch ... ", kind.short());
                let _ = std::io::stdout().flush();
                let aus_result = match kind {
                    BenchKind::Read         => bench_aus_read(&cfg),
                    BenchKind::Write        => bench_aus_write(&cfg),
                    BenchKind::StreamedRead  => bench_aus_streamed_read(&cfg),
                    BenchKind::StreamedWrite => bench_aus_streamed_write(&cfg),
                };
                match &aus_result {
                    Ok(r)  => println!("done  ({:.3} ms avg)", r.average()),
                    Err(e) => println!("FAILED: {e}"),
                }

                let mmap_result = if kind == BenchKind::Read {
                    current += 1;
                    print!("[{current:>3}/{total}] mmap  {} {dtype:<3} {ch}ch ... ", kind.short());
                    let _ = std::io::stdout().flush();
                    let r = bench_mmap_read(&cfg);
                    match &r {
                        Ok(res) => println!("done  ({:.3} ms avg)", res.average()),
                        Err(e)  => println!("FAILED: {e}"),
                    }
                    r.ok()
                } else {
                    None
                };

                pairs.push(BenchPair {
                    dtype: dtype.clone(),
                    channels: ch,
                    hound: hound_result.ok(),
                    aus: aus_result.ok(),
                    mmap: mmap_result,
                });
            }
        }

        groups.push(BenchGroup { kind, pairs });
    }

    BenchSuite { groups, duration_secs, iterations, warmup, chunk_size, cold_cache }
}

const TABLE_HEADERS: &[&str] = &["Benchmark", "avg (ms)", "σ (ms)", "p50 (ms)", "p90 (ms)", "p99 (ms)", "CV"];

fn render_table(title: &str, rows: &[Vec<String>], separators: &[usize]) -> String {
    let ncols = TABLE_HEADERS.len();

    let mut widths: Vec<usize> = TABLE_HEADERS.iter().map(|h| h.len() + 2).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(ncols) {
            widths[i] = widths[i].max(cell.len() + 2);
        }
    }

    let content_w: usize = widths.iter().sum::<usize>() + ncols - 1;

    let col_top: String = widths.iter().map(|w| "─".repeat(*w)).collect::<Vec<_>>().join("┬");
    let col_mid: String = widths.iter().map(|w| "─".repeat(*w)).collect::<Vec<_>>().join("┼");
    let col_bot: String = widths.iter().map(|w| "─".repeat(*w)).collect::<Vec<_>>().join("┴");

    let mut s = String::new();

    let title_inner = format!(" {title} ");
    let pad = content_w.saturating_sub(title_inner.len());
    s += &format!("┌{:─<w$}┐\n", "", w = content_w);
    s += &format!("│{:lp$}{title_inner}{:rp$}│\n", "", "", lp = pad / 2, rp = pad - pad / 2);

    s += &format!("├{col_top}┤\n");
    let header_row: String = TABLE_HEADERS.iter().zip(widths.iter())
        .map(|(h, w)| format!("{:^width$}", h, width = w))
        .collect::<Vec<_>>()
        .join("│");
    s += &format!("│{header_row}│\n");
    s += &format!("├{col_mid}┤\n");

    for (i, row) in rows.iter().enumerate() {
        let cells: String = row.iter().enumerate().zip(widths.iter())
            .map(|((col, cell), w)| {
                if col == 0 {
                    format!(" {:<width$} ", cell, width = w - 2)
                } else {
                    format!(" {:>width$} ", cell, width = w - 2)
                }
            })
            .collect::<Vec<_>>()
            .join("│");
        s += &format!("│{cells}│\n");

        if separators.contains(&i) && i + 1 < rows.len() {
            s += &format!("├{col_mid}┤\n");
        }
    }

    s += &format!("└{col_bot}┘\n");
    s
}

fn group_to_table_rows(group: &BenchGroup) -> (Vec<Vec<String>>, Vec<usize>) {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut separators: Vec<usize> = Vec::new();

    for pair in &group.pairs {
        let start = rows.len();

        for result in pair.hound.iter().chain(pair.aus.iter()).chain(pair.mmap.iter()) {
            // ⚠ marks high-variability results (CV > 0.5: σ exceeds half the mean).
            let name = if result.cv() > 0.5 {
                format!("{} ⚠", result.name)
            } else {
                result.name.clone()
            };
            rows.push(vec![
                name,
                format!("{:.3}", result.average()),
                format!("{:.3}", result.std_dev()),
                format!("{:.3}", result.p50()),
                format!("{:.3}", result.p90()),
                format!("{:.3}", result.p99()),
                format!("{:.3}", result.cv()),
            ]);
        }

        if rows.len() > start {
            separators.push(rows.len() - 1);
        }
    }

    separators.pop();
    (rows, separators)
}

impl BenchSuite {
    pub fn print_terminal(&self) {
        println!();
        let cache_label = if self.cold_cache { " · cold-cache" } else { "" };
        for group in &self.groups {
            let title = if matches!(group.kind, BenchKind::StreamedRead | BenchKind::StreamedWrite) {
                format!(
                    "{} — {}s · {} iters · {} warmup · chunk {}{}",
                    group.kind.label(), self.duration_secs, self.iterations,
                    self.warmup, self.chunk_size, cache_label,
                )
            } else {
                format!(
                    "{} — {}s signal · {} iterations · {} warmup{}",
                    group.kind.label(), self.duration_secs, self.iterations,
                    self.warmup, cache_label,
                )
            };

            let (rows, seps) = group_to_table_rows(group);
            if rows.is_empty() {
                println!("[ {} — no results ]\n", group.kind.label());
            } else {
                print!("{}", render_table(&title, &rows, &seps));
                println!();
            }
        }
        println!("CV > 0.5 (high variance — treat result as indicative)");
    }

    pub fn save_csv(&self, base: &str) -> std::io::Result<()> {
        let path = PathBuf::from(format!("{base}.csv"));
        let mut f = std::fs::File::create(&path)?;

        writeln!(f, "library,operation,dtype,channels,duration_s,iterations,warmup,chunk_size,cold_cache,avg_ms,stddev_ms,p50_ms,p90_ms,p99_ms,cv")?;

        let cold = if self.cold_cache { 1 } else { 0 };

        for group in &self.groups {
            let op = group.kind.slug();
            for pair in &group.pairs {
                let common = format!(
                    "{},{},{},{},{},{},{},{}",
                    op, pair.dtype, pair.channels,
                    self.duration_secs, self.iterations, self.warmup, self.chunk_size, cold,
                );
                for (lib, result) in [("hound", &pair.hound), ("aus", &pair.aus), ("mmap", &pair.mmap)] {
                    if let Some(r) = result {
                        writeln!(f, "{lib},{common},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}",
                            r.average(), r.std_dev(), r.p50(), r.p90(), r.p99(), r.cv())?;
                    }
                }
            }
        }

        println!("CSV  → {}", path.display());
        Ok(())
    }

    pub fn save_markdown(&self, base: &str) -> std::io::Result<()> {
        let path = PathBuf::from(format!("{base}.md"));
        let mut f = std::fs::File::create(&path)?;

        let cache_note = if self.cold_cache { " · **cold-cache**" } else { "" };

        writeln!(f, "# WAV Benchmark Results\n")?;
        writeln!(f,
            "Signal: {}s · Sample rate: 44 100 Hz · Iterations: {} · Warmup: {} · Chunk: {}{}\n",
            self.duration_secs, self.iterations, self.warmup, self.chunk_size, cache_note,
        )?;

        for group in &self.groups {
            writeln!(f, "## {}\n", group.kind.label())?;
            writeln!(f, "| Benchmark | avg (ms) | σ (ms) | p50 (ms) | p90 (ms) | p99 (ms) | CV |")?;
            writeln!(f, "|:----------|--------:|-------:|--------:|--------:|--------:|------:|")?;

            let mut any = false;
            for pair in &group.pairs {
                for (_, result) in [("hound", &pair.hound), ("aus", &pair.aus), ("mmap", &pair.mmap)] {
                    if let Some(r) = result {
                        let name = if r.cv() > 0.5 {
                            format!("{} ⚠", r.name)
                        } else {
                            r.name.clone()
                        };
                        writeln!(f, "| {} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} |",
                            name,
                            r.average(), r.std_dev(),
                            r.p50(), r.p90(), r.p99(), r.cv(),
                        )?;
                        any = true;
                    }
                }
                if pair.hound.is_some() || pair.aus.is_some() || pair.mmap.is_some() {
                    writeln!(f, "| &nbsp; | | | | | | |")?;
                }
            }

            if !any {
                writeln!(f, "| *no results* | | | | | | |")?;
            }

            writeln!(f)?;
        }

        writeln!(f, "> ⚠ = CV > 0.5 (high variance — treat result as indicative)")?;

        println!("MD   → {}", path.display());
        Ok(())
    }
}

struct Args {
    dtypes: Vec<String>,
    channels: Vec<u32>,
    duration_secs: u64,
    iterations: usize,
    warmup: usize,
    chunk_size: usize,
    benches: Vec<BenchKind>,
    output: String,
    cold_cache: bool,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut raw = std::env::args().skip(1);
        let mut dtypes = ["i16", "i32", "f32"].iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let mut channels = vec![1u32, 2u32];
        let mut duration_secs = 10u64;
        let mut iterations = 1000usize;
        let mut warmup = 100usize;
        let mut chunk_size = 4096usize;
        let mut benches = vec![
            BenchKind::Read, BenchKind::Write,
            BenchKind::StreamedRead, BenchKind::StreamedWrite,
        ];
        let mut output = "results".to_string();
        let mut cold_cache = false;

        while let Some(arg) = raw.next() {
            match arg.as_str() {
                "--help" | "-h" => { print_help(); std::process::exit(0); }
                "--dtype" => {
                    let v = raw.next().ok_or("--dtype requires a value")?;
                    dtypes = v.split(',').map(|s| s.trim().to_string()).collect();
                }
                "--channels" => {
                    let v = raw.next().ok_or("--channels requires a value")?;
                    channels = v.split(',').map(|s| {
                        s.trim().parse::<u32>().map_err(|_| format!("invalid channel count: {}", s))
                    }).collect::<Result<Vec<_>, _>>()?;
                }
                "--duration" => {
                    let v = raw.next().ok_or("--duration requires a value")?;
                    duration_secs = v.parse().map_err(|_| format!("invalid duration: {v}"))?;
                }
                "--iterations" => {
                    let v = raw.next().ok_or("--iterations requires a value")?;
                    iterations = v.parse().map_err(|_| format!("invalid iterations: {v}"))?;
                }
                "--warmup" => {
                    let v = raw.next().ok_or("--warmup requires a value")?;
                    warmup = v.parse().map_err(|_| format!("invalid warmup: {v}"))?;
                }
                "--chunk-size" => {
                    let v = raw.next().ok_or("--chunk-size requires a value")?;
                    chunk_size = v.parse().map_err(|_| format!("invalid chunk-size: {v}"))?;
                }
                "--bench" => {
                    let v = raw.next().ok_or("--bench requires a value")?;
                    benches = v.split(',').map(|s| match s.trim() {
                        "read"           => Ok(BenchKind::Read),
                        "write"          => Ok(BenchKind::Write),
                        "streamed-read"  => Ok(BenchKind::StreamedRead),
                        "streamed-write" => Ok(BenchKind::StreamedWrite),
                        other => Err(format!(
                            "unknown bench: {other} (valid: read, write, streamed-read, streamed-write)"
                        )),
                    }).collect::<Result<Vec<_>, _>>()?;
                }
                "--output" => {
                    output = raw.next().ok_or("--output requires a value")?;
                }
                "--cold-cache" => { cold_cache = true; }
                other => return Err(format!("unknown argument: {other}")),
            }
        }

        Ok(Args { dtypes, channels, duration_secs, iterations, warmup, chunk_size, benches, output, cold_cache })
    }
}

fn print_help() {
    println!(
        "aus_vs_hound — WAV read/write benchmark

USAGE:
    aus_vs_hound [OPTIONS]

OPTIONS:
    --dtype        <LIST>   Comma-separated types: i16,i32,f32 (default: i16,i32,f32)
    --channels     <LIST>   Comma-separated channel counts (default: 1,2)
    --duration     <SECS>   Signal duration in seconds (default: 10)
    --iterations   <N>      Measured iterations per benchmark (default: 1000)
    --warmup       <N>      Warmup iterations before measurement (default: 100)
    --chunk-size   <N>      Samples per chunk for streamed benchmarks (default: 4096)
    --bench        <LIST>   read,write,streamed-read,streamed-write (default: all four)
    --output       <PATH>   Base name for output files, no extension (default: results)
    --cold-cache            Drop page cache before each read iteration (Linux only, via
                            posix_fadvise DONTNEED). Measures storage-bound performance.
                            Warmup iterations always run warm to stabilise code paths.
    --help, -h              Show this help

OUTPUTS:
    <output>.csv    Raw timings — one row per benchmark result
    <output>.md     Markdown tables ready to paste into the article

NOTES:
    * The Read benchmark also runs a memory-mapped (mmap) baseline that maps the
      file and reads every sample byte directly, establishing an upper bound on
      read throughput. Results labelled ⚠ have CV > 0.5 (high variance).

EXAMPLES:
    # Full warm-cache run:
    aus_vs_hound

    # Cold-cache read-only sweep, stereo only:
    aus_vs_hound --bench read --cold-cache --channels 2 --output cold_stereo

    # Quick smoke-test:
    aus_vs_hound --duration 1 --iterations 50 --warmup 5"
    );
}

fn main() {
    let args = match Args::parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\nRun with --help for usage.");
            std::process::exit(1);
        }
    };

    let cache_label = if args.cold_cache { " · cold-cache" } else { "" };
    println!(
        "Running: {} | dtypes: {} | channels: {} | signal: {}s | {} iters / {} warmup | chunk: {}{}",
        args.benches.iter().map(|k| k.label()).collect::<Vec<_>>().join(", "),
        args.dtypes.join(", "),
        args.channels.iter().map(|c| format!("{c}ch")).collect::<Vec<_>>().join(", "),
        args.duration_secs,
        args.iterations,
        args.warmup,
        args.chunk_size,
        cache_label,
    );
    println!();

    let suite = run_suite(
        &args.dtypes,
        &args.channels,
        &args.benches,
        args.iterations,
        args.warmup,
        args.duration_secs,
        args.chunk_size,
        args.cold_cache,
    );

    suite.print_terminal();

    println!("Saving results...");
    if let Err(e) = suite.save_csv(&args.output) {
        eprintln!("Failed to save CSV: {e}");
    }
    if let Err(e) = suite.save_markdown(&args.output) {
        eprintln!("Failed to save markdown: {e}");
    }
}
