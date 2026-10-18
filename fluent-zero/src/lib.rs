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

use arc_swap::{ArcSwap, ArcSwapOption};

pub use fluent_bundle::{
    FluentArgs, FluentError, FluentResource, concurrent::FluentBundle as ConcurrentFluentBundle,
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

/// A localization problem observed at runtime.
///
/// Lookups never fail: a missing key renders as the key itself and a broken
/// placeable renders as best `fluent-bundle` can. These errors are only
/// delivered to the handler installed with [`set_error_handler`] (and logged
/// with the optional `tracing` feature) so QA and telemetry can see them.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum LookupError<'a> {
    /// The key resolved in neither the current nor the fallback language, so
    /// the key itself was returned. For a `message.attribute` key this also
    /// covers a message that exists but lacks that attribute.
    MissingKey {
        /// The message ID, or `message.attribute` key, that was looked up.
        key: &'a str,
        /// The language that was active (e.g. "fr-FR").
        lang: &'a str,
    },
    /// The message exists in `lang` but has no value (only attributes), so the
    /// bare message ID cannot be rendered; look up `message.attribute` instead.
    /// The lookup then continues with the fallback language.
    MissingValue {
        /// The message ID that was looked up.
        key: &'a str,
        /// The language whose bundle holds the value-less message.
        lang: &'a str,
    },
    /// Formatting succeeded only partially, typically because of a missing or
    /// mistyped argument. The partially formatted string was still returned.
    Format {
        /// The message ID that was looked up.
        key: &'a str,
        /// The language whose bundle formatted the message.
        lang: &'a str,
        /// The errors reported by `fluent-bundle`.
        errors: &'a [FluentError],
    },
}

impl std::fmt::Display for LookupError<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingKey { key, lang } => {
                // Message IDs cannot contain `.`, so a dotted key is an attribute.
                let what = if key.contains('.') {
                    "attribute"
                } else {
                    "message"
                };
                write!(f, "missing {what} `{key}` (language {lang})")
            }
            Self::MissingValue { key, lang } => {
                write!(f, "message `{key}` has no value (language {lang})")
            }
            Self::Format { key, lang, errors } => {
                write!(f, "failed to format message `{key}` (language {lang})")?;
                for (i, error) in errors.iter().enumerate() {
                    f.write_str(if i == 0 { ": " } else { "; " })?;
                    write!(f, "{error}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for LookupError<'_> {}

/// Signature of a handler installed with [`set_error_handler`].
pub type ErrorHandler = fn(&LookupError<'_>);

/// The installed error handler, if any. Only read on error paths, so an unset
/// handler costs nothing on successful lookups.
static ERROR_HANDLER: ArcSwapOption<ErrorHandler> = ArcSwapOption::const_empty();

/// Installs a process-wide handler that is called for every [`LookupError`].
///
/// The handler runs synchronously on the thread doing the lookup, so keep it
/// cheap (e.g. log or bump a counter). Replaces any previous handler.
///
/// # Example
///
/// ```rust
/// fluent_zero::set_error_handler(|error| eprintln!("i18n: {error}"));
/// ```
pub fn set_error_handler(handler: ErrorHandler) {
    ERROR_HANDLER.store(Some(Arc::new(handler)));
}

/// Removes the handler installed with [`set_error_handler`].
pub fn clear_error_handler() {
    ERROR_HANDLER.store(None);
}

/// Delivers `error` to the installed handler and, with the `tracing` feature,
/// to `tracing`. Kept out of line so the lookup functions stay small.
#[cold]
#[inline(never)]
fn report(error: &LookupError<'_>) {
    #[cfg(feature = "tracing")]
    tracing::warn!(target: "fluent_zero", "{error}");
    if let Some(handler) = &*ERROR_HANDLER.load() {
        handler(error);
    }
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
                None => {
                    let msg = bundle.get_message(key)?;
                    let Some(pattern) = msg.value() else {
                        report(&LookupError::MissingValue { key, lang });
                        return None;
                    };
                    pattern
                }
                Some((id, attribute)) => bundle.get_message(id)?.get_attribute(attribute)?.value(),
            };
            let mut errors = vec![];
            let value = bundle.format_pattern(pattern, args, &mut errors);
            if !errors.is_empty() {
                report(&LookupError::Format {
                    key,
                    lang,
                    errors: &errors,
                });
            }
            Some(value)
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
    report(&LookupError::MissingKey {
        key,
        lang: current_key,
    });
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
/// 3. **Missing Key**: Returns the `key` itself wrapped in `Cow::Borrowed` and
///    reports a [`LookupError::MissingKey`] to the [error handler](set_error_handler).
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

/// Returns whether `key` is in `sorted_keys`, which must be sorted by byte
/// order (as the generated `__FLUENT_ZERO_KEYS` is).
///
/// Support for `t!`: it runs in a `const` block, so a binary search keeps the
/// compile-time cost of each `t!` call logarithmic in the number of keys.
#[doc(hidden)]
#[must_use]
pub const fn __key_exists(sorted_keys: &[&str], key: &str) -> bool {
    let (mut low, mut high) = (0, sorted_keys.len());
    while low < high {
        let mid = low + (high - low) / 2;
        match const_str_cmp(sorted_keys[mid], key) {
            std::cmp::Ordering::Equal => return true,
            std::cmp::Ordering::Less => low = mid + 1,
            std::cmp::Ordering::Greater => high = mid,
        }
    }
    false
}

/// `Ord for str` (byte-wise lexicographic), usable in `const` contexts.
const fn const_str_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut i = 0;
    while i < a.len() && i < b.len() {
        if a[i] != b[i] {
            return if a[i] < b[i] {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        i += 1;
    }
    if a.len() < b.len() {
        std::cmp::Ordering::Less
    } else if a.len() > b.len() {
        std::cmp::Ordering::Greater
    } else {
        std::cmp::Ordering::Equal
    }
}

/// The primary accessor macro for localized strings.
///
/// It delegates to `lookup_static` or `lookup_dynamic` depending on whether arguments
/// are provided.
///
/// # Compile-time key checking
///
/// A string literal key is checked at compile time against every message ID
/// the build script found in any locale, so a typo such as `t!("app-titel")`
/// fails the build instead of showing the raw key on screen. The check runs in
/// a `const` item and adds nothing at run time. Keys given as any other
/// expression (a variable, a constant such as the generated `keys::APP_TITLE`)
/// are not checked by the macro; the generated `keys::` constants are checked
/// by the compiler simply by existing. To pass a key that is deliberately not
/// defined in any locale, bind it to a variable or constant first.
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
    ($key:literal) => {{
        const _: () = ::core::assert!(
            $crate::__key_exists(crate::__FLUENT_ZERO_KEYS, $key),
            "{}",
            ::core::concat!("unknown Fluent message key passed to `t!`: ", $key)
        );
        $crate::lookup_static(&crate::LOCALES, &crate::CACHE, $key)
    }};
    ($key:literal, { $($k:expr => $v:expr),* $(,)? }) => {{
        const _: () = ::core::assert!(
            $crate::__key_exists(crate::__FLUENT_ZERO_KEYS, $key),
            "{}",
            ::core::concat!("unknown Fluent message key passed to `t!`: ", $key)
        );
        let mut args = $crate::FluentArgs::new();
        $( args.set($k, $v); )*
        $crate::lookup_dynamic(&crate::LOCALES, &crate::CACHE, $key, &args)
    }};
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
