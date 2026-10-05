# md-stack Specification

## 1. Overview

Claude Code cannot render math in the terminal, and its code blocks are awkward to copy.
md-stack lets Claude Code post output containing code blocks and KaTeX-syntax math over MCP,
and shows it in a TUI viewer running in a separate terminal, where it is rendered and can be copied.

### 1.1 Goals

- Render math (KaTeX syntax) in Claude Code's output correctly in the terminal
- Copy a code block or a math source to the clipboard with a single key
- Organize posts per Claude Code conversation (session), following `/clear` and `--resume` naturally

### 1.2 Non-goals

- A good experience in terminals without a graphics protocol (they get `ratatui-image`'s low-resolution half-block fallback)
- Operating systems other than Linux (process information is read from `/proc`; left open for later)
- Editing or appending to posts (e.g. an `update` tool); to be considered when needed

## 2. Architecture

A single Rust binary, `md-stack`, runs as separate processes through three subcommands.

| Subcommand | Started by | Role |
|---|---|---|
| `md-stack mcp` | Claude Code (stdio MCP server) | Accepts posts, renders math to SVG, writes to the store |
| `md-stack hook` | Claude Code (`SessionStart` hook) | Records which conversation each Claude Code process currently shows |
| `md-stack tui` | The user (separate terminal) | Session selection, rendering posts, copying |

The processes communicate only through a file-based store (§3).
None of them depends on start order, and a TUI started later still shows earlier posts.

```
 Claude Code (pid P) ──stdio──> md-stack mcp ──write──┐
        │                                              ▼
        └──SessionStart──> md-stack hook ──write──> store ──watch──> md-stack tui
```

### 2.1 Design rationale (observed with Claude Code 2.1.284)

| Event | Observation |
|---|---|
| `/clear` | The session ID changes. The MCP server process is not restarted and keeps running |
| `CLAUDE_CODE_SESSION_ID` in the MCP server's environment after `/clear` | Still the old ID (unusable for identifying the current conversation) |
| `_meta` of tool calls | Only `claudecode/toolUseId` and `progressToken`; no session ID |
| `--resume` | A new MCP server process is started |
| Parent process of the MCP server | Claude Code itself (direct parent) |
| `CLAUDE_PID` in the hook's environment | The pid of Claude Code itself |
| `source` of `SessionStart` | Fires for each of `startup` / `clear` / `resume` |
| Claude Code exits | The MCP server receives SIGTERM |

The MCP server alone cannot tell which conversation is current after `/clear`.
The hook therefore records the mapping "Claude Code process → current conversation", and the MCP server looks it up by its parent pid.

## 3. Store

The root is `$XDG_STATE_HOME/md-stack/` (`~/.local/state/md-stack/` when unset).

```
md-stack/
├── processes/
│   └── <claude_pid>.json        # one file per running Claude Code process
└── sessions/
    └── <session_id>/            # one directory per Claude Code conversation
        ├── session.json
        ├── 0001.md              # post body (the Markdown Claude posted, verbatim)
        ├── 0001.json            # post metadata
        └── 0001/
            ├── 0.svg            # one SVG per math expression (document order, 0-based)
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

- `process_start_time` is the starttime field of `/proc/<pid>/stat`. To avoid confusion from pid reuse, a process counts as alive only if the pid exists and its starttime matches.

### 3.2 `sessions/<session_id>/session.json`

```json
{
  "session_id": "5c5d57c6-30bd-4401-b78f-10381411128e",
  "cwd": "/home/user/project",
  "transcript_path": "/home/user/.claude/projects/.../5c5d57c6-....jsonl",
  "created_at": "2026-10-05T11:26:16+09:00"
}
```

### 3.3 Post metadata `NNNN.json`

```json
{
  "id": 1,
  "title": "The quadratic formula",
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

- `math` is in document order, and the SVG of `math[i]` is `NNNN/<i>.svg`. The TUI matches the math nodes of the body to this array in order of appearance.
- `width_ex` / `height_ex` / `depth_ex` are the dimensions reported by MathJax, in ex. The TUI uses them to size images on the cell grid and to align them to the text baseline.
- Post IDs are sequential within a session, starting at 1.

### 3.4 Write protocol

- Every file is written to a temporary file and then moved into place with `rename`, so the TUI never reads partial contents.
- A post is placed in the order `NNNN/` (SVGs) → `NNNN.md` → `NNNN.json`; **a post is complete once `NNNN.json` exists**. The TUI only uses `NNNN.json` to detect posts.
- A post ID is reserved by creating the `NNNN/` directory for the current maximum ID + 1. Creation is atomic; if the directory already exists, the next ID is tried.

### 3.5 Cleanup

- **Sessions**: when the `transcript_path` in `session.json` no longer exists, the session directory is deleted. This follows Claude Code's own retention of conversations (`cleanupPeriodDays`).
- **Processes**: `processes/*.json` entries that fail the liveness check (§3.1) are deleted.
- Cleanup runs when `md-stack mcp` and `md-stack tui` start.
- `/clear` and exiting Claude Code do not delete posts, since the conversation can be resumed with `/resume`.

## 4. `md-stack hook`

Registered as a `SessionStart` hook without a matcher, so it runs for `startup`, `clear` and `resume`.

1. Read `session_id`, `cwd` and `transcript_path` from the hook JSON on stdin
2. Get the Claude Code pid from the `CLAUDE_PID` environment variable
3. Create or overwrite `processes/<CLAUDE_PID>.json`

The session directory and `session.json` are created by `md-stack mcp` on the conversation's first post.
At hook time the transcript may not exist yet, and creating the session earlier would make it a target of cleanup (§3.5).

So that it never disturbs Claude Code, the hook exits with status 0 even on failure and reports errors on stderr.

## 5. `md-stack mcp`

An MCP server over stdio.

### 5.1 Resolving the current conversation

On every tool call, the server looks up `processes/<ppid>.json` using its parent pid (Claude Code itself) and posts to the `session_id` found there.
Because the process survives `/clear`, the lookup is not cached at startup.

If there is no record (e.g. the hook is not installed), the post is refused with an error describing the setup problem.

### 5.2 Tool `post`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `markdown` | string | ✓ | Post body: CommonMark + GFM + math (`$...$` / `$$...$$`) |
| `title` | string | | Title for the post list. Defaults to the first heading or the first line of the body |

Processing:

1. Resolve the current conversation (§5.1)
2. Parse the Markdown and extract all math (inline and display)
3. Render every expression to SVG (§7)
4. **If any expression fails, save nothing** and return an error
5. If all succeed, save the post (§3.4) and return its ID

Result on success:

```
Posted as md-stack #3.
```

Result on failure (`isError: true`):

```
Post rejected: 2 math expression(s) failed to render. Nothing was saved; fix and post again.

[1] display math, line 12
    source: \frac{a}{b
    error:  Missing close brace

[2] inline math, line 20
    source: \foo{x}
    error:  Undefined control sequence \foo
```

### 5.3 Server instructions

The MCP `instructions` tell Claude to:

- Post the whole response, including the explanation, with `post` whenever it contains a code block or math
- Write only the post ID and a short summary in the terminal, such as `→ md-stack #N`
- Fix the reported expressions and post again when `post` returns an error

## 6. `md-stack tui`

### 6.1 Requirements

- A terminal with a graphics protocol supported by `ratatui-image` (Kitty, iTerm2 or Sixel), e.g. WezTerm, kitty, Ghostty or foot
- A terminal that allows clipboard writes through OSC 52

At startup `ratatui-image` queries the terminal for its graphics protocol and cell size in pixels. The protocol is chosen by `ratatui-image` (for example iTerm2 in WezTerm, whose other protocols are unreliable), and the cell size scales math images.

### 6.2 Session selection

- The choices are **running Claude Code processes** (live entries in `processes/`), shown with cwd, time and post count.
- The selection screen appears at startup. If exactly one process is running, it is selected automatically.
- The selection screen can be reopened with a key while viewing.
- The TUI watches the selected process's `processes/<pid>.json`; when `/clear` or `--resume` changes its `session_id`, it switches to the new conversation automatically.

### 6.3 Layout

```
 md-stack ~/project [follow]
#1 The quadratic formula │  The quadratic formula
#2 Ownership in Rust     │
#3 Diagonalization       │  For [a≠0], the roots are
                         │
                         │        [x = (-b ± √(b²-4ac)) / 2a]        [2]
                         │
                         │  ╭─ rust ───────────────────────────── [3] ─╮
                         │  │ fn main() { ... }                        │
                         │  ╰──────────────────────────────────────────╯
s:sessions  J/K:post  j/k:scroll  Tab:snippet  y:copy  Y:copy post  f:follow/lock  p:list  q:quit
```

(`[...]` marks math drawn as images.)

- Header: cwd of the followed Claude Code and the mode, `[follow]` or `[locked]`
- Post list (ID and title): on the left by default; `p` cycles it through left, bottom and hidden
- Content pane: the selected post, rendered
- Follow mode (on by default) opens new posts as they arrive. Lock mode stays on the current post instead. `f` switches between them, and selecting another post by hand switches to lock mode.

### 6.4 Rendering

- Markdown: headings, emphasis, lists, block quotes, tables, links, rules, code blocks and math
- Code blocks: drawn in a rounded box with the language and snippet number on the top edge, with syntax highlighting based on the language tag
- Display math: the SVG is rasterized and shown centered as an image
- Inline math: the SVG is rasterized and embedded in the text as an image. The TUI wraps text itself, reserving cells for the image width. Images sit on the text baseline; a line grows by the rows an image needs above or below the baseline
- Tables: cells hold the same inline content as paragraphs, including math images. Columns are as wide as their widest cell; a table too wide for the pane narrows its widest columns and wraps their cells
- Rasterized images are cached per cell size and post
- The background color is queried from the terminal with OSC 11 at startup and decides the image background and the math color (light on dark backgrounds)

### 6.5 Snippets

Code blocks, display math and inline math are *snippets*: copyable pieces, numbered in document order.
Focusing a snippet scrolls it into view if it is off screen.
Copying math yields its TeX source (without the `$` delimiters).

### 6.6 Key bindings

| Key | Action |
|---|---|
| `j` / `k`, `↓` / `↑` | Scroll the right pane |
| `Ctrl+d` / `Ctrl+u` | Scroll half a page |
| `g` / `G` | Scroll to the top / bottom |
| `J` / `K`, `]` / `[` | Next / previous post |
| `Tab` / `Shift+Tab` | Focus the next / previous snippet |
| `y` | Copy the focused snippet |
| `Y` | Copy the whole post as Markdown |
| `f` | Switch between follow and lock mode |
| `p` | Move the post list: left → bottom → hidden |
| `s` | Open the session selection screen |
| `Ctrl+L` | Redraw the whole screen |
| `q` | Quit |

Copying uses OSC 52.

## 7. Math rendering

- Method: MathJax runs in an embedded JS engine and converts TeX directly to SVG
- Rationale: KaTeX itself only outputs HTML/MathML and cannot produce SVG on its own. MathJax accepts KaTeX syntax nearly as is and outputs SVG directly
- Rendering happens once per post, in the MCP server. The TUI only rasterizes SVG
- MathJax errors (undefined commands, unbalanced braces, ...) are returned to Claude in the format of §5.2
- Packages that hide errors (`noerrors`, `noundefined`) or load asynchronously (`autoload`, `require`) are disabled, so every failure surfaces as an error
- Macros defined with `\newcommand` are shared within a post, not across posts
- MathJax draws the glyphs of its own fonts as paths. Characters outside them (e.g. CJK text in `\text{}`) are emitted as `<text>` and rasterized with system fonts

## 8. Claude Code integration

The MCP server registration and the `SessionStart` hook are distributed as one Claude Code plugin.

- `mcpServers`: `md-stack mcp`
- `hooks.SessionStart`: `md-stack hook`

The plugin lives in `plugin/`, and the marketplace definition that distributes it is `.claude-plugin/marketplace.json` at the repository root.
The `md-stack` binary is expected to be on `PATH`.

## 9. Libraries

| Purpose | Library |
|---|---|
| TUI | `ratatui`, `crossterm` |
| Terminal images | `ratatui-image` |
| MCP | `rmcp` (official Rust SDK) |
| Markdown parsing | `pulldown-cmark` (with the math extension) |
| Syntax highlighting | `syntect` (with the pure-Rust regex engine `fancy-regex`), with the syntax definitions curated by `bat` from `two-face` |
| JS engine (runs MathJax) | `rquickjs` (QuickJS) |
| MathJax | `mathjax-full` 3.2.2, bundled into one file with `esbuild` and embedded in the binary as `assets/mathjax.js` |
| SVG rasterization | `resvg` |
| File watching | `notify` |

## 10. Open issues

- Drawing performance of the right pane for very long posts
- TUI colors and themes
