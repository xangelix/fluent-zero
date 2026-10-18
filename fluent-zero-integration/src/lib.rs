//! Fixture crate: compiles the code `fluent-zero-build` generates for
//! `locales/` so the tests can exercise it exactly as a consumer would.

// The generated PHF tables carry long hash literals.
#![allow(clippy::unreadable_literal)]

include!(concat!(env!("OUT_DIR"), "/static_cache.rs"));
