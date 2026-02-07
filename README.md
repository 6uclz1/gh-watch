# gh-watch

GitHub の PR / Issue / コメントを定期監視し、重複なしでデスクトップ通知する常駐 CLI + TUI ツールです。

## Prerequisites

- Rust 1.93+
- `gh` CLI
- GitHub 認証済みの `gh` (`gh auth login -h github.com`)

## Installation

```bash
cargo install --git https://github.com/6uclz1/gh-watch gh-watch
```

## Quick Start

1. 設定ファイルを生成

```bash
gh-watch init
```

2. 設定ファイルをエディタで開く（`edit` は `open` の別名）

```bash
gh-watch config open
# または
gh-watch config edit
```

3. `config.toml` の `[[repositories]]` を自分の監視対象に編集

4. 事前チェック

```bash
gh-watch check
```

5. 常駐監視 + TUI 起動

```bash
gh-watch watch
```

設定ファイルの既定位置:
- 実行中 `gh-watch` バイナリと同じディレクトリの `config.toml`

確認コマンド:

```bash
gh-watch config path
```

`watch` / `check` で `--config` を指定した場合のみ、明示パスを優先します。

## Key Bindings

- `q`: 終了
- `r`: 手動更新
- `↑` / `↓`: タイムライン移動
- `Enter`: 選択中タイムライン項目の URL を既定ブラウザで開く
- マウス:
  - 左クリック（タイムライン左ペイン内）: 項目選択
  - ホイール上下（タイムライン左ペイン内）: 項目移動
- 右ペイン（`Watching Repositories`）は閲覧専用

## Behavior

- 初回起動時は通知せず、カーソルのみ初期化
- 通知済みイベントは SQLite で管理し、同一イベントの再通知を防止
- 通知失敗時はカーソルを安全点まで巻き戻し、未通知イベントを次回ポーリングで再試行（at-least-once）
- 監視/通知失敗は SQLite に永続化し、TUI の `latest_failure` で直近失敗を表示
- 失敗履歴は `retention_days` と `failure_history_limit`（既定: 200件）で自動整理
- 既定 state DB パス:
  - macOS/Linux: `~/.local/share/gh-watch/state.db`
  - Windows: `%LOCALAPPDATA%\\gh-watch\\state.db`

## TUI Layout

- Header: `status` / `last_success` / `next_poll` / `failures` / `latest_failure`
- Main(左 70%): `Timeline`（新着順、`↑`/`↓`で選択）
- Main(右 30%): `Watching Repositories`（`enabled=true` の repository 一覧、config 記載順）
- Footer: キーガイド + 選択イベント URL

## Repository Notes

- `config.toml` はローカル環境用のため Git 管理対象外（`.gitignore` で除外）
- 共有用のテンプレートは `config.example.toml`
