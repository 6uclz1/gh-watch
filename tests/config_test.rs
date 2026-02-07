use gh_watch::config::parse_config;

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
