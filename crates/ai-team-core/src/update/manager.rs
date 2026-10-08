//! Shared desktop/CLI update coordination. Polling is read-only; installation is explicit.

use super::{Host, Method};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

mod admission;
mod discovery;
mod files;
mod installation;
mod release;
#[cfg(all(test, unix))]
mod tests;

pub use admission::UpdatePermit;
pub use installation::UpdateInstallation;

const CHECK_INTERVAL: Duration = Duration::from_mins(15);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateTarget {
    pub name: String,
    pub path: PathBuf,
    pub version: Option<String>,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpdateState {
    Idle,
    Updating,
    Inspection,
    Restart,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateStatus {
    pub current: String,
    pub latest: Option<String>,
    pub method: Method,
    pub targets: Vec<UpdateTarget>,
    pub can_update: bool,
    pub update_available: bool,
    pub blocked: Option<String>,
    pub approval: Option<String>,
    pub checked_at: Option<String>,
    pub state: UpdateState,
    pub release_url: String,
}

impl UpdateStatus {
    fn offer(&mut self, version: &str) {
        self.release_url = format!("https://github.com/{}/releases/tag/v{version}", super::REPO);
        let needed = self.targets.iter().any(|t| {
            t.version
                .as_ref()
                .is_none_or(|v| super::is_newer(version, v))
        });
        self.update_available = super::is_newer(version, &self.current) || needed;
        self.can_update = self.blocked.is_none() && self.state == UpdateState::Idle && needed;
        if self.can_update {
            self.approval = Some(approval(version, &self.targets));
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Updated {
    pub version: String,
    pub restart_required: bool,
    pub backup: PathBuf,
    pub targets: Vec<UpdateTarget>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInspection {
    pub installed: bool,
    pub version: String,
    pub backup: PathBuf,
    pub detail: String,
}

#[derive(Debug, Default)]
struct Cache {
    checked: Option<Instant>,
    at: Option<String>,
    latest: Option<String>,
}

#[derive(Debug)]
struct Inner {
    installation: Option<UpdateInstallation>,
    source: release::ReleaseSource,
    cache: Mutex<Cache>,
    applying: Mutex<()>,
    initial: std::sync::Mutex<Option<Vec<UpdateTarget>>>,
    initial_update: std::result::Result<Option<std::time::SystemTime>, String>,
}

#[derive(Debug, Clone)]
pub struct UpdateManager(Arc<Inner>);

impl Default for UpdateManager {
    fn default() -> Self {
        Self::new(None)
    }
}

impl UpdateManager {
    /// Disabled by default so a test server cannot discover the operator's installation.
    pub fn new(installation: Option<UpdateInstallation>) -> Self {
        let initial_update = installation
            .as_ref()
            .map(admission::stamp)
            .transpose()
            .map(Option::flatten)
            .map_err(|e| e.to_string());
        Self(Arc::new(Inner {
            installation,
            initial_update,
            source: release::ReleaseSource::Github,
            cache: Mutex::new(Cache::default()),
            applying: Mutex::new(()),
            initial: std::sync::Mutex::new(None),
        }))
    }

    pub fn discover(host: Host) -> Result<Self> {
        Ok(Self::new(Some(UpdateInstallation::discover(host)?)))
    }

    /// Offline fixtures must keep every input and destination inside their private root.
    pub fn isolated(installation: UpdateInstallation, root: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
        installation.require_inside(&root)?;
        let initial_update = admission::stamp(&installation).map_err(|e| e.to_string());
        Ok(Self(Arc::new(Inner {
            installation: Some(installation),
            initial_update,
            source: release::ReleaseSource::Isolated(root),
            cache: Mutex::new(Cache::default()),
            applying: Mutex::new(()),
            initial: std::sync::Mutex::new(None),
        })))
    }

    pub async fn check(&self, force: bool) -> UpdateStatus {
        let mut cache = self.0.cache.lock().await;
        if self.0.installation.is_some()
            && (force || cache.checked.is_none_or(|t| t.elapsed() >= CHECK_INTERVAL))
        {
            cache.latest = self.0.source.latest().await;
            cache.checked = Some(Instant::now());
            cache.at = Some(crate::now());
        }
        let latest = cache.latest.clone();
        let at = cache.at.clone();
        drop(cache);
        self.status(latest, at).await
    }

    async fn status(&self, latest: Option<String>, checked_at: Option<String>) -> UpdateStatus {
        let mut status = UpdateStatus {
            current: super::current_version().into(),
            latest: latest.clone(),
            method: Method::Unknown,
            targets: vec![],
            can_update: false,
            update_available: false,
            blocked: None,
            approval: None,
            checked_at,
            state: if self.busy() {
                UpdateState::Updating
            } else {
                UpdateState::Idle
            },
            release_url: format!("https://github.com/{}/releases/latest", super::REPO),
        };
        let Some(installation) = &self.0.installation else {
            status.blocked = Some("updates are disabled for this server".into());
            return status;
        };
        status.method = installation.method;
        if installation.pending().exists() && status.state != UpdateState::Updating {
            status.state = UpdateState::Inspection;
        }
        let mut restart = false;
        match installation.targets().await {
            Ok(targets) => status.targets = targets,
            Err(error) => {
                status.blocked = Some(error.to_string());
                return status;
            }
        }
        if let Ok(mut initial) = self.0.initial.lock() {
            match &*initial {
                Some(old) => restart = *old != status.targets,
                None => *initial = Some(status.targets.clone()),
            }
        }
        let running = status
            .targets
            .iter()
            .find(|target| match installation.host {
                Host::Cli => target.name == "CLI",
                Host::Desktop => target.name == "Desktop app",
            });
        restart |= running
            .and_then(|t| t.version.as_ref())
            .is_some_and(|v| super::is_newer(v, super::current_version()));
        restart |= self
            .0
            .initial_update
            .as_ref()
            .is_ok_and(|old| admission::stamp(installation).is_ok_and(|now| now != *old));
        if restart && status.state == UpdateState::Idle {
            status.state = UpdateState::Restart;
        }
        if status.state == UpdateState::Inspection {
            status.blocked = Some(format!(
                "an earlier update needs inspection; recovery evidence: {}",
                installation.pending().display()
            ));
        } else if restart {
            status.blocked = Some("restart this process before updating again".into());
        } else if let Some(reason) = &installation.blocked {
            status.blocked = Some(reason.clone());
        } else if latest.is_none() {
            status.blocked =
                Some("could not check GitHub; your installed programs were not changed".into());
        } else if status.targets.iter().any(|t| {
            t.version
                .as_ref()
                .zip(latest.as_ref())
                .is_some_and(|(current, next)| super::is_newer(current, next))
        }) {
            status.blocked = Some(
                "an installed program is newer than the latest release; updates never downgrade it"
                    .into(),
            );
        }
        if let Some(version) = latest.as_ref() {
            status.offer(version);
        }
        status
    }

    pub async fn apply(&self, version: &str, expected: &str) -> Result<Updated> {
        let guard = self
            .0
            .applying
            .try_lock()
            .map_err(|_| Error::invalid("an update is already running"))?;
        // The guard makes status say busy; comparison below deliberately ignores only that bit.
        let status = self.check(true).await;
        let installation = self
            .0
            .installation
            .clone()
            .ok_or_else(|| Error::invalid("updates are disabled"))?;
        if let Some(reason) = status.blocked {
            return Err(Error::invalid(reason));
        }
        if !status.targets.iter().any(|t| {
            t.version
                .as_ref()
                .is_none_or(|v| super::is_newer(version, v))
        }) || status.latest.as_deref() != Some(version)
            || approval(version, &status.targets) != expected
        {
            return Err(Error::invalid(
                "the release or installed programs changed; check and approve again",
            ));
        }
        let version = version.to_string();
        let targets = status.targets;
        let source = self.0.source.clone();
        let result =
            tokio::task::spawn_blocking(move || installation.install(&version, &targets, &source))
                .await
                .map_err(|e| {
                    Error::invalid(format!(
                        "update worker failed; inspect recovery evidence: {e}"
                    ))
                })?;
        drop(guard);
        result
    }

    pub async fn inspect(&self) -> Result<UpdateInspection> {
        let _guard = self
            .0
            .applying
            .try_lock()
            .map_err(|_| Error::invalid("an update is still running"))?;
        let installation = self
            .0
            .installation
            .clone()
            .ok_or_else(|| Error::invalid("updates are disabled"))?;
        let source = self.0.source.clone();
        tokio::task::spawn_blocking(move || installation.inspect(&source))
            .await
            .map_err(|e| Error::invalid(format!("update inspection failed: {e}")))?
    }

    pub fn busy(&self) -> bool {
        self.0.applying.try_lock().is_err()
            || self
                .0
                .installation
                .as_ref()
                .is_some_and(|i| super::fence::active(&i.data_dir).unwrap_or(true))
    }
}

fn approval(version: &str, targets: &[UpdateTarget]) -> String {
    super::sha256(format!("{version}\n{targets:?}").as_bytes())
}

async fn cli_version(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    let mut command = tokio::process::Command::new(path);
    crate::pi::strip_metered_env(&mut command);
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        command
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| Error::invalid("the installed CLI did not answer its version check"))??;
    let text = String::from_utf8_lossy(&output.stdout);
    let version = text
        .trim()
        .strip_prefix("ait ")
        .filter(|v| super::version(v).is_some());
    if !output.status.success() || version.is_none() {
        return Err(Error::invalid(format!(
            "{} is not a readable AI Team CLI installation",
            path.display()
        )));
    }
    Ok(version.map(str::to_string))
}
