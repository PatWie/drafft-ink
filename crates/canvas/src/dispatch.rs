//! Konva-style event dispatch for library users.
//!
//! Subscribe to element events by element id, by kind, by name, by group or for the whole stage; events bubble
//! from the element up through its groups (the `group_ids` stack, innermost first) to the stage, and a listener
//! can stop the bubble.
//!
//! ```
//! use drafftink_canvas::{EventKind, Selector};
//! # use drafftink_canvas::Dispatcher;
//! let mut d = Dispatcher::new();
//! let id = d.on(EventKind::Click, "rectangle".parse().unwrap(), Box::new(|e| e.stop_propagation()));
//! d.off(id);
//! ```
//!
//! # Event model
//!
//! * `target` is the element the event is about (`None` for empty space). `current` is the node whose listener runs.
//! * Order: the target element's listeners, then its groups from the innermost to the outermost, then the stage.
//!   Inside one node, listeners run in the order they were added.
//! * [`ElementEvent::stop_propagation`] stops the bubble after the current node's listeners have all run (as
//!   Konva's `cancelBubble`).
//! * `PointerEnter` and `PointerLeave` do not bubble. They fire on the element and on every group the pointer
//!   enters or leaves with it, so a listener on a group sees one enter when the pointer moves from outside onto any
//!   member, and none when it moves between two members.
//! * Drag events go to the element that was pressed, wherever the pointer is. Handle drags (resize, rotate, point
//!   moves) arrive on the stage as `TransformStart`, `Transform` and `TransformEnd` with the handle set.

use std::str::FromStr;

use drafftink_geom::kurbo::Vec2;
use drafftink_scene::{ElementId, GroupId, Kind, SceneRead};

use crate::input::{Button, Modifiers, PointerKind};
use crate::intent::{Handle, Pos};

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventKind {
    /// A pointer went down on the target.
    PointerDown,
    /// Press and release without a drag.
    Click,
    /// The second click of a double click.
    DoubleClick,
    /// A long press.
    LongPress,
    /// Secondary button release (the context menu).
    ContextMenu,
    /// A drag of the target started.
    DragStart,
    /// The drag moved.
    DragMove,
    /// The drag ended (see `cancelled`).
    DragEnd,
    /// The pointer entered the element or a group it belongs to. Does not bubble.
    PointerEnter,
    /// The pointer left the element or a group. Does not bubble.
    PointerLeave,
    /// A handle drag started (stage).
    TransformStart,
    /// A handle drag moved (stage).
    Transform,
    /// A handle drag ended (stage).
    TransformEnd,
}

/// A place in the tree an event passes through.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Node {
    /// An element.
    Element(ElementId),
    /// A group of elements.
    Group(GroupId),
    /// The root: every event ends here.
    Stage,
}

/// Which nodes a listener listens on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Selector {
    /// Every node an event passes through.
    Any,
    /// The stage only: events that bubbled all the way up, and events on empty space.
    Stage,
    /// The element with this id.
    Id(ElementId),
    /// Every element of this kind.
    Kind(Kind),
    /// Every element whose `name` contains this word (names are space separated, like CSS classes).
    Name(String),
    /// This group.
    Group(GroupId),
}

/// A selector string was not understood.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not a selector; use `*`, `stage`, `#id`, `.name`, `group:id` or a kind such as `rectangle`")]
pub struct SelectorError(pub String);

impl FromStr for Selector {
    type Err = SelectorError;

    /// `*` any node, `stage`, `#e1` an id, `.label` a name word, `group:g1` a group, `rectangle` a kind.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || SelectorError(s.to_string());
        if s == "*" {
            return Ok(Selector::Any);
        }
        if s == "stage" {
            return Ok(Selector::Stage);
        }
        if let Some(id) = s.strip_prefix('#') {
            return ElementId::new(id).map(Selector::Id).map_err(|_| bad());
        }
        if let Some(name) = s.strip_prefix('.') {
            return if name.is_empty() || name.contains(char::is_whitespace) {
                Err(bad())
            } else {
                Ok(Selector::Name(name.to_string()))
            };
        }
        if let Some(g) = s.strip_prefix("group:") {
            return GroupId::new(g).map(Selector::Group).map_err(|_| bad());
        }
        s.parse::<Kind>().map(Selector::Kind).map_err(|_| bad())
    }
}

/// An event on its way through the tree.
#[derive(Debug, Clone, PartialEq)]
pub struct ElementEvent {
    /// What happened.
    pub kind: EventKind,
    /// The element it is about; `None` for empty space and handle drags.
    pub target: Option<ElementId>,
    /// The node whose listeners are running now.
    pub current: Node,
    /// Pointer position.
    pub pos: Pos,
    /// Drag movement since the press in screen pixels (zero for other events).
    pub delta_screen: Vec2,
    /// Drag movement since the press in world units.
    pub delta_world: Vec2,
    /// Modifiers at the time.
    pub modifiers: Modifiers,
    /// Device kind.
    pub pointer: PointerKind,
    /// Button involved.
    pub button: Button,
    /// The handle, for transform events.
    pub handle: Option<Handle>,
    /// A drag end that was aborted.
    pub cancelled: bool,
    stopped: bool,
}

impl ElementEvent {
    /// A new event with zero deltas and no modifiers.
    pub fn new(kind: EventKind, target: Option<ElementId>, pos: Pos) -> Self {
        Self {
            kind,
            target,
            current: Node::Stage,
            pos,
            delta_screen: Vec2::ZERO,
            delta_world: Vec2::ZERO,
            modifiers: Modifiers::NONE,
            pointer: PointerKind::Mouse,
            button: Button::Primary,
            handle: None,
            cancelled: false,
            stopped: false,
        }
    }

    /// Stops the event from reaching nodes above the current one.
    pub fn stop_propagation(&mut self) {
        self.stopped = true;
    }

    /// True once a listener called [`ElementEvent::stop_propagation`].
    pub fn is_propagation_stopped(&self) -> bool {
        self.stopped
    }
}

/// Handle of a registered listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ListenerId(u64);

/// A listener callback.
pub type Listener = Box<dyn FnMut(&mut ElementEvent)>;

struct Entry {
    id: ListenerId,
    kind: EventKind,
    selector: Selector,
    callback: Listener,
}

/// Holds listeners and delivers events. See the module docs.
#[derive(Default)]
pub struct Dispatcher {
    listeners: Vec<Entry>,
    next: u64,
}

impl std::fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dispatcher").field("listeners", &self.listeners.len()).finish()
    }
}

impl Dispatcher {
    /// A dispatcher without listeners.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a listener for events of `kind` on the nodes `selector` matches.
    pub fn on(&mut self, kind: EventKind, selector: Selector, callback: Listener) -> ListenerId {
        self.next += 1;
        let id = ListenerId(self.next);
        self.listeners.push(Entry { id, kind, selector, callback });
        id
    }

    /// Removes a listener. Returns false when it was already gone.
    pub fn off(&mut self, id: ListenerId) -> bool {
        let before = self.listeners.len();
        self.listeners.retain(|l| l.id != id);
        self.listeners.len() != before
    }

    /// Removes every listener.
    pub fn clear(&mut self) {
        self.listeners.clear();
    }

    /// Number of listeners.
    pub fn len(&self) -> usize {
        self.listeners.len()
    }

    /// True when there are no listeners.
    pub fn is_empty(&self) -> bool {
        self.listeners.is_empty()
    }

    /// True when some listener wants events of this kind. The facade skips building events nobody listens to.
    pub fn wants(&self, kind: EventKind) -> bool {
        self.listeners.iter().any(|l| l.kind == kind)
    }

    /// The path an event bubbles along: the element, its groups from the innermost out, the stage.
    pub fn bubble_path<R: SceneRead>(scene: &R, target: Option<&ElementId>) -> Vec<Node> {
        let mut path = Vec::new();
        if let Some(id) = target {
            path.push(Node::Element(id.clone()));
            if let Some(e) = scene.get(id) {
                path.extend(e.group_ids.iter().map(|g| Node::Group(g.clone())));
            }
        }
        path.push(Node::Stage);
        path
    }

    /// Delivers `ev` along its bubble path. Returns the number of listeners that ran.
    pub fn dispatch<R: SceneRead>(&mut self, scene: &R, ev: &mut ElementEvent) -> usize {
        let path = Self::bubble_path(scene, ev.target.as_ref());
        self.dispatch_along(scene, ev, &path)
    }

    /// Delivers `ev` to exactly the nodes in `path`, in order, honouring `stop_propagation`. Returns the number of
    /// listeners that ran.
    pub fn dispatch_along<R: SceneRead>(&mut self, scene: &R, ev: &mut ElementEvent, path: &[Node]) -> usize {
        let mut ran = 0;
        let kind = ev.kind;
        for node in path {
            ev.current = node.clone();
            for l in self.listeners.iter_mut().filter(|l| l.kind == kind) {
                if matches(&l.selector, node, scene) {
                    (l.callback)(ev);
                    ran += 1;
                }
            }
            if ev.stopped {
                break;
            }
        }
        ran
    }
}

fn matches<R: SceneRead>(sel: &Selector, node: &Node, scene: &R) -> bool {
    match (sel, node) {
        (Selector::Any, _) => true,
        (Selector::Stage, Node::Stage) => true,
        (Selector::Group(g), Node::Group(n)) => g == n,
        (Selector::Id(id), Node::Element(n)) => id == n,
        (Selector::Kind(k), Node::Element(n)) => scene.get(n).is_some_and(|e| e.tag() == *k),
        (Selector::Name(name), Node::Element(n)) => {
            scene.get(n).and_then(|e| e.name.as_deref()).is_some_and(|s| s.split_whitespace().any(|w| w == name))
        }
        _ => false,
    }
}
