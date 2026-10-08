use super::*;

#[test]
fn sha256_matches_the_published_vectors() {
    for (bytes, expected) in [
        (
            b"".to_vec(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            b"abc".to_vec(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        (
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq".to_vec(),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
        ),
        (
            b"a".repeat(1_000_000),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0",
        ),
    ] {
        assert_eq!(sha256(&bytes), expected);
    }
}

#[test]
fn a_cli_standing_in_for_the_app_is_recognised() {
    assert_eq!(
        replaced_app(Path::new(
            "/Applications/ai-team.app/Contents/MacOS/ai-team"
        )),
        Some(PathBuf::from("/Applications/ai-team.app"))
    );
    for path in [
        "/Users/me/.local/bin/ait",
        "/usr/local/bin/ait",
        "/Applications/ai-team.app/Contents/MacOS/ait",
    ] {
        assert_eq!(replaced_app(Path::new(path)), None);
    }
}

#[test]
fn a_bundle_is_recognised_by_its_shape_not_its_name() {
    for name in ["ai-team", "Whatever"] {
        assert_eq!(
            bundle_of(Path::new(&format!(
                "/Applications/{name}.app/Contents/MacOS/anything"
            ))),
            Some(PathBuf::from(format!("/Applications/{name}.app")))
        );
    }
    for path in [
        "/usr/local/bin/ait",
        "/Applications/ai-team.app/ai-team",
        "/Applications/ai-team/Contents/MacOS/ai-team",
    ] {
        assert_eq!(bundle_of(Path::new(path)), None);
    }
}

#[test]
fn prereleases_and_build_metadata_follow_semver_precedence() {
    assert!(!is_newer("1.0.0-rc.1", "1.0.0"));
    assert!(is_newer("1.0.0", "1.0.0-rc.1"));
    assert!(!is_newer("1.0.0+build.9", "1.0.0"));
    assert!(!is_newer("garbage", "0.0.0"));
}

#[test]
fn versions_compare_numerically_not_alphabetically() {
    assert!(is_newer("0.10.0", "0.9.0"));
    assert!(is_newer("1.0.0", "0.99.99"));
    assert!(is_newer("0.2.1", "0.2.0"));
    assert!(!is_newer("0.2.0", "0.2.0"));
    assert!(!is_newer("0.1.9", "0.2.0"));
}

#[test]
fn a_shorter_version_is_not_automatically_older() {
    assert!(!is_newer("1.0", "1.0.0"));
    assert!(is_newer("1.1", "1.0.9"));
}

#[test]
fn a_checksum_that_differs_stops_the_update() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("thing.tar.gz");
    std::fs::write(&archive, b"abc").unwrap();
    let sums = dir.path().join("checksums.txt");
    std::fs::write(&sums, format!("{}  thing.tar.gz\n", sha256(b"abc"))).unwrap();
    assert!(verify(&archive, &sums, "thing.tar.gz").is_ok());
    std::fs::write(&sums, "0000  thing.tar.gz\n").unwrap();
    assert!(verify(&archive, &sums, "thing.tar.gz")
        .unwrap_err()
        .to_string()
        .contains("checksum"));
}

#[test]
fn a_missing_or_suffix_matching_checksum_refuses_installation() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("thing.tar.gz");
    std::fs::write(&archive, b"abc").unwrap();
    let sums = dir.path().join("checksums.txt");
    assert!(verify(&archive, &sums, "thing.tar.gz").is_err());
    for name in ["something-else.tar.gz", "not-thing.tar.gz"] {
        std::fs::write(&sums, format!("{}  {name}\n", sha256(b"abc"))).unwrap();
        assert!(verify(&archive, &sums, "thing.tar.gz").is_err());
    }
}

#[test]
fn the_asset_name_matches_what_the_release_publishes() {
    let name = asset_for("0.2.0");
    match std::env::consts::OS {
        "macos" => assert_eq!(
            name.as_deref(),
            Some("ai-team-v0.2.0-macos-universal.tar.gz")
        ),
        "linux" => assert!(name
            .as_deref()
            .is_some_and(|name| name.starts_with("ai-team-v0.2.0-linux-"))),
        _ => assert!(name.is_none()),
    }
}
