use super::Workbench;
use crate::{appearance::icon, http::Response, workspace_edit};
use base64::Engine;
use gpui::{
    AppContext, Context, Div, ParentElement, SharedString, StatefulInteractiveElement, Styled,
    Window, div, px,
};
use gpui_component::{
    Sizable, WindowExt,
    button::Button,
    input::{Input, InputState},
    menu::{DropdownMenu, PopupMenuItem},
};
use html5ever::{parse_document, tendril::TendrilSink};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use serde_json::{Value, json};

const MAX_PREVIEW_NODES: usize = 2048;

impl Workbench {
    pub(super) fn response_preview(&self) -> Option<SharedString> {
        let wire = &self.active_response()?.as_ref().ok()?.wire;
        if !content_type(wire).is_some_and(is_html)
            || wire.truncated
            || wire.body.len() > 256 * 1024
        {
            return None;
        }
        safe_html(&wire.body).map(Into::into)
    }

    pub(super) fn request_examples(&self, cx: &mut Context<Self>) -> Div {
        let p = &self.palette;
        let mut actions = div().flex().items_center().gap(px(4.));
        let bookmark = super::icon_button("save-example", "bookmark", p.faint, p.hover, 26.);
        if self.viewing_example.is_none() && self.active_response().is_some_and(Result::is_ok) {
            actions = actions.child(
                bookmark.on_click(cx.listener(|this, _, window, cx| this.ask_example(window, cx))),
            );
        } else {
            actions = actions.child(bookmark);
        }
        let Some(examples) = self
            .active_request()
            .and_then(|req| req["examples"].as_array())
            .filter(|rows| !rows.is_empty())
        else {
            return actions;
        };
        let entries: Vec<_> = examples
            .iter()
            .enumerate()
            .map(|(index, example)| (index, super::name(example, "Example").to_owned()))
            .collect();
        let view = cx.entity().downgrade();
        actions.child(
            Button::new("examples")
                .outline()
                .small()
                .label(format!("Examples  {}", entries.len()))
                .dropdown_menu(move |mut menu, _, _| {
                    for (index, name) in &entries {
                        let view = view.clone();
                        let index = *index;
                        menu = menu.item(PopupMenuItem::new(name.clone()).on_click(
                            move |_, window, cx| {
                                let _ = view
                                    .update(cx, |this, cx| this.show_example(index, window, cx));
                            },
                        ));
                    }
                    menu
                }),
        )
    }

    pub(super) fn example_banner(&self, cx: &mut Context<Self>) -> Option<Div> {
        let (name, _) = self.viewing_example.as_ref()?;
        let p = &self.palette;
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(37.))
                .px(px(14.))
                .bg(p.raised)
                .child(icon("bookmark", p.accent, 14.))
                .child(format!("Viewing example: {name}"))
                .child(div().flex_1())
                .child(
                    Button::new("back-to-live")
                        .outline()
                        .small()
                        .label("Back to live response")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.viewing_example = None;
                            this.refresh_body(window, cx);
                            cx.notify();
                        })),
                ),
        )
    }

    fn show_example(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(example) = self
            .active_request()
            .and_then(|request| request["examples"].get(index))
        else {
            return;
        };
        self.viewing_example = Some((
            super::name(example, "Example").to_owned(),
            Ok(example_response(example)),
        ));
        self.refresh_body(window, cx);
        cx.notify();
    }

    fn ask_example(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = cx.new(|cx| InputState::new(window, cx).placeholder("Example name"));
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let input = state.clone();
            let weak = weak.clone();
            dialog
                .title("Save response as example")
                .confirm()
                .child(Input::new(&state))
                .on_ok(move |_, _, cx| {
                    let name = input.read(cx).value().to_string();
                    !name.trim().is_empty()
                        && weak
                            .update(cx, |view, cx| view.save_example(name.trim(), cx))
                            .is_ok()
                })
        });
    }

    fn save_example(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(Ok(response)) = self.active_response() else {
            return;
        };
        let wire = &response.wire;
        let example = json!({
            "id": uuid::Uuid::new_v4().to_string(), "name": name.chars().take(80).collect::<String>(),
            "savedAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "status": wire.status, "statusText": wire.status_text, "headers": wire.headers,
            "body": wire.body, "bodyB64": wire.body_b64, "elapsedMs": wire.elapsed_ms as u64,
            "sizeBytes": wire.size_bytes, "truncated": wire.truncated, "note": wire.note,
        });
        let collection_id = self
            .active_collection()
            .and_then(|collection| collection["id"].as_str())
            .map(str::to_owned);
        let Some(request) = self.active_request_mut() else {
            return;
        };
        if !request["examples"].is_array() {
            request["examples"] = json!([]);
        }
        if let Some(examples) = request["examples"].as_array_mut() {
            examples.push(example);
        }
        if let Some(id) = collection_id {
            let request = request.clone();
            if let Err(error) = workspace_edit::update_request(self.workspace_mut(), &id, &request)
            {
                self.error = Some(error);
            }
        }
        self.changed(cx);
    }

    pub(super) fn save_response_body(&mut self, cx: &mut Context<Self>) {
        let Some(Ok(response)) = self.active_response() else {
            return;
        };
        let wire = &response.wire;
        let suffix = content_type(wire)
            .map(|mime| match mime.to_ascii_lowercase().as_str() {
                "application/json" => "json",
                "application/pdf" => "pdf",
                "text/html" | "application/xhtml+xml" => "html",
                "application/xml" | "text/xml" => "xml",
                "image/png" => "png",
                "image/jpeg" => "jpg",
                "application/octet-stream" => "bin",
                "text/csv" => "csv",
                _ => "txt",
            })
            .unwrap_or("txt");
        let file_name = format!("response.{suffix}");
        let binary = wire.body_b64.clone();
        let text = binary.is_none().then(|| wire.body.clone());
        let location = std::env::current_dir().unwrap_or_default();
        let prompt = cx.prompt_for_new_path(&location, Some(&file_name));
        cx.spawn(async move |view, cx| {
            let result = match prompt.await {
                Ok(Ok(Some(path))) => {
                    cx.background_executor()
                        .spawn(async move {
                            let content = match binary {
                                Some(encoded) => base64::engine::general_purpose::STANDARD
                                    .decode(encoded)
                                    .map_err(|error| error.to_string())?,
                                None => text.unwrap_or_default().into_bytes(),
                            };
                            std::fs::write(path, content)
                                .map_err(|error| format!("Could not save response: {error}"))
                        })
                        .await
                }
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(error)) => Err(format!("Could not open Save As: {error}")),
            };
            let _ = view.update(cx, |this, cx| {
                this.error = result.err();
                cx.notify();
            });
        })
        .detach();
    }
}

pub(super) fn is_html(mime: &str) -> bool {
    mime.eq_ignore_ascii_case("text/html") || mime.eq_ignore_ascii_case("application/xhtml+xml")
}

pub(super) fn content_type(wire: &postlite_core::ResponsePayload) -> Option<&str> {
    wire.headers
        .iter()
        .find(|header| header.key.eq_ignore_ascii_case("content-type"))
        .and_then(|header| header.value.split(';').next())
        .map(str::trim)
}

fn safe_html(source: &str) -> Option<String> {
    let parsed = parse_document(RcDom::default(), Default::default()).one(source);
    let mut output = String::new();
    let mut remaining = MAX_PREVIEW_NODES;
    write_preview_node(&parsed.document, &mut output, 0, &mut remaining)?;
    Some(output)
}

fn write_preview_node(
    node: &Handle,
    output: &mut String,
    depth: usize,
    remaining: &mut usize,
) -> Option<()> {
    *remaining = remaining.checked_sub(1)?;
    if depth >= 32 {
        return Some(());
    }
    let tag = match &node.data {
        NodeData::Element { name, .. } => match name.local.as_ref() {
            "head" | "script" | "style" | "template" | "noscript" | "iframe" | "object" => {
                return Some(());
            }
            tag @ ("h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" | "div" | "blockquote" | "ul"
            | "ol" | "li" | "pre" | "code" | "br" | "hr" | "em" | "i" | "strong" | "b"
            | "s" | "del" | "table" | "thead" | "tbody" | "tr" | "th" | "td") => Some(tag),
            _ => None,
        },
        NodeData::Document => None,
        NodeData::Text { contents } => {
            for character in contents.borrow().chars() {
                match character {
                    '&' => output.push_str("&amp;"),
                    '<' => output.push_str("&lt;"),
                    '>' => output.push_str("&gt;"),
                    _ => output.push(character),
                }
            }
            return Some(());
        }
        _ => return Some(()),
    };
    if let Some(tag) = tag {
        output.push('<');
        output.push_str(tag);
        output.push('>');
    }
    for child in node.children.borrow().iter() {
        write_preview_node(child, output, depth + 1, remaining)?;
    }
    if let Some(tag) = tag.filter(|tag| !matches!(*tag, "br" | "hr")) {
        output.push_str("</");
        output.push_str(tag);
        output.push('>');
    }
    Some(())
}

fn example_response(example: &Value) -> Response {
    let body = example["body"].as_str().unwrap_or_default().to_owned();
    let pretty = crate::http::pretty(&body, example["truncated"] == true);
    Response {
        pretty,
        wire: postlite_core::ResponsePayload {
            status: example["status"].as_u64().unwrap_or(0).min(999) as u16,
            status_text: super::val(example, "statusText").to_owned(),
            body,
            body_b64: example["bodyB64"].as_str().map(str::to_owned),
            headers: serde_json::from_value(example["headers"].clone()).unwrap_or_default(),
            truncated: example["truncated"] == true,
            elapsed_ms: example["elapsedMs"].as_u64().unwrap_or(0) as u128,
            size_bytes: example["sizeBytes"].as_u64().unwrap_or(0) as usize,
            error: None,
            note: example["note"].as_str().map(str::to_owned),
        },
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn html_preview_preserves_content_and_discards_active_markup() {
        let result = super::safe_html(
            r#"<head><title>Hidden title</title></head><h1>Report &amp; summary</h1>
            <strong>Bold</strong><script>hidden script</script><style>hidden style</style>
            <a href="file:///private">Link label</a><img src="https://example.test/image">
            <p onclick="evil()">&lt;img src=escaped&gt;</p>"#,
        )
        .unwrap();
        for expected in [
            "<h1>Report &amp; summary</h1>",
            "<strong>Bold</strong>",
            "Link label",
            "<p>&lt;img src=escaped&gt;</p>",
        ] {
            assert!(result.contains(expected), "missing {expected} in {result}");
        }
        for excluded in [
            "Hidden title",
            "hidden script",
            "hidden style",
            "file://",
            "https://",
            "onclick",
            "<img",
        ] {
            assert!(
                !result.contains(excluded),
                "unexpected {excluded} in {result}"
            );
        }
    }

    #[test]
    fn html_preview_rejects_excessive_element_counts() {
        let source = "<p>x</p>".repeat(super::MAX_PREVIEW_NODES);
        assert!(super::safe_html(&source).is_none());
    }
}
