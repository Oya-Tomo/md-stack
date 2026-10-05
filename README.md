# md-stack

Render the code blocks and KaTeX-syntax math that Claude Code writes in a TUI running in a separate terminal, and copy them with a single key.
See [docs/SPEC.md](docs/SPEC.md) for the specification.

## Requirements

- Rust (edition 2024)
- Node.js and npm (only to rebuild the MathJax bundle)
- A terminal with Sixel and OSC 52 support (e.g. WezTerm)
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

1. Run `md-stack tui` in a separate terminal
2. Start Claude Code; responses containing code or math are posted to md-stack
3. Select the running Claude Code session in the TUI (selected automatically if there is only one)

| Key | Action |
|---|---|
| `j` / `k`, `Ctrl+d` / `Ctrl+u`, `g` / `G` | Scroll |
| `J` / `K`, `]` / `[` | Next / previous post |
| `Tab` / `Shift+Tab` | Select a code block or math expression |
| `y` / `Y` | Copy the selected block / the whole post |
| `f` | Toggle following new posts |
| `s` | Session selection screen |
| `q` | Quit |

Posts are stored in `$XDG_STATE_HOME/md-stack/` and are removed when Claude Code deletes the conversation's transcript.
