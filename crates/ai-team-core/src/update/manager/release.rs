use crate::{Error, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub(super) enum ReleaseSource {
    Github,
    /// An explicitly bounded fixture, not an alternate production release server.
    Isolated(PathBuf),
}

impl ReleaseSource {
    pub(super) async fn latest(&self) -> Option<String> {
        match self {
            Self::Github => super::super::latest().await,
            Self::Isolated(root) => {
                let text = fs::read_to_string(root.join("releases/latest")).ok()?;
                super::super::release_version(&format!(
                    "https://github.com/{}/releases/tag/v{}",
                    super::super::REPO,
                    text.trim()
                ))
            }
        }
    }

    pub(super) fn download(&self, version: &str, name: &str, destination: &Path) -> Result<()> {
        match self {
            Self::Github => tokio::runtime::Handle::current().block_on(super::super::curl(
                &format!(
                    "https://github.com/{}/releases/download/v{version}/{name}",
                    super::super::REPO
                ),
                destination,
            )),
            Self::Isolated(root) => {
                fs::copy(root.join("releases").join(name), destination)?;
                Ok(())
            }
        }
    }

    pub(super) fn native_signature(&self, app: &Path) -> Result<()> {
        if matches!(self, Self::Isolated(_)) {
            return Ok(());
        }
        #[cfg(target_os = "macos")]
        {
            let mut command = std::process::Command::new("/usr/bin/codesign");
            crate::pi::strip_metered_std_env(&mut command);
            if command
                .args(["--verify", "--deep", "--strict", "--all-architectures"])
                .arg(app)
                .stdin(std::process::Stdio::null())
                .status()?
                .success()
            {
                return Ok(());
            }
        }
        Err(Error::invalid(format!(
            "could not verify the native signature of {}",
            app.display()
        )))
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::ReleaseSource;

    #[test]
    fn native_integrity_is_checked_without_executing_the_program() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("ait");
        std::fs::write(&program, "not a signed release").unwrap();
        assert!(ReleaseSource::Github.native_signature(&program).is_err());
        std::fs::copy(std::env::current_exe().unwrap(), &program).unwrap();
        assert!(std::process::Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-"])
            .arg(&program)
            .status()
            .unwrap()
            .success());
        assert!(ReleaseSource::Github.native_signature(&program).is_ok());
    }
}
