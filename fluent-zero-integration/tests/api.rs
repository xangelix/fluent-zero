use std::borrow::Cow;

use fluent_zero::{CacheEntry, LanguageIdentifier, get_lang, set_lang, t};
// `t!` reads `crate::CATALOG` and `crate::__FLUENT_ZERO_KEYS`; importing them
// at the root of this test crate is enough.
use fluent_zero_integration::{__FLUENT_ZERO_KEYS, CACHE, CATALOG, renamed};

#[test]
fn t_macro_and_locale_state_work_through_the_public_api() {
    set_lang("en-US".parse().unwrap());
    let state = get_lang();
    assert_eq!(state.key(), "en-US");
    assert_eq!(state.id(), &"en-US".parse::<LanguageIdentifier>().unwrap());

    let plain = t!("plain");
    assert_eq!(plain, "Hello");
    assert!(matches!(plain, Cow::Borrowed(_)));
    assert_eq!(
        t!("greeting", { "name" => "Ada" }),
        "Hello \u{2068}Ada\u{2069}"
    );
}

#[test]
fn renamed_crate_output_matches_the_default_output() {
    assert_eq!(renamed::CACHE.len(), CACHE.len());
    for (lang, messages) in CACHE.entries() {
        let renamed_messages = renamed::CACHE.get(lang).expect("locale in renamed output");
        for (key, entry) in messages.entries() {
            assert_eq!(renamed_messages.get(key), Some(entry), "{lang}/{key}");
        }
    }
    assert!(matches!(
        renamed::CACHE.get("en-US").and_then(|m| m.get("plain")),
        Some(CacheEntry::Static(_))
    ));
    assert!(renamed::LOCALES.get("fr-FR").is_some());
    assert_eq!(renamed::CATALOG.fallback(), CATALOG.fallback());
    assert_eq!(renamed::__FLUENT_ZERO_KEYS, __FLUENT_ZERO_KEYS);
    assert_eq!(renamed::keys::PLAIN, "plain");
}
