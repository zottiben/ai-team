//! `ait update` - replace this binary with the latest release.

use ai_team_core::{Host, Method, Result, Step};

pub(crate) async fn run(check_only: bool) -> Result<()> {
    let available = ai_team_core::check_update().await;

    println!("installed  {}", available.current);
    match &available.latest {
        Some(latest) => println!("latest     {latest}"),
        None => println!("latest     could not check"),
    }
    println!(
        "installed by {}",
        match available.method {
            Method::Release => "the release installer",
            Method::Source => "cargo, from source",
            Method::Unknown => "something else",
        }
    );

    if let Some(blocked) = &available.blocked {
        println!("\n{blocked}");
        return Ok(());
    }
    if !available.can_update {
        println!("\nAlready up to date.");
        return Ok(());
    }

    let latest = available.latest.clone().unwrap_or_default();
    if check_only {
        println!("\n{latest} is available. Run `ait update` to install it.");
        return Ok(());
    }

    if available.method == Method::Source {
        // Rebuilding a clone is cargo's job and it needs the source tree, which this
        // binary has no reliable way to find - `cargo install` from the repository is the
        // honest instruction rather than a guess at where somebody keeps their checkout.
        println!("\nThis copy was built from source. Update it with:");
        println!("  cargo install --git https://github.com/zottiben/ai-team ai-team --locked");
        return Ok(());
    }

    // The path of the binary actually running, so an update replaces *this* one rather
    // than whichever `ait` happens to come first on PATH.
    let binary = std::env::current_exe()
        .map_err(|error| ai_team_core::Error::invalid(format!("where am I? {error}")))?;

    println!("\nUpdating to {latest}…");
    // This is the CLI saying so, not a guess from the path: the same updater also runs
    // inside the desktop app, where installing `ait` would replace the app with it.
    ai_team_core::apply_update(&latest, &binary, Host::Cli, |step| {
        println!(
            "  {}",
            match step {
                Step::Checking => "checking",
                Step::Downloading => "downloading",
                Step::Verifying => "verifying",
                Step::Replacing => "replacing",
                Step::Done => "done",
            }
        );
    })
    .await?;

    println!("\nUpdated to {latest}. Restart anything already running.");
    Ok(())
}
