//! One update service for the CLI and the desktop window.

use ai_team_core::{Host, Method, Result, UpdateManager};

pub(crate) async fn run(check_only: bool, inspect: bool) -> Result<()> {
    let updates = UpdateManager::discover(Host::Cli)?;
    if inspect {
        let result = updates.inspect().await?;
        println!("{}\nBackup: {}", result.detail, result.backup.display());
        return Ok(());
    }
    let status = updates.check(true).await;
    println!("running    {}", status.current);
    println!(
        "latest     {}",
        status.latest.as_deref().unwrap_or("could not check")
    );
    for target in &status.targets {
        println!(
            "{}  {}  {}",
            target.name,
            target.version.as_deref().unwrap_or("not installed"),
            target.path.display()
        );
    }
    if let Some(reason) = status.blocked {
        println!("\n{reason}");
        return if check_only {
            Ok(())
        } else {
            Err(ai_team_core::Error::invalid(reason))
        };
    }
    if !status.can_update {
        println!("\nAll listed programs are up to date.");
        if status.state == ai_team_core::UpdateState::Restart {
            println!("Restart AI Team to load the updated files.");
        }
        return Ok(());
    }
    if check_only {
        println!("\nRun `ait update` to update the listed programs together.");
        return Ok(());
    }
    if status.method == Method::Source {
        println!("\nReplacing the local build with the published release; no source rebuild.");
    }
    let version = status
        .latest
        .ok_or_else(|| ai_team_core::Error::invalid("no release was checked"))?;
    let approval = status
        .approval
        .ok_or_else(|| ai_team_core::Error::invalid("no installation was reviewed"))?;
    println!("\nUpdating listed programs to {version}… Active work will not be stopped.");
    let result = updates.apply(&version, &approval).await?;
    println!(
        "\nUpdated to {}. Backup: {}",
        result.version,
        result.backup.display()
    );
    for warning in result.warnings {
        eprintln!("{warning}");
    }
    println!("Save unsaved edits, quit and reopen AI Team. Existing windows keep running their old version until restarted.");
    Ok(())
}
