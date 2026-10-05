# md-stack 仕様書

## 1. 概要

Claude Code はターミナル上で数式をレンダリングできず、コードブロックも綺麗にコピーできない。
md-stack は Claude Code にコードブロックや KaTeX 記法の数式を含む出力を MCP 経由で投稿させ、
別ターミナルで動く TUI ビューアでそれをレンダリング表示・コピーできるようにするツールである。

### 1.1 目的

- Claude Code の出力中の数式（KaTeX 記法）を、ターミナル上で正しくレンダリングして読めるようにする
- コードブロック・数式ソースをワンキーでクリップボードにコピーできるようにする
- Claude Code の会話（セッション）単位で投稿を管理し、`/clear`・`--resume` と自然に連動させる

### 1.2 対象外

- Sixel 非対応端末のサポート（画像表示できない環境向けのフォールバック表示は持たない）
- Linux 以外の OS（プロセス情報の取得に `/proc` を用いるため。将来の拡張余地として残す）
- 投稿の編集・追記（`update` ツール等）。必要になった時点で追加を検討する

## 2. 全体構成

Rust 製の単一バイナリ `md-stack` が、3 つのサブコマンドで別々のプロセスとして動作する。

| サブコマンド | 起動者 | 役割 |
|---|---|---|
| `md-stack mcp` | Claude Code（stdio MCP サーバー） | 投稿の受付、数式の SVG 化、ストアへの保存 |
| `md-stack hook` | Claude Code（`SessionStart` フック） | Claude Code プロセスと現在の会話の対応をストアに記録 |
| `md-stack tui` | ユーザー（別ターミナル） | セッション選択、投稿のレンダリング表示、コピー |

プロセス間通信はファイルベースのストア（§3）のみで行う。
各プロセスは起動順序に依存せず、TUI を後から起動しても過去の投稿を閲覧できる。

```
 Claude Code (pid P) ──stdio──> md-stack mcp ──write──┐
        │                                              ▼
        └──SessionStart──> md-stack hook ──write──> ストア ──watch──> md-stack tui
```

### 2.1 設計の根拠（検証結果: Claude Code 2.1.284）

| 事象 | 結果 |
|---|---|
| `/clear` | セッション ID は変わる。MCP サーバープロセスは再起動されず継続する |
| `/clear` 後の MCP サーバーの環境変数 `CLAUDE_CODE_SESSION_ID` | 古い ID のまま（現在の会話の特定には使えない） |
| ツール呼び出しの `_meta` | `claudecode/toolUseId`・`progressToken` のみ。セッション ID は含まれない |
| `--resume` | MCP サーバーは新規起動される |
| MCP サーバーの親プロセス | Claude Code 本体（直接の親） |
| フックの環境変数 `CLAUDE_PID` | Claude Code 本体の pid |
| `SessionStart` の `source` | `startup` / `clear` / `resume` のいずれでも発火する |
| Claude Code 終了時 | MCP サーバーに SIGTERM が届く |

MCP サーバー単体では `/clear` 後の現在の会話を知る手段がない。
そのため「Claude Code プロセス → 現在の会話」の対応はフックが記録し、MCP サーバーは自身の親 pid でそれを引く。

## 3. ストア

ルートは `$XDG_STATE_HOME/md-stack/`（未設定時は `~/.local/state/md-stack/`）。

```
md-stack/
├── processes/
│   └── <claude_pid>.json        # 起動中の Claude Code 1 プロセスにつき 1 ファイル
└── sessions/
    └── <session_id>/            # Claude Code の会話 1 つにつき 1 ディレクトリ
        ├── session.json
        ├── 0001.md              # 投稿本文（Claude が投稿した Markdown そのまま）
        ├── 0001.json            # 投稿メタデータ
        └── 0001/
            ├── 0.svg            # 数式ごとの SVG（本文中の出現順、0 始まり）
            └── 1.svg
```

### 3.1 `processes/<claude_pid>.json`

```json
{
  "claude_pid": 712352,
  "process_start_time": 123456789,
  "session_id": "5c5d57c6-30bd-4401-b78f-10381411128e",
  "cwd": "/home/user/project",
  "transcript_path": "/home/user/.claude/projects/.../5c5d57c6-....jsonl",
  "updated_at": "2026-10-05T11:26:16+09:00"
}
```

- `process_start_time` は `/proc/<pid>/stat` の starttime。pid 再利用による取り違えを防ぐため、生存判定は「pid が存在し、かつ starttime が一致する」ことで行う。

### 3.2 `sessions/<session_id>/session.json`

```json
{
  "session_id": "5c5d57c6-30bd-4401-b78f-10381411128e",
  "cwd": "/home/user/project",
  "transcript_path": "/home/user/.claude/projects/.../5c5d57c6-....jsonl",
  "created_at": "2026-10-05T11:26:16+09:00"
}
```

### 3.3 投稿メタデータ `NNNN.json`

```json
{
  "id": 1,
  "title": "二次方程式の解の公式",
  "created_at": "2026-10-05T11:30:00+09:00",
  "math": [
    {
      "display": true,
      "tex": "x = \\frac{-b \\pm \\sqrt{b^2-4ac}}{2a}",
      "width_ex": 20.765,
      "height_ex": 5.291,
      "depth_ex": 1.575
    }
  ]
}
```

- `math` は本文中の出現順で、`math[i]` の SVG が `NNNN/<i>.svg`。TUI は本文の数式ノードを出現順にこの配列と対応づける。
- `width_ex` / `height_ex` / `depth_ex` は MathJax が出力した寸法（ex 単位）。TUI が画像のセル数を決めるのに使う。
- 投稿 ID はセッション内で 1 から連番。

### 3.4 書き込み規約

- すべてのファイルは一時ファイルに書いてから `rename` で配置する（TUI が書きかけを読まないため）。
- 投稿は `NNNN/`（SVG）→ `NNNN.md` → `NNNN.json` の順に配置し、**`NNNN.json` の出現をもって投稿の完成**とみなす。TUI は `NNNN.json` のみを投稿の存在判定に使う。
- 投稿 ID の採番は、既存の最大 ID + 1 の `NNNN/` ディレクトリを作成して予約する（作成はアトミックで、既に存在すれば次の ID で再試行する）。

### 3.5 削除・掃除

- **セッション**: `session.json` の `transcript_path` が存在しなくなったら、そのセッションディレクトリを削除する。Claude Code 本体の会話保持期間（`cleanupPeriodDays`）にそのまま追従する。
- **プロセス**: 生存判定（§3.1）に失敗した `processes/*.json` を削除する。
- 掃除は `md-stack mcp` と `md-stack tui` の起動時に行う。
- `/clear` や Claude Code の終了ではログを削除しない（その会話は `/resume` で再開できるため）。

## 4. `md-stack hook`

`SessionStart` フックとして登録する（matcher なし。`startup` / `clear` / `resume` のすべてで実行）。

1. 標準入力のフック JSON から `session_id`・`cwd`・`transcript_path` を読む
2. 環境変数 `CLAUDE_PID` から Claude Code の pid を得る
3. `processes/<CLAUDE_PID>.json` を作成・上書きする

セッションディレクトリと `session.json` は、その会話で最初の投稿があったときに `md-stack mcp` が作成する。
フック時点ではトランスクリプトがまだ作られていないことがあり、先に作ると掃除（§3.5）の対象になってしまうため。

フックは Claude Code の動作を妨げないよう、失敗しても終了コード 0 で終了し、エラーは標準エラーに出す。

## 5. `md-stack mcp`

stdio で動く MCP サーバー。

### 5.1 現在の会話の解決

ツール呼び出しのたびに、親 pid（= Claude Code 本体）で `processes/<ppid>.json` を引き、その時点の `session_id` を投稿先とする。
`/clear` 後もプロセスは継続するため、起動時にキャッシュせず毎回引く。

レコードが無い場合（フック未設定など）は投稿を受け付けず、セットアップ不備を示すエラーを返す。

### 5.2 ツール `post`

| 引数 | 型 | 必須 | 説明 |
|---|---|---|---|
| `markdown` | string | ✓ | 投稿本文。CommonMark + GFM + 数式（`$...$` / `$$...$$`） |
| `title` | string | | 一覧表示用のタイトル。省略時は本文の最初の見出しまたは先頭行から生成 |

処理:

1. 現在の会話を解決する（§5.1）
2. Markdown をパースし、全数式（インライン・ディスプレイ）を抽出する
3. 全数式を SVG にレンダリングする（§7）
4. **1 つでも失敗したら、何も保存せず**エラーを返す
5. 全数式が成功したら、ストアに保存し（§3.4）、投稿 ID を返す

成功時の結果:

```
Posted as md-stack #3.
```

失敗時の結果（`isError: true`）:

```
Post rejected: 2 math expression(s) failed to render. Nothing was saved; fix and post again.

[1] display math, line 12
    source: \frac{a}{b
    error:  Missing close brace

[2] inline math, line 20
    source: \foo{x}
    error:  Undefined control sequence \foo
```

### 5.3 サーバー instructions

MCP の `instructions` で、Claude に次の運用を指示する。

- コードブロックまたは数式を含む回答は、説明文も含めて回答全体を `post` で投稿する
- ターミナルには本文を書かず、`→ md-stack #N` のように投稿 ID と短い要約のみを書く
- `post` がエラーを返したら、指摘された数式を修正して投稿し直す

## 6. `md-stack tui`

### 6.1 動作要件

- Sixel 対応端末（例: WezTerm）
- OSC 52 によるクリップボード書き込みが有効な端末

起動時に端末へセルのピクセルサイズを問い合わせ、数式画像のスケーリングに用いる。

### 6.2 セッション選択

- 選択対象は **起動中の Claude Code プロセス**（`processes/` のうち生存しているもの）。表示項目は cwd・セッション開始時刻・投稿数。
- 起動時に選択画面を出す。起動中のプロセスが 1 つだけなら自動で選択する。
- 閲覧中もキー操作で選択画面を開き、切り替えられる。
- 選択したプロセスの `processes/<pid>.json` を監視し、`/clear` や `--resume` で `session_id` が変われば、自動で新しい会話の表示に切り替える。

### 6.3 画面構成

```
┌ md-stack ─ ~/project (pid 712352) ──────────────────────────┐
│ #1 二次方程式の解の公式 │ # 二次方程式の解の公式           │
│ #2 Rust の所有権        │                                 │
│▶#3 行列の対角化         │ 係数 a≠0 のとき                 │
│                         │      -b ± √(b²-4ac)             │
│                         │  x = ──────────────  [数式画像] │
│                         │           2a                    │
│                         │ ┌ rust ─────────────── [2] ─┐  │
│                         │ │ fn main() { ... }          │  │
│                         │ └────────────────────────────┘  │
├─────────────────────────┴─────────────────────────────────┤
│ s:セッション  Tab:ブロック選択  y:コピー  Y:全文コピー  q:終了 │
└──────────────────────────────────────────────────────────────┘
```

- 左ペイン: 投稿一覧（ID・タイトル）
- 右ペイン: 選択中の投稿のレンダリング結果
- フォローモード（既定で有効）: 新しい投稿が来たら自動でその投稿を表示する。手動で別の投稿を選ぶと解除される。

### 6.4 レンダリング

- Markdown: 見出し・強調・リスト・引用・表・リンク・水平線・コードブロック・数式
- コードブロック: 言語指定に基づくシンタックスハイライト
- ディスプレイ数式: SVG をラスタライズし、中央寄せのブロック画像として Sixel 表示する
- インライン数式: SVG をラスタライズし、文中に画像として埋め込む。本文の折り返しは TUI が自前で計算し、数式画像の幅に応じたセル幅を確保する。画像の高さが 1 行を超える場合、その表示行は必要な行数ぶん高くなる
- ラスタライズ結果はセルサイズ・投稿単位でキャッシュする
- 背景色は起動時に OSC 11 で端末に問い合わせ、数式画像の背景と文字色（暗い背景なら明るい色）を決める
- Sixel の描き残しを防ぐため、スクロールや投稿の切り替えで画像の位置が変わるときは画面全体を再描画する

### 6.5 コピー対象（ブロック）

コードブロック・ディスプレイ数式・インライン数式をコピー可能なブロックとし、本文中の出現順に番号を振る。
数式のコピー内容は TeX ソース（区切り記号 `$` を含まない）。

### 6.6 キー操作

| キー | 動作 |
|---|---|
| `j` / `k`, `↓` / `↑` | 右ペインのスクロール |
| `J` / `K`, `]` / `[` | 次 / 前の投稿 |
| `Tab` / `Shift+Tab` | 次 / 前のブロックにフォーカス |
| `y` | フォーカス中のブロックをコピー |
| `Y` | 投稿全体の Markdown をコピー |
| `f` | フォローモードの切り替え |
| `s` | セッション選択画面を開く |
| `q` | 終了 |

コピーは OSC 52 で行う。

## 7. 数式レンダリング

- 方式: 組み込み JS エンジン上で MathJax を実行し、TeX から SVG を直接生成する
- 選定理由: KaTeX 本体は HTML/MathML しか出力できず SVG を単体生成できない。MathJax は KaTeX 記法をほぼそのまま受理でき、SVG を直接出力できる
- レンダリングは投稿時に MCP サーバーで 1 回だけ行う。TUI は SVG のラスタライズのみ行う
- MathJax のエラー（未定義コマンド、括弧不整合など）は §5.2 の形式で Claude に返す
- 生成する SVG はフォントのグリフをパスとして埋め込んだ自己完結形式とし、ラスタライズ時に外部フォントに依存しない

## 8. Claude Code への組み込み

MCP サーバー登録と `SessionStart` フックを、1 つの Claude Code プラグインとして配布する。

- `mcpServers`: `md-stack mcp`
- `hooks.SessionStart`: `md-stack hook`

プラグインは `plugin/` に、それを配布するマーケットプレイス定義はリポジトリ直下の `.claude-plugin/marketplace.json` に置く。
`md-stack` バイナリは `PATH` 上にあるものとする。

## 9. 使用ライブラリ

| 用途 | ライブラリ |
|---|---|
| TUI | `ratatui`, `crossterm` |
| Sixel 画像表示 | `ratatui-image` |
| MCP | `rmcp`（公式 Rust SDK） |
| Markdown パース | `pulldown-cmark`（数式拡張あり） |
| シンタックスハイライト | `syntect`（純 Rust の正規表現エンジン `fancy-regex` を使用） |
| JS エンジン（MathJax 実行） | `rquickjs`（QuickJS） |
| MathJax | `mathjax-full` 3.2.2 を `esbuild` で 1 ファイルにまとめ、`assets/mathjax.js` としてバイナリに埋め込む |
| SVG ラスタライズ | `resvg` |
| ファイル監視 | `notify` |

## 10. 未決事項

- 投稿が長大な場合の右ペイン内の描画性能（Sixel 画像のスクロール時の再描画方式）
- TUI の配色・テーマ
- 表のセル内の数式は画像化せず `$...$` のテキストで表示している
