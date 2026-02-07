# gh-watch

GitHub の PR / Issue / コメントを定期監視し、重複なしでデスクトップ通知する常駐 CLI + TUI ツールです。

## Prerequisites

- Rust 1.93+
- `gh` CLI
- GitHub 認証済みの `gh` (`gh auth login -h github.com`)

## Quick Start

1. 設定ファイルを作成

```bash
cp config.example.toml config.toml
```

2. `config.toml` の `[[repositories]]` を自分の監視対象に編集

3. 事前チェック

```bash
cargo run -- check --config ./config.toml
```

4. 常駐監視 + TUI 起動

```bash
cargo run -- watch --config ./config.toml
```

`--config` を省略した場合の既定パス:
- macOS/Linux: `~/.config/gh-watch/config.toml`
- Windows: `%APPDATA%\\gh-watch\\config.toml`

## Key Bindings

- `q`: 終了
- `r`: 手動更新
- `↑` / `↓`: タイムライン移動

## Behavior

- 初回起動時は通知せず、カーソルのみ初期化
- 通知済みイベントは SQLite で管理し、同一イベントの再通知を防止
- 既定 state DB パス:
  - macOS/Linux: `~/.local/share/gh-watch/state.db`
  - Windows: `%LOCALAPPDATA%\\gh-watch\\state.db`

## Repository Notes

- `config.toml` はローカル環境用のため Git 管理対象外（`.gitignore` で除外）
- 共有用のテンプレートは `config.example.toml`
