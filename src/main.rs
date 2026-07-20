//! Binary entry point. Owns process wiring (tracing init, arg parsing,
//! error-to-exit-code translation) — domain logic lives in the `aag` library.

use aag::cli::{Cli, Command, GroupCommand};
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
        Command::Group { command } => handle_group(command)?,
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
        Command::Communities { query, path } => {
            println!("{}", aag::analysis::communities_format(&path, &query)?);
        }
        Command::Processes { query, path } => {
            println!("{}", aag::analysis::processes_format(&path, &query)?);
        }
        Command::Status { path } => print_status(&path)?,
        Command::Embeddings { path } => {
            let count = aag::semantic::build(&path)?;
            println!("embedded {count} nodes");
        }
        Command::Mcp {
            path,
            transport,
            port,
            api_key,
        } => {
            run_mcp(&path, &transport, port, api_key.as_deref())?;
        }
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
        Command::Export { path, output } => {
            let written = aag::protocol::run_export(&path, output.as_deref())?;
            println!("exported {}", written.display());
        }
        Command::Validate { manifest } => {
            aag::protocol::run_validate(&manifest)?;
            println!("valid {}", manifest.display());
        }
    }

    Ok(())
}

fn run_mcp(
    path: &std::path::Path,
    transport: &str,
    port: u16,
    api_key: Option<&str>,
) -> anyhow::Result<()> {
    if transport == "http" {
        aag::mcp::run_http(path, port, api_key)?;
    } else {
        aag::mcp::run(path)?;
    }
    Ok(())
}

fn print_status(path: &std::path::Path) -> anyhow::Result<()> {
    let graph = aag::storage::Graph::open_existing(path)?;
    let nodes = graph.all_nodes()?;
    let edges = graph.all_edges()?;
    println!(
        "indexed {} nodes, {} edges, {} communities, {} processes",
        nodes.len(),
        edges.len(),
        aag::analysis::communities(&nodes, &edges).len(),
        aag::analysis::processes(&nodes, &edges).len()
    );
    Ok(())
}

fn handle_group(command: GroupCommand) -> anyhow::Result<()> {
    let output = match command {
        GroupCommand::Create { name } => aag::federation::create(&name)?,
        GroupCommand::Add { name, repository } => aag::federation::add(&name, &repository)?,
        GroupCommand::Remove { name, repository } => aag::federation::remove(&name, &repository)?,
        GroupCommand::List { name } => aag::federation::list_group(name.as_deref())?,
        GroupCommand::Query { name, query } => aag::federation::query_group(&name, &query)?,
        GroupCommand::Status { name } => aag::federation::status_group(&name)?,
        GroupCommand::Contracts { name } => aag::federation::contracts_group(&name)?,
        GroupCommand::Sync { name } => aag::federation::sync_group(&name)?,
    };
    println!("{output}");
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
