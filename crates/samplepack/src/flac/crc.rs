// SPDX-License-Identifier: MIT
//! CRC-8 (polynomial 0x07) and CRC-16 (polynomial 0x8005), both zero-initialised and
//! non-reflected, as used by FLAC frame headers and frames.

#[allow(clippy::indexing_slicing, clippy::arithmetic_side_effects)] // const evaluation: overflow or a bad index is a compile error
const fn table16() -> [u16; 256] {
    let mut t = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u16) << 8;
        let mut j = 0;
        while j < 8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 };
            j += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

#[allow(clippy::indexing_slicing, clippy::arithmetic_side_effects)] // const evaluation: overflow or a bad index is a compile error
const fn table8() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u8;
        let mut j = 0;
        while j < 8 {
            c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 };
            j += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static TAB16: [u16; 256] = table16();
static TAB8: [u8; 256] = table8();

/// CRC-8 of `data`.
pub fn crc8(data: &[u8]) -> u8 {
    let mut c = 0u8;
    for &b in data {
        c = TAB8.get(usize::from(c ^ b)).copied().unwrap_or(0);
    }
    c
}

/// CRC-16 of `data`.
pub fn crc16(data: &[u8]) -> u16 {
    let mut c = 0u16;
    for &b in data {
        let idx = usize::from((c >> 8) as u8 ^ b);
        c = (c << 8) ^ TAB16.get(idx).copied().unwrap_or(0);
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_values() {
        // CRC-8/SMBUS and CRC-16/UMTS (BUYPASS) check values for "123456789".
        assert_eq!(crc8(b"123456789"), 0xF4);
        assert_eq!(crc16(b"123456789"), 0xFEE8);
    }
}
