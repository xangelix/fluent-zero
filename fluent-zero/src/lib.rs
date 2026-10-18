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
//! The public API is [`t!`], [`t_in!`], [`set_lang`], [`get_lang`],
//! [`LocaleState`], [`Catalog`], [`CacheEntry`], [`DEFAULT_FALLBACK`],
//! [`join_charsets`], the runtime diagnostics ([`set_error_handler`],
//! [`clear_error_handler`], [`LookupError`], [`FormatErrors`],
//! [`ErrorHandler`]) and the re-exported [`LanguageIdentifier`] and
//! [`FluentArgs`]. Everything under `__private` exists only for the code
//! `fluent-zero-build` generates and the macros; it is not covered by semver
//! and may change in any release.
//!
//! # Example
//!
//! ```rust,standalone_crate
//! # use std::sync::LazyLock;
//! # use fluent_zero::{
//! #     CacheEntry,
//! #     __private::{BundleCollection, CacheStore, ConcurrentFluentBundle, FluentResource},
//! # };
//! # // Hand-written stand-ins for the `CATALOG` static and the key list that
//! # // `fluent-zero-build` generates.
//! # struct Catalog;
//! # impl CacheStore for Catalog {
//! #     fn get_entry(&self, lang: &str, key: &str) -> Option<CacheEntry> {
//! #         match (lang, key) {
//! #             ("en-US", "app-title") => Some(CacheEntry::Static("My App")),
//! #             ("fr-FR", "app-title") => Some(CacheEntry::Static("Mon appli")),
//! #             ("en-US", "welcome-user") => Some(CacheEntry::Dynamic),
//! #             _ => None,
//! #         }
//! #     }
//! # }
//! # static EN_US: LazyLock<ConcurrentFluentBundle<FluentResource>> = LazyLock::new(|| {
//! #     let mut bundle = ConcurrentFluentBundle::new_concurrent(vec!["en-US".parse().unwrap()]);
//! #     bundle.set_use_isolating(false);
//! #     let ftl = "welcome-user = Welcome, { $name }! ({ $unread_count } unread)";
//! #     bundle.add_resource(FluentResource::try_new(ftl.into()).unwrap()).unwrap();
//! #     bundle
//! # });
//! # impl BundleCollection for Catalog {
//! #     fn get_bundle(&self, lang: &str) -> Option<&ConcurrentFluentBundle<FluentResource>> {
//! #         (lang == "en-US").then(|| &*EN_US)
//! #     }
//! # }
//! # static CATALOG: Catalog = Catalog;
//! # const __FLUENT_ZERO_KEYS: &[&str] = &["app-title", "welcome-user"];
//! // Brings the generated `CATALOG`, `keys` and key list into scope.
//! // (Replaced by hand-written stand-ins in this example.)
//! # /*
//! include!(concat!(env!("OUT_DIR"), "/static_cache.rs"));
//! # */
//!
//! use fluent_zero::{set_lang, t};
//!
//! fn main() {
//!     // Static message: a borrowed `&'static str`, no allocation.
//!     assert_eq!(t!("app-title"), "My App");
//!
//!     // Dynamic message: formatted by the Fluent bundle.
//!     let welcome = t!("welcome-user", { "name" => "Alice", "unread_count" => 5 });
//!     assert_eq!(welcome, "Welcome, Alice! (5 unread)");
//!
//!     // Switching language is global and takes effect immediately.
//!     set_lang("fr-FR".parse().unwrap());
//!     assert_eq!(t!("app-title"), "Mon appli");
//!     assert_eq!(fluent_zero::get_lang().key(), "fr-FR");
//!
//!     // Messages missing from fr-FR fall back to en-US.
//!     let welcome = t!("welcome-user", { "name" => "Bob", "unread_count" => 0 });
//!     assert_eq!(welcome, "Welcome, Bob! (0 unread)");
//! }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

extern crate self as fluent_zero;

use std::{
    borrow::Cow,
    sync::{Arc, LazyLock},
};

use arc_swap::{ArcSwap, ArcSwapOption};
use fluent_bundle::FluentError;

pub use fluent_bundle::FluentArgs;
pub use unic_langid::LanguageIdentifier;

use crate::__private::{BundleCollection, CacheStore, CatalogLocale};

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
    /// This indicates that the system must load the Fluent bundle to resolve
    /// the final string.
    Dynamic,
}

/// A language and the locale keys a lookup tries for it.
///
/// The global language set by [`set_lang`] is one of these (see [`get_lang`]),
/// and servers that render for several users at once can build one per user
/// (or request) and pass it to [`t_in!`].
///
/// Building one resolves the fallback chain once (a few small allocations), so
/// every lookup afterwards just walks a short precomputed slice of keys.
#[derive(Debug)]
pub struct LocaleState {
    /// The parsed identifier (e.g., `en-US`).
    id: LanguageIdentifier,
    /// The string representation used for cache keys (e.g., "en-US").
    key: String,
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
            id,
            key,
            chain: chain.into_boxed_slice(),
        }
    }

    /// The language identifier this state was built from (for the global
    /// language, the one passed to [`set_lang`]; `en-US` by default).
    #[must_use]
    pub fn id(&self) -> &LanguageIdentifier {
        &self.id
    }

    /// The canonical string form of [`id`](Self::id) (e.g. `"en-US"`), which is
    /// the first key the generated caches are probed with.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The locale keys a lookup tries, most specific first, before the
    /// catalog's fallback locale.
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
    // `DEFAULT_FALLBACK` is a valid tag (see the unit test), so the default
    // is unreachable; it only keeps a panic path out of the runtime.
    let id: LanguageIdentifier = DEFAULT_FALLBACK.parse().unwrap_or_default();
    ArcSwap::from_pointee(LocaleState::new(id))
});

/// The fallback locale of catalogs that do not configure one.
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
/// The returned `Arc` is a snapshot: a later [`set_lang`] does not change it.
/// Lookups made by [`t!`] read the language directly and do not go through
/// this function.
#[must_use]
pub fn get_lang() -> Arc<LocaleState> {
    CURRENT_LANG.load_full()
}

/// A crate's generated lookup tables, as used by [`t!`] and [`t_in!`].
///
/// `fluent-zero-build` generates one as the `CATALOG` static at the root of
/// each crate that includes its output. Besides each locale under its own key,
/// a catalog answers for the language-level keys that end a fallback chain
/// (`ru` when only `ru-RU/` exists, `zh-TW` when only `zh-Hant/` exists),
/// resolved by `fluent-zero-build` against this crate's own locales. Each
/// entry carries both the static cache and the bundle, so a lookup costs one
/// map probe per candidate key.
pub struct Catalog {
    fallback: &'static str,
    locales: &'static __private::phf::Map<&'static str, CatalogLocale>,
}

impl Catalog {
    /// The locale tried when the language's own chain has no message.
    #[must_use]
    pub const fn fallback(&self) -> &'static str {
        self.fallback
    }

    /// Every locale key this catalog answers for, including negotiation
    /// aliases such as `pt` for `pt-PT`, in no particular order.
    pub fn locales(&self) -> impl ExactSizeIterator<Item = &'static str> {
        self.locales.keys().copied()
    }
}

impl std::fmt::Debug for Catalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut locales: Vec<&str> = self.locales().collect();
        locales.sort_unstable();
        f.debug_struct("Catalog")
            .field("fallback", &self.fallback)
            .field("locales", &locales)
            .finish()
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
/// ```rust
/// # mod my_ui_lib { pub const CHARSET: &str = "cab"; }
/// # const CHARSET: &str = "abd";
/// // Typically `crate::CHARSET` plus the `CHARSET` of each dependency.
/// let complete_charset = fluent_zero::join_charsets(&[
///     CHARSET,
///     my_ui_lib::CHARSET,
/// ]);
/// assert_eq!(complete_charset, "abcd");
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
        errors: FormatErrors<'a>,
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
                if !errors.is_empty() {
                    write!(f, ": {errors}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for LookupError<'_> {}

/// The errors `fluent-bundle` reported while formatting one message, as carried
/// by [`LookupError::Format`].
///
/// Displays as the individual errors joined with `"; "`. The concrete error
/// type belongs to `fluent-bundle` and is deliberately not exposed, so each
/// error is offered as a [`std::error::Error`] trait object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FormatErrors<'a>(&'a [FluentError]);

impl<'a> FormatErrors<'a> {
    /// The number of errors.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no errors.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterates over the errors, in the order `fluent-bundle` reported them.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &'a (dyn std::error::Error + 'static)> {
        self.0
            .iter()
            .map(|error| error as &(dyn std::error::Error + 'static))
    }
}

impl std::fmt::Display for FormatErrors<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, error) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{error}")?;
        }
        Ok(())
    }
}

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
                    errors: FormatErrors(&errors),
                });
            }
            Some(value)
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
    report(&LookupError::MissingKey {
        key,
        lang: &lang.key,
    });
    Cow::Borrowed(key)
}

/// The primary accessor macro for localized strings.
///
/// It looks the key up in the active language (see [`set_lang`]) using the
/// crate's generated `CATALOG` (which carries its configured fallback locale
/// and its locale negotiation aliases). Without arguments it returns
/// `Cow::Borrowed` for static messages; with arguments the message is
/// formatted with them (static messages still return without allocating).
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
/// The macro resolves `crate::CATALOG` and `crate::__FLUENT_ZERO_KEYS`, so the
/// crate calling it must `include!` the file generated by `fluent-zero-build`
/// at its root.
///
/// # Examples
///
/// ```rust,standalone_crate
/// # use std::sync::LazyLock;
/// # use fluent_zero::{
/// #     CacheEntry,
/// #     __private::{BundleCollection, CacheStore, ConcurrentFluentBundle, FluentResource},
/// # };
/// # // Hand-written stand-ins for the `CATALOG` static and the key list that
/// # // `fluent-zero-build` generates.
/// # struct Catalog;
/// # impl CacheStore for Catalog {
/// #     fn get_entry(&self, lang: &str, key: &str) -> Option<CacheEntry> {
/// #         match (lang, key) {
/// #             ("en-US", "app-title") => Some(CacheEntry::Static("My App")),
/// #             ("fr-FR", "app-title") => Some(CacheEntry::Static("Mon appli")),
/// #             ("en-US", "welcome-user") => Some(CacheEntry::Dynamic),
/// #             ("en-US", "login.tooltip") => Some(CacheEntry::Static("Sign in to your account")),
/// #             _ => None,
/// #         }
/// #     }
/// # }
/// # static EN_US: LazyLock<ConcurrentFluentBundle<FluentResource>> = LazyLock::new(|| {
/// #     let mut bundle = ConcurrentFluentBundle::new_concurrent(vec!["en-US".parse().unwrap()]);
/// #     bundle.set_use_isolating(false);
/// #     let ftl = "welcome-user = Welcome, { $name }! ({ $unread_count } unread)";
/// #     bundle.add_resource(FluentResource::try_new(ftl.into()).unwrap()).unwrap();
/// #     bundle
/// # });
/// # impl BundleCollection for Catalog {
/// #     fn get_bundle(&self, lang: &str) -> Option<&ConcurrentFluentBundle<FluentResource>> {
/// #         (lang == "en-US").then(|| &*EN_US)
/// #     }
/// # }
/// # static CATALOG: Catalog = Catalog;
/// # const __FLUENT_ZERO_KEYS: &[&str] = &["app-title", "login.tooltip", "welcome-user"];
/// use fluent_zero::t;
///
/// # fn main() {
/// // Basic usage:
/// let title = t!("app-title");
/// assert_eq!(title, "My App");
///
/// // With arguments:
/// let welcome = t!("welcome-user", {
///     "name" => "Alice",
///     "unread_count" => 5
/// });
/// assert_eq!(welcome, "Welcome, Alice! (5 unread)");
///
/// // Attributes use the `message.attribute` key:
/// let tooltip = t!("login.tooltip");
/// assert_eq!(tooltip, "Sign in to your account");
///
/// // A missing key returns the key itself. (A literal unknown key would not
/// // compile, so it goes through a variable.)
/// let missing = "no-such-message";
/// assert_eq!(t!(missing), "no-such-message");
/// # }
/// ```
#[allow(clippy::crate_in_macro_def)]
#[macro_export]
macro_rules! t {
    ($key:literal) => {{
        const _: () = ::core::assert!(
            $crate::__private::key_exists(crate::__FLUENT_ZERO_KEYS, $key),
            "{}",
            ::core::concat!("unknown Fluent message key passed to `t!`: ", $key)
        );
        $crate::__private::lookup_static(&crate::CATALOG, &crate::CATALOG, $key)
    }};
    ($key:literal, { $($k:expr => $v:expr),* $(,)? }) => {{
        const _: () = ::core::assert!(
            $crate::__private::key_exists(crate::__FLUENT_ZERO_KEYS, $key),
            "{}",
            ::core::concat!("unknown Fluent message key passed to `t!`: ", $key)
        );
        let mut args = $crate::FluentArgs::new();
        $( args.set($k, $v); )*
        $crate::__private::lookup_dynamic(&crate::CATALOG, &crate::CATALOG, $key, &args)
    }};
    ($key:expr) => {
        $crate::__private::lookup_static(
            &crate::CATALOG,
            &crate::CATALOG,
            $key
        )
    };
    ($key:expr, { $($k:expr => $v:expr),* $(,)? }) => {
        {
            let mut args = $crate::FluentArgs::new();
            $( args.set($k, $v); )*
            $crate::__private::lookup_dynamic(
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
/// String literal keys get the same [compile-time check](t!#compile-time-key-checking)
/// as `t!`, and misses are reported to the [error handler](set_error_handler)
/// with the `LocaleState`'s language.
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
    ($lang:expr, $key:literal) => {{
        const _: () = ::core::assert!(
            $crate::__private::key_exists(crate::__FLUENT_ZERO_KEYS, $key),
            "{}",
            ::core::concat!("unknown Fluent message key passed to `t_in!`: ", $key)
        );
        $crate::__private::lookup_static_in($lang, &crate::CATALOG, &crate::CATALOG, $key)
    }};
    ($lang:expr, $key:literal, { $($k:expr => $v:expr),* $(,)? }) => {{
        const _: () = ::core::assert!(
            $crate::__private::key_exists(crate::__FLUENT_ZERO_KEYS, $key),
            "{}",
            ::core::concat!("unknown Fluent message key passed to `t_in!`: ", $key)
        );
        let mut args = $crate::FluentArgs::new();
        $( args.set($k, $v); )*
        $crate::__private::lookup_dynamic_in($lang, &crate::CATALOG, &crate::CATALOG, $key, &args)
    }};
    ($lang:expr, $key:expr) => {
        $crate::__private::lookup_static_in(
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
            $crate::__private::lookup_dynamic_in(
                $lang,
                &crate::CATALOG,
                &crate::CATALOG,
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

    use crate::{CacheEntry, Catalog, DEFAULT_FALLBACK, FluentArgs, LocaleState};

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

    /// Creates a [`Catalog`]. Called by the code `fluent-zero-build` generates.
    #[must_use]
    pub const fn catalog(
        fallback: &'static str,
        locales: &'static phf::Map<&'static str, CatalogLocale>,
    ) -> Catalog {
        Catalog { fallback, locales }
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

    /// Retrieves a localized message without arguments, in the global language.
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
    /// 3. **Missing Key**: Returns the `key` itself wrapped in `Cow::Borrowed` and
    ///    reports a [`LookupError::MissingKey`](crate::LookupError::MissingKey) to
    ///    the [error handler](crate::set_error_handler).
    pub fn lookup_static<'a, B: BundleCollection + ?Sized, C: CacheStore + ?Sized>(
        bundles: &'a B,
        cache: &C,
        key: &'a str,
    ) -> Cow<'a, str> {
        // A borrowed guard, not `get_lang()`: avoids a refcount round trip per lookup.
        super::lookup_core(bundles, cache, &super::CURRENT_LANG.load(), key, None)
    }

    /// Retrieves a localized message with arguments, in the global language.
    ///
    /// Even when arguments are provided, this function checks if the underlying message
    /// is actually static. If so, it ignores the arguments and returns the static string
    /// to preserve performance.
    pub fn lookup_dynamic<'a, B: BundleCollection + ?Sized, C: CacheStore + ?Sized>(
        bundles: &'a B,
        cache: &C,
        key: &'a str,
        args: &FluentArgs,
    ) -> Cow<'a, str> {
        super::lookup_core(bundles, cache, &super::CURRENT_LANG.load(), key, Some(args))
    }

    /// Like [`lookup_static`], but for `lang` instead of the global language.
    pub fn lookup_static_in<'a, B: BundleCollection + ?Sized, C: CacheStore + ?Sized>(
        lang: &LocaleState,
        bundles: &'a B,
        cache: &C,
        key: &'a str,
    ) -> Cow<'a, str> {
        super::lookup_core(bundles, cache, lang, key, None)
    }

    /// Like [`lookup_dynamic`], but for `lang` instead of the global language.
    pub fn lookup_dynamic_in<'a, B: BundleCollection + ?Sized, C: CacheStore + ?Sized>(
        lang: &LocaleState,
        bundles: &'a B,
        cache: &C,
        key: &'a str,
        args: &FluentArgs,
    ) -> Cow<'a, str> {
        super::lookup_core(bundles, cache, lang, key, Some(args))
    }

    /// Returns whether `key` is in `sorted_keys`, which must be sorted by byte
    /// order (as the generated `__FLUENT_ZERO_KEYS` is).
    ///
    /// Support for `t!`: it runs in a `const` block, so a binary search keeps the
    /// compile-time cost of each `t!` call logarithmic in the number of keys.
    #[must_use]
    pub const fn key_exists(sorted_keys: &[&str], key: &str) -> bool {
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
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        __private::{
            ConcurrentFluentBundle, FluentResource, catalog, lookup_dynamic_in, lookup_static_in,
        },
        *,
    };

    fn lang(tag: &str) -> LocaleState {
        LocaleState::new(tag.parse().unwrap())
    }

    fn chain(tag: &str) -> Vec<String> {
        lang(tag).chain().map(str::to_owned).collect()
    }

    // The only test that touches the global language, so its assertions about
    // the default cannot race with another test's `set_lang`.
    #[test]
    fn global_language_defaults_to_the_fallback_and_snapshots() {
        let id: LanguageIdentifier = DEFAULT_FALLBACK.parse().unwrap();
        assert_eq!(id.to_string(), "en-US");
        assert_eq!(get_lang().key(), DEFAULT_FALLBACK);

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
    static CATALOG: Catalog = catalog("en-GB", &CATALOG_LOCALES);

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
    fn catalog_lists_its_locales_and_aliases() {
        let mut locales: Vec<&str> = CATALOG.locales().collect();
        locales.sort_unstable();
        assert_eq!(locales, ["en", "en-GB", "pt", "pt-PT"]);
        assert_eq!(
            format!("{CATALOG:?}"),
            r#"Catalog { fallback: "en-GB", locales: ["en", "en-GB", "pt", "pt-PT"] }"#
        );
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

    #[test]
    fn format_errors_display_and_iterate_without_exposing_their_type() {
        let mut errors = Vec::new();
        let bundle = bundle("en-US", "m = { $a } { $b }");
        let pattern = bundle.get_message("m").unwrap().value().unwrap();
        bundle.format_pattern(pattern, None, &mut errors);
        let errors = FormatErrors(&errors);
        assert_eq!(errors.len(), 2);
        assert!(!errors.is_empty());
        let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
        assert_eq!(errors.to_string(), messages.join("; "));

        let error = LookupError::Format {
            key: "m",
            lang: "en-US",
            errors,
        };
        assert_eq!(
            error.to_string(),
            format!("failed to format message `m` (language en-US): {errors}")
        );
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
