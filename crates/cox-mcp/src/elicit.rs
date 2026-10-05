// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! MCP elicitation (T47.1, plan.md A78): the pure mapping between an rmcp
//! `ElicitationSchema` and the one-question-at-a-time prompts a surface's
//! question modal shows — the schema as ordered fields, each typed answer
//! back to a JSON value the field accepts, and the review step the spec
//! asks for before anything is sent. Separate from `client` because the
//! handler and its tests share it, and it holds no rmcp service type and
//! does no I/O. [`run_form`] (T47.2) drives those prompts through an
//! [`Asker`], the channel a surface with a person present answers;
//! [`run_url`] (T47.4) asks consent before a server's URL is opened.

use rmcp::model::{
    ElicitResult, ElicitationAction, ElicitationSchema, EnumSchema, MultiSelectEnumSchema,
    PrimitiveSchemaDefinition, SingleSelectEnumSchema, StringFormat,
};
use serde_json::{Map, Value};
use tokio::sync::{mpsc, oneshot};

/// The string shapes the spec names; checked by hand, no new crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Email,
    Uri,
    Date,
    DateTime,
}

/// What one field accepts. Enum options are `(const, label)` pairs: the
/// person sees the label, the server gets the const.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Text {
        format: Option<Format>,
        min_len: Option<u32>,
        max_len: Option<u32>,
    },
    Number {
        integer: bool,
        min: Option<f64>,
        max: Option<f64>,
    },
    Bool,
    One(Vec<(Value, String)>),
    Many {
        options: Vec<(Value, String)>,
        min: Option<u64>,
        max: Option<u64>,
    },
}

/// One property of the requested schema, as one question.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub key: String,
    pub label: String,
    pub help: Option<String>,
    pub kind: Kind,
    pub required: bool,
    pub default: Option<Value>,
}

/// The schema's fields in its property order. `Err` names a property whose
/// shape this rmcp version knows but cox does not, so the caller declines
/// rather than send a form the server cannot read.
pub fn fields(schema: &ElicitationSchema) -> Result<Vec<Field>, String> {
    let required = schema.required.as_deref().unwrap_or_default();
    let mut order: Vec<&String> = Vec::new();
    for key in schema
        .property_order
        .iter()
        .flatten()
        .chain(schema.properties.keys())
    {
        if schema.properties.contains_key(key) && !order.contains(&key) {
            order.push(key);
        }
    }
    order
        .into_iter()
        .filter_map(|key| schema.properties.get(key).map(|def| (key, def)))
        .map(|(key, def)| field(key, def, required.contains(key)))
        .collect()
}

fn field(key: &str, def: &PrimitiveSchemaDefinition, required: bool) -> Result<Field, String> {
    let owned = |s: Option<&str>| s.map(str::to_string);
    let (title, help, kind, default) = match def {
        PrimitiveSchemaDefinition::String(s) => (
            owned(s.title.as_deref()),
            owned(s.description.as_deref()),
            Kind::Text {
                format: s.format.as_ref().and_then(format_of),
                min_len: s.min_length,
                max_len: s.max_length,
            },
            s.default.clone().map(Value::String),
        ),
        PrimitiveSchemaDefinition::Number(n) => (
            owned(n.title.as_deref()),
            owned(n.description.as_deref()),
            Kind::Number {
                integer: false,
                min: n.minimum,
                max: n.maximum,
            },
            n.default
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number),
        ),
        PrimitiveSchemaDefinition::Integer(i) => (
            owned(i.title.as_deref()),
            owned(i.description.as_deref()),
            Kind::Number {
                integer: true,
                min: i.minimum.map(|v| v as f64),
                max: i.maximum.map(|v| v as f64),
            },
            i.default.map(Value::from),
        ),
        PrimitiveSchemaDefinition::Boolean(b) => (
            owned(b.title.as_deref()),
            owned(b.description.as_deref()),
            Kind::Bool,
            b.default.map(Value::Bool),
        ),
        PrimitiveSchemaDefinition::Enum(e) => enum_field(e)?,
        _ => return Err(format!("field `{key}` has a type cox cannot ask for")),
    };
    Ok(Field {
        key: key.to_string(),
        label: title.unwrap_or_else(|| key.to_string()),
        help,
        kind,
        required,
        default,
    })
}

type Parts = (Option<String>, Option<String>, Kind, Option<Value>);

fn enum_field(e: &EnumSchema) -> Result<Parts, String> {
    let owned = |s: Option<&str>| s.map(str::to_string);
    let plain = |values: &[String]| -> Vec<(Value, String)> {
        values
            .iter()
            .map(|v| (Value::String(v.clone()), v.clone()))
            .collect()
    };
    let strings = |values: &Option<Vec<String>>| {
        values
            .as_ref()
            .map(|v| Value::Array(v.iter().cloned().map(Value::String).collect()))
    };
    Ok(match e {
        EnumSchema::Single(SingleSelectEnumSchema::Untitled(s)) => (
            owned(s.title.as_deref()),
            owned(s.description.as_deref()),
            Kind::One(plain(&s.enum_)),
            s.default.clone().map(Value::String),
        ),
        EnumSchema::Single(SingleSelectEnumSchema::Titled(s)) => (
            owned(s.title.as_deref()),
            owned(s.description.as_deref()),
            Kind::One(
                s.one_of
                    .iter()
                    .map(|o| (Value::String(o.const_.clone()), o.title.clone()))
                    .collect(),
            ),
            s.default.clone().map(Value::String),
        ),
        EnumSchema::Multi(MultiSelectEnumSchema::Untitled(s)) => (
            owned(s.title.as_deref()),
            owned(s.description.as_deref()),
            Kind::Many {
                options: plain(&s.items.enum_),
                min: s.min_items,
                max: s.max_items,
            },
            strings(&s.default),
        ),
        EnumSchema::Multi(MultiSelectEnumSchema::Titled(s)) => (
            owned(s.title.as_deref()),
            owned(s.description.as_deref()),
            Kind::Many {
                options: s
                    .items
                    .any_of
                    .iter()
                    .map(|o| (Value::String(o.const_.clone()), o.title.clone()))
                    .collect(),
                min: s.min_items,
                max: s.max_items,
            },
            strings(&s.default),
        ),
        // The pre-2025-11-25 shape: `enumNames` label `enum` by position.
        EnumSchema::Legacy(s) => {
            let names = s.enum_names.as_deref().unwrap_or_default();
            let options = s
                .enum_
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let label = names.get(i).cloned().unwrap_or_else(|| v.clone());
                    (Value::String(v.clone()), label)
                })
                .collect();
            (
                owned(s.title.as_deref()),
                owned(s.description.as_deref()),
                Kind::One(options),
                s.default.clone().map(Value::String),
            )
        }
        _ => return Err("an enum field has a shape cox cannot ask for".into()),
    })
}

/// A format this rmcp version names but cox does not check is accepted as
/// plain text: the server still validates what it receives.
fn format_of(f: &StringFormat) -> Option<Format> {
    match f {
        StringFormat::Email => Some(Format::Email),
        StringFormat::Uri => Some(Format::Uri),
        StringFormat::Date => Some(Format::Date),
        StringFormat::DateTime => Some(Format::DateTime),
        _ => None,
    }
}

/// The question and the options the modal shows for `field`. One line, so
/// it fits the modal's header; the modal labels which server asks.
pub fn prompt(message: &str, field: &Field) -> (String, Vec<String>) {
    let mut question = format!("{message} — {}", field.label);
    if let Some(help) = &field.help {
        question.push_str(&format!(" ({help})"));
    }
    if matches!(field.kind, Kind::Many { .. }) {
        question.push_str(" (comma-separated)");
    }
    match &field.default {
        Some(default) => question.push_str(&format!(
            " (default: {}, Enter keeps it)",
            shown(&field.kind, default)
        )),
        None if !field.required => question.push_str(" (optional, Enter skips it)"),
        None => {}
    }
    let options = match &field.kind {
        Kind::Bool => vec!["yes".to_string(), "no".to_string()],
        Kind::One(options) | Kind::Many { options, .. } => {
            options.iter().map(|(_, label)| label.clone()).collect()
        }
        Kind::Text { .. } | Kind::Number { .. } => Vec::new(),
    };
    (question, options)
}

/// `value` as the person would type it: enum consts as their labels.
fn shown(kind: &Kind, value: &Value) -> String {
    let label = |options: &[(Value, String)], v: &Value| {
        options
            .iter()
            .find(|(c, _)| c == v)
            .map_or_else(|| plain(v), |(_, l)| l.clone())
    };
    match (kind, value) {
        (Kind::Bool, Value::Bool(b)) => (if *b { "yes" } else { "no" }).to_string(),
        (Kind::One(options), v) => label(options.as_slice(), v),
        (Kind::Many { options, .. }, Value::Array(items)) => items
            .iter()
            .map(|v| label(options.as_slice(), v))
            .collect::<Vec<_>>()
            .join(", "),
        (_, v) => plain(v),
    }
}

fn plain(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(plain).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

/// The person's answer to `field` as the value the server gets. `Ok(None)`
/// leaves an optional field out; `Err` is the reason to ask again.
pub fn parse(field: &Field, answer: &str) -> Result<Option<Value>, String> {
    let text = answer.trim();
    if text.is_empty() {
        return match (&field.default, field.required) {
            (Some(default), _) => Ok(Some(default.clone())),
            (None, false) => Ok(None),
            (None, true) => Err(format!("{} is required", field.label)),
        };
    }
    match &field.kind {
        Kind::Text {
            format,
            min_len,
            max_len,
        } => {
            let len = text.chars().count();
            if let Some(min) = min_len
                && len < *min as usize
            {
                return Err(format!("at least {min} characters"));
            }
            if let Some(max) = max_len
                && len > *max as usize
            {
                return Err(format!("at most {max} characters"));
            }
            if let Some(format) = format {
                check_format(*format, text)?;
            }
            Ok(Some(Value::String(text.to_string())))
        }
        Kind::Number { integer, min, max } => {
            let (value, n) = if *integer {
                let i: i64 = text.parse().map_err(|_| "a whole number".to_string())?;
                (Value::from(i), i as f64)
            } else {
                let x: f64 = text.parse().map_err(|_| "a number".to_string())?;
                let n = serde_json::Number::from_f64(x).ok_or("a finite number")?;
                (Value::Number(n), x)
            };
            if let Some(min) = min
                && n < *min
            {
                return Err(format!("at least {min}"));
            }
            if let Some(max) = max
                && n > *max
            {
                return Err(format!("at most {max}"));
            }
            Ok(Some(value))
        }
        Kind::Bool => match text.to_ascii_lowercase().as_str() {
            "yes" | "y" | "true" => Ok(Some(Value::Bool(true))),
            "no" | "n" | "false" => Ok(Some(Value::Bool(false))),
            _ => Err("yes or no".into()),
        },
        Kind::One(options) => pick(options, text).map(Some),
        Kind::Many { options, min, max } => {
            let mut picked: Vec<Value> = Vec::new();
            for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                let value = pick(options, part)?;
                if !picked.contains(&value) {
                    picked.push(value);
                }
            }
            let n = picked.len() as u64;
            if let Some(min) = min
                && n < *min
            {
                return Err(format!("pick at least {min}"));
            }
            if let Some(max) = max
                && n > *max
            {
                return Err(format!("pick at most {max}"));
            }
            Ok(Some(Value::Array(picked)))
        }
    }
}

/// An option by its label, or by its const for someone who typed that.
fn pick(options: &[(Value, String)], text: &str) -> Result<Value, String> {
    options
        .iter()
        .find(|(_, label)| label.eq_ignore_ascii_case(text))
        .or_else(|| options.iter().find(|(c, _)| c.as_str() == Some(text)))
        .map(|(c, _)| c.clone())
        .ok_or_else(|| {
            let labels: Vec<&str> = options.iter().map(|(_, l)| l.as_str()).collect();
            format!("one of: {}", labels.join(", "))
        })
}

fn check_format(format: Format, text: &str) -> Result<(), String> {
    let ok = match format {
        Format::Email => is_email(text),
        Format::Uri => is_uri(text),
        Format::Date => is_date(text),
        Format::DateTime => is_date_time(text),
    };
    if ok {
        return Ok(());
    }
    Err(match format {
        Format::Email => "an email address like name@example.com",
        Format::Uri => "a URI with a scheme, like https://example.com",
        Format::Date => "a date like 2026-09-28",
        Format::DateTime => "a date and time like 2026-09-28T14:30:00Z",
    }
    .into())
}

fn is_email(text: &str) -> bool {
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !text.chars().any(char::is_whitespace)
}

/// A scheme and a colon (RFC 3986 §3.1) with something after it; the
/// server checks the rest.
fn is_uri(text: &str) -> bool {
    let Some((scheme, rest)) = text.split_once(':') else {
        return false;
    };
    scheme
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        && !rest.is_empty()
        && !text.chars().any(char::is_whitespace)
}

fn digits(s: &str) -> Option<u32> {
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

/// RFC 3339 `full-date`.
fn is_date(text: &str) -> bool {
    let mut parts = text.split('-');
    let (Some(y), Some(m), Some(d), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    y.len() == 4
        && m.len() == 2
        && d.len() == 2
        && digits(y).is_some()
        && digits(m).is_some_and(|m| (1..=12).contains(&m))
        && digits(d).is_some_and(|d| (1..=31).contains(&d))
}

/// RFC 3339 `date-time`: a date, `T`, `hh:mm:ss`, an optional fraction and
/// `Z` or a `±hh:mm` offset.
fn is_date_time(text: &str) -> bool {
    let (Some(date), Some(sep), Some(rest)) = (text.get(..10), text.get(10..11), text.get(11..))
    else {
        return false;
    };
    if !is_date(date) || !sep.eq_ignore_ascii_case("t") {
        return false;
    }
    let (time, zone) = match rest.find(['Z', 'z', '+', '-']) {
        Some(i) => rest.split_at(i),
        None => return false,
    };
    let clock = time.split_once('.').map_or(time, |(clock, fraction)| {
        let fraction_ok = !fraction.is_empty() && fraction.bytes().all(|b| b.is_ascii_digit());
        if fraction_ok { clock } else { "" }
    });
    let hms: Vec<Option<u32>> = clock.split(':').map(digits).collect();
    let clock_ok = clock.len() == 8
        && matches!(hms.as_slice(), [Some(h), Some(m), Some(s)] if *h < 24 && *m < 60 && *s <= 60);
    let offset_ok = |off: &str| {
        let Some((hours, minutes)) = off.split_once(':') else {
            return false;
        };
        hours.len() == 2
            && minutes.len() == 2
            && digits(hours).is_some_and(|h| h < 24)
            && digits(minutes).is_some_and(|m| m < 60)
    };
    let zone_ok = zone.eq_ignore_ascii_case("z") || zone.get(1..).is_some_and(offset_ok);
    clock_ok && zone_ok
}

/// The spec's "review and modify before sending": which fields would be
/// sent, and the three ways on. It names the fields, not the answers: a
/// question is recorded in the rollout like any event (T47.3), and an
/// answer must reach the server only (A78).
pub fn review(message: &str, answers: &Map<String, Value>) -> (String, Vec<String>) {
    let summary = if answers.is_empty() {
        "nothing".to_string()
    } else {
        answers.keys().cloned().collect::<Vec<_>>().join(", ")
    };
    (
        format!("{message} — send {summary}?"),
        ["send", "edit", "decline"].map(String::from).to_vec(),
    )
}

/// One question for the person, from `server`'s elicitation (T47.2). The
/// surface answers through `reply`; dropping it (Esc, a dismissed modal)
/// cancels the whole elicitation.
pub struct Ask {
    pub server: String,
    pub question: String,
    pub options: Vec<String>,
    pub reply: oneshot::Sender<String>,
}

/// Where a surface with a person present takes [`Ask`]s.
pub type Asker = mpsc::Sender<Ask>;

/// How often a bad answer or an unknown review choice is asked again, and
/// how many `edit` rounds a form gets, before the elicitation gives up.
const TRIES: usize = 3;

fn with(action: ElicitationAction) -> ElicitResult {
    ElicitResult::new(action)
}

/// One question; `None` when the person dismissed it or nobody listens.
async fn ask(
    asker: &Asker,
    server: &str,
    (question, options): (String, Vec<String>),
) -> Option<String> {
    let (reply, answer) = oneshot::channel();
    asker
        .send(Ask {
            server: server.to_string(),
            question,
            options,
            reply,
        })
        .await
        .ok()?;
    answer.await.ok()
}

/// How one field's questioning ended.
enum Step {
    Value(Option<Value>),
    Cancel,
    Decline,
}

/// `last` is the person's own answer from before an `edit`: Enter keeps
/// it, and the question does not repeat it (see [`review`]).
async fn ask_field(
    asker: &Asker,
    server: &str,
    message: &str,
    field: &Field,
    last: Option<&Value>,
) -> Step {
    let (base, options) = match last {
        None => prompt(message, field),
        Some(_) => {
            let bare = Field {
                default: None,
                required: true,
                ..field.clone()
            };
            let (question, options) = prompt(message, &bare);
            (
                format!("{question} (Enter keeps your last answer)"),
                options,
            )
        }
    };
    let mut question = base.clone();
    for _ in 0..TRIES {
        let Some(text) = ask(asker, server, (question, options.clone())).await else {
            return Step::Cancel;
        };
        if let Some(last) = last
            && text.trim().is_empty()
        {
            return Step::Value(Some(last.clone()));
        }
        match parse(field, &text) {
            Ok(value) => return Step::Value(value),
            Err(why) => question = format!("{base} — not accepted: {why}"),
        }
    }
    Step::Decline
}

/// A form elicitation, asked field by field and then reviewed: `send`
/// accepts with the answers, `decline` declines, `edit` asks again with the
/// answers as defaults, a dismissed question cancels. A schema cox cannot
/// map is declined without asking.
pub async fn run_form(
    asker: &Asker,
    server: &str,
    message: &str,
    schema: &ElicitationSchema,
) -> ElicitResult {
    let Ok(fields) = fields(schema) else {
        return with(ElicitationAction::Decline);
    };
    let mut last = Map::new();
    for _ in 0..TRIES {
        let mut content = Map::new();
        for field in &fields {
            match ask_field(asker, server, message, field, last.get(&field.key)).await {
                Step::Value(Some(value)) => {
                    content.insert(field.key.clone(), value);
                }
                Step::Value(None) => {}
                Step::Cancel => return with(ElicitationAction::Cancel),
                Step::Decline => return with(ElicitationAction::Decline),
            }
        }
        match choose(asker, server, review(message, &content)).await {
            Some(Choice::Send) => {
                return with(ElicitationAction::Accept).with_content(Value::Object(content));
            }
            Some(Choice::Decline) => return with(ElicitationAction::Decline),
            Some(Choice::Edit) => last = content,
            None => return with(ElicitationAction::Cancel),
        }
    }
    with(ElicitationAction::Cancel)
}

enum Choice {
    Send,
    Edit,
    Decline,
}

/// Characters `cmd /C start`, the Windows opener, would read as syntax
/// rather than as part of the URL.
const CMD_SYNTAX: &[char] = &['&', '|', '^', '<', '>', '"', '%'];

/// T47.4: the consent question for a URL elicitation, and the URL to open
/// if the person agrees — the normalized form the question shows, so what
/// is opened is exactly what was read. `Err` is why the URL is declined
/// without asking: not http(s), no host, or characters that could hide
/// where it leads.
pub fn url_prompt(message: &str, url: &str) -> Result<(String, String, Vec<String>), String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("not a URL: {e}"))?;
    let scheme = parsed.scheme();
    if scheme != "https" && scheme != "http" {
        return Err(format!("a `{scheme}:` URL is never opened"));
    }
    // The url crate writes a non-ASCII host as punycode and percent-encodes
    // the rest, so a normalized URL is plain ASCII: anything else is odd.
    let shown = parsed.as_str().to_string();
    if !shown.is_ascii() || shown.chars().any(|c| c.is_ascii_control() || c == ' ') {
        return Err("the URL has characters that could hide where it leads".into());
    }
    if cfg!(target_os = "windows") && shown.contains(CMD_SYNTAX) {
        return Err("the URL has characters the Windows opener would run".into());
    }
    let Some(host) = parsed.host_str() else {
        return Err("the URL names no host".into());
    };
    let mut question = format!("{message} · open {shown} · host: {host}");
    if host
        .split('.')
        .any(|label| label.to_ascii_lowercase().starts_with("xn--"))
    {
        question.push_str(" · warning: punycode host, it may imitate another name");
    }
    if scheme == "http" {
        question.push_str(" · warning: not https");
    }
    Ok((
        shown,
        question,
        ["open", "decline"].map(String::from).to_vec(),
    ))
}

/// The opener a URL elicitation uses; `auth::open_browser` outside tests.
pub type Opener = std::sync::Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// A URL elicitation: the URL, its host and any warning are shown, and the
/// browser opens only on an explicit `open` (accept); `decline` declines, a
/// dismissed question cancels. Nothing is fetched before that answer.
pub async fn run_url(
    asker: &Asker,
    server: &str,
    message: &str,
    url: &str,
    open: &Opener,
) -> ElicitResult {
    let Ok((target, question, options)) = url_prompt(message, url) else {
        return with(ElicitationAction::Decline);
    };
    for _ in 0..TRIES {
        let Some(text) = ask(asker, server, (question.clone(), options.clone())).await else {
            return with(ElicitationAction::Cancel);
        };
        match text.trim().to_ascii_lowercase().as_str() {
            "open" => {
                // The opener waits on a child process, so it runs off the
                // async threads. The URL was on screen, so a missing browser
                // still leaves the person able to open it by hand.
                let open = open.clone();
                let _ = tokio::task::spawn_blocking(move || open(&target)).await;
                return with(ElicitationAction::Accept);
            }
            "decline" => return with(ElicitationAction::Decline),
            _ => {}
        }
    }
    with(ElicitationAction::Cancel)
}

/// The review answer; an unknown one is asked again, a dismissed one is
/// `None`.
async fn choose(asker: &Asker, server: &str, review: (String, Vec<String>)) -> Option<Choice> {
    for _ in 0..TRIES {
        let text = ask(asker, server, review.clone()).await?;
        match text.trim().to_ascii_lowercase().as_str() {
            "send" => return Some(Choice::Send),
            "edit" => return Some(Choice::Edit),
            "decline" => return Some(Choice::Decline),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A schema as a server sends it; `from_str` keeps the wire order.
    fn schema(text: &str) -> ElicitationSchema {
        serde_json::from_str(text).expect("schema")
    }

    fn one(text: &str) -> Field {
        let mut fields = fields(&schema(text)).expect("fields");
        assert_eq!(fields.len(), 1);
        fields.remove(0)
    }

    #[test]
    fn fields_keep_wire_order_and_map_every_variant() {
        let fields = fields(&schema(
            r#"{"type":"object","properties":{
                "name":{"type":"string","title":"Name","minLength":1},
                "ratio":{"type":"number"},
                "age":{"type":"integer","minimum":0,"maximum":150},
                "ok":{"type":"boolean","default":true},
                "size":{"type":"string","enum":["s","m"]},
                "color":{"type":"string","oneOf":[{"const":"r","title":"Red"}]},
                "tags":{"type":"array","items":{"type":"string","enum":["a","b"]}},
                "pets":{"type":"array","items":{"anyOf":[{"const":"c","title":"Cat"}]}},
                "old":{"type":"string","enum":["x"],"enumNames":["Ex"]}
            },"required":["name"]}"#,
        ))
        .expect("fields");
        let keys: Vec<&str> = fields.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "name", "ratio", "age", "ok", "size", "color", "tags", "pets", "old"
            ]
        );
        assert_eq!(fields[0].label, "Name");
        assert!(fields[0].required && !fields[1].required);
        assert!(matches!(
            fields[1].kind,
            Kind::Number { integer: false, .. }
        ));
        assert!(matches!(
            fields[2].kind,
            Kind::Number {
                integer: true,
                min: Some(_),
                ..
            }
        ));
        assert_eq!(fields[3].default, Some(json!(true)));
        assert_eq!(
            fields[4].kind,
            Kind::One(vec![(json!("s"), "s".into()), (json!("m"), "m".into())])
        );
        assert_eq!(fields[5].kind, Kind::One(vec![(json!("r"), "Red".into())]));
        assert!(matches!(&fields[6].kind, Kind::Many { options, .. } if options.len() == 2));
        assert!(matches!(&fields[7].kind, Kind::Many { options, .. } if options[0].1 == "Cat"));
        assert_eq!(fields[8].kind, Kind::One(vec![(json!("x"), "Ex".into())]));
    }

    #[test]
    fn titled_enum_answers_its_const() {
        let f = one(r#"{"type":"object","properties":{"color":{"type":"string",
                "oneOf":[{"const":"r","title":"Red"},{"const":"g","title":"Green"}]}}}"#);
        assert_eq!(prompt("Pick", &f).1, ["Red", "Green"]);
        assert_eq!(parse(&f, "Green"), Ok(Some(json!("g"))));
        assert_eq!(parse(&f, "green"), Ok(Some(json!("g"))));
        assert!(parse(&f, "Blue").is_err());
    }

    #[test]
    fn untitled_and_legacy_enums_answer_their_values() {
        let f = one(r#"{"type":"object","properties":{"s":{"type":"string","enum":["a","b"]}}}"#);
        assert_eq!(parse(&f, "b"), Ok(Some(json!("b"))));
        let f = one(r#"{"type":"object","properties":{"s":{"type":"string",
                "enum":["x","y"],"enumNames":["Ex","Why"]}}}"#);
        assert_eq!(parse(&f, "Why"), Ok(Some(json!("y"))));
    }

    #[test]
    fn required_empty_answer_is_rejected() {
        let f = one(r#"{"type":"object","properties":{"n":{"type":"string"}},"required":["n"]}"#);
        assert_eq!(parse(&f, "  "), Err("n is required".into()));
    }

    #[test]
    fn integer_out_of_range_is_rejected() {
        let f = one(
            r#"{"type":"object","properties":{"age":{"type":"integer","minimum":0,"maximum":150}}}"#,
        );
        assert_eq!(parse(&f, "151"), Err("at most 150".into()));
        assert_eq!(parse(&f, "-1"), Err("at least 0".into()));
        assert!(parse(&f, "1.5").is_err());
        assert_eq!(parse(&f, "42"), Ok(Some(json!(42))));
    }

    #[test]
    fn number_accepts_a_fraction() {
        let f = one(r#"{"type":"object","properties":{"r":{"type":"number","maximum":1}}}"#);
        assert_eq!(parse(&f, "0.5"), Ok(Some(json!(0.5))));
        assert!(parse(&f, "1.5").is_err());
    }

    #[test]
    fn bool_takes_yes_and_no() {
        let f = one(r#"{"type":"object","properties":{"ok":{"type":"boolean"}}}"#);
        assert_eq!(prompt("Sure", &f).1, ["yes", "no"]);
        assert_eq!(parse(&f, "yes"), Ok(Some(json!(true))));
        assert_eq!(parse(&f, "No"), Ok(Some(json!(false))));
        assert!(parse(&f, "maybe").is_err());
    }

    #[test]
    fn optional_empty_answer_is_omitted() {
        let f = one(r#"{"type":"object","properties":{"note":{"type":"string"}}}"#);
        assert_eq!(parse(&f, ""), Ok(None));
        assert!(prompt("Say", &f).0.contains("optional"));
    }

    #[test]
    fn default_is_used_on_enter() {
        let f = one(r#"{"type":"object","properties":{"c":{"type":"string",
                "oneOf":[{"const":"r","title":"Red"}],"default":"r"}},"required":["c"]}"#);
        assert_eq!(parse(&f, ""), Ok(Some(json!("r"))));
        assert!(
            prompt("Pick", &f)
                .0
                .contains("(default: Red, Enter keeps it)")
        );
    }

    #[test]
    fn many_select_splits_on_commas() {
        let f = one(
            r#"{"type":"object","properties":{"t":{"type":"array","maxItems":2,
                "items":{"anyOf":[{"const":"a","title":"A"},{"const":"b","title":"B"},
                {"const":"c","title":"C"}]}}}}"#,
        );
        assert_eq!(parse(&f, "A, b ,A"), Ok(Some(json!(["a", "b"]))));
        assert_eq!(parse(&f, "A,B,C"), Err("pick at most 2".into()));
        let untitled = one(r#"{"type":"object","properties":{"t":{"type":"array",
                "items":{"type":"string","enum":["x","y"]}}}}"#);
        assert_eq!(parse(&untitled, "y,x"), Ok(Some(json!(["y", "x"]))));
    }

    #[test]
    fn string_formats_and_lengths_are_checked() {
        let field = |format: &str| {
            one(&format!(
                r#"{{"type":"object","properties":{{"s":{{"type":"string","format":"{format}"}}}}}}"#
            ))
        };
        let email = field("email");
        assert!(parse(&email, "a@example.com").is_ok());
        assert!(parse(&email, "a@b").is_err());
        let uri = field("uri");
        assert!(parse(&uri, "https://example.com").is_ok());
        assert!(parse(&uri, "example.com").is_err());
        let date = field("date");
        assert!(parse(&date, "2026-09-28").is_ok());
        assert!(parse(&date, "2026-13-01").is_err());
        let dt = field("date-time");
        assert!(parse(&dt, "2026-09-28T14:30:00Z").is_ok());
        assert!(parse(&dt, "2026-09-28T14:30:00.5+02:00").is_ok());
        assert!(parse(&dt, "2026-09-28 14:30").is_err());
        let short = one(r#"{"type":"object","properties":{"s":{"type":"string","maxLength":3}}}"#);
        assert_eq!(parse(&short, "abcd"), Err("at most 3 characters".into()));
    }

    #[test]
    fn review_offers_send_edit_decline() {
        let mut answers = Map::new();
        answers.insert("name".into(), json!("Ada"));
        answers.insert("tags".into(), json!(["a", "b"]));
        let (question, options) = review("Sign up", &answers);
        assert_eq!(question, "Sign up — send name, tags?");
        assert!(!question.contains("Ada"));
        assert_eq!(options, ["send", "edit", "decline"]);
    }

    #[test]
    fn url_prompt_shows_the_normalized_url_and_its_host() {
        let (target, question, options) =
            url_prompt("Link your account", "HTTPS://Example.COM/a?x=1").expect("prompt");
        assert_eq!(target, "https://example.com/a?x=1");
        assert_eq!(
            question,
            "Link your account · open https://example.com/a?x=1 · host: example.com"
        );
        assert_eq!(options, ["open", "decline"]);
        let (_, plain, _) = url_prompt("m", "http://example.com/").expect("http");
        assert!(plain.ends_with(" · warning: not https"), "{plain}");
    }

    #[test]
    fn punycode_host_is_flagged() {
        let (_, question, _) = url_prompt("m", "https://xn--pple-43d.com/").expect("puny");
        assert!(question.contains("warning: punycode host"), "{question}");
        // A Cyrillic "а" normalizes to punycode, so a homograph shows as one.
        let (target, question, _) = url_prompt("m", "https://\u{0430}pple.com/").expect("cyr");
        assert!(target.starts_with("https://xn--"), "{target}");
        assert!(question.contains("warning: punycode host"), "{question}");
        let (_, plain, _) = url_prompt("m", "https://apple.com/").expect("ascii");
        assert!(!plain.contains("warning"), "{plain}");
    }

    #[test]
    fn non_web_urls_are_declined() {
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,hi",
            "ssh://example.com",
            "not a url",
        ] {
            assert!(url_prompt("m", url).is_err(), "{url}");
        }
    }

    /// An opener that records instead of launching a browser.
    fn recorder() -> (Opener, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let opened = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = opened.clone();
        let open: Opener = std::sync::Arc::new(move |url: &str| {
            log.lock().expect("log").push(url.to_string());
            true
        });
        (open, opened)
    }

    #[tokio::test]
    async fn url_elicitation_opens_only_after_consent() {
        for (answer, action, want) in [
            (
                "open",
                ElicitationAction::Accept,
                vec!["https://example.com/"],
            ),
            ("decline", ElicitationAction::Decline, vec![]),
        ] {
            let (open, opened) = recorder();
            let (asker, mut asks) = mpsc::channel::<Ask>(1);
            let seen = opened.clone();
            let person = tokio::spawn(async move {
                let ask = asks.recv().await.expect("ask");
                // Nothing is opened while the question is on screen.
                assert!(seen.lock().expect("log").is_empty());
                let _ = ask.reply.send(answer.to_string());
                ask.question
            });
            let result = run_url(&asker, "s", "Link", "https://example.com", &open).await;
            assert_eq!(result.action, action);
            assert!(
                person
                    .await
                    .expect("person")
                    .contains("https://example.com/")
            );
            assert_eq!(*opened.lock().expect("log"), want);
        }
    }

    #[tokio::test]
    async fn file_scheme_is_declined_unasked() {
        let (open, opened) = recorder();
        let (asker, mut asks) = mpsc::channel::<Ask>(1);
        let result = run_url(&asker, "s", "Look", "file:///etc/passwd", &open).await;
        assert_eq!(result.action, ElicitationAction::Decline);
        assert!(asks.try_recv().is_err());
        assert!(opened.lock().expect("log").is_empty());
    }

    #[tokio::test]
    async fn dismissed_url_question_cancels_and_opens_nothing() {
        let (open, opened) = recorder();
        let (asker, mut asks) = mpsc::channel::<Ask>(1);
        tokio::spawn(async move { drop(asks.recv().await) });
        let result = run_url(&asker, "s", "Link", "https://example.com", &open).await;
        assert_eq!(result.action, ElicitationAction::Cancel);
        assert!(opened.lock().expect("log").is_empty());
    }
}
