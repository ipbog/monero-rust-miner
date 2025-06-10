// modules/mining_engine/build.rs
fn main() {
    // Ensure Cargo recompiles if these files change.
    // Note: The prompt used "src/randomx_wrapper.c" and "src/randomx_wrapper.h"
    // but the files were created at the module root "randomx_wrapper.c" and "randomx_wrapper.h".
    // Adjusting paths accordingly.
    println!("cargo:rerun-if-changed=randomx_wrapper.c");
    println!("cargo:rerun-if-changed=randomx_wrapper.h");

    cc::Build::new()
        .file("randomx_wrapper.c") // Path relative to module root (where build.rs is)
        .include(".")              // To find randomx_wrapper.h in the same directory
        // Add any other necessary compiler flags or definitions for the dummy wrapper if needed
        // For this dummy wrapper, no special flags should be necessary.
        .compile("librandomx_wrapper"); // Output static library name: lib<name>.a

    // This line tells Rustc to link against the static library we just compiled.
    println!("cargo:rustc-link-lib=static=randomx_wrapper");

    // The output directory of cc::Build is automatically added to the linker search path.
    // No need for explicit println!("cargo:rustc-link-search=native={}", out_dir.display());
    // unless the library was placed in a non-standard location by the build script.
}
