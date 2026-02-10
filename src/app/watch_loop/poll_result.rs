use anyhow::Result;

use crate::{app::poll_once::PollOutcome, config::Config, ports::ClockPort, ui::tui::TuiModel};

pub(super) fn enabled_repository_names(config: &Config) -> Vec<String> {
    config
        .repositories
        .iter()
        .filter(|repo| repo.enabled)
        .map(|repo| repo.name.clone())
        .collect()
}

pub(super) fn apply_poll_result<K>(result: Result<PollOutcome>, model: &mut TuiModel, clock: &K)
where
    K: ClockPort,
{
    match result {
        Ok(outcome) => {
            let new_count = outcome.timeline_events.len();
            let repo_failure_count = outcome.fetch_failures.len();
            if new_count > 0 {
                model.push_timeline(outcome.timeline_events);
            }
            if repo_failure_count > 0 {
                model.failure_count += repo_failure_count as u64;
                model.status_line =
                    format!("ok (new={new_count}, repo_failures={repo_failure_count})");
            } else {
                model.status_line = format!("ok (new={new_count})");
            }
            model.last_success_at = Some(clock.now());
        }
        Err(err) => {
            model.failure_count += 1;
            model.status_line = format!("poll failed: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;
    use chrono::{TimeZone, Utc};

    use super::{apply_poll_result, enabled_repository_names};
    use crate::{
        app::poll_once::{PollOutcome, RepoFetchFailure},
        config::{Config, FiltersConfig, NotificationConfig, PollConfig, RepositoryConfig},
        domain::events::{EventKind, WatchEvent},
        ports::ClockPort,
        ui::tui::TuiModel,
    };

    struct FixedClock {
        now: chrono::DateTime<Utc>,
    }

    impl ClockPort for FixedClock {
        fn now(&self) -> chrono::DateTime<Utc> {
            self.now
        }
    }

    fn timeline_event(id: &str, created_at: chrono::DateTime<Utc>) -> WatchEvent {
        WatchEvent {
            event_id: id.to_string(),
            repo: "acme/api".to_string(),
            kind: EventKind::IssueCommentCreated,
            actor: "dev".to_string(),
            title: "comment".to_string(),
            url: format!("https://example.com/{id}"),
            created_at,
            source_item_id: id.to_string(),
            subject_author: Some("dev".to_string()),
            requested_reviewer: None,
            mentions: Vec::new(),
        }
    }

    #[test]
    fn enabled_repository_names_keeps_config_order_and_filters_disabled() {
        let config = Config {
            interval_seconds: 300,
            bootstrap_lookback_hours: 24,
            timeline_limit: 500,
            retention_days: 90,
            state_db_path: None,
            repositories: vec![
                RepositoryConfig {
                    name: "acme/one".to_string(),
                    enabled: true,
                    event_kinds: None,
                },
                RepositoryConfig {
                    name: "acme/two".to_string(),
                    enabled: false,
                    event_kinds: None,
                },
                RepositoryConfig {
                    name: "acme/three".to_string(),
                    enabled: true,
                    event_kinds: None,
                },
            ],
            notifications: NotificationConfig::default(),
            filters: FiltersConfig::default(),
            poll: PollConfig::default(),
        };

        let watched = enabled_repository_names(&config);

        assert_eq!(
            watched,
            vec!["acme/one".to_string(), "acme/three".to_string()]
        );
    }

    #[test]
    fn watch_status_new_uses_timeline_reflections() {
        let clock = FixedClock {
            now: Utc.with_ymd_and_hms(2025, 1, 8, 12, 0, 0).unwrap(),
        };
        let mut model = TuiModel::new(10);

        let outcome = PollOutcome {
            timeline_events: vec![
                timeline_event("ev-a", clock.now),
                timeline_event("ev-b", clock.now),
            ],
            ..PollOutcome::default()
        };
        apply_poll_result(Ok(outcome), &mut model, &clock);
        assert_eq!(model.status_line, "ok (new=2)");

        apply_poll_result(Err(anyhow!("boom")), &mut model, &clock);
        assert_eq!(model.status_line, "poll failed: boom");
        assert_eq!(model.failure_count, 1);
    }

    #[test]
    fn watch_status_tracks_repo_fetch_failures_on_partial_success() {
        let clock = FixedClock {
            now: Utc.with_ymd_and_hms(2025, 1, 8, 12, 0, 0).unwrap(),
        };
        let mut model = TuiModel::new(10);

        let outcome = PollOutcome {
            timeline_events: vec![timeline_event("ev-a", clock.now)],
            fetch_failures: vec![RepoFetchFailure {
                repo: "acme/web".to_string(),
                message: "boom".to_string(),
            }],
            ..PollOutcome::default()
        };
        apply_poll_result(Ok(outcome), &mut model, &clock);

        assert_eq!(model.status_line, "ok (new=1, repo_failures=1)");
        assert_eq!(model.failure_count, 1);
    }
}
