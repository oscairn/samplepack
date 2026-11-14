// SPDX-License-Identifier: MIT
//! Builds the static library, compiles `abi_check.c` against `include/samplepack.h` and the
//! library, and runs it. Struct sizes, offsets and enum values in the header are compared with
//! the Rust definitions; the C file also calls the interface.

use samplepack::flac::{Error, FrameEntry, FrameInfo, Scratch, FLAG_CHECK_CRC, MAX_BLOCK, MAX_LPC_ORDER};
use samplepack::pack::{self, IndexEntry, RecordInfo, VerifyScratch};
use std::mem::{align_of, offset_of, size_of};
use std::path::PathBuf;
use std::process::Command;

fn run(cmd: &mut Command) {
    let out = cmd.output().unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
    assert!(
        out.status.success(),
        "{cmd:?}\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

// The C side is built with a Unix-style compiler driver; on Windows this check is not run.
#[cfg_attr(windows, ignore)]
#[test]
fn header_matches_library() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let work = root.join("target").join("c-abi-test");
    std::fs::create_dir_all(&work).unwrap();
    run(Command::new(std::env::var("CARGO").unwrap())
        .args([
            "build",
            "--release",
            "--locked",
            "-p",
            "samplepack-capi",
            "--target-dir",
        ])
        .arg(&work)
        .current_dir(&root));
    let lib = work.join("release").join("libsamplepack_capi.a");

    let errors = [
        ("SP_ERR_INVALID_ARGUMENT", Error::InvalidArgument),
        ("SP_ERR_BUFFER_TOO_SMALL", Error::BufferTooSmall),
        ("SP_ERR_TRUNCATED", Error::Truncated),
        ("SP_ERR_BAD_SYNC", Error::BadSync),
        ("SP_ERR_RESERVED_VALUE", Error::ReservedValue),
        ("SP_ERR_UNSUPPORTED", Error::Unsupported),
        ("SP_ERR_HEADER_CRC", Error::HeaderCrc),
        ("SP_ERR_BAD_SUBFRAME", Error::BadSubframe),
        ("SP_ERR_BAD_RESIDUAL", Error::BadResidual),
        ("SP_ERR_OVERRUN", Error::Overrun),
        ("SP_ERR_FRAME_CRC", Error::FrameCrc),
        ("SP_ERR_BAD_STREAM", Error::BadStream),
    ];
    let mut e = String::new();
    for (name, v) in errors {
        e += &format!("_Static_assert({name} == {}, \"{name}\");\n", v as i32);
    }
    use pack::Error as P;
    let pack_errors = [
        ("SP_ERR_NOT_PACK", P::NotPack),
        ("SP_ERR_UNSUPPORTED_VERSION", P::UnsupportedVersion),
        ("SP_ERR_NO_FOOTER", P::NoFooter),
        ("SP_ERR_TOO_LARGE", P::TooLarge),
        ("SP_ERR_BAD_CHECKSUM", P::BadChecksum),
        ("SP_ERR_BAD_ENTRY", P::BadEntry),
        ("SP_ERR_BAD_FOOTER", P::BadFooter),
        ("SP_ERR_BAD_TRAILER", P::BadTrailer),
        ("SP_ERR_BAD_OFFSET", P::BadOffset),
        ("SP_ERR_OVERLAP", P::Overlap),
        ("SP_ERR_RESERVED_NONZERO", P::ReservedNonZero),
        ("SP_ERR_BAD_FORMAT", P::BadFormat),
        ("SP_ERR_BAD_LAYOUT", P::BadLayout),
        ("SP_ERR_BAD_FRAME_TABLE", P::BadFrameTable),
        ("SP_ERR_BAD_CREDIT", P::BadCredit),
        ("SP_ERR_DUPLICATE_ID", P::DuplicateId),
        ("SP_ERR_NOT_LIVE", P::NotLive),
        ("SP_ERR_MISMATCH", P::Mismatch),
        ("SP_ERR_NOT_FOUND", P::NotFound),
        ("SP_ERR_FRAME_MISMATCH", P::FrameMismatch),
        ("SP_ERR_PADDING", P::Padding),
        ("SP_ERR_WINDOW_MISMATCH", P::WindowMismatch),
        ("SP_ERR_MD5_MISMATCH", P::Md5Mismatch),
        ("SP_ERR_ID_MISMATCH", P::IdMismatch),
        ("SP_ERR_TRAILER_MISMATCH", P::TrailerMismatch),
        ("SP_ERR_NOT_RECORD", P::NotRecord),
        ("SP_ERR_INVALID_ARGUMENT", P::InvalidArgument),
        ("SP_ERR_BUFFER_TOO_SMALL", P::BufferTooSmall),
    ];
    let mut codes: Vec<i32> = pack_errors.iter().map(|(_, v)| v.code()).collect();
    codes.sort();
    codes.dedup();
    assert_eq!(codes.len(), pack_errors.len(), "pack error codes are distinct");
    for (name, v) in pack_errors {
        e += &format!("_Static_assert({name} == {}, \"{name}\");\n", v.code());
    }
    let ri =
        |name: &str, off: usize| format!("_Static_assert(offsetof(sp_record_info, {name}) == {off}, \"ri.{name}\");\n");
    e += &format!(
        "_Static_assert(sizeof(sp_record_info) == {}, \"ri size\");\n",
        size_of::<RecordInfo>()
    );
    e += &format!(
        "_Static_assert(_Alignof(sp_record_info) == {}, \"ri align\");\n",
        align_of::<RecordInfo>()
    );
    for (n, o) in [
        ("offset", offset_of!(RecordInfo, offset)),
        ("length", offset_of!(RecordInfo, length)),
        ("id", offset_of!(RecordInfo, id)),
        ("md5", offset_of!(RecordInfo, md5)),
        ("rate", offset_of!(RecordInfo, rate)),
        ("channels", offset_of!(RecordInfo, channels)),
        ("bits", offset_of!(RecordInfo, bits)),
        ("frames", offset_of!(RecordInfo, frames)),
        ("window_frames", offset_of!(RecordInfo, window_frames)),
        ("block_size", offset_of!(RecordInfo, block_size)),
        ("blocks", offset_of!(RecordInfo, blocks)),
        ("frame_bytes", offset_of!(RecordInfo, frame_bytes)),
        ("head_len", offset_of!(RecordInfo, head_len)),
        ("window_bytes", offset_of!(RecordInfo, window_bytes)),
        ("credit_offset", offset_of!(RecordInfo, credit_offset)),
        ("credit_len", offset_of!(RecordInfo, credit_len)),
        ("table_offset", offset_of!(RecordInfo, table_offset)),
        ("min_frame", offset_of!(RecordInfo, min_frame)),
        ("max_frame", offset_of!(RecordInfo, max_frame)),
        ("minor", offset_of!(RecordInfo, minor)),
    ] {
        e += &ri(n, o);
    }
    e += &format!(
        "_Static_assert(sizeof(sp_index_entry) == {}, \"ie size\");\n",
        size_of::<IndexEntry>()
    );
    e += &format!(
        "_Static_assert(_Alignof(sp_index_entry) == {}, \"ie align\");\n",
        align_of::<IndexEntry>()
    );
    for (n, o) in [
        ("id", offset_of!(IndexEntry, id)),
        ("offset", offset_of!(IndexEntry, offset)),
        ("length", offset_of!(IndexEntry, length)),
        ("preload", offset_of!(IndexEntry, preload)),
    ] {
        e += &format!("_Static_assert(offsetof(sp_index_entry, {n}) == {o}, \"ie.{n}\");\n");
    }
    e += &format!(
        "_Static_assert(sizeof(sp_verify_scratch) == {}, \"vs size\");\n",
        size_of::<VerifyScratch>()
    );
    e += &format!(
        "_Static_assert(_Alignof(sp_verify_scratch) == {}, \"vs align\");\n",
        align_of::<VerifyScratch>()
    );
    for (n, v) in [
        ("SP_PACK_PAGE", pack::PAGE),
        ("SP_PACK_MAX_BYTES", pack::MAX_PACK),
        ("SP_PACK_MAX_RECORDS", pack::MAX_RECORDS),
        ("SP_PACK_MAX_CREDIT", pack::MAX_CREDIT),
        ("SP_SAMPLE_ID_BYTES", samplepack::id::ID_BYTES),
    ] {
        e += &format!("_Static_assert({n} == {v}, \"{n}\");\n");
    }
    let expect = format!(
        "#define EXPECT_SCRATCH_SIZE {}\n#define EXPECT_SCRATCH_ALIGN {}\n#define EXPECT_INFO_SIZE {}\n#define EXPECT_INFO_ALIGN {}\n\
         #define EXPECT_INFO_SAMPLES {}\n#define EXPECT_INFO_CHANNELS {}\n#define EXPECT_INFO_CONSUMED {}\n#define EXPECT_INFO_RESERVED {}\n\
         #define EXPECT_ENTRY_SIZE {}\n#define EXPECT_ENTRY_ALIGN {}\n#define EXPECT_ENTRY_OFFSET {}\n#define EXPECT_ENTRY_LENGTH {}\n#define EXPECT_ENTRY_SAMPLES {}\n\
         #define EXPECT_MAX_BLOCK {}\n#define EXPECT_MAX_LPC {}\n#define EXPECT_FLAG_CRC {}u\n#define EXPECT_ENUM_ASSERTS \\\n{}\n",
        size_of::<Scratch>(), align_of::<Scratch>(), size_of::<FrameInfo>(), align_of::<FrameInfo>(),
        offset_of!(FrameInfo, samples), offset_of!(FrameInfo, channels), offset_of!(FrameInfo, consumed), offset_of!(FrameInfo, reserved),
        size_of::<FrameEntry>(), align_of::<FrameEntry>(), offset_of!(FrameEntry, offset), offset_of!(FrameEntry, length), offset_of!(FrameEntry, samples),
        MAX_BLOCK, MAX_LPC_ORDER, FLAG_CHECK_CRC,
        e.lines().map(|l| format!("{l} \\\n")).collect::<String>() + "\n"
    );
    std::fs::write(work.join("expect.h"), expect).unwrap();

    let exe = work.join("abi_check");
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    run(Command::new(&cc)
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-O1"])
        .arg(format!("-I{}", root.join("include").display()))
        .arg(format!("-I{}", work.display()))
        .arg(root.join("tests").join("abi_check.c"))
        .arg(&lib)
        .arg("-o")
        .arg(&exe));
    // the header must also be valid C++
    let cxx = std::env::var("CXX").unwrap_or_else(|_| "c++".into());
    std::fs::write(
        work.join("hdr.cpp"),
        "#include \"samplepack.h\"\nint main() { sp_flac_scratch* s = nullptr; sp_record_info i{}; (void)s; (void)i; return SP_OK; }\n",
    )
    .unwrap();
    run(Command::new(&cxx)
        .args(["-std=c++17", "-Wall", "-Wextra", "-Werror"])
        .arg(format!("-I{}", root.join("include").display()))
        .arg(work.join("hdr.cpp"))
        .arg(&lib)
        .arg("-o")
        .arg(work.join("hdr")));
    let data = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join("sweep-st-b1024-l4-r44100.flac");
    // a pack of two corpus records, made with the library's writer
    let a = samplepack_tests::record("sweep-st-b1024-l4-r44100", 4096, "abi test");
    let b = samplepack_tests::record("sweep-mono-b1024-l4-r44100", 1024, "");
    let pk = work.join("abi.osp");
    std::fs::write(&pk, samplepack_tests::build(&[&a, &b])).unwrap();
    let mut pcm_file = Vec::new();
    for v in samplepack_tests::stream("sweep-st-b1024-l4-r44100").pcm {
        pcm_file.extend(v.to_le_bytes());
    }
    let pcm = work.join("abi.pcm");
    std::fs::write(&pcm, pcm_file).unwrap();
    run(Command::new(&exe).arg(data).arg(&pk).arg(&pcm));
}
