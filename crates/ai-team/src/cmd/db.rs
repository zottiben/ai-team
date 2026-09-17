//! `ait db` - the database, for people who would rather look at it directly.
//!
//! The demo for M0-S2 is that opening the file explains itself, so this exists to make
//! the file easy to reach rather than to wrap it in anything.

use anyhow::{Context, Result};

use ai_team_core::Store;

use crate::cli::DbCommand;

pub(crate) fn run(command: DbCommand) -> Result<()> {
    let path = ai_team_core::default_db_path()?;

    match command {
        DbCommand::Path => {
            println!("{}", path.display());
        }
        DbCommand::Open => {
            if !path.exists() {
                anyhow::bail!("no database at {} - run `ait init` first", path.display());
            }
            open(&path.to_string_lossy());
            println!("Opened {}", path.display());
        }
        DbCommand::Views => {
            let store =
                Store::open(&path).with_context(|| format!("opening {}", path.display()))?;
            println!("schema v{}", store.schema_version()?);
            for name in store.views()? {
                println!("  {name}");
            }
        }
    }
    Ok(())
}

fn open(target: &str) {
    let (program, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("open", &[])
    } else if cfg!(target_os = "windows") {
        ("cmd", &["/C", "start", ""])
    } else {
        ("xdg-open", &[])
    };
    let _ = std::process::Command::new(program)
        .args(args)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}
