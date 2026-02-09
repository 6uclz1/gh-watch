# gh-watch

デスクトップ通知と TUI タイムラインを備えた、ローカル実行型の GitHub 監視ツールです。

`gh-watch` はローカルで常駐し、対象リポジトリを定期ポーリングしてイベントを重複なく通知します。

English README: [README.md](README.md)

## 5分でできること

1. 1つの設定で複数リポジトリを監視
2. PR / Issue / コメント / レビュー / マージを通知
3. SQLite の event key で再起動後も重複通知を防止
4. 通知失敗時もタイムライン反映を優先し、同一poll内で1回再試行

## デモ

- 次のリリースでデモ素材を追加予定:
- 30-45秒の操作GIF
- タイムライン画面キャプチャ
- フィルタ/検索画面キャプチャ

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
- `gh-watch report [--config <path>] [--since <duration>] [--format markdown|json]`
- `gh-watch doctor [--config <path>]`
- `gh-watch check [--config <path>]`
- `gh-watch init [--path <path>] [--force] [--interactive] [--reset-state]`
- `gh-watch config open|edit`
- `gh-watch config path`
- `gh-watch config doctor`

### `once` の終了コード

- `0`: 成功
- `2`: 一部リポジトリで失敗（部分失敗）
- `1`: 致命的エラー

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
- `event_key` を処理済みチェックポイントとして重複処理を防止
- 通知送信は同一poll内で最大2回試行（初回 + 1回再試行）
- 通知が最終的に失敗しても Timeline には反映し、処理完了扱いにする
- 通知失敗時でもカーソルは巻き戻さず `now` へ進める
- あるリポジトリの失敗が他リポジトリを止めない

## TUI キーバインド

- `q`: 終了
- `r`: 手動更新
- `/`: 検索開始
- `f`: kind フィルタ切替
- `Esc`: 検索/フィルタ解除
- `?`: ヘルプ表示切替
- `Enter`: 選択URLを開く
- `↑` / `↓` or `j` / `k`: 1件移動
- `PageUp` / `PageDown`: 1ページ移動
- `g` / `Home`: 先頭
- `G` / `End`: 末尾
- マウスクリック/ホイール: 選択/スクロール
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

## 通知 Sender ID

- macOS:
  - `notifications.macos_bundle_id` を指定可能（未指定時の既定値: `com.apple.Terminal`）
  - 未指定時は `check` / `watch` 起動時に警告を表示
- Windows:
  - `notifications.windows_app_id` を指定可能
  - 未指定時の既定値は PowerShell の AppUserModelID（`Toast::POWERSHELL_APP_ID`）
  - 未指定時は `check` / `watch` 起動時に警告を表示
- WSL（LinuxバイナリをWSL上で実行する場合）:
  - Linuxデスクトップ通知ではなく `powershell.exe` 経由の Windows Toast を使用
  - `notifications.wsl_windows_app_id` を指定可能（WSL時は `notifications.windows_app_id` を参照しない）
  - 未指定時の既定値は PowerShell の AppUserModelID
  - Toast 送信失敗時は `stderr` を優先し、`stderr` が空の場合は `stdout` を warning に含める
  - `powershell.exe` が利用できない場合:
    - `gh-watch check` は失敗終了
    - `gh-watch doctor` / `gh-watch watch` / `gh-watch once` は warning を表示して継続
- 最終的なバナー表示有無は OS 側の通知設定に依存

## 開発時の品質ゲート

- `cargo fmt --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test`
