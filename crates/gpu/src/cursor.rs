//! The mouse cursor the shell shows: egui's cursor icon as a winit one. Winit's web backend turns the same names
//! into CSS cursors (`canvas.style.cursor`), so native and web share the mapping.

/// egui's cursor icon as a winit one.
pub(crate) fn cursor_icon(icon: egui::CursorIcon) -> Option<winit::window::CursorIcon> {
    use egui::CursorIcon as E;
    use winit::window::CursorIcon as W;
    Some(match icon {
        E::None => return None,
        E::Default => W::Default,
        E::ContextMenu => W::ContextMenu,
        E::Help => W::Help,
        E::PointingHand => W::Pointer,
        E::Progress => W::Progress,
        E::Wait => W::Wait,
        E::Cell => W::Cell,
        E::Crosshair => W::Crosshair,
        E::Text => W::Text,
        E::VerticalText => W::VerticalText,
        E::Alias => W::Alias,
        E::Copy => W::Copy,
        E::Move => W::Move,
        E::NoDrop => W::NoDrop,
        E::NotAllowed => W::NotAllowed,
        E::Grab => W::Grab,
        E::Grabbing => W::Grabbing,
        E::AllScroll => W::AllScroll,
        E::ResizeHorizontal => W::EwResize,
        E::ResizeNeSw => W::NeswResize,
        E::ResizeNwSe => W::NwseResize,
        E::ResizeVertical => W::NsResize,
        E::ResizeEast => W::EResize,
        E::ResizeSouthEast => W::SeResize,
        E::ResizeSouth => W::SResize,
        E::ResizeSouthWest => W::SwResize,
        E::ResizeWest => W::WResize,
        E::ResizeNorthWest => W::NwResize,
        E::ResizeNorth => W::NResize,
        E::ResizeNorthEast => W::NeResize,
        E::ResizeColumn => W::ColResize,
        E::ResizeRow => W::RowResize,
        E::ZoomIn => W::ZoomIn,
        E::ZoomOut => W::ZoomOut,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursors_the_app_uses_map_to_their_winit_names() {
        use winit::window::CursorIcon as W;
        assert_eq!(cursor_icon(egui::CursorIcon::Text), Some(W::Text));
        assert_eq!(cursor_icon(egui::CursorIcon::PointingHand), Some(W::Pointer));
        assert_eq!(cursor_icon(egui::CursorIcon::Move), Some(W::Move));
        assert_eq!(cursor_icon(egui::CursorIcon::Default), Some(W::Default));
        assert_eq!(cursor_icon(egui::CursorIcon::None), None, "no cursor hides it");
    }
}
