use std::io::Write;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::channel;

use codex_core::codex_wrapper::init_codex;
use codex_core::config::Config;
use codex_core::protocol::*;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use crate::cli::Cli;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::ChatComposer;
use crate::bottom_pane::InputResult;

#[derive(Clone, Debug)]
enum ApprovalKind { Exec, Patch }

const RESERVED_INPUT_BUFFER: u16 = 4; // keep N blank lines above the input box to avoid overwriting streamed history.

#[derive(Default)]
struct SharedState {
    approvals: Vec<ApprovalRequestDetails>,
    token_usage: TokenUsage,
    last_bottom_content_height: u16, // height of composer/status (without buffer)
    redraw_tx: Option<std::sync::mpsc::Sender<AppEvent>>, // allow history prints to trigger redraw
}

#[derive(Clone, Debug)]
enum ApprovalRequestDetails {
    Exec { id: String, command: Vec<String>, cwd: std::path::PathBuf, reason: Option<String> },
    Patch { id: String, file_count: usize, reason: Option<String>, grant_root: Option<std::path::PathBuf> },
}

/// Entry point for append‑only hybrid UI.
pub(crate) fn run_hybrid(
    cli: Cli,
    config: Config,
    mut log_rx: tokio::sync::mpsc::UnboundedReceiver<String>,
) -> color_eyre::Result<()> {
    let auto_exit = cli.auto_exit;
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste)?;

    println!("Codex – append-only mode (Ctrl+C twice or Ctrl+D on empty input to quit)\n");

    // Channels & shared state
    let (app_tx, app_rx) = channel::<AppEvent>();
    let shared = Arc::new(Mutex::new(SharedState { redraw_tx: Some(app_tx.clone()), ..Default::default() }));

    // Composer (reuse existing logic)
    let composer_sender = AppEventSender::new(app_tx.clone());
    let mut composer = ChatComposer::new(true, composer_sender);

    let mut ctrl_c_first = false;

    // Submission channel to codex
    let (op_tx, mut op_rx) = unbounded_channel::<Op>();
    let op_tx_main = op_tx.clone();

    // Answer / reasoning streaming buffers
    let answer_buffer = Arc::new(std::sync::Mutex::new(String::new()));
    let reasoning_buffer = Arc::new(std::sync::Mutex::new(String::new()));

    // Spawn crossterm event reader thread
    {
        let app_tx = app_tx.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = crossterm::event::read() {
                match ev {
                    crossterm::event::Event::Key(k) => { let _ = app_tx.send(AppEvent::KeyEvent(k)); }
                    crossterm::event::Event::Paste(p) => { let _ = app_tx.send(AppEvent::Paste(p)); }
                    crossterm::event::Event::Resize(_, _) => { let _ = app_tx.send(AppEvent::RequestRedraw); }
                    _ => {}
                }
            }
        });
    }

    // Spawn codex init + event loop
    let initial_prompt = cli.prompt.clone().unwrap_or_default();
    let initial_images = cli.images.clone();
    let shared_clone = shared.clone();
    let app_tx_clone = app_tx.clone();
    let config_clone = config.clone();
    let answer2 = answer_buffer.clone();
    let reasoning2 = reasoning_buffer.clone();
    let exit_tx = app_tx.clone();
    tokio::spawn(async move {
        #[cfg(feature = "test-fake-agent")]
        {
            if std::env::var("CODEX_TUI_FAKE_AGENT").ok().as_deref()==Some("1") {
                use codex_core::protocol::*;
                let turns: usize = std::env::var("CODEX_TUI_FAKE_AGENT_TURNS").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
                let session = Event { id: "sess".into(), msg: EventMsg::SessionConfigured(SessionConfiguredEvent { session_id: uuid::Uuid::new_v4(), model: "fake-model".into(), history_entry_count: 0, history_log_id: 1 }) };
                let _ = app_tx_clone.send(AppEvent::CodexEvent(session));
                for i in 1..=turns {
                    // Reasoning (delta then final)
                    let _ = app_tx_clone.send(AppEvent::CodexEvent(Event { id: format!("t{i}"), msg: EventMsg::AgentReasoningDelta(AgentReasoningDeltaEvent { delta: format!("thinking {i} ") }) }));
                    let omit_final = std::env::var("CODEX_TUI_FAKE_AGENT_OMIT_FINAL").ok().as_deref()==Some("1");
                    if !omit_final {
                        let _ = app_tx_clone.send(AppEvent::CodexEvent(Event { id: format!("t{i}"), msg: EventMsg::AgentReasoning(AgentReasoningEvent { text: format!("done {i}") }) }));
                    }
                    // Message (delta and maybe final)
                    let _ = app_tx_clone.send(AppEvent::CodexEvent(Event { id: format!("t{i}"), msg: EventMsg::AgentMessageDelta(AgentMessageDeltaEvent { delta: format!("Turn {i} ") }) }));
                    if !omit_final {
                        let _ = app_tx_clone.send(AppEvent::CodexEvent(Event { id: format!("t{i}"), msg: EventMsg::AgentMessage(AgentMessageEvent { message: "response".into() }) }));
                    }
                }
                // Always emit TaskComplete so fallback can flush if finals omitted.
                let _ = app_tx_clone.send(AppEvent::CodexEvent(Event { id: "sess".into(), msg: EventMsg::TaskComplete(TaskCompleteEvent { last_agent_message: None }) }));
                return;
            }
        }
        let (codex, session_event, _ctrl_c, _session_id) = match init_codex(config_clone).await {
            Ok(v) => v,
            Err(e) => { tracing::error!("failed to initialize codex: {e}"); return; }
        };
        if let codex_core::protocol::Event { msg: codex_core::protocol::EventMsg::SessionConfigured(e), .. } = &session_event {
            print_session_configured(e);
        }
        if !initial_prompt.is_empty() || !initial_images.is_empty() {
            submit_user_message(&op_tx_main, initial_prompt.clone(), initial_images.clone());
            print_user_message(&initial_prompt, &shared_clone);
        }
        let codex = Arc::new(codex);
        let submit_clone = codex.clone();
        tokio::spawn(async move {
            while let Some(op) = op_rx.recv().await { if let Err(e) = submit_clone.submit(op).await { tracing::error!("submit op: {e}"); } }
        });
        while let Ok(event) = codex.next_event().await {
            let completed_turn = handle_event(event, &answer2, &reasoning2, &shared_clone, &app_tx_clone);
            if auto_exit && completed_turn { let _ = exit_tx.send(AppEvent::ExitRequest); }
        }
    });

    // Log forwarder
    tokio::spawn(async move { while let Some(line) = log_rx.recv().await { println!("[log] {line}"); } });

    // First render
    redraw_bottom(&composer, &shared, ctrl_c_first)?;

    // Main event loop
    while let Ok(ev) = app_rx.recv() {
        match ev {
            AppEvent::KeyEvent(key) => {
                // Ctrl+C semantics
                if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) && matches!(key.code, crossterm::event::KeyCode::Char('c')) {
                    if ctrl_c_first { break; } else { ctrl_c_first = true; redraw_bottom(&composer, &shared, ctrl_c_first)?; continue; }
                } else if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) && matches!(key.code, crossterm::event::KeyCode::Char('d')) {
                    if composer.is_empty() { break; }
                } else { ctrl_c_first = false; }

                // Approval interception
                let mut handled_approval = false;
                if composer.is_empty() {
                    if let crossterm::event::KeyCode::Char(ch) = key.code {
                        let mut g = shared.lock().unwrap();
                        if let Some(first) = g.approvals.first().cloned() {
                            let (id, kind_id) = match &first {
                                ApprovalRequestDetails::Exec { id, .. } => (id.clone(), ApprovalKind::Exec),
                                ApprovalRequestDetails::Patch { id, .. } => (id.clone(), ApprovalKind::Patch),
                            };
                            let decision = match ch {
                                'y' | 'Y' => Some(ReviewDecision::Approved),
                                'n' | 'N' => Some(ReviewDecision::Denied),
                                _ => None,
                            };
                            if let Some(dec) = decision {
                                let _ = g.approvals.remove(0);
                                drop(g);
                                match kind_id {
                                    ApprovalKind::Exec => { let _ = op_tx.send(Op::ExecApproval { id: id.clone(), decision: dec }); println!("[approval] exec {id} -> {:?}", dec); }
                                    ApprovalKind::Patch => { let _ = op_tx.send(Op::PatchApproval { id: id.clone(), decision: dec }); println!("[approval] patch {id} -> {:?}", dec); }
                                }
                                handled_approval = true;
                            }
                        }
                    }
                }
                if handled_approval { redraw_bottom(&composer, &shared, ctrl_c_first)?; continue; }

                let (res, _changed) = composer.handle_key_event(key);
                if let InputResult::Submitted(txt) = res {
                    submit_user_message(&op_tx, txt.clone(), Vec::new());
                    print_user_message(&txt, &shared);
                }
                redraw_bottom(&composer, &shared, ctrl_c_first)?;
            }
            AppEvent::Paste(p) => { composer.handle_paste(p); redraw_bottom(&composer, &shared, ctrl_c_first)?; }
            AppEvent::RequestRedraw | AppEvent::Redraw => { redraw_bottom(&composer, &shared, ctrl_c_first)?; }
            AppEvent::ExitRequest => break,
            _ => {}
        }
    }

    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste)?;
    println!("\nSession ended.");
    Ok(())
}

fn redraw_bottom(composer: &ChatComposer, shared: &Arc<Mutex<SharedState>>, ctrl_c_first: bool) -> std::io::Result<()> {
    use crossterm::{cursor, terminal, ExecutableCommand};
    let (tw, th) = terminal::size()?;
    let width = tw as usize;
    let (approvals, token_usage, old_content_height) = {
        let g = shared.lock().unwrap();
        (g.approvals.clone(), g.token_usage.clone(), g.last_bottom_content_height)
    };

    let mut approval_lines: Vec<String> = Vec::new();
    if let Some(first) = approvals.first() {
        match first {
            ApprovalRequestDetails::Exec { command, cwd, reason, .. } => {
                approval_lines.push(color_yellow(format!("APPROVAL (exec): {:?} cwd={}", command, cwd.display())));
                if let Some(r) = reason { approval_lines.push(color_dim(format!("reason: {r}"))); }
                approval_lines.push(color_yellow("press y / n".to_string()));
            }
            ApprovalRequestDetails::Patch { file_count, reason, grant_root, .. } => {
                approval_lines.push(color_magenta(format!("APPROVAL (patch): {file_count} files")));
                if let Some(root) = grant_root { approval_lines.push(color_dim(format!("grant root: {}", root.display()))); }
                if let Some(r) = reason { approval_lines.push(color_dim(format!("reason: {r}"))); }
                approval_lines.push(color_magenta("press y / n".to_string()));
            }
        }
    }

    let (mut lines, cursor_row, cursor_col) = export_composer_lines(composer, width, &approval_lines);

    // Status line
    let status = build_status_line(approvals.len(), &token_usage, ctrl_c_first, width);
    lines.push(status);
    let h = lines.len() as u16;
    let content_height_new = h as u16; // composer/status box + status line
    let total_new = content_height_new + RESERVED_INPUT_BUFFER;
    let start_row_new_total = th.saturating_sub(total_new); // first reserved buffer row
    let content_start_new = start_row_new_total + RESERVED_INPUT_BUFFER; // where we draw composer/status

    let content_start_old = if old_content_height > 0 {
        let total_old = old_content_height + RESERVED_INPUT_BUFFER;
        th.saturating_sub(total_old) + RESERVED_INPUT_BUFFER
    } else { th }; // nothing
    let mut stdout = std::io::stdout();
    // Clear old region (if any) row by row to avoid wiping newly printed
    // history lines above.
    if old_content_height > 0 {
        for row in content_start_old..th { stdout.execute(cursor::MoveTo(0,row))?; stdout.execute(terminal::Clear(terminal::ClearType::CurrentLine))?; }
    }
    // Ensure reserved buffer rows are blank
    for row in start_row_new_total..content_start_new { stdout.execute(cursor::MoveTo(0,row))?; stdout.execute(terminal::Clear(terminal::ClearType::CurrentLine))?; }
    // Draw new bottom content lines
    for (idx, l) in lines.iter().enumerate() {
        let row = content_start_new + idx as u16;
        stdout.execute(cursor::MoveTo(0,row))?;
        stdout.execute(terminal::Clear(terminal::ClearType::CurrentLine))?;
        write!(stdout, "{l}")?;
    }
    stdout.execute(cursor::MoveTo(cursor_col as u16, content_start_new + cursor_row as u16))?;
    stdout.flush()?;
    // record bottom content height (without buffer)
    if let Ok(mut g) = shared.lock() { g.last_bottom_content_height = content_height_new; }
    Ok(())
}

fn build_status_line(approvals: usize, token_usage: &TokenUsage, ctrl_c_first: bool, width: usize) -> String {
    let mut parts = Vec::new();
    if approvals > 0 { parts.push(format!("approval pending ({approvals}) y/n")); }
    parts.push(format!("tok in:{} out:{} total:{}", token_usage.input_tokens, token_usage.output_tokens, token_usage.total_tokens));
    if ctrl_c_first { parts.push("press Ctrl+C again to quit".into()); }
    let line = parts.join(" | ");
    truncate_str(&line, width)
}

fn export_composer_lines(composer: &ChatComposer, width: usize, approval_lines: &[String]) -> (Vec<String>, usize, usize) {
    let mut lines: Vec<String> = Vec::new();
    lines.extend(approval_lines.iter().cloned().map(|l| truncate_str(&l, width)));
    lines.extend(composer.popup_plaintext_lines(width));
    let (raw_lines, (crow, ccol)) = composer.export_plaintext();
    let content: Vec<String> = raw_lines
        .into_iter()
        .map(|l| truncate_str(&l, width.saturating_sub(2)))
        .collect();
    let top = format!("┌{}┐", "─".repeat(width.saturating_sub(2)));
    lines.push(truncate_str(&top, width));
    for l in &content { lines.push(format!("│{}│", pad_str(l, width.saturating_sub(2)))); }
    let bottom = format!("└{}┘", "─".repeat(width.saturating_sub(2)));
    lines.push(truncate_str(&bottom, width));
    // Cursor row offset: popup_lines + 1 top border + crow
    let popup_count = lines.len() - content.len() - 2; // lines before top border
    let cursor_row = popup_count + 1 + crow; // inside content area
    let cursor_col = (ccol).min(width.saturating_sub(3)) + 1; // inside box
    (lines, cursor_row, cursor_col)
}

fn handle_event(
    event: Event,
    answer: &std::sync::Mutex<String>,
    reasoning: &std::sync::Mutex<String>,
    shared: &Arc<Mutex<SharedState>>,
    redraw: &std::sync::mpsc::Sender<AppEvent>,
) -> bool {
    use EventMsg::*;
    let Event { id, msg } = event;
    let mut exit_after = false;
    match msg {
        AgentMessageDelta(AgentMessageDeltaEvent { delta }) => { answer.lock().unwrap().push_str(&delta); }
        AgentMessage(AgentMessageEvent { message }) => {
            let mut buf = answer.lock().unwrap();
            if buf.is_empty() { 
                // No deltas received, use the complete message directly
                print_agent_message(&message, shared); 
            } else { 
                // Had deltas, use accumulated buffer (the deltas contain the full message)
                let full = buf.clone();
                print_agent_message(&full, shared); 
                buf.clear(); 
            }
            exit_after = true; // end of a response turn
        }
        AgentReasoningDelta(AgentReasoningDeltaEvent { delta }) => { reasoning.lock().unwrap().push_str(&delta); },
        AgentReasoning(AgentReasoningEvent { text }) => {
            let mut buf = reasoning.lock().unwrap();
            if buf.is_empty() { 
                // No deltas received, use the complete reasoning directly
                print_reasoning(&text, shared); 
            } else { 
                // Had deltas, use accumulated buffer (the deltas contain the full reasoning)
                let full = buf.clone();
                print_reasoning(&full, shared); 
                buf.clear(); 
            }
        }
        TokenCount(u) => { shared.lock().unwrap().token_usage = u.clone(); let _=redraw.send(AppEvent::RequestRedraw); }
        TaskStarted => println!("[task] started"),
        TaskComplete(_) => {
            // Fallback flush: if we received deltas but never a final
            // AgentMessage / Reasoning event, print the accumulated buffers
            // now so the user sees the assistant's reply.
            let mut a = answer.lock().unwrap();
            if !a.is_empty() { let full = a.clone(); print_agent_message(&full, shared); a.clear(); exit_after = true; }
            drop(a);
            let mut r = reasoning.lock().unwrap();
            if !r.is_empty() { let full = r.clone(); print_reasoning(&full, shared); r.clear(); }
            println!("[task] complete");
        }
        Error(ErrorEvent { message }) => println!("[error] {message}"),
        ExecApprovalRequest(ExecApprovalRequestEvent { command, cwd, reason }) => {
            println!("{}", color_yellow(format!("[approval:exec] id={id} {:?} cwd={} reason={reason:?}", command, cwd.display())));
            let mut g = shared.lock().unwrap(); g.approvals.push(ApprovalRequestDetails::Exec { id, command, cwd, reason }); let _=redraw.send(AppEvent::RequestRedraw);
        }
        ApplyPatchApprovalRequest(ApplyPatchApprovalRequestEvent { changes, reason, grant_root }) => {
            println!("{}", color_magenta(format!("[approval:patch] id={id} files={} grant_root={grant_root:?} reason={reason:?}", changes.len())));
            let mut g = shared.lock().unwrap(); g.approvals.push(ApprovalRequestDetails::Patch { id, file_count: changes.len(), reason, grant_root }); let _=redraw.send(AppEvent::RequestRedraw);
        }
        ExecCommandBegin(ExecCommandBeginEvent { command, .. }) => println!("[exec] {command:?}"),
        ExecCommandEnd(ExecCommandEndEvent { exit_code, stdout, stderr, .. }) => { println!("[exec:done] exit_code={exit_code:?}"); if !stdout.is_empty() { println!("[stdout]\n{stdout}"); } if !stderr.is_empty() { println!("[stderr]\n{stderr}"); } },
        PatchApplyBegin(PatchApplyBeginEvent { auto_approved, changes, .. }) => println!("[patch] applying auto_approved={auto_approved} files={}", changes.len()),
        PatchApplyEnd(PatchApplyEndEvent { success, stdout, stderr, .. }) => { println!("[patch:end] success={success}"); if !stdout.is_empty() { println!("[stdout]\n{stdout}"); } if !stderr.is_empty() { println!("[stderr]\n{stderr}"); } },
        McpToolCallBegin(McpToolCallBeginEvent { server, tool, arguments, .. }) => println!("[mcp] {server}:{tool} args={arguments:?}"),
        McpToolCallEnd(ev) => println!("[mcp:done] success={} result={:?}", ev.is_success(), ev.result),
        SessionConfigured(e) => print_session_configured(&e),
        GetHistoryEntryResponse(_) => {},
        BackgroundEvent(BackgroundEventEvent { message }) => println!("[background] {message}"),
        other => println!("[event] {other:?}"),
    }
    exit_after
}

fn submit_user_message(op_tx: &UnboundedSender<Op>, text: String, images: Vec<std::path::PathBuf>) {
    let mut items: Vec<InputItem> = Vec::new();
    if !text.is_empty() { items.push(InputItem::Text { text: text.clone() }); }
    for path in images { items.push(InputItem::LocalImage { path }); }
    if items.is_empty() { return; }
    let _ = op_tx.send(Op::UserInput { items });
    if !text.is_empty() { let _ = op_tx.send(Op::AddToHistory { text }); }
}

// --- Formatting helpers --------------------------------------------------

fn truncate_str(s: &str, width: usize) -> String {
    if s.chars().count() <= width { return s.to_string(); }
    s.chars().take(width).collect()
}
fn pad_str(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width { truncate_str(s,width) } else { format!("{s}{}", " ".repeat(width-len)) }
}

// --- Styled printing helpers (ANSI) --------------------------------------
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const FG_CYAN: &str = "\x1b[36m";
const FG_GREEN: &str = "\x1b[32m";
const FG_YELLOW: &str = "\x1b[33m";
const FG_MAGENTA: &str = "\x1b[35m";
const FG_RED: &str = "\x1b[31m";

fn color_yellow<S: Into<String>>(s: S) -> String { format!("{FG_YELLOW}{}{RESET}", s.into()) }
fn color_magenta<S: Into<String>>(s: S) -> String { format!("{FG_MAGENTA}{}{RESET}", s.into()) }
fn color_dim<S: Into<String>>(s: S) -> String { format!("{DIM}{}{RESET}", s.into()) }

fn append_history_lines<S: AsRef<str>>(lines: &[S], shared: &Arc<Mutex<SharedState>>) {
    if lines.is_empty() { return; }
    use crossterm::{cursor, terminal, ExecutableCommand};
    let mut stdout = std::io::stdout();
    // Compute region bottom dynamically from current bottom content height.
    if let Ok((_,th)) = terminal::size() {
        if let Ok(g) = shared.lock() {
            let total = g.last_bottom_content_height + RESERVED_INPUT_BUFFER;
            if th > total { let rb = th - total - 1; let _ = stdout.execute(cursor::MoveTo(0, rb)); }
        }
    }
    let redraw_tx_opt = match shared.lock(){ Ok(g)=> g.redraw_tx.clone(), Err(_)=> None };
    for l in lines { println!("{}", l.as_ref()); }
    let _ = stdout.flush();
    if let Some(tx)=redraw_tx_opt { let _=tx.send(AppEvent::RequestRedraw); }
}

fn print_user_message(t: &str, shared: &Arc<Mutex<SharedState>>) { append_history_lines(&[format!("{BOLD}{FG_CYAN}you{RESET}: {t}")], shared); }
fn print_agent_message(t: &str, shared: &Arc<Mutex<SharedState>>) {
    // Print the entire assistant message as a single batch so that a redraw
    // only happens after all lines land in scrollback (avoids losing trailing
    // lines when multiple redraws interleave mid‑message).
    let blocks = format_markdown_blocks("assistant", t, FG_GREEN);
    append_history_lines(&blocks, shared);
}
// Streaming helpers removed – we now buffer deltas and print only the final
// assembled message to avoid noisy per-token rendering and layout jitter.
fn print_reasoning(t: &str, shared: &Arc<Mutex<SharedState>>) { append_history_lines(&[format!("{DIM}{FG_MAGENTA}<reasoning>{RESET} {DIM}{t}{RESET}")], shared); }
fn print_session_configured(e: &SessionConfiguredEvent) { println!("{FG_YELLOW}session{RESET} model={} history_entries={} log_id={}", e.model, e.history_entry_count, e.history_log_id); }

fn format_markdown_blocks(prefix: &str, text: &str, color: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_code = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            out.push(format!("{color}{BOLD}{prefix}{RESET}{color}:{RESET} {}{}```{RESET}", if in_code { BOLD } else { DIM }, if in_code { "" } else { "" }));
            continue;
        }
        if in_code {
            out.push(format!("{color}{DIM}│{RESET} {line}"));
        } else if line.starts_with('#') {
            out.push(format!("{color}{BOLD}{prefix}{RESET}{color}:{RESET} {BOLD}{line}{RESET}"));
        } else {
            out.push(format!("{color}{prefix}{RESET}: {line}"));
        }
    }
    out
}
