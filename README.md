# gh-watch

Reliable GitHub watcher with desktop notifications and a terminal timeline UI.

`gh-watch` runs locally, polls your repositories, deduplicates events, and helps you track important updates without opening GitHub all day.

日本語README: [README_ja.md](README_ja.md)

## What You Get In 5 Minutes

1. Watch multiple repositories from one config.
2. Get notifications for PRs/issues/comments and review/merge events.
3. Avoid duplicate alerts across restarts with SQLite-backed event keys.
4. Keep resilience: failed notifications are retried (at-least-once behavior).

## Demo

- Demo assets are prepared for the next release cut:
- 30-45 sec GIF walkthrough
- Timeline screenshot
- Filter/search screenshot

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
- `gh-watch report [--config <path>] [--since <duration>] [--format markdown|json]`
- `gh-watch doctor [--config <path>]`
- `gh-watch check [--config <path>]`
- `gh-watch init [--path <path>] [--force] [--interactive] [--reset-state]`
- `gh-watch config open|edit`
- `gh-watch config path`
- `gh-watch config doctor`

### `once` Exit Codes

- `0`: success
- `2`: partial failure (one or more repositories failed)
- `1`: fatal failure

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

## At-Least-Once Notification Semantics

- First run bootstraps cursor and does not notify.
- Notifications are deduplicated by `event_key`.
- If notification delivery fails, cursor is rolled back to retry missed events.
- Repository-level failures do not block other repositories.

## TUI Key Bindings

- `q`: quit
- `r`: refresh now
- `/`: start search
- `f`: cycle kind filter
- `Esc`: clear search/filter
- `?`: toggle help
- `Enter`: open selected URL
- `↑` / `↓` or `j` / `k`: move one item
- `PageUp` / `PageDown`: move one page
- `g` / `Home`: top
- `G` / `End`: bottom
- Mouse click/wheel in timeline: select/scroll
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

## Notification Sender IDs

- macOS:
  - `notifications.macos_bundle_id` を指定可能（未指定時の既定値: `com.apple.Terminal`）
  - 未指定時は `check` / `watch` 起動時に警告を表示
- Windows:
  - `notifications.windows_app_id` を指定可能
  - 未指定時の既定値は PowerShell の AppUserModelID（`Toast::POWERSHELL_APP_ID`）
  - 未指定時は `check` / `watch` 起動時に警告を表示
- いずれも、最終的なバナー表示有無は OS 側の通知設定に依存

## Developer Quality Gates

CI checks:

- `cargo fmt --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test`
