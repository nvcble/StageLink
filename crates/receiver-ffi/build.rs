// Give the Android cdylib a SONAME.
//
// Without `DT_SONAME`, a consumer that links against this library *by path* —
// which is exactly what the `IMPORTED` CMake target in the Oboe shim does —
// gets that build-machine path recorded as its own `DT_NEEDED` entry. On a
// phone the loader then goes looking for
// `C:/Users/.../jniLibs/arm64-v8a/libstagelink_receiver.so`, does not find it,
// and `System.loadLibrary("stagelink_sink")` throws `UnsatisfiedLinkError`
// from `AudioSink`'s static initialiser — before the first frame. The symptom
// is an app that will not open at all, with nothing on screen to say why.
//
// Android only. `-Wl,-soname` is an ELF linker concept, and the host builds
// this crate as a cdylib too, for the Flutter FFI tests to load.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        // Must match `[lib] name` in Cargo.toml, which is what the file is
        // called and therefore what `System.loadLibrary` resolves to.
        println!("cargo::rustc-link-arg-cdylib=-Wl,-soname,libstagelink_receiver.so");
    }
    println!("cargo::rerun-if-changed=build.rs");
}
