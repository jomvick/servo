use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{EgoError, ErrorCode};

/// Ce que l'agent demande : une représentation texte de la page, bornée en taille.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct SnapshotOptions {
    pub max_chars: usize,
    pub interactive_only: bool,
}

impl Default for SnapshotOptions {
    fn default() -> Self {
        Self {
            max_chars: 12_000,
            interactive_only: false,
        }
    }
}

/// Une cible adressable par réf. `reference` est la forme composée `{frame}:{élément}`,
/// celle que l'agent renvoie dans `click`/`fill`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct RefEntry {
    pub reference: String,
    pub role: String,
    pub name: String,
    pub frame: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct Snapshot {
    pub title: String,
    pub url: String,
    pub text: String,
    pub refs: Vec<RefEntry>,
}

/// Le JSON produit par le walker injecté (`webview-bridge/assets/ego-preload.js`) :
/// `{title, url, frames: [{path, depth, nodes: [...], timeout?}]}`, chaque nœud
/// `{role, name, ref?, value?, unreadable?, children?}`.
pub fn parse_snapshot(raw: &Value, options: &SnapshotOptions) -> Result<Snapshot, EgoError> {
    let frames = frames_of(raw)?;
    let mut lines: Vec<String> = Vec::new();
    let mut refs: Vec<RefEntry> = Vec::new();

    for (i, frame) in frames.iter().enumerate() {
        let path = string_field(frame, "path").ok_or_else(|| {
            EgoError::new(
                ErrorCode::SnapshotFailed,
                format!("frame {i} carries no `path`, its refs would be unaddressable"),
            )
        })?;
        let depth = frame
            .get("depth")
            .and_then(Value::as_u64)
            .and_then(|depth| usize::try_from(depth).ok())
            .unwrap_or(0);
        let indent = "  ".repeat(depth);

        if frame.get("timeout").and_then(Value::as_bool) == Some(true) {
            lines.push(format!("{indent}[frame timeout] {path}"));
            continue;
        }

        let Some(nodes) = frame.get("nodes").and_then(Value::as_array) else {
            continue;
        };
        walk(nodes, &path, depth, options, &mut lines, &mut refs);
    }

    let title = string_field(raw, "title").unwrap_or_default();
    let url = string_field(raw, "url").unwrap_or_default();
    let text = render(&title, &url, &lines, options.max_chars);
    Ok(Snapshot {
        title,
        url,
        text,
        refs,
    })
}

fn walk(
    roots: &[Value],
    frame: &str,
    depth: usize,
    options: &SnapshotOptions,
    lines: &mut Vec<String>,
    refs: &mut Vec<RefEntry>,
) {
    let mut stack: Vec<(&Value, usize)> = roots.iter().rev().map(|node| (node, depth)).collect();
    while let Some((node, node_depth)) = stack.pop() {
        let role = string_field(node, "role").unwrap_or_else(|| "text".to_owned());
        let name = sanitize(&string_field(node, "name").unwrap_or_default());
        let unreadable = node.get("unreadable").and_then(Value::as_bool) == Some(true);
        let reference = string_field(node, "ref").map(|element| format!("{frame}:{element}"));
        let actionable = reference.is_some();

        if actionable {
            refs.push(RefEntry {
                reference: reference.clone().unwrap_or_default(),
                role: role.clone(),
                name: name.clone(),
                frame: frame.to_owned(),
            });
        }

        // Omettre la ligne ne doit pas aussi omettre les contrôles descendants.
        if !options.interactive_only || actionable || unreadable {
            let indent = "  ".repeat(node_depth);
            let mut line = format!("{indent}- {role} \"{name}\"");
            if unreadable {
                line.push_str(" [unreadable]");
            }
            if let Some(reference) = &reference {
                line.push_str(&format!(" [ref={reference}]"));
            }
            if let Some(value) = string_field(node, "value") {
                line.push_str(&format!(" value=\"{}\"", sanitize(&value)));
            }
            lines.push(line);
        }

        if let Some(children) = node.get("children").and_then(Value::as_array) {
            stack.extend(children.iter().rev().map(|child| (child, node_depth + 1)));
        }
    }
}

/// L'en-tête n'est pas compté dans `max_chars` : c'est quelques dizaines de caractères, et
/// couper le titre d'une page pour tenir un budget produirait un snapshot sans identifiant.
fn render(title: &str, url: &str, lines: &[String], max_chars: usize) -> String {
    let mut text = format!("# {title}\n{url}\n");
    let mut used = 0usize;
    let mut dropped = false;
    for line in lines {
        let cost = line.chars().count() + 1;
        if used + cost > max_chars {
            dropped = true;
            break;
        }
        used += cost;
        text.push_str(line);
        text.push('\n');
    }
    // Marquer la coupe : un agent qui lit les dernières lignes doit savoir que la page
    // continue, sinon il conclut à une fin de document.
    if dropped {
        text.push_str("…\n");
    }
    text
}

fn frames_of(raw: &Value) -> Result<&[Value], EgoError> {
    let frames = raw.get("frames").and_then(Value::as_array).ok_or_else(|| {
        EgoError::new(
            ErrorCode::SnapshotFailed,
            "walker output has no `frames` array",
        )
    })?;
    Ok(frames.as_slice())
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Une ligne par nœud : tout retour chariot d'un texte de page casserait le format.
fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(if c == '"' { '\'' } else { c });
    }
    out
}
