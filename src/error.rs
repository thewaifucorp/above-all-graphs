//! Typed errors for the `aag` domain layer.

use std::path::PathBuf;

/// Crate-wide result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Domain-level failures raised by `aag`'s core operations.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A required directory could not be created.
    #[error("failed to create `{path}`: {source}")]
    CreateDir {
        /// Path that failed to be created.
        path: PathBuf,
        /// Underlying I/O failure.
        #[source]
        source: std::io::Error,
    },

    /// An existing index directory could not be removed for a forced rebuild.
    #[error("failed to remove `{path}`: {source}")]
    RemoveDir {
        /// Path that failed to be removed.
        path: PathBuf,
        /// Underlying I/O failure.
        #[source]
        source: std::io::Error,
    },

    /// A command was invoked that has no implementation yet.
    #[error("`{command}` is not implemented yet")]
    NotImplemented {
        /// Name of the unimplemented subcommand.
        command: &'static str,
    },

    /// A graph storage operation failed.
    #[error("failed to {context}: {source}")]
    Storage {
        /// What operation was being attempted.
        context: &'static str,
        /// Underlying `SQLite` failure.
        #[source]
        source: rusqlite::Error,
    },

    /// A source file could not be parsed.
    #[error("failed to parse `{file}`: {reason}")]
    Parse {
        /// File that failed to parse.
        file: String,
        /// Why parsing failed.
        reason: String,
    },

    /// A file could not be read while indexing.
    #[error("failed to read `{path}`: {source}")]
    Io {
        /// Path that failed to be read.
        path: PathBuf,
        /// Underlying I/O failure.
        #[source]
        source: std::io::Error,
    },

    /// A query command was run against a repo with no index yet.
    #[error("no index found at `{path}` — run `aag bigbang` first")]
    IndexMissing {
        /// Repo root that has no `.aag/` index.
        path: PathBuf,
    },

    /// A query command was given a symbol name that doesn't exist in the graph.
    #[error("no symbol named `{name}` found in the index")]
    SymbolNotFound {
        /// The name that was looked up.
        name: String,
    },

    /// An export artifact could not be written.
    #[error("failed to write `{path}`: {source}")]
    Write {
        /// Path that failed to be written.
        path: PathBuf,
        /// Underlying I/O failure.
        #[source]
        source: std::io::Error,
    },

    /// `aag describe` was pointed at a node that exists but isn't a doc.
    #[error("`{name}` is not a doc node")]
    NotADoc {
        /// The name that was looked up.
        name: String,
    },

    /// `aag rename` was given a name shared by more than one symbol — safe
    /// rename requires an unambiguous target.
    #[error(
        "`{name}` is ambiguous: {count} symbols share that name — rename requires a unique target"
    )]
    AmbiguousRename {
        /// The name that was looked up.
        name: String,
        /// How many distinct symbols share it.
        count: usize,
    },
}
