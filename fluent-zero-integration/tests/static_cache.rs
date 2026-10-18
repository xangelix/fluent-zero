use fluent_zero::{CacheEntry, FluentArgs, set_lang, t};
// `t!` resolves `crate::CATALOG` and checks literal keys against
// `crate::__FLUENT_ZERO_KEYS`, as in a consumer crate.
use fluent_zero_integration::{__FLUENT_ZERO_KEYS, CACHE, CATALOG, LOCALES};

/// Formats `key` (`message` or `message.attribute`) with the runtime bundle,
/// bypassing the static cache.
fn format_with_bundle(lang: &str, key: &str, args: Option<&FluentArgs>) -> String {
    let bundle = &***LOCALES.get(lang).expect("locale bundle");
    let pattern = match key.split_once('.') {
        None => bundle.get_message(key).and_then(|msg| msg.value()),
        Some((id, attribute)) => bundle
            .get_message(id)
            .and_then(|msg| msg.get_attribute(attribute))
            .map(|attribute| attribute.value()),
    }
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

#[test]
fn attributes_are_cached_under_dotted_keys() {
    assert_eq!(
        entry("en-US", "login.tooltip"),
        CacheEntry::Static("Sign in to Acme")
    );
    assert_eq!(
        entry("en-US", "search.placeholder"),
        CacheEntry::Static("Search Acme")
    );
    assert_eq!(entry("en-US", "login.greeting"), CacheEntry::Dynamic);
    assert_eq!(
        entry("fr-FR", "login.tooltip"),
        CacheEntry::Static("Se connecter")
    );
    assert!(CACHE["fr-FR"].get("login.en-only").is_none());
}

/// The only test that touches the global language, so parallel tests never
/// observe each other's `set_lang`.
#[test]
fn attribute_lookups_through_t_macro() {
    set_lang("en-US".parse().unwrap());
    assert_eq!(t!("login"), "Log in");
    assert_eq!(t!("login.tooltip"), "Sign in to Acme");
    assert_eq!(t!("search.placeholder"), "Search Acme");
    assert_eq!(
        t!("login.greeting", { "name" => "Bob" }),
        "Welcome back, \u{2068}Bob\u{2069}"
    );

    set_lang("fr-FR".parse().unwrap());
    assert_eq!(t!("login.tooltip"), "Se connecter");
    // Missing in fr-FR: falls back to en-US, exactly like message values.
    assert_eq!(t!("login.en-only"), "Only in English");
    assert_eq!(
        t!("login.greeting", { "name" => "Bob" }),
        "Welcome back, \u{2068}Bob\u{2069}"
    );
    // Missing everywhere: the key itself. A literal unknown key is a compile
    // error, so these go through runtime values.
    let (missing_attr, missing_msg) = ("login.nope", "nope.tooltip");
    assert_eq!(t!(missing_attr), "login.nope");
    assert_eq!(t!(missing_msg), "nope.tooltip");

    set_lang("en-US".parse().unwrap());
}

#[test]
fn generated_code_has_no_panic_paths() {
    let outputs = [
        include_str!(concat!(env!("OUT_DIR"), "/static_cache.rs")),
        include_str!(concat!(env!("OUT_DIR"), "/static_cache_no_isolating.rs")),
    ];
    for generated in outputs {
        for needle in [".unwrap()", ".expect(", "panic!", "unreachable!", "todo!"] {
            assert!(
                !generated.contains(needle),
                "generated code contains `{needle}`"
            );
        }
    }
}

#[test]
fn bundles_initialise_and_format() {
    for (lang, bundle) in LOCALES.entries() {
        assert!(
            bundle.has_message("plain"),
            "{lang} bundle is missing `plain`"
        );
    }
}
