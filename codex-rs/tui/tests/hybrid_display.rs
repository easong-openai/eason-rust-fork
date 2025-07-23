use codex_tui::hybrid_sim::{run_script, SimEvent, Simulation};

fn assert_ok(inv: Result<(), String>) { if let Err(e) = inv { panic!("invariant failed: {e}"); } }

#[test]
fn assistant_final_without_deltas() {
    let sim = run_script([
        SimEvent::RedrawBottom,
        SimEvent::UserInput("Hello".into()),
        SimEvent::RedrawBottom,
        SimEvent::AssistantFinal("Hi there".into()),
        SimEvent::RedrawBottom,
    ]);
    let transcript = sim.export();
    let assistant_index = transcript.iter().position(|l| l.contains("assistant:"));
    let bottom_index = transcript.iter().position(|l| l == "--BOTTOM--");
    assert!(assistant_index.is_some() && bottom_index.is_some());
    assert!(assistant_index.unwrap() < bottom_index.unwrap());
    assert_ok(sim.assert_invariants());
}

#[test]
fn assistant_deltas_then_final_flush() {
    let sim = run_script([
        SimEvent::UserInput("Explain".into()),
        SimEvent::AssistantDelta("Part 1".into()),
        SimEvent::RedrawBottom,
        SimEvent::AssistantDelta(" + Part 2".into()),
        SimEvent::RedrawBottom,
        SimEvent::AssistantFinal(" + Done".into()),
        SimEvent::RedrawBottom,
    ]);
    let transcript = sim.export();
    let full_line = transcript.iter().find(|l| l.starts_with("assistant:"));
    assert_eq!(full_line.cloned(), Some("assistant: Part 1 + Part 2 + Done".into()));
    assert_ok(sim.assert_invariants());
}

#[test]
fn reasoning_deltas_then_final_flush() {
    let sim = run_script([
        SimEvent::ReasoningDelta("A".into()),
        SimEvent::RedrawBottom,
        SimEvent::ReasoningDelta("B".into()),
        SimEvent::RedrawBottom,
        SimEvent::ReasoningFinal("C".into()),
        SimEvent::RedrawBottom,
    ]);
    assert!(sim.export().iter().any(|l| l == "<reasoning> ABC"));
    assert_ok(sim.assert_invariants());
}

#[test]
fn multiple_turns_and_interleaving() {
    let sim = run_script([
        SimEvent::UserInput("One".into()),
        SimEvent::AssistantFinal("Resp1".into()),
        SimEvent::UserInput("Two".into()),
        SimEvent::AssistantDelta("R".into()),
        SimEvent::RedrawBottom,
        SimEvent::AssistantFinal("2".into()),
        SimEvent::RedrawBottom,
        SimEvent::AssistantFinal("Third-without-user".into()), // new turn even w/o user
        SimEvent::RedrawBottom,
    ]);
    let transcript = sim.export();
    let assistants: Vec<&String> = transcript.iter().filter(|l| l.starts_with("assistant:")).collect();
    assert_eq!(assistants.len(), 3);
    assert!(assistants[0].contains("Resp1"));
    assert!(assistants[1].contains("R2"));
    assert!(assistants[2].contains("Third-without-user"));
    assert_ok(sim.assert_invariants());
}

#[test]
fn approvals_queue_and_decisions() {
    let sim = run_script([
        SimEvent::ApprovalRequestExec { id: "1".into(), command: vec!["echo".into(), "hi".into()] },
        SimEvent::ApprovalRequestPatch { id: "p1".into(), file_count: 3 },
        SimEvent::RedrawBottom,
        SimEvent::ApprovalRequestExec { id: "2".into(), command: vec!["ls".into()] },
        SimEvent::ApprovalRequestPatch { id: "p2".into(), file_count: 5 },
        SimEvent::RedrawBottom,
        SimEvent::ApprovalDecisionNo,
        SimEvent::RedrawBottom,
        SimEvent::ApprovalDecisionYes,
        SimEvent::RedrawBottom,
        SimEvent::ApprovalDecisionYes, // approve first patch
        SimEvent::RedrawBottom,
        SimEvent::ApprovalDecisionNo, // deny second patch
        SimEvent::RedrawBottom,
    ]);
    let transcript = sim.export();
    let decisions: Vec<&String> = transcript.iter().filter(|l| l.starts_with("[approval] ")).collect();
    assert_eq!(decisions.len(), 4);
    assert!(decisions[0].contains("exec 1 -> Denied"));
    // FIFO queue means after denying exec1, patch p1 is next, not exec2.
    assert!(decisions[1].contains("patch p1 -> Approved"));
    assert!(decisions[2].contains("exec 2 -> Approved"));
    assert!(decisions[3].contains("patch p2 -> Denied"));
    assert_ok(sim.assert_invariants());
}

#[test]
fn token_usage_updates_affect_only_bottom() {
    let mut sim = Simulation::new();
    sim.apply(SimEvent::TokenUsage { input: 10, output: 0, total: 10 });
    sim.apply(SimEvent::RedrawBottom);
    sim.apply(SimEvent::TokenUsage { input: 10, output: 5, total: 15 });
    sim.apply(SimEvent::RedrawBottom);
    let transcript = sim.export();
    // Last bottom snapshot should reflect second update
    let status = transcript.last().unwrap();
    assert!(status.contains("out:5"));
    assert!(!sim.export().iter().any(|l| l.starts_with("tok in:") && l.contains("out:0") && !l.ends_with("approvals:0"))); // ensure no stray history status lines
    assert_ok(sim.assert_invariants());
}

#[test]
fn resize_does_not_change_history() {
    let mut sim = Simulation::new();
    sim.apply(SimEvent::ComposerInsert("hello world".into()));
    sim.apply(SimEvent::ComposerSubmit); // becomes history line
    let before = sim.export();
    // Perform many resizes and redraws
    for w in [10usize, 20, 5, 80, 12, 7, 50] { sim.apply(SimEvent::SetWidth(w)); sim.apply(SimEvent::RedrawBottom); }
    let after = sim.export();
    // History portion (everything before --BOTTOM--) must be identical
    let before_hist: Vec<_> = before.iter().take_while(|l| *l != "--BOTTOM--").cloned().collect();
    let after_hist: Vec<_> = after.iter().take_while(|l| *l != "--BOTTOM--").cloned().collect();
    assert_eq!(before_hist, after_hist);
    assert_ok(sim.assert_invariants());
}

#[test]
fn composer_wrapping_adjusts_bottom_height() {
    let mut sim = Simulation::new();
    sim.apply(SimEvent::ComposerInsert("abcdefghijklmnopqrstuvwxyz".into()));
    sim.apply(SimEvent::SetWidth(30)); // inner width 28
    sim.apply(SimEvent::RedrawBottom);
    let height_wide = sim.bottom_line_count();
    sim.apply(SimEvent::SetWidth(12)); // inner width 10 -> more wrap lines
    sim.apply(SimEvent::RedrawBottom);
    let height_narrow = sim.bottom_line_count();
    assert!(height_narrow > height_wide, "expected more lines after narrowing: wide={height_wide} narrow={height_narrow}");
    assert_ok(sim.assert_invariants());
}

#[test]
fn patch_apply_and_multiline_blocks() {
    let sim = run_script([
        SimEvent::PatchApplyBegin { id: "p1".into(), file_count: 2, auto: true },
        SimEvent::PatchApplyEnd { id: "p1".into(), success: true, stdout: "line1\nline2".into(), stderr: String::new() },
        SimEvent::AppendBlock { header: "[stdout]".into(), body: "extra1\nextra2\nextra3".into() },
        SimEvent::RedrawBottom,
    ]);
    let transcript = sim.export();
    // Ensure ordering and multi-line expansion
    let idx_apply = transcript.iter().position(|l| l.starts_with("[patch] applying" )).unwrap();
    let idx_end = transcript.iter().position(|l| l.starts_with("[patch:end]" )).unwrap();
    assert!(idx_apply < idx_end);
    // Collect stdout block occurrences
    let stdout_headers: Vec<usize> = transcript.iter().enumerate().filter_map(|(i,l)| if l=="[stdout]" {Some(i)} else {None}).collect();
    assert!(stdout_headers.len() >= 2); // one from patch end, one manual append
    for h in stdout_headers { assert!(h+1 < transcript.len(), "stdout header at end"); }
    assert_ok(sim.assert_invariants());
}

// Captures current bug: assistant response printed "on top of" bottom pane
// and then erased on subsequent redraw. This test should FAIL once the bug
// is fixed (we will then adjust expectations accordingly).
#[test]
fn assistant_persists_after_redraw() {
    let mut sim = Simulation::new();
    sim.apply(SimEvent::SetWidth(60));
    sim.apply(SimEvent::RedrawBottom);
    sim.apply(SimEvent::AssistantDelta("Hello ".into()));
    sim.apply(SimEvent::AssistantFinal("there".into()));
    let before = sim.export();
    assert!(before.iter().any(|l| l.starts_with("assistant:")));
    sim.apply(SimEvent::RedrawBottom);
    let after = sim.export();
    let count_before = before.iter().filter(|l| l.starts_with("assistant:" )).count();
    let count_after = after.iter().filter(|l| l.starts_with("assistant:" )).count();
    assert_eq!(count_before, count_after, "assistant line lost after redraw");
}

#[test]
fn multiple_turns_no_merge() {
    let mut sim = Simulation::new();
    sim.apply(SimEvent::UserInput("Hello".into()));
    sim.apply(SimEvent::AssistantFinal("Hi".into()));
    sim.apply(SimEvent::RedrawBottom);
    sim.apply(SimEvent::UserInput("Second".into()));
    sim.apply(SimEvent::AssistantFinal("There".into()));
    sim.apply(SimEvent::RedrawBottom);
    let transcript = sim.export();
    let assistants: Vec<_> = transcript.iter().filter(|l| l.starts_with("assistant:" )).collect();
    assert_eq!(assistants.len(), 2, "expected two distinct assistant lines got: {assistants:?}");
    assert!(assistants[0].contains("Hi"));
    assert!(assistants[1].contains("There"));
}

#[test]
fn reasoning_then_message_preserved() {
    let mut sim = Simulation::new();
    // Turn 1
    sim.apply(SimEvent::ReasoningDelta("think1".into()));
    sim.apply(SimEvent::ReasoningFinal(" done1".into()));
    sim.apply(SimEvent::AssistantDelta("Answer".into()));
    sim.apply(SimEvent::AssistantFinal(" One".into()));
    sim.apply(SimEvent::RedrawBottom);
    // Turn 2
    sim.apply(SimEvent::ReasoningDelta("think2".into()));
    sim.apply(SimEvent::ReasoningFinal(" done2".into()));
    sim.apply(SimEvent::AssistantFinal("Second".into()));
    sim.apply(SimEvent::RedrawBottom);
    let transcript = sim.export();
    let answers: Vec<_> = transcript.iter().filter(|l| l.starts_with("assistant:" )).collect();
    assert_eq!(answers.len(), 2, "expected two assistant lines, got {answers:?}");
    assert!(answers[0].contains("Answer One"));
    assert!(answers[1].contains("Second"));
}

#[test]
fn delta_only_flushed_on_task_complete() {
    let mut sim = Simulation::new();
    sim.apply(SimEvent::AssistantDelta("Partial".into()));
    sim.apply(SimEvent::RedrawBottom);
    // no final, simulate task complete
    sim.apply(SimEvent::TaskComplete);
    let transcript = sim.export();
    assert!(transcript.iter().any(|l| l == "assistant: Partial"), "delta content not flushed on task complete: {transcript:?}");
    assert!(transcript.iter().any(|l| l == "[task] complete"));
}
