// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Generates the Anthropic wire types (T30.12) from the vendored OpenAPI 3.1
//! spec, `schema/anthropic-openapi.json`, so that file stays the single source
//! of truth: no extracted schema is committed and nothing can go stale.
//!
//! The spec describes the whole API (1300+ schemas); only the schemas
//! reachable from the Messages request body and the stream frames are fed to
//! typify. A few constructs are rewritten first, each for the reason given at
//! the rewrite. The result is written to `$OUT_DIR/anthropic_wire.rs`, which
//! `src/wire.rs` includes.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::path::PathBuf;
use std::{env, fs};

use serde_json::{Map, Value, json};
use typify::{TypeSpace, TypeSpaceSettings};

const SPEC: &str = "schema/anthropic-openapi.json";

/// `POST /v1/messages` body: what cox sends.
const REQUEST_ROOTS: &[&str] = &["CreateMessageParams"];

/// SSE frame bodies: what cox reads. `ErrorResponse` is the body of the
/// stream's `error` frame, which `MessageStreamEvent` does not list.
const RESPONSE_ROOTS: &[&str] = &["MessageStreamEvent", "ErrorResponse"];

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed={SPEC}");
    println!("cargo:rerun-if-changed=build.rs");

    let spec: Value = serde_json::from_str(&fs::read_to_string(SPEC)?)?;
    let schemas = spec
        .pointer("/components/schemas")
        .and_then(Value::as_object)
        .ok_or("the spec has no components.schemas object")?;

    let request = reachable(schemas, REQUEST_ROOTS)?;
    let response = reachable(schemas, RESPONSE_ROOTS)?;

    let mut flat = BTreeMap::new();
    for name in request.union(&response) {
        let mut schema = schemas
            .get(name.as_str())
            .cloned()
            .ok_or_else(|| format!("$ref to missing schema {name}"))?;
        replace_whole(name, &mut schema);
        clean(&mut schema, response.contains(name));
        flat.insert(name.clone(), schema);
    }
    hoist_member_properties(&mut flat)?;

    let mut defs = BTreeMap::new();
    for (name, schema) in &flat {
        let mut schema = schema.clone();
        inline_variants(&mut schema, &flat, &mut vec![name.clone()]);
        // The schema type is typify's own (schemars 0.8), inferred from
        // `add_ref_types` below rather than named, so this crate does not
        // depend on that schemars directly and the workspace stays on 1.x.
        let schema = serde_json::from_value(schema)
            .map_err(|e| format!("schema {name} is not a JSON Schema: {e}"))?;
        defs.insert(name.clone(), schema);
    }

    let mut types = TypeSpace::new(&TypeSpaceSettings::default());
    // typify resolves a `$ref` by its last path segment, so the spec's
    // `#/components/schemas/X` refs need no rewrite to `#/$defs/X`.
    types.add_ref_types(defs)?;

    let out = PathBuf::from(env::var("OUT_DIR")?).join("anthropic_wire.rs");
    fs::write(out, types.to_stream().to_string())?;
    Ok(())
}

/// Names of every schema reachable from `roots` through `$ref`s.
fn reachable(schemas: &Map<String, Value>, roots: &[&str]) -> Result<BTreeSet<String>, String> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<String> = roots.iter().map(|r| r.to_string()).collect();
    while let Some(name) = stack.pop() {
        if seen.contains(&name) {
            continue;
        }
        let schema = schemas
            .get(&name)
            .ok_or_else(|| format!("$ref to missing schema {name}"))?;
        collect_refs(schema, &mut stack);
        seen.insert(name);
    }
    Ok(seen)
}

fn collect_refs(v: &Value, out: &mut Vec<String>) {
    let Value::Object(map) = v else { return };
    if let Some(name) = ref_name(map) {
        out.push(name);
    }
    for child in subschemas_of(map) {
        collect_refs(child, out);
    }
}

/// `X` from `{"$ref": "#/components/schemas/X"}`.
fn ref_name(map: &Map<String, Value>) -> Option<String> {
    let r = map.get("$ref")?.as_str()?;
    Some(r.rsplit('/').next().unwrap_or(r).to_string())
}

/// The schema-valued children of one schema object. Walking only these keeps
/// every rewrite away from data that merely looks like a schema: `default`,
/// `examples`, `const` values, and property *names* such as `title`.
fn subschemas_of(map: &Map<String, Value>) -> Vec<&Value> {
    let mut out = Vec::new();
    if let Some(Value::Object(props)) = map.get("properties") {
        out.extend(props.values());
    }
    for k in ["items", "additionalProperties", "not"] {
        if let Some(v @ Value::Object(_)) = map.get(k) {
            out.push(v);
        }
    }
    for k in ["anyOf", "oneOf", "allOf", "prefixItems"] {
        if let Some(Value::Array(members)) = map.get(k) {
            out.extend(members.iter());
        }
    }
    out
}

fn subschemas_of_mut(map: &mut Map<String, Value>) -> Vec<&mut Value> {
    let mut out = Vec::new();
    for (k, v) in map.iter_mut() {
        match (k.as_str(), v) {
            ("properties", Value::Object(props)) => out.extend(props.values_mut()),
            ("items" | "additionalProperties" | "not", v @ Value::Object(_)) => out.push(v),
            ("anyOf" | "oneOf" | "allOf" | "prefixItems", Value::Array(members)) => {
                out.extend(members.iter_mut());
            }
            _ => {}
        }
    }
    out
}

fn replace_whole(name: &str, schema: &mut Value) {
    match name {
        // The known model ids are an `anyOf` of `string` and ~20 `const`s,
        // which typify turns into an untagged enum; cox sends any id the
        // user configured, so the id is just a string.
        "Model" => *schema = json!({"type": "string"}),
        // A tool's `input_schema` is an arbitrary JSON Schema cox forwards
        // byte for byte; a typed struct would drop every keyword the spec
        // does not list (`description`, `additionalProperties`, `$defs`, ...)
        // and reorder the rest.
        "InputSchema" => *schema = json!({}),
        _ => {}
    }
}

fn clean(v: &mut Value, response_side: bool) {
    let Value::Object(map) = v else { return };
    // typify 0.8 names an inline type after its `title` and silently reuses
    // the first type it saw under a name (`TypeSpace::assign_type`), so the
    // spec's many different inline schemas titled `Content`, `Source`,
    // `Caller`, ... would all collapse into one wrong type. Without titles
    // it names them after their parent type and property, which is unique.
    map.remove("title");
    // String constraints make typify emit a validated newtype with a
    // fallible constructor (and a `regress` dependency for `pattern`) for
    // every id and name; the API validates them and cox never needs to
    // reject its own values client-side.
    for k in ["pattern", "minLength", "maxLength"] {
        map.remove(k);
    }
    // `date-time`/`date` become `chrono` types and `uuid` a `uuid` type: two
    // runtime dependencies for timestamps and ids cox never reads.
    map.remove("format");
    if response_side {
        relax_response(map);
    }
    for child in subschemas_of_mut(map) {
        clean(child, response_side);
    }
}

/// Stream frames must survive additive and rolling API changes: a frame that
/// omits a counter or carries a new enum value is still a frame cox can use.
fn relax_response(map: &mut Map<String, Value>) {
    // A closed string enum (stop reason, refusal category, service tier)
    // would fail the whole frame on a value added after this snapshot;
    // stream.rs compares the strings it knows and treats the rest as before.
    if map.get("type").and_then(Value::as_str) == Some("string") {
        map.remove("enum");
    }
    // A property stays required only when it is an object payload cox
    // dispatches on (`message`, `content_block`, `delta`, `usage`, `error`)
    // and has no `default`. Scalars and arrays (`id`, `index`, `content`,
    // token counters) and anything the spec gives a default may be absent:
    // hand-written fixtures omit them, and T30.10 made a missing one `None`,
    // not a parse error. A union member's tag is required again by
    // `inline_variants`.
    let keep: Vec<Value> = {
        let (Some(Value::Object(props)), Some(Value::Array(required))) =
            (map.get("properties"), map.get("required"))
        else {
            return;
        };
        required
            .iter()
            .filter(|r| {
                r.as_str()
                    .and_then(|r| props.get(r))
                    .and_then(Value::as_object)
                    .is_some_and(|p| {
                        !p.contains_key("default")
                            && (p.contains_key("$ref") || p.contains_key("oneOf"))
                    })
            })
            .cloned()
            .collect()
    };
    map.insert("required".into(), Value::Array(keep));
}

/// Moves the inline property types of every union member into defs of their
/// own, `{Member}_{property}`. typify names an inline type after its parent
/// and property, and inside a tagged enum the parent is the enum: once
/// `inline_variants` puts the members side by side, the `content` of
/// `web_search_tool_result` and of `tool_result` would both be named
/// `InputContentBlockContent`, and typify keeps whichever came first
/// (`TypeSpace::assign_type`) -- a wrong type, not a compile error. As defs
/// they are named after the member, which is unique.
fn hoist_member_properties(flat: &mut BTreeMap<String, Value>) -> Result<(), String> {
    let mut members = BTreeSet::new();
    for schema in flat.values() {
        union_members(schema, &mut members);
    }
    let mut hoisted = BTreeMap::new();
    for name in &members {
        let Some(Value::Object(schema)) = flat.get_mut(name) else {
            continue;
        };
        let Some(Value::Object(props)) = schema.get_mut("properties") else {
            continue;
        };
        for (prop, value) in props.iter_mut() {
            hoist(value, format!("{name}_{prop}"), &mut hoisted);
        }
    }
    for (name, schema) in hoisted {
        if flat.insert(name.clone(), schema).is_some() {
            return Err(format!("hoisted type {name} collides with a spec schema"));
        }
    }
    Ok(())
}

/// Names of the `$ref` members of every discriminated `oneOf` in `v`.
fn union_members(v: &Value, out: &mut BTreeSet<String>) {
    let Value::Object(map) = v else { return };
    if let (Some(_), Some(Value::Array(members))) = (map.get("discriminator"), map.get("oneOf"))
        && members.len() > 1
    {
        out.extend(
            members
                .iter()
                .filter_map(|m| m.as_object().and_then(ref_name)),
        );
    }
    for child in subschemas_of(map) {
        union_members(child, out);
    }
}

/// Replaces `v` with a `$ref` to a new def `name` when typify would give it
/// a named type (an object, enum or union); looks through `T | null`,
/// one-member unions and arrays, which typify renders as `Option<T>`, `T`
/// and `Vec<T>` without a name of their own.
/// A `const` stays inline: it is the tag typify must see on the member.
fn hoist(v: &mut Value, name: String, out: &mut BTreeMap<String, Value>) {
    let Value::Object(map) = v else { return };
    if map.contains_key("$ref") || map.contains_key("const") {
        return;
    }
    if let Some(Value::Array(any)) = map.get_mut("anyOf") {
        let is_null = |m: &Value| m.get("type").and_then(Value::as_str) == Some("null");
        if any.len() == 2 && any.iter().any(is_null) {
            if let Some(inner) = any.iter_mut().find(|m| !is_null(m)) {
                hoist(inner, name, out);
            }
            return;
        }
    }
    for k in ["oneOf", "anyOf"] {
        if let Some(Value::Array(only)) = map.get_mut(k)
            && let [inner] = only.as_mut_slice()
        {
            hoist(inner, name, out);
            return;
        }
    }
    if map.get("type").and_then(Value::as_str) == Some("array") {
        if let Some(items) = map.get_mut("items") {
            hoist(items, format!("{name}_item"), out);
        }
        return;
    }
    let named = ["anyOf", "oneOf", "allOf", "enum", "properties"]
        .iter()
        .any(|k| map.contains_key(*k));
    if named {
        let schema =
            std::mem::replace(v, json!({ "$ref": format!("#/components/schemas/{name}") }));
        out.insert(name, schema);
    }
}

/// Replaces each `$ref` member of a discriminated `oneOf` (two or more
/// members) with the referenced schema, and marks the discriminator
/// property required in that copy. typify 0.8 only recognises an internally
/// tagged union when it can see every member's fixed-value, required tag
/// property, and it does not follow a `$ref` to look; left alone, every
/// union (content blocks, deltas, tool choice, thinking config) becomes
/// `#[serde(untagged)]`, which deserializes a block as the first variant
/// whose fields happen to fit. `visiting` leaves a cyclic `$ref` as is.
fn inline_variants(v: &mut Value, flat: &BTreeMap<String, Value>, visiting: &mut Vec<String>) {
    let Value::Object(map) = v else { return };
    let tag = map
        .get("discriminator")
        .and_then(|d| d.get("propertyName"))
        .and_then(Value::as_str)
        .map(str::to_string);
    if let (Some(tag), Some(Value::Array(members))) = (tag, map.get_mut("oneOf"))
        && members.len() > 1
    {
        for member in members.iter_mut() {
            let Some(name) = member.as_object().and_then(ref_name) else {
                inline_variants(member, flat, visiting);
                continue;
            };
            let Some(target) = flat.get(&name) else {
                continue;
            };
            if visiting.contains(&name) {
                continue;
            }
            let mut copy = target.clone();
            if let Value::Object(obj) = &mut copy {
                require(obj, &tag);
            }
            visiting.push(name);
            inline_variants(&mut copy, flat, visiting);
            visiting.pop();
            *member = copy;
        }
        // The members are done; walk the rest without revisiting them.
        let members = map.remove("oneOf");
        for child in subschemas_of_mut(map) {
            inline_variants(child, flat, visiting);
        }
        if let Some(members) = members {
            map.insert("oneOf".into(), members);
        }
        return;
    }
    // A `$ref` outside a union is typify's to resolve, not ours.
    for child in subschemas_of_mut(map) {
        inline_variants(child, flat, visiting);
    }
}

fn require(obj: &mut Map<String, Value>, prop: &str) {
    let required = obj
        .entry("required")
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Value::Array(list) = required
        && !list.iter().any(|r| r.as_str() == Some(prop))
    {
        list.push(Value::String(prop.to_string()));
    }
}
