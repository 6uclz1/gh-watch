use std::{
    env, fs,
    path::Path,
    sync::{Mutex, OnceLock},
};

use gh_watch::config::{
    parse_config, resolve_config_path, resolve_config_path_with_source, stability_warnings,
    ConfigPathSource,
};
use tempfile::tempdir;

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
    assert_eq!(cfg.bootstrap_lookback_hours, 24);
    assert_eq!(cfg.timeline_limit, 500);
    assert_eq!(cfg.retention_days, 90);
    assert_eq!(cfg.repositories.len(), 1);
    assert!(cfg.repositories[0].enabled);
    assert!(cfg.notifications.enabled);
    assert!(cfg.notifications.include_url);
    assert!(cfg.filters.event_kinds.is_empty());
    assert!(cfg.filters.ignore_actors.is_empty());
    assert!(!cfg.filters.only_involving_me);
    assert_eq!(cfg.poll.max_concurrency, None);
    assert_eq!(cfg.poll.timeout_seconds, 30);
}

#[test]
fn stability_warnings_include_short_interval_warning() {
    let src = r#"
interval_seconds = 10

[[repositories]]
name = "octocat/hello-world"
"#;
    let cfg = parse_config(src).expect("config should parse");

    let warnings = stability_warnings(&cfg);
    assert!(warnings
        .iter()
        .any(|warning| warning.contains("interval_seconds")));
    assert!(warnings.iter().any(|warning| warning.contains(">= 30")));
}

#[test]
fn stability_warnings_include_deprecated_parallelism_warning() {
    let src = r#"
[poll]
max_concurrency = 4

[[repositories]]
name = "octocat/hello-world"
"#;
    let cfg = parse_config(src).expect("config should parse");

    let warnings = stability_warnings(&cfg);
    assert!(warnings
        .iter()
        .any(|warning| warning.contains("poll.max_concurrency is deprecated")));
    assert!(warnings.iter().any(|warning| warning.contains("ignored")));
}

#[test]
fn parse_config_rejects_removed_notification_sender_keys() {
    let src = r#"
[notifications]
enabled = true
include_url = true
macos_bundle_id = "com.example.CustomMacApp"
windows_app_id = "com.example.CustomWinApp"
wsl_windows_app_id = "com.example.CustomWslApp"

[[repositories]]
name = "octocat/hello-world"
"#;

    let err = parse_config(src).expect_err("removed keys should be rejected");
    let msg = format!("{err:#}");
    assert!(msg.contains("failed to parse config TOML"));
    assert!(msg.contains("macos_bundle_id"));
}

#[test]
fn parse_config_rejects_removed_failure_history_limit_key() {
    let src = r#"
failure_history_limit = 200

[[repositories]]
name = "octocat/hello-world"
"#;

    let err = parse_config(src).expect_err("removed key should fail");
    let msg = format!("{err:#}");
    assert!(msg.contains("failed to parse config TOML"));
    assert!(msg.contains("failure_history_limit"));
}

#[test]
fn parse_config_accepts_zero_poll_max_concurrency_for_compatibility() {
    let src = r#"
[poll]
max_concurrency = 0

[[repositories]]
name = "octocat/hello-world"
"#;

    let cfg = parse_config(src).expect("max_concurrency should be accepted for compatibility");
    assert_eq!(cfg.poll.max_concurrency, Some(0));
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
fn parse_config_rejects_zero_bootstrap_lookback_hours() {
    let src = r#"
bootstrap_lookback_hours = 0

[[repositories]]
name = "octocat/hello-world"
"#;

    let err = parse_config(src).expect_err("zero bootstrap lookback should fail");
    assert!(err.to_string().contains("bootstrap_lookback_hours"));
}

#[test]
fn parse_config_parses_global_filters_and_repo_override_event_kinds() {
    let src = r#"
[filters]
event_kinds = ["pr_created", "issue_created"]
ignore_actors = ["dependabot[bot]"]
only_involving_me = true

[[repositories]]
name = "octocat/hello-world"
event_kinds = ["pr_created"]
"#;

    let cfg = parse_config(src).expect("config should parse");
    assert_eq!(cfg.filters.event_kinds.len(), 2);
    assert_eq!(
        cfg.filters.ignore_actors,
        vec!["dependabot[bot]".to_string()]
    );
    assert!(cfg.filters.only_involving_me);
    assert_eq!(cfg.repositories.len(), 1);
    assert_eq!(
        cfg.repositories[0]
            .event_kinds
            .as_ref()
            .expect("repo override should exist")
            .len(),
        1
    );
}

#[test]
fn parse_config_rejects_unknown_filter_event_kind() {
    let src = r#"
[filters]
event_kinds = ["unknown_kind"]

[[repositories]]
name = "octocat/hello-world"
"#;

    let err = parse_config(src).expect_err("unknown kind should fail");
    let msg = format!("{err:#}");
    assert!(msg.contains("unknown_kind"));
    assert!(msg.contains("failed to parse config TOML"));
}

#[test]
fn resolve_config_path_uses_explicit_path_when_provided() {
    let explicit_path = env::temp_dir().join("custom-config.toml");
    let explicit = explicit_path.as_path();
    let resolved = resolve_config_path_with_source(Some(explicit)).expect("path should resolve");
    assert_eq!(resolved.path, explicit);
    assert_eq!(resolved.source, ConfigPathSource::ExplicitArg);
}

#[test]
fn resolve_config_path_uses_gh_watch_config_env_before_other_candidates() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempdir().unwrap();
    let env_config = dir.path().join("env-config.toml");
    let cwd = dir.path().join("cwd");
    fs::create_dir_all(&cwd).unwrap();
    fs::write(
        cwd.join("config.toml"),
        "[[repositories]]\nname=\"acme/repo\"\n",
    )
    .unwrap();

    with_current_dir(&cwd, || {
        env::set_var("GH_WATCH_CONFIG", &env_config);
        let resolved = resolve_config_path_with_source(None).expect("path should resolve");
        assert_eq!(resolved.path, env_config);
        assert_eq!(resolved.source, ConfigPathSource::EnvironmentVariable);
    });
    env::remove_var("GH_WATCH_CONFIG");
}

#[test]
fn resolve_config_path_uses_cwd_when_env_is_not_set() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    env::remove_var("GH_WATCH_CONFIG");

    let dir = tempdir().unwrap();
    let cwd_config = dir.path().join("config.toml");
    fs::write(&cwd_config, "[[repositories]]\nname=\"acme/repo\"\n").unwrap();

    with_current_dir(dir.path(), || {
        let resolved = resolve_config_path_with_source(None).expect("path should resolve");
        assert_eq!(
            fs::canonicalize(&resolved.path).unwrap(),
            fs::canonicalize(&cwd_config).unwrap()
        );
        assert_eq!(resolved.source, ConfigPathSource::CurrentDirectory);
    });
}

#[test]
fn resolve_config_path_falls_back_to_binary_directory_when_cwd_missing() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    env::remove_var("GH_WATCH_CONFIG");

    let dir = tempdir().unwrap();
    with_current_dir(dir.path(), || {
        struct FileRestoreGuard {
            path: std::path::PathBuf,
            previous: Option<String>,
        }
        impl Drop for FileRestoreGuard {
            fn drop(&mut self) {
                if let Some(previous) = &self.previous {
                    fs::write(&self.path, previous).expect("restore config should succeed");
                } else if self.path.exists() {
                    fs::remove_file(&self.path).expect("cleanup config should succeed");
                }
            }
        }

        let expected = env::current_exe()
            .expect("current exe should resolve")
            .parent()
            .expect("exe should have a parent")
            .join("config.toml");

        let _restore = FileRestoreGuard {
            previous: fs::read_to_string(&expected).ok(),
            path: expected.clone(),
        };
        fs::write(&expected, "[[repositories]]\nname=\"acme/repo\"\n").unwrap();

        let resolved = resolve_config_path_with_source(None).expect("path should resolve");
        assert_eq!(resolve_config_path(None).unwrap(), expected);
        assert_eq!(resolved.path, expected);
        assert_eq!(resolved.source, ConfigPathSource::BinaryDirectory);
    });
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn with_current_dir(path: &Path, test: impl FnOnce()) {
    struct CwdGuard(std::path::PathBuf);
    impl Drop for CwdGuard {
        fn drop(&mut self) {
            env::set_current_dir(&self.0).expect("restore current dir should succeed");
        }
    }

    let previous = env::current_dir().expect("current dir should resolve");
    let _guard = CwdGuard(previous);
    env::set_current_dir(path).expect("set current dir should succeed");
    test();
}
