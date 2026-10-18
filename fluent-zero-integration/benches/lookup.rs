//! Lookup benchmarks against the real generated cache, through the public
//! `t!` macro (non-literal keys, so `black_box` can hide them).
//!
//! Run with `cargo bench -p fluent-zero-integration`.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use fluent_zero::{set_lang, t};
// `t!` resolves it through `crate::`, as in a consumer crate.
use fluent_zero_integration::CATALOG;

fn lookups(c: &mut Criterion) {
    set_lang("en-US".parse().expect("valid language id"));

    c.bench_function("static", |b| {
        b.iter(|| t!(black_box("plain")));
    });

    let name = "Alice";
    c.bench_function("dynamic_with_args", |b| {
        b.iter(|| t!(black_box("greeting"), { "name" => black_box(name) }));
    });

    c.bench_function("miss", |b| {
        b.iter(|| t!(black_box("no-such-message")));
    });

    // `about` exists only in en-US, so fr-FR resolves it through the fallback.
    set_lang("fr-FR".parse().expect("valid language id"));
    c.bench_function("fallback_static", |b| {
        b.iter(|| t!(black_box("about")));
    });

    set_lang("en-US".parse().expect("valid language id"));
}

criterion_group!(benches, lookups);
criterion_main!(benches);
