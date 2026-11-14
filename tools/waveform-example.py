# SPDX-License-Identifier: MIT
"""Worked example of docs/waveform.md, computed independently of the Rust code (struct, hashlib
and a bitwise CRC-32C): prints the canonical file in hex and its waveform id. tests/waveform.rs
holds the same bytes and id."""
import hashlib
import struct


def crc32c(data):
    c = 0xFFFFFFFF
    for b in data:
        c ^= b
        for _ in range(8):
            c = (c >> 1) ^ 0x82F63B78 if c & 1 else c >> 1
    return c ^ 0xFFFFFFFF


def sample_id(rate, pcm):
    return hashlib.sha256(b"oscairn-sample-1\0" + struct.pack("<IBB", rate, 1, 16) + struct.pack(f"<{len(pcm)}h", *pcm)).digest()


A = sample_id(44100, [0, 1, -1, 32767])
B = sample_id(48000, [0, 1, -1, 32767])
# key_lo key_hi vel_lo vel_hi root transpose loop sample-id tune level pan start end loop_start loop_end
given = [
    (60, 127, 1, 127, 72, 0, 1, B, -1250, -600, 0, 0, 0, 100, 2000),
    (0, 59, 1, 127, 48, 0, 0, A, 0, 0, -10000, 0, 0, 0, 0),
    (0, 59, 1, 127, 48, 0, 0, B, 0, 0, 10000, 0, 0, 0, 0),
]


def bank(f, index):
    return struct.pack("<5BbBBHHiihHIIII", *f[:7], 0, index, 0, *f[8:11], 0, *f[11:])


def key(f):  # the bank's bytes with the sample index replaced by the sample id
    b = bank(f, 0)
    return b[:8] + f[7] + b[10:]


ordered = sorted(given, key=key)
ids = []
for f in ordered:
    if f[7] not in ids:
        ids.append(f[7])
body = struct.pack("<HH", len(ids), len(ordered)) + b"".join(ids) + b"".join(bank(f, ids.index(f[7])) for f in ordered)
file = b"OPWF" + struct.pack("<HHII", 1, 0, 16 + len(body), crc32c(body)) + body
print("sample ids:", A.hex(), B.hex())
for at in range(0, len(file), 16):
    print(f"{at:04x}: " + " ".join(f"{b:02x}" for b in file[at:at + 16]))
print("waveform id:", hashlib.sha256(b"oscairn-waveform-1\0" + body).hexdigest())
