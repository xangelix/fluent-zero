use std::{env, fs, path::Path};

fn main() {
    // The same locales without bidi isolation, kept under a second name for
    // `src/lib.rs` to include alongside the default output.
    fluent_zero_build::FluentZeroBuilder::new("locales")
        .use_isolating(false)
        .generate();
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let out_dir = Path::new(&out_dir);
    fs::rename(
        out_dir.join("static_cache.rs"),
        out_dir.join("static_cache_no_isolating.rs"),
    )
    .expect("Failed to rename the non-isolating cache");

    fluent_zero_build::generate_static_cache("locales");
}
