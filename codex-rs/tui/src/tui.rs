use std::io::Result;
use std::io::Stdout;
use std::io::stdout;

use codex_core::config::Config;
use crossterm::event::DisableBracketedPaste;
use crossterm::event::DisableMouseCapture;
use crossterm::event::EnableBracketedPaste;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::disable_raw_mode;
use ratatui::crossterm::terminal::enable_raw_mode;
use ratatui::TerminalOptions;
use ratatui::Viewport;

use crate::mouse_capture::MouseCapture;

/// A type alias for the terminal type used in this application
pub type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Initialize the terminal
pub fn init(config: &Config) -> Result<(Tui, MouseCapture)> {
    // We intentionally do NOT enter the alternate screen: the new hybrid UI
    // model keeps history in the native terminal scrollback so users get
    // natural scrolling + text selection. We still enable bracketed paste so
    // multi‑line pastes arrive as a single event.
    execute!(stdout(), EnableBracketedPaste)?;
    let mouse_capture = MouseCapture::new_with_capture(!config.tui.disable_mouse_capture)?;

    enable_raw_mode()?;
    set_panic_hook();

    // Reserve a fixed number of lines for the interactive viewport (composer,
    // status, popups). History is injected above using `insert_before`. This
    // is an initial step of the refactor – later the height can become
    // dynamic. For now a conservative default keeps enough room for the
    // multi‑line composer while not occupying the whole screen.
    const BOTTOM_VIEWPORT_HEIGHT: u16 = 8;
    let backend = CrosstermBackend::new(stdout());
    let tui = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(BOTTOM_VIEWPORT_HEIGHT),
        },
    )?;
    Ok((tui, mouse_capture))
}

fn set_panic_hook() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = restore(); // ignore any errors as we are already failing
        hook(panic_info);
    }));
}

/// Restore the terminal to its original state
pub fn restore() -> Result<()> {
    // We are shutting down, and we cannot reference the `MouseCapture`, so we
    // categorically disable mouse capture just to be safe.
    if execute!(stdout(), DisableMouseCapture).is_err() {
        // It is possible that `DisableMouseCapture` is written more than once
        // on shutdown, so ignore the error in this case.
    }
    execute!(stdout(), DisableBracketedPaste)?;
    disable_raw_mode()?;
    Ok(())
}
