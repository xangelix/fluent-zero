//! The opt-in runtime error handler, driven through real generated code.

use std::{
    borrow::Cow,
    sync::{Mutex, MutexGuard},
};

use fluent_zero::{
    FluentArgs, LookupError, clear_error_handler, lookup_dynamic, lookup_static, set_error_handler,
    set_lang,
};
use fluent_zero_integration::{CACHE, LOCALES};

/// What the handler saw, as owned strings: `(variant, key, lang, display)`.
static SEEN: Mutex<Vec<(&str, String, String, String)>> = Mutex::new(Vec::new());

/// The handler and the language are process-wide; run one test at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn record(error: &LookupError<'_>) {
    let (variant, key, lang) = match error {
        LookupError::MissingKey { key, lang } => ("missing-key", key, lang),
        LookupError::MissingValue { key, lang } => ("missing-value", key, lang),
        LookupError::Format { key, lang, errors } => {
            assert!(!errors.is_empty());
            ("format", key, lang)
        }
        _ => unreachable!("unknown LookupError variant"),
    };
    SEEN.lock().unwrap().push((
        variant,
        key.to_string(),
        lang.to_string(),
        error.to_string(),
    ));
}

/// Serialises the test, resets the language and installs a fresh recorder.
fn setup() -> MutexGuard<'static, ()> {
    let guard = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    set_lang("en-US".parse().unwrap());
    SEEN.lock().unwrap().clear();
    set_error_handler(record);
    guard
}

fn seen() -> Vec<(&'static str, String, String, String)> {
    std::mem::take(&mut *SEEN.lock().unwrap())
}

#[test]
fn successful_lookups_report_nothing() {
    let _guard = setup();
    assert!(matches!(
        lookup_static(&LOCALES, &CACHE, "plain"),
        Cow::Borrowed("Hello")
    ));
    let mut args = FluentArgs::new();
    args.set("name", "Bob");
    assert_eq!(
        lookup_dynamic(&LOCALES, &CACHE, "greeting", &args),
        "Hello \u{2068}Bob\u{2069}"
    );
    // A key missing from fr-FR but present in the fallback is not an error.
    set_lang("fr-FR".parse().unwrap());
    assert_eq!(lookup_static(&LOCALES, &CACHE, "about"), "About Acme");
    assert_eq!(seen(), []);
}

#[test]
fn missing_key_is_reported_with_the_current_language() {
    let _guard = setup();
    set_lang("fr-FR".parse().unwrap());
    assert_eq!(
        lookup_static(&LOCALES, &CACHE, "no-such-key"),
        "no-such-key"
    );
    assert_eq!(
        seen(),
        [(
            "missing-key",
            "no-such-key".to_string(),
            "fr-FR".to_string(),
            "missing message `no-such-key` (language fr-FR)".to_string()
        )]
    );
}

#[test]
fn missing_argument_is_reported_and_still_renders() {
    let _guard = setup();
    let value = lookup_static(&LOCALES, &CACHE, "greeting");
    assert_eq!(value, "Hello \u{2068}{$name}\u{2069}");
    let seen = seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    let (variant, key, lang, display) = &seen[0];
    assert_eq!(
        (*variant, key.as_str(), lang.as_str()),
        ("format", "greeting", "en-US")
    );
    assert!(
        display.starts_with("failed to format message `greeting` (language en-US): "),
        "{display}"
    );
    assert!(display.contains("name"), "{display}");
}

#[test]
fn message_without_value_is_reported_then_missing() {
    let _guard = setup();
    // The attribute of an attribute-only message is a normal lookup...
    assert_eq!(
        lookup_static(&LOCALES, &CACHE, "attributes-only.title"),
        "Title"
    );
    assert_eq!(seen(), []);
    // ...but the bare ID has no value to render.
    assert_eq!(
        lookup_static(&LOCALES, &CACHE, "attributes-only"),
        "attributes-only"
    );
    let variants: Vec<_> = seen().into_iter().map(|(v, k, l, _)| (v, k, l)).collect();
    assert_eq!(
        variants,
        [
            ("missing-value", "attributes-only".into(), "en-US".into()),
            ("missing-key", "attributes-only".into(), "en-US".into()),
        ]
    );
}

#[test]
fn attribute_fallback_reports_nothing() {
    let _guard = setup();
    set_lang("fr-FR".parse().unwrap());
    assert_eq!(
        lookup_static(&LOCALES, &CACHE, "login.tooltip"),
        "Se connecter"
    );
    assert_eq!(
        lookup_static(&LOCALES, &CACHE, "login.en-only"),
        "Only in English"
    );
    assert_eq!(seen(), []);
}

#[test]
fn missing_attribute_is_reported_as_an_attribute() {
    let _guard = setup();
    set_lang("fr-FR".parse().unwrap());
    // The message exists in both languages, the attribute in neither.
    assert_eq!(lookup_static(&LOCALES, &CACHE, "login.nope"), "login.nope");
    assert_eq!(
        seen(),
        [(
            "missing-key",
            "login.nope".to_string(),
            "fr-FR".to_string(),
            "missing attribute `login.nope` (language fr-FR)".to_string()
        )]
    );
}

#[test]
fn missing_attribute_argument_is_reported_with_the_attribute_key() {
    let _guard = setup();
    assert_eq!(
        lookup_static(&LOCALES, &CACHE, "login.greeting"),
        "Welcome back, \u{2068}{$name}\u{2069}"
    );
    let variants: Vec<_> = seen().into_iter().map(|(v, k, l, _)| (v, k, l)).collect();
    assert_eq!(
        variants,
        [("format", "login.greeting".into(), "en-US".into())]
    );
}

#[test]
fn cleared_handler_is_not_called() {
    let _guard = setup();
    clear_error_handler();
    assert_eq!(
        lookup_static(&LOCALES, &CACHE, "no-such-key"),
        "no-such-key"
    );
    assert_eq!(seen(), []);
}
