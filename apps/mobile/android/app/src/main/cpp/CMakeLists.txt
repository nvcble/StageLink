# The audio sink.
#
# This is the only part of the receiver written twice — once per platform —
# which is why it is kept as small as it can be: open an Oboe stream, and in
# its callback call `slr_render_stereo`. Every decision about what those
# samples are has already been made in Rust.

cmake_minimum_required(VERSION 3.22.1)
project(stagelink_sink)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# Oboe arrives as a prefab AAR, so CMake finds it by package rather than by
# path. Oboe rather than raw AAudio because it handles the device quirks and
# the pre-AAudio fallback that would otherwise be ours to get wrong on
# hardware we do not have.
find_package(oboe REQUIRED CONFIG)

add_library(stagelink_sink SHARED stagelink_sink.cpp)

# The repository root, computed rather than written as a run of `../`:
# counting seven of those by hand is how the include path ends up one level
# short and the error is a missing header rather than a wrong path.
#
#   cpp -> main -> src -> app -> android -> mobile -> apps -> <repo>
get_filename_component(REPO_ROOT "${CMAKE_SOURCE_DIR}/../../../../../../.." ABSOLUTE)

if(NOT EXISTS "${REPO_ROOT}/crates/receiver-ffi/include/stagelink_receiver.h")
    message(FATAL_ERROR
        "Could not find the receiver header under ${REPO_ROOT}. "
        "This file assumes apps/mobile/android/app/src/main/cpp inside the "
        "StageLink repository.")
endif()

# The Rust library, cross-compiled by `cargo build --target <abi>
# --profile mobile` and copied into jniLibs by `tool/build_android.ps1`.
# Imported rather than built here: Gradle does not know how to drive Cargo and
# teaching it would couple two build systems for no gain.
set(RECEIVER_SO
    "${CMAKE_SOURCE_DIR}/../jniLibs/${ANDROID_ABI}/libstagelink_receiver.so")
if(NOT EXISTS "${RECEIVER_SO}")
    message(FATAL_ERROR
        "${RECEIVER_SO} is missing. Build it first:\n"
        "  pwsh apps/mobile/tool/build_android.ps1")
endif()

add_library(stagelink_receiver SHARED IMPORTED)
set_target_properties(stagelink_receiver PROPERTIES
    IMPORTED_LOCATION "${RECEIVER_SO}")

target_include_directories(stagelink_sink PRIVATE
    "${REPO_ROOT}/crates/receiver-ffi/include")

target_link_libraries(stagelink_sink
    PRIVATE oboe::oboe stagelink_receiver android log)
