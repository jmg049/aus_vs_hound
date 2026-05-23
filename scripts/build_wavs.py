import audio_samples as aus
from audio_samples import SampleType
import os
from tqdm import tqdm

DURATIONS = [1, 5, 10, 30, 60, 300, 600]
DTYPES = [SampleType.I16, SampleType.F32, SampleType.I32]
CHANNELS = [1, 2]

if __name__ == "__main__":
    os.makedirs("./resources", exist_ok=True)
    for duration in tqdm(DURATIONS, desc="Durations", position=0):
        for dtype in tqdm(DTYPES, desc="Data Types", position=1, leave=False):
            for ch in CHANNELS:
                tqdm.write(f"Generating {duration}s of {dtype} audio ({ch} channel(s))...")
                samples = aus.sine_wave(440, duration, 44100, 1.0)
                if ch > 1:
                    samples = samples.duplicate_to_channels(ch)
                samples = samples.to_format(dtype)
                dtype_str = str(dtype).split(".")[-1].lower()
                filename = f"./resources/sine_{duration}s_{dtype_str}_{ch}ch.wav"
                aus.io.save(filename, samples)
