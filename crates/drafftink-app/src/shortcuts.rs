//! Keyboard shortcut registry and documentation.

/// A keyboard shortcut definition.
#[derive(Debug, Clone)]
pub struct Shortcut {
    pub key: &'static str,
    pub ctrl: bool,
    pub shift: bool,
    pub description: &'static str,
}

impl Shortcut {
    pub const fn new(
        key: &'static str,
        ctrl: bool,
        shift: bool,
        description: &'static str,
    ) -> Self {
        Self {
            key,
            ctrl,
            shift,
            description,
        }
    }

    /// Format the shortcut for display (e.g., "Ctrl+S").
    pub fn format(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.shift {
            parts.push("Shift");
        }
        parts.push(self.key);
        parts.join("+")
    }
}

/// Registry of all keyboard shortcuts.
pub struct ShortcutRegistry;

impl ShortcutRegistry {
    /// Get all registered shortcuts.
    pub fn all() -> Vec<Shortcut> {
        vec![
            Shortcut::new("A", true, false, "Select all shapes"),
            Shortcut::new("S", true, false, "Save Local"),
            Shortcut::new("O", true, false, "Open..."),
            Shortcut::new("E", true, false, "Export to PNG"),
            Shortcut::new("Z", true, false, "Undo"),
            Shortcut::new("Z", true, true, "Redo"),
            Shortcut::new("Y", true, false, "Redo"),
            Shortcut::new("G", true, false, "Group selected shapes"),
            Shortcut::new("G", true, true, "Ungroup selected shapes"),
            Shortcut::new("C", true, true, "Copy selection as PNG"),
            Shortcut::new("C", true, false, "Copy shapes"),
            Shortcut::new("X", true, false, "Cut shapes"),
            Shortcut::new("V", true, false, "Paste shapes or image"),
            Shortcut::new("Delete", false, false, "Delete selected shapes"),
            Shortcut::new("Backspace", false, false, "Delete selected shapes"),
            Shortcut::new("Escape", false, false, "Cancel current action"),
            Shortcut::new(
                "Shift+Drag",
                false,
                false,
                "Maintain aspect ratio while resizing",
            ),
        ]
    }

    /// Print all shortcuts to console.
    pub fn print_all() {
        println!("\n=== Keyboard Shortcuts ===");
        for shortcut in Self::all() {
            println!("  {:20} {}", shortcut.format(), shortcut.description);
        }
        println!();
    }
}
