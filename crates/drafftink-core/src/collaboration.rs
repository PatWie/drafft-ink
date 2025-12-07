//! Collaboration management for real-time multi-user editing.
//!
//! This module provides the bridge between local document state and CRDT-based
//! synchronization for collaborative editing.

use serde_json;

use crate::canvas::CanvasDocument;
use crate::crdt::CrdtDocument;
use crate::shapes::{Shape, ShapeId};
use crate::sync::{
    AwarenessState, ClientMessage, CursorPosition, ServerMessage, SyncEvent, base64_decode,
    base64_encode,
};

/// Manages collaboration state and synchronization between local and CRDT documents.
pub struct CollaborationManager {
    /// The CRDT document for synchronization.
    crdt: CrdtDocument,
    /// Whether collaboration is currently enabled.
    enabled: bool,
    /// Peer ID for this client (from Loro).
    peer_id: u64,
    /// Current room ID (if connected).
    current_room: Option<String>,
    /// Local awareness state (cursor, user info).
    awareness: AwarenessState,
    /// Pending outgoing messages (JSON strings).
    outgoing: Vec<String>,
}

impl CollaborationManager {
    /// Create a new collaboration manager.
    pub fn new() -> Self {
        let crdt = CrdtDocument::new();
        let peer_id = crdt.loro_doc().peer_id();
        Self {
            crdt,
            enabled: false,
            peer_id,
            current_room: None,
            awareness: AwarenessState::default(),
            outgoing: Vec::new(),
        }
    }

    /// Create from an existing CRDT document (e.g., loaded from storage or network).
    pub fn from_crdt(crdt: CrdtDocument) -> Self {
        let peer_id = crdt.loro_doc().peer_id();
        Self {
            crdt,
            enabled: false,
            peer_id,
            current_room: None,
            awareness: AwarenessState::default(),
            outgoing: Vec::new(),
        }
    }

    /// Get the peer ID for this client.
    pub fn peer_id(&self) -> u64 {
        self.peer_id
    }

    /// Check if collaboration is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Enable collaboration.
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    /// Disable collaboration.
    pub fn disable(&mut self) {
        self.enabled = false;
    }

    /// Get a reference to the CRDT document.
    pub fn crdt(&self) -> &CrdtDocument {
        &self.crdt
    }

    /// Get a mutable reference to the CRDT document.
    pub fn crdt_mut(&mut self) -> &mut CrdtDocument {
        &mut self.crdt
    }

    // --- Sync Operations ---

    /// Sync local document state to CRDT.
    /// This should be called after local operations to propagate changes.
    pub fn sync_to_crdt(&mut self, doc: &CanvasDocument) {
        if !self.enabled {
            return;
        }

        // Clear existing CRDT shapes
        let _ = self.crdt.clear();

        // Set document name
        let _ = self.crdt.set_name(&doc.name);

        // Add all shapes in z-order
        for shape_id in &doc.z_order {
            if let Some(shape) = doc.shapes.get(shape_id) {
                let _ = self.crdt.add_shape(shape);
            }
        }
    }

    /// Sync CRDT state to local document.
    /// This should be called when receiving remote updates.
    pub fn sync_from_crdt(&self, doc: &mut CanvasDocument) {
        // Clear local document
        doc.shapes.clear();
        doc.z_order.clear();

        // Update name
        let name = self.crdt.name();
        if !name.is_empty() {
            doc.name = name;
        }

        // Rebuild from CRDT
        let z_order = self.crdt.z_order();
        for id_str in z_order {
            if let Some(shape) = self.crdt.get_shape(&id_str) {
                let shape_id = shape.id();
                doc.shapes.insert(shape_id, shape);
                doc.z_order.push(shape_id);
            }
        }
    }

    // --- Shape Operations (synced to CRDT) ---

    /// Add a shape, syncing to CRDT if enabled.
    pub fn add_shape(&mut self, doc: &mut CanvasDocument, shape: Shape) {
        let shape_clone = shape.clone();
        doc.add_shape(shape);

        if self.enabled {
            let _ = self.crdt.add_shape(&shape_clone);
        }
    }

    /// Remove a shape, syncing to CRDT if enabled.
    pub fn remove_shape(&mut self, doc: &mut CanvasDocument, id: ShapeId) -> Option<Shape> {
        let result = doc.remove_shape(id);

        if self.enabled && result.is_some() {
            let _ = self.crdt.remove_shape(&id.to_string());
        }

        result
    }

    /// Update a shape, syncing to CRDT if enabled.
    pub fn update_shape(&mut self, doc: &mut CanvasDocument, shape: Shape) {
        let id = shape.id();
        if let Some(existing) = doc.shapes.get_mut(&id) {
            *existing = shape.clone();

            if self.enabled {
                let _ = self.crdt.update_shape(&shape);
            }
        }
    }

    // --- Network Sync ---

    /// Export full document snapshot for initial sync.
    pub fn export_snapshot(&self) -> Vec<u8> {
        self.crdt.export_snapshot()
    }

    /// Export incremental updates since a version.
    pub fn export_updates(&self, since: &loro::VersionVector) -> Vec<u8> {
        self.crdt.export_updates(since)
    }

    /// Import updates from remote peer.
    /// Returns true if import was successful.
    pub fn import_updates(&mut self, bytes: &[u8]) -> bool {
        self.crdt.import(bytes).is_ok()
    }

    /// Get the current version vector.
    pub fn version(&self) -> loro::VersionVector {
        self.crdt.version()
    }

    // --- Undo/Redo (using CRDT's undo manager) ---

    /// Undo the last local change.
    pub fn undo(&mut self) -> bool {
        if self.enabled {
            self.crdt.undo()
        } else {
            false
        }
    }

    /// Redo the last undone change.
    pub fn redo(&mut self) -> bool {
        if self.enabled {
            self.crdt.redo()
        } else {
            false
        }
    }

    /// Check if undo is available.
    pub fn can_undo(&self) -> bool {
        if self.enabled {
            self.crdt.can_undo()
        } else {
            false
        }
    }

    /// Check if redo is available.
    pub fn can_redo(&self) -> bool {
        if self.enabled {
            self.crdt.can_redo()
        } else {
            false
        }
    }

    // --- Room/Connection Management ---

    /// Get the current room ID.
    pub fn current_room(&self) -> Option<&str> {
        self.current_room.as_deref()
    }

    /// Check if we're in a room.
    pub fn is_in_room(&self) -> bool {
        self.current_room.is_some()
    }

    /// Set the current room (called when server confirms join).
    pub fn set_room(&mut self, room: Option<String>) {
        self.current_room = room;
        if self.current_room.is_some() {
            self.enable();
        }
    }

    /// Request to join a room. Queues the join message.
    pub fn join_room(&mut self, room: &str) {
        let msg = ClientMessage::Join {
            room: room.to_string(),
        };
        if let Ok(json) = serde_json::to_string(&msg) {
            self.outgoing.push(json);
        }
    }

    /// Request to leave the current room. Queues the leave message.
    pub fn leave_room(&mut self) {
        if self.current_room.is_some() {
            let msg = ClientMessage::Leave;
            if let Ok(json) = serde_json::to_string(&msg) {
                self.outgoing.push(json);
            }
            self.current_room = None;
        }
    }

    /// Take pending outgoing messages (drains the queue).
    pub fn take_outgoing(&mut self) -> Vec<String> {
        std::mem::take(&mut self.outgoing)
    }

    /// Check if there are pending outgoing messages.
    pub fn has_outgoing(&self) -> bool {
        !self.outgoing.is_empty()
    }

    // --- Awareness ---

    /// Update local cursor position.
    pub fn set_cursor(&mut self, x: f64, y: f64) {
        self.awareness.cursor = Some(CursorPosition { x, y });
        self.queue_awareness();
    }

    /// Clear local cursor (e.g., mouse left canvas).
    pub fn clear_cursor(&mut self) {
        self.awareness.cursor = None;
        self.queue_awareness();
    }

    /// Set user info (name, color).
    pub fn set_user_info(&mut self, name: String, color: String) {
        self.awareness.user = Some(crate::sync::UserInfo { name, color });
        self.queue_awareness();
    }

    /// Get local awareness state.
    pub fn awareness(&self) -> &AwarenessState {
        &self.awareness
    }

    /// Queue awareness broadcast.
    fn queue_awareness(&mut self) {
        if self.current_room.is_some() {
            let msg = ClientMessage::Awareness {
                peer_id: self.peer_id,
                state: self.awareness.clone(),
            };
            if let Ok(json) = serde_json::to_string(&msg) {
                self.outgoing.push(json);
            }
        }
    }

    // --- Sync Broadcast ---

    /// Queue a sync broadcast with current CRDT state.
    pub fn broadcast_sync(&mut self) {
        if self.current_room.is_some() && self.enabled {
            let snapshot = self.crdt.export_snapshot();
            let data = base64_encode(&snapshot);
            let msg = ClientMessage::Sync { data };
            if let Ok(json) = serde_json::to_string(&msg) {
                self.outgoing.push(json);
            }
        }
    }

    // --- Incoming Message Handling ---

    /// Handle an incoming server message.
    /// Returns a SyncEvent describing what happened.
    pub fn handle_message(&mut self, json: &str) -> Option<SyncEvent> {
        let msg: ServerMessage = serde_json::from_str(json).ok()?;

        match msg {
            ServerMessage::Joined {
                room,
                peer_count,
                initial_sync,
            } => {
                self.current_room = Some(room.clone());
                self.enable();

                // Import initial state if provided
                let initial_data = initial_sync.and_then(|s| {
                    base64_decode(&s).and_then(|bytes| {
                        if self.crdt.import(&bytes).is_ok() {
                            Some(bytes)
                        } else {
                            None
                        }
                    })
                });

                Some(SyncEvent::JoinedRoom {
                    room,
                    peer_count,
                    initial_sync: initial_data,
                })
            }
            ServerMessage::PeerJoined { peer_id } => Some(SyncEvent::PeerJoined { peer_id }),
            ServerMessage::PeerLeft { peer_id } => Some(SyncEvent::PeerLeft { peer_id }),
            ServerMessage::Sync { from, data } => {
                // Decode and import CRDT data
                if let Some(bytes) = base64_decode(&data) {
                    if self.crdt.import(&bytes).is_ok() {
                        return Some(SyncEvent::SyncReceived { from, data: bytes });
                    }
                }
                None
            }
            ServerMessage::Awareness {
                from,
                peer_id,
                state,
            } => Some(SyncEvent::AwarenessReceived {
                from,
                peer_id,
                state,
            }),
            ServerMessage::Error { message } => Some(SyncEvent::Error { message }),
        }
    }
}

impl Default for CollaborationManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::Rectangle;
    use kurbo::Point;

    #[test]
    fn test_collaboration_disabled_by_default() {
        let manager = CollaborationManager::new();
        assert!(!manager.is_enabled());
    }

    #[test]
    fn test_sync_to_crdt() {
        let mut manager = CollaborationManager::new();
        manager.enable();

        let mut doc = CanvasDocument::new();
        let rect = Rectangle::new(Point::new(0.0, 0.0), 100.0, 50.0);
        doc.add_shape(Shape::Rectangle(rect));

        manager.sync_to_crdt(&doc);

        assert_eq!(manager.crdt().shape_count(), 1);
    }

    #[test]
    fn test_sync_from_crdt() {
        let mut manager = CollaborationManager::new();

        // Add shape to CRDT
        let rect = Rectangle::new(Point::new(10.0, 20.0), 100.0, 50.0);
        manager
            .crdt_mut()
            .add_shape(&Shape::Rectangle(rect))
            .unwrap();

        // Sync to local document
        let mut doc = CanvasDocument::new();
        manager.sync_from_crdt(&mut doc);

        assert_eq!(doc.shapes.len(), 1);
        assert_eq!(doc.z_order.len(), 1);
    }

    #[test]
    fn test_export_import_snapshot() {
        let mut manager1 = CollaborationManager::new();
        manager1.enable();

        // Add shape to first manager
        let rect = Rectangle::new(Point::new(50.0, 50.0), 200.0, 100.0);
        manager1
            .crdt_mut()
            .add_shape(&Shape::Rectangle(rect))
            .unwrap();

        // Export snapshot
        let snapshot = manager1.export_snapshot();

        // Import to second manager
        let mut manager2 = CollaborationManager::new();
        assert!(manager2.import_updates(&snapshot));

        assert_eq!(manager2.crdt().shape_count(), 1);
    }

    #[test]
    fn test_add_shape_with_sync() {
        let mut manager = CollaborationManager::new();
        manager.enable();

        let mut doc = CanvasDocument::new();
        let rect = Rectangle::new(Point::new(0.0, 0.0), 100.0, 100.0);

        manager.add_shape(&mut doc, Shape::Rectangle(rect));

        // Both local and CRDT should have the shape
        assert_eq!(doc.shapes.len(), 1);
        assert_eq!(manager.crdt().shape_count(), 1);
    }

    #[test]
    fn test_remove_shape_with_sync() {
        let mut manager = CollaborationManager::new();
        manager.enable();

        let mut doc = CanvasDocument::new();
        let rect = Rectangle::new(Point::new(0.0, 0.0), 100.0, 100.0);
        let shape = Shape::Rectangle(rect);
        let id = shape.id();

        manager.add_shape(&mut doc, shape);
        assert_eq!(doc.shapes.len(), 1);
        assert_eq!(manager.crdt().shape_count(), 1);

        manager.remove_shape(&mut doc, id);
        assert_eq!(doc.shapes.len(), 0);
        assert_eq!(manager.crdt().shape_count(), 0);
    }
}
