use super::super::{Host, Method};
use super::{files, UpdateInstallation};
use crate::{Error, Result};
use std::{
    ffi::OsStr,
    fs,
    io::Read as _,
    path::{Path, PathBuf},
};

fn install_roots(home: &Path, system_bin: &Path, method: Method) -> Vec<PathBuf> {
    let mut roots = vec![home.join(".local/bin"), home.join(".cargo/bin")];
    // The bootstrap falls back here when neither user bin is on PATH. Only its
    // release registration authorizes a direct native binary, never a Cellar link.
    if method == Method::Release {
        roots.push(system_bin.to_path_buf());
    }
    roots
}

pub(super) fn discover(host: Host) -> Result<UpdateInstallation> {
    let binary = std::env::current_exe()?.canonicalize()?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| Error::invalid("HOME is not set"))?;
    let data_dir = crate::paths::data_dir()?;
    let method = super::super::method();
    let roots = install_roots(&home, Path::new("/usr/local/bin"), method);
    let candidates = if host == Host::Cli {
        vec![binary.clone()]
    } else {
        let mut paths: Vec<_> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).map(|p| p.join("ait")).collect())
            .unwrap_or_default();
        // Finder's PATH omits user-installed CLIs. Look in actual install locations,
        // but never create a second CLI just because PATH did not contain the first.
        paths.extend(roots.iter().map(|p| p.join("ait")));
        // Detect system copies too. Canonicalization below still refuses package
        // manager links outside the bootstrap's supported install roots.
        paths.extend(["/usr/local/bin/ait", "/opt/homebrew/bin/ait"].map(PathBuf::from));
        paths
    };
    let (cli, mut blocked) = installed_cli(&candidates, &roots, method)?;
    if binary.components().any(|c| c.as_os_str() == "target") {
        blocked = Some(
            "run updates from the installed CLI or desktop app, not a development checkout".into(),
        );
    }
    if files::resolved(&data_dir)? != files::resolved(&home.join(".ai-team"))? {
        blocked = Some("this process uses isolated state; update from your normal installed app or CLI instead".into());
    }
    let desktop = if cfg!(target_os = "macos") {
        if host == Host::Desktop {
            super::super::bundle_of(&binary)
        } else {
            let candidates = [
                PathBuf::from("/Applications/ai-team.app"),
                PathBuf::from("/Applications/AI Team.app"),
                home.join("Applications/ai-team.app"),
                home.join("Applications/AI Team.app"),
            ];
            let mut found = candidates
                .into_iter()
                .filter(|p| p.is_dir())
                .map(|p| p.canonicalize())
                .collect::<std::io::Result<Vec<_>>>()?;
            found.sort();
            found.dedup();
            if found.len() > 1 {
                blocked =
                    Some("multiple AI Team apps are installed; update from the app you use".into());
            }
            found.into_iter().next()
        }
    } else {
        if host == Host::Desktop {
            blocked = Some("this Linux desktop is package-managed; install its AppImage or .deb from the release page. It will not be overwritten with the CLI".into());
        }
        None
    };
    let database = Some(crate::default_db_path()?);
    let config_dir = std::env::var_os("XDG_CONFIG_HOME")
        .map_or_else(|| home.join(".config"), PathBuf::from)
        .join("ai-team");
    Ok(UpdateInstallation {
        host,
        cli,
        desktop,
        home,
        data_dir,
        database,
        config_dir: Some(config_dir),
        method,
        blocked,
    })
}

fn installed_cli(
    candidates: &[PathBuf],
    managed_roots: &[PathBuf],
    method: Method,
) -> Result<(Option<PathBuf>, Option<String>)> {
    let mut found = candidates
        .iter()
        .filter(|p| p.is_file())
        .map(|p| p.canonicalize())
        .collect::<std::io::Result<Vec<_>>>()?;
    found.sort();
    found.dedup();
    let Some(path) = found.first().cloned() else {
        return Ok((None, None));
    };
    if found.len() > 1 {
        return Ok((Some(path), Some(format!(
            "multiple CLI installations were found: {}. Choose the CLI you use and run its update command; the desktop will not guess",
            found.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
        ))));
    }
    let roots = managed_roots
        .iter()
        .map(|p| files::resolved(p))
        .collect::<Result<Vec<_>>>()?;
    if method == Method::Unknown
        || path.file_name() != Some(OsStr::new("ait"))
        || !roots.iter().any(|p| Some(p.as_path()) == path.parent())
    {
        return Ok((Some(path), Some("the CLI is package-managed or outside AI Team's install locations; update it with its own installer rather than overwrite it".into())));
    }
    if !native_binary(&path)? {
        return Ok((Some(path), Some("the CLI is truncated, a launcher or source shim, not an installed binary; it will not be overwritten".into())));
    }
    Ok((Some(path), None))
}

fn native_binary(path: &Path) -> Result<bool> {
    let mut magic = [0; 4];
    match fs::File::open(path)?.read_exact(&mut magic) {
        Ok(()) => Ok(matches!(
            magic,
            [0x7f, b'E', b'L', b'F']
                | [0xcf, 0xfa, 0xed, 0xfe]
                | [0xfe, 0xed, 0xfa, 0xcf]
                | [0xca, 0xfe, 0xba, 0xbe | 0xbf]
        )),
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn finder_accepts_the_bootstrap_system_cli_only_for_known_release_installs() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let bin = root.path().join("usr/local/bin");
        fs::create_dir_all(&bin).unwrap();
        let cli = bin.join("ait");
        fs::write(&cli, b"\x7fELF").unwrap();
        let roots = install_roots(&home, &bin, Method::Release);
        let (found, blocked) =
            installed_cli(std::slice::from_ref(&cli), &roots, Method::Release).unwrap();
        assert_eq!(found, Some(cli.canonicalize().unwrap()));
        assert!(blocked.is_none(), "{blocked:?}");
        for method in [Method::Unknown, Method::Source] {
            let roots = install_roots(&home, &bin, method);
            assert!(installed_cli(std::slice::from_ref(&cli), &roots, method)
                .unwrap()
                .1
                .is_some());
        }
        let package = root.path().join("Cellar/ai-team/ait");
        fs::create_dir_all(package.parent().unwrap()).unwrap();
        fs::rename(&cli, &package).unwrap();
        std::os::unix::fs::symlink(&package, &cli).unwrap();
        assert!(installed_cli(&[cli], &roots, Method::Release)
            .unwrap()
            .1
            .is_some());
    }

    #[test]
    fn finder_reports_truncated_clis_as_blocked_without_executing_them() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        for bytes in [b"".as_slice(), b"#!", b"\x7fEL"] {
            let cli = bin.join("ait");
            fs::write(&cli, bytes).unwrap();
            let (_, blocked) =
                installed_cli(&[cli], std::slice::from_ref(&bin), Method::Release).unwrap();
            assert!(blocked.is_some());
        }
    }

    #[test]
    fn finder_finds_existing_user_cli_without_creating_a_second_one() {
        let root = tempfile::tempdir().unwrap();
        let absent = root.path().join("local");
        let cargo = root.path().join("cargo");
        fs::create_dir(&cargo).unwrap();
        fs::write(cargo.join("ait"), b"\x7fELF").unwrap();
        let roots = [absent.clone(), cargo.clone()];
        let (cli, blocked) = installed_cli(
            &[absent.join("ait"), cargo.join("ait")],
            &roots,
            Method::Source,
        )
        .unwrap();
        assert_eq!(cli, Some(cargo.join("ait").canonicalize().unwrap()));
        assert!(blocked.is_none());
        assert!(!absent.exists());
        assert_eq!(
            installed_cli(&[absent.join("ait")], &roots, Method::Release).unwrap(),
            (None, None)
        );
    }

    #[test]
    fn finder_refuses_multiple_distinct_clis_but_deduplicates_aliases() {
        let root = tempfile::tempdir().unwrap();
        let roots = [root.path().join("local"), root.path().join("cargo")];
        for bin in &roots {
            fs::create_dir(bin).unwrap();
            fs::write(bin.join("ait"), b"\x7fELF").unwrap();
        }
        let candidates: Vec<_> = roots.iter().map(|bin| bin.join("ait")).collect();
        let (_, blocked) = installed_cli(&candidates, &roots, Method::Release).unwrap();
        let blocked = blocked.expect("Finder must not guess which CLI the shell uses");
        for path in &candidates {
            assert!(blocked.contains(path.to_str().unwrap()), "{blocked}");
        }
        let (_, blocked) = installed_cli(
            &[candidates[0].clone(), candidates[0].clone()],
            &roots,
            Method::Release,
        )
        .unwrap();
        assert!(blocked.is_none());
    }

    #[test]
    fn registration_does_not_authorize_a_package_manager_symlink_or_source_shim() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        let package = root.path().join("package");
        fs::create_dir(&bin).unwrap();
        fs::create_dir(&package).unwrap();
        fs::write(package.join("ait"), b"\x7fELF").unwrap();
        std::os::unix::fs::symlink(package.join("ait"), bin.join("ait")).unwrap();
        assert!(installed_cli(
            &[bin.join("ait")],
            std::slice::from_ref(&bin),
            Method::Release
        )
        .unwrap()
        .1
        .is_some());
        fs::remove_file(bin.join("ait")).unwrap();
        fs::write(bin.join("ait"), b"#!/bin/sh\n").unwrap();
        assert!(installed_cli(
            &[bin.join("ait")],
            std::slice::from_ref(&bin),
            Method::Source
        )
        .unwrap()
        .1
        .is_some());
        fs::write(bin.join("ait"), b"\x7fELF").unwrap();
        assert!(installed_cli(&[bin.join("ait")], &[bin], Method::Unknown)
            .unwrap()
            .1
            .is_some());
    }
}
