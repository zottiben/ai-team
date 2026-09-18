//! `ait` - ai-team from the terminal.
//!
//! The CLI and the window are the same program looking at the same rows. Nothing here
//! keeps state: every command reads or writes through `ai-team-core`, which is what
//! stops the terminal and the window from ever disagreeing.

mod cli;
mod cmd;

use anyhow::Result;
use clap::Parser;

use cli::{Cli, Command};

fn main() {
    if let Err(err) = run() {
        // `{err:#}` prints the whole `anyhow` chain on one line. The context each
        // command attaches is the difference between "permission denied" and
        // "permission denied while starting the local server".
        eprintln!("ait: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        // Only the commands that need one pay for an async runtime. Starting a
        // multi-thread reactor to print a few paths would be silly.
        Command::Ui(args) => runtime()?.block_on(cmd::ui::run(args)),
        Command::Init(args) => cmd::init::run(args),
        Command::Ingest(args) => cmd::ingest::run(args),
        Command::Db(command) => cmd::db::run(command),
        Command::Team(command) => cmd::team::run(command),
        Command::Agents(command) => cmd::agents::run(command),
        Command::Run(args) => runtime()?.block_on(cmd::run::run(args)),
        // Today asks ai-planner for its open questions, so it needs the runtime.
        Command::Today => runtime()?.block_on(cmd::today::run()).map_err(Into::into),
        // Doctor probes the neighbours by running them, so it needs the runtime too.
        Command::Doctor => {
            runtime()?.block_on(cmd::doctor::run());
            Ok(())
        }
    }
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// clap panics on a malformed command definition, and it does it at parse time -
    /// which is to say, in front of the user. This is clap's own remedy.
    #[test]
    fn the_command_tree_is_well_formed() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
