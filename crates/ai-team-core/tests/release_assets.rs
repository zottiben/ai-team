//! The asset names are a contract between three files that never see each other.
//!
//! `release.yml` writes them, `install.sh` downloads them, and `update.rs` fetches them
//! when somebody presses Update. Nothing links the three, and getting one wrong is a 404
//! at the exact moment a person is trying to install or update - which is the worst
//! moment to discover a typo, because it happens on their machine and not on ours.
//!
//! So the strings are read out of the real files and compared.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/<crate> is two below the root")
        .to_path_buf()
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The shape of an asset name with the version and architecture blanked out.
///
/// Compared as a shape rather than literally, because each file spells the substitution
/// differently - `${VERSION}`, `${num}`, and a Rust format argument all mean the same
/// thing and none of them match as text.
fn shape(name: &str) -> String {
    let mut out = String::new();
    let mut chars = name.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            // `${VERSION}`, `${num}`, `${{ matrix.arch }}`, `{version}`.
            '$' if chars.peek() == Some(&'{') => {
                for skipped in chars.by_ref() {
                    if skipped == '}' {
                        break;
                    }
                }
                // A `${{ ... }}` leaves one brace behind.
                if chars.peek() == Some(&'}') {
                    chars.next();
                }
                out.push('*');
            }
            '{' => {
                for skipped in chars.by_ref() {
                    if skipped == '}' {
                        break;
                    }
                }
                out.push('*');
            }
            other => out.push(other),
        }
    }
    // `-linux-x86_64` and `-linux-*` are the same shape once the architecture is blanked.
    out.replace("x86_64", "*")
        .replace("aarch64", "*")
        .replace("**", "*")
}

#[test]
fn every_file_agrees_on_the_macos_archive_name() {
    let expected = shape("ai-team-v${VERSION}-macos-universal.tar.gz");

    // What the workflow writes.
    assert!(
        read(".github/workflows/release.yml")
            .lines()
            .any(|line| line.contains("tar czf") && shape(line).contains(&expected)),
        "release.yml does not build {expected}"
    );

    // What the installer downloads.
    assert!(
        read("install/install.sh")
            .lines()
            .any(|line| shape(line).contains(&expected)),
        "install.sh does not download {expected}"
    );

    // What self-update fetches.
    assert!(
        read("crates/ai-team-core/src/update.rs")
            .lines()
            .any(|line| shape(line).contains(&expected)),
        "update.rs does not fetch {expected}"
    );
}

#[test]
fn every_file_agrees_on_the_linux_archive_name() {
    let expected = shape("ai-team-v${VERSION}-linux-${ARCH}.tar.gz");

    for (file, what) in [
        (".github/workflows/release.yml", "build"),
        ("install/install.sh", "download"),
        ("crates/ai-team-core/src/update.rs", "fetch"),
    ] {
        assert!(
            read(file)
                .lines()
                .any(|line| shape(line).contains(&expected)),
            "{file} does not {what} {expected}"
        );
    }
}

#[test]
fn the_architectures_the_workflow_builds_are_the_ones_update_asks_for() {
    // A release that ships `aarch64` while update.rs asks for `arm64` installs on nobody's
    // Apple Silicon Linux box, and the failure looks like a missing release.
    let workflow = read(".github/workflows/release.yml");
    let update = read("crates/ai-team-core/src/update.rs");

    for arch in ["x86_64", "aarch64"] {
        assert!(
            workflow.contains(&format!("arch: {arch}")),
            "release.yml does not build {arch}"
        );
        assert!(
            update.contains(&format!("\"{arch}\"")),
            "update.rs does not handle {arch}"
        );
    }
}

#[test]
fn the_installer_and_self_update_write_and_read_the_same_marker() {
    // The marker is what stops a self-update rebuilding an old clone over a release
    // install and silently downgrading somebody. It only works if both ends spell it the
    // same way, in the same place.
    let installer = read("install/install.sh");
    let update = read("crates/ai-team-core/src/update.rs");

    assert!(
        installer.contains(".ai-team/install-method"),
        "install.sh does not write the marker"
    );
    assert!(
        update.contains("install-method"),
        "update.rs does not read the marker"
    );
    for value in ["release", "source"] {
        assert!(
            installer.contains(&format!("printf '{value}\\n'")),
            "install.sh never writes `{value}`"
        );
        assert!(
            update.contains(&format!("\"{value}\"")),
            "update.rs does not understand `{value}`"
        );
    }
}

#[test]
fn the_installer_is_valid_shell() {
    // It is piped straight into `sh` on somebody else's machine. A syntax error there is
    // not a test failure, it is a broken install page.
    let status = std::process::Command::new("sh")
        .arg("-n")
        .arg(repo_root().join("install/install.sh"))
        .status();
    match status {
        Ok(status) => assert!(status.success(), "install.sh is not valid sh"),
        Err(error) => panic!("could not run sh: {error}"),
    }
}
