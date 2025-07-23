# Transition Plan: From Custom Scrollback to Ratatui's insert_before

## Current State Analysis

The current TUI implementation uses a sophisticated custom scrollback system with these key components:

- **ConversationHistoryWidget**: Custom scrolling with cached line counts and "stick-to-bottom" behavior
- **Manual viewport management**: Complex scroll position tracking with `usize::MAX` for auto-scroll
- **Custom rendering**: Windowed rendering of visible content with partial buffer updates
- **Focus-based navigation**: Tab-toggle between history pane and input with distinct visual styles

**Current files involved:**

- `conversation_history_widget.rs` (main scrollback logic)
- `history_cell.rs` (message type definitions)
- `cell_widget.rs` (rendering interface)
- `scroll_event_helper.rs` (mouse scroll debouncing)
- `chatwidget.rs` (layout orchestration)

## Ratatui's insert_before Method

Based on research, `insert_before` is a terminal-level method available in ratatui 0.29.0+ with the `scrolling-regions` feature flag. This method:

- Uses terminal scrolling regions to reduce flickering
- Allows inserting content at specific positions without full redraws
- Provides native terminal scrolling behavior
- Should be more performant than custom buffer management

**Feature flag required:**

```toml
ratatui = { version = "0.29.0", features = ["scrolling-regions"] }
```

## Transition Strategy

### Phase 1: Foundation Setup

1. **Enable scrolling-regions feature** in `Cargo.toml`
2. **Research insert_before API** through source code or experimentation
3. **Create proof-of-concept** with simple message insertion
4. **Benchmark performance** against current implementation

### Phase 2: Core Migration

1. **Simplify ConversationHistoryWidget**:

   - Remove custom scroll position tracking
   - Remove cached line counting (let terminal handle)
   - Remove windowed rendering logic
   - Keep message storage and type definitions

2. **Leverage native terminal scrolling**:

   - Use `insert_before` for new messages
   - Let terminal handle viewport management
   - Remove manual scroll calculations

3. **Maintain message types**:
   - Keep `HistoryCell` enum structure
   - Keep `CellWidget` trait for rendering consistency
   - Adapt rendering to work with terminal scrolling

### Phase 3: UX Enhancement

1. **Preserve navigation controls**:

   - Keep Tab focus toggle
   - Map j/k, ↑/↓ to terminal scroll commands
   - Keep PageUp/PageDown behavior
   - Maintain mouse scroll support

2. **Improve auto-scroll behavior**:

   - Use terminal's native "stick-to-bottom"
   - Simplify new message insertion
   - Remove complex scroll position management

3. **Maintain visual feedback**:
   - Keep focus-dependent styling
   - Keep scrollbar if needed
   - Keep title updates based on focus

## Expected Benefits

### Performance Improvements

- **Reduced CPU usage**: No manual line counting or viewport calculations
- **Less flickering**: Native terminal scrolling regions
- **Simpler rendering**: Terminal handles content positioning
- **Better memory efficiency**: No need to cache line counts

### Code Simplification

- **Remove ~200 lines** from ConversationHistoryWidget
- **Eliminate scroll_event_helper.rs** (use terminal events)
- **Simplify chatwidget.rs** layout logic
- **Reduce state management complexity**

### Enhanced UX

- **Smoother scrolling**: Native terminal performance
- **Better responsiveness**: Reduced input lag
- **More reliable behavior**: Fewer edge cases in scroll logic
- **Consistent with terminal expectations**: Standard scroll behavior

## Implementation Steps

### Step 1: Minimal Viable Transition

```rust
// New simplified ConversationHistoryWidget
pub struct ConversationHistoryWidget {
    entries: Vec<Entry>,
    terminal: &mut Terminal<impl Backend>,  // Direct terminal access
    has_input_focus: bool,
}

impl ConversationHistoryWidget {
    pub fn add_message(&mut self, cell: HistoryCell) {
        let entry = Entry { cell };
        self.entries.push(entry);

        // Use insert_before instead of custom rendering
        self.terminal.insert_before(/* render entry */)?;
    }

    pub fn handle_key_event(&mut self, key_event: KeyEvent) -> bool {
        match key_event.code {
            KeyCode::Up => self.terminal.scroll_up(1),
            KeyCode::Down => self.terminal.scroll_down(1),
            // Map other keys to terminal scroll commands
            _ => false,
        }
    }
}
```

### Step 2: Preserve Advanced Features

- Maintain message type diversity (images, patches, errors)
- Keep focus management and visual styling
- Preserve keyboard shortcuts and mouse support
- Ensure compatibility with existing chat flow

### Step 3: Testing and Refinement

- A/B test performance against current implementation
- Verify all message types render correctly
- Test edge cases (very long messages, rapid updates)
- Ensure backwards compatibility with existing saved conversations

## Risk Mitigation

### Potential Issues

1. **insert_before API limitations**: May not support complex rendering needs
2. **Feature flag stability**: `scrolling-regions` is likely experimental
3. **Terminal compatibility**: May not work on all terminal emulators
4. **Message ordering**: Need to ensure proper insertion order

### Mitigation Strategies

1. **Gradual rollout**: Keep current implementation as fallback
2. **Feature detection**: Check terminal capabilities at runtime
3. **Comprehensive testing**: Test across different terminals and platforms
4. **Performance monitoring**: Compare before/after metrics

## Success Metrics

- **Performance**: 30%+ reduction in CPU usage during scrolling
- **Code reduction**: Remove 200+ lines of custom scroll logic
- **UX improvement**: Smoother scrolling with less input lag
- **Maintainability**: Fewer bug reports related to scroll behavior
- **Compatibility**: Works on 95%+ of supported terminal emulators

## Timeline

- **Week 1**: Research and proof-of-concept
- **Week 2**: Core migration and basic functionality
- **Week 3**: UX preservation and advanced features
- **Week 4**: Testing, refinement, and performance validation

This transition should result in a cleaner, more performant, and more maintainable chat interface while preserving the excellent UX that users expect from the Codex TUI.
