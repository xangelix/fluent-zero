fn main() {
    fluent_zero_build::FluentZeroBuilder::new("locales")
        .fallback_locale("en-US")
        .generate();
}
