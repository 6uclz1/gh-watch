use assert_cmd::cargo::cargo_bin_cmd;
use predicates::str::contains;

#[test]
fn completion_generates_bash_script() {
    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("completion")
        .arg("bash")
        .assert()
        .success()
        .stdout(contains("complete"))
        .stdout(contains("gh-watch"));
}

#[test]
fn completion_generates_zsh_script() {
    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("completion")
        .arg("zsh")
        .assert()
        .success()
        .stdout(contains("#compdef gh-watch"));
}

#[test]
fn completion_generates_fish_script() {
    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("completion")
        .arg("fish")
        .assert()
        .success()
        .stdout(contains("complete -c gh-watch"));
}

#[test]
fn completion_generates_pwsh_script() {
    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("completion")
        .arg("pwsh")
        .assert()
        .success()
        .stdout(contains("Register-ArgumentCompleter"))
        .stdout(contains("gh-watch"));
}

#[test]
fn completion_accepts_powershell_alias() {
    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("completion")
        .arg("powershell")
        .assert()
        .success()
        .stdout(contains("Register-ArgumentCompleter"))
        .stdout(contains("gh-watch"));
}

#[test]
fn completion_rejects_unknown_shell_with_candidates() {
    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("completion")
        .arg("invalid-shell")
        .assert()
        .failure()
        .stderr(contains("invalid value"))
        .stderr(contains("possible values"))
        .stderr(contains("bash"))
        .stderr(contains("zsh"))
        .stderr(contains("fish"))
        .stderr(contains("pwsh"));
}
