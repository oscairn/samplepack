# SPDX-License-Identifier: MIT
#
# Builds the samplepack static library with cargo for the active target and exposes it as the
# imported target samplepack::samplepack (include directory: include/, header: samplepack.h).
#
#   list(APPEND CMAKE_MODULE_PATH "${CMAKE_CURRENT_SOURCE_DIR}/path/to/samplepack/cmake")
#   include(SamplePack)
#   target_link_libraries(my_plugin PRIVATE samplepack::samplepack)
#
# Inputs (cache variables, all optional):
#   SAMPLEPACK_RUST_TARGET   Rust target triple. Default: derived from CMAKE_SYSTEM_NAME / architecture;
#                            for a native build, the host triple that rustc reports.
#   SAMPLEPACK_RUSTFLAGS     Extra rustc flags, a ;-list (e.g. "-C;target-cpu=x86-64-v2").
#   SAMPLEPACK_CARGO         cargo executable (default: cargo on PATH; use the rustup shim so that
#                            rust-toolchain.toml selects the pinned compiler).
# macOS: with CMAKE_OSX_ARCHITECTURES set to several architectures, each is built and joined with lipo.
#
# The build needs no network: the workspace has no dependencies and cargo runs with --locked --offline.
# Paths are remapped (--remap-path-prefix) so the library does not depend on where the source or build
# tree lives.

get_filename_component(_sp_root "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)

set(SAMPLEPACK_CARGO "cargo" CACHE STRING "cargo executable used to build samplepack")
set(SAMPLEPACK_RUST_TARGET "" CACHE STRING "Rust target triple for samplepack (empty: derive)")
set(SAMPLEPACK_RUSTFLAGS "" CACHE STRING "extra rustc flags for samplepack (;-list)")

# Maps a CMake architecture name to the Rust triple for the active system.
function(_samplepack_triple arch out)
  string(TOLOWER "${arch}" a)
  if(a MATCHES "^(arm64|aarch64|arm64e)$")
    set(cpu aarch64)
  elseif(a MATCHES "^(x86_64|amd64|x64)$")
    set(cpu x86_64)
  else()
    message(FATAL_ERROR "samplepack: unsupported architecture '${arch}'; set SAMPLEPACK_RUST_TARGET")
  endif()
  if(CMAKE_SYSTEM_NAME STREQUAL "Darwin")
    set(t "${cpu}-apple-darwin")
  elseif(CMAKE_SYSTEM_NAME STREQUAL "Linux")
    set(t "${cpu}-unknown-linux-gnu")
  elseif(CMAKE_SYSTEM_NAME STREQUAL "Windows")
    set(t "${cpu}-pc-windows-msvc")
  else()
    message(FATAL_ERROR "samplepack: unsupported system '${CMAKE_SYSTEM_NAME}'; set SAMPLEPACK_RUST_TARGET")
  endif()
  set(${out} "${t}" PARENT_SCOPE)
endfunction()

set(_sp_triples "")
if(SAMPLEPACK_RUST_TARGET)
  set(_sp_triples "${SAMPLEPACK_RUST_TARGET}")
elseif(CMAKE_SYSTEM_NAME STREQUAL "Darwin" AND CMAKE_OSX_ARCHITECTURES)
  foreach(a IN LISTS CMAKE_OSX_ARCHITECTURES)
    _samplepack_triple("${a}" t)
    list(APPEND _sp_triples "${t}")
  endforeach()
else()
  _samplepack_triple("${CMAKE_SYSTEM_PROCESSOR}" t)
  set(_sp_triples "${t}")
endif()

if(MSVC OR CMAKE_SYSTEM_NAME STREQUAL "Windows")
  set(_sp_libname "samplepack_capi.lib")
else()
  set(_sp_libname "libsamplepack_capi.a")
endif()

# CARGO_ENCODED_RUSTFLAGS separates flags with 0x1f, so paths may contain spaces.
string(ASCII 31 _sp_us)
set(_sp_flags "--remap-path-prefix" "${_sp_root}=/samplepack" "--remap-path-prefix" "${CMAKE_BINARY_DIR}=/build")
list(APPEND _sp_flags ${SAMPLEPACK_RUSTFLAGS})
string(REPLACE ";" "${_sp_us}" _sp_encoded "${_sp_flags}")

set(_sp_outputs "")
set(_sp_built "")
foreach(t IN LISTS _sp_triples)
  set(_dir "${CMAKE_BINARY_DIR}/samplepack-cargo/${t}")
  set(_out "${_dir}/${t}/release/${_sp_libname}")
  add_custom_command(
    OUTPUT "${_out}"
    COMMAND ${CMAKE_COMMAND} -E env "CARGO_ENCODED_RUSTFLAGS=${_sp_encoded}"
            ${SAMPLEPACK_CARGO} build --release --locked --offline -p samplepack-capi
            --target "${t}" --target-dir "${_dir}"
    WORKING_DIRECTORY "${_sp_root}"
    DEPENDS "${_sp_root}/Cargo.toml" "${_sp_root}/Cargo.lock" "${_sp_root}/rust-toolchain.toml"
            "${_sp_root}/crates/samplepack/Cargo.toml" "${_sp_root}/crates/samplepack-capi/Cargo.toml"
    COMMENT "cargo build samplepack (${t})"
    VERBATIM)
  list(APPEND _sp_built "${_out}")
endforeach()

list(LENGTH _sp_built _sp_n)
if(_sp_n EQUAL 1)
  list(GET _sp_built 0 _sp_lib)
else()
  set(_sp_lib "${CMAKE_BINARY_DIR}/samplepack-cargo/universal/${_sp_libname}")
  add_custom_command(
    OUTPUT "${_sp_lib}"
    COMMAND ${CMAKE_COMMAND} -E make_directory "${CMAKE_BINARY_DIR}/samplepack-cargo/universal"
    COMMAND lipo -create ${_sp_built} -output "${_sp_lib}"
    DEPENDS ${_sp_built}
    COMMENT "lipo samplepack"
    VERBATIM)
endif()

add_custom_target(samplepack_build DEPENDS "${_sp_lib}")
add_library(samplepack::samplepack STATIC IMPORTED GLOBAL)
set_target_properties(samplepack::samplepack PROPERTIES
  IMPORTED_LOCATION "${_sp_lib}"
  INTERFACE_INCLUDE_DIRECTORIES "${_sp_root}/include")
add_dependencies(samplepack::samplepack samplepack_build)
