# Rust TUI Application Development Best Practices

Guidelines for architecting robust, responsive, and panic-safe Terminal User Interfaces in Rust using standard frameworks like `ratatui`.

## 1. Architectural Patterns

### The Elm Architecture (TEA)
* **Model**: Keep all state in a single, centralized struct (e.g., input buffers, active tabs).
* **View**: Implement rendering logic as pure functions that accept state references and draw frames. Do not modify state during rendering.
* **Update**: Centralize logic in an `update(&mut self, action: Action)` method driven by explicit command enums.

### Component Decentralization
* **Atomic Components**: Divide UI sections into independent sub-components using a trait (e.g., `Component`).
* **Trait Methods**: Include `handle_events`, `update`, and `render(frame, area)` on components.
* **State Isolation**: Keep child component data self-contained; do not bloat the main application loop.

---

## 2. Event Handling & Async I/O

### Async Decoupling
* **Non-Blocking Main Loop**: Keep heavy I/O (API calls, disk scanning, database queries) out of the main thread.
* **Event Bus**: Establish an async event loop (via `tokio` or `crossbeam-channel`).
* **MPSC Channels**: Pipe keystrokes and background job results to the main thread as unified enums.

### Declarative Inputs
* **Key Mapping**: Avoid heavily nested `match` statements for keyboard events.
* **Abstraction**: Use dedicated management crates (e.g., `tui-input`) or map actions to configurations explicitly.

---

## 3. Error Handling & Lifecycle

### Panic Mitigation
* **Terminal Corruption**: Raw terminal state must be reversed on sudden aborts to avoid ruining the user's shell window.
* **Custom Hooks**: Bind terminal restoration steps to custom panic handlers using `color-eyre` or `std::panic::set_hook`.

```rust
// Basic Panic Hook Pattern
std::panic::set_hook(Box::new(|panic_info| {
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
    eprintln!("Application Panicked: {panic_info}");
}));
```

### RAII Guards
* **Scope-based Cleanup**: Wrap terminal setup inside a struct with a dedicated `Drop` implementation to safely unwind raw terminal settings automatically.

---

## 4. UI Layout & UX Strategy

### Relative Layouts
* **No Hardcoded Dimensions**: Calculate UI zones based on dynamic terminal sizes.
* **Constraints**: Leverage layout models like `Constraint::Percentage` or `Constraint::Min/Max`.
* **Resize Resilience**: Explicitly catch terminal resize signals to force canvas updates immediately.

### Theme & Styling
* **Decoupled Esthetics**: Isolate standard color sets, borders, and margins into a modular `Theme` structure.

---

## 5. Development Utilities

### Diagnostic Logging
* **No `println!`**: Writing directly to stdout crashes the active layout grid.
* **File-Based Trailing**: Redirect application logs out to text logs utilizing the `tracing` or `log` crates.

### Stateful Objects
* **Cursor Continuity**: Use stateful widgets (`ListState`, `TableState`) to maintain position consistency through frames.
