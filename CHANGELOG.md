# Changelog

## v0.2.0 (unreleased)

**⚠️ Breaking:** keep `fluent-zero` and `fluent-zero-build` on the same version; generated code now checks this at compile time.

- **Public API trimmed:** lookup functions, `CacheStore`, `BundleCollection` and dependency re-exports (`phf`, `fluent_syntax`, `FluentError`, bundle types) moved behind the hidden `__private` module. Use `t!` / `t_in!`. `get_lang()` returns `Arc<LocaleState>`; `LocaleState` has `id()` and `key()`.
- **Fallible builder:** `try_generate()` returns `fluent_zero_build::Error` (parse errors with `path:line:col`, duplicate IDs, missing or invalid fallback locale, denied missing translations, I/O); `generate()` panics with the same message. `crate_path(...)` supports renamed dependencies.
- **Locales:** negotiation with a fallback chain (`set_lang("pt-BR")` finds `pt-PT`), configurable `fallback_locale`, per-call `t_in!`, generated `CATALOG`.
- **Correctness:** charset covers select variants, attributes and terms; quotes in static messages no longer break the build; duplicate IDs are reported with both locations; message attributes via `message.attribute` keys; deterministic generated output.
- **Compile-time checks:** literal keys passed to `t!` / `t_in!` must exist; generated `keys` module with one constant per key.
- **Diagnostics:** `set_error_handler` / `LookupError` (or the `tracing` feature) for missing keys and arguments; translation coverage warnings via `missing_translations`.
- **Options:** `use_isolating(false)` to drop U+2068/U+2069 bidi marks.
- **Packaging:** MIT `LICENSE` shipped in both crates, MSRV 1.88, hardened CI, benchmarks and a zero-allocation test.

## v0.1.5

- **Avoid Redundant Rebuilds:** `fluent-zero-build` now only writes the character set file if its content has actually changed, preserving file modification timestamps and preventing unnecessary rebuilds on every Cargo invocation.
- **Dependency Upgrades:** Upgraded `phf` and `phf_codegen` to `0.14`.
- **CI & Repository Maintenance:** Added GitHub Actions CI workflow for `cargo fmt` and `cargo test`, `.editorconfig`, and funding metadata.

## v0.1.4

**Enterprise Font Subsetting (DAG IPC)**

- **Hermetic & Cache-Safe:** `fluent-zero-build` now natively leverages Cargo's Inter-Package Communication (IPC) to aggregate character sets across all nested dependencies.
- **No More Scraping:** Replaces brittle `cargo_metadata` workspace-walking with a 100% `sccache`, `Bazel`, and `Nix` compatible build pipeline.

**⚠️ Migration & Setup:** Dependency crates must now declare a globally unique `links` key in their `Cargo.toml`. Please see the **Enterprise Font Subsetting (DAG IPC)** section in the [README.md](README.md) for full instructions and a copy-paste Python subsetter script template.

## v0.1.3

New `FluentZeroBuilder` API for generating charset, allowing easy font subsetting.

## v0.1.2

Small, miscellaneous documentation improvements and fixes.

## v0.1.1

Hotfix:

- add shields to the README.md
- link to the source code repository and docs.rs documentation from the Cargo.toml metadata.
