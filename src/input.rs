//! Keyboard intents. Crossterm events become these; `app.rs` executes them.

#[derive(Clone, Debug, PartialEq)]
pub enum Intent {
    Up,
    Down,
    Left,
    Right,
    PageDown,
    PageUp,
    First,
    Last,
    FilterCycle,
    ToggleSelect,
    Enter,
    Search,
    Refresh,
    Update,
    DeleteMenu,
    Settings,
    Help,
    Quit,
    Cancel,
    ConfirmYes,
    ConfirmNo,
    Char(char),
    Backspace,
}
