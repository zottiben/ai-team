use super::*;

#[test]
fn terminal_admission_survives_the_request_and_ends_only_after_the_process_and_pipes() {
    let f = Fixture::new();
    let manager = f.manager();
    let terminals = crate::Terminals::new();
    let permit = manager.admit().unwrap();
    let id = terminals.open_program(&f.root, "/bin/cat", &[]).unwrap();
    terminals.protect_update(id, permit).unwrap();
    let lock = super::super::super::fence::open(&f.installation.data_dir).unwrap();
    assert!(lock.try_lock().is_err());
    terminals.write(id, "\u{4}").unwrap();
    let started = Instant::now();
    while lock.try_lock().is_err() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "finished terminal retained its update permit"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(terminals.read(id, 0).unwrap().done);
    terminals.close(id);
}

#[test]
fn the_update_fence_excludes_an_independent_writer_process() {
    let f = Fixture::new();
    let lock = super::super::super::fence::open(&f.installation.data_dir).unwrap();
    lock.try_lock().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "update::manager::tests::terminals::writer_process_fixture",
            "--nocapture",
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &f.root)
        .env(
            "AI_TEAM_UPDATE_TEST_DB",
            f.installation.database.as_ref().unwrap(),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("independent writer refused"));
}

#[test]
fn writer_process_fixture() {
    let Some(path) = std::env::var_os("AI_TEAM_UPDATE_TEST_DB") else {
        return;
    };
    assert!(Store::open(&PathBuf::from(path))
        .unwrap_err()
        .to_string()
        .contains("updating"));
    println!("independent writer refused");
}
