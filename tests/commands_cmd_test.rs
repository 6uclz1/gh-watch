use assert_cmd::cargo::cargo_bin_cmd;
use predicates::str::contains;

#[test]
fn commands_prints_core_commands_and_completion_hint() {
    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("commands")
        .assert()
        .success()
        .stdout(contains("Core Commands"))
        .stdout(contains("gh-watch watch"))
        .stdout(contains("gh-watch once"))
        .stdout(contains("gh-watch check"))
        .stdout(contains("gh-watch init"))
        .stdout(contains("gh-watch config open"))
        .stdout(contains("gh-watch config path"))
        .stdout(contains("gh-watch commands"))
        .stdout(contains("gh-watch completion <shell>"))
        .stdout(contains("gh-watch completion zsh"));
}
