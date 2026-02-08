# gh-watch

デスクトップ通知と TUI タイムラインを備えた、ローカル実行型の GitHub 監視ツールです。

`gh-watch` はローカルで常駐し、対象リポジトリを定期ポーリングしてイベントを重複なく通知します。

English README: [README.md](README.md)

## 5分でできること

1. 1つの設定で複数リポジトリを監視
2. PR / Issue / コメント / レビュー / マージを通知
3. SQLite の event key で再起動後も重複通知を防止
4. 通知失敗時は再試行（at-least-once）

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

## at-least-once 通知セマンティクス

- 初回はカーソル初期化のみ（通知なし）
- `event_key` で重複通知を防止
- 通知失敗時はカーソル巻き戻しで再試行
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

## 開発時の品質ゲート

- `cargo fmt --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test`
