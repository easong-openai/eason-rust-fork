//! Hybrid UI simulation harness.
//!
//! This module provides a pure, side‑effect free model of the append‑only
//! terminal behaviour implemented in `hybrid_mode.rs`. It intentionally
//! ignores ANSI colour / cursor math details and focuses on the semantic
//! guarantees we care about during refactors:
//!
//! * Assistant & reasoning deltas are buffered until their corresponding
//!   final events – exactly one history line is appended per final.
//! * Redrawing the bottom pane never mutates or removes prior history.
//! * Approval requests enqueue; approval decisions append a single line and
//!   dequeue the request in FIFO order.
//! * Token usage updates only affect the bottom pane snapshot.
//!
//! The simulator exposes a small event language (`SimEvent`). Higher level
//! tests can generate long, adversarial sequences (e.g. redraw after every
//! delta) to ensure invariants hold. This gives us confidence to restructure
//! the real terminal code while keeping user‑visible behaviour stable.

#[derive(Debug, Clone)]
pub enum SimEvent {
    UserInput(String),
    AssistantDelta(String),
    AssistantFinal(String),
    ReasoningDelta(String),
    ReasoningFinal(String),
    ApprovalRequestExec { id: String, command: Vec<String> },
    ApprovalRequestPatch { id: String, file_count: usize },
    ApprovalDecisionYes, // acts on the front of the queue
    ApprovalDecisionNo,
    TokenUsage { input: u32, output: u32, total: u32 },
    ComposerInsert(String),
    ComposerSubmit,
    SetWidth(usize),
    PatchApplyBegin { id: String, file_count: usize, auto: bool },
    PatchApplyEnd { id: String, success: bool, stdout: String, stderr: String },
    AppendBlock { header: String, body: String },
    // Simulate current bug: assistant printed inside bottom pane area and
    // lost on next redraw. We model this separately so we can lock in a
    // regression test before fixing the real code.
    BuggyAssistantFinal(String),
    TaskComplete,
    RedrawBottom,
}

#[derive(Debug, Default)]
pub struct Simulation {
    history: Vec<String>,
    bottom: Vec<String>,
    answer_buf: String,
    reasoning_buf: String,
    pending_approvals: Vec<ApprovalRequest>,
    answered_turns: Vec<String>, // canonical assistant turn payloads
    reasoning_turns: Vec<String>,
    token_usage: (u32, u32, u32),
    composer_content: String,
    width: usize,
}

#[derive(Debug, Clone)]
enum ApprovalRequest { Exec { id: String, command: Vec<String> }, Patch { id: String, file_count: usize } }

impl Simulation {
    pub fn new() -> Self { Self { width: 40, ..Default::default() } }

    pub fn apply(&mut self, ev: SimEvent) { self.apply_internal(ev); }

    fn apply_internal(&mut self, ev: SimEvent) {
        match ev {
            SimEvent::UserInput(t) => self.push_history(format!("you: {t}")),
            SimEvent::AssistantDelta(d) => self.answer_buf.push_str(&d),
            SimEvent::AssistantFinal(msg) => {
                if !msg.is_empty() { self.answer_buf.push_str(&msg); }
                let out = std::mem::take(&mut self.answer_buf);
                self.answered_turns.push(out.clone());
                self.push_history(format!("assistant: {out}"));
            }
            SimEvent::ReasoningDelta(d) => self.reasoning_buf.push_str(&d),
            SimEvent::ReasoningFinal(text) => {
                if !text.is_empty() { self.reasoning_buf.push_str(&text); }
                let out = std::mem::take(&mut self.reasoning_buf);
                self.reasoning_turns.push(out.clone());
                self.push_history(format!("<reasoning> {out}"));
            }
            SimEvent::ApprovalRequestExec { id, command } => {
                self.pending_approvals.push(ApprovalRequest::Exec { id: id.clone(), command: command.clone() });
                self.push_history(format!("[approval:exec] id={id} cmd={:?}", command));
            }
            SimEvent::ApprovalRequestPatch { id, file_count } => {
                self.pending_approvals.push(ApprovalRequest::Patch { id: id.clone(), file_count });
                self.push_history(format!("[approval:patch] id={id} files={file_count}"));
            }
            SimEvent::ApprovalDecisionYes | SimEvent::ApprovalDecisionNo => {
                if let Some(req) = self.pending_approvals.first() {
                    let (id, kind) = match req { ApprovalRequest::Exec { id, .. } => (id.clone(), "exec"), ApprovalRequest::Patch { id, .. } => (id.clone(), "patch") };
                    let decision = matches!(ev, SimEvent::ApprovalDecisionYes);
                    self.pending_approvals.remove(0);
                    self.push_history(format!("[approval] {kind} {id} -> {}", if decision { "Approved" } else { "Denied" }));
                }
            }
            SimEvent::TokenUsage { input, output, total } => {
                self.token_usage = (input, output, total);
            }
            SimEvent::ComposerInsert(s) => { self.composer_content.push_str(&s); }
            SimEvent::ComposerSubmit => {
                if !self.composer_content.is_empty() {
                    let submitted = std::mem::take(&mut self.composer_content);
                    self.push_history(format!("you: {submitted}"));
                }
            }
            SimEvent::SetWidth(w) => { self.width = w.max(4); }
            SimEvent::PatchApplyBegin { id, file_count, auto } => { self.push_history(format!("[patch] applying id={id} files={file_count} auto_approved={auto}")); }
            SimEvent::PatchApplyEnd { id, success, stdout, stderr } => {
                self.push_history(format!("[patch:end] id={id} success={success}"));
                if !stdout.is_empty() { self.append_block("[stdout]".into(), stdout); }
                if !stderr.is_empty() { self.append_block("[stderr]".into(), stderr); }
            }
            SimEvent::AppendBlock { header, body } => { self.append_block(header, body); }
            SimEvent::BuggyAssistantFinal(text) => {
                // Visible until next redraw, then lost.
                self.history.push(format!("assistant(ephemeral): {text}"));
            }
            SimEvent::TaskComplete => {
                if !self.answer_buf.is_empty() { let full = std::mem::take(&mut self.answer_buf); self.answered_turns.push(full.clone()); self.push_history(format!("assistant: {full}")); }
                if !self.reasoning_buf.is_empty() { let full = std::mem::take(&mut self.reasoning_buf); self.reasoning_turns.push(full.clone()); self.push_history(format!("<reasoning> {full}")); }
                self.push_history("[task] complete".into());
            }
            SimEvent::RedrawBottom => self.redraw_bottom(),
        }
    }

    fn redraw_bottom(&mut self) {
        let (inp, out, total) = self.token_usage;
        let approvals = self.pending_approvals.len();
        let status = format!("tok in:{inp} out:{out} total:{total} approvals:{approvals}");
        let w = self.width;
        let inner_w = w.saturating_sub(2);
        // Wrap composer content to inner width.
        let mut wrapped: Vec<String> = Vec::new();
        if self.composer_content.is_empty() {
            wrapped.push(String::new());
        } else {
            let mut current = String::new();
            for ch in self.composer_content.chars() {
                current.push(ch);
                if current.chars().count() >= inner_w {
                    wrapped.push(current);
                    current = String::new();
                }
            }
            if !current.is_empty() { wrapped.push(current); }
        }
        let top = format!("┌{}┐", "─".repeat(inner_w));
        let bottom = format!("└{}┘", "─".repeat(inner_w));
        let mut out_lines = vec![top];
        for l in wrapped { out_lines.push(format!("│{}│", pad_to_width(&l, inner_w))); }
        out_lines.push(bottom);
        out_lines.push(status);
        self.bottom = out_lines;
    }

    fn push_history(&mut self, line: String) { self.history.push(line); }
    fn append_block(&mut self, header: String, body: String) { self.push_history(header); for l in body.lines() { self.push_history(l.to_string()); } }

    pub fn export(&self) -> Vec<String> {
        let mut out = self.history.clone();
        out.push("--BOTTOM--".into());
        out.extend(self.bottom.clone());
        out
    }
    pub fn bottom_line_count(&self) -> usize { self.bottom.len() }

    /// Validate core invariants. Returns `Ok(())` if all hold or a textual
    /// description of the first violation.
    pub fn assert_invariants(&self) -> Result<(), String> {
        // Assistant final count invariants
        let printed_assistant: Vec<&String> = self.history.iter().filter(|l| l.starts_with("assistant:" )).collect();
        if printed_assistant.len() != self.answered_turns.len() { return Err(format!("assistant turn count mismatch printed={} recorded={}" , printed_assistant.len(), self.answered_turns.len())); }
        for (i, (expected, printed)) in self.answered_turns.iter().zip(printed_assistant.iter()).enumerate() {
            let p = printed.strip_prefix("assistant: ").unwrap_or(printed);
            if p != expected { return Err(format!("assistant turn {i} payload mismatch expected='{expected}' got='{p}'")); }
        }
        // NOTE: we intentionally do not include ephemeral assistant lines
        // (assistant(ephemeral): ...) in invariants; the dedicated test
        // asserts their disappearance after redraw to capture the real bug.
        // Reasoning
        let printed_reasoning: Vec<&String> = self.history.iter().filter(|l| l.starts_with("<reasoning> ")).collect();
        if printed_reasoning.len() != self.reasoning_turns.len() { return Err("reasoning turn count mismatch".into()); }
        for (i, (expected, printed)) in self.reasoning_turns.iter().zip(printed_reasoning.iter()).enumerate() {
            let p = printed.strip_prefix("<reasoning> ").unwrap_or(printed);
            if p != expected { return Err(format!("reasoning turn {i} payload mismatch expected='{expected}' got='{p}'")); }
        }
        // Bottom does not leak into history
        if self.history.iter().any(|l| l.starts_with("┌") || l == "--BOTTOM--") { return Err("bottom artefacts leaked into history".into()); }
        for (i, line) in self.history.iter().enumerate() { if (line == "[stdout]" || line == "[stderr]") && self.history.get(i+1).map(|n| n.starts_with('[')).unwrap_or(true) { return Err("stdout/stderr header not followed by content".into()); } }
        Ok(())
    }
}

/// Convenience – drive a simulation with a script.
pub fn run_script(events: impl IntoIterator<Item = SimEvent>) -> Simulation {
    let mut sim = Simulation::new();
    for ev in events { sim.apply(ev); }
    sim
}

fn pad_to_width(s: &str, width: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    for ch in s.chars() { if count >= width { break; } out.push(ch); count += 1; }
    if count < width { out.push_str(&" ".repeat(width - count)); }
    out
}
