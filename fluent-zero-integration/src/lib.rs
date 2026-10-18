//! Fixture crate: compiles the code `fluent-zero-build` generates for
//! `locales/` so the tests can exercise it exactly as a consumer would.
//!
//! String literal keys passed to `t!` are checked at compile time. The
//! doctests below pin both sides: a known key compiles, and the same code
//! with a typo fails with `E0080` (the `const` assertion inside `t!`).
//!
//! ```
//! use fluent_zero_integration::{__FLUENT_ZERO_KEYS, CACHE, LOCALES};
//!
//! fn main() {
//!     assert_eq!(fluent_zero::t!("plain"), "Hello");
//!     let _ = fluent_zero::t!("greeting", { "name" => "Bob" });
//! }
//! ```
//!
//! ```compile_fail,E0080
//! use fluent_zero_integration::{__FLUENT_ZERO_KEYS, CACHE, LOCALES};
//!
//! fn main() {
//!     let _ = fluent_zero::t!("plian");
//! }
//! ```
//!
//! ```compile_fail,E0080
//! use fluent_zero_integration::{__FLUENT_ZERO_KEYS, CACHE, LOCALES};
//!
//! fn main() {
//!     let _ = fluent_zero::t!("greting", { "name" => "Bob" });
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

include!(concat!(env!("OUT_DIR"), "/static_cache.rs"));
