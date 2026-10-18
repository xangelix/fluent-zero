# fluent-zero

[![Crates.io](https://img.shields.io/crates/v/fluent-zero)](https://crates.io/crates/fluent-zero)
[![Docs.rs](https://docs.rs/fluent-zero/badge.svg)](https://docs.rs/fluent-zero)
[![License](https://img.shields.io/crates/l/fluent-zero)](https://spdx.org/licenses/MIT)

**Zero-allocation, high-performance [Fluent](https://projectfluent.org/) localization for Rust.**

`fluent-zero` is a specialized localization loader designed for high-performance applications, such as **GUI clients** ([egui](https://egui.rs/), [iced](https://iced.rs/), [winit](https://github.com/rust-windowing/winit)) and **Game Development** ([Bevy](https://bevy.org/), [Fyrox](https://fyrox.rs/)).

Unlike other loaders that prioritize template engine integration or hot-reloading, `fluent-zero` prioritizes **runtime speed and memory efficiency**. It generates static code at build time to allow for `O(1)` lookups that return `&'static str` whenever possible, eliminating the heap allocation overhead typical of localization libraries.

## ⚡ Why fluent-zero?

Most Fluent implementations (like `fluent-templates`) wrap the standard `fluent-bundle`. When you request a translation, they look it up in a `HashMap`, parse the pattern, and allocate a new `String` on the heap to return the result—even if the text is static.

In an immediate-mode GUI (like `egui`) running at 60 FPS, looking up 50 strings per frame results in **3,000 allocations per second**. This causes allocator contention and Garbage Collection-like micro-stutters.

**`fluent-zero` solves this by pre-computing the cache at compile time.**

| Feature                | `fluent-templates`         | `fluent-zero`                        |
| ---------------------- | -------------------------- | ------------------------------------ |
| **Static Text Lookup** | Heap Allocation (`String`) | **Zero Allocation** (`&'static str`) |
| **Lookup Speed**       | HashMap + AST traversal    | **Perfect Hash Function (PHF)**      |
| **Memory Usage**       | Full AST loaded on start   | **Lazy / Zero-Cost Abstraction**     |
| **Best For**           | Web Servers (Tera/Askama)  | **Desktop GUIs & Games**             |

## 🚀 Usage

### 1. Installation

You need both the runtime library and the build-time code generator.

```toml
[dependencies]
fluent-zero = "0.2"
unic-langid = "0.9"

[build-dependencies]
fluent-zero-build = "0.2"
```

### 2. File Structure

Organize your Fluent files using standard locale directories:

```text
assets/
└── locales/
    ├── en-US/
    │   └── main.ftl
    ├── fr-FR/
    │   └── main.ftl
    └── de/
        └── main.ftl
```

### 3. Build Script (`build.rs`)

Configure the code generator to read your locales directory. This will generate the static PHF maps and Rust code required for the zero-allocation cache inside your `OUT_DIR`.

```rust
fn main() {
    // Generates static_cache.rs in your OUT_DIR
    fluent_zero_build::generate_static_cache("assets/locales");
}
```

For more control, use the builder. `try_generate()` returns a
`Result<(), fluent_zero_build::Error>` (missing `OUT_DIR`, unreadable files, FTL
syntax errors, duplicate message IDs, an invalid or missing fallback locale,
missing translations under `MissingTranslations::Deny`) instead of panicking
like `generate()`:

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    fluent_zero_build::FluentZeroBuilder::new("assets/locales")
        // Only needed if fluent-zero is renamed in Cargo.toml,
        // e.g. `fz = { package = "fluent-zero", version = "..." }`.
        .crate_path("::fz")
        .try_generate()?;
    Ok(())
}
```

Keep `fluent-zero` and `fluent-zero-build` on matching releases. The generated
code checks that it was produced for the runtime it is compiled against; a
mismatched pair fails to compile with a message saying which crate to upgrade.

### 4. Application Code

In your `lib.rs` (or `main.rs`), you must include the generated file. This brings the `CATALOG` static (plus the plain `CACHE` and `LOCALES` maps, the `FALLBACK` locale and the key list used for [compile-time key checking](#7-compile-time-key-checking)) into scope, which the `t!` macro relies on.

```rust
use fluent_zero::{t, set_lang};

// 1. Include the generated code from build.rs
include!(concat!(env!("OUT_DIR"), "/static_cache.rs"));

fn main() {
    // 2. (Optional) Set the runtime language. Defaults to en-US.
    // The parse() method comes from unic_langid::LanguageIdentifier
    set_lang("fr-FR".parse().expect("Invalid lang ID"));

    // 3. Use the t! macro for lookups.

    // CASE A: Static String
    // Returns &'static str. ZERO ALLOCATION.
    let title = t!("app-title");

    // CASE B: Dynamic String (with variables)
    // Returns Cow<'static, str>. Allocates only if variables are resolved.
    let welcome = t!("welcome-user", {
        "name" => "Alice",
        "unread_count" => 5
    });

    // CASE C: Attributes, addressed as `message.attribute`
    // Static attributes are zero-allocation too; arguments work as in CASE B.
    let tooltip = t!("login.tooltip");

    println!("{}", title);
    println!("{}", welcome);
    println!("{}", tooltip);
}
```

Attributes (`.tooltip`, `.placeholder`, `.aria-label`, ...) are looked up with the conventional `message.attribute` key and follow the same fallback as message values: the current language's negotiated fallback chain, then the fallback locale (`en-US` by default), then the key itself. `t_in!` resolves them the same way against its `LocaleState`. A message with only attributes (no value) works for its attributes:

```fluent
login = Log in
    .tooltip = Sign in to your account
search =
    .placeholder = Search
```

### 5. Locale Negotiation & Fallback

`set_lang` resolves a fallback chain once, so lookups never parse or negotiate. The chain drops variants, script and region in turn (`sr-Latn-RS` → `sr-Latn` → `sr-RS` → `sr`), and the build step maps each language (and script-implying tags such as `zh-TW`) to the best locale **your crate** actually ships, using [`fluent-langneg`](https://crates.io/crates/fluent-langneg) with CLDR likely subtags. So with these directories:

| `set_lang(...)`  | `en-US/`, `fr-FR/`, `pt-PT/`, `zh-Hant/` |
| ---------------- | ---------------------------------------- |
| `fr`, `fr-CA`    | `fr-FR`                                  |
| `pt-BR`          | `pt-PT`                                  |
| `zh-TW`, `zh-HK` | `zh-Hant`                                |
| `de`             | the fallback locale (`en-US`)            |

A message missing from the chosen locale comes from the fallback locale, then the key itself. The fallback defaults to `en-US`; change it in `build.rs` (the build fails if no such locale directory exists, so a typo cannot silently disable it):

```rust
fn main() {
    fluent_zero_build::FluentZeroBuilder::new("assets/locales")
        .fallback_locale("en-GB")
        .generate();
}
```

The generated code exports it as `FALLBACK`.

### 6. Per-Call Locale (servers, multi-user tools)

`t!` reads the global language. To render for several users at once, build a `LocaleState` per user (or request) and use `t_in!`, which bypasses the global language entirely:

```rust
use fluent_zero::{LocaleState, t_in};

let lang = LocaleState::new("pt-BR".parse()?); // resolves the chain once
let title = t_in!(&lang, "app-title");          // still zero-allocation
let welcome = t_in!(&lang, "welcome-user", { "name" => "Alice" });
```

### 7. Compile-Time Key Checking

A mistyped key never reaches your users as a raw key on screen; it fails the build.

**String literals are checked by `t!` and `t_in!`.** Every literal key is checked at compile time against the message IDs and `message.attribute` keys found in *any* locale, on stable Rust and without a proc-macro. Existing `t!("literal")` and `t_in!(&lang, "literal")` calls compile unchanged, and the check costs nothing at run time (it runs in a `const` item: a binary search over a sorted key list the build script generates).

```rust
let title = t!("app-titel");
// error[E0080]: evaluation panicked: unknown Fluent message key passed to `t!`: app-titel
```

Only literals are checked: a key held in a variable or constant is looked up at run time as before. To pass a key that is deliberately missing from every locale, bind it to a variable first.

**Every key is also a constant.** The generated code contains a `keys` module with one `pub const` per message ID and per attribute key (`login.tooltip` becomes `LOGIN_TOOLTIP`), documented with its source in the [fallback locale](#5-locale-negotiation--fallback) (`en-US` unless configured), so your IDE can autocomplete keys and show the text on hover:

```rust
let title = t!(keys::APP_TITLE);
let welcome = t!(keys::WELCOME_USER, { "name" => "Alice" });
let tooltip = t!(keys::LOGIN_TOOLTIP);
```

Constant names are derived deterministically:

- ASCII letters are uppercased and ASCII digits kept; every other character (`-`, `_`, `.`, anything non-ASCII) becomes `_`, so `app-title` becomes `APP_TITLE`.
- A name that would not start with an ASCII letter is prefixed with `KEY_`.
- If several IDs map to the same name (`foo-bar` and `foo_bar`), they are sorted by byte order: the first keeps the plain name and each later one gets `_2`, `_3`, ... (skipping any name that another ID maps to on its own), and the build prints a `cargo:warning` naming the constant it received.

> **Note:** the generated file now defines a `keys` module and a hidden `__FLUENT_ZERO_KEYS` constant next to `CATALOG`, `CACHE` and `LOCALES`, so it cannot be included into a module that already has its own item named `keys`. Keep `fluent-zero` and `fluent-zero-build` on matching releases: the generated code checks that it was produced for the runtime it is compiled against.

### 8. Public API

`fluent_zero::get_lang()` returns an `Arc<LocaleState>` snapshot of the active
language; `.id()` gives the `LanguageIdentifier`, `.key()` its string form
(e.g. `"fr-FR"`) and `.chain()` the locale keys a lookup tries.

The public API of `fluent-zero` is `t!`, `t_in!`, `set_lang`, `get_lang`,
`LocaleState`, `Catalog` (the type of the generated `CATALOG`), `CacheEntry`,
`DEFAULT_FALLBACK`, `join_charsets`, the diagnostics (`set_error_handler`,
`clear_error_handler`, `LookupError`, `FormatErrors`, `ErrorHandler`) and the
re-exported `LanguageIdentifier` and `FluentArgs`. The hidden
`fluent_zero::__private` module exists for the macros and the generated code
and may change in any release.

## 🩺 Diagnostics

### Runtime errors

Lookups never panic: a missing key renders as the key itself, and a missing argument renders as `{$name}`. To see these problems in QA or telemetry, install a handler. It is a plain `fn` pointer, read only when something went wrong, so successful lookups (and the static fast path) cost exactly the same with or without it.

```rust
use fluent_zero::{LookupError, set_error_handler};

set_error_handler(|error: &LookupError| match error {
    LookupError::MissingKey { key, lang } => eprintln!("untranslated {key} ({lang})"),
    other => eprintln!("i18n: {other}"),
});
```

`LookupError` covers a key found in neither the current nor the fallback language (`MissingKey`; for a `message.attribute` key this includes a message that lacks that attribute), a bare lookup of a message that has only attributes (`MissingValue`; look up `message.attribute` instead), and partial formatting such as a missing argument (`Format`, whose `errors` field lists the `fluent-bundle` errors as `std::error::Error`s). A key missing from the current language but found through its fallback chain or in the fallback locale is not an error. Misses through `t_in!` are reported too, with the `LocaleState`'s language. `clear_error_handler()` removes the handler.

Alternatively, enable the off-by-default `tracing` feature to log every `LookupError` with `tracing::warn!` (target `fluent_zero`):

```toml
fluent-zero = { version = "0.2", features = ["tracing"] }
```

### Build errors

`FluentZeroBuilder::try_generate()` returns every build failure as a `fluent_zero_build::Error` (`Parse`, `DuplicateId`, `InvalidFallbackLocale`, `MissingFallbackLocale`, `MissingTranslations`, `Io`, ...); `generate()` panics with the same message, prefixed with `fluent-zero-build: `. An `.ftl` syntax error reports the file, line and column of each error and the offending line:

```text
fluent-zero-build: failed to parse assets/locales/en-US/main.ftl:
assets/locales/en-US/main.ftl:2:8: Expected a token starting with "="
2 | broken Value
  |        ^
```

### Translation coverage

The builder warns, once per locale, about message IDs present in the fallback locale (`en-US` unless configured with `.fallback_locale(...)`) but missing from another locale (those fall back to it at runtime). Coverage is per message: attributes are not counted separately.

```text
warning: my-app@0.1.0: fr-FR is missing 12 of 40 messages from en-US: about, greeting, menu-file, menu-open, title and 7 more
```

Turn this into a build error, or silence it:

```rust
use fluent_zero_build::{FluentZeroBuilder, MissingTranslations};

FluentZeroBuilder::new("assets/locales")
    .missing_translations(MissingTranslations::Deny) // or Allow; default Warn
    .generate();
```

## 📦 Library Support & Nested Translations

`fluent-zero` supports a modular architecture where libraries and dependencies manage their own translations independently, but share their end results with the caller.

While the **translation data** is isolated per crate (compile-time), the **language selection** is global (runtime). When your application calls `fluent_zero::set_lang("fr-FR")`, all UI plugins, logging dependencies, and nested widgets will instantly switch contexts without manual propagation. Each crate negotiates that language against its own locales and uses its own fallback locale, so a library shipping only `fr/` still answers `fr-FR`.

---

## 🔠 Enterprise Font Subsetting (DAG IPC)

When deploying to environments (like WASM or games) that bundle custom fonts, you must reliably compute the unique characters used across **all your dependencies** to prevent missing glyphs at runtime.

`fluent-zero` achieves this with a hermetic, build-system-agnostic pipeline (100% compatible with `sccache`, `Bazel`, and `Nix`). It utilizes Cargo's native IPC (Inter-Package Communication) via the DAG to safely bubble up characters from dependencies without requiring brittle `cargo_metadata` JSON scraping or workspace directory-walking.

### Step 1: Opt-in your Dependencies

For Cargo to authorize data bubbling up to your main application, any UI dependency or plugin using `fluent-zero` must declare a **globally unique** `links` key in its `Cargo.toml`.

```toml
[package]
name = "my-ui-library"
version = "0.1.0"
links = "my_ui_library" # <-- **REQUIRED FOR IPC BUBBLING**
```

### Step 2: Configure the Application `build.rs`

In your top-level application, configure `fluent-zero-build` to export the charset. The builder will automatically read the injected IPC variables from Cargo (`DEP_<LINKS>_FLUENT_CHARSET_PATH`) and merge all dependency characters into a single master file.

```rust
use std::{env, path::Path, process::Command};

use anyhow::Context as _;

fn main() -> anyhow::Result<()> {
    let out_dir = env::var("OUT_DIR").context("OUT_DIR environment variable not set")?;
    let out_dir_path = Path::new(&out_dir);
    let charset_path = out_dir_path.join("fluent_chars.txt");

    // 1. Enterprise Dependency Aggregation via Cargo IPC
    // The builder natively aggregates character sets exposed by dependencies.
    fluent_zero_build::FluentZeroBuilder::new("assets/locales")
        .export_charset(&charset_path)
        .generate();

    // 2. Run the Python Subsetter securely locked inside the OUT_DIR phase
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").context("CARGO_MANIFEST_DIR not set")?;
    let script_path = Path::new(&manifest_dir).join("scripts/subset_fonts.py");
    let fonts_dir = Path::new(&manifest_dir).join("assets/fonts");

    let status = Command::new("python3")
        .arg(&script_path)
        .arg(&charset_path)
        .arg(&fonts_dir)
        .arg(out_dir_path)
        .status()
        .context("Failed to execute python subsetting script")?;

    anyhow::ensure!(
        status.success(),
        "Python font subsetting failed with exit status: {status}"
    );

    // Tell Cargo to re-run this script if fonts or scripts change
    println!("cargo:rerun-if-changed={}", fonts_dir.display());
    println!("cargo:rerun-if-changed={}", script_path.display());

    Ok(())
}
```

Write the exported file inside `OUT_DIR`, as above. A relative path is resolved against the package root, which writes into your source tree and breaks hermetic builds (`sccache`, `Bazel`, `Nix`); the builder still honours it, but emits a `cargo:warning`.

#### Limitations of `links`

`links` was designed for native libraries, and two of its rules carry over:

- **One package per `links` value in the whole dependency graph.** Cargo refuses to build a graph in which two packages declare the same `links` key, so two semver-incompatible versions of the same UI library (say `my-ui-library` 1.x and 2.x pulled in by different dependents) cannot coexist.
- **`DEP_<LINKS>_*` variables only reach direct dependents.** If your application depends on `widgets`, which depends on `my-ui-library`, the application never sees `my-ui-library`'s charset unless `widgets` also runs `fluent-zero-build` in its build script (and declares its own `links` key). Its builder merges the dependency's characters into its own exported charset and relays them upward, even if `widgets` has no `.ftl` files itself.

#### Alternative without `links`

Every crate that runs the builder also gets a `CHARSET` constant in its generated code. A library can re-export it (`include!` already makes it `pub`), and the application can join them at runtime or in a build step with `fluent_zero::join_charsets`, with no `links` key anywhere:

```rust
let complete_charset = fluent_zero::join_charsets(&[
    crate::CHARSET,
    my_ui_library::CHARSET,
    widgets::CHARSET,
]);
```

The trade-off is that the application has to list each crate explicitly, and the joined set is only available to code that can depend on those crates (not to the application's own build script).

### Step 3: The Python Subsetter Script

You will need the `fonttools` library (`pip install fonttools`) to strip out unneeded glyphs. Place this production-ready script in `scripts/subset_fonts.py`.

```python
#!/usr/bin/env python3
import sys
import subprocess
from pathlib import Path

def main():
    if len(sys.argv) != 4:
        print("Usage: subset_fonts.py <charset_path> <fonts_dir> <out_dir>")
        sys.exit(1)

    charset_path = Path(sys.argv[1]).resolve()
    fonts_dir = Path(sys.argv[2]).resolve()
    out_dir = Path(sys.argv[3]).resolve()

    if not charset_path.exists():
        sys.exit(f"Error: Charset file not found at {charset_path}")

    # Read the unified character set
    text = charset_path.read_text(encoding="utf-8")

    # Enterprise Safety: Always include basic ASCII (32-126) for debug text and fallbacks
    basic_ascii = "".join(chr(i) for i in range(32, 127))
    master_text = text + basic_ascii

    # Write out a temporary file for pyftsubset to consume safely
    out_dir.mkdir(parents=True, exist_ok=True)
    temp_text_path = out_dir / "subset_target.txt"
    temp_text_path.write_text(master_text, encoding="utf-8")

    # Iterate and subset all fonts in the directory
    for font_file in fonts_dir.glob("*.ttf"):
        out_font = out_dir / font_file.name

        # pyftsubset CLI arguments for safe GUI subsetting
        args = [
            "pyftsubset",
            str(font_file),
            f"--text-file={temp_text_path}",
            f"--output-file={out_font}",
            "--layout-features=*",
            "--glyph-names",
            "--symbol-cmap",
            "--legacy-cmap",
            "--notdef-glyph",
            "--notdef-outline",
            "--recommended-glyphs",
            "--name-IDs=*",
            "--name-legacy",
            "--name-languages=*",
            "--desubroutinize"
        ]

        print(f"Subsetting {font_file.name} -> {out_font.name}...")
        subprocess.run(args, check=True)

if __name__ == "__main__":
    main()
```

Your optimized `.ttf` files are now securely located in `OUT_DIR` and can be seamlessly embedded into your binary using `include_bytes!(concat!(env!("OUT_DIR"), "/my_font.ttf"))`.

---

## ↔️ Bidi Isolation Marks

Like `fluent-bundle`, the generated code wraps placeables (`{ $name }`, `{ 5 }`) in the Unicode isolation marks U+2068 / U+2069 so right-to-left arguments cannot scramble left-to-right text, and vice versa. When enabled, the marks are added to `CHARSET` if any message can emit them. Many GUI font stacks render them as boxes or odd gaps; if your UI is left-to-right only, turn them off:

```rust
fluent_zero_build::FluentZeroBuilder::new("assets/locales")
    .use_isolating(false)
    .generate();
```

The setting applies to static strings formatted at build time and to the runtime bundles alike, so both stay byte-identical.

---

## 🧠 How it Works

1. **Build Time**: `fluent-zero-build` scans your `.ftl` files. It identifies which messages are static (their output cannot depend on runtime arguments: plain or multiline text, string and number literals, references to other static messages and terms, and selects over static selectors) and which are dynamic. Static messages are formatted at build time by the same `fluent-bundle` the runtime uses, so the embedded string is byte-identical to what the runtime would produce.
2. **Code Gen**: It generates a Rust module containing **Perfect Hash Maps** (via `phf`) for every locale.

- Static messages are compiled directly into the binary's read-only data section (`.rodata`).
- Dynamic messages are stored as raw FTL strings, wrapped in `LazyLock`.

3. **Run Time**:

- When you call `t!("hello")`, `fluent-zero` checks the PHF map.
- If it finds a static entry, it returns a reference to the binary data instantly. **No parsing. No allocation.**
- If it finds a dynamic entry, it initializes the heavy `FluentBundle` (only once) and performs the variable substitution.

## 🛠️ Example: using with `egui`

This crate shines in immediate mode GUIs. Because `t!` returns `Cow<'static, str>`, you can pass the result directly to widgets without `.to_string()` clones.

```rust
impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            // These calls are effectively free (nanoseconds).
            // They do not allocate memory.
            ui.heading(t!("menu_title"));

            if ui.button(t!("btn_submit")).clicked() {
                // ...
            }

            // Only this allocates, and only when 'count' changes if the UI is smart
            ui.label(t!("items_remaining", {
                "count" => self.items.len()
            }));
        });
    }
}
```

## ⚠️ Trade-offs

While `fluent-zero` is faster at runtime, it comes with trade-offs compared to `fluent-templates`:

1. **Compile Times**: Because it generates Rust code for every string in your FTL files, heavily localized applications may see increased compile times.
2. **Binary Size**: Static strings are embedded into the binary executable code.
3. **Flexibility**: You cannot easily load new FTL files from the filesystem at runtime without restarting the application (the cache is baked in).

## Stability and MSRV Policy

- **Versioning**: the crates follow [Semantic Versioning](https://semver.org/). While on `0.x`, breaking changes bump the minor version (`0.1` to `0.2`) and are listed in `CHANGELOG.md`; patch releases are non-breaking.
- **Generated code**: `fluent-zero` and `fluent-zero-build` are released together with the same version. Use matching versions; the generated code is only guaranteed to compile against the runtime of the same release.
- **MSRV**: the minimum supported Rust version is **Rust 1.88**, declared as `rust-version` in each crate's `Cargo.toml` and tested in CI. It is only raised in a minor release (never a patch release) and the change is noted in `CHANGELOG.md`.
- **Security**: see [`SECURITY.md`](SECURITY.md) for supported versions and how to report a vulnerability privately.

## Performance

`cargo bench -p fluent-zero-integration` benchmarks static, dynamic, fallback and missing-key lookups against real generated code, and `cargo test -p fluent-zero-integration --test zero_alloc` asserts with a counting allocator that static lookups (including through the en-US fallback) perform zero heap allocations.

## License

This project is licensed under the [MIT license](LICENSE).

## Notice

This crate is not related to [Mozilla Project Fluent](https://projectfluent.org/) in any official capacity. All usage is at your own risk.
