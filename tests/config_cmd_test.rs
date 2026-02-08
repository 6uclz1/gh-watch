use std::{env, fs, path::PathBuf};

use assert_cmd::Command;
use predicates::str::contains;
use tempfile::{tempdir, TempDir};

#[test]
fn config_path_prints_binary_directory_config() {
    let (dir, bin_path) = copy_binary_to_tempdir();
    let expected = dir.path().join("config.toml");

    let mut cmd = Command::new(&bin_path);
    cmd.arg("config")
        .arg("path")
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(contains(expected.to_string_lossy().to_string()))
        .stdout(contains("source: ./config.toml"));
}

#[test]
fn config_open_fails_when_config_is_missing() {
    let (dir, bin_path) = copy_binary_to_tempdir();

    let mut cmd = Command::new(&bin_path);
    cmd.arg("config")
        .arg("open")
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(contains("run `gh-watch init`"));
}

#[test]
fn config_doctor_reports_missing_selected_config() {
    let (dir, bin_path) = copy_binary_to_tempdir();

    let mut cmd = Command::new(&bin_path);
    cmd.arg("config")
        .arg("doctor")
        .current_dir(dir.path())
        .env_remove("GH_WATCH_CONFIG")
        .assert()
        .success()
        .stdout(contains("selected:"))
        .stdout(contains("next: run `gh-watch init`"));
}

#[test]
fn config_doctor_reports_parse_error_for_invalid_selected_config() {
    let (dir, bin_path) = copy_binary_to_tempdir();
    fs::write(dir.path().join("config.toml"), "not = [valid").unwrap();

    let mut cmd = Command::new(&bin_path);
    cmd.arg("config")
        .arg("doctor")
        .current_dir(dir.path())
        .env_remove("GH_WATCH_CONFIG")
        .assert()
        .success()
        .stdout(contains("doctor: selected config has errors"))
        .stdout(contains("parse=error"));
}

#[cfg(unix)]
#[test]
fn config_edit_alias_uses_visual_editor() {
    let (dir, bin_path) = copy_binary_to_tempdir();
    let config_path = dir.path().join("config.toml");
    fs::write(&config_path, "[[repositories]]\nname = \"acme/api\"\n").unwrap();

    let marker = dir.path().join("marker.txt");
    let visual = dir.path().join("visual-editor");
    write_executable(
        &visual,
        r#"#!/usr/bin/env bash
set -euo pipefail
echo "visual:$1" > "$MARKER"
"#,
    );

    let mut cmd = Command::new(&bin_path);
    cmd.arg("config")
        .arg("edit")
        .current_dir(dir.path())
        .env("VISUAL", &visual)
        .env_remove("EDITOR")
        .env("MARKER", &marker)
        .assert()
        .success();

    let got = fs::read_to_string(&marker).unwrap();
    assert_marker_path(&got, "visual:", &config_path);
}

#[cfg(unix)]
#[test]
fn config_open_uses_editor_when_visual_fails() {
    let (dir, bin_path) = copy_binary_to_tempdir();
    let config_path = dir.path().join("config.toml");
    fs::write(&config_path, "[[repositories]]\nname = \"acme/api\"\n").unwrap();

    let marker = dir.path().join("marker.txt");
    let visual = dir.path().join("visual-editor");
    let editor = dir.path().join("terminal-editor");
    write_executable(
        &visual,
        r#"#!/usr/bin/env bash
set -euo pipefail
exit 1
"#,
    );
    write_executable(
        &editor,
        r#"#!/usr/bin/env bash
set -euo pipefail
echo "editor:$1" > "$MARKER"
"#,
    );

    let mut cmd = Command::new(&bin_path);
    cmd.arg("config")
        .arg("open")
        .current_dir(dir.path())
        .env("VISUAL", &visual)
        .env("EDITOR", &editor)
        .env("MARKER", &marker)
        .assert()
        .success();

    let got = fs::read_to_string(&marker).unwrap();
    assert_marker_path(&got, "editor:", &config_path);
}

#[cfg(unix)]
#[test]
fn config_open_falls_back_to_os_default_opener() {
    let (dir, bin_path) = copy_binary_to_tempdir();
    let config_path = dir.path().join("config.toml");
    fs::write(&config_path, "[[repositories]]\nname = \"acme/api\"\n").unwrap();

    let marker = dir.path().join("marker.txt");
    let opener = opener_script_path(dir.path());
    write_executable(
        &opener,
        r#"#!/usr/bin/env bash
set -euo pipefail
echo "os:$1" > "$MARKER"
"#,
    );

    let old_path = env::var_os("PATH").unwrap_or_default();
    let mut composed = dir.path().as_os_str().to_os_string();
    composed.push(if cfg!(windows) { ";" } else { ":" });
    composed.push(old_path);

    let mut cmd = Command::new(&bin_path);
    cmd.arg("config")
        .arg("open")
        .current_dir(dir.path())
        .env_remove("VISUAL")
        .env_remove("EDITOR")
        .env("MARKER", &marker)
        .env("PATH", composed)
        .assert()
        .success();

    let got = fs::read_to_string(&marker).unwrap();
    assert_marker_path(&got, "os:", &config_path);
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

#[cfg(unix)]
fn write_executable(path: &PathBuf, script: &str) {
    fs::write(path, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perm = fs::metadata(path).unwrap().permissions();
    perm.set_mode(0o755);
    fs::set_permissions(path, perm).unwrap();
}

#[cfg(unix)]
fn opener_script_path(base: &std::path::Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        return base.join("open");
    }
    #[cfg(target_os = "linux")]
    {
        return base.join("xdg-open");
    }
    #[allow(unreachable_code)]
    base.join("open")
}

fn assert_marker_path(raw: &str, prefix: &str, expected: &PathBuf) {
    let trimmed = raw.trim();
    assert!(
        trimmed.starts_with(prefix),
        "marker should start with {prefix}, got {trimmed}"
    );
    let actual = trimmed.trim_start_matches(prefix);
    let actual = fs::canonicalize(actual).unwrap();
    let expected = fs::canonicalize(expected).unwrap();
    assert_eq!(actual, expected);
}
