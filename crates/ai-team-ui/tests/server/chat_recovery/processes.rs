//! Actual socket-server startup versus a harmless journalled process group. No Pi,
//! installed provider, credentials or global environment changes are involved.
use super::*;
use std::os::unix::process::CommandExt;

struct Sleeper(std::process::Child);
impl Drop for Sleeper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn boot() -> String {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .unwrap()
            .trim()
            .to_owned()
    }
    #[cfg(target_os = "macos")]
    {
        String::from_utf8(
            std::process::Command::new("/usr/sbin/sysctl")
                .args(["-n", "kern.bootsessionuuid"])
                .env_clear()
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_owned()
    }
}

#[test]
fn server_startup_drains_only_matching_journalled_groups_and_keeps_failures_visible() {
    for matches in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("team.db");
        let mut store = Store::init(&db).unwrap();
        let (chat, turn) = interrupted(&mut store, dir.path());
        let mut sleeper = Sleeper(
            std::process::Command::new("/bin/sleep")
                .arg("60")
                .env_clear()
                .process_group(0)
                .spawn()
                .unwrap(),
        );
        let pid = i64::from(sleeper.0.id());
        let output = std::process::Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "lstart="])
            .env_clear()
            .env("LC_ALL", "C")
            .output()
            .unwrap();
        assert!(output.status.success());
        let identity = if matches {
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        } else {
            "not this process's identity".into()
        };
        Db::open(&db).unwrap().conn().execute("INSERT INTO chat_child(run_id,epoch,kind,program,boot,pid,identity,state,created_at) VALUES (?1,1,'command','/bin/sleep',?2,?3,?4,'running',?5)", (turn.run_id,boot(),pid,identity,ai_team_core::now())).unwrap();
        let another = dir.path().join("independent");
        std::fs::create_dir(&another).unwrap();
        let (_, other) = interrupted(&mut store, &another);
        let h = Harness::watching(&db);
        let execution = store.chat_team_run(turn.run_id).unwrap().unwrap();
        assert_eq!(execution.quiescent, matches);
        assert!(
            store
                .chat_team_run(other.run_id)
                .unwrap()
                .unwrap()
                .quiescent,
            "one uncertain group must not block independent recovery"
        );
        let detail = h.get(&format!("/api/chats/{chat}")).json();
        assert_eq!(
            detail["team_recovery"]["state"],
            if matches {
                "quiescent"
            } else {
                "needs_inspection"
            }
        );
        assert_eq!(detail["can_resume"], false);
        if matches {
            assert!(
                sleeper.0.try_wait().unwrap().is_some(),
                "the recorded group was not drained"
            );
        } else {
            assert!(
                sleeper.0.try_wait().unwrap().is_none(),
                "a mismatched identity was signalled"
            );
            assert!(detail["team_recovery"]["reason"]
                .as_str()
                .unwrap()
                .contains("different leader identity"));
            h.get(&format!("/api/chats/{chat}"));
            assert_eq!(
                store.chat_team_run(turn.run_id).unwrap().unwrap().rev,
                execution.rev,
                "GET must not loop over failed recovery"
            );
        }
        assert_eq!(store.chat(chat).unwrap().active_node_id, Some(turn.node_id));
        assert_eq!(store.node_runs(turn.run_id).unwrap().len(), 1);
    }
}
