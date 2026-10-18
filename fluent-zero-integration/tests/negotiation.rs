//! Locale negotiation, the configured fallback and per-call locales, through
//! the `CATALOG` the builder generates for `locales/` (`en-US`, `fr-FR`,
//! `pt-PT`, `zh-Hant`).

use std::borrow::Cow;

use fluent_zero::{LocaleState, set_lang, t, t_in};
// `t!` and `t_in!` resolve `crate::CATALOG`.
use fluent_zero_integration::{CACHE, CATALOG, FALLBACK, LOCALES};

fn lang(tag: &str) -> LocaleState {
    LocaleState::new(tag.parse().expect("valid language id"))
}

#[test]
fn bare_language_finds_its_regional_locale() {
    // The review's repro: `ru` with only `ru-RU/`; here `fr` with `fr-FR/`.
    let plain = t_in!(&lang("fr"), "plain");
    assert_eq!(plain, "Bonjour");
    assert!(matches!(plain, Cow::Borrowed(_)));
    assert_eq!(t_in!(&lang("pt"), "plain"), "Olá");
}

#[test]
fn regional_variant_finds_a_sibling_locale() {
    assert_eq!(t_in!(&lang("pt-BR"), "plain"), "Olá");
    assert_eq!(t_in!(&lang("fr-CA"), "plain"), "Bonjour");
    assert_eq!(t_in!(&lang("en-GB"), "plain"), "Hello");
}

#[test]
fn script_implying_regions_find_the_script_locale() {
    for tag in ["zh-TW", "zh-HK", "zh-Hant-TW", "zh"] {
        assert_eq!(t_in!(&lang(tag), "plain"), "你好", "{tag}");
    }
}

#[test]
fn dynamic_messages_resolve_through_negotiation() {
    assert_eq!(
        t_in!(&lang("pt-BR"), "greeting", { "name" => "Ana" }),
        "Olá \u{2068}Ana\u{2069}"
    );
}

#[test]
fn missing_messages_use_the_configured_fallback() {
    assert_eq!(FALLBACK, "en-US");
    assert_eq!(CATALOG.fallback(), FALLBACK);
    assert_eq!(t_in!(&lang("pt-BR"), "about"), "About Acme");
    assert_eq!(t_in!(&lang("de"), "plain"), "Hello");
    assert_eq!(t_in!(&lang("de"), "no-such-message"), "no-such-message");
}

#[test]
fn attributes_resolve_through_negotiation_and_fallback() {
    // `fr-CA` negotiates to `fr-FR`, which has a static `login.tooltip`.
    let tooltip = t_in!(&lang("fr-CA"), "login.tooltip");
    assert_eq!(tooltip, "Se connecter");
    assert!(matches!(tooltip, Cow::Borrowed(_)));
    // Missing from `fr-FR`: the configured fallback, like message values.
    assert_eq!(t_in!(&lang("fr"), "login.en-only"), "Only in English");
    assert_eq!(t_in!(&lang("pt-BR"), "search.placeholder"), "Search Acme");
    assert_eq!(
        t_in!(&lang("zh-TW"), "login.greeting", { "name" => "Ana" }),
        "Welcome back, \u{2068}Ana\u{2069}"
    );
    // Missing everywhere: the key itself.
    assert_eq!(t_in!(&lang("fr-CA"), "login.nope"), "login.nope");
}

#[test]
fn aliases_stay_out_of_the_plain_maps() {
    let mut cache: Vec<&str> = CACHE.keys().copied().collect();
    let mut bundles: Vec<&str> = LOCALES.keys().copied().collect();
    cache.sort_unstable();
    bundles.sort_unstable();
    assert_eq!(cache, ["en-US", "fr-FR", "pt-PT", "zh-Hant"]);
    assert_eq!(bundles, cache);
}

// One test owns the global language so no concurrent test can change it.
#[test]
fn global_language_is_negotiated_and_per_call_locales_bypass_it() {
    set_lang("fr".parse().expect("valid language id"));
    assert_eq!(t!("plain"), "Bonjour");
    assert_eq!(t!("about"), "About Acme");
    assert_eq!(t_in!(&lang("pt-BR"), "plain"), "Olá");
    assert_eq!(t!("plain"), "Bonjour");

    assert_eq!(t!("login.tooltip"), "Se connecter");
    assert_eq!(t!("login.en-only"), "Only in English");

    set_lang("pt-BR".parse().expect("valid language id"));
    assert_eq!(
        t!("greeting", { "name" => "Ana" }),
        "Olá \u{2068}Ana\u{2069}"
    );

    set_lang("en-US".parse().expect("valid language id"));
    assert_eq!(t!("plain"), "Hello");
}
