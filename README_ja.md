# gh-watch

デスクトップ通知と TUI タイムラインを備えた、ローカル実行型の GitHub 監視ツールです。

`gh-watch` はローカルで常駐し、対象リポジトリを定期ポーリングしてイベントを重複なく通知します。

English README: [README.md](README.md)

## 5分でできること

1. 1つの設定で複数リポジトリを監視
2. PR / Issue / コメント / レビュー / マージを通知
3. SQLite の event key で再起動後も重複通知を防止
4. 新規に観測したイベントごとに通知は最大1回のみ

## デモ

- 次のリリースでデモ素材を追加予定:
- 30-45秒の操作GIF
- タイムライン画面キャプチャ
- タブ切り替え画面キャプチャ

## 前提条件

- Rust 1.93+
- `gh` CLI
- `gh` 認証済み

```bash
gh auth login -h github.com
```

## インストール

### Cargo（現行）

```bash
cargo install --git https://github.com/6uclz1/gh-watch gh-watch
```

### GitHub Releases

タグ付きリリース時に macOS / Linux / Windows 向けバイナリを配布します。

## クイックスタート

1. 設定作成

```bash
gh-watch init
```

2. 設定を開く

```bash
gh-watch config open
```

3. `[[repositories]]` を編集

4. 事前チェック

```bash
gh-watch check
```

5. 常駐監視 + TUI 起動

```bash
gh-watch watch
```

## 主なコマンド

- `gh-watch watch [--config <path>] [--interval-seconds <n>]`
- `gh-watch once [--config <path>] [--dry-run] [--json]`
- `gh-watch check [--config <path>]`
- `gh-watch init [--path <path>] [--force] [--reset-state]`
- `gh-watch config open`
- `gh-watch config path`

### `once` の終了コード

- `0`: 成功
- `1`: 失敗

## 監視イベント

- `pr_created`
- `issue_created`
- `issue_comment_created`
- `pr_review_comment_created`
- `pr_review_requested`
- `pr_review_submitted`
- `pr_merged`

## フィルタ

グローバルフィルタ:

- `[filters].event_kinds`
- `[filters].ignore_actors`
- `[filters].only_involving_me`

`only_involving_me = true` のとき、次を満たすイベントのみ通知:

- 自分宛てのレビュー依頼
- コメント/レビュー本文で自分がメンションされている
- 自分が作成した PR / Issue への更新

## Timeline優先の通知セマンティクス

- 初回はカーソル初期化のみ（通知なし）
- ポーリング境界取りこぼし対策として、固定5分オーバーラップ（`since = last_cursor - 300秒`）を利用
- リポジトリごとのカーソルは poll 開始時刻で更新（処理後の `now` ではない）
- 新規イベントは先に永続化し、同一 poll 内で即時通知
- `event_key` で重複取得を吸収し、既に記録済みのイベントを再通知しない
- 取得/通知のエラーは fail-fast でそのサイクルを即失敗にする

## TUI キーバインド

- `q`: 終了
- `Esc` を1.5秒以内に2回: 終了
- `r`: 手動更新
- `Tab` / `Shift+Tab`: `Timeline` / `Repositories` タブ切替
- `?`: ヘルプ表示切替
- `Enter`: 選択URLを開く（WSLでは `powershell.exe` 経由でWindows既定ブラウザを開く）
- `↑` / `↓` or `j` / `k`: 1件移動（Timelineタブ）
- `PageUp` / `PageDown`: 1ページ移動（Timelineタブ）
- `g` / `Home`: 先頭（Timelineタブ）
- `G` / `End`: 末尾（Timelineタブ）
- マウスクリック/ホイール: 選択/スクロール（Timelineタブ）
- タイムライン未読マーカー: `*` は未読、空白は既読
- 既読化タイミング: 選択移動時または `Enter` でURLを開いたとき（再起動後も保持）

## 設定ファイル解決順

1. `--config <path>`
2. `GH_WATCH_CONFIG`
3. `./config.toml`
4. バイナリ配置ディレクトリの `config.toml`

既定の state DB:

- macOS/Linux: `~/.local/share/gh-watch/state.db`
- Windows: `%LOCALAPPDATA%\gh-watch\state.db`

共有用テンプレートは `config.example.toml` を利用してください。

旧バージョンで作成した `state.db` を使っている場合は `gh-watch init --reset-state` を実行してください。

通知設定キー:

- `[notifications].enabled`
- `[notifications].include_url`

## 通知バックエンド

- macOS: `osascript`（`display notification`）で通知
- WSL: `powershell.exe` + WinForms `NotifyIcon.ShowBalloonTip(10000)` で通知
- WSLのバルーン通知クリックで対象イベントのURLを開く
- それ以外の環境: 通知は Noop（起動時に warning を表示）
- 最終的なバナー表示有無は OS 側の通知設定やフォーカスモードに依存

## 開発時の品質ゲート

- `cargo fmt --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test`
