#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Highlight in the shared document, and fields written by a newer build.

mod common;

use common::*;
use drafftink_doc::{DocError, LoroStore};
use drafftink_scene::*;
use loro::{ExportMode, LoroDoc};

/// A second writer that shares `store`'s history and writes raw Loro, as a newer build would.
struct Newer {
    doc: LoroDoc,
}

impl Newer {
    fn of(store: &LoroStore) -> Self {
        let doc = LoroDoc::new();
        doc.set_peer_id(777).unwrap();
        doc.import(&store.export_snapshot().unwrap()).unwrap();
        Self { doc }
    }

    fn element(&self, id: &str) -> loro::LoroMap {
        self.doc.get_map("elements").ensure_mergeable_map(id).unwrap()
    }

    fn send(&self, to: &mut LoroStore) -> Result<ChangeSet, DocError> {
        self.doc.commit();
        let from = loro::VersionVector::decode(&to.version_vector().unwrap()).unwrap();
        let bytes = self.doc.export(ExportMode::updates(&from)).unwrap();
        to.apply_remote(&bytes)
    }
}

#[test]
fn a_highlight_syncs_between_peers_and_clearing_it_syncs_too() {
    let (mut a, mut b) = (peer(1), peer(2));
    add(&mut a, vec![rect("r", &zs(1)[0])]);
    sync(&mut a, &mut b);
    edit(&mut a, "r", |e| e.style.highlight = Some(MarkerColor::Orange));
    push(&a, &mut b);
    assert_eq!(b.get(&eid("r")).unwrap().style.highlight, Some(MarkerColor::Orange));
    assert_view_matches_loro(&b);
    edit(&mut b, "r", |e| e.style.highlight = None);
    push(&b, &mut a);
    assert_eq!(a.get(&eid("r")).unwrap().style.highlight, None);
    let json = a.debug_json().to_string();
    assert!(!json.contains("style.highlight"), "a cleared highlight is not stored: {json}");
}

#[test]
fn the_highlight_of_a_run_is_a_mark_that_syncs_and_moves_with_its_characters() {
    let (mut a, mut b) = (peer(1), peer(2));
    add(&mut a, vec![text("t", &zs(1)[0], "one two three")]);
    sync(&mut a, &mut b);
    let runs = vec![
        TextRun::new(4, 7, CharStyle { highlight: SpanHighlight::On(MarkerColor::Green), ..CharStyle::default() }),
        TextRun::new(8, 13, CharStyle { highlight: SpanHighlight::Off, bold: true, ..CharStyle::default() }),
    ];
    edit(&mut a, "t", |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.runs = runs.clone();
        }
    });
    push(&a, &mut b);
    let ElementKind::Text(t) = &b.get(&eid("t")).unwrap().kind else { panic!("text") };
    assert_eq!(t.runs, runs);
    // Typing in front of the words shifts the marks with them.
    edit(&mut b, "t", |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.content = format!("so {}", t.content);
            t.runs = TextData::runs_from_char_styles(
                &[vec![CharStyle::default(); 3], t.char_styles()].concat(),
                t.content.chars().count(),
            );
        }
    });
    push(&b, &mut a);
    let ElementKind::Text(t) = &a.get(&eid("t")).unwrap().kind else { panic!("text") };
    assert_eq!(t.runs[0].start, 7);
    assert_eq!(t.runs[0].highlight, SpanHighlight::On(MarkerColor::Green));
}

#[test]
fn an_element_with_fields_of_a_newer_build_is_shown_and_its_fields_survive_an_edit() {
    let mut s = peer(1);
    add(&mut s, vec![rect("fine", &zs(1)[0])]);
    let n = Newer::of(&s);
    let m = n.element("new");
    m.insert("kind", "rectangle").unwrap();
    for k in ["x", "y", "w", "h"] {
        m.insert(k, 5.0).unwrap();
    }
    m.insert("index", "a5").unwrap();
    m.insert("style.highlight", "pink").unwrap();
    m.insert("style.glow", 3i64).unwrap();
    m.insert("kind.sparkle.level", 2i64).unwrap();
    n.send(&mut s).unwrap();

    let shown = s.get(&eid("new")).expect("shown, not quarantined");
    assert_eq!(shown.style.highlight, Some(MarkerColor::Pink));
    assert_eq!(s.invalid_elements().count(), 0);
    assert_eq!(s.newer_element_count(), 1);

    // An edit of the whole style does not delete what the newer build wrote.
    edit(&mut s, "new", |e| {
        e.style.stroke_width = 4.0;
        e.style.opacity = 0.5;
    });
    let json = s.debug_json().to_string();
    assert!(json.contains("style.glow") && json.contains("kind.sparkle.level"), "kept: {json}");
    assert_eq!(s.get(&eid("new")).unwrap().style.stroke_width, 4.0);

    // The newer peer sees its field and the edit, and this build still has none of its own to show.
    let mut peer_b = peer(3);
    peer_b.apply_remote(&s.export_snapshot().unwrap()).unwrap();
    assert_eq!(peer_b.get(&eid("new")).unwrap().style.opacity, 0.5);
    assert_eq!(peer_b.newer_element_count(), 1);
}
