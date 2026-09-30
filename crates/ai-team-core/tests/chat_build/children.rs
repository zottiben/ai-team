//! Separate OS-host crashes, not just aborted Rust futures. All children are offline.
use super::*;

#[test]
#[ignore = "invoked only by the isolated parent test"]
fn child_host() {
    let Ok(db) = std::env::var("TEAM_CHILD_HOST_DB") else {
        return;
    };
    let values: Vec<i64> =
        serde_json::from_str(&std::env::var("TEAM_CHILD_HOST_START").unwrap()).unwrap();
    let start = ChatBuildStart {
        chat_id: values[0],
        node_id: values[1],
        run_id: values[2],
        revision: values[3],
    };
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(drive_chat_team_build(Path::new(&db), start))
        .unwrap();
}

fn target(f: &Fixture) -> ChatBuildRecovery {
    ChatBuildRecovery {
        chat_id: f.chat.id,
        run_id: f.turn.run_id,
        node_id: f.turn.node_id,
        expect_revision: f.store.chat_team_run(f.turn.run_id).unwrap().unwrap().rev,
    }
}

pub(super) async fn killed_hosts_drain_only_their_recorded_children() {
    a_failed_identity_registration_keeps_the_known_pid().await;
    for mode in ["slow-maker", "slow-gate", "settled-maker"] {
        let mut f = worker_fixture(mode);
        let start = f.approve().await;
        let mut host = spawn_host(&f, &start);
        let _cleanup = HostCleanup {
            db: f.store.path().to_owned(),
            pid: host.id().unwrap(),
            identity: identity(host.id().unwrap()).unwrap(),
        };
        let marker = if mode == "slow-gate" {
            "gate-waiting"
        } else {
            "worker-waiting"
        };
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            while !f.dir.path().join(marker).exists() {
                assert!(
                    host.try_wait().unwrap().is_none(),
                    "host exited before {marker}"
                );
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        if mode == "settled-maker" {
            // Fault injection: terminal node evidence and process cleanup are separate.
            // Recovery must clear process references on terminal members too.
            f.conn()
                .execute(
                    "UPDATE node_run SET status = 'done' WHERE run_id = ?1 AND role = 'backend'",
                    [f.turn.run_id],
                )
                .unwrap();
        }
        assert!(f
            .store
            .chat_team_run(f.turn.run_id)
            .unwrap()
            .unwrap()
            .supervisor_alive());
        assert!(
            recover_chat_team_processes(f.store.path(), &target(&f))
                .await
                .is_err(),
            "cannot steal a live host"
        );
        host.kill().await.unwrap();
        host.wait().await.unwrap();
        let before = calls(&f);
        assert!(!f
            .store
            .chat_team_run(f.turn.run_id)
            .unwrap()
            .unwrap()
            .supervisor_alive());
        let stale = target(&f);
        let result = recover_chat_team_processes(f.store.path(), &stale)
            .await
            .unwrap();
        assert!(result.quiescent);
        assert_eq!(result.phase, ChatTeamPhase::Blocked);
        assert!(f.store.chat(f.chat.id).unwrap().active_node_id.is_some());
        assert!(recover_chat_team_processes(f.store.path(), &stale)
            .await
            .is_err());
        assert_process_dead(&f.dir.path().join(if mode == "slow-gate" {
            "gate-tool.pid"
        } else {
            "worker-tool.pid"
        }))
        .await;
        assert_eq!(calls(&f), before, "recovery cannot restart a model");
        let open: i64 = f
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM chat_child WHERE state != 'drained'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(open, 0);
        let path = f.lease().worktree_path.unwrap();
        assert_eq!(
            std::fs::read_to_string(Path::new(&path).join("crates/S1.txt")).unwrap(),
            "GOOD\n"
        );
        assert!(!std::fs::read_to_string(f.dir.path().join("pool-calls"))
            .unwrap()
            .contains("return"));
        assert!(f
            .store
            .node_runs(f.turn.run_id)
            .unwrap()
            .iter()
            .all(|node| node.pi_pid.is_none()));
    }
}

fn spawn_host(f: &Fixture, start: &ChatBuildStart) -> tokio::process::Child {
    tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "children_tests::child_host"])
        .env("TEAM_CHILD_HOST_DB", f.store.path())
        .env(
            "TEAM_CHILD_HOST_START",
            serde_json::to_string(&[start.chat_id, start.node_id, start.run_id, start.revision])
                .unwrap(),
        )
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap()
}

async fn a_failed_identity_registration_keeps_the_known_pid() {
    let mut f = worker_fixture("success");
    let start = f.approve().await;
    f.conn().execute_batch("CREATE TRIGGER identity_failure BEFORE UPDATE OF identity ON chat_child WHEN NEW.kind = 'pi' BEGIN SELECT RAISE(ABORT, 'injected identity registration failure'); END;").unwrap();
    assert!(drive_chat_team_build(f.store.path(), start).await.is_err());
    let pid: Option<i64> = f
        .conn()
        .query_row("SELECT pid FROM chat_child WHERE kind = 'pi'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(
        pid.is_some(),
        "a known spawned pid must survive failed identity registration"
    );
    let report = recover_chat_team_processes(f.store.path(), &target(&f))
        .await
        .unwrap();
    assert!(report.quiescent);
    assert!(f.store.chat(f.chat.id).unwrap().active_node_id.is_some());
}

pub(super) async fn unknown_spawns_and_reused_pids_are_not_guessed_dead() {
    for case in ["intent", "identity", "reused", "old-boot"] {
        let mut f = worker_fixture("success");
        let start = f.approve().await;
        let owner = f.store.claim_chat_build(&start).unwrap();
        let mut unrelated = tokio::process::Command::new("sleep")
            .arg("60")
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let saved_pid = (case != "intent").then(|| i64::from(unrelated.id().unwrap()));
        let boot = if case == "old-boot" {
            "a previous OS boot".into()
        } else {
            boot_identity()
        };
        let saved_identity = (saved_pid.is_some() && case != "identity")
            .then_some("not this process's start identity");
        f.conn().execute("INSERT INTO chat_child(run_id,epoch,kind,program,boot,pid,identity,state,created_at) SELECT run_id,child_epoch,'command','fixture',?2,?3,?4,?5,'fixture' FROM chat_team_run WHERE run_id=?1", rusqlite::params![f.turn.run_id,boot,saved_pid,saved_identity,if saved_identity.is_some() {"running"} else {"intent"}]).unwrap();
        drop(owner);
        let result = recover_chat_team_processes(f.store.path(), &target(&f)).await;
        if case == "old-boot" {
            assert!(result.unwrap().quiescent);
        } else {
            assert!(result.is_err());
            assert!(
                !f.store
                    .chat_team_run(f.turn.run_id)
                    .unwrap()
                    .unwrap()
                    .quiescent
            );
        }
        assert!(
            unrelated.try_wait().unwrap().is_none(),
            "a reused/unrelated process was signalled"
        );
        assert!(calls(&f).is_empty());
        assert!(!f.dir.path().join("pool-calls").exists());
        unrelated.kill().await.unwrap();
        unrelated.wait().await.unwrap();
    }
}

struct HostCleanup {
    db: std::path::PathBuf,
    pid: u32,
    identity: String,
}
impl Drop for HostCleanup {
    fn drop(&mut self) {
        if identity(self.pid).as_deref() == Some(&self.identity) {
            let _ = rustix::process::kill_process(
                rustix::process::Pid::from_raw(self.pid.cast_signed()).unwrap(),
                rustix::process::Signal::KILL,
            );
        }
        if let Ok(conn) = rusqlite::Connection::open(&self.db) {
            let mut query = conn.prepare("SELECT pid,identity FROM chat_child WHERE pid IS NOT NULL AND state != 'drained'").unwrap();
            for row in query
                .query_map([], |row| {
                    Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
                })
                .unwrap()
            {
                let (pid, expected) = row.unwrap();
                if identity(pid).as_deref() == Some(&expected) {
                    let _ = rustix::process::kill_process_group(
                        rustix::process::Pid::from_raw(pid.cast_signed()).unwrap(),
                        rustix::process::Signal::KILL,
                    );
                }
            }
        }
    }
}
fn identity(pid: u32) -> Option<String> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().to_owned())
}

fn boot_identity() -> String {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .unwrap()
            .trim()
            .into()
    }
    #[cfg(target_os = "macos")]
    {
        String::from_utf8(
            Command::new("/usr/sbin/sysctl")
                .args(["-n", "kern.bootsessionuuid"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .into()
    }
}
