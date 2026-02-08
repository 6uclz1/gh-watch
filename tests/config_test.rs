use std::{
    env,
    sync::{Mutex, OnceLock},
};

use gh_watch::config::{parse_config, resolve_config_path};

#[test]
fn parse_config_rejects_invalid_repo_name() {
    let src = r#"
interval_seconds = 300

[[repositories]]
name = "invalid-repo-name"
"#;

    let err = parse_config(src).expect_err("invalid repo should fail");
    assert!(err.to_string().contains("owner/repo"));
}

#[test]
fn parse_config_applies_defaults() {
    let src = r#"
[[repositories]]
name = "octocat/hello-world"
"#;

    let cfg = parse_config(src).expect("config should parse");
    assert_eq!(cfg.interval_seconds, 300);
    assert_eq!(cfg.timeline_limit, 500);
    assert_eq!(cfg.retention_days, 90);
    assert_eq!(cfg.failure_history_limit, 200);
    assert_eq!(cfg.repositories.len(), 1);
    assert!(cfg.repositories[0].enabled);
    assert!(cfg.notifications.enabled);
    assert!(cfg.notifications.include_url);
    assert_eq!(cfg.poll.max_concurrency, 4);
    assert_eq!(cfg.poll.timeout_seconds, 30);
}

#[test]
fn parse_config_rejects_zero_failure_history_limit() {
    let src = r#"
failure_history_limit = 0

[[repositories]]
name = "octocat/hello-world"
"#;

    let err = parse_config(src).expect_err("zero limit should fail");
    assert!(err.to_string().contains("failure_history_limit"));
}

#[test]
fn parse_config_rejects_zero_poll_max_concurrency() {
    let src = r#"
[poll]
max_concurrency = 0

[[repositories]]
name = "octocat/hello-world"
"#;

    let err = parse_config(src).expect_err("zero max_concurrency should fail");
    assert!(err.to_string().contains("poll.max_concurrency"));
}

#[test]
fn parse_config_rejects_zero_poll_timeout_seconds() {
    let src = r#"
[poll]
timeout_seconds = 0

[[repositories]]
name = "octocat/hello-world"
"#;

    let err = parse_config(src).expect_err("zero timeout_seconds should fail");
    assert!(err.to_string().contains("poll.timeout_seconds"));
}

#[test]
fn resolve_config_path_uses_explicit_path_when_provided() {
    let explicit_path = env::temp_dir().join("custom-config.toml");
    let explicit = explicit_path.as_path();
    let resolved = resolve_config_path(Some(explicit)).expect("path should resolve");
    assert_eq!(resolved, explicit);
}

#[test]
fn resolve_config_path_defaults_to_installed_location() {
    let resolved = resolve_config_path(None).expect("path should resolve");
    let exe = env::current_exe().expect("current exe should resolve");
    let expected = exe
        .parent()
        .expect("exe should have a parent")
        .join("config.toml");
    assert_eq!(resolved, expected);
}

#[test]
fn resolve_config_path_ignores_gh_watch_config_env() {
    let _guard = env_lock().lock().expect("env lock should work");
    let prev = env::var_os("GH_WATCH_CONFIG");
    env::set_var("GH_WATCH_CONFIG", "/tmp/legacy-config.toml");

    let resolved = resolve_config_path(None).expect("path should resolve");
    let exe = env::current_exe().expect("current exe should resolve");
    let expected = exe
        .parent()
        .expect("exe should have a parent")
        .join("config.toml");
    assert_eq!(resolved, expected);

    if let Some(prev) = prev {
        env::set_var("GH_WATCH_CONFIG", prev);
    } else {
        env::remove_var("GH_WATCH_CONFIG");
    }
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
