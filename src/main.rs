//! Binary entry point. Owns process wiring (tracing init, arg parsing,
//! error-to-exit-code translation) — domain logic lives in the `aag` library.

use aag::cli::{Cli, Command};
use clap::Parser;

fn main() -> anyhow::Result<()> {
    init_tracing();

    let cli = Cli::parse();
    match cli.command {
        Command::Bigbang {
            path,
            force,
            no_viz,
            obsidian,
            obsidian_dir,
            no_install,
        } => {
            let obsidian_dir =
                obsidian_dir.or_else(|| obsidian.then(|| path.join(".aag").join("obsidian")));
            aag::bigbang::run(
                &path,
                &aag::bigbang::Options {
                    force,
                    no_viz,
                    obsidian_dir,
                    no_install,
                },
            )?;
        }
        Command::Sync { path, file, no_viz } => {
            aag::sync::run(&path, file.as_deref(), no_viz)?;
        }
        Command::Install { path, force } => {
            aag::install::run(&path, force)?;
        }
        Command::Workspaces => aag::workspaces::list()?,
        Command::Ui { port, no_open } => aag::hub::run(port, no_open)?,
        Command::Uninstall { path } => aag::install::uninstall(&path)?,
        Command::Hook { event } => {
            let (path, hook_event) = match event {
                aag::cli::HookEvent::PreEdit { path } => (path, aag::hook::Event::PreEdit),
                aag::cli::HookEvent::PostEdit { path } => (path, aag::hook::Event::PostEdit),
                aag::cli::HookEvent::SessionStart { path } => {
                    (path, aag::hook::Event::SessionStart)
                }
            };
            aag::hook::run(&path, hook_event, &mut std::io::stdin().lock());
        }
        Command::Explore { query, path } => aag::explore::run(&path, &query)?,
        Command::Impact { symbol, path } => aag::impact::run(&path, &symbol)?,
        Command::Mcp { path } => aag::mcp::run(&path)?,
        Command::Describe {
            doc,
            description,
            path,
        } => aag::docs::run(&path, &doc, &description)?,
        Command::Rename {
            old_name,
            new_name,
            write,
            path,
        } => {
            aag::refactor::rename_run(&path, &old_name, &new_name, write)?;
        }
        Command::Affected { path, .. } => {
            let stdin = std::io::stdin();
            aag::refactor::affected_run(&path, stdin.lock())?;
        }
    }

    Ok(())
}

/// Console output in dev, JSON when `AAG_LOG_FORMAT=json` (house rule: dev vs prod format).
fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let json = std::env::var("AAG_LOG_FORMAT").is_ok_and(|value| value == "json");

    if json {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
}
