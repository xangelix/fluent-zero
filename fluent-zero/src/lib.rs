//! # fluent-zero
//!
//! A zero-allocation, high-performance Fluent localization loader designed for
//! GUIs and games.
//!
//! This crate works in tandem with the `fluent-zero-build` crate. The build script
//! generates static Perfect Hash Maps (PHF) that allow `O(1)` lookups for localized
//! strings. When a string is static (contains no variables), it returns a `&'static str`
//! reference to the binary's read-only data, avoiding all heap allocations.
//!
//! The public API is [`t!`], [`set_lang`], [`get_lang`], [`LocaleState`],
//! [`CacheEntry`], [`join_charsets`] and the re-exported [`LanguageIdentifier`]
//! and [`FluentArgs`]. Everything under `__private` exists only for the code
//! `fluent-zero-build` generates and the [`t!`] macro; it is not covered by
//! semver and may change in any release.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

extern crate self as fluent_zero;

use std::{
    borrow::Cow,
    sync::{Arc, LazyLock},
};

use arc_swap::ArcSwap;

pub use fluent_bundle::FluentArgs;
pub use unic_langid::LanguageIdentifier;

use crate::__private::{BundleCollection, CacheStore};

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

/// The currently active language, as returned by [`get_lang`].
#[derive(Debug)]
pub struct LocaleState {
    /// The parsed identifier (e.g., `en-US`).
    id: LanguageIdentifier,
    /// The string representation used for cache keys (e.g., "en-US").
    key: String,
}

impl LocaleState {
    fn new(id: LanguageIdentifier) -> Self {
        Self {
            key: id.to_string(),
            id,
        }
    }

    /// The language identifier passed to [`set_lang`] (`en-US` by default).
    #[must_use]
    pub fn id(&self) -> &LanguageIdentifier {
        &self.id
    }

    /// The canonical string form of [`id`](Self::id) (e.g. `"en-US"`), which is
    /// the key the generated caches are indexed by.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }
}

/// The global thread-safe storage for the current language.
///
/// Uses `ArcSwap` to allow lock-free reads, which is critical for high-performance
/// hot paths in GUI rendering loops.
static CURRENT_LANG: LazyLock<ArcSwap<LocaleState>> = LazyLock::new(|| {
    let id: LanguageIdentifier = "en-US".parse().unwrap();
    ArcSwap::from_pointee(LocaleState::new(id))
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
    CURRENT_LANG.store(Arc::new(LocaleState::new(lang)));
}

/// Retrieves the current language state.
///
/// The returned `Arc` is a snapshot: a later [`set_lang`] does not change it.
/// Lookups made by [`t!`] read the language directly and do not go through
/// this function.
#[must_use]
pub fn get_lang() -> Arc<LocaleState> {
    CURRENT_LANG.load_full()
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

/// Abstracted O(1) cascade matching the runtime environment language against established caches.
#[inline]
fn lookup_core<'a, B: BundleCollection + ?Sized, C: CacheStore + ?Sized>(
    bundles: &'a B,
    cache: &C,
    key: &'a str,
    args: Option<&FluentArgs>,
) -> Cow<'a, str> {
    // A borrowed guard, not `get_lang()`: avoids a refcount round trip per lookup.
    let lang_guard = CURRENT_LANG.load();
    let current_key = lang_guard.key();

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

/// The primary accessor macro for localized strings.
///
/// It expects the `CACHE` and `LOCALES` statics generated by `fluent-zero-build` to
/// be included at the root of the calling crate. Without arguments it returns
/// `Cow::Borrowed` for static messages; with arguments the message is formatted
/// with them (static messages still return without allocating).
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
        $crate::__private::lookup_static(
            &crate::LOCALES,
            &crate::CACHE,
            $key
        )
    };
    ($key:expr, { $($k:expr => $v:expr),* $(,)? }) => {
        {
            let mut args = $crate::FluentArgs::new();
            $( args.set($k, $v); )*
            $crate::__private::lookup_dynamic(
                &crate::LOCALES,
                &crate::CACHE,
                $key,
                &args
            )
        }
    };
}

/// Support for the macros and the code generated by `fluent-zero-build`.
///
/// Not part of the public API: nothing here is covered by semver.
#[doc(hidden)]
pub mod __private {
    use std::{borrow::Cow, collections::HashMap, hash::BuildHasher, sync::LazyLock};

    pub use fluent_bundle::{FluentResource, concurrent::FluentBundle as ConcurrentFluentBundle};
    pub use phf;

    use crate::{CacheEntry, FluentArgs};

    /// The contract version between generated code and this runtime.
    ///
    /// `fluent-zero-build` embeds the version it generates for into its output as
    /// `const _: () = <crate>::__private::assert_codegen_version(N);`, so a
    /// mismatched pair of crates fails to compile with a readable message instead
    /// of an unresolved path. It is a plain counter, independent of the crates'
    /// semver versions: bump it (in both crates, to the same value) whenever the
    /// generated code starts relying on anything in this module that older
    /// runtimes lack, or stops being accepted by newer ones.
    pub const CODEGEN_VERSION: u32 = 1;

    /// Fails const evaluation (a compile error in the calling crate) unless
    /// `generated` equals [`CODEGEN_VERSION`].
    ///
    /// # Panics
    ///
    /// Panics when the versions differ; in the generated `const` that is a
    /// compile-time error, never a runtime panic.
    pub const fn assert_codegen_version(generated: u32) {
        if generated > CODEGEN_VERSION {
            panic!(
                "fluent-zero is older than the fluent-zero-build that generated this code: \
                 upgrade fluent-zero to the release matching fluent-zero-build"
            );
        } else if generated < CODEGEN_VERSION {
            panic!(
                "fluent-zero-build is older than the fluent-zero runtime it generated code for: \
                 upgrade fluent-zero-build to the release matching fluent-zero"
            );
        }
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
        super::lookup_core(bundles, cache, key, None)
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
        super::lookup_core(bundles, cache, key, Some(args))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_state_exposes_id_and_key() {
        set_lang("fr-FR".parse().unwrap());
        let state = get_lang();
        assert_eq!(state.id(), &"fr-FR".parse::<LanguageIdentifier>().unwrap());
        assert_eq!(state.key(), "fr-FR");

        // The returned Arc is a snapshot that outlives later switches.
        set_lang("de".parse().unwrap());
        assert_eq!(state.key(), "fr-FR");
        assert_eq!(get_lang().key(), "de");
    }

    #[test]
    fn matching_codegen_version_is_accepted() {
        const _: () = __private::assert_codegen_version(__private::CODEGEN_VERSION);
    }

    #[test]
    #[should_panic(expected = "upgrade fluent-zero to the release matching fluent-zero-build")]
    fn newer_codegen_version_is_rejected() {
        __private::assert_codegen_version(__private::CODEGEN_VERSION + 1);
    }

    #[test]
    #[should_panic(expected = "upgrade fluent-zero-build to the release matching fluent-zero")]
    fn older_codegen_version_is_rejected() {
        __private::assert_codegen_version(__private::CODEGEN_VERSION - 1);
    }
}
