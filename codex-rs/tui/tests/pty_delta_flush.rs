//! PTY test: only deltas, no final messages; flush should occur on TaskComplete.
#![cfg(all(unix, feature = "test-fake-agent"))]

use std::io::Read;
use std::process::{Command, Stdio};
use nix::pty::{openpty, Winsize, OpenptyResult};

#[test]
fn delta_only_message_flushes() {
    let winsize = Winsize { ws_row: 50, ws_col: 100, ws_xpixel: 0, ws_ypixel: 0 };
    let pty: OpenptyResult = openpty(&winsize, None).expect("openpty");
    let exe_dir = std::env::current_exe().unwrap();
    let mut bin_path = exe_dir.parent().unwrap().to_path_buf();
    bin_path.push("codex-tui");
    assert!(bin_path.exists());

    let child = {
        use std::os::fd::FromRawFd;
        let mut cmd = Command::new(&bin_path);
        unsafe {
            cmd.env("CODEX_TUI_FAKE_AGENT", "1")
                .env("CODEX_TUI_FAKE_AGENT_TURNS", "1")
                .env("CODEX_TUI_FAKE_AGENT_OMIT_FINAL", "1")
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
        let mut chunk = [0u8; 4096];
        match master.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(std::time::Duration::from_millis(25)),
            Err(e) => panic!("read error: {e}"),
        }
        let s = String::from_utf8_lossy(&buf);
        if s.contains("[task] complete") { break; }
    }
    let out = String::from_utf8_lossy(&buf);
    assert!(out.contains("assistant: Turn 1 response") || out.contains("assistant: Turn 1 "), "expected flushed assistant content: {out}");
    assert!(out.contains("[task] complete"), "missing task completion line: {out}");
    let _ = child;
}

