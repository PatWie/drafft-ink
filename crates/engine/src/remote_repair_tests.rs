//! The merge repair (`remote_repair`): two or three sessions edit concurrently, exchange updates, and must end
//! with the same scene, a scene `validate_scene` accepts, arrows that sit on their targets, repairs that are on no
//! undo stack, and no update ping-pong.

use drafftink_doc::{LoroConfig, LoroStore};
use drafftink_edit::binding;
use drafftink_scene::{
    DocError, Document, Element, ElementId, ElementKind, GroupId, IdSource, SceneRead, Severity, snapshot,
    validate_scene,
};
use proptest::prelude::*;
use serde_json::{Value, json};

use crate::ports::fake::Fakes;
use crate::ring::Origin;
use crate::services::Services;
use crate::session::Session;

/// Ids that differ between peers, as random ids do.
struct PeerIds {
    prefix: &'static str,
    n: u64,
}

impl IdSource for PeerIds {
    fn element_id(&mut self) -> ElementId {
        self.n += 1;
        ElementId::new(&format!("{}{}", self.prefix, self.n)).unwrap()
    }
    fn group_id(&mut self) -> GroupId {
        self.n += 1;
        GroupId::new(&format!("g{}{}", self.prefix, self.n)).unwrap()
    }
    fn seed(&mut self) -> u32 {
        self.n += 1;
        u32::try_from(self.n).unwrap_or(1)
    }
}

/// A session with its own id prefix and a fixed CRDT peer id, so a failing case repeats.
fn peer(prefix: &'static str, crdt_peer: u64) -> Session {
    let mut ports = Fakes::new().headless_ports();
    ports.ids = Box::new(PeerIds { prefix, n: 0 });
    let mut s = Session::with_services(ports, Services::fake());
    *s.tab_mut().doc.replicated_mut() =
        LoroStore::with_config(LoroConfig { peer_id: Some(crdt_peer), ..LoroConfig::default() }).unwrap();
    s
}

fn run(s: &mut Session, id: &str, args: Value) -> Value {
    s.execute(id, args.clone()).unwrap_or_else(|r| panic!("{id} {args} refused: {} ({})", r.message, r.code))
}

fn id_of(v: &Value) -> String {
    v["id"].as_str().unwrap().to_owned()
}

fn rect(s: &mut Session, x: f64, y: f64) -> String {
    id_of(&run(s, "element.create", json!({"kind": "rectangle", "bounds": {"x": x, "y": y, "w": 100, "h": 60}})))
}

/// What `from` has and `to` lacks goes to `to`.
fn send(from: &Session, to: &mut Session) {
    let vv = to.tab().doc.replicated().version_vector().unwrap();
    let bytes = from.tab().doc.replicated().export_updates_since(&vv).unwrap();
    let tab = to.tab().id;
    to.apply_remote(tab, &bytes).unwrap();
}

/// Every write any peer has made, repairs included.
fn writes(peers: &[Session]) -> u64 {
    peers.iter().map(|p| p.tab().doc.local_writes()).sum()
}

/// Exchanges updates until a whole round of all-to-all delivery makes nobody write. Repairs are writes, so this also
/// proves that the peers stop answering each other.
fn settle(peers: &mut [Session]) {
    for round in 0..30 {
        let before = writes(peers);
        for to in 0..peers.len() {
            for from in 0..peers.len() {
                if from != to {
                    let (src, dst) = two(peers, from, to);
                    send(src, dst);
                }
            }
        }
        if writes(peers) == before {
            return;
        }
        assert!(round < 29, "the peers are still writing after 30 rounds of exchanging updates: a repair ping-pongs");
    }
}

fn two(peers: &mut [Session], from: usize, to: usize) -> (&Session, &mut Session) {
    if from < to {
        let (l, r) = peers.split_at_mut(to);
        (&l[from], &mut r[0])
    } else {
        let (l, r) = peers.split_at_mut(from);
        (&r[0], &mut l[to])
    }
}

/// The scene without the local `version` counters.
fn content(r: &impl SceneRead) -> Document {
    let mut d = snapshot(r);
    for e in &mut d.elements {
        e.version = 0;
    }
    d
}

/// Warnings a reader resolves and the repair leaves alone on purpose (docs/crdt.md section 10a).
fn tolerated(e: &DocError) -> bool {
    matches!(e, DocError::GroupSingleton(_) | DocError::DuplicateName { .. } | DocError::FileMissing(_))
}

/// Every peer holds the same scene, the working store shows what the CRDT holds, the scene is valid and the arrows
/// sit where their bindings put them.
fn assert_converged_and_valid(peers: &[Session]) {
    let first = content(peers[0].tab().doc.replicated());
    for (i, p) in peers.iter().enumerate() {
        let truth = p.tab().doc.replicated();
        assert_eq!(content(truth), first, "peer {i} differs from peer 0");
        assert_eq!(content(p.store()), content(truth), "peer {i}: the working store differs from the CRDT");
        let report = validate_scene(truth);
        let bad: Vec<String> = report
            .issues
            .iter()
            .filter(|i| i.severity == Severity::Error || !tolerated(&i.error))
            .map(ToString::to_string)
            .collect();
        assert!(bad.is_empty(), "peer {i}: the merged scene is invalid:\n{}", bad.join("\n"));
        let stale = binding::stale_arrows(truth);
        assert!(stale.is_empty(), "peer {i}: arrows off their targets: {stale:?}");
    }
}

fn element(s: &Session, id: &str) -> Element {
    s.tab().doc.replicated().get(&ElementId::new(id).unwrap()).unwrap().clone()
}

/// An arrow between two rectangles that every peer holds.
fn arrow_between_two_rects(kind: &str) -> ([Session; 2], String, String, String) {
    let mut p = [peer("a", 1), peer("b", 2)];
    let r1 = rect(&mut p[0], 0.0, 0.0);
    let r2 = rect(&mut p[0], 400.0, 0.0);
    let arrow = id_of(&run(&mut p[0], "element.connect", json!({"from": r1, "to": r2, "arrow_type": kind})));
    let (a, b) = p.split_at_mut(1);
    send(&a[0], &mut b[0]);
    (p, r1, r2, arrow)
}

#[test]
fn two_peers_moving_different_targets_of_one_arrow_leave_it_on_both() {
    let (mut p, r1, r2, arrow) = arrow_between_two_rects("sharp");
    run(&mut p[0], "element.move", json!({"ids": [r1], "dx": 0, "dy": 200}));
    run(&mut p[1], "element.move", json!({"ids": [r2], "dx": 0, "dy": -150}));
    settle(&mut p);
    assert_converged_and_valid(&p);
    let a = element(&p[0], &arrow);
    let ElementKind::Arrow(d) = &a.kind else { panic!("an arrow") };
    assert!(d.start_binding.is_some() && d.end_binding.is_some(), "both ends stay bound");
}

#[test]
fn the_same_for_an_elbow_arrow() {
    let (mut p, r1, r2, _) = arrow_between_two_rects("elbow");
    run(&mut p[0], "element.move", json!({"ids": [r1], "dx": 0, "dy": 200}));
    run(&mut p[1], "element.move", json!({"ids": [r2], "dx": 80, "dy": -150}));
    settle(&mut p);
    assert_converged_and_valid(&p);
}

#[test]
fn a_delete_racing_a_bind_frees_the_arrow_end_on_every_peer() {
    let (mut p, r1, r2, arrow) = arrow_between_two_rects("sharp");
    let r3 = rect(&mut p[0], 0.0, 300.0);
    let (a, b) = p.split_at_mut(1);
    send(&a[0], &mut b[0]);
    // B deletes r3 while A binds the arrow's end to it.
    run(&mut p[1], "element.delete", json!({"ids": [r3]}));
    run(&mut p[0], "arrow.bind", json!({"arrow": arrow, "end": "end", "target": r3}));
    settle(&mut p);
    assert_converged_and_valid(&p);
    for s in &p {
        let ElementKind::Arrow(d) = element(s, &arrow).kind else { panic!("an arrow") };
        assert_eq!(d.end_binding, None, "the end bound to the deleted shape is free");
        assert_eq!(d.start_binding.map(|b| b.target.to_string()), Some(r1.clone()), "the other end is untouched");
    }
    let _ = r2;
}

#[test]
fn a_repair_is_on_no_undo_stack_and_is_recorded_in_the_ring_as_remote() {
    let (mut p, _r1, _r2, arrow) = arrow_between_two_rects("sharp");
    let r3 = rect(&mut p[0], 0.0, 300.0);
    let (a, b) = p.split_at_mut(1);
    send(&a[0], &mut b[0]);
    run(&mut p[1], "element.delete", json!({"ids": [r3]}));
    run(&mut p[0], "arrow.bind", json!({"arrow": arrow, "end": "end", "target": r3}));
    let depths: Vec<usize> = p.iter().map(|s| s.tab().doc.undo_depth()).collect();
    let writes: Vec<u64> = p.iter().map(|s| s.tab().doc.local_writes()).collect();
    settle(&mut p);
    let repairers: Vec<usize> = (0..2).filter(|i| p[*i].tab().doc.local_writes() > writes[*i]).collect();
    assert!(!repairers.is_empty(), "somebody repaired");
    for (i, s) in p.iter().enumerate() {
        assert_eq!(s.tab().doc.undo_depth(), depths[i], "peer {i}: a repair added an undo step");
    }
    for i in repairers {
        let entry = p[i].ring().iter().find(|e| e.id == "remote.repair").expect("a ring entry for the repair");
        assert_eq!(entry.source, Origin::Remote);
        assert!(entry.outcome.is_ok());
        let args: Value = serde_json::from_str(&entry.args).unwrap();
        assert_eq!(args["ends_freed"], 1);
    }
    // Undo on the binder takes back its own bind and nothing else; the arrow keeps no binding to r3.
    run(&mut p[0], "history.undo", json!({}));
    settle(&mut p);
    assert_converged_and_valid(&p);
}

#[test]
fn a_repair_is_written_once_and_a_second_pass_changes_nothing() {
    let (mut p, _r1, _r2, arrow) = arrow_between_two_rects("sharp");
    let r3 = rect(&mut p[0], 0.0, 300.0);
    let (a, b) = p.split_at_mut(1);
    send(&a[0], &mut b[0]);
    run(&mut p[1], "element.delete", json!({"ids": [r3]}));
    run(&mut p[0], "arrow.bind", json!({"arrow": arrow, "end": "end", "target": r3}));
    settle(&mut p);
    let before = writes(&p);
    for (i, peer) in p.iter_mut().enumerate() {
        let cs = peer.tab().doc.store().iter_all().map(|e| drafftink_scene::Change {
            id: e.id.clone(),
            kind: drafftink_scene::ChangeKind::Updated,
            fields: drafftink_scene::Fields::all(),
            version: 0,
        });
        let cs = drafftink_scene::ChangeSet { revision: 1, elements: cs.collect(), meta: false, files: Vec::new() };
        let tab = peer.tab().id;
        let r = peer.repair_merge(tab, &cs).unwrap();
        assert!(r.is_empty(), "peer {i} repaired a scene that was repaired already: {r:?}");
    }
    assert_eq!(before, writes(&p), "a repair of a repaired scene wrote");
}

#[test]
fn a_deleted_container_releases_a_label_bound_concurrently() {
    let mut p = [peer("a", 1), peer("b", 2)];
    let r = rect(&mut p[0], 0.0, 0.0);
    let (a, b) = p.split_at_mut(1);
    send(&a[0], &mut b[0]);
    let t = id_of(&run(&mut p[0], "element.create", json!({"kind": "text", "text": "hi", "at": [300, 300]})));
    let (a, b) = p.split_at_mut(1);
    send(&a[0], &mut b[0]);
    run(&mut p[0], "text.bind", json!({"text": t, "container": r}));
    run(&mut p[1], "element.delete", json!({"ids": [r]}));
    settle(&mut p);
    assert_converged_and_valid(&p);
    for s in &p {
        let text = element(s, &t);
        assert!(!text.deleted, "the label survives as free text");
        assert_eq!(text.container_id(), None);
    }
}

#[test]
fn two_labels_for_one_container_leave_the_smaller_id_bound() {
    let mut p = [peer("a", 1), peer("b", 2)];
    let r = rect(&mut p[0], 0.0, 0.0);
    let ta = id_of(&run(&mut p[0], "element.create", json!({"kind": "text", "text": "one", "at": [300, 300]})));
    let tb = id_of(&run(&mut p[0], "element.create", json!({"kind": "text", "text": "two", "at": [300, 400]})));
    let (a, b) = p.split_at_mut(1);
    send(&a[0], &mut b[0]);
    run(&mut p[0], "text.bind", json!({"text": ta, "container": r}));
    run(&mut p[1], "text.bind", json!({"text": tb, "container": r}));
    settle(&mut p);
    assert_converged_and_valid(&p);
    let winner = ta.clone().min(tb.clone());
    for s in &p {
        for t in [&ta, &tb] {
            assert_eq!(element(s, t).container_id().is_some(), *t == winner, "{t}");
        }
    }
}

#[test]
fn a_frame_deleted_while_another_peer_adds_a_child_releases_the_child() {
    let mut p = [peer("a", 1), peer("b", 2)];
    let child = rect(&mut p[0], 10.0, 10.0);
    let other = rect(&mut p[0], 10.0, 100.0);
    let wrapped = run(&mut p[0], "frame.wrap", json!({"ids": [other]}));
    let frame = wrapped["frame"].as_str().unwrap_or_else(|| panic!("no frame in {wrapped}")).to_owned();
    let (a, b) = p.split_at_mut(1);
    send(&a[0], &mut b[0]);
    run(&mut p[1], "frame.unwrap", json!({"frame": frame}));
    run(&mut p[0], "frame.add", json!({"frame": frame, "ids": [child]}));
    settle(&mut p);
    assert_converged_and_valid(&p);
    for s in &p {
        assert_eq!(element(s, &child).frame_id, None);
    }
}

#[test]
fn groups_that_disagree_about_their_outer_group_are_dissolved_the_same_way_everywhere() {
    let mut p = [peer("a", 1), peer("b", 2)];
    let ids: Vec<String> = (0..3).map(|i| rect(&mut p[0], f64::from(i) * 150.0, 0.0)).collect();
    let (a, b) = p.split_at_mut(1);
    send(&a[0], &mut b[0]);
    // What a peer with an older build, or a lost race, can send: the members of g1 do not share their outer group.
    let g = |n: &str| GroupId::new(n).unwrap();
    let eid = |s: &str| ElementId::new(s).unwrap();
    let doc = &mut p[0].tab_mut().doc;
    let mark = doc.mark();
    doc.edit::<(), drafftink_scene::StoreError>(|tx| {
        use drafftink_scene::SceneTx;
        tx.update(&eid(&ids[0]), |e| e.group_ids = vec![g("g1"), g("g2")].into())?;
        tx.update(&eid(&ids[1]), |e| e.group_ids = vec![g("g1")].into())?;
        tx.update(&eid(&ids[2]), |e| e.group_ids = vec![g("g2")].into())
    })
    .unwrap();
    let o = crate::state::Observed::default();
    doc.commit_since(
        mark,
        &crate::store::StepMeta { label: "Group", undoable: true, before: &o, after: &o, depth: 50, coalesce: None },
    )
    .unwrap();
    settle(&mut p);
    assert_converged_and_valid(&p);
    assert_eq!(element(&p[1], &ids[0]).group_ids.as_slice(), [g("g2")]);
    assert!(element(&p[1], &ids[1]).group_ids.is_empty());
}

#[test]
fn a_text_grouped_by_one_peer_and_bound_as_a_label_by_another_takes_the_groups_of_its_container() {
    // Found by the random test: the merge left a label in a group its container is not in, and `group.create` then
    // failed with a duplicate group id.
    let mut p = [peer("a", 1), peer("b", 2)];
    let container = rect(&mut p[0], 0.0, 0.0);
    let other = rect(&mut p[0], 300.0, 0.0);
    let text = id_of(&run(&mut p[0], "element.create", json!({"kind": "text", "text": "hi", "at": [300, 300]})));
    let (a, b) = p.split_at_mut(1);
    send(&a[0], &mut b[0]);
    run(&mut p[0], "group.create", json!({"ids": [text, other]}));
    run(&mut p[1], "text.bind", json!({"text": text, "container": container}));
    settle(&mut p);
    assert_converged_and_valid(&p);
    for s in &p {
        assert_eq!(element(s, &text).group_ids, element(s, &container).group_ids);
        assert_eq!(element(s, &text).container_id().map(ToString::to_string), Some(container.clone()));
    }
    run(&mut p[0], "group.create", json!({"ids": [container, other]}));
}

#[test]
fn a_view_only_peer_never_writes_a_repair() {
    let (mut p, _r1, _r2, arrow) = arrow_between_two_rects("sharp");
    let r3 = rect(&mut p[0], 0.0, 300.0);
    let (a, b) = p.split_at_mut(1);
    send(&a[0], &mut b[0]);
    run(&mut p[1], "element.delete", json!({"ids": [r3]}));
    run(&mut p[0], "arrow.bind", json!({"arrow": arrow, "end": "end", "target": r3}));
    p[0].view_only = true;
    let writes = p[0].tab().doc.local_writes();
    let (src, dst) = two(&mut p, 1, 0);
    send(src, dst);
    assert_eq!(p[0].tab().doc.local_writes(), writes, "a view-only peer writes nothing");
    let ElementKind::Arrow(d) = element(&p[0], &arrow).kind else { panic!("an arrow") };
    assert!(d.end_binding.is_some(), "it waits for a peer that can edit");
}

// ---- random edits on three peers ----------------------------------------------------------------------------

/// One random action of one peer. Targets are picked from what that peer sees at the time.
#[derive(Debug, Clone)]
struct Act {
    peer: usize,
    kind: u8,
    a: u8,
    b: u8,
    n: i8,
}

fn act() -> impl Strategy<Value = Act> {
    (0usize..3, 0u8..22, any::<u8>(), any::<u8>(), any::<i8>()).prop_map(|(peer, kind, a, b, n)| Act {
        peer,
        kind,
        a,
        b,
        n,
    })
}

/// The ids of the live elements that `keep` accepts, in z-order.
fn live_where(s: &Session, keep: impl Fn(&Element) -> bool) -> Vec<String> {
    s.store().iter().filter(|e| keep(e)).map(|e| e.id.to_string()).collect()
}

fn pick(ids: &[String], i: u8) -> Option<String> {
    (!ids.is_empty()).then(|| ids[usize::from(i) % ids.len()].clone())
}

/// Runs one action as a command. Targets of the right kind are picked from what that peer sees at the time. A
/// refusal is fine (the target may be locked, already bound, in the wrong state); an internal error, a panic or a
/// call with wrong arguments is not.
fn apply(s: &mut Session, act: &Act) {
    use drafftink_scene::Kind;
    let any = live_where(s, |_| true);
    let arrows = live_where(s, |e| e.tag().is_linear());
    let frames = live_where(s, |e| e.tag() == Kind::Frame);
    let texts = live_where(s, |e| e.tag() == Kind::Text);
    let labels = live_where(s, |e| e.container_id().is_some());
    let containers = live_where(s, |e| e.tag().is_label_container());
    let targets = live_where(s, |e| e.tag().is_bindable() && e.container_id().is_none());
    let grouped = live_where(s, |e| !e.group_ids.is_empty());
    let (x, y) = (pick(&any, act.a), pick(&any, act.b));
    let d = f64::from(act.n) * 3.0;
    let call: Option<(&str, Value)> = match act.kind {
        0 | 1 => Some((
            "element.create",
            json!({"kind": if act.kind == 0 { "rectangle" } else { "ellipse" }, "bounds": {"x": d * 2.0, "y": f64::from(act.a), "w": 80, "h": 50}}),
        )),
        2 => Some(("element.create", json!({"kind": "text", "text": "t", "at": [d, f64::from(act.b)]}))),
        3 | 4 => pick(&targets, act.a).zip(pick(&targets, act.b)).map(|(from, to)| {
            (
                "element.connect",
                json!({"from": from, "to": to, "arrow_type": if act.kind == 3 { "sharp" } else { "elbow" }}),
            )
        }),
        5 => pick(&arrows, act.a).zip(pick(&targets, act.b)).map(|(arrow, target)| {
            (
                "arrow.bind",
                json!({"arrow": arrow, "end": if act.n % 2 == 0 { "start" } else { "end" }, "target": target}),
            )
        }),
        6 => pick(&arrows, act.a).map(|arrow| ("arrow.unbind", json!({"arrow": arrow}))),
        7 | 8 => x.map(|x| ("element.move", json!({"ids": [x], "dx": d, "dy": -d / 2.0, "snap": "none"}))),
        9 => x.map(|x| ("element.resize", json!({"ids": [x], "handle": "se", "dx": d, "dy": d}))),
        10 | 11 => x.map(|x| ("element.delete", json!({"ids": [x]}))),
        12 | 13 => x.zip(y).map(|(x, y)| ("group.create", json!({"ids": [x, y]}))),
        14 => pick(&grouped, act.a).or(x).map(|x| ("group.ungroup", json!({"ids": [x]}))),
        15 => x.zip(y).map(|(x, y)| ("frame.wrap", json!({"ids": [x, y]}))),
        16 => pick(&frames, act.a).zip(y).map(|(f, y)| ("frame.add", json!({"frame": f, "ids": [y]}))),
        17 => y.map(|y| ("frame.remove", json!({"ids": [y]}))),
        18 => pick(&frames, act.a).map(|f| ("frame.unwrap", json!({"frame": f}))),
        19 => pick(&texts, act.a)
            .zip(pick(&containers, act.b))
            .map(|(t, c)| ("text.bind", json!({"text": t, "container": c}))),
        20 => pick(&labels, act.a).map(|t| ("text.unbind", json!({"text": t}))),
        _ => None,
    };
    let note = call.as_ref().map(|(id, args)| format!("{id} {args}")).unwrap_or_default();
    let dump: String = s
        .store()
        .iter_all()
        .map(|e| {
            format!(
                "{}{} g={:?} f={:?} c={:?} idx={}",
                e.id,
                if e.deleted { "x" } else { "" },
                e.group_ids,
                e.frame_id,
                e.container_id(),
                e.index
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let result = match call {
        Some((id, args)) => s.execute(id, args).map(|_| ()),
        None if act.a.is_multiple_of(2) => s.execute("history.undo", json!({})).map(|_| ()),
        None => s.execute("history.redo", json!({})).map(|_| ()),
    };
    if let Err(r) = result {
        assert_ne!(r.code, "internal", "{act:?} {note} failed inside the engine: {}\n{dump}", r.message);
        // A wrong argument is a mistake of this generator, and would hide the command from the test.
        assert_ne!(r.code, "bad_args", "{act:?} is not a valid call: {}", r.message);
    }
}

/// Four shapes, a text and an arrow that every peer starts from.
fn shared_start() -> [Session; 3] {
    let mut p = [peer("a", 11), peer("b", 22), peer("c", 33)];
    let ids: Vec<String> = (0..4).map(|i| rect(&mut p[0], f64::from(i) * 160.0, f64::from(i % 2) * 120.0)).collect();
    run(&mut p[0], "element.create", json!({"kind": "text", "text": "hello", "at": [20, 300]}));
    run(&mut p[0], "element.connect", json!({"from": ids[0], "to": ids[1]}));
    run(&mut p[0], "element.connect", json!({"from": ids[2], "to": ids[3], "arrow_type": "elbow"}));
    settle_from(&mut p, 0);
    p
}

/// Sends everything of peer `source` to the others.
fn settle_from(p: &mut [Session; 3], source: usize) {
    for to in (0..3).filter(|t| *t != source) {
        let (src, dst) = two(p, source, to);
        send(src, dst);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn random_concurrent_commands_on_three_peers_converge_to_a_valid_scene(
        acts in proptest::collection::vec(act(), 1..45),
        syncs in proptest::collection::vec((0usize..3, 0usize..3), 0..15),
    ) {
        let mut p = shared_start();
        let mut syncs = syncs.into_iter();
        for (i, a) in acts.iter().enumerate() {
            apply(&mut p[a.peer], a);
            if i % 3 == 2 && let Some((from, to)) = syncs.next() && from != to {
                let (src, dst) = two(&mut p, from, to);
                send(src, dst);
            }
        }
        settle(&mut p);
        assert_converged_and_valid(&p);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 120, ..ProptestConfig::default() })]

    /// The baseline of the test above: without a merge there is nothing to repair, so any invalid scene there is a
    /// bug of the merge repair or of a command, not of concurrency.
    #[test]
    fn one_peer_alone_is_valid_after_any_commands(acts in proptest::collection::vec(act(), 1..45)) {
        let mut p = shared_start();
        for a in &acts {
            apply(&mut p[0], a);
        }
        let truth = p[0].tab().doc.replicated();
        let bad: Vec<String> = validate_scene(truth)
            .issues
            .iter()
            .filter(|i| i.severity == Severity::Error || !tolerated(&i.error))
            .map(ToString::to_string)
            .collect();
        prop_assert!(bad.is_empty(), "{:?}\n{}", acts, bad.join("\n"));
        prop_assert!(binding::stale_arrows(truth).is_empty());
    }
}

#[test]
fn a_gesture_on_an_element_a_peer_deleted_is_cancelled_and_the_delete_stands() {
    let mut p = [peer("a", 1), peer("b", 2)];
    let r = rect(&mut p[0], 0.0, 0.0);
    let other = rect(&mut p[0], 300.0, 0.0);
    let (a, b) = p.split_at_mut(1);
    send(&a[0], &mut b[0]);
    let g = run(&mut p[1], "gesture.begin", json!({"command": "element.move", "args": {"ids": [r], "dx": 5, "dy": 0}}));
    let gid = g["gesture"].as_str().unwrap().to_owned();
    run(&mut p[1], "gesture.update", json!({"gesture": gid, "args": {"ids": [r], "dx": 20, "dy": 0}}));
    run(&mut p[0], "element.delete", json!({"ids": [r]}));
    let (a, b) = p.split_at_mut(1);
    send(&a[0], &mut b[0]);
    assert!(p[1].running_gesture().is_none(), "the gesture on the deleted element is over");
    let notices = p[1].take_collab_notices();
    assert!(
        matches!(&notices[..], [crate::cmd::collab::CollabNotice::Info(t)] if t.contains("Move was cancelled") && t.contains("deleted")),
        "{notices:?}"
    );
    let refusal = p[1].execute("gesture.end", json!({"gesture": gid})).unwrap_err();
    assert_eq!(refusal.code, "no_gesture");
    settle(&mut p);
    assert_converged_and_valid(&p);
    assert!(element(&p[1], &r).deleted, "the collaborator's delete is not undone by the gesture");
    assert_eq!(element(&p[1], &other).transform.x, 300.0);
    assert!(p[1].ring().iter().any(|e| e.id == "gesture.cancel" && e.source == Origin::Remote));
    // A gesture on something else goes on.
    let g =
        run(&mut p[1], "gesture.begin", json!({"command": "element.move", "args": {"ids": [other], "dx": 5, "dy": 0}}));
    let gid = g["gesture"].as_str().unwrap().to_owned();
    run(&mut p[0], "element.set_name", json!({"id": other, "name": "x"}));
    let (a, b) = p.split_at_mut(1);
    send(&a[0], &mut b[0]);
    assert!(p[1].running_gesture().is_some());
    run(&mut p[1], "gesture.end", json!({"gesture": gid}));
}
