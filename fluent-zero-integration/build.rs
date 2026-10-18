use std::{env, fs, path::Path};

use fluent_zero_build::{FluentZeroBuilder, MissingTranslations};

fn main() {
    // fr-FR is deliberately partial, so neither run warns about coverage.
    //
    // The same locales without bidi isolation, kept under a second name for
    // `src/lib.rs` to include alongside the default output.
    FluentZeroBuilder::new("locales")
        .use_isolating(false)
        .missing_translations(MissingTranslations::Allow)
        .generate();
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let out_dir = Path::new(&out_dir);
    fs::rename(
        out_dir.join("static_cache.rs"),
        out_dir.join("static_cache_no_isolating.rs"),
    )
    .expect("Failed to rename the non-isolating cache");

    FluentZeroBuilder::new("locales")
        .missing_translations(MissingTranslations::Allow)
        .generate();
}
