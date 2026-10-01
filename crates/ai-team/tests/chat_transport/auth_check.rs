use super::fixture::Fixture;
use anyhow::Context;
use std::{process::Stdio, time::Duration};

pub(super) async fn run(f: &Fixture) -> anyhow::Result<()> {
    // Unlike sessions/catalogue queries, Pi's auth parser/runtime does not load
    // extensions. Exercise that claim with an unknown provider, never an account.
    // The isolated settings explicitly include the adapter and its global sentinel.
    let child = tokio::process::Command::new(&f.pi)
        .args([
            "auth",
            "check",
            "--provider",
            "ai-team-fixture-unknown",
            "--json",
            "--no-refresh",
        ])
        .current_dir(&f.repo)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let output = tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
        .await
        .context("isolated auth-check timed out")??;
    let answer: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(answer["provider"], "ai-team-fixture-unknown");
    assert_eq!(answer["reason"], "provider_not_found");
    assert!(
        !f.root.join("unexpected-tool").exists(),
        "auth-check loaded a global MCP or credential helper"
    );
    assert!(
        !f.root.join("network-denied").exists(),
        "auth-check attempted network access"
    );
    Ok(())
}
