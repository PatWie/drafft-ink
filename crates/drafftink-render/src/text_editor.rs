//! Text editing state using Parley's PlainEditor.

use parley::editing::{Generation, PlainEditor, PlainEditorDriver};
use parley::{FontContext, LayoutContext, StyleProperty};
use peniko::Brush;
use std::time::Duration;

// Use web_time for WASM compatibility
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
#[cfg(target_arch = "wasm32")]
use web_time::Instant;

/// Keyboard key for text editing.
#[derive(Debug, Clone, PartialEq)]
pub enum TextKey {
    Character(String),
    Backspace,
    Delete,
    Enter,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Escape,
    Copy,
    Cut,
    Paste(String),
}

/// Keyboard modifiers.
#[derive(Debug, Clone, Copy, Default)]
pub struct TextModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

impl TextModifiers {
    /// Get the action modifier (Ctrl on Windows/Linux, Cmd on macOS).
    pub fn action_mod(&self) -> bool {
        if cfg!(target_os = "macos") {
            self.meta
        } else {
            self.ctrl
        }
    }
}

/// Result of handling a text editing event.
#[derive(Debug, Clone, PartialEq)]
pub enum TextEditResult {
    /// Event was handled, text may have changed.
    Handled,
    /// Event was handled, user wants to exit editing.
    ExitEdit,
    /// Event was not handled (pass to other handlers).
    NotHandled,
    /// Copy requested - contains the selected text to copy.
    Copy(String),
}

/// Text editor state for a single text shape being edited.
pub struct TextEditState {
    /// The Parley PlainEditor for handling text editing.
    editor: PlainEditor<Brush>,
    /// Whether the cursor is currently visible (for blinking).
    cursor_visible: bool,
    /// Start time for cursor blinking.
    start_time: Option<Instant>,
    /// Blink period.
    blink_period: Duration,
    /// Whether a mouse drag is in progress for selection.
    is_dragging: bool,
    /// Cached layout width for bounds calculation.
    cached_width: f32,
    /// Cached layout height for bounds calculation.
    cached_height: f32,
}

impl TextEditState {
    /// Create a new text edit state with the given text content.
    pub fn new(text: &str, font_size: f32) -> Self {
        use parley::GenericFamily;

        let mut editor = PlainEditor::new(font_size);
        editor.set_text(text);
        editor.set_scale(1.0);

        // Set default styles - use SansSerif generic family
        // The renderer will set the specific font (GelPen) via styles
        let styles = editor.edit_styles();
        styles.insert(GenericFamily::SansSerif.into());
        styles.insert(StyleProperty::Brush(Brush::Solid(peniko::Color::BLACK)));

        Self {
            editor,
            cursor_visible: true,
            start_time: None,
            blink_period: Duration::ZERO,
            is_dragging: false,
            cached_width: 0.0,
            cached_height: 0.0,
        }
    }

    /// Get a mutable reference to the PlainEditor.
    pub fn editor_mut(&mut self) -> &mut PlainEditor<Brush> {
        &mut self.editor
    }

    /// Get a reference to the PlainEditor.
    pub fn editor(&self) -> &PlainEditor<Brush> {
        &self.editor
    }

    /// Create a driver for performing edit operations.
    pub fn driver<'a>(
        &'a mut self,
        font_cx: &'a mut FontContext,
        layout_cx: &'a mut LayoutContext<Brush>,
    ) -> PlainEditorDriver<'a, Brush> {
        self.editor.driver(font_cx, layout_cx)
    }

    /// Get the current text content.
    pub fn text(&self) -> String {
        self.editor.text().to_string()
    }

    /// Set the text content.
    pub fn set_text(&mut self, text: &str) {
        self.editor.set_text(text);
    }

    /// Set the text brush color.
    pub fn set_brush(&mut self, brush: Brush) {
        let styles = self.editor.edit_styles();
        styles.insert(StyleProperty::Brush(brush));
    }

    /// Set the font size.
    pub fn set_font_size(&mut self, size: f32) {
        let styles = self.editor.edit_styles();
        styles.insert(StyleProperty::FontSize(size));
    }

    /// Set the text width constraint.
    pub fn set_width(&mut self, width: Option<f32>) {
        self.editor.set_width(width);
    }

    /// Reset cursor to visible state and start blinking.
    pub fn cursor_reset(&mut self) {
        self.start_time = Some(Instant::now());
        self.blink_period = Duration::from_millis(500);
        self.cursor_visible = true;
    }

    /// Disable cursor blinking.
    pub fn disable_blink(&mut self) {
        self.start_time = None;
    }

    /// Calculate the next blink time.
    pub fn next_blink_time(&self) -> Option<Instant> {
        self.start_time.map(|start_time| {
            let phase = Instant::now().duration_since(start_time);
            start_time
                + Duration::from_nanos(
                    ((phase.as_nanos() / self.blink_period.as_nanos() + 1)
                        * self.blink_period.as_nanos()) as u64,
                )
        })
    }

    /// Update cursor visibility based on blink state.
    pub fn cursor_blink(&mut self) {
        self.cursor_visible = self.start_time.is_some_and(|start_time| {
            let elapsed = Instant::now().duration_since(start_time);
            (elapsed.as_millis() / self.blink_period.as_millis()) % 2 == 0
        });
    }

    /// Check if cursor should be visible.
    pub fn is_cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    /// Get the current generation (for change detection).
    pub fn generation(&self) -> Generation {
        self.editor.generation()
    }

    /// Check if the editor is composing (IME active).
    pub fn is_composing(&self) -> bool {
        self.editor.is_composing()
    }

    /// Get cached layout dimensions.
    pub fn layout_size(&self) -> (f32, f32) {
        (self.cached_width, self.cached_height)
    }

    /// Update cached layout dimensions from the current layout.
    pub fn update_layout_cache(
        &mut self,
        font_cx: &mut FontContext,
        layout_cx: &mut LayoutContext<Brush>,
    ) {
        let layout = self.editor.layout(font_cx, layout_cx);
        self.cached_width = layout.width();
        self.cached_height = layout.height();
    }

    /// Handle a key press event.
    /// Returns whether the event was handled and if editing should exit.
    pub fn handle_key(
        &mut self,
        key: TextKey,
        modifiers: TextModifiers,
        font_cx: &mut FontContext,
        layout_cx: &mut LayoutContext<Brush>,
    ) -> TextEditResult {
        // Don't process keys while composing (IME)
        if self.editor.is_composing() {
            return TextEditResult::NotHandled;
        }

        self.cursor_reset();
        let action_mod = modifiers.action_mod();
        let shift = modifiers.shift;

        let mut drv = self.editor.driver(font_cx, layout_cx);

        match key {
            TextKey::Escape => {
                return TextEditResult::ExitEdit;
            }
            TextKey::Backspace => {
                if action_mod {
                    drv.backdelete_word();
                } else {
                    drv.backdelete();
                }
            }
            TextKey::Delete => {
                if action_mod {
                    drv.delete_word();
                } else {
                    drv.delete();
                }
            }
            TextKey::Enter => {
                drv.insert_or_replace_selection("\n");
            }
            TextKey::Left => {
                if action_mod {
                    if shift {
                        drv.select_word_left();
                    } else {
                        drv.move_word_left();
                    }
                } else if shift {
                    drv.select_left();
                } else {
                    drv.move_left();
                }
            }
            TextKey::Right => {
                if action_mod {
                    if shift {
                        drv.select_word_right();
                    } else {
                        drv.move_word_right();
                    }
                } else if shift {
                    drv.select_right();
                } else {
                    drv.move_right();
                }
            }
            TextKey::Up => {
                if shift {
                    drv.select_up();
                } else {
                    drv.move_up();
                }
            }
            TextKey::Down => {
                if shift {
                    drv.select_down();
                } else {
                    drv.move_down();
                }
            }
            TextKey::Home => {
                if action_mod {
                    if shift {
                        drv.select_to_text_start();
                    } else {
                        drv.move_to_text_start();
                    }
                } else if shift {
                    drv.select_to_line_start();
                } else {
                    drv.move_to_line_start();
                }
            }
            TextKey::End => {
                if action_mod {
                    if shift {
                        drv.select_to_text_end();
                    } else {
                        drv.move_to_text_end();
                    }
                } else if shift {
                    drv.select_to_line_end();
                } else {
                    drv.move_to_line_end();
                }
            }
            TextKey::Copy => {
                // Return selected text for clipboard
                drop(drv);
                if let Some(text) = self.editor.selected_text() {
                    return TextEditResult::Copy(text.to_string());
                }
                return TextEditResult::Handled;
            }
            TextKey::Cut => {
                // Get selected text first, then delete
                drop(drv);
                let text_to_copy = self.editor.selected_text().map(|s| s.to_string());
                if let Some(text) = text_to_copy {
                    let mut drv = self.editor.driver(font_cx, layout_cx);
                    drv.delete();
                    drop(drv);
                    self.update_layout_cache(font_cx, layout_cx);
                    return TextEditResult::Copy(text);
                }
                return TextEditResult::Handled;
            }
            TextKey::Paste(ref text) => {
                drv.insert_or_replace_selection(text);
            }
            TextKey::Character(ref c) => {
                // Handle Ctrl+A for select all
                if action_mod && (c == "a" || c == "A") {
                    if shift {
                        drv.collapse_selection();
                    } else {
                        drv.select_all();
                    }
                } else if !action_mod {
                    // Insert character (including space)
                    drv.insert_or_replace_selection(c);
                }
            }
        }

        // Update layout cache after changes
        drop(drv);
        self.update_layout_cache(font_cx, layout_cx);

        TextEditResult::Handled
    }

    /// Handle mouse press at the given local coordinates (relative to text position).
    pub fn handle_mouse_down(
        &mut self,
        local_x: f32,
        local_y: f32,
        shift: bool,
        font_cx: &mut FontContext,
        layout_cx: &mut LayoutContext<Brush>,
    ) {
        self.cursor_reset();
        self.is_dragging = true;

        let mut drv = self.editor.driver(font_cx, layout_cx);
        if shift {
            drv.extend_selection_to_point(local_x, local_y);
        } else {
            drv.move_to_point(local_x, local_y);
        }
    }

    /// Handle mouse drag at the given local coordinates.
    pub fn handle_mouse_drag(
        &mut self,
        local_x: f32,
        local_y: f32,
        font_cx: &mut FontContext,
        layout_cx: &mut LayoutContext<Brush>,
    ) {
        if !self.is_dragging {
            return;
        }

        self.cursor_reset();
        let mut drv = self.editor.driver(font_cx, layout_cx);
        drv.extend_selection_to_point(local_x, local_y);
    }

    /// Handle mouse release.
    pub fn handle_mouse_up(&mut self) {
        self.is_dragging = false;
    }

    /// Check if a drag is in progress.
    pub fn is_dragging(&self) -> bool {
        self.is_dragging
    }

    /// Handle double-click to select word.
    pub fn handle_double_click(
        &mut self,
        local_x: f32,
        local_y: f32,
        font_cx: &mut FontContext,
        layout_cx: &mut LayoutContext<Brush>,
    ) {
        self.cursor_reset();
        let mut drv = self.editor.driver(font_cx, layout_cx);
        drv.select_word_at_point(local_x, local_y);
    }

    /// Handle triple-click to select line.
    pub fn handle_triple_click(
        &mut self,
        local_x: f32,
        local_y: f32,
        font_cx: &mut FontContext,
        layout_cx: &mut LayoutContext<Brush>,
    ) {
        self.cursor_reset();
        let mut drv = self.editor.driver(font_cx, layout_cx);
        drv.select_hard_line_at_point(local_x, local_y);
    }
}

impl Default for TextEditState {
    fn default() -> Self {
        Self::new("", 32.0)
    }
}
