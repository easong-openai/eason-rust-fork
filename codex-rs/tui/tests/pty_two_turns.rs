//! PTY test ensuring two sequential assistant turns both appear.
#![cfg(all(unix, feature = "test-fake-agent"))]

use std::io::Read;
use std::process::{Command, Stdio};
use nix::pty::{openpty, Winsize, OpenptyResult};

#[test]
fn two_turns_persist() {
    let winsize = Winsize { ws_row: 60, ws_col: 120, ws_xpixel: 0, ws_ypixel: 0 };
    let pty: OpenptyResult = openpty(&winsize, None).expect("openpty");

    let exe_dir = std::env::current_exe().unwrap();
    let mut bin_path = exe_dir.parent().unwrap().to_path_buf();
    bin_path.push("codex-tui");
    assert!(bin_path.exists(), "codex-tui binary missing at {:?}", bin_path);

    let child = {
        use std::os::fd::FromRawFd;
        let mut cmd = Command::new(&bin_path);
        unsafe {
            cmd.env("CODEX_TUI_FAKE_AGENT", "1")
                .env("CODEX_TUI_FAKE_AGENT_TURNS", "2")
                // no --auto-exit so both turns are emitted
                .stdin(Stdio::from_raw_fd(pty.slave.as_raw_fd()))
                .stdout(Stdio::from_raw_fd(pty.slave.as_raw_fd()))
                .stderr(Stdio::null());
        }
        cmd.spawn().expect("spawn child")
    };

    let mut master = pty.master;
    master.set_nonblocking(true).ok();
    let start = std::time::Instant::now();
    let mut buf = Vec::new();
    while start.elapsed() < std::time::Duration::from_secs(5) {
        let mut chunk = [0u8; 8192];
        match master.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(std::time::Duration::from_millis(20)),
            Err(e) => panic!("pty read error: {e}"),
        }
        // Stop early if both assistant lines observed
        let s = String::from_utf8_lossy(&buf);
        if s.contains("assistant: Turn 1 response") && s.contains("assistant: Turn 2 response") && s.contains("thinking 1") && s.contains("thinking 2") { break; }
    }
    let out = String::from_utf8_lossy(&buf);

    // Simple ANSI stripper (only colour sequences \x1b[..m)
    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i+1] == b'[' {
                i += 2;
                while i < bytes.len() && bytes[i] != b'm' { i += 1; }
                if i < bytes.len() { i += 1; }
            } else {
                out.push(bytes[i] as char);
                i += 1;
            }
        }
        out
    }
    let clean = strip_ansi(&out);
    assert!(clean.contains("assistant: Turn 1 response"), "first turn missing: {clean}");
    assert!(clean.contains("assistant: Turn 2 response"), "second turn missing: {clean}");
    assert!(clean.contains("<reasoning> thinking 1 done 1"), "reasoning for turn1 missing: {clean}");
    assert!(clean.contains("<reasoning> thinking 2 done 2"), "reasoning for turn2 missing: {clean}");
    let _ = child; // allow drop
}
