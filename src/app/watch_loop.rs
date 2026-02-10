use std::{future::Future, pin::Pin, process::Command, time::Duration};

use anyhow::{anyhow, Result};
use crossterm::event::Event;
use futures_util::StreamExt;
use ratatui::layout::Rect;
use tokio::time::MissedTickBehavior;

use crate::{
    app::poll_once::{poll_once, PollOutcome},
    config::Config,
    ports::{ClockPort, GhClientPort, NotifierPort, StateStorePort},
    ui::tui::{
        handle_input, parse_input, parse_mouse_input, ActiveTab, InputCommand, TerminalUi, TuiModel,
    },
};

const SPINNER_REDRAW_INTERVAL_MS: u64 = 120;
const ESC_DOUBLE_PRESS_WINDOW_MS: i64 = 1500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopControl {
    Continue,
    RequestPoll,
    Redraw,
    Quit,
}

type PollFuture<'a> = Pin<Box<dyn Future<Output = Result<PollOutcome>> + 'a>>;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct PollExecutionState {
    poll_requested: bool,
    in_flight: bool,
    queued_refresh: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenCommandResult {
    success: bool,
    stderr: String,
}

fn run_open_command(command: &mut Command) -> OpenCommandResult {
    match command.output() {
        Ok(output) => OpenCommandResult {
            success: output.status.success(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        },
        Err(err) => OpenCommandResult {
            success: false,
            stderr: err.to_string(),
        },
    }
}

impl PollExecutionState {
    fn request_poll(&mut self) -> bool {
        if self.in_flight {
            self.queued_refresh = true;
            return false;
        }

        if self.poll_requested {
            return false;
        }

        self.poll_requested = true;
        true
    }

    fn start_poll(&mut self) -> bool {
        if self.in_flight || !self.poll_requested {
            return false;
        }

        self.poll_requested = false;
        self.in_flight = true;
        true
    }

    fn finish_poll_and_take_next_request(&mut self) -> bool {
        self.in_flight = false;

        if self.queued_refresh {
            self.queued_refresh = false;
            self.poll_requested = true;
            return true;
        }

        false
    }

    fn in_flight(&self) -> bool {
        self.in_flight
    }

    fn queued_refresh(&self) -> bool {
        self.queued_refresh
    }
}

fn handle_stream_event<S, K>(
    maybe_event: Option<Result<Event, std::io::Error>>,
    model: &mut TuiModel,
    state: &S,
    clock: &K,
    terminal_area: Rect,
    open_url: &dyn Fn(&str) -> Result<()>,
) -> LoopControl
where
    S: StateStorePort,
    K: ClockPort,
{
    match maybe_event {
        Some(Ok(Event::Key(key))) => {
            let cmd = parse_input(key);
            if cmd != InputCommand::EscapePressed {
                model.esc_armed_until = None;
            }

            match cmd {
                InputCommand::Quit => LoopControl::Quit,
                InputCommand::EscapePressed => {
                    let now = clock.now();
                    if model
                        .esc_armed_until
                        .is_some_and(|armed_until| now <= armed_until)
                    {
                        LoopControl::Quit
                    } else {
                        model.esc_armed_until =
                            Some(now + chrono::Duration::milliseconds(ESC_DOUBLE_PRESS_WINDOW_MS));
                        model.status_line = "press Esc again to quit (1.5s)".to_string();
                        LoopControl::Redraw
                    }
                }
                InputCommand::Refresh => LoopControl::RequestPoll,
                InputCommand::OpenSelectedUrl => {
                    let Some(url) = model
                        .timeline
                        .get(model.selected)
                        .map(|event| event.url.clone())
                    else {
                        return LoopControl::Continue;
                    };

                    match open_url(&url) {
                        Ok(()) => {
                            model.status_line = format!("opened: {url}");
                        }
                        Err(err) => {
                            model.status_line = format!("open failed: {err}");
                        }
                    }
                    mark_selected_event_read(model, state, clock);
                    LoopControl::Redraw
                }
                InputCommand::ToggleHelp | InputCommand::NextTab | InputCommand::PrevTab => {
                    handle_input(model, cmd);
                    LoopControl::Redraw
                }
                InputCommand::ScrollUp
                | InputCommand::ScrollDown
                | InputCommand::PageUp
                | InputCommand::PageDown
                | InputCommand::JumpTop
                | InputCommand::JumpBottom
                | InputCommand::SelectIndex(_) => {
                    handle_input(model, cmd);
                    if model.active_tab == ActiveTab::Timeline {
                        mark_selected_event_read(model, state, clock);
                    }
                    LoopControl::Redraw
                }
                InputCommand::None => LoopControl::Continue,
            }
        }
        Some(Ok(Event::Mouse(mouse))) => {
            let cmd = parse_mouse_input(mouse, terminal_area, model);
            match cmd {
                InputCommand::ScrollUp
                | InputCommand::ScrollDown
                | InputCommand::SelectIndex(_) => {
                    handle_input(model, cmd);
                    if model.active_tab == ActiveTab::Timeline {
                        mark_selected_event_read(model, state, clock);
                    }
                    LoopControl::Redraw
                }
                _ => LoopControl::Continue,
            }
        }
        Some(Ok(Event::Resize(_, _))) => LoopControl::Redraw,
        Some(Ok(_)) => LoopControl::Continue,
        Some(Err(err)) => {
            tracing::warn!(error = %err, "input stream failed");

            model.failure_count += 1;
            model.status_line = format!("input stream failed: {err}");
            LoopControl::Redraw
        }
        None => LoopControl::Quit,
    }
}

fn mark_selected_event_read<S, K>(model: &mut TuiModel, state: &S, clock: &K)
where
    S: StateStorePort,
    K: ClockPort,
{
    let Some(event_key) = model
        .timeline
        .get(model.selected)
        .map(|event| event.event_key())
    else {
        return;
    };

    if model.is_event_read(&event_key) {
        return;
    }

    if let Err(err) = state.mark_timeline_event_read(&event_key, clock.now()) {
        tracing::warn!(error = %err, event_key = %event_key, "failed to persist read state");
        model.status_line = format!("read mark failed: {err}");
        return;
    }

    model.mark_event_read(&event_key);
}

fn enabled_repository_names(config: &Config) -> Vec<String> {
    config
        .repositories
        .iter()
        .filter(|repo| repo.enabled)
        .map(|repo| repo.name.clone())
        .collect()
}

fn apply_poll_result<K>(result: Result<PollOutcome>, model: &mut TuiModel, clock: &K)
where
    K: ClockPort,
{
    match result {
        Ok(outcome) => {
            let new_count = outcome.timeline_events.len();
            if new_count > 0 {
                model.push_timeline(outcome.timeline_events);
            }
            model.status_line = format!("ok (new={new_count})");
            model.last_success_at = Some(clock.now());
        }
        Err(err) => {
            model.failure_count += 1;
            model.status_line = format!("poll failed: {err}");
        }
    }
}

pub async fn run_watch<C, S, N, K>(
    config: &Config,
    gh: &C,
    state: &S,
    notifier: &N,
    clock: &K,
) -> Result<()>
where
    C: GhClientPort,
    S: StateStorePort,
    N: NotifierPort,
    K: ClockPort,
{
    let mut ui = TerminalUi::new()?;
    let mut model = TuiModel::new(config.timeline_limit);
    model.watched_repositories = enabled_repository_names(config);
    let timeline = state.load_timeline_events(config.timeline_limit)?;
    let timeline_keys = timeline
        .iter()
        .map(|event| event.event_key())
        .collect::<Vec<_>>();
    let read_event_keys = state.load_read_event_keys(&timeline_keys)?;
    model.replace_timeline(timeline);
    model.replace_read_event_keys(read_event_keys);
    model.status_line = "ready".to_string();
    model.next_poll_at = Some(clock.now());
    ui.draw(&mut model)?;

    let mut interval = tokio::time::interval(Duration::from_secs(config.interval_seconds));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut spinner_interval =
        tokio::time::interval(Duration::from_millis(SPINNER_REDRAW_INTERVAL_MS));
    spinner_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    spinner_interval.tick().await;
    let mut reader = crossterm::event::EventStream::new();
    let mut poll_state = PollExecutionState::default();
    let mut in_flight_poll: Option<PollFuture<'_>> = None;
    poll_state.request_poll();

    loop {
        if poll_state.start_poll() {
            model.is_polling = poll_state.in_flight();
            model.poll_started_at = Some(clock.now());
            model.queued_refresh = poll_state.queued_refresh();
            model.status_line = "polling".to_string();
            ui.draw(&mut model)?;
            in_flight_poll = Some(Box::pin(poll_once(config, gh, state, notifier, clock)));
        }

        tokio::select! {
            _ = interval.tick() => {
                poll_state.request_poll();
                model.queued_refresh = poll_state.queued_refresh();
                if model.is_polling {
                    ui.draw(&mut model)?;
                }
            }
            _ = spinner_interval.tick(), if model.is_polling => {
                ui.draw(&mut model)?;
            }
            poll_result = async {
                match in_flight_poll.as_mut() {
                    Some(fut) => Some(fut.await),
                    None => None,
                }
            }, if in_flight_poll.is_some() => {
                let result = poll_result.expect("poll future must exist when branch is active");
                in_flight_poll = None;

                apply_poll_result(result, &mut model, clock);
                let queued_for_immediate_next = poll_state.finish_poll_and_take_next_request();

                model.is_polling = poll_state.in_flight();
                model.poll_started_at = None;
                model.queued_refresh = poll_state.queued_refresh();
                model.next_poll_at =
                    Some(clock.now() + chrono::Duration::seconds(config.interval_seconds as i64));

                if queued_for_immediate_next {
                    model.status_line = format!("{} | queued refresh", model.status_line);
                }

                ui.draw(&mut model)?;
            }
            maybe_event = reader.next() => {
                let terminal_area = ui.terminal_area().unwrap_or_default();
                match handle_stream_event(
                    maybe_event,
                    &mut model,
                    state,
                    clock,
                    terminal_area,
                    &open_url_in_browser,
                ) {
                    LoopControl::Quit => break,
                    LoopControl::RequestPoll => {
                        poll_state.request_poll();
                        model.queued_refresh = poll_state.queued_refresh();
                        if model.is_polling {
                            ui.draw(&mut model)?;
                        }
                    }
                    LoopControl::Redraw => {
                        ui.draw(&mut model)?;
                    }
                    LoopControl::Continue => {}
                }
            }
        }
    }

    Ok(())
}

fn open_url_in_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("open");
        cmd.arg(url);
        let result = run_open_command(&mut cmd);
        if result.success {
            return Ok(());
        }
        if !result.stderr.is_empty() {
            tracing::debug!(url = %url, stderr = %result.stderr, "open command failed");
        }

        return Err(anyhow!("failed to open URL with open: {url}"));
    }

    #[cfg(target_os = "linux")]
    {
        let browser_env = std::env::var("BROWSER").ok();
        return open_url_on_linux(
            url,
            detect_wsl(),
            browser_env.as_deref(),
            run_linux_open_backend,
        );
    }

    #[cfg(target_os = "windows")]
    {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "start", ""]).arg(url);
        let result = run_open_command(&mut cmd);
        if result.success {
            return Ok(());
        }
        if !result.stderr.is_empty() {
            tracing::debug!(url = %url, stderr = %result.stderr, "start command failed");
        }

        return Err(anyhow!("failed to open URL with start: {url}"));
    }

    #[allow(unreachable_code)]
    Err(anyhow!("unsupported OS for opening URLs"))
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinuxOpenBackend {
    BrowserEnv,
    XdgOpen,
}

#[cfg(target_os = "linux")]
fn open_url_on_linux<F>(
    url: &str,
    is_wsl: bool,
    browser_env: Option<&str>,
    mut runner: F,
) -> Result<()>
where
    F: FnMut(LinuxOpenBackend, &str, Option<&str>) -> bool,
{
    let browser_env = browser_env.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    });

    if is_wsl {
        if let Some(browser_env) = browser_env {
            if runner(LinuxOpenBackend::BrowserEnv, url, Some(browser_env)) {
                return Ok(());
            }
            if runner(LinuxOpenBackend::XdgOpen, url, None) {
                return Ok(());
            }
            return Err(anyhow!(
                "failed to open URL in WSL with $BROWSER and xdg-open: {url}"
            ));
        }

        if runner(LinuxOpenBackend::XdgOpen, url, None) {
            return Ok(());
        }
        return Err(anyhow!("failed to open URL in WSL with xdg-open: {url}"));
    }

    if runner(LinuxOpenBackend::XdgOpen, url, None) {
        return Ok(());
    }

    Err(anyhow!("failed to open URL with xdg-open: {url}"))
}

#[cfg(target_os = "linux")]
fn run_linux_open_backend(backend: LinuxOpenBackend, url: &str, browser_env: Option<&str>) -> bool {
    match backend {
        LinuxOpenBackend::BrowserEnv => browser_env
            .and_then(|raw| browser_command_from_env(raw, url))
            .map(|(bin, args)| {
                let mut cmd = Command::new(&bin);
                cmd.args(args);
                let result = run_open_command(&mut cmd);
                if !result.success && !result.stderr.is_empty() {
                    tracing::debug!(
                        command = %bin,
                        stderr = %result.stderr,
                        "linux browser open command failed"
                    );
                }
                result.success
            })
            .unwrap_or(false),
        LinuxOpenBackend::XdgOpen => {
            let mut cmd = Command::new("xdg-open");
            cmd.arg(url);
            let result = run_open_command(&mut cmd);
            if !result.success && !result.stderr.is_empty() {
                tracing::debug!(
                    stderr = %result.stderr,
                    "linux xdg-open command failed"
                );
            }
            result.success
        }
    }
}

#[cfg(target_os = "linux")]
fn browser_command_from_env(raw: &str, url: &str) -> Option<(String, Vec<String>)> {
    let mut tokens = split_shell_words(raw)?;
    let has_placeholder = tokens.iter().any(|token| token.contains("%s"));

    for token in &mut tokens {
        if token.contains("%s") {
            *token = token.replace("%s", url);
        }
    }

    if !has_placeholder {
        tokens.push(url.to_string());
    }

    let bin = tokens.remove(0);
    Some((bin, tokens))
}

#[cfg(target_os = "linux")]
fn split_shell_words(raw: &str) -> Option<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut active_quote: Option<char> = None;
    let mut escaped = false;

    for ch in raw.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        if let Some(quote) = active_quote {
            if ch == quote {
                active_quote = None;
            } else if quote == '"' && ch == '\\' {
                escaped = true;
            } else {
                current.push(ch);
            }
            continue;
        }

        match ch {
            '\'' | '"' => active_quote = Some(ch),
            '\\' => escaped = true,
            c if c.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }

    if escaped || active_quote.is_some() {
        return None;
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    if tokens.is_empty() {
        return None;
    }

    Some(tokens)
}

#[cfg(target_os = "linux")]
fn detect_wsl() -> bool {
    let distro_name = std::env::var("WSL_DISTRO_NAME").ok();
    let interop = std::env::var("WSL_INTEROP").ok();
    let proc_hint = read_proc_wsl_hint();
    is_wsl_from_inputs(
        distro_name.as_deref(),
        interop.as_deref(),
        proc_hint.as_deref(),
    )
}

#[cfg(target_os = "linux")]
fn is_wsl_from_inputs(
    wsl_distro_name: Option<&str>,
    wsl_interop: Option<&str>,
    proc_hint: Option<&str>,
) -> bool {
    if wsl_distro_name.is_some_and(|value| !value.trim().is_empty()) {
        return true;
    }
    if wsl_interop.is_some_and(|value| !value.trim().is_empty()) {
        return true;
    }
    proc_hint
        .map(|value| value.to_ascii_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn read_proc_wsl_hint() -> Option<String> {
    let version = std::fs::read_to_string("/proc/version").ok();
    let osrelease = std::fs::read_to_string("/proc/sys/kernel/osrelease").ok();
    match (version, osrelease) {
        (Some(version), Some(osrelease)) => Some(format!("{version}\n{osrelease}")),
        (Some(version), None) => Some(version),
        (None, Some(osrelease)) => Some(osrelease),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        sync::{Arc, Mutex},
    };

    use anyhow::{anyhow, Result};
    use chrono::{TimeZone, Utc};
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Rect;

    use super::{
        apply_poll_result, enabled_repository_names, handle_stream_event, LoopControl,
        PollExecutionState, PollOutcome,
    };
    use crate::{
        config::{Config, FiltersConfig, NotificationConfig, PollConfig, RepositoryConfig},
        domain::events::{EventKind, WatchEvent},
        ports::{ClockPort, StateStorePort},
        ui::tui::{ActiveTab, TuiModel},
    };

    #[derive(Clone, Default)]
    struct FakeState {
        marked_read_event_keys: Arc<Mutex<Vec<String>>>,
        fail_mark_read: Arc<Mutex<bool>>,
    }

    impl FakeState {
        fn marked_read_event_keys(&self) -> Vec<String> {
            self.marked_read_event_keys.lock().unwrap().clone()
        }

        fn set_mark_read_error(&self, should_fail: bool) {
            *self.fail_mark_read.lock().unwrap() = should_fail;
        }
    }

    impl StateStorePort for FakeState {
        fn get_cursor(&self, _repo: &str) -> Result<Option<chrono::DateTime<Utc>>> {
            Ok(None)
        }

        fn set_cursor(&self, _repo: &str, _at: chrono::DateTime<Utc>) -> Result<()> {
            Ok(())
        }

        fn load_timeline_events(&self, _limit: usize) -> Result<Vec<WatchEvent>> {
            Ok(Vec::new())
        }

        fn mark_timeline_event_read(
            &self,
            event_key: &str,
            _read_at: chrono::DateTime<Utc>,
        ) -> Result<()> {
            if *self.fail_mark_read.lock().unwrap() {
                return Err(anyhow!("state store down"));
            }
            self.marked_read_event_keys
                .lock()
                .unwrap()
                .push(event_key.to_string());
            Ok(())
        }

        fn load_read_event_keys(&self, event_keys: &[String]) -> Result<HashSet<String>> {
            let existing = self.marked_read_event_keys.lock().unwrap().clone();
            let existing = existing.into_iter().collect::<HashSet<_>>();
            Ok(event_keys
                .iter()
                .filter(|key| existing.contains(*key))
                .cloned()
                .collect())
        }

        fn cleanup_old(&self, _retention_days: u32, _now: chrono::DateTime<Utc>) -> Result<()> {
            Ok(())
        }

        fn persist_repo_batch(
            &self,
            _batch: &crate::ports::RepoPersistBatch,
        ) -> Result<crate::ports::PersistBatchResult> {
            Ok(crate::ports::PersistBatchResult::default())
        }
    }

    struct FixedClock {
        now: chrono::DateTime<Utc>,
    }

    impl ClockPort for FixedClock {
        fn now(&self) -> chrono::DateTime<Utc> {
            self.now
        }
    }

    fn test_area() -> Rect {
        Rect::new(0, 0, 120, 40)
    }

    fn open_ok(_url: &str) -> Result<()> {
        Ok(())
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
    fn stream_error_sets_status_and_redraws() {
        let state = FakeState::default();
        let clock = FixedClock {
            now: Utc.with_ymd_and_hms(2025, 1, 7, 0, 0, 0).unwrap(),
        };
        let mut model = TuiModel::new(10);

        let control = handle_stream_event(
            Some(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "event stream disconnected",
            ))),
            &mut model,
            &state,
            &clock,
            test_area(),
            &open_ok,
        );

        assert_eq!(control, LoopControl::Redraw);
        assert_eq!(model.failure_count, 1);
        assert!(model.status_line.contains("input stream failed"));
        assert!(model.status_line.contains("event stream disconnected"));
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
    fn enter_opens_selected_url_and_requests_redraw() {
        let state = FakeState::default();
        let clock = FixedClock {
            now: Utc.with_ymd_and_hms(2025, 1, 9, 0, 0, 0).unwrap(),
        };
        let mut model = TuiModel::new(10);
        model.timeline = vec![timeline_event("ev1", clock.now)];
        model.selected = 0;

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let control = handle_stream_event(
            Some(Ok(Event::Key(key))),
            &mut model,
            &state,
            &clock,
            test_area(),
            &open_ok,
        );

        assert_eq!(control, LoopControl::Redraw);
        assert!(model
            .status_line
            .contains("opened: https://example.com/ev1"));
    }

    #[test]
    fn esc_first_press_arms_quit_and_requests_redraw() {
        let state = FakeState::default();
        let clock = FixedClock {
            now: Utc.with_ymd_and_hms(2025, 1, 9, 0, 0, 0).unwrap(),
        };
        let mut model = TuiModel::new(10);

        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let control = handle_stream_event(
            Some(Ok(Event::Key(key))),
            &mut model,
            &state,
            &clock,
            test_area(),
            &open_ok,
        );

        assert_eq!(control, LoopControl::Redraw);
        assert_eq!(model.status_line, "press Esc again to quit (1.5s)");
        assert_eq!(
            model.esc_armed_until,
            Some(clock.now + chrono::Duration::milliseconds(1500))
        );
    }

    #[test]
    fn esc_second_press_within_window_quits() {
        let state = FakeState::default();
        let first_clock = FixedClock {
            now: Utc.with_ymd_and_hms(2025, 1, 9, 0, 0, 0).unwrap(),
        };
        let second_clock = FixedClock {
            now: Utc.with_ymd_and_hms(2025, 1, 9, 0, 0, 1).unwrap(),
        };
        let mut model = TuiModel::new(10);

        let first_press = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(
            handle_stream_event(
                Some(Ok(Event::Key(first_press))),
                &mut model,
                &state,
                &first_clock,
                test_area(),
                &open_ok
            ),
            LoopControl::Redraw
        );

        let second_press = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let control = handle_stream_event(
            Some(Ok(Event::Key(second_press))),
            &mut model,
            &state,
            &second_clock,
            test_area(),
            &open_ok,
        );
        assert_eq!(control, LoopControl::Quit);
    }

    #[test]
    fn esc_second_press_after_window_rearms_without_quit() {
        let state = FakeState::default();
        let first_clock = FixedClock {
            now: Utc.with_ymd_and_hms(2025, 1, 9, 0, 0, 0).unwrap(),
        };
        let second_clock = FixedClock {
            now: Utc.with_ymd_and_hms(2025, 1, 9, 0, 0, 2).unwrap(),
        };
        let mut model = TuiModel::new(10);

        let first_press = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(
            handle_stream_event(
                Some(Ok(Event::Key(first_press))),
                &mut model,
                &state,
                &first_clock,
                test_area(),
                &open_ok
            ),
            LoopControl::Redraw
        );

        let second_press = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let control = handle_stream_event(
            Some(Ok(Event::Key(second_press))),
            &mut model,
            &state,
            &second_clock,
            test_area(),
            &open_ok,
        );
        assert_eq!(control, LoopControl::Redraw);
        assert_eq!(model.status_line, "press Esc again to quit (1.5s)");
        assert_eq!(
            model.esc_armed_until,
            Some(second_clock.now + chrono::Duration::milliseconds(1500))
        );
    }

    #[test]
    fn non_escape_key_disarms_pending_esc_quit() {
        let state = FakeState::default();
        let clock = FixedClock {
            now: Utc.with_ymd_and_hms(2025, 1, 9, 0, 0, 1).unwrap(),
        };
        let mut model = TuiModel::new(10);
        model.esc_armed_until = Some(clock.now + chrono::Duration::milliseconds(500));

        let key = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
        let control = handle_stream_event(
            Some(Ok(Event::Key(key))),
            &mut model,
            &state,
            &clock,
            test_area(),
            &open_ok,
        );

        assert_eq!(control, LoopControl::Redraw);
        assert!(model.esc_armed_until.is_none());
    }

    #[test]
    fn navigation_marks_selected_event_as_read() {
        let state = FakeState::default();
        let clock = FixedClock {
            now: Utc.with_ymd_and_hms(2025, 1, 9, 0, 30, 0).unwrap(),
        };
        let mut model = TuiModel::new(10);
        model.timeline = vec![
            timeline_event("ev-nav-1", clock.now),
            timeline_event("ev-nav-2", clock.now),
        ];
        model.selected = 0;

        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let control = handle_stream_event(
            Some(Ok(Event::Key(key))),
            &mut model,
            &state,
            &clock,
            test_area(),
            &open_ok,
        );

        assert_eq!(control, LoopControl::Redraw);
        assert_eq!(model.selected, 1);
        assert_eq!(
            state.marked_read_event_keys(),
            vec![model.timeline[1].event_key()]
        );
    }

    #[test]
    fn navigation_does_not_mark_events_read_on_repositories_tab() {
        let state = FakeState::default();
        let clock = FixedClock {
            now: Utc.with_ymd_and_hms(2025, 1, 9, 0, 30, 0).unwrap(),
        };
        let mut model = TuiModel::new(10);
        model.timeline = vec![
            timeline_event("ev-nav-1", clock.now),
            timeline_event("ev-nav-2", clock.now),
        ];
        model.selected = 0;
        model.active_tab = ActiveTab::Repositories;

        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let control = handle_stream_event(
            Some(Ok(Event::Key(key))),
            &mut model,
            &state,
            &clock,
            test_area(),
            &open_ok,
        );

        assert_eq!(control, LoopControl::Redraw);
        assert_eq!(model.selected, 0);
        assert!(state.marked_read_event_keys().is_empty());
    }

    #[test]
    fn read_mark_failure_keeps_event_unread_and_sets_status() {
        let state = FakeState::default();
        state.set_mark_read_error(true);
        let clock = FixedClock {
            now: Utc.with_ymd_and_hms(2025, 1, 9, 1, 30, 0).unwrap(),
        };
        let mut model = TuiModel::new(10);
        model.timeline = vec![
            timeline_event("ev-read-fail-1", clock.now),
            timeline_event("ev-read-fail-2", clock.now),
        ];
        model.selected = 0;

        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let control = handle_stream_event(
            Some(Ok(Event::Key(key))),
            &mut model,
            &state,
            &clock,
            test_area(),
            &open_ok,
        );

        assert_eq!(control, LoopControl::Redraw);
        assert_eq!(model.selected, 1);
        assert!(state.marked_read_event_keys().is_empty());
        assert!(model
            .status_line
            .contains("read mark failed: state store down"));
        assert!(!model.is_event_read(&model.timeline[1].event_key()));
    }

    #[test]
    fn refresh_requested_while_polling_is_queued_without_parallel_start() {
        let mut state = PollExecutionState::default();

        assert!(state.request_poll());
        assert!(!state.request_poll());

        assert!(state.start_poll());
        assert!(state.in_flight());

        assert!(!state.request_poll());
        assert!(state.queued_refresh());

        let queued_for_next = state.finish_poll_and_take_next_request();
        assert!(queued_for_next);
        assert!(!state.in_flight());
        assert!(!state.queued_refresh());

        assert!(state.start_poll());
        assert!(state.in_flight());
    }

    #[cfg(unix)]
    #[test]
    fn open_command_capture_collects_stderr_for_failed_process() {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "printf 'launcher missing\\n' >&2; exit 1"]);

        let result = super::run_open_command(&mut cmd);
        assert!(!result.success);
        assert_eq!(result.stderr, "launcher missing");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_open_wsl_prefers_browser_env_first() {
        let url = "https://example.com/wsl";
        let mut calls = Vec::new();

        let result = super::open_url_on_linux(
            url,
            true,
            Some("firefox --new-window"),
            |backend, _url, _| {
                calls.push(backend);
                true
            },
        );

        assert!(result.is_ok());
        assert_eq!(calls, vec![super::LinuxOpenBackend::BrowserEnv]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_open_wsl_falls_back_to_xdg_open_when_browser_env_fails() {
        let url = "https://example.com/wsl-fallback";
        let mut calls = Vec::new();

        let result = super::open_url_on_linux(url, true, Some("firefox"), |backend, _url, _| {
            calls.push(backend);
            backend == super::LinuxOpenBackend::XdgOpen
        });

        assert!(result.is_ok());
        assert_eq!(
            calls,
            vec![
                super::LinuxOpenBackend::BrowserEnv,
                super::LinuxOpenBackend::XdgOpen
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_open_wsl_uses_xdg_open_only_when_browser_env_absent() {
        let url = "https://example.com/wsl-no-browser-env";
        let mut calls = Vec::new();

        let result = super::open_url_on_linux(url, true, Some("   "), |backend, _url, _| {
            calls.push(backend);
            true
        });

        assert!(result.is_ok());
        assert_eq!(calls, vec![super::LinuxOpenBackend::XdgOpen]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_open_wsl_failure_after_browser_and_xdg_open_returns_expected_error() {
        let url = "https://example.com/wsl-fail";
        let mut calls = Vec::new();

        let err = super::open_url_on_linux(url, true, Some("firefox"), |backend, _url, _| {
            calls.push(backend);
            false
        })
        .expect_err("wsl browser and xdg-open failures should bubble up");

        assert_eq!(
            calls,
            vec![
                super::LinuxOpenBackend::BrowserEnv,
                super::LinuxOpenBackend::XdgOpen
            ]
        );
        assert_eq!(
            err.to_string(),
            format!("failed to open URL in WSL with $BROWSER and xdg-open: {url}")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_open_uses_xdg_open_when_not_wsl() {
        let url = "https://example.com/linux";
        let mut calls = Vec::new();

        let result = super::open_url_on_linux(url, false, Some("firefox"), |backend, _url, _| {
            calls.push(backend);
            true
        });

        assert!(result.is_ok());
        assert_eq!(calls, vec![super::LinuxOpenBackend::XdgOpen]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn browser_command_from_env_appends_url_without_placeholder() {
        let url = "https://example.com/arg";
        let (bin, args) = super::browser_command_from_env("firefox --new-window", url)
            .expect("browser command should parse");

        assert_eq!(bin, "firefox");
        assert_eq!(
            args,
            vec![
                "--new-window".to_string(),
                "https://example.com/arg".to_string()
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn browser_command_from_env_replaces_percent_s_placeholder() {
        let url = "https://example.com/placeholder";
        let (bin, args) = super::browser_command_from_env("w3m %s --title=%s", url)
            .expect("browser command should parse");

        assert_eq!(bin, "w3m");
        assert_eq!(
            args,
            vec![
                "https://example.com/placeholder".to_string(),
                "--title=https://example.com/placeholder".to_string()
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn browser_command_from_env_supports_quoted_command_path() {
        let url = "https://example.com/quoted";
        let (bin, args) = super::browser_command_from_env(
            "\"/mnt/c/Program Files/Browser/browser.exe\" --profile default",
            url,
        )
        .expect("browser command should parse");

        assert_eq!(bin, "/mnt/c/Program Files/Browser/browser.exe");
        assert_eq!(
            args,
            vec![
                "--profile".to_string(),
                "default".to_string(),
                "https://example.com/quoted".to_string()
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn browser_command_from_env_returns_none_for_unclosed_quote() {
        let parsed = super::browser_command_from_env("\"/usr/bin/firefox", "https://example.com");
        assert!(parsed.is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn wsl_detection_inputs_follow_expected_precedence() {
        assert!(super::is_wsl_from_inputs(
            Some("Ubuntu"),
            None,
            Some("Linux version 6.6.87.2-generic")
        ));
        assert!(super::is_wsl_from_inputs(
            None,
            Some("/run/WSL/123_interop"),
            Some("Linux version 6.6.87.2-generic")
        ));
        assert!(super::is_wsl_from_inputs(
            None,
            None,
            Some("Linux version 6.6.87.2-microsoft-standard-WSL2")
        ));
        assert!(!super::is_wsl_from_inputs(
            None,
            None,
            Some("Linux version 6.6.87.2-generic")
        ));
        assert!(!super::is_wsl_from_inputs(None, None, None));
    }
}
