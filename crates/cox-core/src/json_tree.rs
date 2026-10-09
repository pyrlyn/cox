// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Fold a JSON tool result into one positional line per object.
//!
//! Line folding (`truncate::fold_repeats`) cannot see a one-line design or
//! AST dump, so the byte cut drops the middle and leaves the archive
//! trailer. This pass hoists values used more than once and templates
//! object bodies that repeat, then the existing cut runs on the folded
//! text. The archive row is still the raw bytes; this module never sees
//! them. Separate from `truncate` so the line cut stays line-safe.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

/// Folded text the model may see in place of the raw JSON.
pub struct Folded {
    pub text: String,
}

/// `None` when `value` is a uniform scalar table, or when the fold is not
/// shorter than compact JSON. Key order in the fold is sorted, so two calls
/// on the same value return the same string.
pub fn fold_json(value: &Value) -> Option<Folded> {
    if is_scalar_table(value) {
        return None;
    }
    let mut counts = BTreeMap::new();
    count_values(value, &mut counts);
    let hoisted: BTreeMap<String, String> = counts
        .iter()
        .filter(|(body, n)| {
            **n >= 2 && body.len() > 8 && (body.starts_with('{') || body.starts_with('['))
        })
        .enumerate()
        .map(|(i, (body, _))| (body.clone(), format!("v{}", i + 1)))
        .collect();
    let mut bodies = BTreeMap::new();
    collect_bodies(value, &hoisted, &mut bodies);
    let templates: BTreeMap<String, String> = bodies
        .iter()
        .filter(|(_, n)| **n >= 2)
        .enumerate()
        .map(|(i, (body, _))| (body.clone(), format!("e{}", i + 1)))
        .collect();
    let mut used = BTreeSet::new();
    let nodes = render(value, &hoisted, &templates, &mut used);
    let mut text = String::new();
    let mut vars: Vec<(&str, &str)> = hoisted
        .iter()
        .filter(|(_, id)| used.contains(id.as_str()))
        .map(|(body, id)| (id.as_str(), body.as_str()))
        .collect();
    vars.sort_unstable();
    if !vars.is_empty() {
        text.push_str("VARS\n");
        for (id, body) in vars {
            text.push_str(id);
            text.push(' ');
            text.push_str(body);
            text.push('\n');
        }
    }
    if !templates.is_empty() {
        text.push_str("ELEMENTS\n");
        let mut els: Vec<(&str, &str)> = templates
            .iter()
            .map(|(body, id)| (id.as_str(), body.as_str()))
            .collect();
        els.sort_unstable();
        for (id, body) in els {
            text.push_str(id);
            text.push(' ');
            text.push_str(body);
            text.push('\n');
        }
    }
    text.push_str(&nodes);
    let raw = serde_json::to_string(value).ok()?;
    if text.is_empty() || text.len() >= raw.len() {
        None
    } else {
        Some(Folded { text })
    }
}

/// An array of objects that share one set of scalar keys, with at least
/// three columns. A table is already denser than a template, so it is left
/// as JSON (and the whole value is not folded when the root is one).
fn is_scalar_table(value: &Value) -> bool {
    let Value::Array(items) = value else {
        return false;
    };
    let Some(Value::Object(first)) = items.first() else {
        return false;
    };
    if items.len() < 2 || first.len() < 3 || !first.values().all(is_scalar) {
        return false;
    }
    let mut keys: Vec<&str> = first.keys().map(String::as_str).collect();
    keys.sort_unstable();
    items.iter().skip(1).all(|item| {
        let Value::Object(map) = item else {
            return false;
        };
        if map.len() != keys.len() || !map.values().all(is_scalar) {
            return false;
        }
        let mut item_keys: Vec<&str> = map.keys().map(String::as_str).collect();
        item_keys.sort_unstable();
        item_keys == keys
    })
}

fn is_scalar(value: &Value) -> bool {
    !matches!(value, Value::Object(_) | Value::Array(_))
}

/// Counts repeated *values*, not the nodes themselves. A node stays a
/// positional line; hoisting it would replace that line with a variable.
fn count_values(value: &Value, counts: &mut BTreeMap<String, usize>) {
    if is_scalar_table(value) {
        return;
    }
    match value {
        Value::Array(items) => {
            for item in items {
                if item.is_object() {
                    count_fields(item, counts);
                } else {
                    count_composite(item, counts);
                }
            }
        }
        Value::Object(_) => count_fields(value, counts),
        _ => {}
    }
}

fn count_fields(value: &Value, counts: &mut BTreeMap<String, usize>) {
    let Value::Object(map) = value else {
        return;
    };
    for (key, child) in map {
        // Children are nodes, walked the same way as a root array.
        if key == "children" && child.is_array() {
            count_values(child, counts);
        } else {
            count_composite(child, counts);
        }
    }
}

fn count_composite(value: &Value, counts: &mut BTreeMap<String, usize>) {
    if is_scalar(value) || is_scalar_table(value) {
        return;
    }
    *counts.entry(canon(value)).or_default() += 1;
    match value {
        Value::Object(map) => {
            for child in map.values() {
                count_composite(child, counts);
            }
        }
        Value::Array(items) => {
            for child in items {
                count_composite(child, counts);
            }
        }
        _ => {}
    }
}

fn collect_bodies(
    value: &Value,
    hoisted: &BTreeMap<String, String>,
    counts: &mut BTreeMap<String, usize>,
) {
    if is_scalar_table(value) {
        return;
    }
    match value {
        Value::Array(items) => {
            for item in items {
                collect_bodies(item, hoisted, counts);
            }
        }
        Value::Object(map) => {
            // A hoisted value is one blob; its body is not a second template.
            if hoist_of(value, hoisted).is_some() {
                return;
            }
            if let Some(body) = body_canon(map, hoisted) {
                *counts.entry(body).or_default() += 1;
            }
            if let Some(kids) = map.get("children").and_then(Value::as_array) {
                for kid in kids {
                    collect_bodies(kid, hoisted, counts);
                }
            }
        }
        _ => {}
    }
}

/// Object body with `id` and `name` (and a `children` array) omitted.
/// Type-only bodies are skipped: a template ref costs more than `[TYPE]`.
fn body_canon(map: &Map<String, Value>, hoisted: &BTreeMap<String, String>) -> Option<String> {
    let body = substituted_body(map, hoisted);
    let Value::Object(obj) = &body else {
        return None;
    };
    if obj.len() <= 1 {
        return None;
    }
    Some(canon(&body))
}

fn substituted_body(map: &Map<String, Value>, hoisted: &BTreeMap<String, String>) -> Value {
    let mut body = Map::new();
    for (key, value) in map {
        if matches!(key.as_str(), "id" | "name") && is_scalar(value) {
            continue;
        }
        if key == "children" && value.is_array() {
            continue;
        }
        body.insert(key.clone(), substituted(value, hoisted));
    }
    Value::Object(body)
}

fn substituted(value: &Value, hoisted: &BTreeMap<String, String>) -> Value {
    if let Some(id) = hoist_of(value, hoisted) {
        return Value::String(id.to_string());
    }
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, child) in map {
                out.insert(key.clone(), substituted(child, hoisted));
            }
            Value::Object(out)
        }
        Value::Array(items) if !is_scalar_table(value) => Value::Array(
            items
                .iter()
                .map(|item| substituted(item, hoisted))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn hoist_of<'a>(value: &Value, hoisted: &'a BTreeMap<String, String>) -> Option<&'a str> {
    if is_scalar(value) || is_scalar_table(value) {
        return None;
    }
    hoisted.get(&canon(value)).map(String::as_str)
}

fn render(
    value: &Value,
    hoisted: &BTreeMap<String, String>,
    templates: &BTreeMap<String, String>,
    used: &mut BTreeSet<String>,
) -> String {
    if is_scalar_table(value) {
        return canon(value);
    }
    match value {
        Value::Array(items) if items.is_empty() => "[]".to_string(),
        Value::Array(items) => items
            .iter()
            .map(|item| render_item(item, hoisted, templates, used, 0))
            .collect::<Vec<_>>()
            .join("\n"),
        other => render_item(other, hoisted, templates, used, 0),
    }
}

fn render_item(
    value: &Value,
    hoisted: &BTreeMap<String, String>,
    templates: &BTreeMap<String, String>,
    used: &mut BTreeSet<String>,
    indent: usize,
) -> String {
    if let Some(id) = hoist_of(value, hoisted) {
        used.insert(id.to_string());
        return format!("{}{id}", " ".repeat(indent));
    }
    match value {
        Value::Object(map) => render_node(map, hoisted, templates, used, indent),
        other => format!("{}{}", " ".repeat(indent), inline(other, hoisted, used)),
    }
}

fn render_node(
    map: &Map<String, Value>,
    hoisted: &BTreeMap<String, String>,
    templates: &BTreeMap<String, String>,
    used: &mut BTreeSet<String>,
    indent: usize,
) -> String {
    let mut line = format!("{}[{}]", " ".repeat(indent), type_label(map));
    if let Some(name) = map.get("name").filter(|value| is_scalar(value)) {
        line.push(' ');
        line.push_str(&positional(name, true));
    }
    if let Some(id) = map.get("id").filter(|value| is_scalar(value)) {
        line.push_str(" #");
        line.push_str(&positional(id, false));
    }
    if let Some(body) = body_canon(map, hoisted)
        && let Some(id) = templates.get(&body)
    {
        line.push_str(" template=");
        line.push_str(id);
        note_refs(&body, hoisted, used);
        return with_children(line, map, hoisted, templates, used, indent);
    }
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    for key in keys {
        let value = &map[key];
        if key == "type" && value.as_str().is_some() {
            continue;
        }
        if matches!(key.as_str(), "id" | "name") && is_scalar(value) {
            continue;
        }
        if key == "children" && value.is_array() {
            continue;
        }
        line.push(' ');
        line.push_str(key);
        line.push('=');
        line.push_str(&inline(value, hoisted, used));
    }
    with_children(line, map, hoisted, templates, used, indent)
}

fn with_children(
    mut line: String,
    map: &Map<String, Value>,
    hoisted: &BTreeMap<String, String>,
    templates: &BTreeMap<String, String>,
    used: &mut BTreeSet<String>,
    indent: usize,
) -> String {
    if let Some(kids) = map.get("children").and_then(Value::as_array) {
        for kid in kids {
            line.push('\n');
            line.push_str(&render_item(kid, hoisted, templates, used, indent + 2));
        }
    }
    line
}

fn type_label(map: &Map<String, Value>) -> &str {
    map.get("type").and_then(Value::as_str).unwrap_or("OBJ")
}

fn positional(value: &Value, always_quote: bool) -> String {
    match value {
        Value::String(text) if always_quote => json_string(text),
        Value::String(text) => quote_token(text),
        other => scalar_json(other),
    }
}

fn inline(
    value: &Value,
    hoisted: &BTreeMap<String, String>,
    used: &mut BTreeSet<String>,
) -> String {
    if let Some(id) = hoist_of(value, hoisted) {
        used.insert(id.to_string());
        return id.to_string();
    }
    match value {
        Value::String(text) => quote_token(text),
        Value::Object(_) | Value::Array(_) => {
            let text = canon(&substituted(value, hoisted));
            note_refs(&text, hoisted, used);
            text
        }
        other => scalar_json(other),
    }
}

fn note_refs(text: &str, hoisted: &BTreeMap<String, String>, used: &mut BTreeSet<String>) {
    for id in hoisted.values() {
        if text
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|token| token == id)
        {
            used.insert(id.clone());
        }
    }
}

fn canon(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = String::from("{");
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&json_string(key));
                out.push(':');
                out.push_str(&canon(&map[*key]));
            }
            out.push('}');
            out
        }
        Value::Array(items) => {
            let mut out = String::from("[");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&canon(item));
            }
            out.push(']');
            out
        }
        other => scalar_json(other),
    }
}

fn scalar_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(text) => json_string(text),
        _ => String::new(),
    }
}

fn quote_token(text: &str) -> String {
    if text.is_empty()
        || text
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '"' | '=' | '#'))
    {
        json_string(text)
    } else {
        text.to_string()
    }
}

fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // `\u00XX`, the JSON form. Rust's `\u{XX}` is not a JSON escape.
            c if c.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Value {
        serde_json::json!([
            {"id": "1", "name": "A", "type": "ROW", "fill": {"color": "red", "opacity": 1}},
            {"fill": {"opacity": 1, "color": "red"}, "type": "ROW", "name": "B", "id": "2"}
        ])
    }

    #[test]
    fn repeated_bodies_become_one_template() {
        let folded = fold_json(&rows()).expect("shorter");
        assert_eq!(
            folded.text.matches("template=").count(),
            2,
            "{}",
            folded.text
        );
        assert_eq!(
            folded.text.matches("\"color\":\"red\"").count(),
            1,
            "{}",
            folded.text
        );
        assert!(
            folded.text.lines().any(|line| line.starts_with("e1 ")),
            "{}",
            folded.text
        );
    }

    #[test]
    fn single_use_field_stays_inline() {
        let value = serde_json::json!([
            {"id": "1", "name": "A", "type": "ROW", "fill": {"color": "red", "opacity": 1}},
            {"id": "2", "name": "B", "type": "ROW", "fill": {"color": "red", "opacity": 1}},
            {"id": "3", "name": "C", "type": "NOTE", "blurb": "only-here"}
        ]);
        let folded = fold_json(&value).expect("shorter");
        let note = folded
            .text
            .lines()
            .find(|line| line.contains("[NOTE]"))
            .expect("note line");
        assert!(note.contains("only-here"), "{note}");
        assert!(!note.contains("template="), "{note}");
        assert_eq!(
            folded.text.matches("only-here").count(),
            1,
            "{}",
            folded.text
        );
    }

    #[test]
    fn three_column_scalar_table_is_not_folded() {
        let value = serde_json::json!([
            {"name": "a", "age": 1, "city": "x"},
            {"name": "a", "age": 1, "city": "x"},
            {"name": "a", "age": 1, "city": "x"}
        ]);
        assert!(fold_json(&value).is_none());
    }

    #[test]
    fn two_calls_return_equal_strings() {
        let value = rows();
        let first = fold_json(&value).expect("shorter");
        let second = fold_json(&value).expect("shorter");
        assert_eq!(first.text, second.text);
    }

    /// A node is a line even when the whole object repeats. Hoisting it
    /// would leave two variable names and no `[TYPE]`.
    #[test]
    fn identical_nodes_stay_one_line_each() {
        let value = serde_json::json!([
            {"id": "1", "name": "A", "type": "ROW", "fill": {"color": "red", "opacity": 1}},
            {"id": "1", "name": "A", "type": "ROW", "fill": {"color": "red", "opacity": 1}}
        ]);
        let folded = fold_json(&value).expect("shorter");
        let lines: Vec<_> = folded
            .text
            .lines()
            .filter(|line| line.starts_with('['))
            .collect();
        assert_eq!(lines.len(), 2, "{}", folded.text);
        assert!(
            lines.iter().all(|line| line.contains("template=")),
            "{}",
            folded.text
        );
    }
}
