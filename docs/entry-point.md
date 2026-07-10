---
wiki: src/main.rs
---

## Binary entry point

`main` owns process wiring only — argument parsing via `Cli::parse`, `tracing`
setup, and translating the parsed `Command` into a call into the `aag`
library. All domain logic lives in the library modules (`bigbang`, `sync`,
`install`, `hook`, `explore`, `impact`, `mcp`, `docs`, `refactor`,
`workspaces`); `main` just matches on the `Command` enum from `cli.rs` and
dispatches.

`init_tracing` picks the log format: human-readable `fmt` output in dev, or
JSON when the `AAG_LOG_FORMAT` environment variable is set to `json`. Level
filtering comes from `RUST_LOG` (via `EnvFilter::try_from_default_env`),
defaulting to `info`.

Two dispatch details worth noting:

- `Command::Bigbang` resolves `obsidian_dir`: if `--obsidian-dir` wasn't
  given but `--obsidian` was, it derives `<path>/.aag/obsidian` before
  calling `bigbang::run`.
- `Command::Hook` maps the CLI's `HookEvent` variants (`PreEdit`,
  `PostEdit`, `SessionStart`) onto the library's `hook::Event` and always
  reads the hook payload from stdin.

The crate root, `lib.rs`, just declares the module tree and re-exports
`error::{Error, Result}` as the crate-wide result alias — see
[errors](errors.md). `#![forbid(unsafe_code)]` and `#![warn(missing_docs)]`
are set there, applying to the whole library surface.
