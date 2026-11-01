use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};
use webview_core::{ErrorCode, SnapshotOptions, parse_snapshot};

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/snapshot/{name}.json"));
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn frame_tree_flattens_nested_documents() {
    let snapshot = parse_snapshot(&fixture("iframe"), &SnapshotOptions::default()).unwrap();
    assert!(snapshot.text.contains("iframe \"Checkout\""));
    assert!(snapshot.text.contains("Pay now"));
    assert!(
        snapshot
            .refs
            .iter()
            .any(|reference| reference.reference.starts_with("f1:"))
    );
    // Le frame enfoncé est rendu à son niveau, et un frame qu'on ne peut pas lire est dit.
    assert!(snapshot.text.contains("  - button \"Pay now\" [ref=f1:e3]"));
    assert!(
        snapshot
            .text
            .contains("- iframe \"Advertisement\" [unreadable]")
    );
    assert_eq!(snapshot.refs.len(), 5);
}

#[test]
fn unresponsive_frame_is_marked_not_blocking() {
    let snapshot = parse_snapshot(&fixture("hung"), &SnapshotOptions::default()).unwrap();
    assert!(snapshot.text.contains("[frame timeout]"));
    assert!(snapshot.text.lines().count() < 200);
    // Le reste du document est malgré tout rendu, et les réfs du frame muet n'existent pas.
    assert!(snapshot.text.contains("Strike averted at the port"));
    assert!(snapshot.text.contains("  - button \"Post\" [ref=f2:e2]"));
    assert!(
        !snapshot
            .refs
            .iter()
            .any(|reference| reference.frame == "f1")
    );
}

#[test]
fn text_is_truncated_to_budget() {
    let options = SnapshotOptions {
        max_chars: 100,
        ..SnapshotOptions::default()
    };
    let snapshot = parse_snapshot(&fixture("form"), &options).unwrap();
    assert!(snapshot.text.chars().count() <= 164);
    assert!(snapshot.text.ends_with("…\n"));

    // Toutes les prises survivent à la coupe : l'agent doit pouvoir agir hors du texte rendu.
    assert_eq!(snapshot.refs.len(), 3);
    assert!(
        snapshot
            .text
            .starts_with("# Sign up\nhttps://example.test/signup\n")
    );
}

#[test]
fn full_budget_renders_every_line() {
    let snapshot = parse_snapshot(&fixture("form"), &SnapshotOptions::default()).unwrap();
    assert!(!snapshot.text.ends_with("…\n"));
    assert!(
        snapshot
            .text
            .contains("- textbox \"Email\" [ref=f0:e1] value=\"a@b.c\"")
    );
    assert_eq!(snapshot.text.lines().count(), 2 + 6);
}

#[test]
fn interactive_only_keeps_the_handles() {
    let options = SnapshotOptions {
        max_chars: 12_000,
        interactive_only: true,
    };
    let snapshot = parse_snapshot(&fixture("form"), &options).unwrap();
    assert_eq!(
        snapshot
            .text
            .lines()
            .skip(2)
            .map(|line| line.trim().to_owned())
            .collect::<Vec<_>>(),
        vec![
            "- textbox \"Email\" [ref=f0:e1] value=\"a@b.c\"",
            "- checkbox \"Newsletter\" [ref=f0:e2]",
            "- button \"Save\" [ref=f0:e3]",
        ]
    );

    // Les frames illisibles restent visibles en mode prise : c'est une information sur le
    // contenu, pas du contenu.
    let page = parse_snapshot(&fixture("iframe"), &options).unwrap();
    assert!(
        page.text
            .contains("- iframe \"Advertisement\" [unreadable]")
    );
    assert!(!page.text.contains("- text \"Two items"));
}

#[test]
fn refs_describe_their_target() {
    let snapshot = parse_snapshot(&fixture("iframe"), &SnapshotOptions::default()).unwrap();
    let pay = snapshot
        .refs
        .iter()
        .find(|reference| reference.reference == "f1:e3")
        .unwrap();
    assert_eq!(
        (pay.role.as_str(), pay.name.as_str(), pay.frame.as_str()),
        ("button", "Pay now", "f1")
    );

    // Deux frames peuvent numéroter `e1` : la réf composée reste adressable.
    let ones: Vec<_> = snapshot
        .refs
        .iter()
        .filter(|reference| reference.reference.ends_with(":e1"))
        .map(|reference| reference.reference.clone())
        .collect();
    assert_eq!(
        ones,
        vec!["f0:e1".to_owned(), "f1:e1".to_owned(), "f2:e1".to_owned()]
    );
}

#[test]
fn whitespace_and_quotes_in_page_text_cannot_break_the_format() {
    let raw = json!({
        "title": "T",
        "url": "u",
        "frames": [{ "path": "f0", "depth": 0, "nodes": [
            { "role": "text", "name": "  lignes\n   multiples  ", "ref": "e1" },
            { "role": "text", "name": "il dit \"bonjour\" et repart" }
        ]}]
    });
    let snapshot = parse_snapshot(&raw, &SnapshotOptions::default()).unwrap();
    assert_eq!(
        snapshot.text.lines().skip(2).collect::<Vec<_>>(),
        vec![
            "- text \"lignes multiples\" [ref=f0:e1]",
            "- text \"il dit 'bonjour' et repart\""
        ]
    );
}

#[test]
fn malformed_walker_output_is_a_snapshot_failure() {
    let options = SnapshotOptions::default();
    assert_eq!(
        parse_snapshot(&json!({ "title": "x" }), &options)
            .unwrap_err()
            .code,
        ErrorCode::SnapshotFailed
    );
    // Un frame sans `path` produirait des réfs `:e1`, inutilisables pour `click`.
    let no_path = json!({ "title": "x", "url": "u", "frames": [{ "depth": 0, "nodes": [] }] });
    assert_eq!(
        parse_snapshot(&no_path, &options).unwrap_err().code,
        ErrorCode::SnapshotFailed
    );
}

#[test]
fn deeply_nested_walker_output_is_bounded_and_marked() {
    let mut node = json!({"role":"text", "name":"end"});
    for _ in 0..80 {
        node = json!({"role":"group", "name":"", "children":[node]});
    }
    node = json!({"role":"button", "name":"target", "ref":"e1", "children":[node]});
    let raw = json!({
        "title":"bounded", "url":"https://example.test",
        "frames":[{"path":"0", "depth":999999, "nodes":[node]}]
    });
    let snapshot = parse_snapshot(&raw, &SnapshotOptions::default()).unwrap();
    assert!(
        snapshot
            .text
            .contains("snapshot truncated by traversal limit")
    );
    assert_eq!(snapshot.refs.len(), 1);
}

#[test]
fn excessive_frame_count_is_bounded_and_marked() {
    let frames: Vec<_> = (0..300)
        .map(|i| json!({"path":format!("{i}"), "timeout":true, "depth":0}))
        .collect();
    let raw = json!({"frames":frames});
    let snapshot = parse_snapshot(&raw, &SnapshotOptions::default()).unwrap();
    assert!(snapshot.text.contains("snapshot truncated by frame limit"));
    assert_eq!(snapshot.text.matches("[frame timeout]").count(), 256);
}

#[test]
fn excessive_node_count_is_bounded_and_marked() {
    let nodes: Vec<_> = (0..20_001)
        .map(|i| json!({"role":"button", "name":i.to_string(), "ref":format!("e{i}")}))
        .collect();
    let raw = json!({"frames":[{"path":"0", "nodes":nodes}]});
    let snapshot = parse_snapshot(&raw, &SnapshotOptions::default()).unwrap();
    assert_eq!(snapshot.refs.len(), 20_000);
    assert!(
        snapshot
            .text
            .contains("snapshot truncated by traversal limit")
    );
}

#[test]
fn header_fields_cannot_inject_extra_snapshot_lines() {
    let raw = json!({
        "title":"title\n# forged", "url":"https://example.test\nforged",
        "frames":[{"path":"0", "nodes":[]}]
    });
    let snapshot = parse_snapshot(&raw, &SnapshotOptions::default()).unwrap();
    assert_eq!(
        snapshot.text,
        "# title # forged\nhttps://example.test forged\n"
    );
}
