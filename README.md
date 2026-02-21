# gh-watch

Reliable GitHub watcher with desktop notifications and a terminal timeline UI.

`gh-watch` runs locally, polls your repositories, deduplicates events, and helps you track important updates without opening GitHub all day.

日本語README: [README_ja.md](README_ja.md)

## What You Get In 5 Minutes

1. Watch multiple repositories from one config.
2. Get notifications for PRs/issues/comments and review/merge events.
3. Avoid duplicate alerts across restarts with SQLite-backed event keys.
4. Keep behavior simple: each newly observed event is notified at most once.

## Demo

- Demo assets are prepared for the next release cut:
- 30-45 sec GIF walkthrough
- Timeline screenshot
- Tab switch screenshot

## Prerequisites

- Rust 1.93+
- `gh` CLI
- Authenticated GitHub CLI:

```bash
gh auth login -h github.com
```

## Installation

### Cargo (current)

```bash
cargo install --git https://github.com/6uclz1/gh-watch gh-watch
```

### GitHub Releases

Prebuilt binaries are published for macOS/Linux/Windows on every tag release.

## Quick Start

1. Create config

```bash
gh-watch init
```

2. Open config

```bash
gh-watch config open
```

3. Edit repositories in `[[repositories]]`

4. Validate setup

```bash
gh-watch check
```

5. Start watcher + TUI

```bash
gh-watch watch
```

## Core Commands

- `gh-watch watch [--config <path>] [--interval-seconds <n>]`
- `gh-watch once [--config <path>] [--dry-run] [--json]`
- `gh-watch check [--config <path>]`
- `gh-watch init [--path <path>] [--force] [--reset-state]`
- `gh-watch config open`
- `gh-watch config path`
- `gh-watch commands`
- `gh-watch completion <shell>` (`bash` | `zsh` | `fish` | `pwsh`)

## Shell Completion

Generate completion scripts with:

```bash
gh-watch completion <shell>
```

zsh:

```bash
mkdir -p ~/.zfunc
gh-watch completion zsh > ~/.zfunc/_gh-watch
echo 'fpath=(~/.zfunc $fpath)' >> ~/.zshrc
echo 'autoload -Uz compinit && compinit' >> ~/.zshrc
```

bash:

```bash
gh-watch completion bash > ~/.gh-watch.bash
echo 'source ~/.gh-watch.bash' >> ~/.bashrc
```

fish:

```bash
mkdir -p ~/.config/fish/completions
gh-watch completion fish > ~/.config/fish/completions/gh-watch.fish
```

pwsh:

```powershell
gh-watch completion pwsh > "$HOME/.gh-watch.ps1"
Add-Content -Path $PROFILE -Value '. "$HOME/.gh-watch.ps1"'
```

### `once` Exit Codes

- `0`: success
- `1`: any failure
- In text mode output, `notified` means the number of dispatched desktop notifications (not the number of matched events).

## Events

Default supported event kinds:

- `pr_created`
- `issue_created`
- `issue_comment_created`
- `pr_review_comment_created`
- `pr_review_requested`
- `pr_review_submitted`
- `pr_merged`

## Filters

Global filter keys:

- `[filters].event_kinds`
- `[filters].ignore_actors`
- `[filters].only_involving_me`

`only_involving_me = true` keeps notifications when any of these are true:

- Review request targets you.
- Comment/review body mentions you.
- Update happens on a PR/Issue authored by you.

## Timeline-First Notification Semantics

- First run bootstraps cursor and does not notify.
- Polling uses a fixed 5-minute overlap (`since = last_cursor - 300s`) to reduce boundary misses.
- Repository fetches run sequentially for reliability (parallel fetch is disabled).
- Each repository fetch retries up to 3 attempts (backoff: 1s, then 2s).
- Per-repository cursor is updated to poll start time (not post-processing `now`).
- New events are durably persisted first, then notified immediately in the same poll cycle.
- When a poll has 2+ newly logged events, desktop notification dispatch is collapsed into one digest notification.
- `event_key` deduplicates overlap re-fetches and prevents re-notifying already logged events.
- Repository fetch failures are treated as partial failures: successful repositories still complete.
- If all repositories fail to fetch in a cycle, that cycle fails.

## TUI Key Bindings

- `q`: quit
- `Esc` twice within 1.5 seconds: quit
- `r`: refresh now
- `Tab` / `Shift+Tab`: switch `Timeline`, `My PR`, and `Repositories` tabs
- `?`: toggle help
- `Enter`: open selected URL (on WSL, tries `$BROWSER` first, then falls back to `xdg-open`)
- `↑` / `↓` or `j` / `k`: move one item (Timeline/My PR tabs)
- `PageUp` / `PageDown`: move one page (Timeline/My PR tabs)
- `g` / `Home`: top (Timeline/My PR tabs)
- `G` / `End`: bottom (Timeline/My PR tabs)
- Mouse click/wheel in timeline table: select/scroll (Timeline/My PR tabs)
- Timeline unread marker: `*` means unread, blank means read
- Read timing: selected by navigation or opened with `Enter` (persisted across restarts)

## Configuration Notes

Default config resolution order:

1. `--config <path>`
2. `GH_WATCH_CONFIG`
3. `./config.toml`
4. installed binary directory `config.toml`

Default state DB path:

- macOS/Linux: `~/.local/share/gh-watch/state.db`
- Windows: `%LOCALAPPDATA%\gh-watch\state.db`

Use `config.example.toml` as a shareable template.

If your `state.db` was created by an older release, run `gh-watch init --reset-state`.

Notification config keys:

- `[notifications].enabled`
- `[notifications].include_url`

Polling reliability notes:

- `interval_seconds < 30` is allowed but prints a stability warning at startup.
- Removed/unknown config keys are rejected as parse errors, including `poll.max_concurrency` and `failure_history_limit` (also for `gh-watch init --reset-state`).

## Notification Backend

- macOS: notifications are sent via `osascript` (`display notification`).
- WSL: notifications are sent via `powershell.exe` + BurntToast (`New-BurntToastNotification`).
- On WSL, URL click action is not supported; with `include_url = true`, the URL is included in the notification body.
- Other environments: notifier runs in noop mode and prints a startup warning.
- Banner visibility still depends on OS notification settings / focus mode.

## Developer Quality Gates

CI checks:

- `cargo fmt --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test`
