//! Lookup benchmarks against the real generated cache.
//!
//! Run with `cargo bench -p fluent-zero-integration`.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use fluent_zero::{FluentArgs, lookup_dynamic, lookup_static, set_lang};
use fluent_zero_integration::{CACHE, LOCALES};

fn lookups(c: &mut Criterion) {
    set_lang("en-US".parse().expect("valid language id"));

    c.bench_function("static", |b| {
        b.iter(|| lookup_static(&LOCALES, &CACHE, black_box("plain")));
    });

    let mut args = FluentArgs::new();
    args.set("name", "Alice");
    c.bench_function("dynamic_with_args", |b| {
        b.iter(|| lookup_dynamic(&LOCALES, &CACHE, black_box("greeting"), black_box(&args)));
    });

    c.bench_function("miss", |b| {
        b.iter(|| lookup_static(&LOCALES, &CACHE, black_box("no-such-message")));
    });

    // `about` exists only in en-US, so fr-FR resolves it through the fallback.
    set_lang("fr-FR".parse().expect("valid language id"));
    c.bench_function("fallback_static", |b| {
        b.iter(|| lookup_static(&LOCALES, &CACHE, black_box("about")));
    });

    set_lang("en-US".parse().expect("valid language id"));
}

criterion_group!(benches, lookups);
criterion_main!(benches);
