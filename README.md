# md-stack

Claude Code が書くコードブロックや KaTeX 記法の数式を、別ターミナルの TUI でレンダリング表示・コピーするためのツール。
仕様は [docs/SPEC.md](docs/SPEC.md) を参照。

## 必要なもの

- Rust（edition 2024）
- Node.js と npm（MathJax のバンドルを作るときのみ）
- Sixel と OSC 52 に対応した端末（例: WezTerm）
- Linux

## ビルドとインストール

```sh
# MathJax を assets/mathjax.js にバンドルする（バイナリに埋め込まれる）
(cd mathjax && npm ci && npm run build)

# md-stack を PATH に入れる
cargo install --path .

# Claude Code にプラグイン（MCP サーバー + SessionStart フック）を入れる
claude plugin marketplace add "$PWD"
claude plugin install md-stack@md-stack
```

プラグインは Claude Code の次回起動から有効になる。

## 使い方

1. 別ターミナルで `md-stack tui` を起動する
2. Claude Code を起動すると、コードや数式を含む回答が md-stack に投稿される
3. TUI で起動中の Claude Code セッションを選ぶ（1 つだけなら自動で選ばれる）

| キー | 動作 |
|---|---|
| `j` / `k`, `Ctrl+d` / `Ctrl+u`, `g` / `G` | スクロール |
| `J` / `K`, `]` / `[` | 次 / 前の投稿 |
| `Tab` / `Shift+Tab` | コードブロック・数式を選択 |
| `y` / `Y` | 選択中のブロック / 投稿全体をコピー |
| `f` | 新しい投稿への自動追従を切り替え |
| `s` | セッション選択画面 |
| `q` | 終了 |

投稿は `$XDG_STATE_HOME/md-stack/` に保存され、Claude Code が会話の記録を消すと一緒に消える。
