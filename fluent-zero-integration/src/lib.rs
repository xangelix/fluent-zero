//! Fixture crate: compiles the code `fluent-zero-build` generates for
//! `locales/` so the tests can exercise it exactly as a consumer would.
//!
//! String literal keys passed to `t!` and `t_in!` are checked at compile
//! time. The doctests below pin both sides: a known key compiles, and the same
//! code with a typo fails with `E0080` (the `const` assertion inside the macro).
//!
//! ```
//! use fluent_zero_integration::{__FLUENT_ZERO_KEYS, CATALOG};
//!
//! fn main() {
//!     assert_eq!(fluent_zero::t!("plain"), "Hello");
//!     let _ = fluent_zero::t!("greeting", { "name" => "Bob" });
//!     let lang = fluent_zero::LocaleState::new("fr".parse().unwrap());
//!     assert_eq!(fluent_zero::t_in!(&lang, "plain"), "Bonjour");
//!     let _ = fluent_zero::t_in!(&lang, "greeting", { "name" => "Bob" });
//! }
//! ```
//!
//! ```compile_fail,E0080
//! use fluent_zero_integration::{__FLUENT_ZERO_KEYS, CATALOG};
//!
//! fn main() {
//!     let _ = fluent_zero::t!("plian");
//! }
//! ```
//!
//! ```compile_fail,E0080
//! use fluent_zero_integration::{__FLUENT_ZERO_KEYS, CATALOG};
//!
//! fn main() {
//!     let _ = fluent_zero::t!("greting", { "name" => "Bob" });
//! }
//! ```
//!
//! ```compile_fail,E0080
//! use fluent_zero_integration::{__FLUENT_ZERO_KEYS, CATALOG};
//!
//! fn main() {
//!     let lang = fluent_zero::LocaleState::new("fr".parse().unwrap());
//!     let _ = fluent_zero::t_in!(&lang, "plian");
//! }
//! ```
//!
//! ```compile_fail,E0080
//! use fluent_zero_integration::{__FLUENT_ZERO_KEYS, CATALOG};
//!
//! fn main() {
//!     let lang = fluent_zero::LocaleState::new("fr".parse().unwrap());
//!     let _ = fluent_zero::t_in!(&lang, "greting", { "name" => "Bob" });
//! }
//! ```
//!
//! A mistyped generated constant is an ordinary unresolved-path error:
//!
//! ```compile_fail,E0425
//! let _ = fluent_zero_integration::keys::PLIAN;
//! ```

// The generated PHF tables carry long hash literals.
#![allow(clippy::unreadable_literal)]

// A root `extern crate` alias puts `fz` in the extern prelude exactly like a
// `fz = { package = "fluent-zero" }` rename would (Cargo refuses to list the
// same crate twice under different names).
extern crate fluent_zero as fz;

include!(concat!(env!("OUT_DIR"), "/static_cache.rs"));

/// The same locales generated with `.crate_path("::fz")`, so every path in it
/// goes through the renamed crate.
pub mod renamed {
    include!(concat!(env!("OUT_DIR"), "/static_cache_renamed.rs"));
}

/// The same locales generated with `.use_isolating(false)`.
pub mod no_isolating {
    include!(concat!(env!("OUT_DIR"), "/static_cache_no_isolating.rs"));
}
