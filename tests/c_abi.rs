// SPDX-License-Identifier: MIT
//! Builds the static library, compiles `abi_check.c` against `include/samplepack.h` and the
//! library, and runs it. Struct sizes, offsets and enum values in the header are compared with
//! the Rust definitions; the C file also calls the interface.

use samplepack::flac::{Error, FrameEntry, FrameInfo, Scratch, FLAG_CHECK_CRC, MAX_BLOCK, MAX_LPC_ORDER};
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
        "#include \"samplepack.h\"\nint main() { sp_flac_scratch* s = nullptr; (void)s; return SP_OK; }\n",
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
    run(Command::new(&exe).arg(data));
}
