use std::collections::BTreeSet;

use fluent_zero::t;
// `t!` resolves these through `crate::`, exactly as in a consumer that
// includes the generated file at its root.
use fluent_zero_integration::{__FLUENT_ZERO_KEYS, CACHE, LOCALES, keys};

#[test]
fn constants_hold_their_message_ids() {
    assert_eq!(keys::PLAIN, "plain");
    assert_eq!(keys::LITERAL_BRACE, "literal-brace");
    assert_eq!(keys::CROSS_FILE, "cross-file");
    // Defined only in fr-FR: every locale contributes keys.
    assert_eq!(keys::FR_ONLY, "fr-only");
}

#[test]
fn constants_resolve_through_t() {
    assert_eq!(t!(keys::PLAIN), "Hello");
    assert_eq!(t!(keys::CROSS_FILE), "About Acme and more");
    assert_eq!(
        t!(keys::GREETING, { "name" => "Bob" }),
        "Hello \u{2068}Bob\u{2069}"
    );
}

#[test]
fn literal_keys_still_compile_and_resolve() {
    assert_eq!(t!("plain"), "Hello");
    assert_eq!(
        t!("greeting", { "name" => "Bob" }),
        "Hello \u{2068}Bob\u{2069}"
    );
}

#[test]
fn non_literal_keys_skip_the_check() {
    let key = "not-a-key";
    assert_eq!(t!(key), "not-a-key");
}

#[test]
fn key_list_is_the_sorted_union_of_every_locale() {
    let union: BTreeSet<&str> = CACHE
        .values()
        .flat_map(|messages| messages.keys().copied())
        .collect();
    assert_eq!(__FLUENT_ZERO_KEYS, union.into_iter().collect::<Vec<_>>());
    assert!(__FLUENT_ZERO_KEYS.is_sorted());
    for key in __FLUENT_ZERO_KEYS {
        assert!(fluent_zero::__key_exists(__FLUENT_ZERO_KEYS, key), "{key}");
    }
    for missing in ["", "a", "plai", "plainx", "zzz"] {
        assert!(
            !fluent_zero::__key_exists(__FLUENT_ZERO_KEYS, missing),
            "{missing}"
        );
    }
}
