use std::{fs, path::PathBuf};

use assert_cmd::{cargo::cargo_bin_cmd, Command};
use predicates::str::contains;
use tempfile::{tempdir, TempDir};

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

#[test]
fn init_without_path_uses_binary_directory_config() {
    let (dir, bin_path) = copy_binary_to_tempdir();
    let config_path = dir.path().join("config.toml");

    let mut cmd = Command::new(&bin_path);
    cmd.arg("init").assert().success();

    let content = fs::read_to_string(config_path).unwrap();
    assert!(content.contains("[[repositories]]"));
}

fn copy_binary_to_tempdir() -> (TempDir, PathBuf) {
    let dir = tempdir().unwrap();
    let src = assert_cmd::cargo::cargo_bin!("gh-watch");
    let bin_name = if cfg!(windows) {
        "gh-watch.exe"
    } else {
        "gh-watch"
    };
    let dst = dir.path().join(bin_name);
    fs::copy(src, &dst).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = fs::metadata(&dst).unwrap().permissions();
        perm.set_mode(0o755);
        fs::set_permissions(&dst, perm).unwrap();
    }

    (dir, dst)
}
