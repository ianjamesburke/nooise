#!/usr/bin/env python3
"""Print sample peak, RMS, crest, clip count, and (via ffmpeg) LUFS / true peak.

Usage:
    python3 scripts/measure_wav.py path/to/render.wav
"""

import math
import shutil
import struct
import subprocess
import sys
import wave


def db(linear: float) -> float:
    if linear <= 0.0:
        return float("-inf")
    return 20.0 * math.log10(linear)


def pcm_stats(path: str) -> None:
    with wave.open(path) as handle:
        channels = handle.getnchannels()
        rate = handle.getframerate()
        width = handle.getsampwidth()
        frames = handle.getnframes()
        if width != 2:
            raise SystemExit(f"{path}: expected 16-bit PCM, got {width * 8}-bit")
        raw = handle.readframes(frames)
    samples = struct.unpack("<" + "h" * (frames * channels), raw)
    peak = 0
    sum_sq = 0.0
    clips = 0
    full_scale = 32768.0
    for sample in samples:
        if sample in (-32768, 32767):
            clips += 1
        peak = max(peak, abs(sample))
        sum_sq += float(sample * sample)
    rms = math.sqrt(sum_sq / len(samples))
    seconds = frames / rate
    print(f"file {path}")
    print(f"format {rate} Hz {channels} ch pcm_s16 {seconds:.3f} s {frames} frames")
    print(f"sample_peak {db(peak / full_scale):.2f} dBFS")
    print(f"rms {db(rms / full_scale):.2f} dBFS")
    print(f"crest {db(peak / full_scale) - db(rms / full_scale):.2f} dB")
    print(f"full_scale_samples {clips}")


def ffmpeg_loudness(path: str) -> None:
    if shutil.which("ffmpeg") is None:
        print("ffmpeg not available; LUFS and true peak skipped")
        return
    result = subprocess.run(
        [
            "ffmpeg",
            "-hide_banner",
            "-i",
            path,
            "-af",
            "ebur128=peak=true",
            "-f",
            "null",
            "-",
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    text = result.stderr
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("I:") or stripped.startswith("LRA:") or stripped.startswith("Peak:"):
            print(stripped)


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: python3 scripts/measure_wav.py FILE.wav")
    pcm_stats(sys.argv[1])
    ffmpeg_loudness(sys.argv[1])


if __name__ == "__main__":
    main()
