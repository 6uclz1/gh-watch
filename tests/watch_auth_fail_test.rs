use std::{fs, io::Write};

use assert_cmd::cargo::cargo_bin_cmd;
use tempfile::tempdir;

#[test]
fn watch_fails_fast_when_gh_auth_is_invalid() {
    let dir = tempdir().unwrap();

    let gh_path = dir.path().join("gh");
    let script = r#"#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == "auth" && "$2" == "status" ]]; then
  echo "token invalid" >&2
  exit 1
fi
echo "unexpected: $@" >&2
exit 1
"#;
    fs::write(&gh_path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = fs::metadata(&gh_path).unwrap().permissions();
        perm.set_mode(0o755);
        fs::set_permissions(&gh_path, perm).unwrap();
    }

    let cfg_path = dir.path().join("config.toml");
    let mut cfg = fs::File::create(&cfg_path).unwrap();
    writeln!(cfg, "[[repositories]]").unwrap();
    writeln!(cfg, "name = \"acme/api\"").unwrap();

    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("watch")
        .arg("--config")
        .arg(&cfg_path)
        .env("GH_WATCH_GH_BIN", &gh_path)
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "GitHub authentication is invalid",
        ));
}
