//! Widget state definitions.

/// The UI state of a widget/shape.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum WidgetState {
    /// Normal display state - no interaction.
    #[default]
    Normal,
    /// Mouse is hovering over the widget.
    Hovered,
    /// Widget is selected (shows handles, can be moved/resized).
    Selected,
    /// Widget is in editing mode (e.g., text editing).
    Editing(EditingKind),
}

impl WidgetState {
    /// Check if widget is selected (either just selected or editing).
    pub fn is_selected(&self) -> bool {
        matches!(self, Self::Selected | Self::Editing(_))
    }

    /// Check if widget is in editing mode.
    pub fn is_editing(&self) -> bool {
        matches!(self, Self::Editing(_))
    }
}

/// Kind of editing mode.
#[derive(Debug, Clone, PartialEq)]
pub enum EditingKind {
    /// Text editing mode - cursor position, selection, etc.
    Text,
    /// Path editing mode - for freehand shapes.
    Path,
}
