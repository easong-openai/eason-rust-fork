Hybrid Append‑Only History Refactor (ONE SHOT PLAN)

Goal
Replace the existing fullscreen ratatui UI with a single implementation that:

1. Prints every past message/log/patch/exec line exactly once to stdout (native scrollback & copy).
2. Keeps the current rich editing UX (multi‑line textarea, large‑paste placeholders, slash‑command popup, @file search popup, history navigation, Ctrl+C/D semantics, token usage placeholder, approvals) using the existing ChatComposer logic.
3. Only redraws a small bottom region (composer + transient popups + status/approvals) using raw ANSI – no alternate screen, no global re-render, no interference with scrollback.

Key Principles
Append-only printing for history; immutable once written.
Isolate bottom interactive zone; redraw in place only when it changes.
Reuse existing editing/business logic (ChatComposer, command/file popups, approval queue handling) by exposing plain-text render methods instead of ratatui widgets.
Eliminate ratatui::Terminal usage; keep tui_textarea + style types internally where convenient.

Top-Level Changes

1. Remove alternate screen + fullscreen draw loop.
2. Introduce a small “bottom renderer” that converts composer + popups + status into Vec<String> plus cursor coordinates.
3. Replace WidgetRef rendering calls with direct ANSI printing.
4. Convert conversation/history widgets into formatting helpers that emit plain lines; print them immediately on events.
5. Provide a thread reading crossterm events and forwarding to the main loop (like current AppEvent thread) but without ratatui.

Data Flow
Codex events -> event handler -> (a) update bottom state (token usage, approvals) OR (b) print history lines immediately.
User input -> ChatComposer (unchanged logic) -> on submit -> print user message + send Op to agent.
Logs -> printed immediately -> bottom redraw if status preview needs update.

Rendering Strategy
Bottom redraw:

1. Compute lines (popups first, then bordered textarea, then status line).
2. Determine required height H.
3. Move cursor to (0, term_h - H); Clear from cursor down.
4. Print lines; place cursor at composer cursor position inside textarea region.
   Borders: use simple unicode box chars (┌ ┐ └ ┘ │ ─) or ASCII fallback if width < 4.
   Popups: simplified textual representation (selected item prefixed with '>').
   File popup: show query line + up to N matches (highlight selection with '>').
   Command popup: list matching commands with descriptions.

Approvals
Approval requests printed to history (summary) AND appear as inline prompt in bottom (first pending request) until answered (y/n). Keys intercepted before composer when composer empty and no active popup.

Token Usage / Placeholder
Reuse ChatComposer::set_token_usage; placeholder string preserved.

Event Loop
Channels:

- Input events (Key, Paste, Resize) from reader thread.
- Log lines (existing mpsc -> immediate print).
- Codex events (async task -> immediate print + bottom state updates).
  Main loop processes events in order; coalesces rapid compose edits by redrawing bottom after each processed event (cheap small region).

History Formatting
Re-use existing logic from conversation_history_widget / history_cell where possible for:

- Agent/user prefixes
- Exec / patch summaries
  Simplify by dropping complex inline colour if necessary initially – can add ANSI later.

Removed/Deprecated Components
Ratatui Terminal drawing & related widget render paths.
conversation_history_widget.rs rendering surfaces (logic converted to helpers if still needed for formatting).
status_indicator_widget.rs (status folded into bottom status line).
user_approval_widget.rs modal (inline approval prompt replaces modal).
Scroll logic & scroll wheel event handling (native terminal scroll now).

Edge Cases
Resize: re-measure terminal size; re-render bottom only; history remains as-is (no reflow).
Very small terminal: truncate popup/composer lines to fit; never crash.
Large pastes: placeholder substitution preserved via existing ChatComposer logic.

Completion Criteria

1. No alternate screen; exiting leaves history in scrollback.
2. Multi-line editing, slash popup, file search popup, placeholders, history navigation all work.
3. Messages/logs never disappear or re-render; copy/paste is natural.
4. Approvals can be granted/denied (y/n) and recorded in history.
5. Ctrl+C (double) and Ctrl+D (empty composer) exit semantics preserved.
6. Token usage placeholder updates still appear.

Out-of-Scope (Future Enhancements)
Incremental streaming (mid-delta live painting). Currently accumulate deltas then print final message (can extend later to flush periodically).
Markdown rich ANSI styling for headings / code blocks (initial pass minimal).
Scroll-region optimization (DECSTBM) – optional later.

Implementation Order (still single commit, conceptual sub-steps)

1. Remove flag / fullscreen path.
2. Add bottom renderer + composer export functions.
3. Adapt popups to expose plain-text render lines.
4. Implement event loop + history printer.
5. Port approval flow (y/n interception) + token usage updates.
6. Remove obsolete modules / code paths.

---

## Overview

Current: ratatui owns an alternate screen; whole frame re-renders → hard to copy, scroll jank, internal scroll logic.
Target: Hybrid mode (no alternate screen) + streaming prints for history + pinned interactive bottom region.

---

## Top-Level Tasks

1. Remove alternate screen and all ratatui fullscreen draw code.
2. Expose message/log “print events” from App (PrintableEvent channel).
3. Implement lightweight bottom composer + status renderer.
4. Append history lines via plain stdout writes; redraw only bottom region.
5. Graceful shutdown + maintain backward compatibility (fullscreen path removed).

---

## Phase 0: Remove Legacy Fullscreen Mode

- Delete any CLI/config flags (e.g. `--hybrid-static`) and the `TuiMode` enum.
- Eliminate all branching on UI mode; the codebase now always runs in the
  append-only, no-alternate-screen mode.
- Remove every use of `ratatui::Terminal`’s alternate-screen draw loop.

---

## Phase 1: Core State Exposure

App now runs exclusively in hybrid mode.  
Introduce `printable_tx: Option<UnboundedSender<PrintableEvent>>` and the
associated setters to let the UI emit printable events.

---

## Phase 2: PrintableEvent Channel

Enum PrintableEvent:

- MessageLines(Vec<String>) // fully wrapped lines for a conversation message
- LogLine(String) // single log line
- StatusChange(StatusState) // bottom status update trigger
  Hybrid runner receives these and prints / redraws accordingly.

---

## Phase 3: Hybrid Output Layer

Module hybrid/output.rs:
struct HybridOutput { term_w, term_h, reserved: u16 }
Methods:
fn update_size(&mut self)
fn print_history_lines(&mut self, lines: &[String])
fn redraw_bottom(&mut self, composer: &ComposerState, status: &StatusState)

Terminal control (crossterm):

- Do NOT enter alternate screen.
- enable_raw_mode + EnableBracketedPaste.
- History insertion row = term_h - reserved.
- For each batch of lines: MoveTo(0, history_insert_row - 1) then println! per line; natural scroll handles overflow.
- After printing: redraw_bottom.

Clearing bottom: MoveTo(0, term_h - reserved) then Clear(FromCursorDown) (or per-line Clear(CurrentLine)).

---

## Phase 4: Composer Extraction

New module hybrid/composer.rs:
struct ComposerState { lines: Vec<String>, cursor_row, cursor_col }
Methods: insert_char, insert_str, backspace, move_left/right/up/down, take() -> String.
fn render_lines(&self, width: u16) -> Vec<String> (wrap algorithm: simple Unicode width accumulation).
Use existing formatting helpers where feasible (adapt from text_formatting.rs).

---

## Phase 5: Status Handling

StatusState { mode: enum (Idle, Streaming, AwaitingApproval, Error(String)), token_count, etc. }
Status updates push PrintableEvent::StatusChange OR just store and trigger bottom redraw.

---

## Phase 6: Hybrid Run Loop

hybrid/run.rs core algorithm:

- Initialize terminal (raw mode + bracketed paste; no alternate screen).
- Create PrintableEvent channel; register with App.
- Print optional banner.
- Event loop (select across):
  - PrintableEvent rx
  - crossterm::event::read (Key, Paste, Resize)
- On MessageLines / LogLine: output.print_history_lines(); output.redraw_bottom().
- On StatusChange: output.redraw_bottom().
- On Key:
  Enter => submit_user_message(composer.take()), rely on App to emit user message lines later, redraw.
  Editing keys => mutate composer; redraw bottom.
  Ctrl+C => break loop.
- On Paste (BracketedPaste): composer.insert_str(); redraw bottom.
- On Resize: update term size; recompute composer wrapped lines; redraw bottom.
- Shutdown: disable_raw_mode, DisableBracketedPaste, newline.

---

## Phase 7: Emission Points in App

Where user message created:

- Format lines (prefix, color via ANSI) → PrintableEvent::MessageLines.
  Where assistant streaming completes:
- Assemble final content; format/wrap; emit PrintableEvent::MessageLines.
  Log forwarding task: convert existing log_rx to PrintableEvent::LogLine.

Formatting helper: format_message_as_plain_lines(&ConversationMessage, width) -> Vec<String>

- Convert markdown to ANSI/plain (reuse markdown.rs or fallback to simple wrap ignoring styling initially).

---

## Phase 8: Wrapping Strategy

Initial implementation: pre-wrap only when emitting; no reflow on resize (document limitation).
If resizing: bottom composer rewraps; historical lines remain as originally printed.

---

## Phase 9: Approvals / Modals in Hybrid Mode

MVP: Inline textual prompt appended to history:
"Approval needed: (A)ccept / (E)dit / (R)eject"
Then capture next qualifying key. Skip ratatui modal layout.
Future: dedicated overlay inside bottom reserved region.

---

## Phase 10: Incremental Delivery Steps

1. Delete fullscreen code path; always use the append-only hybrid runner.
2. Implement raw-mode loop with a trivial echo composer.
3. Hook PrintableEvent for user messages only.
4. Emit assistant messages after completion.
5. Add logs.
6. Flesh out composer multi-line + wrapping.
7. Add status line.
8. Implement approvals inline.
9. Polish: colors, prefixes, error states.

---

## Phase 11: Testing

Unit tests: composer editing + wrapping logic.
Integration (optional): spawn binary with --hybrid-static feeding keystrokes; assert stdout contains expected sequences (may need PTY; mark ignored when unsupported).
Regression: ensure existing ratatui snapshot tests unaffected (they execute only Full mode path).

---

## Phase 12: Performance & Future Enhancements

Later improvements (optional):

- Scroll region (DECSTBM) to shield bottom area instead of manual clearing.
- Reflow history on resize by keeping a retained buffer (requires clearing / reprinting, losing pure native scroll fidelity).
- Streaming partial assistant deltas (emit incremental lines) once stable.

---

## Edge Cases & Handling

Terminal size too small: clamp reserved <= term_h - 1; truncate rendered composer lines.
Large paste: accept; optionally enforce max character limit.
Mouse capture: likely disable in hybrid (selection friendliness). Offer toggle key if needed.

---

## Data Structure Summary

enum PrintableEvent { MessageLines(Vec<String>), LogLine(String), StatusChange(StatusState) }
struct ComposerState { lines: Vec<String>, cursor_row: usize, cursor_col: usize }
struct HybridOutput { term_w: u16, term_h: u16, reserved: u16 }
struct StatusState { mode: StatusMode, token_count: u32, ... }
enum StatusMode { Idle, Streaming, AwaitingApproval, Error(String) }

---

## Risks & Mitigations

Accidental overwriting of history: ensure bottom redraw only touches reserved lines (Clear FromCursorDown starting at reserved top).
Interleaved writes from threads: funnel all printing through Hybrid run loop (PrintableEvent channel) rather than direct println! from other tasks.
Unstable formatting: start simple (no markdown) then incrementally add ANSI formatting.

---

## Completion Criteria

1. No alternate screen; exiting leaves history in scrollback.
2. Composer functions (multi-line edit, paste, submit) and bottom status
   updates without affecting earlier lines.
3. Messages/logs never disappear or re-render; copy/paste is natural.
4. Approvals can be granted/denied (y/n) and recorded in history.
5. Ctrl+C (double) and Ctrl+D (empty composer) exit semantics preserved.
6. Token usage placeholder updates still appear.

---

## Notes

Keep initial scope tight: no resize reflow, no complex popups. Add sophistication only after baseline UX improves.
