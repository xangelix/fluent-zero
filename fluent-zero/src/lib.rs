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

/// Internal state holding the currently active language configuration.
pub struct LocaleState {
    /// The parsed identifier (e.g., `en-US`).
    _id: LanguageIdentifier,
    /// The string representation used for cache keys (e.g., "en-US").
    pub key: String,
}

/// The global thread-safe storage for the current language.
///
/// Uses `ArcSwap` to allow lock-free reads, which is critical for high-performance
/// hot paths in GUI rendering loops.
static CURRENT_LANG: LazyLock<ArcSwap<LocaleState>> = LazyLock::new(|| {
    let id: LanguageIdentifier = "en-US".parse().unwrap();
    ArcSwap::from_pointee(LocaleState {
        key: id.to_string(),
        _id: id,
    })
});

/// Constant fallback key allows for pointer-sized `&str` checks instead of parsing.
static FALLBACK_LANG_KEY: &str = "en-US";

/// Updates the runtime language for the application.
///
/// This operation is atomic. Subsequent calls to `t!` will immediately reflect
/// the new language.
///
/// # Arguments
///
/// * `lang` - The new `LanguageIdentifier` to set (e.g., parsed from "fr-FR").
pub fn set_lang(lang: LanguageIdentifier) {
    let key = lang.to_string();
    let new_state = LocaleState { _id: lang, key };
    CURRENT_LANG.store(Arc::new(new_state));
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
            // Fluent message IDs cannot contain `.`, so a dotted key always
            // names an attribute (`message.attribute`). Only this cold path
            // splits the key; static entries never get here.
            let pattern = match key.split_once('.') {
                None => bundle.get_message(key)?.value()?,
                Some((id, attribute)) => bundle.get_message(id)?.get_attribute(attribute)?.value(),
            };
            let mut errors = vec![];
            Some(bundle.format_pattern(pattern, args, &mut errors))
        }
    }
}

/// Abstracted O(1) cascade matching the runtime environment language against established caches.
#[inline]
fn lookup_core<'a, B: BundleCollection + ?Sized, C: CacheStore + ?Sized>(
    bundles: &'a B,
    cache: &C,
    key: &'a str,
    args: Option<&FluentArgs>,
) -> Cow<'a, str> {
    let lang_guard = get_lang();
    let current_key = &lang_guard.key;

    // 1. Current Language
    if let Some(res) = resolve_entry(bundles, cache, current_key, key, args) {
        return res;
    }

    // 2. Fallback Language
    if current_key != FALLBACK_LANG_KEY
        && let Some(res) = resolve_entry(bundles, cache, FALLBACK_LANG_KEY, key, args)
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
/// 1. **Current Language**: Checks if the key exists in the current language.
/// 2. **Fallback Language**: If missing, checks the `FALLBACK_LANG_KEY` (en-US).
/// 3. **Missing Key**: Returns the `key` itself wrapped in `Cow::Borrowed`.
///
/// # Arguments
///
/// * `bundles` - The collection of Fluent bundles (usually `crate::LOCALES`).
/// * `cache` - The static cache map (usually `crate::CACHE`).
/// * `key` - The message ID to look up, or `message.attribute` for an attribute.
pub fn lookup_static<'a, B: BundleCollection + ?Sized, C: CacheStore + ?Sized>(
    bundles: &'a B,
    cache: &C,
    key: &'a str,
) -> Cow<'a, str> {
    lookup_core(bundles, cache, key, None)
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
/// * `key` - The message ID to look up, or `message.attribute` for an attribute.
/// * `args` - The arguments to interpolate into the message.
pub fn lookup_dynamic<'a, B: BundleCollection + ?Sized, C: CacheStore + ?Sized>(
    bundles: &'a B,
    cache: &C,
    key: &'a str,
    args: &FluentArgs,
) -> Cow<'a, str> {
    lookup_core(bundles, cache, key, Some(args))
}

/// The primary accessor macro for localized strings.
///
/// It delegates to `lookup_static` or `lookup_dynamic` depending on whether arguments
/// are provided.
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
///
/// Attributes use the `message.attribute` key:
/// ```rust,ignore
/// let tooltip = t!("login.tooltip");
/// ```
#[allow(clippy::crate_in_macro_def)]
#[macro_export]
macro_rules! t {
    ($key:expr) => {
        $crate::lookup_static(
            &crate::LOCALES,
            &crate::CACHE,
            $key
        )
    };
    ($key:expr, { $($k:expr => $v:expr),* $(,)? }) => {
        {
            let mut args = $crate::FluentArgs::new();
            $( args.set($k, $v); )*
            $crate::lookup_dynamic(
                &crate::LOCALES,
                &crate::CACHE,
                $key,
                &args
            )
        }
    };
}
