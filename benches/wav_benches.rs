use std::{
    num::{NonZeroU32, NonZeroUsize},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use audio_samples::{AudioSamples, StandardSample, sample_rate};
use audio_samples_io::{
    create_streamed, open_streamed,
    traits::{AudioStreamWrite, AudioStreamWriter},
};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

const DURATIONS_S: &[u64] = &[1, 5, 10, 30, 60, 300, 600];
const CHUNK_SIZES: &[usize] = &[512, 1024, 4096, 8192, 16384];
const CHANNELS: &[u32] = &[1, 2];
const DTYPES: &[&str] = &["i16", "i32", "f32"];

fn wav_path(dur: u64, dtype: &str, ch: u32) -> PathBuf {
    PathBuf::from(format!("resources/sine_{dur}s_{dtype}_{ch}ch.wav"))
}

// ── Hound helpers ─────────────────────────────────────────────────────────────

fn hound_read_i16(path: &Path) {
    let mut reader = hound::WavReader::open(path).unwrap();
    let s: Vec<i16> = reader.samples().collect::<Result<_, _>>().unwrap();
    black_box(s);
}

fn hound_read_i32(path: &Path) {
    let mut reader = hound::WavReader::open(path).unwrap();
    let s: Vec<i32> = reader.samples().collect::<Result<_, _>>().unwrap();
    black_box(s);
}

fn hound_read_f32(path: &Path) {
    let mut reader = hound::WavReader::open(path).unwrap();
    let s: Vec<f32> = reader.samples().collect::<Result<_, _>>().unwrap();
    black_box(s);
}

fn hound_write_i16(path: &Path, samples: &[i16]) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let mut iw = w.get_i16_writer(samples.len() as u32);
    for &s in samples {
        iw.write_sample(s);
    }
    iw.flush().unwrap();
    w.finalize().unwrap();
}

fn hound_write_i32(path: &Path, samples: &[i32]) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for &s in samples {
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
}

fn hound_write_f32(path: &Path, samples: &[f32]) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for &s in samples {
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
}

fn hound_streamed_read_i16(path: &Path, chunk_size: usize) {
    let mut reader = hound::WavReader::open(path).unwrap();
    let mut iter = reader.samples::<i16>();
    let mut buf: Vec<i16> = Vec::with_capacity(chunk_size);
    loop {
        buf.clear();
        for s in iter.by_ref().take(chunk_size) {
            buf.push(s.unwrap());
        }
        if buf.is_empty() {
            break;
        }
        black_box(&buf);
    }
}

fn hound_streamed_read_i32(path: &Path, chunk_size: usize) {
    let mut reader = hound::WavReader::open(path).unwrap();
    let mut iter = reader.samples::<i32>();
    let mut buf: Vec<i32> = Vec::with_capacity(chunk_size);
    loop {
        buf.clear();
        for s in iter.by_ref().take(chunk_size) {
            buf.push(s.unwrap());
        }
        if buf.is_empty() {
            break;
        }
        black_box(&buf);
    }
}

fn hound_streamed_read_f32(path: &Path, chunk_size: usize) {
    let mut reader = hound::WavReader::open(path).unwrap();
    let mut iter = reader.samples::<f32>();
    let mut buf: Vec<f32> = Vec::with_capacity(chunk_size);
    loop {
        buf.clear();
        for s in iter.by_ref().take(chunk_size) {
            buf.push(s.unwrap());
        }
        if buf.is_empty() {
            break;
        }
        black_box(&buf);
    }
}

fn hound_streamed_write_i16(path: &Path, chunk: &[i16], total_frames: usize) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let mut written = 0;
    while written < total_frames {
        let end = (written + chunk.len()).min(total_frames);
        let mut iw = w.get_i16_writer((end - written) as u32);
        for &s in &chunk[..end - written] {
            iw.write_sample(s);
        }
        iw.flush().unwrap();
        written = end;
    }
    w.finalize().unwrap();
}

fn hound_streamed_write_i32(path: &Path, chunk: &[i32], total_frames: usize) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let mut written = 0;
    while written < total_frames {
        let end = (written + chunk.len()).min(total_frames);
        for &s in &chunk[..end - written] {
            w.write_sample(s).unwrap();
        }
        written = end;
    }
    w.finalize().unwrap();
}

fn hound_streamed_write_f32(path: &Path, chunk: &[f32], total_frames: usize) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let mut written = 0;
    while written < total_frames {
        let end = (written + chunk.len()).min(total_frames);
        for &s in &chunk[..end - written] {
            w.write_sample(s).unwrap();
        }
        written = end;
    }
    w.finalize().unwrap();
}

// ── audio_samples_io helpers ───────────────────────────────────────────────────

fn aus_read<T: StandardSample>(path: &Path) {
    let s = audio_samples_io::read::<_, T>(path).unwrap();
    black_box(s);
}

fn aus_write<T: StandardSample>(path: &Path, signal: &AudioSamples<T>) {
    audio_samples_io::write::<_, T>(path, signal).unwrap();
}

fn aus_streamed_read<T: StandardSample + 'static>(path: &Path, chunk_size: usize) {
    let nz_chunk = NonZeroUsize::new(chunk_size).unwrap();
    let mut streamed = open_streamed(path).unwrap();
    let nz_sr = NonZeroU32::new(streamed.sample_rate()).unwrap();
    let channels = streamed.num_channels();
    let mut buffer = if channels == 1 {
        AudioSamples::<T>::zeros_mono(nz_chunk, nz_sr)
    } else {
        let nz_ch = NonZeroU32::new(channels as u32).unwrap();
        AudioSamples::<T>::zeros_multi(nz_ch, nz_chunk, nz_sr)
    };
    while streamed.remaining_frames() > 0 {
        streamed
            .read_frames_into(&mut buffer, nz_chunk)
            .unwrap();
        black_box(&buffer);
    }
}

fn aus_streamed_write<T: StandardSample>(path: &Path, chunk: &AudioSamples<T>, total_frames: usize) {
    let mut writer = create_streamed::<_, T>(path, 1, 44_100).unwrap();
    let chunk_frames = chunk.len().get();
    let mut written = 0;
    while written < total_frames {
        writer.write_frames(chunk).unwrap();
        written += chunk_frames;
    }
    writer.finalize().unwrap();
}

// ── Bulk read ─────────────────────────────────────────────────────────────────

fn bench_bulk_read(c: &mut Criterion) {
    let mut group = c.benchmark_group("bulk_read");

    for &ch in CHANNELS {
        for &dtype in DTYPES {
            for &dur in DURATIONS_S {
                let path = wav_path(dur, dtype, ch);
                if !path.exists() {
                    continue;
                }
                let fn_name = format!("hound_{dtype}_{ch}ch");
                let param = format!("{dur}s");
                group.bench_with_input(BenchmarkId::new(&fn_name, &param), &path, |b, p| {
                    b.iter_custom(|iters| {
                        let mut total = Duration::ZERO;
                        for _ in 0..iters {
                            let start = Instant::now();
                            match dtype {
                                "i16" => hound_read_i16(p),
                                "i32" => hound_read_i32(p),
                                "f32" => hound_read_f32(p),
                                _ => unreachable!(),
                            }
                            total += start.elapsed();
                        }
                        total
                    });
                });

                let fn_name = format!("aus_{dtype}_{ch}ch");
                group.bench_with_input(BenchmarkId::new(&fn_name, &param), &path, |b, p| {
                    b.iter_custom(|iters| {
                        let mut total = Duration::ZERO;
                        for _ in 0..iters {
                            let start = Instant::now();
                            match dtype {
                                "i16" => aus_read::<i16>(p),
                                "i32" => aus_read::<i32>(p),
                                "f32" => aus_read::<f32>(p),
                                _ => unreachable!(),
                            }
                            total += start.elapsed();
                        }
                        total
                    });
                });
            }
        }
    }
    group.finish();
}

// ── Bulk write ────────────────────────────────────────────────────────────────

fn bench_bulk_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("bulk_write");
    let sr = sample_rate!(44100);
    let out_path = PathBuf::from("target/criterion_bench_write_tmp.wav");

    for &ch in CHANNELS {
        for &dtype in DTYPES {
            for &dur in DURATIONS_S {
                let total_frames = 44_100 * dur as usize;
                let fn_name_h = format!("hound_{dtype}_{ch}ch");
                let fn_name_a = format!("aus_{dtype}_{ch}ch");
                let param = format!("{dur}s");
                let out = out_path.clone();

                match dtype {
                    "i16" => {
                        let signal: Vec<i16> = (0..total_frames)
                            .map(|n| {
                                let t = n as f32 / 44_100.0;
                                ((t * 440.0 * 2.0 * std::f32::consts::PI).sin()
                                    * i16::MAX as f32) as i16
                            })
                            .collect();
                        let aus_signal = audio_samples_io::read::<_, i16>(
                            &wav_path(dur.min(10), "i16", 1),
                        )
                        .unwrap_or_else(|_| {
                            AudioSamples::<i16>::zeros_mono(
                                NonZeroUsize::new(total_frames).unwrap(),
                                sr,
                            )
                        });
                        group.bench_function(BenchmarkId::new(&fn_name_h, &param), |b| {
                            b.iter_custom(|iters| {
                                let mut total = Duration::ZERO;
                                for _ in 0..iters {
                                    let start = Instant::now();
                                    hound_write_i16(&out, &signal);
                                    total += start.elapsed();
                                }
                                total
                            });
                        });
                        group.bench_function(BenchmarkId::new(&fn_name_a, &param), |b| {
                            b.iter_custom(|iters| {
                                let mut total = Duration::ZERO;
                                for _ in 0..iters {
                                    let start = Instant::now();
                                    aus_write::<i16>(&out, &aus_signal);
                                    total += start.elapsed();
                                }
                                total
                            });
                        });
                    }
                    "i32" => {
                        let signal: Vec<i32> = (0..total_frames)
                            .map(|n| {
                                let t = n as f32 / 44_100.0;
                                ((t * 440.0 * 2.0 * std::f32::consts::PI).sin()
                                    * i32::MAX as f32) as i32
                            })
                            .collect();
                        let aus_signal = audio_samples_io::read::<_, i32>(
                            &wav_path(dur.min(10), "i32", 1),
                        )
                        .unwrap_or_else(|_| {
                            AudioSamples::<i32>::zeros_mono(
                                NonZeroUsize::new(total_frames).unwrap(),
                                sr,
                            )
                        });
                        group.bench_function(BenchmarkId::new(&fn_name_h, &param), |b| {
                            b.iter_custom(|iters| {
                                let mut total = Duration::ZERO;
                                for _ in 0..iters {
                                    let start = Instant::now();
                                    hound_write_i32(&out, &signal);
                                    total += start.elapsed();
                                }
                                total
                            });
                        });
                        group.bench_function(BenchmarkId::new(&fn_name_a, &param), |b| {
                            b.iter_custom(|iters| {
                                let mut total = Duration::ZERO;
                                for _ in 0..iters {
                                    let start = Instant::now();
                                    aus_write::<i32>(&out, &aus_signal);
                                    total += start.elapsed();
                                }
                                total
                            });
                        });
                    }
                    "f32" => {
                        let signal: Vec<f32> = (0..total_frames)
                            .map(|n| {
                                let t = n as f32 / 44_100.0;
                                (t * 440.0 * 2.0 * std::f32::consts::PI).sin()
                            })
                            .collect();
                        let aus_signal = audio_samples_io::read::<_, f32>(
                            &wav_path(dur.min(10), "f32", 1),
                        )
                        .unwrap_or_else(|_| {
                            AudioSamples::<f32>::zeros_mono(
                                NonZeroUsize::new(total_frames).unwrap(),
                                sr,
                            )
                        });
                        group.bench_function(BenchmarkId::new(&fn_name_h, &param), |b| {
                            b.iter_custom(|iters| {
                                let mut total = Duration::ZERO;
                                for _ in 0..iters {
                                    let start = Instant::now();
                                    hound_write_f32(&out, &signal);
                                    total += start.elapsed();
                                }
                                total
                            });
                        });
                        group.bench_function(BenchmarkId::new(&fn_name_a, &param), |b| {
                            b.iter_custom(|iters| {
                                let mut total = Duration::ZERO;
                                for _ in 0..iters {
                                    let start = Instant::now();
                                    aus_write::<f32>(&out, &aus_signal);
                                    total += start.elapsed();
                                }
                                total
                            });
                        });
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
    group.finish();
}

// ── Streamed read ─────────────────────────────────────────────────────────────

fn bench_streamed_read(c: &mut Criterion) {
    let mut group = c.benchmark_group("streamed_read");

    for &ch in CHANNELS {
        for &dtype in DTYPES {
            for &dur in DURATIONS_S {
                let path = wav_path(dur, dtype, ch);
                if !path.exists() {
                    continue;
                }
                for &chunk in CHUNK_SIZES {
                    let fn_name_h = format!("hound_{dtype}_{ch}ch_{dur}s");
                    let fn_name_a = format!("aus_{dtype}_{ch}ch_{dur}s");
                    let param = format!("chunk{chunk}");

                    group.bench_with_input(
                        BenchmarkId::new(&fn_name_h, &param),
                        &(&path, chunk),
                        |b, &(p, cs)| {
                            b.iter_custom(|iters| {
                                let mut total = Duration::ZERO;
                                for _ in 0..iters {
                                    let start = Instant::now();
                                    match dtype {
                                        "i16" => hound_streamed_read_i16(p, cs),
                                        "i32" => hound_streamed_read_i32(p, cs),
                                        "f32" => hound_streamed_read_f32(p, cs),
                                        _ => unreachable!(),
                                    }
                                    total += start.elapsed();
                                }
                                total
                            });
                        },
                    );

                    group.bench_with_input(
                        BenchmarkId::new(&fn_name_a, &param),
                        &(&path, chunk),
                        |b, &(p, cs)| {
                            b.iter_custom(|iters| {
                                let mut total = Duration::ZERO;
                                for _ in 0..iters {
                                    let start = Instant::now();
                                    match dtype {
                                        "i16" => aus_streamed_read::<i16>(p, cs),
                                        "i32" => aus_streamed_read::<i32>(p, cs),
                                        "f32" => aus_streamed_read::<f32>(p, cs),
                                        _ => unreachable!(),
                                    }
                                    total += start.elapsed();
                                }
                                total
                            });
                        },
                    );
                }
            }
        }
    }
    group.finish();
}

// ── Streamed write ────────────────────────────────────────────────────────────

fn bench_streamed_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("streamed_write");
    let sr = sample_rate!(44100);
    let out_path = PathBuf::from("target/criterion_bench_streamed_write_tmp.wav");

    for &ch in &[1u32] {
        for &dtype in DTYPES {
            for &dur in DURATIONS_S {
                let total_frames = 44_100 * dur as usize;
                for &chunk in CHUNK_SIZES {
                    let fn_name_h = format!("hound_{dtype}_{ch}ch_{dur}s");
                    let fn_name_a = format!("aus_{dtype}_{ch}ch_{dur}s");
                    let param = format!("chunk{chunk}");
                    let out = out_path.clone();
                    let nz_chunk = NonZeroUsize::new(chunk).unwrap();

                    match dtype {
                        "i16" => {
                            let chunk_data: Vec<i16> = (0..chunk)
                                .map(|n| {
                                    let t = n as f32 / 44_100.0;
                                    ((t * 440.0 * 2.0 * std::f32::consts::PI).sin()
                                        * i16::MAX as f32) as i16
                                })
                                .collect();
                            let aus_chunk = AudioSamples::<i16>::zeros_mono(nz_chunk, sr);
                            group.bench_function(BenchmarkId::new(&fn_name_h, &param), |b| {
                                b.iter_custom(|iters| {
                                    let mut total = Duration::ZERO;
                                    for _ in 0..iters {
                                        let start = Instant::now();
                                        hound_streamed_write_i16(&out, &chunk_data, total_frames);
                                        total += start.elapsed();
                                    }
                                    total
                                });
                            });
                            group.bench_function(BenchmarkId::new(&fn_name_a, &param), |b| {
                                b.iter_custom(|iters| {
                                    let mut total = Duration::ZERO;
                                    for _ in 0..iters {
                                        let start = Instant::now();
                                        aus_streamed_write::<i16>(&out, &aus_chunk, total_frames);
                                        total += start.elapsed();
                                    }
                                    total
                                });
                            });
                        }
                        "i32" => {
                            let chunk_data: Vec<i32> = (0..chunk)
                                .map(|n| {
                                    let t = n as f32 / 44_100.0;
                                    ((t * 440.0 * 2.0 * std::f32::consts::PI).sin()
                                        * i32::MAX as f32) as i32
                                })
                                .collect();
                            let aus_chunk = AudioSamples::<i32>::zeros_mono(nz_chunk, sr);
                            group.bench_function(BenchmarkId::new(&fn_name_h, &param), |b| {
                                b.iter_custom(|iters| {
                                    let mut total = Duration::ZERO;
                                    for _ in 0..iters {
                                        let start = Instant::now();
                                        hound_streamed_write_i32(&out, &chunk_data, total_frames);
                                        total += start.elapsed();
                                    }
                                    total
                                });
                            });
                            group.bench_function(BenchmarkId::new(&fn_name_a, &param), |b| {
                                b.iter_custom(|iters| {
                                    let mut total = Duration::ZERO;
                                    for _ in 0..iters {
                                        let start = Instant::now();
                                        aus_streamed_write::<i32>(&out, &aus_chunk, total_frames);
                                        total += start.elapsed();
                                    }
                                    total
                                });
                            });
                        }
                        "f32" => {
                            let chunk_data: Vec<f32> = (0..chunk)
                                .map(|n| {
                                    let t = n as f32 / 44_100.0;
                                    (t * 440.0 * 2.0 * std::f32::consts::PI).sin()
                                })
                                .collect();
                            let aus_chunk = AudioSamples::<f32>::zeros_mono(nz_chunk, sr);
                            group.bench_function(BenchmarkId::new(&fn_name_h, &param), |b| {
                                b.iter_custom(|iters| {
                                    let mut total = Duration::ZERO;
                                    for _ in 0..iters {
                                        let start = Instant::now();
                                        hound_streamed_write_f32(&out, &chunk_data, total_frames);
                                        total += start.elapsed();
                                    }
                                    total
                                });
                            });
                            group.bench_function(BenchmarkId::new(&fn_name_a, &param), |b| {
                                b.iter_custom(|iters| {
                                    let mut total = Duration::ZERO;
                                    for _ in 0..iters {
                                        let start = Instant::now();
                                        aus_streamed_write::<f32>(&out, &aus_chunk, total_frames);
                                        total += start.elapsed();
                                    }
                                    total
                                });
                            });
                        }
                        _ => unreachable!(),
                    }
                }
            }
        }
    }
    group.finish();
}

criterion_group!(benches, bench_bulk_read, bench_bulk_write, bench_streamed_read, bench_streamed_write);
criterion_main!(benches);
