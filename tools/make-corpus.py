#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Generate the synthetic test corpus: tests/data/*.flac (stock `flac` encoder) and the matching
tests/data/*.pcm (what `flac -d` decodes them to: interleaved signed 16-bit little endian).

Usage: tools/make-corpus.py [outdir]     (needs python3 and flac on PATH; no audio is read)
"""
import math, os, random, struct, subprocess, sys

OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "tests", "data")
N = 5000


def clip(v):
    return max(-32768, min(32767, int(round(v))))


def sweep(n, ch):
    f0, f1, out = 50.0, 15000.0, []
    ph = [0.0, 0.7]
    for i in range(n):
        f = f0 * (f1 / f0) ** (i / n)
        row = []
        for c in range(ch):
            ph[c] += 2 * math.pi * f / 44100.0
            row.append(clip(19000 * math.sin(ph[c]) * (1.0 if c == 0 else 0.8)))
        out.append(row)
    return out


def noise(n, ch, amp=29000, seed=1):
    r = random.Random(seed)
    return [[r.randint(-amp, amp) for _ in range(ch)] for _ in range(n)]


def colored(n, ch, seed=2):
    r = random.Random(seed)
    st = [0.0] * ch
    out = []
    for i in range(n):
        row = []
        for c in range(ch):
            st[c] = 0.97 * st[c] + r.gauss(0, 900)
            row.append(clip(st[c]))
        out.append(row)
    return out


def silence(n, ch):
    return [[0] * ch for _ in range(n)]


def dc(n, ch):
    return [[1234 - 77 * c] * ch for c in range(1)] * 0 or [[1234 - 77 * c for c in range(ch)] for _ in range(n)]


def impulses(n, ch):
    out = silence(n, ch)
    vals = [32767, -32768, 1, -1, 20000, -32768, 32767]
    for k, v in enumerate(vals):
        for c in range(ch):
            out[(k * 697 + 13 + c * 5) % n][c] = v if (k + c) % 2 == 0 else -v if v != -32768 else 32767
    return out


def clipped(n, ch):
    return [[clip(48000 * math.sin(2 * math.pi * 330 * i / 44100 + c)) for c in range(ch)] for i in range(n)]


def shifted(n, ch):
    return [[clip(8000 * math.sin(2 * math.pi * 440 * i / 44100)) // 8 * 8 for _ in range(ch)] for i in range(n)]


def correlated(n, ch):
    r = random.Random(3)
    out = []
    for i in range(n):
        s = 15000 * math.sin(2 * math.pi * 220 * i / 44100) + 4000 * math.sin(2 * math.pi * 1900 * i / 44100)
        out.append([clip(s), clip(0.5 * s + r.gauss(0, 30))][:ch])
    return out


def antiphase(n, ch):
    out = []
    for i in range(n):
        s = clip(16000 * math.sin(2 * math.pi * 500 * i / 44100))
        out.append([s, -s][:ch] if s != -32768 else [s, 32767][:ch])
    return out


SIGNALS = dict(sweep=sweep, noise=noise, colored=colored, silence=silence, dc=dc, impulses=impulses,
               clipped=clipped, shifted=shifted, correlated=correlated, antiphase=antiphase)

# (signal, channels, blocksize, -l order, extra flac options, sample rate)
CASES = []
for sig in SIGNALS:
    for ch in (1, 2):
        if ch == 1 and sig in ("correlated", "antiphase"):
            continue
        CASES.append((sig, ch, 1024, 4, [], 44100))
for sig in ("sweep", "noise", "colored", "correlated", "antiphase", "clipped"):
    for b in (256, 4096):
        for l in (0, 8):
            CASES.append((sig, 2, b, l, [], 48000))
    CASES.append((sig, 2, 1024, 8, ["--no-mid-side"], 44100))
    CASES.append((sig, 2, 1024, 8, ["-m", "-e"], 96000))
    CASES.append((sig, 2, 1000, 4, ["-M", "-e"], 22050))
    CASES.append((sig, 2, 192, 4, [], 8000))
    CASES.append((sig, 2, 576, 4, [], 37800))
    CASES.append((sig, 2, 1152, 6, ["-A", "tukey(0.5)"], 12345))
CASES.append(("sweep", 1, 4096, 8, [], 192000))
CASES.append(("sweep", 1, 100, 3, [], 44100))
CASES.append(("sweep", 2, 1024, 12, [], 44100))   # LPC orders above 8 appear
CASES.append(("sweep", 1, 1024, 9, [], 44100))
CASES.append(("sweep", 1, 4096, 10, [], 44100))
CASES.append(("sweep", 2, 4096, 11, [], 44100))
CASES.append(("sweep", 1, 4096, 12, ["-q", "8"], 44100))   # 32-bit prediction sums
CASES.append(("sweep", 1, 4096, 32, [], 44100))
CASES.append(("sweep", 2, 1024, 32, [], 44100))
CASES.append(("noise", 2, 4096, 4, ["-r", "0"], 44100))
CASES.append(("colored", 2, 4096, 4, ["-r", "15"], 44100))


def name(sig, ch, b, l, extra, rate):
    x = "".join(c for c in "".join(extra) if c.isalnum())
    return f"{sig}-{'mono' if ch == 1 else 'st'}-b{b}-l{l}{('-' + x) if x else ''}-r{rate}"


def main():
    os.makedirs(OUT, exist_ok=True)
    names = []
    for sig, ch, b, l, extra, rate in CASES:
        nm = name(sig, ch, b, l, extra, rate)
        rows = SIGNALS[sig](N, ch)
        raw = b"".join(struct.pack("<h", v) for row in rows for v in row)
        src = os.path.join(OUT, nm + ".src")
        with open(src, "wb") as f:
            f.write(raw)
        flac = os.path.join(OUT, nm + ".flac")
        subprocess.run(["flac", "-s", "-f", "--lax", "--no-padding", "--no-seektable", "-b", str(b), "-l", str(l), *extra,
                        "--force-raw-format", "--endian=little", "--sign=signed", "--bps=16", f"--channels={ch}",
                        f"--sample-rate={rate}", "-o", flac, src], check=True)
        subprocess.run(["flac", "-s", "-d", "-f", "--force-raw-format", "--endian=little", "--sign=signed", "-o",
                        os.path.join(OUT, nm + ".pcm"), flac], check=True)
        if open(os.path.join(OUT, nm + ".pcm"), "rb").read() != raw:
            sys.exit("flac -d does not reproduce the source for " + nm)
        os.remove(src)
        names.append(nm)
    with open(os.path.join(OUT, "manifest.txt"), "w") as f:
        f.write("\n".join(names) + "\n")
    print(len(names), "cases")


main()
