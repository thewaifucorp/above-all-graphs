#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `AboveAllGraphs` (`aag`) — code knowledge graph, always fresh, MCP-native.
//!
//! Library surface behind the `aag` binary. See `SPEC.md` at the repo root
//! for the full design contract.

pub mod analysis;
pub mod api;
pub mod areas;
mod artifacts;
pub mod bigbang;
pub mod bindings;
pub mod cli;
pub mod database;
pub mod docs;
pub mod error;
pub mod explore;
pub mod export;
pub mod extract;
pub mod federation;
pub mod flow;
pub mod hook;
pub mod hub;
pub mod impact;
pub mod install;
pub mod lock;
pub mod mcp;
pub mod memory;
mod openapi;
pub mod parse;
pub mod pr;
pub mod protocol;
pub mod query;
pub mod refactor;
pub mod refs;
pub mod resolve;
pub mod semantic;
pub mod storage;
pub mod sync;
pub mod toolchain;
pub mod transport;
pub mod watch;
pub mod workspaces;

pub use error::{Error, Result};
