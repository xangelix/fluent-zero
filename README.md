# fluent-zero

[![Crates.io](https://img.shields.io/crates/v/fluent-zero)](https://crates.io/crates/fluent-zero)
[![Docs.rs](https://docs.rs/fluent-zero/badge.svg)](https://docs.rs/fluent-zero)
[![License](https://img.shields.io/crates/l/fluent-zero)](https://spdx.org/licenses/MIT)

**Zero-allocation, high-performance Fluent localization for Rust.**

`fluent-zero` is a specialized localization loader designed for high-performance applications, such as **GUI clients** (egui, iced, winit) and **Game Development** (Bevy, Fyrox).

Unlike other loaders that prioritize template engine integration or hot-reloading, `fluent-zero` prioritizes **runtime speed and memory efficiency**. It generates static code at build time to allow for `O(1)` lookups that return `&'static str` whenever possible, eliminating the heap allocation overhead typical of localization libraries.

## ⚡ Why fluent-zero?

Most Fluent implementations (like `fluent-templates`) wrap the standard `fluent-bundle`. When you request a translation, they look it up in a `HashMap`, parse the pattern, and allocate a new `String` on the heap to return the result—even if the text is static.

In an immediate-mode GUI (like `egui`) running at 60 FPS, looking up 50 strings per frame results in **3,000 allocations per second**. This causes allocator contention and Garbage Collection-like micro-stutters.

**`fluent-zero` solves this by pre-computing the cache at compile time.**

| Feature | `fluent-templates` | `fluent-zero` |
| --- | --- | --- |
| **Static Text Lookup** | Heap Allocation (`String`) | **Zero Allocation** (`&'static str`) |
| **Lookup Speed** | HashMap + AST traversal | **Perfect Hash Function (PHF)** |
| **Memory Usage** | Full AST loaded on start | **Lazy / Zero-Cost Abstraction** |
| **Best For** | Web Servers (Tera/Askama) | **Desktop GUIs & Games** |

## 🚀 Usage

### 1. Installation

You need both the runtime library and the build-time code generator.

```toml
[dependencies]
fluent-zero = "0.1"
unic-langid = "0.9"

[build-dependencies]
fluent-zero-build = "0.1"
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
// build.rs
fn main() {
    // Generates static_cache.rs in your OUT_DIR
    fluent_zero_build::generate_static_cache("assets/locales");
}

```

### 4. Application Code

In your `lib.rs` (or `main.rs`), you must include the generated file. This brings the `CACHE` and `LOCALES` statics into scope, which the `t!` macro relies on.

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
    
    println!("{}", title);
    println!("{}", welcome);
}

```

## 📦 Library Support & Nested Translations

`fluent-zero` supports a modular architecture where libraries and dependencies manage their own translations independently, but share their end results with the caller.

If you are writing a library (e.g., a widget crate or a game engine plugin), you can include `fluent-zero-build` in your library's `build.rs`. Your library will generate its own private `CACHE` and `LOCALES` maps based on its own `.ftl` files, and your callers will automatically receive those translations via your normal APIs based on the set global language.

### Global Synchronization

While the **translation data** is isolated per crate (compile-time), the **language selection** is global (runtime).

* **`fluent_zero::set_lang(...)`**: Updates the language for the **entire application stack**.
* **`t!(...)`**: Uses the library-specific translation files but respects the globally set language.

This allows for a cohesive ecosystem where the end-user application sets the language once, and all dependencies (UI widgets, logging, error messages) switch context instantly without manual propagation.

```text
my-app/
├── Cargo.toml
├── build.rs             // Generates app-specific cache
├── assets/locales/      // Contains "welcome-screen.ftl"
└── src/main.rs          // Calls set_lang("fr-FR")

my-ui-library/ (Dependency)
├── Cargo.toml
├── build.rs             // Generates library-specific cache
├── assets/locales/      // Contains "ok-button.ftl", "cancel.ftl"
└── src/lib.rs           // Calls t!("ok-button")

```

In this example, when `my-app` calls `set_lang("fr-FR")`, the `my-ui-library` automatically begins serving French strings for its internal components.

## 🧠 How it Works

1. **Build Time**: `fluent-zero-build` scans your `.ftl` files. It identifies which messages are purely static (no variables) and which are dynamic.
2. **Code Gen**: It generates a Rust module containing **Perfect Hash Maps** (via `phf`) for every locale.
* Static messages are compiled directly into the binary's read-only data section (`.rodata`).
* Dynamic messages are stored as raw FTL strings, wrapped in `LazyLock`.
3. **Run Time**:
* When you call `t!("hello")`, `fluent-zero` checks the PHF map.
* If it finds a static entry, it returns a reference to the binary data instantly. **No parsing. No allocation.**
* If it finds a dynamic entry, it initializes the heavy `FluentBundle` (only once) and performs the variable substitution.

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

## License

This project is licensed under the MIT license.
