// fluent-zero/src/lib.rs
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

// =========================================================================
// 1. UNIFIED CACHE TYPES
// =========================================================================

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

// =========================================================================
// 2. GLOBAL STATE
// =========================================================================

/// Internal state holding the currently active language configuration.
pub struct LocaleState {
    /// The parsed identifier (e.g., `en-US`).
    id: LanguageIdentifier,
    /// The string representation used for cache keys (e.g., "en-US").
    key: String,
}

/// The global thread-safe storage for the current language.
///
/// Uses `ArcSwap` to allow lock-free reads, which is critical for high-performance
/// hot paths in GUI rendering loops.
static CURRENT_LANG: LazyLock<ArcSwap<LocaleState>> = LazyLock::new(|| {
    let id: LanguageIdentifier = "en-US".parse().unwrap();
    ArcSwap::from_pointee(LocaleState {
        key: id.to_string(),
        id,
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
    let new_state = LocaleState { id: lang, key };
    CURRENT_LANG.store(Arc::new(new_state));
}

/// Retrieves the current language state.
///
/// Returns a guard containing the `Arc<LocaleState>`. This is primarily used
/// internally by the lookup functions but is exposed for diagnostics.
pub fn get_lang() -> arc_swap::Guard<std::sync::Arc<LocaleState>> {
    CURRENT_LANG.load()
}

// =========================================================================
// 3. TRAIT ABSTRACTIONS
// =========================================================================

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

// =========================================================================
// 4. LOOKUP HELPER (STATIC)
// =========================================================================

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
/// * `key` - The message ID to look up.
pub fn lookup_static<'a, B: BundleCollection + ?Sized, C: CacheStore + ?Sized>(
    bundles: &'a B,
    cache: &C,
    key: &'a str,
) -> Cow<'a, str> {
    let current_key = &get_lang().key;
    let is_fallback = current_key == FALLBACK_LANG_KEY;

    // --- STEP 1: CURRENT LANGUAGE ---
    if let Some(entry) = cache.get_entry(current_key, key) {
        match entry {
            CacheEntry::Static(s) => return Cow::Borrowed(s),
            CacheEntry::Dynamic => {
                if let Some(b) = bundles.get_bundle(current_key)
                    && let Some(val) = lookup_in_bundle(b, key)
                {
                    return val;
                }
            }
        }
    }

    // --- STEP 2: FALLBACK LANGUAGE ---
    if !is_fallback && let Some(entry) = cache.get_entry(FALLBACK_LANG_KEY, key) {
        match entry {
            CacheEntry::Static(s) => return Cow::Borrowed(s),
            CacheEntry::Dynamic => {
                if let Some(b) = bundles.get_bundle(FALLBACK_LANG_KEY)
                    && let Some(val) = lookup_in_bundle(b, key)
                {
                    return val;
                }
            }
        }
    }

    Cow::Borrowed(key)
}

// =========================================================================
// 5. LOOKUP DYNAMIC
// =========================================================================

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
    let current_key = &get_lang().key;
    let is_fallback = current_key == FALLBACK_LANG_KEY;

    // --- STEP 1: CURRENT LANGUAGE ---
    if let Some(entry) = cache.get_entry(current_key, key) {
        match entry {
            // Even if args are provided, if it's static, ignore args and return static string (Zero alloc)
            CacheEntry::Static(s) => return Cow::Borrowed(s),
            CacheEntry::Dynamic => {
                if let Some(b) = bundles.get_bundle(current_key)
                    && let Some(val) = format_in_bundle(b, key, args)
                {
                    return val;
                }
            }
        }
    }

    // --- STEP 2: FALLBACK LANGUAGE ---
    if !is_fallback && let Some(entry) = cache.get_entry(FALLBACK_LANG_KEY, key) {
        match entry {
            CacheEntry::Static(s) => return Cow::Borrowed(s),
            CacheEntry::Dynamic => {
                if let Some(b) = bundles.get_bundle(FALLBACK_LANG_KEY)
                    && let Some(val) = format_in_bundle(b, key, args)
                {
                    return val;
                }
            }
        }
    }

    Cow::Borrowed(key)
}

fn lookup_in_bundle<'a>(
    bundle: &'a ConcurrentFluentBundle<FluentResource>,
    key: &str,
) -> Option<Cow<'a, str>> {
    let msg = bundle.get_message(key)?;
    let pattern = msg.value()?;
    let mut errors = vec![];
    Some(bundle.format_pattern(pattern, None, &mut errors))
}

fn format_in_bundle<'a>(
    bundle: &'a ConcurrentFluentBundle<FluentResource>,
    key: &str,
    args: &FluentArgs,
) -> Option<Cow<'a, str>> {
    let msg = bundle.get_message(key)?;
    let pattern = msg.value()?;
    let mut errors = vec![];
    Some(bundle.format_pattern(pattern, Some(args), &mut errors))
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
