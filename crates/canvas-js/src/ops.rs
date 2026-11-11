//! The write operations of the stage, as values.
//!
//! One operation is one command of the stage (`add`, `update`, `remove`, `order`, `group`, ...). A batch is a list
//! of them that runs in a single store transaction: it commits once, as one change set, or not at all. Every
//! single-call method of the stage is a batch of one, so the rules live here and nowhere else.

use std::collections::BTreeSet;

use drafftink_edit::group::{group, ungroup};
use drafftink_edit::lifecycle::{ChildPolicy, delete, lock};
use drafftink_edit::transform::move_by;
use drafftink_edit::zorder::{ZOp, z_order};
use drafftink_geom::kurbo::Vec2;
use drafftink_scene::{Element, ElementId, GroupId, IdSource, SceneRead, SceneTx, SeqIds};
use serde::Deserialize;
use serde_json::Value;

use crate::error::ApiError;
use crate::fonts::Fonts;
use crate::json::{self, Expand, Obj};

/// Where the next element, group id and sketch seed come from. Ids never collide with ones already in the scene.
#[derive(Default)]
pub(crate) struct Ids {
    seq: SeqIds,
}

impl Ids {
    /// A fresh element id that no element, deleted ones included, has.
    pub(crate) fn element<R: SceneRead>(&mut self, scene: &R) -> ElementId {
        loop {
            let id = self.seq.element_id();
            if !scene.contains(&id) {
                return id;
            }
        }
    }

    /// A fresh group id.
    pub(crate) fn group<R: SceneRead>(&mut self, scene: &R) -> GroupId {
        loop {
            let id = self.seq.group_id();
            if !scene.iter_all().any(|e| e.group_ids.contains(&id)) {
                return id;
            }
        }
    }

    /// A seed for the hand-drawn look of a new element.
    pub(crate) fn seed(&mut self) -> u32 {
        self.seq.seed()
    }
}

/// What an operation needs besides the transaction.
pub(crate) struct OpCx<'a> {
    pub(crate) ids: &'a mut Ids,
    pub(crate) fonts: &'a mut Fonts,
    /// The stage's default style, merged under the style of every added shape.
    pub(crate) default_style: &'a Obj,
}

/// A step of the z-order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Order {
    /// To the top.
    Front,
    /// To the bottom.
    Back,
    /// Over the next element above.
    Forward,
    /// Under the next element below.
    Backward,
}

impl From<Order> for ZOp {
    fn from(o: Order) -> Self {
        match o {
            Order::Front => ZOp::Front,
            Order::Back => ZOp::Back,
            Order::Forward => ZOp::Forward,
            Order::Backward => ZOp::Backward,
        }
    }
}

fn yes() -> bool {
    true
}

/// One write.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum Op {
    /// Adds a shape on top of everything.
    Add { shape: Value },
    /// Merge-patches a shape.
    Update { id: ElementId, patch: Value },
    /// Removes elements for good, with their bound text and the children of removed frames.
    Remove { ids: Vec<ElementId> },
    /// Changes the stacking order.
    Order { ids: Vec<ElementId>, to: Order },
    /// Groups elements; `id` names the new group.
    Group {
        ids: Vec<ElementId>,
        #[serde(default)]
        id: Option<GroupId>,
    },
    /// Dissolves the groups the elements stand for.
    Ungroup { ids: Vec<ElementId> },
    /// Locks or unlocks.
    Lock {
        ids: Vec<ElementId>,
        #[serde(default = "yes")]
        locked: bool,
    },
    /// Moves by a vector in world units.
    Move { ids: Vec<ElementId>, dx: f64, dy: f64 },
}

/// What an operation produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The id of an added element.
    Added(ElementId),
    /// The id of a new group.
    Grouped(GroupId),
    /// Nothing to report.
    Done,
}

impl Outcome {
    /// The JSON the JS side sees: the new id, or `null`.
    pub(crate) fn into_json(self) -> Value {
        match self {
            Outcome::Added(id) => Value::String(id.as_str().to_owned()),
            Outcome::Grouped(id) => Value::String(id.as_str().to_owned()),
            Outcome::Done => Value::Null,
        }
    }
}

/// Runs one operation in `tx`.
pub(crate) fn apply<T: SceneTx>(tx: &mut T, cx: &mut OpCx<'_>, op: Op) -> Result<Outcome, ApiError> {
    match op {
        Op::Add { shape } => add(tx, cx, shape).map(Outcome::Added),
        Op::Update { id, patch } => update(tx, cx, &id, patch).map(|()| Outcome::Done),
        Op::Remove { ids } => remove(tx, &ids).map(|()| Outcome::Done),
        Op::Order { ids, to } => {
            z_order(tx, &ids, to.into(), None)?;
            Ok(Outcome::Done)
        }
        Op::Group { ids, id } => {
            let new_group = id.unwrap_or_else(|| cx.ids.group(&*tx));
            Ok(Outcome::Grouped(group(tx, &ids, None, new_group)?.group))
        }
        Op::Ungroup { ids } => {
            ungroup(tx, &ids, None, None)?;
            Ok(Outcome::Done)
        }
        Op::Lock { ids, locked } => {
            lock(tx, &ids, Some(locked))?;
            Ok(Outcome::Done)
        }
        Op::Move { ids, dx, dy } => {
            move_by(tx, &ids, Vec2::new(dx, dy))?;
            Ok(Outcome::Done)
        }
    }
}

/// Merges `defaults` under `style`: what the shape sets wins.
fn with_default_style(style: Option<Value>, defaults: &Obj) -> Value {
    let mut merged = Value::Object(defaults.clone());
    if let Some(style) = style {
        json::merge_patch(&mut merged, style);
    }
    merged
}

fn add<T: SceneTx>(tx: &mut T, cx: &mut OpCx<'_>, shape: Value) -> Result<ElementId, ApiError> {
    let mut obj = json::expand(shape, Expand::Create)?;
    let id = match obj.get("id") {
        Some(Value::String(s)) => ElementId::new(s)?,
        Some(_) => return Err(ApiError::invalid("`id` must be a string of letters, digits, `_` and `-`")),
        None => cx.ids.element(&*tx),
    };
    obj.insert("id".to_owned(), Value::String(id.as_str().to_owned()));
    let mut style = with_default_style(obj.remove("style"), cx.default_style);
    if let Value::Object(s) = &mut style {
        s.entry("seed").or_insert_with(|| Value::from(cx.ids.seed()));
    }
    obj.insert("style".to_owned(), style);
    if !obj.contains_key("index") {
        obj.insert(
            "index".to_owned(),
            Value::String(tx.next_top_index().map_err(|e| ApiError::invalid(e.to_string()))?.to_string()),
        );
    }
    let shape = Value::Object(obj);
    json::check_unit_kind(&shape)?;
    let mut element: Element = serde_json::from_value(shape)?;
    cx.fonts.settle(&mut element);
    tx.insert(element)?;
    Ok(id)
}

fn update<T: SceneTx>(tx: &mut T, cx: &mut OpCx<'_>, id: &ElementId, patch: Value) -> Result<(), ApiError> {
    let patch = Value::Object(json::expand(patch, Expand::Patch)?);
    let (mut value, tag) = {
        let old = tx.get(id).filter(|e| !e.deleted).ok_or_else(|| ApiError::NotFound(id.clone()))?;
        (serde_json::to_value(old)?, old.tag())
    };
    json::merge_patch(&mut value, patch);
    json::check_unit_kind(&value)?;
    let mut element: Element = serde_json::from_value(value)?;
    if element.tag() != tag {
        return Err(ApiError::invalid(format!(
            "an element cannot change its type from {} to {}; remove it and add a new one",
            tag.name(),
            element.tag().name()
        )));
    }
    cx.fonts.settle(&mut element);
    tx.update(id, |e| *e = element)?;
    Ok(())
}

/// Hard removal: the soft delete of the edit crate decides what goes with the elements (bound text, frame
/// children, arrow ends), then every tombstone is dropped, so a scene that is added to and removed from stays small.
fn remove<T: SceneTx>(tx: &mut T, ids: &[ElementId]) -> Result<(), ApiError> {
    if ids.is_empty() {
        return Ok(());
    }
    let report = delete(tx, ids, ChildPolicy::Delete)?;
    let gone: BTreeSet<ElementId> = report.deleted.into_iter().chain(ids.iter().cloned()).collect();
    let present: Vec<ElementId> = gone.into_iter().filter(|id| tx.contains(id)).collect();
    for id in &present {
        tx.remove(id)?;
    }
    Ok(())
}
