//! The command line as a process: arguments in, files and streams out, and the exit code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_drafftink-headless")).args(args).output().unwrap()
}

fn cli_stdin(args: &[&str], input: &str) -> Output {
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_drafftink-headless"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}

const BOXES: &str = r#"{"id":"element.create","args":{"kind":"rectangle","bounds":{"x":0,"y":0,"w":120,"h":60},"name":"first"}}
{"id":"element.create","args":{"kind":"ellipse","bounds":{"x":240,"y":0,"w":120,"h":60},"name":"second"}}
"#;

fn script(dir: &Path, body: &str) -> String {
    let path = dir.join("script.jsonl");
    std::fs::write(&path, body).unwrap();
    p(&path).to_owned()
}

#[test]
fn run_executes_each_line_prints_one_result_per_step_and_writes_the_output() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.drafftink");
    let o = cli(&["run", &script(dir.path(), BOXES), "--out", p(&out)]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let lines: Vec<Value> = text(&o.stdout).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 2);
    assert!(lines.iter().all(|l| l["ok"] == true));
    assert_eq!((lines[0]["step"].clone(), lines[1]["id"].clone()), (1.into(), "element.create".into()));
    assert_eq!(lines[0]["result"]["created"].as_array().unwrap().len(), 1);
    // The written file is a document: its outline lists both shapes.
    let o = cli(&["outline", p(&out)]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let outline = text(&o.stdout);
    assert!(outline.contains("first") && outline.contains("second"), "{outline}");
}

#[test]
fn nothing_is_written_without_out_and_nothing_when_a_step_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.drafftink");
    let bad = format!("{BOXES}{{\"id\":\"element.delete\",\"args\":{{\"ids\":[\"nope\"]}}}}\n");
    let o = cli(&["run", &script(dir.path(), &bad), "--out", p(&out)]);
    assert_eq!(o.status.code(), Some(1));
    assert!(!out.exists(), "a refused run must not write");
    let err = text(&o.stderr);
    assert!(err.contains("step 3 refused") && err.contains("No element matches `nope`"), "{err}");
    // The refusal is also on stdout as data, with its code.
    let last: Value = serde_json::from_str(text(&o.stdout).lines().last().unwrap()).unwrap();
    assert_eq!((last["ok"].clone(), last["refusal"]["code"].clone()), (false.into(), "not_found".into()));
    // Without --out a good run only prints.
    let o = cli(&["run", &script(dir.path(), BOXES)]);
    assert!(o.status.success());
    assert!(!out.exists());
}

#[test]
fn keep_going_runs_the_steps_after_a_refusal_and_still_fails() {
    let dir = tempfile::tempdir().unwrap();
    let bad = format!("{{\"id\":\"element.delete\",\"args\":{{\"ids\":[\"nope\"]}}}}\n{BOXES}");
    let o = cli(&["run", &script(dir.path(), &bad), "--keep-going"]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(text(&o.stdout).lines().count(), 3);
    assert!(text(&o.stderr).contains("1 step(s) refused"));
}

#[test]
fn a_batch_is_one_transaction_with_names_and_an_atomic_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.drafftink");
    let batch = r#"{"steps":[
        {"id":"element.create","args":{"kind":"rectangle","bounds":{"x":0,"y":0,"w":100,"h":50}},"as":"a"},
        {"id":"element.move","args":{"ids":["$a"],"dx":10,"dy":0}}
    ]}"#;
    let o = cli(&["run", &script(dir.path(), batch), "--out", p(&out)]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let result: Value = serde_json::from_str(text(&o.stdout).trim()).unwrap();
    assert_eq!(result["failed"], Value::Null);
    assert_eq!(result["rolled_back"], false);
    // Plain steps become a batch with --batch, which accepts `as`; without it `as` is explained.
    let steps = r#"{"id":"element.create","args":{"kind":"rectangle","bounds":{"x":0,"y":0,"w":100,"h":50}},"as":"a"}"#;
    let o = cli(&["run", &script(dir.path(), steps)]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("--batch"), "{}", text(&o.stderr));
    assert!(cli(&["run", &script(dir.path(), steps), "--batch"]).status.success());
    // A failing step rolls the whole batch back: the output file is not written, the exit code is 1.
    let failing = r#"{"steps":[
        {"id":"element.create","args":{"kind":"rectangle","bounds":{"x":0,"y":0,"w":100,"h":50}}},
        {"id":"element.delete","args":{"ids":["nope"]}}]}"#;
    let out2 = dir.path().join("never.drafftink");
    let o = cli(&["run", &script(dir.path(), failing), "--out", p(&out2)]);
    assert_eq!(o.status.code(), Some(1));
    assert!(!out2.exists());
    let result: Value = serde_json::from_str(text(&o.stdout).trim()).unwrap();
    assert_eq!(result["rolled_back"], true, "{result}");
    assert!(text(&o.stderr).contains("step 2 refused"), "{}", text(&o.stderr));
}

#[test]
fn a_script_can_come_from_standard_input_and_start_from_a_document() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first.drafftink");
    assert!(cli(&["run", &script(dir.path(), BOXES), "--out", p(&first)]).status.success());
    let second = dir.path().join("second.drafftink");
    let o = cli_stdin(
        &["run", "-", "--doc", p(&first), "--out", p(&second)],
        "{\"id\":\"element.create\",\"args\":{\"kind\":\"diamond\",\"bounds\":{\"x\":0,\"y\":200,\"w\":80,\"h\":80}}}",
    );
    assert!(o.status.success(), "{}", text(&o.stderr));
    let o = cli(&["inspect", p(&second)]);
    let summary: Value = serde_json::from_str(&text(&o.stdout)).unwrap();
    assert_eq!(summary["elements"], 3, "{summary}");
}

#[test]
fn convert_goes_between_native_excalidraw_and_svg() {
    let dir = tempfile::tempdir().unwrap();
    let native = dir.path().join("a.drafftink");
    assert!(cli(&["run", &script(dir.path(), BOXES), "--out", p(&native)]).status.success());
    let excalidraw = dir.path().join("a.excalidraw");
    let o = cli(&["convert", p(&native), p(&excalidraw)]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let scene: Value = serde_json::from_str(&std::fs::read_to_string(&excalidraw).unwrap()).unwrap();
    assert_eq!(scene["type"], "excalidraw");
    assert_eq!(scene["elements"].as_array().unwrap().len(), 2);
    // And back, by extension and content.
    let back = dir.path().join("b.drafftink");
    assert!(cli(&["convert", p(&excalidraw), p(&back)]).status.success());
    let summary: Value = serde_json::from_str(&text(&cli(&["inspect", p(&back)]).stdout)).unwrap();
    assert_eq!(summary["elements"], 2);
    let svg = dir.path().join("a.svg");
    assert!(cli(&["convert", p(&native), p(&svg)]).status.success());
    let svg = std::fs::read_to_string(svg).unwrap();
    assert!(svg.starts_with("<svg") || svg.contains("<svg"), "{}", &svg[..svg.len().min(80)]);
}

#[test]
fn mermaid_converts_with_subgraph_boxes_and_bound_arrows() {
    let dir = tempfile::tempdir().unwrap();
    let mmd = dir.path().join("d.mmd");
    std::fs::write(&mmd, "graph TD\nsubgraph one[Group one]\nA[Start]-->B{Ok?}\nend\nB-->|yes| C[Done]\n").unwrap();
    let out = dir.path().join("d.drafftink");
    let o = cli(&["convert", p(&mmd), p(&out)]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let o = cli(&["outline", p(&out)]);
    let outline = text(&o.stdout);
    for want in ["Start", "Ok?", "Done", "Group one"] {
        assert!(outline.contains(want), "{want} missing:\n{outline}");
    }
    let svg = dir.path().join("d.svg");
    assert!(cli(&["convert", p(&mmd), p(&svg), "--from", "mermaid"]).status.success());
}

#[test]
fn png_says_it_needs_a_gpu_before_it_does_anything() {
    let dir = tempfile::tempdir().unwrap();
    let native = dir.path().join("a.drafftink");
    assert!(cli(&["run", &script(dir.path(), BOXES), "--out", p(&native)]).status.success());
    let png = dir.path().join("a.png");
    let o = cli(&["convert", p(&native), p(&png)]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("needs a GPU"), "{}", text(&o.stderr));
    assert!(!png.exists());
    let o = cli(&["run", &script(dir.path(), BOXES), "--out", p(&png)]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("needs a GPU"));
    assert!(o.stdout.is_empty(), "nothing ran");
    // A script that exports PNG itself gets the engine's refusal.
    let export = r#"{"id":"doc.export","args":{"format":"png"}}"#;
    let o = cli(&["run", &script(dir.path(), &format!("{BOXES}{export}\n"))]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("renderer"), "{}", text(&o.stderr));
}

#[test]
fn outline_prints_text_or_json() {
    let dir = tempfile::tempdir().unwrap();
    let native = dir.path().join("a.drafftink");
    assert!(cli(&["run", &script(dir.path(), BOXES), "--out", p(&native)]).status.success());
    let text_form = text(&cli(&["outline", p(&native)]).stdout);
    assert!(text_form.lines().count() >= 2, "{text_form}");
    let json_form: Value = serde_json::from_str(&text(&cli(&["outline", p(&native), "--json"]).stdout)).unwrap();
    assert_eq!(json_form["nodes"].as_array().unwrap().len(), 2, "{json_form}");
}

#[test]
fn schemas_prints_one_table_for_every_command_that_the_web_app_fetches() {
    let o = cli(&["schemas"]);
    assert_eq!(o.status.code(), Some(0));
    let table: Value = serde_json::from_str(&text(&o.stdout)).unwrap();
    let commands = table["commands"].as_object().unwrap();
    let listed = text(&cli(&["commands"]).stdout).lines().filter(|l| l.contains('.')).count();
    assert_eq!(commands.len(), listed, "one entry per command");
    let create = &commands["element.create"];
    assert!(create["args"]["properties"].is_object() && create["output"].is_object());
    assert_eq!(text(&o.stdout).trim_end().lines().count(), 1, "one line, so a build script can hash and compress it");
}

#[test]
fn commands_lists_the_registry_and_describes_one() {
    let all = text(&cli(&["commands"]).stdout);
    assert!(all.lines().count() > 100, "the whole registry is listed");
    assert!(all.contains("element.create") && all.contains("diagram.import_mermaid"));
    let diagram = text(&cli(&["commands", "--family", "diagram"]).stdout);
    assert_eq!(diagram.lines().count(), 3, "{diagram}");
    let found = text(&cli(&["commands", "--search", "mermaid"]).stdout);
    assert!(found.contains("diagram.import_mermaid") && !found.contains("element.create"), "{found}");
    let json: Value = serde_json::from_str(&text(&cli(&["commands", "--family", "diagram", "--json"]).stdout)).unwrap();
    let first = &json["commands"][0];
    assert!(first["args_schema"].is_object() && first["output_schema"].is_object(), "schemas are included");
    let one: Value = serde_json::from_str(&text(&cli(&["commands", "element.create"]).stdout)).unwrap();
    assert_eq!(one["id"], "element.create");
    assert!(one["example"].is_object() && one["refusals"].is_array());
    let o = cli(&["commands", "no.such"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("no.such"), "{}", text(&o.stderr));
}

#[test]
fn mistakes_in_the_input_have_plain_messages_and_the_right_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let o = cli(&["run", p(&dir.path().join("missing.jsonl"))]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("could not read"), "{}", text(&o.stderr));
    let o = cli(&["run", &script(dir.path(), "not json\n")]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("line 1"), "{}", text(&o.stderr));
    let o = cli(&["convert", p(&dir.path().join("x.drafftink")), p(&dir.path().join("y.txt"))]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("--to"), "{}", text(&o.stderr));
    let junk = dir.path().join("junk.drafftink");
    std::fs::write(&junk, "{ this is not a document").unwrap();
    let o = cli(&["outline", p(&junk)]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("not a drafft.ink or Excalidraw document"), "{}", text(&o.stderr));
    // Arguments that do not fit together are clap's: exit code 2.
    assert_eq!(cli(&["run", "x", "--dry-run"]).status.code(), Some(2));
    assert_eq!(cli(&[]).status.code(), Some(2));
}

#[test]
fn a_fixed_seed_makes_the_same_ids_and_another_seed_makes_other_ids() {
    let dir = tempfile::tempdir().unwrap();
    let path = script(dir.path(), BOXES);
    let write = |seed: &str, name: &str| {
        let out = dir.path().join(name);
        let o = cli(&["run", &path, "--seed", seed, "--out", p(&out)]);
        assert!(o.status.success(), "{}", text(&o.stderr));
        std::fs::read_to_string(out).unwrap()
    };
    assert_eq!(write("5", "a.drafftink"), write("5", "b.drafftink"));
    assert_ne!(write("5", "c.drafftink"), write("6", "d.drafftink"));
}

#[test]
fn comment_lines_in_a_script_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let o = cli(&["run", &script(dir.path(), &format!("// two shapes\n{BOXES}// done\n"))]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert_eq!(text(&o.stdout).lines().count(), 2);
}
