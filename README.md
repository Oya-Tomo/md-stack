# md-stack

A viewer that you open **next to Claude Code**, in another terminal pane or window. Claude Code's terminal cannot render math or make code blocks easy to copy, so md-stack gives Claude Code an **MCP server** to send its answers to, and shows them in a TUI with rendered KaTeX math, syntax-highlighted code, and one-key copying.

https://github.com/user-attachments/assets/c102d69f-7f83-4868-86b4-5fa955818220

## How it works

```
┌─ terminal 1 ─────────────────┐        ┌─ terminal 2 ─────────────────┐
│ claude                       │        │ md-stack tui                 │
│                              │  post  │                              │
│ > derive the formula         │ ─────> │  x = (-b ± √(b²-4ac)) / 2a   │
│ → md-stack #3                │  (MCP) │  ╭─ rust ──────────── [2] ─╮ │
│                              │        │  │ fn solve(...)           │ │
└──────────────────────────────┘        └──────────────────────────────┘
```

- The md-stack plugin adds an MCP server (`md-stack mcp`) with a `post` tool to Claude Code. Whenever an answer contains code or math, Claude posts it there and writes only `→ md-stack #N` in its own terminal.
- The server renders the math with MathJax when the post arrives. If an expression does not render, the post is rejected and Claude is told what to fix.
- `md-stack tui` follows the Claude Code session you pick and shows each post as it arrives. A `SessionStart` hook (`md-stack hook`) keeps it on the right conversation across `/clear` and `--resume`.

See [docs/SPEC.md](docs/SPEC.md) for the specification.

## Requirements

- Rust (edition 2024)
- Node.js and npm (only to rebuild the MathJax bundle)
- A terminal with a graphics protocol (Kitty, iTerm2 or Sixel) and OSC 52, e.g. WezTerm, kitty, Ghostty or foot
- Linux

## Build and install

```sh
# Install md-stack on your PATH
cargo install --path .

# Install the Claude Code plugin (MCP server + SessionStart hook)
claude plugin marketplace add "$PWD"
claude plugin install md-stack@md-stack
```

The plugin takes effect the next time Claude Code starts.

The MathJax bundle `assets/mathjax.js` is checked in and embedded in the binary.
To rebuild it, run `(cd mathjax && npm ci && npm run build)`.

## Usage

1. Open a terminal pane or window next to the one where you run Claude Code, and run `md-stack tui` there
2. Start Claude Code as usual; answers containing code or math appear in md-stack
3. If several Claude Code sessions are running, pick one in the TUI (with only one, it is picked automatically)

| Key | Action |
|---|---|
| `j` / `k`, `Ctrl+d` / `Ctrl+u`, `g` / `G` | Scroll |
| `J` / `K`, `]` / `[` | Next / previous post |
| `Tab` / `Shift+Tab` | Select a code block or math expression |
| `y` / `Y` | Copy the selected code block or math expression / the whole post |
| `f` | Switch between following new posts and locking to the current one |
| `p` | Move the post list: left → bottom → hidden |
| `s` | Session selection screen |
| `Ctrl+L` | Redraw the whole screen |
| `q` | Quit |

Posts are stored in `$XDG_STATE_HOME/md-stack/` and are removed when Claude Code deletes the conversation's transcript.
