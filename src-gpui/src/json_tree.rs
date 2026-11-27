//! The Pretty response view for JSON: a foldable tree drawn as a virtual list,
//! so only the visible lines are laid out however large the document is.

use super::Workbench;
use crate::appearance::{Palette, icon, mono};
use gpui::{
    Context, Div, FontWeight, InteractiveElement, IntoElement, ListHorizontalSizingBehavior,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, div, px, uniform_list,
};
use gpui_component::{
    Sizable,
    button::{Button, ButtonVariants},
};
use serde_json::Value;
use std::collections::HashMap;
use std::rc::Rc;

/// Containers open by default down to this depth when they are small enough.
const OPEN_DEPTH: usize = 3;
const OPEN_SIZE: usize = 100;
/// Expand all stops opening containers after this many visible lines.
const EXPAND_BUDGET: usize = 20_000;
/// Longer strings are cut in the tree; Raw and Copy keep the full text.
const MAX_SCALAR: usize = 4_000;
const LINE_HEIGHT: f32 = 20.;
const INDENT: f32 = 16.;
/// Path separator that cannot appear in a JSON key typed by a person.
const SEP: char = '\u{1f}';

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Token {
    Str(SharedString),
    Num(SharedString),
    Bool(bool),
    Null,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Body {
    /// An object or array header; `open` is false when folded.
    Open {
        path: Rc<str>,
        array: bool,
        count: usize,
        open: bool,
    },
    Close {
        array: bool,
    },
    Empty {
        array: bool,
    },
    Scalar(Token),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Line {
    pub depth: usize,
    pub key: Option<SharedString>,
    pub body: Body,
    pub comma: bool,
}

pub(super) struct JsonTree {
    root: Value,
    /// Identity of the response body this tree was parsed from.
    source: (usize, usize),
    folds: HashMap<Rc<str>, bool>,
    lines: Rc<Vec<Line>>,
    widest: usize,
}

impl JsonTree {
    /// Parses an object or array body. Scalars and invalid JSON return None so
    /// the caller falls back to plain text.
    pub(super) fn parse(body: &str) -> Option<Self> {
        let root: Value = serde_json::from_str(body).ok()?;
        let count = container_len(&root)?;
        if count == 0 {
            return None;
        }
        let mut tree = Self {
            root,
            source: source_of(body),
            folds: HashMap::new(),
            lines: Rc::new(Vec::new()),
            widest: 0,
        };
        tree.rebuild();
        Some(tree)
    }

    pub(super) fn is_from(&self, body: &str) -> bool {
        self.source == source_of(body)
    }

    pub(super) fn lines(&self) -> &Rc<Vec<Line>> {
        &self.lines
    }

    pub(super) fn toggle(&mut self, path: &Rc<str>) {
        let open = self
            .lines
            .iter()
            .find_map(|line| match &line.body {
                Body::Open { path: at, open, .. } if at == path => Some(*open),
                _ => None,
            })
            .unwrap_or(false);
        self.folds.insert(path.clone(), !open);
        self.rebuild();
    }

    pub(super) fn collapse_all(&mut self) {
        self.folds.clear();
        visit_containers(&self.root, "", &mut |path, _| {
            self.folds.insert(path.into(), path.is_empty());
        });
        self.rebuild();
    }

    pub(super) fn expand_all(&mut self) {
        self.folds.clear();
        let mut budget = EXPAND_BUDGET;
        // Preorder: parents spend the budget before their children, so the
        // top of the document opens first when a payload is very large.
        visit_containers(&self.root, "", &mut |path, count| {
            let open = budget > 0;
            budget = budget.saturating_sub(count + 1);
            self.folds.insert(path.into(), open);
        });
        self.rebuild();
    }

    fn rebuild(&mut self) {
        let mut lines = Vec::new();
        self.push(&self.root, None, String::new(), 0, false, &mut lines);
        self.widest = lines
            .iter()
            .enumerate()
            .max_by_key(|(_, line)| line_width(line))
            .map_or(0, |(index, _)| index);
        self.lines = Rc::new(lines);
    }

    fn push(
        &self,
        value: &Value,
        key: Option<SharedString>,
        path: String,
        depth: usize,
        comma: bool,
        lines: &mut Vec<Line>,
    ) {
        let line = |body| Line {
            depth,
            key: key.clone(),
            body,
            comma,
        };
        let Some(count) = container_len(value) else {
            lines.push(line(Body::Scalar(token(value))));
            return;
        };
        let array = value.is_array();
        if count == 0 {
            lines.push(line(Body::Empty { array }));
            return;
        }
        let path: Rc<str> = path.into();
        let open = self
            .folds
            .get(&path)
            .copied()
            .unwrap_or(depth == 0 || depth < OPEN_DEPTH && count <= OPEN_SIZE);
        lines.push(line(Body::Open {
            path: path.clone(),
            array,
            count,
            open,
        }));
        if !open {
            return;
        }
        let child_path = |segment: &str| {
            if path.is_empty() {
                segment.to_owned()
            } else {
                format!("{path}{SEP}{segment}")
            }
        };
        match value {
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    let last = index + 1 == items.len();
                    self.push(
                        item,
                        None,
                        child_path(&index.to_string()),
                        depth + 1,
                        !last,
                        lines,
                    );
                }
            }
            Value::Object(entries) => {
                for (index, (name, item)) in entries.iter().enumerate() {
                    let last = index + 1 == entries.len();
                    let key = SharedString::from(quote(name));
                    self.push(item, Some(key), child_path(name), depth + 1, !last, lines);
                }
            }
            _ => {}
        }
        lines.push(Line {
            depth,
            key: None,
            body: Body::Close { array },
            comma,
        });
    }
}

fn source_of(body: &str) -> (usize, usize) {
    (body.as_ptr() as usize, body.len())
}

fn container_len(value: &Value) -> Option<usize> {
    match value {
        Value::Array(items) => Some(items.len()),
        Value::Object(entries) => Some(entries.len()),
        _ => None,
    }
}

fn visit_containers(value: &Value, path: &str, visit: &mut impl FnMut(&str, usize)) {
    let Some(count) = container_len(value) else {
        return;
    };
    if count == 0 {
        return;
    }
    visit(path, count);
    let join = |segment: &str| {
        if path.is_empty() {
            segment.to_owned()
        } else {
            format!("{path}{SEP}{segment}")
        }
    };
    match value {
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                visit_containers(item, &join(&index.to_string()), visit);
            }
        }
        Value::Object(entries) => {
            for (name, item) in entries {
                visit_containers(item, &join(name), visit);
            }
        }
        _ => {}
    }
}

fn quote(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| format!("\"{text}\""))
}

fn token(value: &Value) -> Token {
    match value {
        Value::String(text) => {
            let mut quoted = quote(text);
            if quoted.len() > MAX_SCALAR {
                let cut = quoted.floor_char_boundary(MAX_SCALAR);
                quoted.truncate(cut);
                quoted.push_str("…\"");
            }
            Token::Str(quoted.into())
        }
        Value::Number(number) => Token::Num(number.to_string().into()),
        Value::Bool(flag) => Token::Bool(*flag),
        _ => Token::Null,
    }
}

fn line_width(line: &Line) -> usize {
    let key = line.key.as_ref().map_or(0, |key| key.len() + 2);
    let body = match &line.body {
        Body::Scalar(Token::Str(text) | Token::Num(text)) => text.len(),
        Body::Scalar(_) => 5,
        Body::Open { open: false, .. } => 16,
        _ => 2,
    };
    line.depth * 4 + key + body
}

fn count_label(count: usize, array: bool) -> String {
    let unit = match (array, count == 1) {
        (true, true) => "item",
        (true, false) => "items",
        (false, true) => "key",
        (false, false) => "keys",
    };
    format!("{count} {unit}")
}

impl Workbench {
    /// The JSON tree with its toolbar, or None when the body is not a JSON
    /// object or array.
    pub(super) fn json_view(&self, cx: &mut Context<Self>) -> Option<Div> {
        let tree = self.json.as_ref()?;
        let p = &self.palette;
        let lines = tree.lines().clone();
        let count = lines.len();
        let gutter = 18. + 7.5 * count.to_string().len() as f32;
        let palette = p.clone();
        let list = uniform_list(
            "json-tree",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|index| json_line(&lines[index], index, gutter, &palette, this, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .with_width_from_item(Some(tree.widest))
        .track_scroll(self.json_scroll.clone())
        .flex_1()
        .min_h_0()
        .pb(px(12.));

        Some(
            div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .h(px(32.))
                        .flex_none()
                        .px(px(10.))
                        .border_b_1()
                        .border_color(p.soft_line)
                        .child(
                            Button::new("json-collapse-all")
                                .ghost()
                                .xsmall()
                                .child(json_tool("fold", "Collapse all", p))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(tree) = &mut this.json {
                                        tree.collapse_all();
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("json-expand-all")
                                .ghost()
                                .xsmall()
                                .child(json_tool("unfold", "Expand all", p))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(tree) = &mut this.json {
                                        tree.expand_all();
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .font_family(mono())
                                .text_size(px(11.))
                                .text_color(p.faint)
                                .child(format!(
                                    "JSON · {} {}",
                                    count,
                                    if count == 1 { "line" } else { "lines" }
                                )),
                        ),
                )
                .child(list),
        )
    }
}

fn json_tool(glyph: &'static str, label: &'static str, p: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(5.))
        .text_size(px(12.))
        .text_color(p.dim)
        .child(icon(glyph, p.faint, 13.))
        .child(label)
}

fn json_line(
    line: &Line,
    index: usize,
    gutter: f32,
    p: &Palette,
    _: &mut Workbench,
    cx: &mut Context<Workbench>,
) -> gpui::AnyElement {
    let punct = |text: &'static str| div().text_color(p.faint).child(text);
    let mut row = div()
        .id(("json-line", index))
        .w_full()
        .h(px(LINE_HEIGHT))
        .flex()
        .items_center()
        .whitespace_nowrap()
        .font_family(mono())
        .text_size(px(12.))
        .text_color(p.text)
        .child(
            div()
                .w(px(gutter))
                .h_full()
                .flex_none()
                .flex()
                .items_center()
                .justify_end()
                .pr(px(12.))
                .text_size(px(11.))
                .text_color(p.faint)
                .child((index + 1).to_string()),
        )
        .pl(px(2.));
    // Indent guides, one per level, like the Tauri tree's left rules.
    for _ in 0..line.depth {
        row = row.child(
            div()
                .w(px(INDENT))
                .h_full()
                .flex_none()
                .ml(px(5.))
                .border_l_1()
                .border_color(p.soft_line),
        );
    }
    let toggle = match &line.body {
        Body::Open { path, open, .. } => Some((path.clone(), *open)),
        _ => None,
    };
    row = row.child(
        div().w(px(14.)).flex_none().flex().items_center().children(
            toggle
                .as_ref()
                .map(|(_, open)| icon(if *open { "chevron-down" } else { "caret" }, p.faint, 11.)),
        ),
    );
    if let Some(key) = &line.key {
        row = row
            .child(div().text_color(p.key).child(key.clone()))
            .child(punct(": "));
    }
    let comma = if line.comma { "," } else { "" };
    row = match &line.body {
        Body::Scalar(token) => {
            let value = match token {
                Token::Str(text) => div().text_color(p.string).child(text.clone()),
                Token::Num(text) => div().text_color(p.warn).child(text.clone()),
                Token::Bool(flag) => {
                    div()
                        .text_color(p.violet)
                        .child(if *flag { "true" } else { "false" })
                }
                Token::Null => div().text_color(p.faint).italic().child("null"),
            };
            row.child(value)
                .child(punct(if line.comma { "," } else { "" }))
        }
        Body::Empty { array } => row.child(punct(match (array, line.comma) {
            (true, true) => "[],",
            (true, false) => "[]",
            (false, true) => "{},",
            (false, false) => "{}",
        })),
        Body::Close { array } => row.child(punct(match (array, line.comma) {
            (true, true) => "],",
            (true, false) => "]",
            (false, true) => "},",
            (false, false) => "}",
        })),
        Body::Open {
            array, count, open, ..
        } => {
            let (open_mark, close_mark) = if *array { ("[", "]") } else { ("{", "}") };
            let row = row.child(punct(open_mark));
            if *open {
                row
            } else {
                row.child(punct(" … "))
                    .child(
                        div()
                            .px(px(6.))
                            .rounded_full()
                            .border_1()
                            .border_color(p.line)
                            .bg(p.raised)
                            .text_size(px(10.5))
                            .text_color(p.dim)
                            .font_weight(FontWeight::MEDIUM)
                            .child(count_label(*count, *array)),
                    )
                    .child(
                        div()
                            .text_color(p.faint)
                            .child(format!(" {close_mark}{comma}")),
                    )
            }
        }
    };
    match toggle {
        Some((path, _)) => row
            .cursor_pointer()
            .rounded(px(4.))
            .hover(|style| style.bg(p.hover))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(tree) = &mut this.json {
                    tree.toggle(&path);
                }
                cx.notify();
            }))
            .into_any_element(),
        None => row.into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(tree: &JsonTree) -> Vec<String> {
        tree.lines()
            .iter()
            .map(|line| {
                let key = line
                    .key
                    .as_ref()
                    .map(|k| format!("{k}: "))
                    .unwrap_or_default();
                let body = match &line.body {
                    Body::Open {
                        array, open: true, ..
                    } => (if *array { "[" } else { "{" }).to_owned(),
                    Body::Open {
                        count, open: false, ..
                    } => format!("…{count}"),
                    Body::Close { array } => (if *array { "]" } else { "}" }).to_owned(),
                    Body::Empty { array } => (if *array { "[]" } else { "{}" }).to_owned(),
                    Body::Scalar(Token::Str(s) | Token::Num(s)) => s.to_string(),
                    Body::Scalar(Token::Bool(b)) => b.to_string(),
                    Body::Scalar(Token::Null) => "null".to_owned(),
                };
                format!(
                    "{}{key}{body}{}",
                    "  ".repeat(line.depth),
                    if line.comma && !matches!(line.body, Body::Open { open: true, .. }) {
                        ","
                    } else {
                        ""
                    }
                )
            })
            .collect()
    }

    #[test]
    fn scalars_and_invalid_bodies_fall_back_to_text() {
        assert!(JsonTree::parse("42").is_none());
        assert!(JsonTree::parse("{}").is_none());
        assert!(JsonTree::parse("not json").is_none());
    }

    #[test]
    fn small_documents_open_and_render_like_pretty_json() {
        let tree = JsonTree::parse(r#"{"a":1,"b":[true,null],"c":{},"d":"x"}"#).unwrap();
        assert_eq!(
            text(&tree),
            [
                "{",
                "  \"a\": 1,",
                "  \"b\": [",
                "    true,",
                "    null",
                "  ],",
                "  \"c\": {},",
                "  \"d\": \"x\"",
                "}"
            ]
        );
    }

    #[test]
    fn folding_hides_children_and_survives_rebuilds() {
        let mut tree = JsonTree::parse(r#"{"a":{"b":[1,2]},"c":3}"#).unwrap();
        let path: Rc<str> = "a".into();
        tree.toggle(&path);
        assert_eq!(text(&tree), ["{", "  \"a\": …1,", "  \"c\": 3", "}"]);
        tree.collapse_all();
        assert_eq!(text(&tree), ["{", "  \"a\": …1,", "  \"c\": 3", "}"]);
        tree.expand_all();
        assert_eq!(text(&tree).len(), 9);
    }

    #[test]
    fn deep_or_large_containers_start_folded() {
        let wide = format!("[{}]", vec!["1"; OPEN_SIZE + 1].join(","));
        let tree = JsonTree::parse(&format!(r#"{{"wide":{wide}}}"#)).unwrap();
        assert_eq!(text(&tree)[1], format!("  \"wide\": …{}", OPEN_SIZE + 1));
        let deep = JsonTree::parse(r#"{"a":{"b":{"c":{"d":1}}}}"#).unwrap();
        assert!(text(&deep).iter().any(|line| line.ends_with("\"c\": …1")));
    }
}
