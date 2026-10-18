use fluent_zero_build::{FluentZeroBuilder, MissingTranslations};

fn main() {
    // fr-FR is deliberately partial; don't warn about it on every build.
    FluentZeroBuilder::new("locales")
        .missing_translations(MissingTranslations::Allow)
        .generate();
}
