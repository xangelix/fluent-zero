use std::{env, fs, path::Path};

use fluent_zero_build::{FluentZeroBuilder, MissingTranslations};

/// The settings every variant shares.
///
/// fr-FR, pt-PT and zh-Hant are deliberately partial, so no run warns about
/// coverage. The fallback is set explicitly (to its default) so the build
/// exercises the check that its directory exists.
fn builder() -> FluentZeroBuilder {
    FluentZeroBuilder::new("locales")
        .fallback_locale("en-US")
        .missing_translations(MissingTranslations::Allow)
}

fn main() {
    // The output file name is fixed, so each extra variant is generated first
    // and moved aside under its own name before the default one is written.
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let out_dir = Path::new(&out_dir);
    let move_aside = |name: &str| {
        fs::rename(out_dir.join("static_cache.rs"), out_dir.join(name))
            .unwrap_or_else(|err| panic!("failed to move the generated cache to {name}: {err}"));
    };

    // Every path in it goes through the renamed crate (see `src/lib.rs`).
    builder()
        .crate_path("::fz")
        .try_generate()
        .expect("renamed-crate generation");
    move_aside("static_cache_renamed.rs");

    // The same locales without bidi isolation.
    builder().use_isolating(false).generate();
    move_aside("static_cache_no_isolating.rs");

    builder().generate();
}
