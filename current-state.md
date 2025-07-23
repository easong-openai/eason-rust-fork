# Hybrid TUI Current State (Append-Only Refactor)

## Overview

The legacy fullscreen ratatui `Terminal` draw loop has been replaced at runtime by a hybrid append‑only mode. History (user, assistant, reasoning, logs, approvals, tool events) is now printed once to stdout and left intact in the terminal scrollback. A narrow bottom region (composer + status + inline approval prompt) is re‑drawn in place using raw ANSI / crossterm without entering the alternate screen. Formatting still needs polish (there are known visual glitches), but the TypeScript and Rust builds now succeed (`pnpm run build`, `cargo test --all-features`).

## Key Characteristics

- No alternate screen; scrollback and native copy/paste work normally.
- Chat input still uses existing `ChatComposer` (tui-textarea + ratatui styles) for multi‑line editing, slash commands, file search tokens, history navigation, large‑paste placeholders, approvals interception.
- Streaming per‑token output has been disabled for now: assistant & reasoning deltas are buffered and only the final assembled message is appended (fallback flush on TaskComplete if no final `AgentMessage`).
- All dynamic events now funnel through a single `append_history_lines` helper that inserts lines immediately above the bottom pane based on the last recorded bottom height.
- Bottom pane redraw clears only its own lines (not the whole remaining screen) to avoid erasing newly appended history lines.
- Auto‑exit behavior is off by default; optional via `--auto-exit` flag. Non-interactive initial prompts keep the session open unless that flag is passed.

## State & Data Flow

- `SharedState` tracks approvals, token usage, pending answer state, and `last_bottom_height`.
- `redraw_bottom` records the composer/status height every time it re-renders; `append_history_lines` uses this to position new history content.
- Key events: approval keystrokes (y/n) when composer empty and an approval is pending intercept first, send decision, append result line, then redraw.
- Logs are forwarded asynchronously and appended (not rendered inside bottom region).

## Formatting

- Simple ANSI color usage (cyan user, green assistant, magenta reasoning, yellow approvals).
- Minimal markdown handling for assistant replies (code fence toggle, headings bolded, code lines prefixed with a dim │).
- Reasoning printed in full (could be optionally summarized later).

## What’s Removed / Deprecated (Runtime Path)

- ratatui `Terminal` full-frame redraw loop (legacy code still present but unused).
- Internal conversation history widget rendering logic (superseded by plain text lines).
- Per-delta streaming UI (reduced flicker & duplication issues for now).

## Remaining Direct println!

- Only startup banner and final "Session ended." message intentionally use direct println!; all other dynamic content is routed through `append_history_lines`.

## Known Limitations / Next Steps

1. Unreachable pattern & unused code warnings from legacy fullscreen modules (can be removed in a cleanup pass).
2. No scroll-region (DECSTBM) protection: rewriting relies on precise cursor math; extremely fast concurrent output during redraw could still cause transient flicker.
3. No reflow of previously printed lines on terminal resize (documented trade‑off for simplicity and scrollback fidelity).
4. Reasoning verbosity may push important output upward quickly; optional collapse or summary mode could help.
5. Streaming (incremental) output could be reintroduced with a stable single-block updater once baseline is solid.
6. Some multi-line blocks (stdout/stderr, patch diffs) are appended as separate lines; could group with a blank separator or framing for readability in a future pass.

## Flags / UX

- `--auto-exit`: exit after first full assistant response (one‑shot mode) – documented in README & help.
- Ctrl+C once: show quit hint (status line) / second Ctrl+C: exit.
- Ctrl+D on empty composer: exit.

## Quality / Tests

- All existing Rust tests pass (`cargo test --all-features`).
- `pnpm run build` succeeds for the CLI (ensure you install deps with `pnpm install`; running `npm install` alone will not wire up the workspace and can look like a broken build because dev deps such as `esbuild` are missing).
- Added no new test coverage specifically for hybrid output; future improvement could add snapshot tests using a pseudo‑terminal harness.

## Future Enhancement Ideas

- Scroll region to lock bottom pane without manual clearing.
- Optional condensed reasoning (first line only unless expanded).
- Richer markdown / ANSI styling (lists, links, code block theming).
- Configurable streaming cadence (e.g. flush every N chars or T ms).
- Remove unused fullscreen modules and associated warnings.

## Summary

Hybrid append‑only history mode is operational: history lines are stable, the input composer remains functional, and output no longer disappears due to redraws. The foundation is in place for further polish (streaming, styling, cleanup) without needing the alternate screen.
