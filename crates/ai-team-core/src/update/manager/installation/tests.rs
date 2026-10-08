use super::*;
use std::cell::Cell;

fn staged() -> (tempfile::TempDir, UpdateInstallation, Prepared) {
    let root = tempfile::tempdir().unwrap();
    let cli = root.path().join("ait");
    let app = root.path().join("ai-team.app");
    fs::write(&cli, "old CLI").unwrap();
    fs::create_dir(&app).unwrap();
    fs::write(app.join("contents"), "old app").unwrap();
    let cli_stage = Stage::new(&cli).unwrap();
    fs::write(cli_stage.0.join("fresh"), "new CLI").unwrap();
    let app_stage = Stage::new(&app).unwrap();
    fs::create_dir(app_stage.0.join("fresh.app")).unwrap();
    fs::write(app_stage.0.join("fresh.app/contents"), "new app").unwrap();
    let installation = UpdateInstallation {
        host: Host::Desktop,
        cli: Some(cli),
        desktop: Some(app),
        home: root.path().into(),
        data_dir: root.path().into(),
        database: None,
        config_dir: None,
        method: Method::Release,
        blocked: None,
    };
    (
        root,
        installation,
        Prepared {
            cli: Some(cli_stage),
            app: Some(app_stage),
        },
    )
}

#[test]
fn either_swap_failure_preserves_both_original_programs() {
    for fail_app in [false, true] {
        let (_root, installation, prepared) = staged();
        let fail = Cell::new(true);
        let destination = if fail_app {
            installation.desktop.as_ref().unwrap()
        } else {
            installation.cli.as_ref().unwrap()
        };
        let result = prepared.install_with(&installation, |from, to| {
            if to == destination && fail.replace(false) {
                return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
            }
            fs::rename(from, to)
        });
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(installation.cli.as_ref().unwrap()).unwrap(),
            "old CLI"
        );
        assert_eq!(
            fs::read_to_string(installation.desktop.as_ref().unwrap().join("contents")).unwrap(),
            "old app"
        );
    }
}

#[test]
fn failed_restoration_keeps_the_original_app_after_staging_drops() {
    let (_root, installation, prepared) = staged();
    let previous = prepared.app.as_ref().unwrap().0.join("previous.app");
    let result = prepared.install_with(&installation, |from, to| {
        if to == installation.cli.as_ref().unwrap() || from == previous {
            return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        }
        fs::rename(from, to)
    });
    assert!(result.unwrap_err().to_string().contains("restore"));
    drop(prepared);
    assert_eq!(
        fs::read_to_string(previous.join("contents")).unwrap(),
        "old app"
    );
    assert_eq!(
        fs::read_to_string(installation.cli.unwrap()).unwrap(),
        "old CLI"
    );
}
