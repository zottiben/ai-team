//! The project's own checks, discovered rather than assumed.
//!
//! "Done" means this project's gates pass, and every project spells them differently.
//! Hardcoding `cargo test` would be right here and wrong in the next repo, and a verifier
//! that runs the wrong command is worse than no verifier: it reports green for a check
//! that never ran.
//!
//! Discovery reads the manifests the project already has, because those are what CI runs
//! too. Deliberately not `.github/workflows/*.yml`: parsing it needs a YAML dependency
//! to learn what the manifests already say exactly, and a half-parsed workflow is how a
//! gate gets silently dropped.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;

use crate::error::Result;

/// A gate runs for at most this long. A hung test suite must not hold a worktree and an
/// turn open indefinitely.
const GATE_TIMEOUT: Duration = Duration::from_mins(15);

/// What a gate is for. Ordered cheap to expensive, which is also the order to run them:
/// a formatting failure should not wait behind a ten-minute test suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GateKind {
    Format,
    Typecheck,
    Lint,
    Test,
    Build,
}

impl GateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            GateKind::Format => "format",
            GateKind::Typecheck => "typecheck",
            GateKind::Lint => "lint",
            GateKind::Test => "test",
            GateKind::Build => "build",
        }
    }
}

/// One check, as this project spells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gate {
    pub kind: GateKind,
    /// Relative to the worktree root, so a gate in `ui/` runs where its manifest is.
    pub dir: String,
    pub program: String,
    pub args: Vec<String>,
}

impl Gate {
    pub fn command(&self) -> String {
        let command = format!("{} {}", self.program, self.args.join(" "));
        if self.dir == "." {
            command.trim_end().to_string()
        } else {
            format!("({}) {}", self.dir, command.trim_end())
        }
    }
}

/// What running one gate produced.
#[derive(Debug, Clone)]
pub struct GateResult {
    pub gate: Gate,
    pub passed: bool,
    /// Combined stdout and stderr, tail-truncated: a failure's last lines are the ones
    /// that say what went wrong, and the whole log would not fit in a prompt.
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DependencySetup {
    dir: String,
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
}

impl DependencySetup {
    fn command(&self) -> String {
        format!("{} {}", self.program, self.args.join(" "))
            .trim_end()
            .to_string()
    }
}

#[derive(Debug, Deserialize)]
struct PackageJson {
    #[serde(default)]
    scripts: std::collections::BTreeMap<String, String>,
}

/// Find this project's checks, cheapest first.
///
/// A worktree with no recognised manifest yields nothing, and that is reported as "no
/// gates" rather than as success - a verifier that finds nothing to run has verified
/// nothing.
pub(crate) fn discover_dependency_setup(worktree: &Path) -> Vec<DependencySetup> {
    let mut steps = Vec::new();
    if worktree.join("composer.json").exists()
        && worktree.join("composer.lock").exists()
        && !worktree.join("vendor/autoload.php").exists()
    {
        steps.push(DependencySetup {
            dir: ".".into(),
            program: "composer".into(),
            args: [
                "install",
                "--no-interaction",
                "--prefer-dist",
                "--no-progress",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        });
    }

    for dir in [".", "ui", "web", "frontend", "app"] {
        let root = worktree.join(dir);
        if !root.join("package.json").exists() || root.join("node_modules").exists() {
            continue;
        }
        let command = if root.join("bun.lock").exists() || root.join("bun.lockb").exists() {
            Some(("bun", vec!["install", "--frozen-lockfile"]))
        } else if root.join("pnpm-lock.yaml").exists() {
            Some(("pnpm", vec!["install", "--frozen-lockfile"]))
        } else if root.join("yarn.lock").exists() {
            let immutable = root.join(".yarnrc.yml").exists();
            Some((
                "yarn",
                vec![
                    "install",
                    if immutable {
                        "--immutable"
                    } else {
                        "--frozen-lockfile"
                    },
                ],
            ))
        } else if root.join("package-lock.json").exists() {
            Some(("npm", vec!["ci"]))
        } else {
            None
        };
        if let Some((program, args)) = command {
            steps.push(DependencySetup {
                dir: dir.into(),
                program: program.into(),
                args: args.into_iter().map(str::to_string).collect(),
            });
        }
    }
    steps
}

/// Install only from committed lockfiles, before a paid model turn starts. Package
/// managers retain their ordinary shared caches, while ignored dependency directories
/// remain in awt's pooled checkout across `git clean -fd` returns.
pub(crate) async fn prepare_dependencies(worktree: &Path) -> Result<Vec<String>> {
    let steps = discover_dependency_setup(worktree);
    let mut completed = Vec::new();
    for step in steps {
        let command = step.command();
        let output = Command::new(&step.program)
            .args(&step.args)
            .current_dir(worktree.join(&step.dir))
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output();
        let output = match tokio::time::timeout(GATE_TIMEOUT, output).await {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => {
                return Err(crate::Error::invalid(format!(
                    "could not prepare dependencies with `{command}`: {error}"
                )))
            }
            Err(_) => {
                return Err(crate::Error::invalid(format!(
                    "dependency setup `{command}` was still running after {} minutes",
                    GATE_TIMEOUT.as_secs() / 60
                )))
            }
        };
        if !output.status.success() {
            let mut detail = String::from_utf8_lossy(&output.stdout).into_owned();
            detail.push_str(&String::from_utf8_lossy(&output.stderr));
            return Err(crate::Error::invalid(format!(
                "dependency setup `{command}` failed:\n{}",
                tail(&detail, 6_000)
            )));
        }
        completed.push(command);
    }
    Ok(completed)
}

pub fn discover_gates(worktree: &Path) -> Vec<Gate> {
    let mut gates = Vec::new();
    if worktree.join("Cargo.toml").exists() {
        gates.extend(cargo_gates("."));
    }
    // Node projects nest: the frontend of a Rust workspace is normally its own package.
    for dir in [".", "ui", "web", "frontend", "app"] {
        let manifest = worktree.join(dir).join("package.json");
        if manifest.exists() {
            gates.extend(npm_gates(&manifest, dir));
        }
    }
    gates.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.dir.cmp(&b.dir)));
    gates
}

fn cargo_gates(dir: &str) -> Vec<Gate> {
    let gate = |kind, args: &[&str]| Gate {
        kind,
        dir: dir.to_string(),
        program: "cargo".to_string(),
        args: args.iter().map(ToString::to_string).collect(),
    };
    vec![
        gate(GateKind::Format, &["fmt", "--all", "--check"]),
        // `--all-targets` so a broken test file is a lint failure here rather than a
        // surprise when the test gate compiles it.
        gate(
            GateKind::Lint,
            &["clippy", "--all-targets", "--", "-D", "warnings"],
        ),
        gate(GateKind::Test, &["test"]),
    ]
}

/// Only the scripts a project actually declares. Running `npm test` against a package
/// with no test script reports a failure the project never asked for.
fn npm_gates(manifest: &Path, dir: &str) -> Vec<Gate> {
    let Ok(text) = std::fs::read_to_string(manifest) else {
        return Vec::new();
    };
    let Ok(package) = serde_json::from_str::<PackageJson>(&text) else {
        return Vec::new();
    };

    let mut gates = Vec::new();
    for (script, kind) in [
        ("typecheck", GateKind::Typecheck),
        ("lint", GateKind::Lint),
        ("test", GateKind::Test),
        ("build", GateKind::Build),
    ] {
        if package.scripts.contains_key(script) {
            gates.push(Gate {
                kind,
                dir: dir.to_string(),
                program: "npm".to_string(),
                args: vec!["run".to_string(), script.to_string()],
            });
        }
    }
    gates
}

/// Run the gates in order, stopping at the first failure.
///
/// Stopping is the point: the first failure is the one to fix, and running the rest
/// produces a wall of output where the cause is no longer obvious.
pub async fn run_gates(worktree: &Path, gates: &[Gate]) -> Result<Vec<GateResult>> {
    let mut results = Vec::new();
    for gate in gates {
        let result = run_one(worktree, gate).await?;
        let failed = !result.passed;
        results.push(result);
        if failed {
            break;
        }
    }
    Ok(results)
}

async fn run_one(worktree: &Path, gate: &Gate) -> Result<GateResult> {
    let dir = worktree.join(&gate.dir);
    let output = Command::new(&gate.program)
        .args(&gate.args)
        .current_dir(&dir)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();

    let output = match tokio::time::timeout(GATE_TIMEOUT, output).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            // A missing toolchain is a real answer: the gate did not pass, and saying
            // why beats reporting a green nobody earned.
            return Ok(GateResult {
                gate: gate.clone(),
                passed: false,
                output: format!("could not run `{}`: {error}", gate.command()),
            });
        }
        Err(_) => {
            return Ok(GateResult {
                gate: gate.clone(),
                passed: false,
                output: format!(
                    "`{}` was still running after {} minutes",
                    gate.command(),
                    GATE_TIMEOUT.as_secs() / 60
                ),
            })
        }
    };

    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(GateResult {
        gate: gate.clone(),
        passed: output.status.success(),
        output: tail(&combined, 6_000),
    })
}

/// Keep the end of a log. Compilers and test runners put the summary last, and the head
/// of a build log is the part nobody needs.
fn tail(text: &str, max: usize) -> String {
    let text = text.trim_end();
    if text.len() <= max {
        return text.to_string();
    }
    let cut = text.len() - max;
    // Start at a line boundary, so the output does not open mid-token.
    let start = text[cut..]
        .find('\n')
        .map_or(cut, |offset| cut + offset + 1);
    format!(
        "…{} earlier lines…\n{}",
        text[..start].lines().count(),
        &text[start..]
    )
}

/// A report a model can act on: which gate, and what it actually printed.
pub fn evidence(results: &[GateResult]) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    for result in results {
        let _ = writeln!(
            out,
            "{} {} `{}`",
            if result.passed { "PASS" } else { "FAIL" },
            result.gate.kind.as_str(),
            result.gate.command()
        );
    }
    if let Some(failed) = results.iter().find(|result| !result.passed) {
        let _ = write!(
            out,
            "\nOutput of the failing gate (`{}`):\n\n{}\n",
            failed.gate.command(),
            failed.output
        );
    }
    out
}

pub fn all_passed(results: &[GateResult]) -> bool {
    !results.is_empty() && results.iter().all(|result| result.passed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, path: &str, contents: &str) {
        let full = dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, contents).unwrap();
    }

    #[test]
    fn a_rust_project_gets_its_own_checks_cheapest_first() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Cargo.toml", "[package]\nname = \"x\"\n");

        let gates = discover_gates(dir.path());
        assert_eq!(
            gates.iter().map(|g| g.kind).collect::<Vec<_>>(),
            [GateKind::Format, GateKind::Lint, GateKind::Test],
            "format before lint before test, so the cheap failure reports first"
        );
        assert_eq!(gates[0].command(), "cargo fmt --all --check");
    }

    #[test]
    fn dependency_bootstrap_is_locked_composer_first_and_skips_ready_installs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("composer.json"), "{}").unwrap();
        std::fs::write(dir.path().join("composer.lock"), "{}").unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        std::fs::write(dir.path().join("bun.lock"), "").unwrap();

        let steps = discover_dependency_setup(dir.path());
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].program, "composer");
        assert_eq!(steps[1].program, "bun");
        assert!(steps[0].args.contains(&"--no-interaction".into()));
        assert!(steps[1].args.contains(&"--frozen-lockfile".into()));

        std::fs::create_dir_all(dir.path().join("vendor")).unwrap();
        std::fs::write(dir.path().join("vendor/autoload.php"), "ready").unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules")).unwrap();
        assert!(discover_dependency_setup(dir.path()).is_empty());
    }

    #[test]
    fn each_javascript_lockfile_uses_its_own_package_manager() {
        for (lockfile, program, flag) in [
            ("package-lock.json", "npm", "ci"),
            ("pnpm-lock.yaml", "pnpm", "--frozen-lockfile"),
            ("yarn.lock", "yarn", "--frozen-lockfile"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("package.json"), "{}").unwrap();
            std::fs::write(dir.path().join(lockfile), "").unwrap();
            let steps = discover_dependency_setup(dir.path());
            assert_eq!(steps.len(), 1, "{lockfile}");
            assert_eq!(steps[0].program, program, "{lockfile}");
            assert!(steps[0].args.contains(&flag.into()), "{lockfile}");
        }
    }

    #[test]
    fn only_the_npm_scripts_a_project_actually_declares_become_gates() {
        // Running `npm run lint` where no lint script exists fails for a reason the
        // project never asked about, and that failure would be blamed on the agent.
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"scripts":{"typecheck":"tsc","build":"vite build","dev":"vite"}}"#,
        );

        let gates = discover_gates(dir.path());
        assert_eq!(
            gates.iter().map(|g| g.kind).collect::<Vec<_>>(),
            [GateKind::Typecheck, GateKind::Build]
        );
        assert!(gates.iter().all(|g| g.program == "npm"));
        // `dev` is not a check, and running it would hang forever.
        assert!(!gates.iter().any(|g| g.args.contains(&"dev".to_string())));
    }

    #[test]
    fn a_nested_frontend_is_found_and_runs_where_its_manifest_is() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Cargo.toml", "[package]\nname = \"x\"\n");
        write(
            dir.path(),
            "ui/package.json",
            r#"{"scripts":{"test":"vitest"}}"#,
        );

        let gates = discover_gates(dir.path());
        let ui = gates
            .iter()
            .find(|g| g.program == "npm")
            .expect("the frontend's own checks count too");
        assert_eq!(ui.dir, "ui");
        assert_eq!(ui.command(), "(ui) npm run test");
    }

    #[test]
    fn a_project_with_no_manifest_reports_no_gates_rather_than_success() {
        let dir = tempfile::tempdir().unwrap();
        let gates = discover_gates(dir.path());
        assert!(gates.is_empty());
        // The distinction that matters: nothing ran, so nothing passed.
        assert!(!all_passed(&[]));
    }

    #[tokio::test]
    async fn running_stops_at_the_first_failure_and_keeps_its_output() {
        let dir = tempfile::tempdir().unwrap();
        let gate = |kind, args: &[&str]| Gate {
            kind,
            dir: ".".into(),
            program: "sh".into(),
            args: args.iter().map(ToString::to_string).collect(),
        };
        let results = run_gates(
            dir.path(),
            &[
                gate(GateKind::Format, &["-c", "echo formatted"]),
                gate(GateKind::Lint, &["-c", "echo 'the lint broke' >&2; exit 1"]),
                gate(GateKind::Test, &["-c", "echo should-not-run"]),
            ],
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 2, "the third gate never ran");
        assert!(results[0].passed);
        assert!(!results[1].passed);
        assert!(results[1].output.contains("the lint broke"));
        assert!(!all_passed(&results));

        // The evidence names the gate and carries what it printed, which is the whole
        // point of handing it back to the maker.
        let evidence = evidence(&results);
        assert!(evidence.contains("PASS format"), "{evidence}");
        assert!(evidence.contains("FAIL lint"), "{evidence}");
        assert!(evidence.contains("the lint broke"), "{evidence}");
    }

    #[tokio::test]
    async fn a_gate_whose_tool_is_missing_fails_rather_than_passing_quietly() {
        let dir = tempfile::tempdir().unwrap();
        let results = run_gates(
            dir.path(),
            &[Gate {
                kind: GateKind::Test,
                dir: ".".into(),
                program: "definitely-not-a-real-program".into(),
                args: vec![],
            }],
        )
        .await
        .unwrap();
        assert!(!results[0].passed);
        assert!(
            results[0].output.contains("could not run"),
            "{:?}",
            results[0]
        );
    }

    #[test]
    fn a_long_log_keeps_its_end_where_the_error_is() {
        let long = (0..4_000)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let kept = tail(&format!("{long}\nerror: the actual problem"), 200);
        assert!(kept.contains("error: the actual problem"));
        assert!(kept.contains("earlier lines"), "{kept}");
        assert!(!kept.contains("line 0\n"));
    }
}
