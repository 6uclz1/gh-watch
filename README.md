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
- Per-repository cursor is updated to poll start time (not post-processing `now`).
- New events are durably persisted first, then notified immediately in the same poll cycle.
- `event_key` deduplicates overlap re-fetches and prevents re-notifying already logged events.
- Polling and notification errors fail the current cycle immediately (fail-fast).

## TUI Key Bindings

- `q`: quit
- `Esc` twice within 1.5 seconds: quit
- `r`: refresh now
- `Tab` / `Shift+Tab`: switch `Timeline` and `Repositories` tabs
- `?`: toggle help
- `Enter`: open selected URL (on WSL, tries `$BROWSER` first, then falls back to `xdg-open`)
- `↑` / `↓` or `j` / `k`: move one item (Timeline tab)
- `PageUp` / `PageDown`: move one page (Timeline tab)
- `g` / `Home`: top (Timeline tab)
- `G` / `End`: bottom (Timeline tab)
- Mouse click/wheel in timeline: select/scroll (Timeline tab)
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

## Notification Backend

- macOS: notifications are sent via `osascript` (`display notification`).
- WSL: notifications are sent via `powershell.exe` + WinForms `NotifyIcon.ShowBalloonTip(10000)`.
- On WSL, URL click action is not supported; with `include_url = true`, the URL is included in the notification body.
- Other environments: notifier runs in noop mode and prints a startup warning.
- Banner visibility still depends on OS notification settings / focus mode.

## Developer Quality Gates

CI checks:

- `cargo fmt --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test`
