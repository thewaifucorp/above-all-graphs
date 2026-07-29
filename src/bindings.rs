//! Language-aware module and binding resolution.
//!
//! `crate::resolve` used to resolve every cross-file reference by symbol
//! name alone: a call to `run` matched every `run` in the repo, and an
//! import of `crate::b::Widget` matched every `Widget`. That is cheap and
//! language-neutral, but it fans out AMBIGUOUS edges exactly where a real
//! answer exists — the import statement already says *which file* the name
//! comes from.
//!
//! This module turns an import's *source* (`./helper`, `crate::sync`,
//! `os.path`, `github.com/org/repo/internal/foo`, `com.acme.Widget`) into
//! the repository file(s) it names, using each language's own module
//! conventions, and then builds a per-file table of local name → target.
//! Resolution stays offline and index-only: no toolchain, no type checker,
//! no network. A source that resolves to nothing is an external dependency
//! (`std`, `react`, a vendored crate) and is left alone.

use std::collections::{HashMap, HashSet};

use crate::parse::ImportRef;
use crate::toolchain::Toolchain;

/// Extensions tried when an import source omits one, keyed by the extension
/// of the *importing* file — a TypeScript file importing `./helper` means
/// `helper.ts` before `helper.js`, and never `helper.py`.
fn extensions_for(from_file: &str) -> &'static [&'static str] {
    match from_file.rsplit('.').next().unwrap_or_default() {
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => {
            &["ts", "tsx", "js", "jsx", "mjs", "cjs", "d.ts"]
        }
        "py" | "pyw" => &["py", "pyw"],
        "go" => &["go"],
        "java" => &["java"],
        "cs" => &["cs"],
        "rs" => &["rs"],
        "rb" | "rake" => &["rb", "rake"],
        "php" | "phtml" => &["php", "phtml"],
        "kt" | "kts" => &["kt", "kts"],
        "swift" => &["swift"],
        "scala" | "sc" => &["scala", "sc"],
        "dart" => &["dart"],
        "ex" | "exs" => &["ex", "exs"],
        "sh" | "bash" | "zsh" => &["sh", "bash", "zsh"],
        "lua" => &["lua"],
        "r" => &["r", "R"],
        "m" | "mm" => &["m", "mm", "h"],
        "c" | "h" => &["c", "h"],
        "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" => &["cpp", "cc", "hpp", "h", "hh", "cxx"],
        _ => &[],
    }
}

/// Directory part of a repo-relative path (`src/a/b.rs` → `src/a`).
fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// Joins a `./`-style relative source onto `dir`, collapsing `.` and `..`.
fn join_relative(dir: &str, source: &str) -> String {
    let mut segments: Vec<&str> = if dir.is_empty() {
        Vec::new()
    } else {
        dir.split('/').collect()
    };
    for part in source.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    segments.join("/")
}

/// Module path of a Rust `use` source, with the leading `crate`/`self`/
/// `super` markers dropped (`crate::a::b` → `a/b`). `None` when nothing is
/// left, which means the import named the crate root itself.
fn rust_module_path(source: &str) -> Option<String> {
    let segments: Vec<&str> = source
        .split("::")
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .skip_while(|segment| matches!(*segment, "crate" | "self" | "super" | "Self"))
        .collect();
    if segments.is_empty() {
        None
    } else {
        Some(segments.join("/"))
    }
}

/// Every indexed file path, queryable by the module notations the supported
/// languages use to name each other.
#[derive(Debug, Default, Clone)]
pub struct ModuleIndex {
    files: Vec<String>,
    set: HashSet<String>,
    toolchain: Toolchain,
}

impl ModuleIndex {
    /// Builds an index over repo-relative source paths.
    pub fn new<I: IntoIterator<Item = String>>(files: I) -> Self {
        Self::with_toolchain(files, Toolchain::default())
    }

    /// Builds an index that also honors the repository's own import aliases
    /// (`tsconfig` paths, workspace package names, the `go.mod` module
    /// prefix). Those rename the module space, so they are consulted before
    /// any path convention.
    pub fn with_toolchain<I: IntoIterator<Item = String>>(files: I, toolchain: Toolchain) -> Self {
        let mut files: Vec<String> = files.into_iter().collect();
        files.sort_unstable();
        files.dedup();
        let set = files.iter().cloned().collect();
        Self {
            files,
            set,
            toolchain,
        }
    }

    /// Repository files an import source names, or empty when the source
    /// points outside the repo (an external package, `std`, a vendored SDK).
    ///
    /// More than one file comes back only for directory-as-module languages
    /// (a Go package is a directory, not a file).
    #[must_use]
    pub fn resolve(&self, source: &str, from_file: &str) -> Vec<String> {
        let source = source
            .trim()
            .trim_matches(|character| character == '"' || character == '\'' || character == '`');
        if source.is_empty() {
            return Vec::new();
        }
        let extensions = extensions_for(from_file);
        if extensions.is_empty() {
            return Vec::new();
        }

        if source.starts_with("./") || source.starts_with("../") {
            let joined = join_relative(parent_dir(from_file), source);
            return self.exact_candidates(&joined, extensions);
        }
        if source.starts_with('.') && !source.contains('/') {
            return self.resolve_dotted_relative(source, from_file, extensions);
        }

        // A manifest alias is an explicit statement about what a specifier
        // means, so it outranks every path convention below.
        for candidate in self.toolchain.expand(source) {
            let hits = self.exact_candidates(&candidate, extensions);
            if !hits.is_empty() {
                return hits;
            }
        }

        let joined = if source.contains("::") {
            let Some(path) = rust_module_path(source) else {
                return Vec::new();
            };
            path
        } else if source.contains('/') || ends_with_source_extension(source, extensions) {
            // Already a path (`internal/store`, `local/header.h`) — a dot in
            // it is a file extension, not a package separator.
            source.trim_end_matches('/').to_string()
        } else if source.contains('.') {
            source.replace('.', "/")
        } else {
            source.to_string()
        };
        self.suffix_candidates(&joined, extensions, from_file)
    }

    /// Where an import statement points, accounting for the named member
    /// itself being a module (`use crate::sync;`, `import com.acme.Widget`).
    #[must_use]
    pub fn resolve_import(&self, import: &ImportRef, from_file: &str) -> ImportTarget {
        if !import.glob
            && !import.namespace
            && let Some(name) = import.name.as_deref()
        {
            let nested = self.resolve(&join_module(&import.source, name), from_file);
            if !nested.is_empty() {
                return ImportTarget::Module { files: nested };
            }
            let files = self.resolve(&import.source, from_file);
            return if files.is_empty() {
                ImportTarget::External
            } else {
                ImportTarget::Member {
                    files,
                    name: name.to_string(),
                }
            };
        }
        let files = self.resolve(&import.source, from_file);
        if files.is_empty() {
            ImportTarget::External
        } else {
            ImportTarget::Module { files }
        }
    }

    /// Python's `from .helper import x` / `from ..pkg.mod import y`: leading
    /// dots count how far up from the importing file's package to start.
    fn resolve_dotted_relative(
        &self,
        source: &str,
        from_file: &str,
        extensions: &[&str],
    ) -> Vec<String> {
        let ups = source
            .chars()
            .take_while(|character| *character == '.')
            .count();
        let rest = source[ups..].replace('.', "/");
        let mut dir = parent_dir(from_file).to_string();
        for _ in 1..ups {
            dir = parent_dir(&dir).to_string();
        }
        let joined = match (dir.is_empty(), rest.is_empty()) {
            (_, true) => dir,
            (true, false) => rest,
            (false, false) => format!("{dir}/{rest}"),
        };
        if joined.is_empty() {
            return Vec::new();
        }
        self.exact_candidates(&joined, extensions)
    }

    /// Candidate file names for a module path, in preference order: the path
    /// itself, then each language's "this directory is the module" file.
    fn candidate_names(joined: &str, extensions: &[&str]) -> Vec<String> {
        let mut stems = vec![joined.to_string()];
        // `./helper.js` in TypeScript usually means `helper.ts` on disk.
        for extension in extensions {
            if let Some(stem) = joined.strip_suffix(&format!(".{extension}")) {
                stems.push(stem.to_string());
                break;
            }
        }
        let mut names = Vec::new();
        for stem in &stems {
            for extension in extensions {
                names.push(format!("{stem}.{extension}"));
            }
            for extension in extensions {
                for module_file in ["mod", "index", "__init__", "lib", "main"] {
                    names.push(format!("{stem}/{module_file}.{extension}"));
                }
            }
        }
        names
    }

    /// Relative imports name an exact location — no repo-wide search.
    fn exact_candidates(&self, joined: &str, extensions: &[&str]) -> Vec<String> {
        if self.set.contains(joined) {
            return vec![joined.to_string()];
        }
        for name in Self::candidate_names(joined, extensions) {
            if self.set.contains(&name) {
                return vec![name];
            }
        }
        self.files_in_directory(joined, extensions)
    }

    /// Absolute module paths (`com.acme.Widget`, `internal/foo`, `crate::b`)
    /// are written against a root the index does not know — a Go module
    /// prefix, a source root, a package namespace — so they match on a path
    /// suffix, longest first. Ties break toward the file nearest the
    /// importer, which is what a monorepo with parallel copies of a package
    /// needs.
    fn suffix_candidates(&self, joined: &str, extensions: &[&str], from_file: &str) -> Vec<String> {
        let segments: Vec<&str> = joined.split('/').filter(|part| !part.is_empty()).collect();
        for start in 0..segments.len() {
            let tail = segments[start..].join("/");
            let exact = self.exact_candidates(&tail, extensions);
            if !exact.is_empty() {
                return exact;
            }
            if let Some(matched) = self.best_suffix_match(&tail, extensions, from_file) {
                return vec![matched];
            }
            // Directory-as-module (a Go package) below an unknown root.
            let needle = format!("/{tail}/");
            if let Some(directory) = self
                .files
                .iter()
                .find(|file| file.contains(&needle))
                .and_then(|file| file.split_once(&needle).map(|(head, _)| head))
                .map(|head| format!("{head}/{tail}"))
            {
                let files = self.files_in_directory(&directory, extensions);
                if !files.is_empty() {
                    return files;
                }
            }
        }
        Vec::new()
    }

    fn best_suffix_match(
        &self,
        tail: &str,
        extensions: &[&str],
        from_file: &str,
    ) -> Option<String> {
        for name in Self::candidate_names(tail, extensions) {
            let needle = format!("/{name}");
            let matched = self
                .files
                .iter()
                .filter(|file| file.ends_with(&needle))
                .min_by_key(|file| {
                    (
                        std::cmp::Reverse(shared_prefix_len(file, from_file)),
                        file.len(),
                        (*file).clone(),
                    )
                });
            if let Some(matched) = matched {
                return Some(matched.clone());
            }
        }
        None
    }

    /// Files sitting directly inside `directory` with a matching extension —
    /// a Go package, or a Python package imported without `__init__.py`.
    fn files_in_directory(&self, directory: &str, extensions: &[&str]) -> Vec<String> {
        if directory.is_empty() {
            return Vec::new();
        }
        let prefix = format!("{directory}/");
        self.files
            .iter()
            .filter(|file| {
                file.strip_prefix(&prefix)
                    .is_some_and(|rest| !rest.contains('/'))
                    && extensions
                        .iter()
                        .any(|extension| file.ends_with(&format!(".{extension}")))
            })
            .cloned()
            .collect()
    }
}

/// How many leading path segments two files share — the tiebreaker that
/// makes a suffix match prefer the copy closest to the importer.
fn shared_prefix_len(left: &str, right: &str) -> usize {
    left.split('/')
        .zip(right.split('/'))
        .take_while(|(a, b)| a == b)
        .count()
}

/// Whether an import source ends in a file extension the language uses, so
/// `local/header.h` is read as a path instead of a dotted package name.
fn ends_with_source_extension(source: &str, extensions: &[&str]) -> bool {
    extensions
        .iter()
        .any(|extension| source.ends_with(&format!(".{extension}")))
}

/// Where an import statement points inside the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportTarget {
    /// A named member of the resolved module.
    Member {
        /// Files the module resolved to.
        files: Vec<String>,
        /// Member taken from it.
        name: String,
    },
    /// The module itself — a namespace, wildcard, or bare module import.
    Module {
        /// Files the module resolved to.
        files: Vec<String>,
    },
    /// Not part of this repository.
    External,
}

/// What a local name in a file refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// Repository files the name was imported from.
    pub files: Vec<String>,
    /// Symbol name in those files, when the import named a member. `None`
    /// for a module/namespace binding, where the member comes from the call
    /// site (`utils.parse()` → `parse` in whatever `utils` resolved to).
    pub symbol: Option<String>,
    /// Whether this binding names a module rather than a symbol.
    pub namespace: bool,
}

/// Per-file map of local name → import target, plus the wildcard imports
/// whose whole surface is in scope.
#[derive(Debug, Default, Clone)]
pub struct Bindings {
    by_file: HashMap<String, HashMap<String, Binding>>,
    globs: HashMap<String, Vec<String>>,
}

impl Bindings {
    /// Builds the table from each file's imports, resolved against `index`.
    #[must_use]
    pub fn build(index: &ModuleIndex, imports: &HashMap<String, Vec<ImportRef>>) -> Self {
        let mut table = Self::default();
        for (file, refs) in imports {
            for import in refs {
                let (files, symbol) = match index.resolve_import(import, file) {
                    ImportTarget::External => continue,
                    ImportTarget::Member { files, name } => (files, Some(name)),
                    ImportTarget::Module { files } => (files, None),
                };
                if import.glob {
                    table.globs.entry(file.clone()).or_default().extend(files);
                    continue;
                }
                let Some(local) = import.local_name() else {
                    continue;
                };
                let namespace = symbol.is_none();
                table.by_file.entry(file.clone()).or_default().insert(
                    local.to_string(),
                    Binding {
                        files,
                        symbol,
                        namespace,
                    },
                );
            }
        }
        for files in table.globs.values_mut() {
            files.sort_unstable();
            files.dedup();
        }
        table
    }

    /// What `name` refers to inside `file`, if it was imported there.
    #[must_use]
    pub fn lookup(&self, file: &str, name: &str) -> Option<&Binding> {
        self.by_file.get(file)?.get(name)
    }

    /// Files whose entire surface is in scope in `file` via a wildcard import.
    #[must_use]
    pub fn glob_targets(&self, file: &str) -> &[String] {
        self.globs.get(file).map_or(&[], Vec::as_slice)
    }
}

/// Re-joins a module source and a member using the source's own separator,
/// so `crate::sync` + `run` stays Rust-shaped and `./utils` + `parse` stays
/// path-shaped.
fn join_module(source: &str, name: &str) -> String {
    if source.contains("::") {
        format!("{source}::{name}")
    } else if source.contains('/') || source.starts_with('.') {
        format!("{}/{name}", source.trim_end_matches('/'))
    } else {
        format!("{source}.{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(files: &[&str]) -> ModuleIndex {
        ModuleIndex::new(files.iter().map(|file| (*file).to_string()))
    }

    #[test]
    fn resolves_rust_module_path_to_file() {
        let index = index(&["src/main.rs", "src/sync.rs", "vendor/other/sync.rs"]);
        assert_eq!(index.resolve("crate::sync", "src/main.rs"), ["src/sync.rs"]);
    }

    #[test]
    fn resolves_rust_module_directory_form() {
        let index = index(&["src/main.rs", "src/sync/mod.rs"]);
        assert_eq!(
            index.resolve("crate::sync", "src/main.rs"),
            ["src/sync/mod.rs"]
        );
    }

    #[test]
    fn external_crate_resolves_to_nothing() {
        let index = index(&["src/main.rs"]);
        assert!(index.resolve("std::fs", "src/main.rs").is_empty());
    }

    #[test]
    fn resolves_relative_javascript_import_without_extension() {
        let index = index(&["src/app.js", "src/lib/helper.js"]);
        assert_eq!(
            index.resolve("./lib/helper", "src/app.js"),
            ["src/lib/helper.js"]
        );
    }

    #[test]
    fn resolves_typescript_import_written_with_js_extension() {
        let index = index(&["src/app.ts", "src/helper.ts"]);
        assert_eq!(
            index.resolve("./helper.js", "src/app.ts"),
            ["src/helper.ts"]
        );
    }

    #[test]
    fn resolves_relative_import_to_directory_index() {
        let index = index(&["src/app.ts", "src/widgets/index.ts"]);
        assert_eq!(
            index.resolve("./widgets", "src/app.ts"),
            ["src/widgets/index.ts"]
        );
    }

    #[test]
    fn relative_import_escaping_the_repo_resolves_to_nothing() {
        let index = index(&["src/app.js", "helper.js"]);
        assert!(
            index
                .resolve("../../outside/helper", "src/app.js")
                .is_empty()
        );
    }

    #[test]
    fn resolves_python_relative_import() {
        let index = index(&["pkg/app.py", "pkg/helper.py"]);
        assert_eq!(index.resolve(".helper", "pkg/app.py"), ["pkg/helper.py"]);
    }

    #[test]
    fn resolves_python_parent_relative_import() {
        let index = index(&["pkg/sub/app.py", "pkg/helper.py"]);
        assert_eq!(
            index.resolve("..helper", "pkg/sub/app.py"),
            ["pkg/helper.py"]
        );
    }

    #[test]
    fn resolves_python_absolute_package_import() {
        let index = index(&["app.py", "pkg/mod.py"]);
        assert_eq!(index.resolve("pkg.mod", "app.py"), ["pkg/mod.py"]);
    }

    #[test]
    fn resolves_python_package_init() {
        let index = index(&["app.py", "pkg/__init__.py"]);
        assert_eq!(index.resolve("pkg", "app.py"), ["pkg/__init__.py"]);
    }

    #[test]
    fn resolves_java_package_import_by_suffix() {
        let index = index(&[
            "src/main/java/com/acme/App.java",
            "src/main/java/com/acme/Widget.java",
        ]);
        assert_eq!(
            index.resolve("com.acme.Widget", "src/main/java/com/acme/App.java"),
            ["src/main/java/com/acme/Widget.java"]
        );
    }

    #[test]
    fn resolves_go_package_directory_to_every_file() {
        let index = index(&[
            "main.go",
            "internal/store/store.go",
            "internal/store/query.go",
        ]);
        let resolved = index.resolve("github.com/acme/app/internal/store", "main.go");
        assert_eq!(
            resolved,
            ["internal/store/query.go", "internal/store/store.go"]
        );
    }

    #[test]
    fn suffix_match_prefers_the_copy_nearest_the_importer() {
        let index = index(&[
            "apps/web/src/app.ts",
            "apps/web/src/lib/format.ts",
            "apps/api/src/lib/format.ts",
        ]);
        assert_eq!(
            index.resolve("src/lib/format", "apps/web/src/app.ts"),
            ["apps/web/src/lib/format.ts"]
        );
    }

    #[test]
    fn binds_named_import_to_its_symbol() {
        let index = index(&["src/app.ts", "src/helper.ts"]);
        let mut imports = HashMap::new();
        imports.insert(
            "src/app.ts".to_string(),
            vec![ImportRef {
                source: "./helper".into(),
                name: Some("parse".into()),
                alias: None,
                glob: false,
                namespace: false,
            }],
        );
        let bindings = Bindings::build(&index, &imports);
        let binding = bindings.lookup("src/app.ts", "parse").unwrap();
        assert_eq!(binding.files, ["src/helper.ts"]);
        assert_eq!(binding.symbol.as_deref(), Some("parse"));
        assert!(!binding.namespace);
    }

    #[test]
    fn binds_alias_to_the_original_symbol() {
        let index = index(&["src/app.ts", "src/helper.ts"]);
        let mut imports = HashMap::new();
        imports.insert(
            "src/app.ts".to_string(),
            vec![ImportRef {
                source: "./helper".into(),
                name: Some("parse".into()),
                alias: Some("readIt".into()),
                glob: false,
                namespace: false,
            }],
        );
        let bindings = Bindings::build(&index, &imports);
        assert!(bindings.lookup("src/app.ts", "parse").is_none());
        let binding = bindings.lookup("src/app.ts", "readIt").unwrap();
        assert_eq!(binding.symbol.as_deref(), Some("parse"));
    }

    #[test]
    fn last_segment_naming_a_module_binds_as_namespace() {
        let index = index(&["src/main.rs", "src/sync.rs"]);
        let mut imports = HashMap::new();
        imports.insert(
            "src/main.rs".to_string(),
            vec![ImportRef {
                source: "crate".into(),
                name: Some("sync".into()),
                alias: None,
                glob: false,
                namespace: false,
            }],
        );
        let bindings = Bindings::build(&index, &imports);
        let binding = bindings.lookup("src/main.rs", "sync").unwrap();
        assert!(binding.namespace);
        assert_eq!(binding.files, ["src/sync.rs"]);
        assert_eq!(binding.symbol, None);
    }

    #[test]
    fn glob_import_records_the_whole_module() {
        let index = index(&["src/main.rs", "src/prelude.rs"]);
        let mut imports = HashMap::new();
        imports.insert(
            "src/main.rs".to_string(),
            vec![ImportRef {
                source: "crate::prelude".into(),
                name: None,
                alias: None,
                glob: true,
                namespace: false,
            }],
        );
        let bindings = Bindings::build(&index, &imports);
        assert_eq!(bindings.glob_targets("src/main.rs"), ["src/prelude.rs"]);
    }
}
