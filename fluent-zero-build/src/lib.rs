//! # fluent-zero-build
//!
//! Build-time code generator for [`fluent-zero`](https://docs.rs/fluent-zero).
//! Call it from `build.rs`; it compiles the `.ftl` files of every locale
//! directory into `$OUT_DIR/static_cache.rs`, which the consuming crate
//! `include!`s at its root.
//!
//! ```rust,no_run
//! // In build.rs's `main`:
//! fluent_zero_build::FluentZeroBuilder::new("assets/locales").generate();
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

use std::{
    collections::{BTreeSet, HashMap},
    env, fmt, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use fluent_bundle::{FluentBundle, FluentResource};
use fluent_syntax::{ast, parser, unicode::unescape_unicode_to_string};
use unic_langid::LanguageIdentifier;

/// The contract version between the generated code and the `fluent-zero`
/// runtime; must equal `fluent_zero::__private::CODEGEN_VERSION`.
///
/// The generated file asserts it at compile time, so a mismatched pair of
/// crates fails with a message saying which one to upgrade. It is a plain
/// counter, independent of the crates' semver versions: bump it in both crates,
/// to the same value, whenever the generated code changes what it needs from
/// the runtime.
const CODEGEN_VERSION: u32 = 1;

/// The path the generated code uses for the runtime crate unless
/// [`FluentZeroBuilder::crate_path`] overrides it.
const DEFAULT_CRATE_PATH: &str = "::fluent_zero";

/// Why [`FluentZeroBuilder::try_generate`] failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The `OUT_DIR` environment variable is not set, i.e. the builder is not
    /// running inside a Cargo build script.
    MissingOutDir,
    /// The path given to [`FluentZeroBuilder::crate_path`] is not a Rust path.
    InvalidCratePath(String),
    /// Reading a locale file or writing generated output failed.
    Io {
        /// The file or directory being read or written.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
    /// A message or term ID is defined more than once within one locale.
    DuplicateId {
        /// The ID as written (terms keep their `-` prefix).
        id: String,
        /// The locale, e.g. `en-US`.
        lang: String,
        /// File and 1-based line of the first definition.
        first: (PathBuf, usize),
        /// File and 1-based line of the repeated definition.
        second: (PathBuf, usize),
    },
    /// An `.ftl` file contains syntax errors.
    Parse {
        /// The offending file.
        path: PathBuf,
        /// The parser's description of every error in the file.
        message: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingOutDir => {
                f.write_str("OUT_DIR is not set; fluent-zero-build must run from a build script")
            }
            Self::InvalidCratePath(path) => write!(
                f,
                "invalid crate path {path:?}: expected a Rust path such as \"::fluent_zero\""
            ),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::DuplicateId {
                id,
                lang,
                first,
                second,
            } => write!(
                f,
                "`{id}` is defined twice in locale `{lang}`: \
                 first at {}:{}, again at {}:{}. \
                 Fluent bundles allow one definition per ID; rename or remove one.",
                first.0.display(),
                first.1,
                second.0.display(),
                second.1,
            ),
            Self::Parse { path, message } => {
                write!(f, "failed to parse {}: {message}", path.display())
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Adapts an [`io::Error`] on `path` into an [`Error`], for `map_err`.
fn io_err(path: &Path) -> impl FnOnce(io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// A builder to configure and generate the static cache code for `fluent-zero`.
#[derive(Debug, Clone)]
pub struct FluentZeroBuilder {
    locales_dir: PathBuf,
    charset_dest: Option<PathBuf>,
    crate_path: String,
}

impl Default for FluentZeroBuilder {
    fn default() -> Self {
        Self::new("assets/locales")
    }
}

impl FluentZeroBuilder {
    /// Creates a new builder pointing to the directory containing locale subdirectories.
    ///
    /// # Arguments
    ///
    /// * `locales_dir_path` - Relative path to the folder containing locale subdirectories.
    #[must_use]
    pub fn new<P: AsRef<Path>>(locales_dir_path: P) -> Self {
        Self {
            locales_dir: locales_dir_path.as_ref().to_path_buf(),
            charset_dest: None,
            crate_path: DEFAULT_CRATE_PATH.to_string(),
        }
    }

    /// Sets the path the generated code uses to reach the `fluent-zero` crate.
    ///
    /// Defaults to `::fluent_zero`. Set it when the dependency is renamed in
    /// `Cargo.toml`, e.g. `fz = { package = "fluent-zero", ... }` needs
    /// `.crate_path("::fz")`. An invalid path makes
    /// [`try_generate`](Self::try_generate) return [`Error::InvalidCratePath`].
    #[must_use]
    pub fn crate_path(mut self, path: impl Into<String>) -> Self {
        self.crate_path = path.into();
        self
    }

    /// Opt-in to exporting a deterministic text file containing all unique
    /// characters used across all `.ftl` files in this crate **and its dependencies**.
    ///
    /// **Dependency Aggregation:** This natively aggregates characters from dependencies
    /// using Cargo's standard IPC. No metadata scraping or tree walking is required.
    #[must_use]
    pub fn export_charset<P: AsRef<Path>>(mut self, dest: P) -> Self {
        self.charset_dest = Some(dest.as_ref().to_path_buf());
        self
    }

    /// Consumes the builder, parses the Fluent files, aggregates IPC variables,
    /// and generates the Rust static cache code.
    ///
    /// This is the panicking convenience over [`try_generate`](Self::try_generate),
    /// meant for build scripts where a panic is the natural way to fail.
    ///
    /// # Panics
    ///
    /// Panics with the error's message whenever [`try_generate`](Self::try_generate)
    /// returns an error.
    pub fn generate(self) {
        if let Err(err) = self.try_generate() {
            panic!("fluent-zero-build: {err}");
        }
    }

    /// Consumes the builder, parses the Fluent files, aggregates IPC variables,
    /// and generates the Rust static cache code.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if `OUT_DIR` is not set, the
    /// [`crate_path`](Self::crate_path) is invalid, an `.ftl` file fails to
    /// parse, an ID is defined twice in one locale, or reading the locales or
    /// writing the output fails.
    pub fn try_generate(self) -> Result<(), Error> {
        if !is_rust_path(&self.crate_path) {
            return Err(Error::InvalidCratePath(self.crate_path));
        }
        let out_dir = env::var("OUT_DIR").map_err(|_| Error::MissingOutDir)?;
        let dest_path = Path::new(&out_dir).join("static_cache.rs");
        let mut file = fs::File::create(&dest_path).map_err(io_err(&dest_path))?;
        let krate = self.crate_path.as_str();

        writeln!(
            &mut file,
            "// @generated by fluent-zero-build\n\
             const _: () = {krate}::__private::assert_codegen_version({CODEGEN_VERSION});"
        )
        .map_err(io_err(&dest_path))?;

        let mut unique_chars: BTreeSet<char> = BTreeSet::new();

        // 1. Dependency Aggregation via Cargo IPC.
        // Cargo automatically propagates `cargo:KEY=VALUE` emitted by dependency build
        // scripts up to dependents as `DEP_<LINKS>_<KEY>` if the dependency specifies a `links` key.
        for (env_key, env_val) in env::vars() {
            if env_key.starts_with("DEP_")
                && env_key.ends_with("_FLUENT_CHARSET_PATH")
                && let Ok(content) = fs::read_to_string(&env_val)
            {
                extend_visible(&mut unique_chars, &content);
            }
        }

        let mut bundle_entries: Vec<(String, String)> = Vec::new();
        let mut cache_root_entries: Vec<(String, String)> = Vec::new();

        // 2. Process Local Translations (If present)
        if self.locales_dir.exists() {
            println!("cargo:rerun-if-changed={}", self.locales_dir.display());
            let dir_entries = fs::read_dir(&self.locales_dir).map_err(io_err(&self.locales_dir))?;

            for entry in dir_entries {
                let entry = entry.map_err(io_err(&self.locales_dir))?;
                let path = entry.path();

                if !path.is_dir() {
                    continue;
                }

                let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let Ok(lang_id) = dir_name.parse::<LanguageIdentifier>() else {
                    continue;
                };

                let lang_key = lang_id.to_string();
                let sanitized_lang = lang_key.replace('-', "_").to_uppercase();

                let cache_name = format!("CACHE_{sanitized_lang}");
                let bundle_name = format!("BUNDLE_{sanitized_lang}");

                let combined_ftl_source =
                    Self::process_locale_dir(&path, &lang_key, &mut unique_chars)?;
                let cache_entries = static_cache_entries(krate, &lang_id, &combined_ftl_source);

                Self::write_bundle_initializer(
                    &mut file,
                    krate,
                    &bundle_name,
                    &lang_key,
                    &combined_ftl_source,
                )
                .map_err(io_err(&dest_path))?;
                Self::write_cache_map(&mut file, krate, &cache_name, &cache_entries)
                    .map_err(io_err(&dest_path))?;

                bundle_entries.push((lang_key.clone(), format!("&{bundle_name}")));
                cache_root_entries.push((lang_key, cache_name));
            }
        }

        // 3. Complete Code Generation & Upstream Exporting
        // This unconditionally runs to guarantee valid empty PHF maps in aggregator crates.
        Self::write_root_maps(&mut file, krate, &cache_root_entries, &bundle_entries)
            .map_err(io_err(&dest_path))?;
        self.export_charset_data(&mut file, &dest_path, &unique_chars, &out_dir)
    }

    fn process_locale_dir(
        path: &Path,
        lang_key: &str,
        unique_chars: &mut BTreeSet<char>,
    ) -> Result<String, Error> {
        let mut combined_ftl_source = String::new();
        let mut definitions = Definitions::default();

        let entries = fs::read_dir(path).map_err(io_err(path))?;

        for file_entry in entries {
            let file_entry = file_entry.map_err(io_err(path))?;
            let file_path = file_entry.path();

            if file_path.extension().is_some_and(|ext| ext == "ftl") {
                println!("cargo:rerun-if-changed={}", file_path.display());
                let source = fs::read_to_string(&file_path).map_err(io_err(&file_path))?;

                combined_ftl_source.push_str(&source);
                combined_ftl_source.push('\n');

                definitions.record(lang_key, &file_path, &source)?;
                let ast = parser::parse(source).map_err(|(_, errors)| Error::Parse {
                    message: errors
                        .iter()
                        .map(|e| format!("{e} at byte {}", e.pos.start))
                        .collect::<Vec<_>>()
                        .join("; "),
                    path: file_path.clone(),
                })?;
                for entry in ast.body {
                    match entry {
                        ast::Entry::Message(msg) => harvest_message_chars(&msg, unique_chars),
                        ast::Entry::Term(term) => harvest_term_chars(&term, unique_chars),
                        _ => {}
                    }
                }
            }
        }

        Ok(combined_ftl_source)
    }

    fn write_bundle_initializer(
        file: &mut impl Write,
        krate: &str,
        bundle_name: &str,
        lang_key: &str,
        combined_ftl_source: &str,
    ) -> io::Result<()> {
        let escaped_ftl = format!("{combined_ftl_source:?}");
        // This runs once per locale, on its first dynamic lookup, so it must
        // never panic. Each fallible step below was already proven to succeed
        // at build time, so the fallbacks are unreachable and exist only to
        // keep panics out of consumers' binaries:
        // - `lang_key` is the `Display` of a parsed `LanguageIdentifier`,
        //   which always parses back.
        // - Every file was parsed without errors in `process_locale_dir`.
        // - Duplicate IDs were rejected there too, the only way adding a
        //   resource can fail; overriding makes that infallible by signature.
        let init_code = format!(
            "std::sync::LazyLock::new(|| {{\n\
            \x20   let lang: {krate}::LanguageIdentifier = \"{lang_key}\".parse().unwrap_or_default();\n\
            \x20   let mut bundle = {krate}::__private::ConcurrentFluentBundle::new_concurrent(vec![lang]);\n\
            \x20   let res = {krate}::__private::FluentResource::try_new({escaped_ftl}.to_string()).unwrap_or_else(|(res, _)| res);\n\
            \x20   bundle.add_resource_overriding(res);\n\
            \x20   bundle\n\
            }})"
        );
        writeln!(
            file,
            "static {bundle_name}: std::sync::LazyLock<{krate}::__private::ConcurrentFluentBundle<{krate}::__private::FluentResource>> = {init_code};"
        )
    }

    fn write_cache_map(
        file: &mut impl Write,
        krate: &str,
        cache_name: &str,
        cache_entries: &[(String, String)],
    ) -> io::Result<()> {
        let phf_path = format!("{krate}::__private::phf");
        let mut map = phf_codegen::Map::new();
        map.phf_path(&phf_path);
        for (k, v) in cache_entries {
            map.entry(k.as_str(), v.as_str());
        }
        writeln!(
            file,
            "static {cache_name}: {phf_path}::Map<&'static str, {krate}::CacheEntry> = {};",
            map.build()
        )
    }

    fn write_root_maps(
        file: &mut impl Write,
        krate: &str,
        cache_root_entries: &[(String, String)],
        bundle_entries: &[(String, String)],
    ) -> io::Result<()> {
        let phf_path = format!("{krate}::__private::phf");
        let mut root_map = phf_codegen::Map::new();
        root_map.phf_path(&phf_path);
        for (l, v) in cache_root_entries {
            root_map.entry(l.as_str(), format!("&{v}"));
        }
        writeln!(
            file,
            "pub static CACHE: {phf_path}::Map<&'static str, &'static {phf_path}::Map<&'static str, {krate}::CacheEntry>> = {};",
            root_map.build()
        )?;

        let mut bundle_map = phf_codegen::Map::new();
        bundle_map.phf_path(&phf_path);
        for (l, c) in bundle_entries {
            bundle_map.entry(l.as_str(), c.as_str());
        }
        writeln!(
            file,
            "pub static LOCALES: {phf_path}::Map<&'static str, &'static std::sync::LazyLock<{krate}::__private::ConcurrentFluentBundle<{krate}::__private::FluentResource>>> = {};",
            bundle_map.build()
        )
    }

    fn export_charset_data(
        &self,
        file: &mut impl Write,
        dest_path: &Path,
        unique_chars: &BTreeSet<char>,
        out_dir: &str,
    ) -> Result<(), Error> {
        let charset_string: String = unique_chars.iter().collect();

        // Embed the charset constant natively into the module.
        writeln!(
            file,
            "\n/// A deterministically sorted string containing all unique characters\n\
             /// used across all `.ftl` files in this crate and its dependencies.\n\
             pub const CHARSET: &str = {charset_string:?};"
        )
        .map_err(io_err(dest_path))?;

        // 1. Emit absolute path for the NEXT build.rs to consume natively.
        // Cargo automatically captures this and converts it into `DEP_<LINKS>_FLUENT_CHARSET_PATH`.
        // We write to a file rather than passing the string to bypass Windows 32KB limits.
        let internal_dest = Path::new(out_dir).join("fluent_charset_internal.txt");
        if write_if_changed(&internal_dest, &charset_string).is_ok() {
            println!("cargo:fluent_charset_path={}", internal_dest.display());
        }

        if let Some(charset_dest) = &self.charset_dest {
            if let Some(parent) = charset_dest.parent() {
                let _ = fs::create_dir_all(parent);
            }
            write_if_changed(charset_dest, &charset_string).map_err(io_err(charset_dest))?;
            println!("cargo:rerun-if-changed={}", charset_dest.display());
        }
        Ok(())
    }
}

/// Whether `path` is a plain Rust path (`::a::b`, `a`, `crate::a`) that the
/// generated code can prefix item paths with.
fn is_rust_path(path: &str) -> bool {
    let path = path.strip_prefix("::").unwrap_or(path);
    !path.is_empty()
        && path.split("::").all(|segment| {
            let mut chars = segment.chars();
            chars
                .next()
                .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
                && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
                && segment != "_"
        })
}

/// Where each message and term of one locale is defined, to reject duplicates.
///
/// Every file of a locale is merged into a single `FluentBundle`, which
/// refuses a second definition of the same ID. Catching it here turns what
/// would be a runtime failure into a build error that names both locations.
#[derive(Default)]
struct Definitions {
    /// Keyed by the ID as written, so terms keep their `-` prefix and live in
    /// their own namespace, exactly as Fluent treats them.
    locations: HashMap<String, (PathBuf, usize)>,
}

impl Definitions {
    /// Records every entry defined in `source`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DuplicateId`] with both locations when an ID is
    /// already defined in this locale.
    fn record(&mut self, lang_key: &str, file: &Path, source: &str) -> Result<(), Error> {
        for (id, line) in entry_definitions(source) {
            if let Some(first) = self.locations.get(id) {
                return Err(Error::DuplicateId {
                    id: id.to_string(),
                    lang: lang_key.to_string(),
                    first: first.clone(),
                    second: (file.to_path_buf(), line),
                });
            }
            self.locations
                .insert(id.to_string(), (file.to_path_buf(), line));
        }
        Ok(())
    }
}

/// Yields `(id, 1-based line)` for every message and term in `source`.
///
/// In Fluent syntax an entry can only start at column 0 with an identifier
/// (`-` for terms) followed by `=`: continuation lines must be indented and
/// comments start with `#`, so this line scan finds exactly the entries the
/// parser does, plus the line numbers its AST does not carry.
fn entry_definitions(source: &str) -> impl Iterator<Item = (&str, usize)> {
    source.lines().enumerate().filter_map(|(index, line)| {
        let (head, _) = line.split_once('=')?;
        let id = head.trim_end();
        let name = id.strip_prefix('-').unwrap_or(id);
        let mut chars = name.chars();
        let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        valid.then_some((id, index + 1))
    })
}

/// Builds the `(message id, CacheEntry expression)` pairs for one locale.
///
/// A message is static when its output cannot depend on runtime arguments:
/// only text, string/number literals, and references to other static
/// messages or terms, including selects over static selectors. Static
/// messages are formatted here, at build time, by the same `fluent-bundle`
/// the runtime uses with the same settings, so the embedded `&'static str` is
/// byte-identical to what the runtime bundle would produce (bidi isolation
/// marks included). Anything that fails to format cleanly stays dynamic so the
/// runtime keeps its existing behaviour for it.
fn static_cache_entries(
    krate: &str,
    lang_id: &LanguageIdentifier,
    source: &str,
) -> Vec<(String, String)> {
    // Syntax errors were already rejected per file, so a parse error here is
    // impossible; fall back to the partial resource rather than panicking.
    let resource = FluentResource::try_new(source.to_owned()).unwrap_or_else(|(res, _)| res);
    let mut bundle = FluentBundle::new(vec![lang_id.clone()]);
    bundle.add_resource_overriding(&resource);

    let mut analysis = StaticAnalysis::new(&resource);
    let mut entries = Vec::new();
    for entry in resource.entries() {
        let ast::Entry::Message(msg) = entry else {
            continue;
        };
        let static_value = msg
            .value
            .as_ref()
            .filter(|_| analysis.message(msg.id.name, None))
            .and_then(|pattern| {
                let mut errors = Vec::new();
                let value = bundle.format_pattern(pattern, None, &mut errors);
                errors.is_empty().then(|| value.into_owned())
            });
        let cache_entry = match static_value {
            // `{:?}` on a `str` is a valid Rust string literal: quotes,
            // backslashes and invisible characters are all escaped.
            Some(value) => format!("{krate}::CacheEntry::Static({value:?})"),
            None => format!("{krate}::CacheEntry::Dynamic"),
        };
        entries.push((msg.id.name.to_string(), cache_entry));
    }
    entries
}

/// Decides which messages and terms render the same text for any arguments.
struct StaticAnalysis<'r> {
    messages: HashMap<&'r str, &'r ast::Message<&'r str>>,
    terms: HashMap<&'r str, &'r ast::Term<&'r str>>,
    /// Memoised verdicts keyed by (is term, id, attribute). An entry is seeded
    /// with `false` while it is being analysed, so reference cycles (which
    /// fail to format anyway) resolve to dynamic instead of recursing forever.
    verdicts: HashMap<(bool, &'r str, Option<&'r str>), bool>,
}

impl<'r> StaticAnalysis<'r> {
    fn new(resource: &'r FluentResource) -> Self {
        let mut messages = HashMap::new();
        let mut terms = HashMap::new();
        for entry in resource.entries() {
            match entry {
                ast::Entry::Message(msg) => {
                    messages.insert(msg.id.name, msg);
                }
                ast::Entry::Term(term) => {
                    terms.insert(term.id.name, term);
                }
                _ => {}
            }
        }
        Self {
            messages,
            terms,
            verdicts: HashMap::new(),
        }
    }

    fn message(&mut self, id: &'r str, attribute: Option<&'r str>) -> bool {
        let messages = &self.messages;
        let pattern = messages.get(id).and_then(|msg| match attribute {
            None => msg.value.as_ref(),
            Some(name) => attribute_value(&msg.attributes, name),
        });
        self.verdict((false, id, attribute), pattern)
    }

    fn term(&mut self, id: &'r str, attribute: Option<&'r str>) -> bool {
        let terms = &self.terms;
        let pattern = terms.get(id).and_then(|term| match attribute {
            None => Some(&term.value),
            Some(name) => attribute_value(&term.attributes, name),
        });
        self.verdict((true, id, attribute), pattern)
    }

    fn verdict(
        &mut self,
        key: (bool, &'r str, Option<&'r str>),
        pattern: Option<&'r ast::Pattern<&'r str>>,
    ) -> bool {
        if let Some(&verdict) = self.verdicts.get(&key) {
            return verdict;
        }
        self.verdicts.insert(key, false);
        let verdict = pattern.is_some_and(|pattern| self.pattern(pattern));
        self.verdicts.insert(key, verdict);
        verdict
    }

    fn pattern(&mut self, pattern: &'r ast::Pattern<&'r str>) -> bool {
        pattern.elements.iter().all(|element| match element {
            ast::PatternElement::TextElement { .. } => true,
            ast::PatternElement::Placeable { expression } => self.expression(expression),
        })
    }

    fn expression(&mut self, expression: &'r ast::Expression<&'r str>) -> bool {
        match expression {
            ast::Expression::Select { selector, variants } => {
                self.inline(selector) && variants.iter().all(|v| self.pattern(&v.value))
            }
            ast::Expression::Inline(inline) => self.inline(inline),
        }
    }

    fn inline(&mut self, inline: &'r ast::InlineExpression<&'r str>) -> bool {
        match inline {
            ast::InlineExpression::StringLiteral { .. }
            | ast::InlineExpression::NumberLiteral { .. } => true,
            ast::InlineExpression::MessageReference { id, attribute } => {
                self.message(id.name, attribute.as_ref().map(|a| a.name))
            }
            // Term arguments only feed `$variables` inside the term, and any
            // variable makes a pattern dynamic, so literal arguments are fine.
            ast::InlineExpression::TermReference {
                id,
                attribute,
                arguments,
            } => {
                arguments.as_ref().is_none_or(|args| {
                    args.positional.iter().all(|a| self.inline(a))
                        && args.named.iter().all(|a| self.inline(&a.value))
                }) && self.term(id.name, attribute.as_ref().map(|a| a.name))
            }
            ast::InlineExpression::Placeable { expression } => self.expression(expression),
            // Functions may format with runtime state (e.g. locale-aware
            // `NUMBER`) and variables are runtime arguments by definition.
            ast::InlineExpression::FunctionReference { .. }
            | ast::InlineExpression::VariableReference { .. } => false,
        }
    }
}

fn attribute_value<'r>(
    attributes: &'r [ast::Attribute<&'r str>],
    name: &str,
) -> Option<&'r ast::Pattern<&'r str>> {
    attributes
        .iter()
        .find(|attribute| attribute.id.name == name)
        .map(|attribute| &attribute.value)
}

/// Collects every character a message can render: its value, all of its
/// attributes, and every branch of any select expression inside them.
fn harvest_message_chars(msg: &ast::Message<String>, unique_chars: &mut BTreeSet<char>) {
    if let Some(pattern) = &msg.value {
        harvest_pattern_chars(pattern, unique_chars);
    }
    for attribute in &msg.attributes {
        harvest_pattern_chars(&attribute.value, unique_chars);
    }
}

/// Terms are never looked up directly, but their text is spliced into any
/// message that references them, so it is rendered all the same.
fn harvest_term_chars(term: &ast::Term<String>, unique_chars: &mut BTreeSet<char>) {
    harvest_pattern_chars(&term.value, unique_chars);
    for attribute in &term.attributes {
        harvest_pattern_chars(&attribute.value, unique_chars);
    }
}

fn harvest_pattern_chars(pattern: &ast::Pattern<String>, unique_chars: &mut BTreeSet<char>) {
    for element in &pattern.elements {
        match element {
            ast::PatternElement::TextElement { value } => extend_visible(unique_chars, value),
            ast::PatternElement::Placeable { expression } => {
                harvest_expression_chars(expression, unique_chars);
            }
        }
    }
}

fn harvest_expression_chars(
    expression: &ast::Expression<String>,
    unique_chars: &mut BTreeSet<char>,
) {
    match expression {
        // Every variant can be rendered depending on the runtime arguments, not
        // just the default one. The selector itself is only compared, never shown.
        ast::Expression::Select { variants, .. } => {
            for variant in variants {
                harvest_pattern_chars(&variant.value, unique_chars);
            }
        }
        ast::Expression::Inline(inline) => harvest_inline_chars(inline, unique_chars),
    }
}

fn harvest_inline_chars(inline: &ast::InlineExpression<String>, unique_chars: &mut BTreeSet<char>) {
    match inline {
        // String literals may carry escapes (`{"\u00A0"}`); harvest what renders.
        ast::InlineExpression::StringLiteral { value } => {
            extend_visible(unique_chars, &unescape_unicode_to_string(value));
        }
        ast::InlineExpression::NumberLiteral { value } => extend_visible(unique_chars, value),
        // Positional arguments may be echoed by custom functions. Named arguments
        // are formatting options (`style: "percent"`) and never rendered.
        ast::InlineExpression::FunctionReference { arguments, .. } => {
            for argument in &arguments.positional {
                harvest_inline_chars(argument, unique_chars);
            }
        }
        ast::InlineExpression::Placeable { expression } => {
            harvest_expression_chars(expression, unique_chars);
        }
        // References resolve to entries harvested on their own; variables are
        // runtime data the charset cannot know about.
        ast::InlineExpression::MessageReference { .. }
        | ast::InlineExpression::TermReference { .. }
        | ast::InlineExpression::VariableReference { .. } => {}
    }
}

fn extend_visible(unique_chars: &mut BTreeSet<char>, text: &str) {
    unique_chars.extend(text.chars().filter(|c| !c.is_control()));
}

/// Generates the static cache code for `fluent-zero` from `locales_dir_path`
/// with the default settings; shorthand for
/// `FluentZeroBuilder::new(locales_dir_path).generate()`.
///
/// # Panics
///
/// Panics when [`FluentZeroBuilder::generate`] does.
pub fn generate_static_cache(locales_dir_path: &str) {
    FluentZeroBuilder::new(locales_dir_path).generate();
}

/// Writes `contents` to `path` only when it differs from what is already
/// there, so an unchanged file keeps its modification time.
///
/// Cargo compares a `rerun-if-changed` path against the time the build
/// script STARTED: a file the script rewrites on every run is always newer,
/// so the script (and the crate depending on its output) re-ran on every
/// cargo invocation. Unchanged content is now left untouched; a changed or
/// deleted file is still (re)written, exactly as before.
fn write_if_changed(path: &Path, contents: &str) -> std::io::Result<()> {
    if fs::read_to_string(path).is_ok_and(|existing| existing == contents) {
        return Ok(());
    }
    fs::write(path, contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Harvests the charset of an FTL source exactly as `generate` does.
    fn charset(source: &str) -> String {
        let ast = parser::parse(source.to_string()).expect("test FTL must parse");
        let mut unique_chars = BTreeSet::new();
        for entry in &ast.body {
            match entry {
                ast::Entry::Message(msg) => harvest_message_chars(msg, &mut unique_chars),
                ast::Entry::Term(term) => harvest_term_chars(term, &mut unique_chars),
                _ => {}
            }
        }
        unique_chars.into_iter().collect()
    }

    fn assert_contains_all(charset: &str, expected: &str) {
        for c in expected.chars() {
            assert!(
                charset.contains(c),
                "{c:?} missing from charset {charset:?}"
            );
        }
    }

    /// The generated `CacheEntry` expression for each message, by id.
    fn entries(source: &str) -> HashMap<String, String> {
        static_cache_entries(DEFAULT_CRATE_PATH, &"en-US".parse().unwrap(), source)
            .into_iter()
            .collect()
    }

    fn static_literal(value: &str) -> String {
        format!("::fluent_zero::CacheEntry::Static({value:?})")
    }

    const DYNAMIC: &str = "::fluent_zero::CacheEntry::Dynamic";

    #[test]
    fn finds_entry_definitions_with_line_numbers() {
        let source = "\
# comment = not an entry
hello = Hi
    continued = not an entry
-brand=Acme

multi =
    .attr = x
bad id = not an entry
";
        let found: Vec<_> = entry_definitions(source).collect();
        assert_eq!(found, [("hello", 2), ("-brand", 4), ("multi", 6)]);
    }

    #[test]
    fn terms_and_messages_have_separate_namespaces() {
        let mut definitions = Definitions::default();
        definitions
            .record("en-US", Path::new("a.ftl"), "brand = Message\n")
            .unwrap();
        definitions
            .record("en-US", Path::new("b.ftl"), "-brand = Term\n")
            .unwrap();
    }

    #[test]
    fn duplicate_across_files_names_both_locations() {
        let mut definitions = Definitions::default();
        definitions
            .record("en-US", Path::new("a.ftl"), "# header\nhello = Hi\n")
            .unwrap();
        let err = definitions
            .record("en-US", Path::new("b.ftl"), "x = 1\n\nhello = Again\n")
            .unwrap_err();
        assert!(
            matches!(&err, Error::DuplicateId { id, lang, first, second }
                if id == "hello" && lang == "en-US"
                    && first == &(PathBuf::from("a.ftl"), 2)
                    && second == &(PathBuf::from("b.ftl"), 3)),
            "{err:?}"
        );
        assert!(
            err.to_string().contains(
                "`hello` is defined twice in locale `en-US`: first at a.ftl:2, again at b.ftl:3"
            ),
            "{err}"
        );
    }

    #[test]
    fn duplicate_within_one_file_is_rejected() {
        let err = Definitions::default()
            .record("fr-FR", Path::new("a.ftl"), "-brand = A\n-brand = B\n")
            .unwrap_err();
        assert!(
            err.to_string().contains(
                "`-brand` is defined twice in locale `fr-FR`: first at a.ftl:1, again at a.ftl:2"
            ),
            "{err}"
        );
    }

    #[test]
    fn plain_text_is_static() {
        assert_eq!(entries("a = Hello\n")["a"], static_literal("Hello"));
    }

    #[test]
    fn quotes_and_backslashes_are_escaped_and_stay_static() {
        let entries = entries("q = Say \"hi\"\nb = C:\\path\n");
        assert_eq!(
            entries["q"],
            r#"::fluent_zero::CacheEntry::Static("Say \"hi\"")"#
        );
        assert_eq!(
            entries["b"],
            r#"::fluent_zero::CacheEntry::Static("C:\\path")"#
        );
    }

    #[test]
    fn multiline_text_is_static() {
        let entries = entries("m =\n    First line\n    second line\n");
        assert_eq!(entries["m"], static_literal("First line\nsecond line"));
    }

    #[test]
    fn string_literals_are_resolved() {
        let entries = entries("brace = Open {\"{\"} here\nnbsp = a{\"\\u00A0\"}b\n");
        assert_eq!(entries["brace"], static_literal("Open { here"));
        assert_eq!(entries["nbsp"], static_literal("a\u{a0}b"));
    }

    #[test]
    fn number_literals_keep_bundle_isolation_marks() {
        // fluent-bundle isolates number placeables in multi-element patterns;
        // the static value must match it byte for byte.
        assert_eq!(
            entries("n = Take { 5 } now\n")["n"],
            static_literal("Take \u{2068}5\u{2069} now")
        );
    }

    #[test]
    fn static_terms_messages_and_selectors_are_inlined() {
        let source = "\
-brand = Acme
    .gender = neuter
about = About { -brand }
pronoun = { -brand.gender ->
    [neuter] it
   *[other] they
}
title = { about }!
";
        let entries = entries(source);
        assert_eq!(entries["about"], static_literal("About Acme"));
        assert_eq!(entries["pronoun"], static_literal("it"));
        assert_eq!(entries["title"], static_literal("About Acme!"));
    }

    #[test]
    fn term_arguments_feeding_variables_are_dynamic() {
        let source = "\
-thing = { $case ->
    [gen] thing's
   *[nom] thing
}
a = The { -thing(case: \"gen\") }
";
        assert_eq!(entries(source)["a"], DYNAMIC);
    }

    #[test]
    fn anything_touching_variables_or_functions_is_dynamic() {
        let source = "\
v = Hi { $name }
f = { NUMBER(5) }
s = { $n ->
    [one] one
   *[other] many
}
via = Says { v }
";
        let entries = entries(source);
        for id in ["v", "f", "s", "via"] {
            assert_eq!(entries[id], DYNAMIC, "{id} should be dynamic");
        }
    }

    #[test]
    fn broken_references_and_cycles_stay_dynamic() {
        let entries = entries("missing = { nope }\nx = { y }\ny = { x }\nattrs =\n    .a = A\n");
        for id in ["missing", "x", "y", "attrs"] {
            assert_eq!(entries[id], DYNAMIC, "{id} should be dynamic");
        }
    }

    #[test]
    fn crate_path_prefixes_cache_entries() {
        let entries: HashMap<_, _> =
            static_cache_entries("::fz", &"en-US".parse().unwrap(), "a = A\nb = { $x }\n")
                .into_iter()
                .collect();
        assert_eq!(entries["a"], r#"::fz::CacheEntry::Static("A")"#);
        assert_eq!(entries["b"], "::fz::CacheEntry::Dynamic");
    }

    #[test]
    fn crate_path_prefixes_generated_items() {
        let mut out = Vec::new();
        FluentZeroBuilder::write_bundle_initializer(&mut out, "::fz", "B", "en-US", "a = A")
            .unwrap();
        FluentZeroBuilder::write_root_maps(&mut out, "::fz", &[], &[]).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(!out.contains("fluent_zero"), "default path leaked: {out}");
        assert!(out.contains("::fz::__private::ConcurrentFluentBundle"));
        assert!(out.contains("::fz::__private::phf::Map"));
    }

    #[test]
    fn validates_crate_paths() {
        for ok in ["::fluent_zero", "fz", "crate::reexports::fz", "_fz2"] {
            assert!(is_rust_path(ok), "{ok:?} should be accepted");
        }
        for bad in [
            "",
            "::",
            "fz::",
            "a::::b",
            "2fz",
            "fluent-zero",
            "_",
            "fz;",
            "::a b",
        ] {
            assert!(!is_rust_path(bad), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn invalid_crate_path_is_an_error_not_a_panic() {
        let err = FluentZeroBuilder::new("locales")
            .crate_path("fluent-zero")
            .try_generate()
            .unwrap_err();
        assert!(matches!(&err, Error::InvalidCratePath(p) if p == "fluent-zero"));
        assert!(err.to_string().contains("\"fluent-zero\""), "{err}");
    }

    #[test]
    fn syntax_errors_are_reported_with_the_file() {
        let dir = env::temp_dir().join(format!("fluent-zero-build-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("bad.ftl");
        fs::write(&file, "ok = fine\nbroken = { $x\n").unwrap();
        let err =
            FluentZeroBuilder::process_locale_dir(&dir, "en-US", &mut BTreeSet::new()).unwrap_err();
        fs::remove_dir_all(&dir).unwrap();

        let Error::Parse { path, message } = &err else {
            panic!("expected a parse error, got {err:?}");
        };
        assert_eq!(path, &file);
        assert!(message.contains("at byte"), "{message}");
        assert!(err.to_string().contains("bad.ftl"), "{err}");
        assert!(std::error::Error::source(&err).is_none());
    }

    #[test]
    fn io_errors_carry_path_and_source() {
        let err = io_err(Path::new("x/y.ftl"))(io::Error::new(io::ErrorKind::NotFound, "gone"));
        assert_eq!(
            err.to_string(),
            format!("{}: gone", Path::new("x/y.ftl").display())
        );
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn harvests_plain_text() {
        assert_eq!(charset("hello = Hi you\n"), " Hiouy");
    }

    #[test]
    fn harvests_every_select_variant() {
        let charset = charset("files = { $n ->\n    [one] Ёж\n   *[other] Щука\n}\n");
        assert_contains_all(&charset, "ЁжЩука");
    }

    #[test]
    fn harvests_nested_selects() {
        let source = "\
toggle = { $side ->
    [left] { $shown ->
        [true] ▶ 左
       *[false] ◀ 左
    }
   *[right] 右侧
}
";
        assert_contains_all(&charset(source), "▶◀左右侧");
    }

    #[test]
    fn harvests_attributes() {
        let charset = charset("login = Log in\n    .tooltip = Ẩn\n");
        assert_contains_all(&charset, "Ẩ");
    }

    #[test]
    fn harvests_message_with_only_attributes() {
        let charset = charset("login =\n    .placeholder = 왼쪽\n");
        assert_contains_all(&charset, "왼쪽");
    }

    #[test]
    fn harvests_terms_and_their_attributes() {
        let charset = charset("-brand = Ωmega\n    .short = Ж\n");
        assert_contains_all(&charset, "ΩmegaЖ");
    }

    #[test]
    fn harvests_unescaped_string_literals() {
        let charset = charset("price = {\"\\u00A0\"}{\"€\"}\n");
        assert_contains_all(&charset, "\u{a0}€");
        assert!(!charset.contains('\\'), "escape syntax leaked: {charset:?}");
    }

    #[test]
    fn harvests_number_literals() {
        assert_contains_all(&charset("n = { 42 }\n"), "42");
    }

    #[test]
    fn skips_identifiers_variables_and_named_options() {
        let source = "\
n = { NUMBER($count, style: \"percent\") ->
    [one] a
   *[other] b
}
";
        assert_eq!(charset(source), "ab");
    }
}
