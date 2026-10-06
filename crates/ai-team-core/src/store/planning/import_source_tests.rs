use super::*;

#[test]
fn snapshot_discards_stale_journals_before_retrying() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let copy = dir.path().join("copy");
    std::fs::write(&source, b"new checkpoint").unwrap();
    for suffix in ["-wal", "-shm", "-journal"] {
        std::fs::write(with_suffix(&copy, suffix), b"stale attempt").unwrap();
    }
    snapshot(&source, &copy).unwrap();
    for suffix in ["-wal", "-shm", "-journal"] {
        assert!(
            !with_suffix(&copy, suffix).exists(),
            "stale {suffix} survived"
        );
    }
}

#[test]
fn copied_owned_planning_store_is_not_a_standalone_import_source() {
    let dir = tempfile::tempdir().unwrap();
    let owned = dir.path().join("owned.sqlite");
    super::super::engine::open(&owned, true).unwrap();
    let copy = dir.path().join("copy.sqlite");
    std::fs::copy(&owned, &copy).unwrap();
    let result = Source::open(copy.to_str().unwrap(), &[owned]);
    assert!(
        result.is_err(),
        "a copied owned store must not create a second writable plan"
    );
}
