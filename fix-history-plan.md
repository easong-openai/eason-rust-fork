Hybrid Append‑Only History Refactor (ONE SHOT PLAN)

Goal
Replace the existing fullscreen ratatui UI with a single implementation that:

1. Prints every past message/log/patch/exec line exactly once to scrollback (native scroll + copy) using ratatui Terminal::insert_before.
2. Keeps the current rich editing UX (multi‑line textarea, large‑paste placeholders, slash‑command popup, @file search popup, history navigation, Ctrl+C/D semantics, token usage placeholder, approvals) using the existing ChatComposer logic.
3. Only redraws a small bottom region (composer + transient popups + status/approvals) inside an inline ratatui viewport – no alternate screen, no global history re-render.

Key Principles
Append-only history leveraging ratatui Viewport::Inline + Terminal::insert_before; once inserted, lines are immutable.
Isolate bottom interactive zone as a fixed-height inline viewport; only it is diff-drawn.
Reuse existing editing/business logic; adapt render methods to produce Lines/Spans (ratatui styling).
Avoid manual ANSI cursor math; rely on ratatui for viewport drawing & cursor placement.

Top-Level Changes (Revised)

1. Drop alternate screen; initialize Terminal with Viewport::Inline(reserved_height).
2. Use Terminal::draw only for the bottom viewport (composer + popups + status).
3. Insert history lines above the viewport via Terminal::insert_before(height, |buf| {...}).
4. Refactor history formatting into helpers that build Lines for insert_before.
5. Input loop mutates composer → redraw viewport; history never re-renders.

Data Flow
Codex events -> PrintableEvent (history/status) -> run loop.
PrintableEvent::MessageLines/LogLine -> terminal.insert_before.
PrintableEvent::StatusChange -> viewport redraw.
User input -> ChatComposer -> submit -> App emits MessageLines.
Logs -> LogLine events.

Rendering Strategy

History:

- Each history event pre-wraps & styles into Lines.
- Group contiguous events when possible; compute total lines N; call terminal.insert_before(N, |buf| render_lines(buf)).

Bottom viewport:

- Fixed initial height (e.g. 14 lines) via Viewport::Inline(H). Later dynamic change by reconstructing Terminal.
- Render order: popups (overlay), bordered textarea, status/approval line.
- Cursor set with Frame::set_cursor relative to viewport origin.

Benefits of insert_before

- No manual ANSI clearing or cursor reposition.
- History insertion cost proportional only to new lines.
- Retains ratatui styling for bottom UI while keeping native scrollback.

Approvals
Inline prompt in status/approval line. Arrival inserts a history summary line. Resolution (accept/reject/edit) inserts another summary line and clears approval state.
Key interception (y/n/e) when composer empty & no popup & approval pending.

Token Usage / Placeholder
ChatComposer retains token placeholder logic; status line pulls data to display.

Event Loop
Select over PrintableEvent rx + crossterm events.
History events -> insert_before -> redraw viewport.
StatusChange -> redraw viewport.
Input keys -> update composer → redraw; Enter submits; Ctrl+C (double) / Ctrl+D(empty) exit.
Resize -> maybe adjust viewport height; redraw viewport; history fixed.

History Formatting
Helpers produce Vec<Line> (ratatui::text::Line) from structured messages (agent/user/exec/patch/log). Markdown optionally converted to styled spans (initially minimal styling).

Removed/Deprecated Components

- Alternate screen / fullscreen draw loop.
- Internal history scroll logic & scroll wheel handling (native scrollback now).
- Fullscreen approval modal (replaced by inline viewport prompt + history summary line).
- Redundant history widget layers (converted to formatting helpers).

Edge Cases
Resize: recompute size; optional viewport height adjustment; history lines remain (no reflow).
Very small terminal: clamp viewport height; if viewport fills screen, insert_before pushes directly to scrollback.
Large pastes: placeholder substitution preserved; size guard optional.

Completion Criteria

1. No alternate screen; exiting leaves history in scrollback.
2. Multi-line editing, popups, placeholders, history navigation all work in inline viewport.
3. Messages/logs never disappear or re-render; natural copy/paste.
4. Approvals flow (y/n/e) works; recorded in history.
5. Ctrl+C (double) and Ctrl+D (empty composer) exit semantics preserved.
6. Token usage placeholder updates appear.

Out-of-Scope (Initial)
Incremental token streaming (can later emit partial lines) – start with post-completion emission.
Historical reflow on resize (document limitation).
Scroll-region / DECSTBM optimization (maybe later).

Implementation Order

1. Remove fullscreen flags/modes; init inline viewport terminal.
2. Minimal composer + submit path emitting user MessageLines via insert_before.
3. Assistant message emission after completion.
4. Log forwarding.
5. Expand composer (multi-line, history nav, placeholders).
6. Status + token usage + approvals inline.
7. Popups (slash, file search) drawn in viewport.
8. History formatting helpers with styling.
9. Batch insert optimization.
10. Cleanup legacy modules.

Overview
Current: fullscreen alternate screen with full-frame redraw → poor copy & scroll.
Target: inline viewport anchored at bottom; immutable history via insert_before; minimal redraw surface.

Top-Level Tasks

1. Delete fullscreen mode; inline viewport terminal initialization.
2. PrintableEvent channel with styled/wrapped lines.
3. Viewport draw for composer/status/popups.
4. Insert history via terminal.insert_before.
5. Graceful shutdown leaving scrollback intact.

Phase 0: Transition to Inline Viewport

- Delete CLI/config flags for legacy modes; drop TuiMode.
- Initialize Terminal with Viewport::Inline(reserved_height); remove alternate screen enter/leave.
- Keep raw mode + bracketed paste enable.

Phase 1: Core State Exposure
Expose `printable_tx: Option<UnboundedSender<PrintableEvent>>` for history & status emission.
Provide getters for composer, popup, status state for viewport draw.

Phase 2: PrintableEvent Channel
enum PrintableEvent {
MessageLines(Vec<Line>),
LogLine(Line),
StatusChange(StatusState),
}
Lines are pre-wrapped & styled; runner maps to insert_before or viewport redraw.

Phase 3: Hybrid Output Layer (insert_before)
struct HybridOutput { terminal: Terminal<Backend>, viewport_height: u16 }
Methods:

- insert_history(&mut self, lines: &[Line]) → batch events, terminal.insert_before.
- redraw_viewport(&mut self, composer: &ComposerState, status: &StatusState, popups: &PopupState)
- maybe_resize(new_size) → adjust cached size & possibly viewport height policy.
  No manual clearing; ratatui handles diffs.

Phase 4: Composer Extraction
Composer maintains editing logic; render adapter returns (Paragraph widget, cursor_pos). Wrapping reused; output converted to Vec<Line> for textarea.

Phase 5: Status Handling
StatusState { mode: Idle | Streaming | AwaitingApproval | Error(String), token_count, ... }
Status changes -> PrintableEvent::StatusChange -> viewport redraw.

Phase 6: Hybrid Run Loop

- Initialize inline terminal.
- Select over events & input.
- MessageLines/LogLine: batch → insert_history → redraw viewport.
- StatusChange: redraw viewport.
- Keys: update composer; Enter submit; Ctrl+C (double) exit; Ctrl+D on empty exit.
- Paste: composer insert; redraw.
- Resize: maybe adjust viewport height; redraw.
- Shutdown: restore terminal, leave history.

Phase 7: Emission Points in App
On user submit / assistant completion: build Vec<Line>; emit MessageLines.
Logs -> LogLine events.
Markdown conversion optional (spans).

Phase 8: Wrapping Strategy
Wrap once at emission (width = terminal width minus margins). No historical reflow.

Phase 9: Approvals
Insert history summary on request + resolution. Display interactive prompt in status line; intercept keys.

Phase 10: Incremental Delivery Steps (detailed above) – ensures usable baseline early.

Phase 11: Testing
Unit: composer edits, wrapping, insert_before line count, approval key handling.
Integration: PTY run; feed keystrokes; assert stdout sequence includes emitted message lines (prefix markers) before viewport content markers.
Regression: viewport snapshot test for deterministic composer state.

Phase 12: Performance & Future Enhancements

- Dynamic viewport height adjustments.
- Batch multiple events per insert_before call.
- Streaming partial assistant tokens (flush line by line).
- Historical reflow (opt-in) by retaining logical paragraphs.
- Theming / color customization.

Edge Cases & Handling
Terminal very small: viewport shrinks but not below minimal composer+status lines (e.g. 5). insert_before still functions.
Large paste: optional placeholder token if exceeding threshold.
Mouse capture: disabled by default to allow selection; toggle later if needed.

Data Structure Summary
enum PrintableEvent { MessageLines(Vec<Line>), LogLine(Line), StatusChange(StatusState) }
struct ComposerState { lines: Vec<String>, cursor_row: usize, cursor_col: usize }
struct HybridOutput { terminal: Terminal<Backend>, viewport_height: u16 }
struct StatusState { mode: StatusMode, token_count: u32, ... }
enum StatusMode { Idle, Streaming, AwaitingApproval, Error(String) }

Risks & Mitigations
History overwrite: insert_before only appends; viewport redraw isolated.
Interleaved writes: single run loop funnels all events; no direct println! elsewhere.
Formatting complexity: start minimal, incrementally add markdown styling.

Completion Criteria

1. No alternate screen; exiting leaves history in scrollback.
2. Composer functions (multi-line edit, paste, submit) + status updates without affecting earlier lines.
3. Messages/logs never disappear or re-render; copy/paste natural.
4. Approvals handled inline; recorded in history.
5. Ctrl+C (double) & Ctrl+D semantics preserved.
6. Token usage placeholder updates appear.

Notes
Keep initial scope tight: fixed viewport height, no historical reflow, post-completion message emission. Expand only after baseline UX stable.
