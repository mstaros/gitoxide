//! Binding generation.
//!
//! This is a test, not a `build.rs`, because a build script cannot call
//! into the crate it is building - and the inventory is defined by that
//! crate. This is also how interoptopus' own reference project does it.
//!
//! Run with `cargo test --test generate_bindings`.

use interoptopus_csharp::RustLibrary;
use interoptopus_csharp::dispatch::Dispatch;

/// Where generated C# lands. Checked in, so the C# side can build without
/// a Rust toolchain and so generator changes show up as reviewable diffs.
const OUT_DIR: &str = "bindings";

/// C# namespace for all generated declarations.
///
/// This namespace is reserved for the generated implementation. The managed
/// public-surface check identifies all generated types and nested cases by
/// this boundary, without maintaining a fixed list of native resources.
const NAMESPACE: &str = "GixSharp.Native";

#[test]
fn generate_csharp_bindings() -> Result<(), Box<dyn std::error::Error>> {
    let inventory = gix_ffi::ffi_inventory();

    // `dll_name` is what lands in `[DllImport("...")]`. The cdylib produced
    // by this crate is `gix_ffi.dll` / `libgix_ffi.so`, so the name has an
    // underscore even though the package is `gix-ffi`.
    let output = RustLibrary::builder(inventory)
        .dll_name("gix_ffi")
        .dispatch(Dispatch::single_file(NAMESPACE))
        .build()
        .process()?;

    // `write_buffers_to` does not create the directory.
    std::fs::create_dir_all(OUT_DIR)?;
    output.write_buffers_to(OUT_DIR)?;

    Ok(())
}
