//! fixtures-gen: regenerate (or check) tests/fixtures from the OLD drafftink-core.
//!
//!   fixtures-gen [--out DIR] [--check]
//!
//! DIR defaults to the parent of this crate (tests/fixtures). Hand-written inputs under
//! DIR/excalidraw and DIR/mermaid are read; everything the old code derives is written (or, with
//! --check, compared byte for byte with what is on disk).

mod canon;
mod facts;
mod fixtures;
mod util;

use drafftink_core::canvas::CanvasDocument;
use drafftink_core::crdt::CrdtDocument;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use util::{fmt_json, gzip, sha256_hex, Out, Res};

/// Canonical document file + facts file (+ optional gz for big ones).
fn emit_doc(out: &mut Out, dir: &str, name: &str, doc: &CanvasDocument, big: bool, with_crdt: bool) -> Res<CanvasDocument> {
    let text = canon::canonical_json(doc, name)?;
    // Facts come from the old loader reading the canonical file, so ids in facts are the file's ids.
    let loaded = CanvasDocument::from_json(&text)?;
    let doc_facts = facts::compute(&loaded, with_crdt)?;
    let src = if big { format!("{dir}/{name}.json.gz") } else { format!("{dir}/{name}.json") };
    let f = json!({
        "facts_version": facts::FACTS_VERSION,
        "oracle": {"repo": "drafft-ink", "commit": facts::OLD_COMMIT, "crate": "drafftink-core"},
        "source": src,
        "source_sha256": sha256_hex(text.as_bytes()),
        "source_sha256_note": "sha256 of the UNCOMPRESSED json bytes",
        "doc": doc_facts,
    });
    let facts_text = fmt_json(&f);
    if big {
        out.put(&src, gzip(text.as_bytes())?);
        out.put(&format!("{dir}/{name}.facts.json.gz"), gzip(facts_text.as_bytes())?);
    } else {
        out.put(&src, text.into_bytes());
        out.put(&format!("{dir}/{name}.facts.json"), facts_text.into_bytes());
    }
    Ok(loaded)
}

fn emit_crdt(out: &mut Out, name: &str, doc: &CanvasDocument) -> Res<()> {
    let mut crdt = CrdtDocument::new();
    crdt.loro_doc().set_peer_id(1)?;
    for s in doc.shapes_ordered() {
        crdt.add_shape(s)?;
    }
    crdt.set_name(&doc.name)?;
    let snap = crdt.export_snapshot();
    let deep = serde_json::to_value(crdt.loro_doc().get_deep_value())?;
    let back = CrdtDocument::from_snapshot(&snap)?;
    let back_ids: Vec<String> = back.shapes_ordered().iter().map(|s| s.id().to_string()).collect();
    let orig_ids: Vec<String> = doc.shapes_ordered().map(|s| s.id().to_string()).collect();
    if back_ids != orig_ids {
        return Err("crdt snapshot order differs".into());
    }
    out.put(&format!("legacy/crdt/{name}.loro"), snap.clone());
    let f = json!({
        "facts_version": facts::FACTS_VERSION,
        "oracle": {"repo": "drafft-ink", "commit": facts::OLD_COMMIT, "crate": "drafftink-core"},
        "source": format!("legacy/crdt/{name}.loro"),
        "source_sha256": sha256_hex(&snap),
        "built_from": format!("legacy/{name}.json"),
        "how": "CrdtDocument::new(), peer id 1, add_shape for each shape in z-order, set_name, export_snapshot",
        "name": crdt.name(),
        "z_order": crdt.z_order(),
        "shape_count": crdt.shape_count(),
        "deep_value": deep,
    });
    out.put(&format!("legacy/crdt/{name}.facts.json"), fmt_json(&f).into_bytes());
    Ok(())
}

fn read_inputs(root: &Path, sub: &str, ext: &str) -> Res<Vec<(String, Vec<u8>)>> {
    let dir = root.join(sub);
    let mut v = Vec::new();
    if !dir.is_dir() {
        return Ok(v);
    }
    for e in std::fs::read_dir(&dir)? {
        let path = e?.path();
        if path.is_file() && path.extension().and_then(|x| x.to_str()) == Some(ext) {
            let stem = path.file_stem().and_then(|x| x.to_str()).unwrap_or("").to_string();
            v.push((stem, std::fs::read(&path)?));
        }
    }
    v.sort();
    Ok(v)
}

fn generate(root: &Path) -> Res<(Out, BTreeMap<String, String>)> {
    let mut out = Out::default();
    let mut inputs = BTreeMap::new();

    // 1. Native documents.
    let mut kept: BTreeMap<String, CanvasDocument> = BTreeMap::new();
    let named: Vec<(&str, CanvasDocument)> = vec![
        ("all_shapes", fixtures::all_shapes()?),
        ("styles_rectangle", fixtures::styles_rectangle()),
        ("styles_ellipse", fixtures::styles_ellipse()),
        ("styles_line", fixtures::styles_line()),
        ("styles_arrow", fixtures::styles_arrow()),
        ("styles_freehand", fixtures::styles_freehand()),
        ("text_fonts", fixtures::text_fonts()),
        ("text_char_colors", fixtures::text_char_colors()),
        ("math", fixtures::math()),
        ("images", fixtures::images()?),
        ("groups_nested", fixtures::groups_nested()),
        ("z_order", fixtures::z_order()),
    ];
    for (name, doc) in &named {
        let loaded = emit_doc(&mut out, "legacy", name, doc, false, true)?;
        kept.insert(name.to_string(), loaded);
    }

    // 2. The welcome document: the shipped asset, byte for byte.
    let intro_path = std::env::var("OLD_REPO").unwrap_or_else(|_| "/home/ubuntu/git/drafft-ink".into());
    let intro = std::fs::read_to_string(Path::new(&intro_path).join("crates/drafftink-app/assets/intro.json"))?;
    let intro_doc = CanvasDocument::from_json(&intro)?;
    let f = json!({
        "facts_version": facts::FACTS_VERSION,
        "oracle": {"repo": "drafft-ink", "commit": facts::OLD_COMMIT, "crate": "drafftink-core"},
        "source": "legacy/welcome.json",
        "source_sha256": sha256_hex(intro.as_bytes()),
        "provenance": "crates/drafftink-app/assets/intro.json copied byte for byte (embedded by app.rs and loaded on first start)",
        "doc": facts::compute(&intro_doc, true)?,
    });
    out.put("legacy/welcome.json", intro.into_bytes());
    out.put("legacy/welcome.facts.json", fmt_json(&f).into_bytes());
    kept.insert("welcome".into(), intro_doc);

    // 3. Large documents.
    for (n, seed) in [(1000usize, 1000u64), (10000, 10000)] {
        let d = fixtures::large(n, seed);
        emit_doc(&mut out, "legacy", &format!("large_{n}"), &d, true, n <= 1000)?;
    }

    // 4. Loro snapshots of the old CRDT encoding.
    for name in ["all_shapes", "groups_nested", "styles_line", "text_char_colors", "images"] {
        if let Some(d) = kept.get(name) {
            emit_crdt(&mut out, name, d)?;
        }
    }

    // 5. Excalidraw inputs -> old importer output.
    let mut ex_summary = Vec::new();
    for (name, bytes) in read_inputs(root, "excalidraw", "excalidraw")? {
        inputs.insert(format!("excalidraw/{name}.excalidraw"), sha256_hex(&bytes));
        let text = String::from_utf8(bytes)?;
        let (doc, ok) = match CanvasDocument::from_excalidraw(&text) {
            Ok(d) => (d, true),
            Err(_) => (CanvasDocument::new(), false),
        };
        let src_elems: Vec<Value> = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v.get("elements").and_then(|e| e.as_array().cloned()))
            .unwrap_or_default();
        let type_counts = {
            let mut m: BTreeMap<String, usize> = BTreeMap::new();
            for e in &src_elems {
                *m.entry(e.get("type").and_then(Value::as_str).unwrap_or("?").to_string()).or_default() += 1;
            }
            m
        };
        if ok {
            emit_doc(&mut out, "excalidraw/expected", &name, &doc, false, false)?;
        }
        ex_summary.push(json!({"name": name, "parsed": ok, "source_element_types": type_counts, "imported_shapes": doc.len()}));
    }
    for (name, bytes) in read_inputs(root, "excalidraw", "excalidrawlib")? {
        inputs.insert(format!("excalidraw/{name}.excalidrawlib"), sha256_hex(&bytes));
        let text = String::from_utf8(bytes)?;
        let items = drafftink_core::library_from_excalidrawlib(&text);
        let listing = items.as_ref().map(|its| {
            its.iter()
                .map(|it| {
                    let b = it.shape.bounds();
                    json!({"name": it.name, "bounds": [b.x0, b.y0, b.x1, b.y1],
                           "children": it.shape.as_group().map(|g| g.children().len())})
                })
                .collect::<Vec<_>>()
        });
        let mut grid = CanvasDocument::new();
        if let Some(its) = &items {
            for s in drafftink_core::library_layout_grid(its) {
                grid.add_shape(s);
            }
        }
        if items.is_some() {
            emit_doc(&mut out, "excalidraw/expected", &format!("{name}.grid"), &grid, false, false)?;
        }
        let l = json!({"facts_version": facts::FACTS_VERSION, "source": format!("excalidraw/{name}.excalidrawlib"),
                       "recognized": items.is_some(), "items": listing});
        out.put(&format!("excalidraw/expected/{name}.items.json"), fmt_json(&l).into_bytes());
    }
    for (name, bytes) in read_inputs(root, "excalidraw/clipboard", "json")? {
        inputs.insert(format!("excalidraw/clipboard/{name}.json"), sha256_hex(&bytes));
        let text = String::from_utf8(bytes)?;
        let shapes = CanvasDocument::shapes_from_excalidraw_clipboard(&text);
        let mut doc = CanvasDocument::new();
        for s in shapes.clone().unwrap_or_default() {
            doc.add_shape(s);
        }
        if shapes.is_some() {
            emit_doc(&mut out, "excalidraw/expected", &format!("clipboard_{name}"), &doc, false, false)?;
        }
        let r = json!({"source": format!("excalidraw/clipboard/{name}.json"), "accepted": shapes.is_some()});
        out.put(&format!("excalidraw/expected/clipboard_{name}.result.json"), fmt_json(&r).into_bytes());
    }
    out.put("excalidraw/expected/summary.json", fmt_json(&json!({"files": ex_summary})).into_bytes());

    // 6. Mermaid inputs -> old importer output.
    let mut mm = Vec::new();
    for sub in ["mermaid", "mermaid/rejected"] {
        for (name, bytes) in read_inputs(root, sub, "mmd")? {
            let rel = format!("{sub}/{name}.mmd");
            inputs.insert(rel.clone(), sha256_hex(&bytes));
            let text = String::from_utf8(bytes)?;
            let shapes = drafftink_core::shapes_from_mermaid(&text);
            let mut doc = CanvasDocument::new();
            for s in shapes.clone().unwrap_or_default() {
                doc.add_shape(s);
            }
            if shapes.is_some() {
                emit_doc(&mut out, "mermaid/expected", &name, &doc, false, false)?;
            }
            mm.push(json!({"input": rel, "accepted": shapes.is_some()}));
        }
    }
    out.put("mermaid/expected/summary.json", fmt_json(&json!({"inputs": mm})).into_bytes());
    Ok((out, inputs))
}

fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().collect();
    let check = args.iter().any(|a| a == "--check");
    let root: PathBuf = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join(".."));
    let root = root.canonicalize().unwrap_or(root);
    let (mut out, inputs) = generate(&root)?;

    let mut files = serde_json::Map::new();
    for (k, v) in &out.files {
        files.insert(k.clone(), json!({"sha256": sha256_hex(v), "bytes": v.len()}));
    }
    let manifest = json!({
        "manifest_version": 1,
        "oracle_commit": facts::OLD_COMMIT,
        "generated": files,
        "inputs": inputs,
    });
    out.put("MANIFEST.json", fmt_json(&manifest).into_bytes());

    if check {
        let mut bad = Vec::new();
        for (rel, bytes) in &out.files {
            match std::fs::read(root.join(rel)) {
                Ok(on_disk) if &on_disk == bytes => {}
                Ok(_) => bad.push(format!("differs: {rel}")),
                Err(_) => bad.push(format!("missing: {rel}")),
            }
        }
        if bad.is_empty() {
            println!("fixtures up to date ({} files)", out.files.len());
            return Ok(());
        }
        for b in &bad {
            eprintln!("{b}");
        }
        return Err(format!("{} fixture file(s) out of date", bad.len()).into());
    }
    for (rel, bytes) in &out.files {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bytes)?;
    }
    println!("wrote {} files under {}", out.files.len(), root.display());
    Ok(())
}
