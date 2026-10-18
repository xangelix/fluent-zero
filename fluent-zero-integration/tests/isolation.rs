use fluent_zero::{CacheEntry, FluentArgs};
use fluent_zero_integration::{CHARSET, no_isolating};

const FSI: char = '\u{2068}';
const PDI: char = '\u{2069}';

#[test]
fn isolation_is_on_by_default_and_in_the_charset() {
    assert_eq!(
        fluent_zero_integration::CACHE["en-US"]["number"],
        CacheEntry::Static("Take \u{2068}5\u{2069} now")
    );
    assert!(
        CHARSET.contains(FSI) && CHARSET.contains(PDI),
        "{CHARSET:?}"
    );
}

#[test]
fn disabled_isolation_applies_to_static_and_runtime_output() {
    assert_eq!(
        no_isolating::CACHE["en-US"]["number"],
        CacheEntry::Static("Take 5 now")
    );

    let bundle = &***no_isolating::LOCALES.get("en-US").expect("locale bundle");
    let mut args = FluentArgs::new();
    args.set("n", 3);
    let pattern = bundle.get_message("count").and_then(|m| m.value()).unwrap();
    let mut errors = Vec::new();
    let value = bundle.format_pattern(pattern, Some(&args), &mut errors);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(value, "3 files");

    // Static entries still match their runtime bundle byte for byte.
    for (lang, messages) in no_isolating::CACHE.entries() {
        let bundle = &***no_isolating::LOCALES.get(lang).unwrap();
        for (key, entry) in messages.entries() {
            if let CacheEntry::Static(value) = entry {
                let pattern = bundle.get_message(key).and_then(|m| m.value()).unwrap();
                let mut errors = Vec::new();
                assert_eq!(*value, bundle.format_pattern(pattern, None, &mut errors));
                assert!(errors.is_empty(), "{lang}/{key}: {errors:?}");
            }
        }
    }

    assert!(
        !no_isolating::CHARSET.contains(FSI) && !no_isolating::CHARSET.contains(PDI),
        "{:?}",
        no_isolating::CHARSET
    );
}
