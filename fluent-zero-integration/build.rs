use std::{env, fs, path::Path};

fn main() {
    // The output file name is fixed, so the renamed-crate variant is generated
    // first and moved aside before the default one is written.
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR");
    let generated = Path::new(&out_dir).join("static_cache.rs");
    fluent_zero_build::FluentZeroBuilder::new("locales")
        .crate_path("::fz")
        .try_generate()
        .expect("renamed-crate generation");
    fs::rename(
        &generated,
        generated.with_file_name("static_cache_renamed.rs"),
    )
    .expect("move renamed-crate output");

    fluent_zero_build::generate_static_cache("locales");
}
