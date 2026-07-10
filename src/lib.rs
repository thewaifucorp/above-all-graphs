#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `AboveAllGraphs` (`aag`) — code knowledge graph, always fresh, MCP-native.
//!
//! Library surface behind the `aag` binary. See `SPEC.md` at the repo root
//! for the full design contract.

pub mod bigbang;
pub mod cli;
pub mod docs;
pub mod error;
pub mod explore;
pub mod export;
pub mod hook;
pub mod hub;
pub mod impact;
pub mod install;
pub mod mcp;
pub mod parse;
pub mod refactor;
pub mod resolve;
pub mod storage;
pub mod sync;
pub mod watch;
pub mod workspaces;

pub use error::{Error, Result};
