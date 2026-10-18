//! # fluent-zero
//!
//! A zero-allocation, high-performance Fluent localization loader designed for
//! GUIs and games.
//!
//! This crate works in tandem with the `fluent-zero-build` crate. The build script
//! generates static Perfect Hash Maps (PHF) that allow `O(1)` lookups for localized
//! strings. When a string is static (contains no variables), it returns a `&'static str`
//! reference to the binary's read-only data, avoiding all heap allocations.

extern crate self as fluent_zero;

use std::{
    borrow::Cow,
    collections::HashMap,
    hash::BuildHasher,
    sync::{Arc, LazyLock},
};

use arc_swap::ArcSwap;

pub use fluent_bundle::{
    FluentArgs, FluentResource, concurrent::FluentBundle as ConcurrentFluentBundle,
};
pub use fluent_syntax;
pub use phf;
pub use unic_langid::LanguageIdentifier;

/// Represents the result of a cache lookup from the generated PHF map.
///
/// This enum allows the system to distinguish between zero-cost static strings
/// and those that require the heavier `FluentBundle` machinery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheEntry {
    /// The message is static and contains no variables.
    ///
    /// The payload is a direct reference to the string in the binary's data section.
    Static(&'static str),
    /// The message is dynamic (contains variables/selectors).
    ///
    /// This indicates that the system must load the `ConcurrentFluentBundle` to
    /// resolve the final string.
    Dynamic,
}

/// A language and the locale keys a lookup tries for it.
///
/// The global language set by [`set_lang`] is one of these, and servers that
/// render for several users at once can build one per user (or request) and
/// pass it to [`t_in!`], [`lookup_static_in`] or [`lookup_dynamic_in`].
///
/// Building one resolves the fallback chain once (a few small allocations), so
/// every lookup afterwards just walks a short precomputed slice of keys.
pub struct LocaleState {
    /// The parsed identifier (e.g., `en-US`).
    _id: LanguageIdentifier,
    /// The string representation used for cache keys (e.g., "en-US").
    pub key: String,
    /// Candidate locale keys, most specific first: the identifier itself, then
    /// with variants, script and region dropped in turn (`sr-Latn-RS` tries
    /// `sr-Latn-RS`, `sr-Latn`, `sr-RS`, `sr`). The bare language is always
    /// last; each crate's generated `CATALOG` maps it (and script-implying
    /// tags such as `zh-TW`) to its best available locale, so `ru` finds
    /// `ru-RU/` and `pt-BR` finds `pt-PT/` without negotiating per lookup.
    chain: Box<[Box<str>]>,
}

impl LocaleState {
    /// Resolves the fallback chain for `id`.
    #[must_use]
    pub fn new(id: LanguageIdentifier) -> Self {
        let key = id.to_string();
        let language = id.language;
        let (script, region) = (id.script, id.region);
        let mut chain: Vec<Box<str>> = vec![key.as_str().into()];
        for (script, region) in [
            (script, region),
            (script, None),
            (None, region),
            (None, None),
        ] {
            let candidate =
                LanguageIdentifier::from_parts(language, script, region, &[]).to_string();
            if !chain.iter().any(|existing| **existing == *candidate) {
                chain.push(candidate.into());
            }
        }
        Self {
            _id: id,
            key,
            chain: chain.into_boxed_slice(),
        }
    }

    /// The locale keys a lookup tries, most specific first, before the
    /// store's fallback locale.
    #[must_use]
    pub fn chain(&self) -> impl ExactSizeIterator<Item = &str> {
        self.chain.iter().map(|key| &**key)
    }
}

impl From<LanguageIdentifier> for LocaleState {
    fn from(id: LanguageIdentifier) -> Self {
        Self::new(id)
    }
}

/// The global thread-safe storage for the current language.
///
/// Uses `ArcSwap` to allow lock-free reads, which is critical for high-performance
/// hot paths in GUI rendering loops.
static CURRENT_LANG: LazyLock<ArcSwap<LocaleState>> = LazyLock::new(|| {
    let id: LanguageIdentifier = DEFAULT_FALLBACK.parse().unwrap();
    ArcSwap::from_pointee(LocaleState::new(id))
});

/// The fallback locale of stores that do not configure one.
///
/// Generated code exports its own `FALLBACK` (see
/// `FluentZeroBuilder::fallback_locale`), which defaults to this value.
pub const DEFAULT_FALLBACK: &str = "en-US";

/// Updates the runtime language for the application.
///
/// This operation is atomic. Subsequent calls to `t!` will immediately reflect
/// the new language. The fallback chain (see [`LocaleState`]) is resolved here,
/// once, rather than on every lookup.
///
/// # Arguments
///
/// * `lang` - The new `LanguageIdentifier` to set (e.g., parsed from "fr-FR").
pub fn set_lang(lang: LanguageIdentifier) {
    CURRENT_LANG.store(Arc::new(LocaleState::new(lang)));
}

/// Retrieves the current language state.
///
/// Returns a guard containing the `Arc<LocaleState>`. This is primarily used
/// internally by the lookup functions but is exposed for diagnostics.
pub fn get_lang() -> arc_swap::Guard<std::sync::Arc<LocaleState>> {
    CURRENT_LANG.load()
}

/// A store that maps `(Locale, Key)` to a `CacheEntry`.
///
/// This trait exists to abstract over the generated `phf::Map` and standard `HashMap`s
/// used in testing.
pub trait CacheStore: Sync + Send {
    /// Retrieves a cache entry for a specific language and message key.
    fn get_entry(&self, lang: &str, key: &str) -> Option<CacheEntry>;

    /// The locale tried after every key of the language's fallback chain.
    fn fallback_locale(&self) -> &str {
        DEFAULT_FALLBACK
    }
}

// Impl for Generated PHF Map
impl CacheStore for phf::Map<&'static str, &'static phf::Map<&'static str, CacheEntry>> {
    fn get_entry(&self, lang: &str, key: &str) -> Option<CacheEntry> {
        // Single hash on `lang` (usually very small map), then Single hash on `key`.
        self.get(lang).and_then(|m| m.get(key)).copied()
    }
}

/// A collection capable of retrieving a `ConcurrentFluentBundle` by language key.
pub trait BundleCollection: Sync + Send {
    /// Retrieves the bundle for the specified language.
    fn get_bundle(&self, lang: &str) -> Option<&ConcurrentFluentBundle<FluentResource>>;
}

// Impl for Generated PHF Map
impl BundleCollection
    for phf::Map<&'static str, &'static LazyLock<ConcurrentFluentBundle<FluentResource>>>
{
    fn get_bundle(&self, lang: &str) -> Option<&ConcurrentFluentBundle<FluentResource>> {
        self.get(lang).map(|lazy| &***lazy)
    }
}

// Impl for HashMap (For Tests)
impl<S: BuildHasher + Sync + Send> BundleCollection
    for HashMap<String, ConcurrentFluentBundle<FluentResource>, S>
{
    fn get_bundle(&self, lang: &str) -> Option<&ConcurrentFluentBundle<FluentResource>> {
        self.get(lang)
    }
}

/// One locale's tables in a [`Catalog`]: its static cache and its bundle.
pub type CatalogLocale = (
    &'static phf::Map<&'static str, CacheEntry>,
    &'static LazyLock<ConcurrentFluentBundle<FluentResource>>,
);

/// A crate's generated lookup tables, as used by [`t!`] and [`t_in!`].
///
/// Unlike the plain `CACHE` and `LOCALES` maps, a catalog also answers for the
/// language-level keys that end a fallback chain (`ru` when only `ru-RU/`
/// exists, `zh-TW` when only `zh-Hant/` exists), resolved by
/// `fluent-zero-build` against this crate's own locales. Each entry carries
/// both tables, so a lookup costs one map probe per candidate key.
pub struct Catalog {
    fallback: &'static str,
    locales: &'static phf::Map<&'static str, CatalogLocale>,
}

impl Catalog {
    /// Creates a catalog. Called by the code `fluent-zero-build` generates.
    #[must_use]
    pub const fn new(
        fallback: &'static str,
        locales: &'static phf::Map<&'static str, CatalogLocale>,
    ) -> Self {
        Self { fallback, locales }
    }

    /// The locale tried when the language's own chain has no message.
    #[must_use]
    pub const fn fallback(&self) -> &'static str {
        self.fallback
    }
}

impl CacheStore for Catalog {
    fn get_entry(&self, lang: &str, key: &str) -> Option<CacheEntry> {
        self.locales
            .get(lang)
            .and_then(|(messages, _)| messages.get(key))
            .copied()
    }

    fn fallback_locale(&self) -> &str {
        self.fallback
    }
}

impl BundleCollection for Catalog {
    fn get_bundle(&self, lang: &str) -> Option<&ConcurrentFluentBundle<FluentResource>> {
        self.locales.get(lang).map(|(_, bundle)| &***bundle)
    }
}

/// Merges multiple charset strings into a single, deduplicated, and sorted string.
///
/// This function provides an enterprise-quality, build-system agnostic solution for
/// aggregating characters for font subsetting. By combining the `CHARSET` constants
/// generated natively by `fluent-zero-build` from your application and all its
/// dependencies, you seamlessly compute a master charset without brittle `cargo_metadata`
/// scripts or violating Cargo's `OUT_DIR` sandboxing limits.
///
/// # Arguments
///
/// * `charsets` - A slice of `&str` references, typically pointing to `crate::CHARSET`.
///
/// # Returns
///
/// A deterministically sorted `String` containing every unique character.
///
/// # Example
///
/// ```rust,ignore
/// let complete_charset = fluent_zero::join_charsets(&[
///     crate::CHARSET,
///     my_ui_lib::CHARSET,
/// ]);
/// std::fs::write("target/font_charset.txt", complete_charset).unwrap();
/// ```
#[must_use]
pub fn join_charsets(charsets: &[&str]) -> String {
    let mut unique_chars = std::collections::BTreeSet::new();
    for charset in charsets {
        unique_chars.extend(charset.chars());
    }
    unique_chars.into_iter().collect()
}

/// Core internal helper resolving entries efficiently, eliminating fallback duplicate closures.
#[inline]
fn resolve_entry<'a, B: BundleCollection + ?Sized, C: CacheStore + ?Sized>(
    bundles: &'a B,
    cache: &C,
    lang: &str,
    key: &'a str,
    args: Option<&FluentArgs>,
) -> Option<Cow<'a, str>> {
    match cache.get_entry(lang, key)? {
        CacheEntry::Static(s) => Some(Cow::Borrowed(s)),
        CacheEntry::Dynamic => {
            let bundle = bundles.get_bundle(lang)?;
            let msg = bundle.get_message(key)?;
            let pattern = msg.value()?;
            let mut errors = vec![];
            Some(bundle.format_pattern(pattern, args, &mut errors))
        }
    }
}

/// Abstracted O(1) cascade matching a language's fallback chain against established caches.
#[inline]
fn lookup_core<'a, B: BundleCollection + ?Sized, C: CacheStore + ?Sized>(
    bundles: &'a B,
    cache: &C,
    lang: &LocaleState,
    key: &'a str,
    args: Option<&FluentArgs>,
) -> Cow<'a, str> {
    // 1. The language and its fallback chain
    for candidate in &lang.chain {
        if let Some(res) = resolve_entry(bundles, cache, candidate, key, args) {
            return res;
        }
    }

    // 2. Fallback Language
    let fallback = cache.fallback_locale();
    if !lang.chain.iter().any(|candidate| **candidate == *fallback)
        && let Some(res) = resolve_entry(bundles, cache, fallback, key, args)
    {
        return res;
    }

    // 3. Miss
    Cow::Borrowed(key)
}

/// Retrieves a localized message without arguments.
///
/// This function attempts to return a `Cow::Borrowed` referencing static binary data
/// whenever possible to avoid allocation.
///
/// # Resolution Order
///
/// 1. **Current Language**: Checks each key of the current language's fallback
///    chain (see [`LocaleState`]), most specific first.
/// 2. **Fallback Language**: If missing, checks the store's fallback locale
///    ([`DEFAULT_FALLBACK`] unless the store configures one).
/// 3. **Missing Key**: Returns the `key` itself wrapped in `Cow::Borrowed`.
///
/// # Arguments
///
/// * `bundles` - The collection of Fluent bundles (usually `crate::LOCALES`).
/// * `cache` - The static cache map (usually `crate::CACHE`).
/// * `key` - The message ID to look up.
pub fn lookup_static<'a, B: BundleCollection + ?Sized, C: CacheStore + ?Sized>(
    bundles: &'a B,
    cache: &C,
    key: &'a str,
) -> Cow<'a, str> {
    lookup_core(bundles, cache, &get_lang(), key, None)
}

/// Retrieves a localized message with arguments.
///
/// Even when arguments are provided, this function checks if the underlying message
/// is actually static. If so, it ignores the arguments and returns the static string
/// to preserve performance.
///
/// # Arguments
///
/// * `bundles` - The collection of Fluent bundles.
/// * `cache` - The static cache map.
/// * `key` - The message ID to look up.
/// * `args` - The arguments to interpolate into the message.
pub fn lookup_dynamic<'a, B: BundleCollection + ?Sized, C: CacheStore + ?Sized>(
    bundles: &'a B,
    cache: &C,
    key: &'a str,
    args: &FluentArgs,
) -> Cow<'a, str> {
    lookup_core(bundles, cache, &get_lang(), key, Some(args))
}

/// Like [`lookup_static`], but for `lang` instead of the global language.
///
/// For servers and tools rendering for several users at once: build one
/// [`LocaleState`] per user and pass it here (or to [`t_in!`]).
pub fn lookup_static_in<'a, B: BundleCollection + ?Sized, C: CacheStore + ?Sized>(
    lang: &LocaleState,
    bundles: &'a B,
    cache: &C,
    key: &'a str,
) -> Cow<'a, str> {
    lookup_core(bundles, cache, lang, key, None)
}

/// Like [`lookup_dynamic`], but for `lang` instead of the global language.
pub fn lookup_dynamic_in<'a, B: BundleCollection + ?Sized, C: CacheStore + ?Sized>(
    lang: &LocaleState,
    bundles: &'a B,
    cache: &C,
    key: &'a str,
    args: &FluentArgs,
) -> Cow<'a, str> {
    lookup_core(bundles, cache, lang, key, Some(args))
}

/// The primary accessor macro for localized strings.
///
/// It delegates to `lookup_static` or `lookup_dynamic` depending on whether arguments
/// are provided, using the crate's generated `CATALOG` (which carries its
/// configured fallback locale and its locale negotiation aliases).
///
/// # Examples
///
/// Basic usage:
/// ```rust,ignore
/// let title = t!("app-title");
/// ```
///
/// With arguments:
/// ```rust,ignore
/// let welcome = t!("welcome-user", {
///     "name" => "Alice",
///     "unread_count" => 5
/// });
/// ```
#[allow(clippy::crate_in_macro_def)]
#[macro_export]
macro_rules! t {
    ($key:expr) => {
        $crate::lookup_static(
            &crate::CATALOG,
            &crate::CATALOG,
            $key
        )
    };
    ($key:expr, { $($k:expr => $v:expr),* $(,)? }) => {
        {
            let mut args = $crate::FluentArgs::new();
            $( args.set($k, $v); )*
            $crate::lookup_dynamic(
                &crate::CATALOG,
                &crate::CATALOG,
                $key,
                &args
            )
        }
    };
}

/// Like [`t!`], but for an explicit [`LocaleState`] instead of the global
/// language.
///
/// # Examples
///
/// ```rust,ignore
/// let lang = fluent_zero::LocaleState::new("pt-BR".parse()?);
/// let title = t_in!(&lang, "app-title");
/// let welcome = t_in!(&lang, "welcome-user", { "name" => "Alice" });
/// ```
#[allow(clippy::crate_in_macro_def)]
#[macro_export]
macro_rules! t_in {
    ($lang:expr, $key:expr) => {
        $crate::lookup_static_in(
            $lang,
            &crate::CATALOG,
            &crate::CATALOG,
            $key
        )
    };
    ($lang:expr, $key:expr, { $($k:expr => $v:expr),* $(,)? }) => {
        {
            let mut args = $crate::FluentArgs::new();
            $( args.set($k, $v); )*
            $crate::lookup_dynamic_in(
                $lang,
                &crate::CATALOG,
                &crate::CATALOG,
                $key,
                &args
            )
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lang(tag: &str) -> LocaleState {
        LocaleState::new(tag.parse().unwrap())
    }

    fn chain(tag: &str) -> Vec<String> {
        lang(tag).chain().map(str::to_owned).collect()
    }

    #[test]
    fn chain_drops_variants_script_and_region_in_turn() {
        assert_eq!(chain("en-US"), ["en-US", "en"]);
        assert_eq!(chain("ru"), ["ru"]);
        assert_eq!(
            chain("sr-Latn-RS"),
            ["sr-Latn-RS", "sr-Latn", "sr-RS", "sr"]
        );
        assert_eq!(chain("zh-Hant"), ["zh-Hant", "zh"]);
        assert_eq!(chain("de-DE-1996"), ["de-DE-1996", "de-DE", "de"]);
    }

    fn bundle(lang: &str, source: &str) -> ConcurrentFluentBundle<FluentResource> {
        let mut bundle = ConcurrentFluentBundle::new_concurrent(vec![lang.parse().unwrap()]);
        bundle.set_use_isolating(false);
        bundle
            .add_resource(FluentResource::try_new(source.to_string()).unwrap())
            .unwrap();
        bundle
    }

    static CACHE_EN_GB: phf::Map<&'static str, CacheEntry> = phf::phf_map! {
        "colour" => CacheEntry::Static("Colour"),
    };
    static BUNDLE_EN_GB: LazyLock<ConcurrentFluentBundle<FluentResource>> =
        LazyLock::new(|| bundle("en-GB", "colour = Colour"));
    static CACHE_PT_PT: phf::Map<&'static str, CacheEntry> = phf::phf_map! {
        "hello" => CacheEntry::Static("Olá"),
        "greet" => CacheEntry::Dynamic,
    };
    static BUNDLE_PT_PT: LazyLock<ConcurrentFluentBundle<FluentResource>> =
        LazyLock::new(|| bundle("pt-PT", "hello = Olá\ngreet = Olá { $name }"));

    /// What `fluent-zero-build` generates for `en-GB/` and `pt-PT/` with
    /// `.fallback_locale("en-GB")`: `en` and `pt` alias their only locale.
    static CATALOG_LOCALES: phf::Map<&'static str, CatalogLocale> = phf::phf_map! {
        "en-GB" => (&CACHE_EN_GB, &BUNDLE_EN_GB),
        "en" => (&CACHE_EN_GB, &BUNDLE_EN_GB),
        "pt-PT" => (&CACHE_PT_PT, &BUNDLE_PT_PT),
        "pt" => (&CACHE_PT_PT, &BUNDLE_PT_PT),
    };
    static CATALOG: Catalog = Catalog::new("en-GB", &CATALOG_LOCALES);

    fn t_in(tag: &str, key: &'static str) -> Cow<'static, str> {
        lookup_static_in(&lang(tag), &CATALOG, &CATALOG, key)
    }

    #[test]
    fn region_variant_finds_the_language_locale() {
        // `pt-BR` -> `pt` (an alias of `pt-PT`), still zero-copy.
        let hello = t_in("pt-BR", "hello");
        assert_eq!(hello, "Olá");
        assert!(matches!(hello, Cow::Borrowed(_)));
        assert_eq!(t_in("pt", "hello"), "Olá");
    }

    #[test]
    fn dynamic_messages_resolve_through_aliases() {
        let mut args = FluentArgs::new();
        args.set("name", "Ana");
        let greet = lookup_dynamic_in(&lang("pt-BR"), &CATALOG, &CATALOG, "greet", &args);
        assert_eq!(greet, "Olá Ana");
    }

    #[test]
    fn configured_fallback_replaces_en_us() {
        assert_eq!(CATALOG.fallback(), "en-GB");
        assert_eq!(t_in("pt-BR", "colour"), "Colour");
        assert_eq!(t_in("de-DE", "colour"), "Colour");
        assert_eq!(t_in("de-DE", "missing"), "missing");
    }

    #[test]
    fn stores_without_a_fallback_use_the_default() {
        struct Empty;
        impl CacheStore for Empty {
            fn get_entry(&self, _: &str, _: &str) -> Option<CacheEntry> {
                None
            }
        }
        let store: HashMap<String, ConcurrentFluentBundle<FluentResource>> = HashMap::new();
        assert_eq!(Empty.fallback_locale(), DEFAULT_FALLBACK);
        assert_eq!(lookup_static_in(&lang("fr"), &store, &Empty, "k"), "k");
    }
}
