use std::fs;

use assert_cmd::cargo::cargo_bin_cmd;
use predicates::str::contains;
use tempfile::tempdir;

#[test]
fn init_creates_config_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("init").arg("--path").arg(&path).assert().success();

    let content = fs::read_to_string(path).unwrap();
    assert!(content.contains("[[repositories]]"));
    assert!(content.contains("[notifications]"));
}

#[test]
fn init_prevents_overwrite_without_force() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "interval_seconds = 123\n").unwrap();

    let mut fail_cmd = cargo_bin_cmd!("gh-watch");
    fail_cmd
        .arg("init")
        .arg("--path")
        .arg(&path)
        .assert()
        .failure()
        .stderr(contains("use --force to overwrite"));

    let mut ok_cmd = cargo_bin_cmd!("gh-watch");
    ok_cmd
        .arg("init")
        .arg("--path")
        .arg(&path)
        .arg("--force")
        .assert()
        .success();

    let content = fs::read_to_string(path).unwrap();
    assert!(content.contains("[[repositories]]"));
}
