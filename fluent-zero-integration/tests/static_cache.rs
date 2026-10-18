use fluent_zero::{CacheEntry, FluentArgs};
use fluent_zero_integration::{CACHE, LOCALES};

/// Formats `key` with the runtime bundle, bypassing the static cache.
fn format_with_bundle(lang: &str, key: &str, args: Option<&FluentArgs>) -> String {
    let bundle = &***LOCALES.get(lang).expect("locale bundle");
    let pattern = bundle
        .get_message(key)
        .and_then(|msg| msg.value())
        .expect("message value");
    let mut errors = Vec::new();
    let value = bundle.format_pattern(pattern, args, &mut errors);
    assert!(errors.is_empty(), "{key}: {errors:?}");
    value.into_owned()
}

fn entry(lang: &str, key: &str) -> CacheEntry {
    *CACHE
        .get(lang)
        .and_then(|messages| messages.get(key))
        .unwrap_or_else(|| panic!("no cache entry for {lang}/{key}"))
}

#[test]
fn every_static_entry_matches_the_runtime_bundle() {
    let mut checked = 0;
    for (lang, messages) in CACHE.entries() {
        for (key, entry) in messages.entries() {
            if let CacheEntry::Static(value) = entry {
                assert_eq!(*value, format_with_bundle(lang, key, None), "{lang}/{key}");
                checked += 1;
            }
        }
    }
    assert!(checked >= 10, "only {checked} static entries were checked");
}

#[test]
fn messages_resolvable_at_build_time_are_static() {
    for key in [
        "plain",
        "quoted",
        "multiline",
        "literal-brace",
        "number",
        "about",
        "pronoun",
        "nested",
        "cross-file",
    ] {
        assert!(
            matches!(entry("en-US", key), CacheEntry::Static(_)),
            "{key} should be static"
        );
    }
    assert_eq!(
        entry("en-US", "quoted"),
        CacheEntry::Static(r#"Say "hi" to C:\Users"#)
    );
    assert_eq!(
        entry("en-US", "cross-file"),
        CacheEntry::Static("About Acme and more")
    );
}

#[test]
fn messages_with_arguments_stay_dynamic() {
    for key in ["greeting", "count"] {
        assert_eq!(entry("en-US", key), CacheEntry::Dynamic, "{key}");
    }
    let mut args = FluentArgs::new();
    args.set("n", 3);
    assert_eq!(
        format_with_bundle("en-US", "count", Some(&args)),
        "\u{2068}3\u{2069} files"
    );
}
