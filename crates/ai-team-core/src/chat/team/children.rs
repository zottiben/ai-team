//! Durable evidence for team-owned process groups. An intent without an identity is
//! uncertainty, not proof no child started. No command arguments or environment are saved.
use super::ownership::{self, Ownership};
use crate::{Error, Result, Store};
use std::{path::Path, sync::Arc, time::Duration};

#[derive(Debug)]
pub(crate) struct ChildRecord {
    pub id: i64,
    pub run: i64,
    pub boot: String,
    pub pid: Option<i64>,
    pub identity: Option<String>,
    pub state: String,
}

#[derive(Debug)]
pub(crate) struct Receipt {
    owner: Arc<Ownership>,
    record: ChildRecord,
    ended: bool,
}
impl Receipt {
    pub(crate) fn begin(
        kind: &str,
        program: &str,
        workspace: Option<&Path>,
    ) -> Result<Option<Self>> {
        let Some(owner) = ownership::current() else {
            return Ok(None);
        };
        let record =
            Store::open(&owner.db)?.begin_chat_child(&owner, kind, program, workspace, &boot()?)?;
        Ok(Some(Self {
            owner,
            record,
            ended: false,
        }))
    }
    pub(crate) fn attach(&mut self, pid: i32) -> Result<()> {
        // A failed identity probe must not erase the PID we already know. Recovery
        // may acknowledge an empty group, but cannot signal it without its identity.
        Store::open(&self.owner.db)?.record_chat_child_pid(
            &self.owner,
            self.record.id,
            i64::from(pid),
        )?;
        self.record.pid = Some(i64::from(pid));
        let identity = crate::chat::process_identity(i64::from(pid)).ok_or_else(|| {
            Error::invalid("could not identify the new team child; retain its spawn intent")
        })?;
        Store::open(&self.owner.db)?.attach_chat_child(
            &self.owner,
            self.record.id,
            i64::from(pid),
            &identity,
        )?;
        self.record.identity = Some(identity);
        Ok(())
    }
    pub(crate) fn spawn_failed(&mut self) -> Result<()> {
        self.acknowledge()
    }
    fn acknowledge(&mut self) -> Result<()> {
        Store::open(&self.owner.db)?.finish_chat_child(&self.owner, self.record.id)?;
        self.ended = true;
        Ok(())
    }
    pub(crate) async fn finish(&mut self) -> Result<()> {
        if self.ended {
            return Ok(());
        }
        let pid = self
            .record
            .pid
            .ok_or_else(|| Error::invalid("this child has no recorded identity"))?;
        wait_empty(pid).await?;
        self.acknowledge()
    }
}
impl Drop for Receipt {
    fn drop(&mut self) {
        if !self.ended {
            self.owner.doubt();
        }
    }
}

pub(crate) fn spawn(
    command: &mut tokio::process::Command,
    kind: &str,
) -> Result<(tokio::process::Child, Option<Receipt>)> {
    crate::pi::strip_metered_env(command);
    command.kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut receipt = Receipt::begin(
        kind,
        &command.as_std().get_program().to_string_lossy(),
        command.as_std().get_current_dir(),
    )?;
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            if let Some(receipt) = &mut receipt {
                receipt.spawn_failed().map_err(|cleanup| {
                    Error::invalid(format!(
                        "{error}; recording failed spawn also failed: {cleanup}"
                    ))
                })?;
            }
            return Err(error.into());
        }
    };
    if let Some(receipt) = &mut receipt {
        let pid = child.id().unwrap_or(0).cast_signed();
        if let Err(error) = receipt.attach(pid) {
            #[cfg(unix)]
            if let Some(pid) = rustix::process::Pid::from_raw(pid) {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
            }
            child.start_kill().map_err(|cleanup| {
                Error::invalid(format!(
                    "{error}; killing unregistered child also failed: {cleanup}"
                ))
            })?;
            return Err(error);
        }
    }
    Ok((child, receipt))
}

/// Boot-scoped process identities survive app restarts, not OS restarts.
pub(crate) fn boot() -> Result<String> {
    static BOOT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    if let Some(value) = BOOT.get() {
        return Ok(value.clone());
    }
    let value = read_boot()?;
    if value.split('-').map(str::len).collect::<Vec<_>>() != [8, 4, 4, 4, 12]
        || !value.chars().all(|c| c == '-' || c.is_ascii_hexdigit())
    {
        return Err(Error::invalid("invalid OS boot UUID; keep ownership"));
    }
    let _ = BOOT.set(value.clone());
    Ok(value)
}

#[cfg(target_os = "linux")]
fn read_boot() -> Result<String> {
    Ok(std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .to_owned())
}
#[cfg(target_os = "macos")]
fn read_boot() -> Result<String> {
    {
        let mut command = std::process::Command::new("/usr/sbin/sysctl");
        crate::pi::strip_metered_std_env(&mut command);
        let output = command.args(["-n", "kern.bootsessionuuid"]).output()?;
        if output.status.success() {
            let value = String::from_utf8(output.stdout)
                .map_err(|_| Error::invalid("invalid boot identity"))?;
            if !value.trim().is_empty() {
                return Ok(value.trim().to_owned());
            }
        }
    }
    Err(Error::invalid(
        "cannot establish this machine's boot identity for team supervision",
    ))
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_boot() -> Result<String> {
    Err(Error::invalid(
        "team process recovery needs a supported OS boot identity",
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[tokio::test]
    async fn spawn_owns_its_group_even_when_the_caller_does_not_set_one() {
        let (mut child, receipt) = spawn(
            tokio::process::Command::new("sleep")
                .arg("5")
                .kill_on_drop(true),
            "command",
        )
        .unwrap();
        assert!(receipt.is_none());
        let pid = i64::from(child.id().unwrap());
        let group = processes()
            .unwrap()
            .into_iter()
            .find(|process| process.pid == pid)
            .unwrap()
            .group;
        child.kill().await.unwrap();
        child.wait().await.unwrap();
        assert_eq!(
            group, pid,
            "cleanup must never inspect the caller's process group"
        );
    }
}

#[derive(Debug)]
struct Process {
    pid: i64,
    group: i64,
    zombie: bool,
}
fn processes() -> Result<Vec<Process>> {
    let mut command = std::process::Command::new("ps");
    crate::pi::strip_metered_std_env(&mut command);
    let output = command
        .args(["-axo", "pid=,pgid=,stat="])
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(Error::invalid(
            "could not inspect process groups; keep ownership",
        ));
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| Error::invalid("invalid process group metadata"))?;
    let records: Vec<_> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() != 3 {
                return Err(Error::invalid("incomplete process group metadata"));
            }
            Ok(Process {
                pid: fields[0]
                    .parse()
                    .map_err(|_| Error::invalid("invalid process id"))?,
                group: fields[1]
                    .parse()
                    .map_err(|_| Error::invalid("invalid group id"))?,
                zombie: fields[2].starts_with('Z'),
            })
        })
        .collect::<Result<_>>()?;
    if !records
        .iter()
        .any(|process| process.pid == i64::from(std::process::id()))
    {
        return Err(Error::invalid(
            "process inventory is incomplete; keep ownership",
        ));
    }
    Ok(records)
}
async fn snapshot() -> Result<Vec<Process>> {
    tokio::task::spawn_blocking(processes)
        .await
        .map_err(|error| Error::invalid(format!("process probe failed: {error}")))?
}
pub(crate) async fn live_writer(pid: i64) -> Result<bool> {
    Ok(snapshot()
        .await?
        .iter()
        .any(|process| process.pid == pid && !process.zombie))
}
async fn wait_empty(pid: i64) -> Result<()> {
    for _ in 0..100 {
        if !snapshot()
            .await?
            .iter()
            .any(|process| process.group == pid && !process.zombie)
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Err(Error::invalid(
        "the child process group is not drained; keep ownership",
    ))
}

/// Signal only a still-identifiable group leader. A leaderless live group or reused
/// PID is deliberately not guessed to be ours, even when the number matches.
pub(crate) async fn drain(record: &ChildRecord) -> Result<()> {
    if record.state == "drained" || record.boot != boot()? {
        return Ok(());
    }
    let pid = record.pid.ok_or_else(|| {
        Error::invalid("spawn intent has no process identity; cannot certify cleanup")
    })?;
    let members = snapshot().await?;
    if !members
        .iter()
        .any(|process| process.group == pid && !process.zombie)
    {
        return Ok(());
    }
    let identity = record.identity.as_deref().ok_or_else(|| {
        Error::invalid("live child has no saved start identity; do not signal it")
    })?;
    if !members
        .iter()
        .any(|process| process.pid == pid && process.group == pid)
        || crate::chat::process_identity(pid).as_deref() != Some(identity)
    {
        return Err(Error::invalid(
            "live process group has an absent or different leader identity; do not signal it",
        ));
    }
    #[cfg(unix)]
    rustix::process::kill_process_group(
        rustix::process::Pid::from_raw(
            i32::try_from(pid).map_err(|_| Error::invalid("invalid saved process id"))?,
        )
        .ok_or_else(|| Error::invalid("invalid saved process group"))?,
        rustix::process::Signal::KILL,
    )
    .map_err(std::io::Error::from)?;
    wait_empty(pid).await
}
