# AGENT.md

## このファイルの目的
このリポジトリの現在の実装コンテキストを、後続開発で短時間に把握できるようにまとめる。

## プロジェクト概要
- プロジェクト名: `gh-watch`
- 言語: Rust (edition 2021)
- 目的: GitHubのPR/Issue/コメントを定期監視し、重複なしでデスクトップ通知しつつTUIでタイムライン表示する常駐CLI

## 現在の公開CLI
- `gh-watch watch [--config <path>] [--interval-seconds <n>]`
- `gh-watch check [--config <path>]`
- `gh-watch init [--path <path>] [--force]`
- `gh-watch config open` (`gh-watch config edit` は別名)
- `gh-watch config path`

### `watch` の挙動
- 起動時に `gh auth status` を検証し、失敗時は即終了
- TUIを表示しつつ監視ループを実行
- キー操作: `q` 終了 / `r` 手動更新 / `↑↓` スクロール

### `check` の挙動
- 設定ファイル読み込み
- `gh auth status` 検証
- 通知バックエンド健全性確認
- state DB の初期化確認

### `init` の挙動
- 設定テンプレートを `--path` 指定先、またはバイナリ配置先の `config.toml` に生成
- 既存ファイルがある場合は `--force` なしで失敗

### `config` の挙動
- `config open` / `config edit`
  - バイナリ配置先 `config.toml` を開く
  - 起動優先順: `VISUAL` -> `EDITOR` -> OS既定オープナー
  - config 未作成時は `gh-watch init` を案内して失敗
- `config path`
  - 現在参照する既定 config パスを表示

## 設計方針（現在実装）
- レイヤ分割: `domain` / `app` / `infra` / `ui` / `cli`
- 副作用境界は `ports` トレイトで分離
- `domain` は純粋データ型・判定ロジック中心

### 主なモジュール
- `src/config.rs`: 設定読込・デフォルト適用・バリデーション
- `src/domain/events.rs`: `EventKind`, `WatchEvent`
- `src/domain/decision.rs`: 通知判定/タイムライン整形
- `src/app/poll_once.rs`: 1サイクルの監視ユースケース
- `src/app/watch_loop.rs`: 定期実行 + TUI入力のイベントループ（enabled repo一覧をTUIモデルへ供給）
- `src/infra/gh_client.rs`: `gh api` 実行とJSON正規化
- `src/infra/state_sqlite.rs`: SQLite永続化
- `src/infra/notifier/*`: OS別通知実装
- `src/ui/tui.rs`: TUIモデル・入力変換・描画

## 設定仕様（実装済み）
- `--config` を指定した場合は明示パスを読む
- `--config` 省略時は、実行中バイナリの親ディレクトリにある `config.toml` を読む
- 既定 state DB パス
  - macOS/Linux: `~/.local/share/gh-watch/state.db`
  - Windows: `%LOCALAPPDATA%\\gh-watch\\state.db`
- 設定キー
  - `interval_seconds` (default 300)
  - `timeline_limit` (default 500)
  - `retention_days` (default 90)
  - `state_db_path` (optional)
  - `repositories = [{ name = "owner/repo", enabled = true }]`
  - `[notifications] enabled = true, include_url = true`

## 監視対象イベント（実装済み）
- `PrCreated`
- `IssueCreated` (issue一覧から `pull_request` 付き要素を除外)
- `IssueCommentCreated`
- `PrReviewCommentCreated`

## 監視ロジックの要点
- 初回（repoごとのカーソル未作成時）は bootstrap 扱い
  - `last_polled_at` を現在時刻で保存
  - 通知しない
- 2回目以降は `since=last_polled_at` で差分取得
- 通知済み判定は `event_key` をSQLiteで管理
- 通知は同一poll内で最大2回試行（初回+1回再試行）
- 通知成否に関わらず timeline/notified を記録（Timeline反映を優先）
- 通知が最終的に失敗した場合もイベントは処理済み扱いとし、カーソルは巻き戻さない
- repo単位の取得失敗は他repo処理を止めない

## `gh` 連携仕様（実装済み）
- 認証確認: `gh auth status`
- 取得:
  - PR/Issueは `page` を進めながら手動ページング（`per_page=100`）
  - PR/Issueは created降順の最古要素が `since` 以下になったら打ち切り
  - `repos/{repo}/issues/comments?since=<RFC3339>&per_page=100` を `gh api --paginate --slurp` で取得
  - `repos/{repo}/pulls/comments?since=<RFC3339>&per_page=100` を `gh api --paginate --slurp` で取得
  - ページング無限化対策として endpointごとに最大ページ上限あり（1000）
- テスト向けに `GH_WATCH_GH_BIN` で `gh` バイナリパスを差し替え可能

## SQLiteスキーマ
- `notified_events(event_key PK, repo, kind, source_id, notified_at, event_created_at, url)`
- `polling_cursors(repo PK, last_polled_at)`
- `timeline_events(event_key PK, payload_json, created_at)`

## TUI仕様（実装済み）
- Header: status / last_success / next_poll / failures
- Main(left 70%): タイムライン（新着順、選択可能）
- Main(right 30%): Watching Repositories（enabled=true の監視対象repo、config記載順、閲覧専用）
- Footer: キーガイド + 選択イベントURL

## 通知実装
- macOS: `mac-notification-sys`
- Linux: `notify-rust`
- Windows: `winrt-notification`
- 通知本文: `"<title> by @<actor>"` + URL（`include_url=true`時）

## テスト状況
- テストファイル群
  - `tests/config_test.rs`
  - `tests/config_cmd_test.rs`
  - `tests/domain_decision_test.rs`
  - `tests/state_sqlite_test.rs`
  - `tests/gh_normalization_test.rs`
  - `tests/poll_once_test.rs`
  - `tests/tui_state_test.rs`
  - `tests/notifier_test.rs`
  - `tests/watch_e2e_test.rs`
  - `tests/watch_auth_fail_test.rs`
- 現在の実行結果
  - `cargo test`: pass
  - `cargo clippy --all-targets --all-features -- -D warnings`: pass

## 実行メモ
- まず `gh auth login -h github.com` が必要
- サンプル設定は `config.example.toml`
- ローカル確認手順
  1. `cargo run -- check --config ./config.example.toml`
  2. `cargo run -- watch --config ./config.example.toml`

## 既知の制約 / 次の改善候補
- repo間ポーリングは逐次実行（並列化なし）
- 監視失敗の永続ログ基盤は未実装（TUIステータス中心）
- 通知セマンティクスは Timeline優先（同一poll内で最大2回試行し、失敗時もTimeline反映して処理完了）
- 通知クリックでURLを開く統一UXは未実装（本文にURL表示のみ）
