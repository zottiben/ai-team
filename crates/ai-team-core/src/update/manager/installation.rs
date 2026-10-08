use super::super::{Host, Method};
use super::{files, release::ReleaseSource, UpdateTarget, Updated};
use crate::{Error, Result, Store};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[cfg(test)]
mod tests;

/// Trusted host configuration, never accepted from an HTTP request.
#[derive(Debug, Clone)]
pub struct UpdateInstallation {
    pub host: Host,
    pub cli: Option<PathBuf>,
    pub desktop: Option<PathBuf>,
    pub home: PathBuf,
    pub data_dir: PathBuf,
    pub database: Option<PathBuf>,
    pub config_dir: Option<PathBuf>,
    pub method: Method,
    pub blocked: Option<String>,
}

impl UpdateInstallation {
    pub fn discover(host: Host) -> Result<Self> {
        super::discovery::discover(host)
    }

    pub(super) fn require_inside(&self, root: &Path) -> Result<()> {
        for path in [
            self.cli.as_ref(),
            self.desktop.as_ref(),
            Some(&self.home),
            Some(&self.data_dir),
            self.database.as_ref(),
            self.config_dir.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if !files::resolved(path)?.starts_with(root) {
                return Err(Error::invalid(
                    "an update fixture reaches outside its isolated root",
                ));
            }
        }
        if !root.join("releases").canonicalize()?.starts_with(root) {
            return Err(Error::invalid(
                "fixture releases escape their isolated root",
            ));
        }
        Ok(())
    }

    pub(super) fn pending(&self) -> PathBuf {
        self.data_dir.join(".app-update-pending.json")
    }

    pub(super) async fn targets(&self) -> Result<Vec<UpdateTarget>> {
        let mut targets = Vec::new();
        if let Some(cli) = &self.cli {
            targets.push(UpdateTarget {
                name: "CLI".into(),
                path: cli.clone(),
                version: if self.blocked.is_none() {
                    super::cli_version(cli).await?
                } else {
                    None
                },
                fingerprint: String::new(),
            });
        }
        if let Some(app) = &self.desktop {
            if files::plist(app, "CFBundleIdentifier")? != "dev.zottiben.ai-team" {
                return Err(Error::invalid(
                    "the companion app is not AI Team; it will not be replaced",
                ));
            }
            let version = files::plist(app, "CFBundleShortVersionString")?;
            if super::super::version(&version).is_none() {
                return Err(Error::invalid(
                    "the installed desktop version cannot be read",
                ));
            }
            targets.push(UpdateTarget {
                name: "Desktop app".into(),
                path: app.clone(),
                version: Some(version),
                fingerprint: String::new(),
            });
        }
        if targets.is_empty() {
            return Err(Error::invalid("no installed AI Team programs were found"));
        }
        tokio::task::spawn_blocking(move || {
            for target in &mut targets {
                target.fingerprint = files::fingerprint(&target.path)?;
            }
            Ok(targets)
        })
        .await
        .map_err(|e| Error::invalid(format!("could not inspect installed programs: {e}")))?
    }

    pub(super) fn install(
        &self,
        version: &str,
        targets: &[UpdateTarget],
        source: &ReleaseSource,
    ) -> Result<Updated> {
        // Opening/migrating stores and ordinary writes acquire the shared side. The
        // exclusive side stays held through download, final admission and both swaps.
        if let Some(db) = &self.database {
            if files::resolved(db)?.parent() != Some(files::resolved(&self.data_dir)?.as_path()) {
                return Err(Error::invalid("the update and database admission fences must share their canonical state directory"));
            }
            if db.exists() {
                Store::check_update_schema(db)?;
            }
        }
        let mut store = self
            .database
            .as_ref()
            .filter(|p| p.exists())
            .map(|db| Store::open(db))
            .transpose()?;
        files::private_dir(&self.data_dir)?;
        let _lock = super::super::fence::exclusive(&self.data_dir)?;
        if self.pending().exists() {
            return Err(Error::invalid(
                "a previous update needs inspection before retrying",
            ));
        }
        Self::unchanged(targets)?;
        if store.is_none() && self.database.as_ref().is_some_and(|p| p.exists()) {
            return Err(Error::invalid("application data was initialized while preparing the update; check and approve again"));
        }
        if let Some(store) = &store {
            store.check_update_idle()?;
        }
        let jobs = self.data_dir.join("updates");
        files::private_dir(&jobs)?;
        let backup = jobs.join(crate::util::mint_token());
        fs::create_dir(&backup)?;
        files::private_dir(&backup)?;
        let receipt = Pending {
            version: version.into(),
            backup: backup.clone(),
            targets: targets.to_vec(),
        };
        let result = (|| {
            let prepared = self.prepare(version, &backup, source)?;
            let next = prepared.targets(version, targets)?;
            files::record(&backup.join("prepared.json"), &next)?;
            let perform = || {
                Self::unchanged(targets)?;
                self.backup_files(targets, &backup)?;
                files::record(&self.pending(), &receipt)?;
                prepared.install(self)?;
                fs::write(self.data_dir.join("install-method"), "release\n")?;
                Ok(())
            };
            if let Some(store) = &mut store {
                store.install_application_update(version, &backup, perform)?;
            } else {
                perform()?;
            }
            Ok((next, prepared))
        })();
        let (next, prepared) = match result {
            Ok(result) => result,
            Err(error) => {
                if self.pending().try_exists()? && Self::unchanged(targets).is_ok() {
                    fs::remove_file(self.pending())?;
                }
                return Err(error);
            }
        };
        files::record(&backup.join("completed.json"), &receipt)?;
        // The database write committed before removing the uncertain-outcome fence.
        fs::remove_file(self.pending())?;
        let warnings = prepared.cleanup(&backup);
        Ok(Updated {
            version: version.into(),
            restart_required: true,
            backup,
            targets: next,
            warnings,
        })
    }

    pub(super) fn inspect(&self, source: &ReleaseSource) -> Result<super::UpdateInspection> {
        let pending = self.pending();
        if fs::metadata(&pending)?.len() > 64 * 1024 {
            return Err(Error::invalid("the update receipt is not bounded"));
        }
        let _lock = super::super::fence::exclusive(&self.data_dir)?;
        let receipt: Pending = serde_json::from_slice(&fs::read(&pending)?)?;
        let expected: Vec<_> = self
            .cli
            .as_ref()
            .map(|p| ("CLI", p))
            .into_iter()
            .chain(self.desktop.as_ref().map(|p| ("Desktop app", p)))
            .collect();
        if receipt.targets.len() != expected.len()
            || receipt
                .targets
                .iter()
                .zip(expected)
                .any(|(t, (name, path))| t.name != name || t.path != *path)
            || receipt.backup.canonicalize()?.parent()
                != Some(self.data_dir.join("updates").canonicalize()?.as_path())
            || super::super::version(&receipt.version).is_none()
        {
            return Err(Error::invalid(
                "this receipt belongs to another installation; inspect from the same app or CLI",
            ));
        }
        if let Some(db) = &self.database {
            if files::resolved(db)?.parent() != Some(files::resolved(&self.data_dir)?.as_path()) {
                return Err(Error::invalid("the database and update fence disagree"));
            }
        }
        let installed = if Self::unchanged(&receipt.targets).is_ok() {
            if let Some(db) = self.database.as_ref().filter(|p| p.exists()) {
                if Store::installed_application_version(db)?.as_deref() == Some(&receipt.version) {
                    return Err(Error::invalid("the database records an update but its programs differ; retain the recovery fence"));
                }
            }
            false
        } else {
            let prepared = receipt.backup.join("prepared.json");
            if fs::metadata(&prepared)?.len() > 64 * 1024 {
                return Err(Error::invalid("the prepared receipt is not bounded"));
            }
            let next: Vec<UpdateTarget> = serde_json::from_slice(&fs::read(prepared)?)?;
            if next.len() != receipt.targets.len()
                || next.iter().zip(&receipt.targets).any(|(next, old)| {
                    next.path != old.path
                        || next.name != old.name
                        || next.version.as_deref() != Some(&receipt.version)
                })
            {
                return Err(Error::invalid(
                    "the prepared update does not match its approved destinations",
                ));
            }
            Self::unchanged(&next).map_err(|_| {
                Error::invalid(
                    "the installation is mixed or changed; retain its backups and recovery fence",
                )
            })?;
            if let Some(app) = &self.desktop {
                source.native_signature(app)?;
            }
            #[cfg(target_os = "macos")]
            if let Some(cli) = &self.cli {
                source.native_signature(cli)?;
            }
            if let Some(db) = self.database.as_ref().filter(|p| p.exists()) {
                Store::record_inspected_application_update(db, &receipt.version, &receipt.backup)?;
            }
            fs::write(self.data_dir.join("install-method"), "release\n")?;
            true
        };
        files::record(
            &receipt
                .backup
                .join(format!("inspection-{}.json", crate::util::mint_token())),
            &serde_json::json!({"installed":installed,"version":receipt.version}),
        )?;
        fs::remove_file(pending)?;
        Ok(super::UpdateInspection {
            installed,
            version: receipt.version,
            backup: receipt.backup,
            detail: if installed {
                "The prepared programs are installed. Restart AI Team."
            } else {
                "The original programs are unchanged. You may check and retry the update."
            }
            .into(),
        })
    }

    fn unchanged(targets: &[UpdateTarget]) -> Result<()> {
        for target in targets {
            if files::fingerprint(&target.path)? != target.fingerprint {
                return Err(Error::invalid(
                    "an installed program changed; check and approve again",
                ));
            }
        }
        Ok(())
    }

    fn backup_files(&self, targets: &[UpdateTarget], backup: &Path) -> Result<()> {
        for (index, target) in targets.iter().enumerate() {
            if target.path.exists() {
                files::copy(&target.path, &backup.join(format!("program-{index}")))?;
            }
        }
        if let Some(config) = self.config_dir.as_ref().filter(|p| p.exists()) {
            files::copy(config, &backup.join("config"))?;
        }
        let oauth = self.data_dir.join("oauth");
        if oauth.exists() {
            files::copy(&oauth, &backup.join("oauth"))?;
        }
        let marker = self.data_dir.join("install-method");
        if marker.exists() {
            fs::copy(marker, backup.join("install-method"))?;
        }
        Ok(())
    }

    fn prepare(&self, version: &str, backup: &Path, source: &ReleaseSource) -> Result<Prepared> {
        let asset = if self.desktop.is_some() {
            format!("ai-team-v{version}-macos-universal.tar.gz")
        } else {
            super::super::asset_for(version)
                .ok_or_else(|| Error::invalid("no release archive for this platform"))?
        };
        let archive = backup.join(&asset);
        let sums = backup.join("checksums.txt");
        source.download(version, &asset, &archive)?;
        source.download(version, "checksums.txt", &sums)?;
        super::super::verify(&archive, &sums, &asset)?;
        let payload = backup.join("payload");
        fs::create_dir(&payload)?;
        let output = Command::new("tar")
            .arg("xzf")
            .arg(&archive)
            .arg("-C")
            .arg(&payload)
            .stdin(Stdio::null())
            .output()?;
        if !output.status.success() {
            return Err(Error::invalid(format!(
                "the archive would not unpack: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let cli = payload.join("ait");
        let app = payload.join("ai-team.app");
        files::program(&cli)?;
        if !cli.is_file() {
            return Err(Error::invalid("the release CLI is not a regular file"));
        }
        if self.desktop.is_some() {
            files::program(&app)?;
            if files::plist(&app, "CFBundleIdentifier")? != "dev.zottiben.ai-team"
                || files::plist(&app, "CFBundleShortVersionString")? != version
            {
                return Err(Error::invalid(
                    "the downloaded desktop does not match the approved release",
                ));
            }
            let executable = super::super::executable_of(&app)?;
            if super::super::same_contents(&cli, &executable) {
                return Err(Error::invalid(
                    "the release contains the CLI in place of its desktop app",
                ));
            }
            source.native_signature(&app)?;
        }
        #[cfg(target_os = "macos")]
        source.native_signature(&cli)?;
        if tokio::runtime::Handle::current()
            .block_on(super::cli_version(&cli))?
            .as_deref()
            != Some(version)
        {
            return Err(Error::invalid(
                "the downloaded CLI does not match the approved version",
            ));
        }
        // Stage on each destination's filesystem. No program moves until both copies
        // and the native signature have verified, and the old fingerprints revalidate.
        let cli_stage = self
            .cli
            .as_ref()
            .map(|target| {
                let stage = Stage::new(target)?;
                files::copy(&cli, &stage.0.join("fresh"))?;
                Ok::<_, Error>(stage)
            })
            .transpose()?;
        let app_stage = if let Some(target) = &self.desktop {
            let stage = Stage::new(target)?;
            files::copy(&app, &stage.0.join("fresh.app"))?;
            source.native_signature(&stage.0.join("fresh.app"))?;
            Some(stage)
        } else {
            None
        };
        Ok(Prepared {
            cli: cli_stage,
            app: app_stage,
        })
    }
}

struct Stage(PathBuf);
impl Stage {
    fn new(target: &Path) -> Result<Self> {
        let parent = target
            .parent()
            .ok_or_else(|| Error::invalid("an update destination has no directory"))?;
        fs::create_dir_all(parent)?;
        let stage = parent.join(format!(".ai-team-update-{}", crate::util::mint_token()));
        fs::create_dir(&stage).map_err(|e| {
            Error::invalid(format!(
                "cannot stage an update beside {}: {e}; no administrator command was started",
                target.display()
            ))
        })?;
        files::private_dir(&stage)?;
        Ok(Self(stage))
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        // A failed restoration keeps its previous app, never cleans away the recovery copy.
        if !self.0.join("previous.app").exists() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

struct Prepared {
    cli: Option<Stage>,
    app: Option<Stage>,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    version: String,
    backup: PathBuf,
    targets: Vec<UpdateTarget>,
}

impl Prepared {
    fn cleanup(&self, backup: &Path) -> Vec<String> {
        let mut warnings = Vec::new();
        let mut paths = vec![backup.join("payload")];
        if let Some(stage) = &self.app {
            paths.push(stage.0.join("previous.app"));
        }
        match fs::read_dir(backup) {
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Ok(entry) if entry.file_name().to_string_lossy().ends_with(".tar.gz") => {
                            paths.push(entry.path());
                        }
                        Ok(_) => {}
                        Err(error) => warnings.push(error.to_string()),
                    }
                }
            }
            Err(error) => warnings.push(error.to_string()),
        }
        for path in paths {
            let result = if path.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            };
            if let Err(error) = result {
                warnings.push(format!(
                    "temporary update files retained at {}: {error}",
                    path.display()
                ));
            }
        }
        warnings
    }

    fn targets(&self, version: &str, targets: &[UpdateTarget]) -> Result<Vec<UpdateTarget>> {
        targets
            .iter()
            .map(|target| {
                let staged = if target.name == "CLI" {
                    self.cli
                        .as_ref()
                        .ok_or_else(|| Error::invalid("the CLI was not staged"))?
                        .0
                        .join("fresh")
                } else {
                    self.app
                        .as_ref()
                        .ok_or_else(|| Error::invalid("the desktop was not staged"))?
                        .0
                        .join("fresh.app")
                };
                Ok(UpdateTarget {
                    name: target.name.clone(),
                    path: target.path.clone(),
                    version: Some(version.into()),
                    fingerprint: files::fingerprint(&staged)?,
                })
            })
            .collect()
    }

    fn install(&self, installation: &UpdateInstallation) -> Result<()> {
        self.install_with(installation, |from, to| fs::rename(from, to))
    }

    fn install_with(
        &self,
        installation: &UpdateInstallation,
        rename: impl Fn(&Path, &Path) -> std::io::Result<()>,
    ) -> Result<()> {
        if let Some((stage, target)) = self.app.as_ref().zip(installation.desktop.as_ref()) {
            let previous = stage.0.join("previous.app");
            rename(target, &previous)?;
            if let Err(error) = rename(&stage.0.join("fresh.app"), target) {
                rename(&previous, target).map_err(|restore| {
                    Error::invalid(format!(
                        "app swap failed ({error}); restore {} manually ({restore})",
                        previous.display()
                    ))
                })?;
                return Err(error.into());
            }
            if let Some((cli, destination)) = self.cli.as_ref().zip(installation.cli.as_ref()) {
                if let Err(error) = rename(&cli.0.join("fresh"), destination) {
                    rename(target, &stage.0.join("fresh.app"))?;
                    rename(&previous, target).map_err(|restore| {
                        Error::invalid(format!(
                            "CLI swap failed ({error}); restore {} manually ({restore})",
                            previous.display()
                        ))
                    })?;
                    return Err(error.into());
                }
            }
        } else if let Some((cli, destination)) = self.cli.as_ref().zip(installation.cli.as_ref()) {
            rename(&cli.0.join("fresh"), destination)?;
        }
        Ok(())
    }
}
