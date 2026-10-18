//! Fixture crate: compiles the code `fluent-zero-build` generates for
//! `locales/` so the tests can exercise it exactly as a consumer would.

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
