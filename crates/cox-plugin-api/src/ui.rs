//! The widget tree a plugin returns from `cox_render`/`cox_render_item`
//! (PL§8): declarative and closed, so the host alone decides how anything is
//! drawn. Colours are `StyleToken`s naming `cox-tui`'s `Theme` fields, never
//! raw values, so themes, `NO_COLOR` and colour downgrade keep working for
//! plugin output too. Pure data here; the ratatui conversion lives in
//! `cox-tui`'s `plugin_ui`, which is also where every string is sanitized.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// At most this many `Widget` nodes per render (PL§8).
pub const MAX_NODES: usize = 512;
/// At most this nesting depth; the root is depth 1 (PL§8).
pub const MAX_DEPTH: usize = 8;
/// At most this many bytes of span text per render (PL§8).
pub const MAX_TEXT_BYTES: usize = 16 * 1024;

/// A `Theme` field. The permission-mode tints are left out on purpose: the
/// composer's mode colour is a trust signal a plugin must not imitate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum StyleToken {
    #[default]
    Text,
    Dim,
    Accent,
    User,
    Agent,
    Tool,
    Ok,
    Warn,
    Error,
    DiffAdd,
    DiffDel,
    DiffHunk,
    Border,
    Selection,
}

/// A run of text with one style.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Span {
    /// The text; the host sanitizes it before drawing.
    pub text: String,
    /// Its colour role.
    #[serde(default)]
    pub style: StyleToken,
    /// Bold.
    #[serde(default)]
    pub bold: bool,
    /// Italic.
    #[serde(default)]
    pub italic: bool,
    /// An `https://` URL or a workspace-relative path; the host drops
    /// anything else to plain text and opens a link only on a click.
    #[serde(default)]
    pub link: Option<String>,
}

/// One row of spans.
pub type Line = Vec<Span>;

/// The closed set of widgets (PL§8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Widget {
    /// Plain lines.
    Text(Vec<Line>),
    /// Lines with an optional highlighted index.
    List {
        /// The rows.
        items: Vec<Line>,
        /// The highlighted row.
        #[serde(default)]
        selected: Option<usize>,
    },
    /// A table; a column without a width shares the rest equally.
    Table {
        /// Column headings.
        header: Vec<Span>,
        /// One cell per column.
        rows: Vec<Vec<Span>>,
        /// Column widths in cells.
        #[serde(default)]
        widths: Vec<u16>,
    },
    /// Aligned `key: value` rows.
    KeyValue(Vec<(Span, Vec<Span>)>),
    /// A one-row progress bar; `ratio` is clamped to `0..=1`.
    Gauge {
        /// Fraction done.
        ratio: f64,
        /// Text before the bar.
        #[serde(default)]
        label: Span,
    },
    /// Children side by side or stacked; a child without a size shares the rest.
    Stack {
        /// Stacked top to bottom rather than left to right.
        vertical: bool,
        /// The children.
        children: Vec<Widget>,
        /// Rows (vertical) or columns (horizontal) per child.
        #[serde(default)]
        sizes: Vec<u16>,
    },
    /// A package-relative PNG the desktop draws (PL§15.4); the terminal draws
    /// `alt` as one dim line. A path, never a URL, so the host never fetches
    /// for a plugin.
    Image {
        /// Path inside the plugin package.
        path: String,
        /// Accessibility label and the terminal fallback.
        alt: String,
    },
    /// A bordered box around one child.
    Block {
        /// Title on the top border.
        #[serde(default)]
        title: Option<Span>,
        /// The content.
        child: Box<Widget>,
    },
}

impl Widget {
    /// Whether the tree fits PL§8's node, depth and text caps. The walk
    /// stops at the first breach, so an oversize tree costs no more than a
    /// legal one to reject.
    pub fn within_limits(&self) -> bool {
        let (mut nodes, mut text) = (0, 0);
        self.fits(1, &mut nodes, &mut text)
    }

    fn fits(&self, depth: usize, nodes: &mut usize, text: &mut usize) -> bool {
        *nodes += 1;
        if depth > MAX_DEPTH || *nodes > MAX_NODES {
            return false;
        }
        let mut add = |spans: &mut dyn Iterator<Item = &Span>| {
            *text += spans.map(|s| s.text.len()).sum::<usize>();
            *text <= MAX_TEXT_BYTES
        };
        match self {
            Widget::Text(lines) | Widget::List { items: lines, .. } => {
                add(&mut lines.iter().flatten())
            }
            Widget::Table { header, rows, .. } => {
                add(&mut header.iter().chain(rows.iter().flatten()))
            }
            Widget::KeyValue(pairs) => {
                add(&mut pairs.iter().flat_map(|(k, v)| std::iter::once(k).chain(v)))
            }
            Widget::Gauge { label, .. } => add(&mut std::iter::once(label)),
            Widget::Image { path, alt } => {
                *text += path.len() + alt.len();
                *text <= MAX_TEXT_BYTES
            }
            Widget::Stack { children, .. } => {
                children.iter().all(|c| c.fits(depth + 1, nodes, text))
            }
            Widget::Block { title, child } => {
                add(&mut title.iter()) && child.fits(depth + 1, nodes, text)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The JSON a guest in any language writes: externally tagged,
    /// snake_case, and a span needs only its text.
    #[test]
    fn widget_json_uses_snake_case_tags_and_span_defaults() {
        let w: Widget = serde_json::from_str(r#"{"block":{"child":{"text":[[{"text":"hi"}]]}}}"#)
            .expect("parses");
        let span = Span {
            text: "hi".into(),
            ..Span::default()
        };
        assert_eq!(
            w,
            Widget::Block {
                title: None,
                child: Box::new(Widget::Text(vec![vec![span]])),
            }
        );
    }

    #[test]
    fn span_link_and_image_round_trip_and_default_to_absent() {
        let w: Widget = serde_json::from_str(
            r#"{"stack":{"vertical":true,"children":[
                {"text":[[{"text":"docs","link":"https://example.com"},{"text":"plain"}]]},
                {"image":{"path":"img/logo.png","alt":"logo"}}]}}"#,
        )
        .expect("parses");
        let Widget::Stack { children, .. } = &w else {
            panic!("a stack");
        };
        let Widget::Text(lines) = &children[0] else {
            panic!("text");
        };
        assert_eq!(lines[0][0].link.as_deref(), Some("https://example.com"));
        assert_eq!(lines[0][1].link, None);
        assert_eq!(
            children[1],
            Widget::Image {
                path: "img/logo.png".into(),
                alt: "logo".into()
            }
        );
        let again: Widget =
            serde_json::from_str(&serde_json::to_string(&w).expect("serializes")).expect("parses");
        assert_eq!(again, w);
    }

    #[test]
    fn image_counts_toward_the_node_and_text_caps() {
        let image = || Widget::Image {
            path: "a.png".into(),
            alt: "a".into(),
        };
        let stack = |n: usize| Widget::Stack {
            vertical: true,
            children: (0..n).map(|_| image()).collect(),
            sizes: vec![],
        };
        assert!(stack(MAX_NODES - 1).within_limits());
        assert!(!stack(MAX_NODES).within_limits());
        let long = Widget::Image {
            path: "a.png".into(),
            alt: "x".repeat(MAX_TEXT_BYTES),
        };
        assert!(!long.within_limits());
    }
}
