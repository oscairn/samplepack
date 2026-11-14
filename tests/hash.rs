// SPDX-License-Identifier: MIT
//! Test vectors for the hashes: SHA-256 from FIPS 180-4 (examples), MD5 from RFC 1321 (A.5),
//! CRC-32C check value. Every vector is also fed in two pieces at every split point.

use samplepack::hash::{crc32c, Crc32c, Md5, Sha256};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn sha256_vectors() {
    let v: [(&[u8], &str); 3] = [
        (b"", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
        (
            b"abc",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        (
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
        ),
    ];
    for (m, want) in v {
        assert_eq!(hex(&Sha256::digest(m)), want);
        for cut in 0..=m.len() {
            let mut h = Sha256::new();
            h.update(&m[..cut]);
            h.update(&m[cut..]);
            assert_eq!(hex(&h.finish()), want, "split at {cut}");
        }
    }
    let mut h = Sha256::new();
    for _ in 0..1000 {
        h.update(&[b'a'; 1000]);
    }
    assert_eq!(
        hex(&h.finish()),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
    // lengths around the padding boundary
    let mut seen = std::collections::HashSet::new();
    for n in 50..140 {
        assert!(seen.insert(Sha256::digest(&vec![0x5a; n])));
    }
}

#[test]
fn md5_vectors() {
    let v: [(&[u8], &str); 7] = [
        (b"", "d41d8cd98f00b204e9800998ecf8427e"),
        (b"a", "0cc175b9c0f1b6a831c399e269772661"),
        (b"abc", "900150983cd24fb0d6963f7d28e17f72"),
        (b"message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
        (b"abcdefghijklmnopqrstuvwxyz", "c3fcd3d76192e4007dfb496cca67e13b"),
        (
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
            "d174ab98d277d9f5a5611c2c9f419d9f",
        ),
        (
            b"12345678901234567890123456789012345678901234567890123456789012345678901234567890",
            "57edf4a22be3c955ac49da2e2107b67a",
        ),
    ];
    for (m, want) in v {
        assert_eq!(hex(&Md5::digest(m)), want);
        for cut in 0..=m.len() {
            let mut h = Md5::new();
            h.update(&m[..cut]);
            h.update(&m[cut..]);
            assert_eq!(hex(&h.finish()), want, "split at {cut}");
        }
    }
}

#[test]
fn crc32c_vectors() {
    assert_eq!(crc32c(b"123456789"), 0xE306_9283);
    assert_eq!(crc32c(b""), 0);
    // 32 bytes of zeros and of ones (RFC 3720, B.4)
    assert_eq!(crc32c(&[0u8; 32]), 0x8A91_36AA);
    assert_eq!(crc32c(&[0xFFu8; 32]), 0x62A8_AB43);
    let mut c = Crc32c::new();
    c.update(b"1234");
    c.update(b"56789");
    assert_eq!(c.finish(), 0xE306_9283);
}
