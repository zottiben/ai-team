//! Bounded command output and process-group lifetime for checks and control-plane tools.
//! Like Pi, child exit and pipe EOF are separate events; dropping a task kills its group.

use crate::{Error, Result};
use std::{
    process::{ExitStatus, Stdio},
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
};

/// For commands whose explicit checkout is authoritative. Call before setting any
/// intentional temporary-index override; do not retarget through the parent shell.
pub(crate) fn strip_git_overrides(command: &mut Command) {
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
    ] {
        command.env_remove(key);
    }
}

pub(crate) struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: bool,
}

struct Process {
    child: Child,
    receipt: Option<crate::chat::team::children::Receipt>,
    pid: i32,
    owner: Option<std::sync::Arc<crate::chat::team::ownership::Ownership>>,
}
impl Process {
    fn kill_group(&self) {
        #[cfg(unix)]
        if let Some(pid) = rustix::process::Pid::from_raw(self.pid) {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
    }
    async fn terminate(&mut self) -> Result<()> {
        self.kill_group();
        self.child.start_kill()?;
        self.child.wait().await?;
        self.pid = 0;
        if let Some(receipt) = &mut self.receipt {
            receipt.finish().await?;
        }
        Ok(())
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if self.pid != 0 {
            if let Some(owner) = &self.owner {
                owner.doubt();
            }
        }
        self.kill_group();
    }
}

pub(crate) async fn run<S>(
    command: &mut Command,
    timeout: Duration,
    limit: usize,
    stop: S,
) -> Result<Output>
where
    S: std::future::Future<Output = String> + Send,
{
    crate::pi::strip_metered_env(command);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let (child, receipt) = crate::chat::team::children::spawn(command, "command")?;
    let pid = child.id().unwrap_or(0).cast_signed();
    let mut process = Process {
        child,
        receipt,
        pid,
        owner: crate::chat::team::ownership::current(),
    };
    let mut out = process
        .child
        .stdout
        .take()
        .ok_or_else(|| Error::invalid("command has no stdout"))?;
    let mut err = process
        .child
        .stderr
        .take()
        .ok_or_else(|| Error::invalid("command has no stderr"))?;
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let (mut out_buf, mut err_buf) = (vec![0; 8192], vec![0; 8192]);
    let (mut out_open, mut err_open, mut exited, mut truncated) = (true, true, None, false);
    tokio::pin!(stop);
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    while out_open || err_open || exited.is_none() {
        let error = tokio::select! {
            reason = &mut stop => Some(Error::Io(std::io::Error::new(std::io::ErrorKind::Interrupted, reason))),
            () = &mut deadline => Some(Error::invalid(format!("command exceeded its {}s deadline", timeout.as_secs()))),
            status = process.child.wait(), if exited.is_none() => {
                match status {
                    Ok(status) => { exited = Some(status); process.kill_group(); process.pid = 0; None }
                    Err(error) => Some(error.into()),
                }
            }
            bytes = out.read(&mut out_buf), if out_open => match bytes {
                Ok(0) => { out_open = false; None }
                Ok(n) => { truncated |= append_tail(&mut stdout, &out_buf[..n], limit); None }
                Err(error) => Some(error.into()),
            },
            bytes = err.read(&mut err_buf), if err_open => match bytes {
                Ok(0) => { err_open = false; None }
                Ok(n) => { truncated |= append_tail(&mut stderr, &err_buf[..n], limit); None }
                Err(error) => Some(error.into()),
            },
        };
        if let Some(error) = error {
            process.terminate().await.map_err(|cleanup| {
                Error::invalid(format!("{error}; reaping command also failed: {cleanup}"))
            })?;
            return Err(error);
        }
    }
    if let Some(receipt) = &mut process.receipt {
        receipt.finish().await?;
    }
    Ok(Output {
        status: exited.ok_or_else(|| Error::invalid("command never exited"))?,
        stdout,
        stderr,
        truncated,
    })
}

/// Synchronous metadata callers can already be inside Tokio. Use the same bounded
/// runner on a dedicated thread, carrying any controller receipt rather than nesting
/// runtimes or losing child-journal ownership.
pub(crate) fn run_blocking(
    command: std::process::Command,
    timeout: Duration,
    limit: usize,
) -> Result<Output> {
    let owner = crate::chat::team::ownership::current();
    std::thread::Builder::new()
        .name("ai-team-metadata".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let mut command = Command::from(command);
            let work = run(&mut command, timeout, limit, std::future::pending());
            runtime.block_on(async {
                match owner {
                    Some(owner) => owner.track(work).await,
                    None => work.await,
                }
            })
        })?
        .join()
        .map_err(|_| Error::invalid("metadata command worker panicked"))?
}

fn append_tail(output: &mut Vec<u8>, bytes: &[u8], limit: usize) -> bool {
    output.extend_from_slice(bytes);
    let overflow = output.len().saturating_sub(limit);
    if overflow > 0 {
        output.drain(..overflow);
    }
    overflow > 0
}
