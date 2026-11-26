#[path = "panel_edit.rs"]
mod edit;
pub(super) use edit::FieldEditor;
use edit::FieldTarget;

use super::Workbench;
use crate::appearance::{Palette, icon};
use gpui::{
    AnyElement, Bounds, ClipboardItem, Context, Div, FontWeight, Hsla, InteractiveElement,
    IntoElement, ParentElement, Rgba, Stateful, StatefulInteractiveElement, Styled, canvas, div,
    fill, point, px, size,
};
use gpui_component::{
    Sizable,
    button::Button,
    input::Input,
    menu::{DropdownMenu, PopupMenuItem},
};
use serde_json::{Value, json};

const REQUEST_TABS: [&str; 5] = ["Params", "Auth", "Headers", "Body", "Scripts"];
const RESPONSE_TABS: [&str; 5] = ["Pretty", "Raw", "Preview", "Headers", "Tests"];
const SECRET_MASK: &str = "••••••••";

impl Workbench {
    pub(super) fn request_panel(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let palette = &self.palette;
        let request = self.active_request().unwrap_or(&Value::Null);
        let params = values(&request["params"]);
        let path_vars = values(&request["pathVars"]);
        let headers = values(&request["headers"]);
        let counts = [
            params.iter().filter(|row| counted(row)).count() + path_vars.len(),
            0,
            headers.iter().filter(|row| counted(row)).count(),
            0,
            0,
        ];
        let tabs = section_tabs(
            "request",
            REQUEST_TABS,
            counts,
            self.req_section,
            palette,
            cx,
        );
        let section = match self.req_section {
            1 => auth_panel(self, request, cx),
            2 => kv_table(self, headers, Table::Headers, cx).child(add_caption(
                "+ Add header",
                Table::Headers,
                palette,
                cx,
            )),
            3 => body_panel(self, &request["body"], cx),
            4 => div()
                .flex()
                .flex_col()
                .gap_2()
                .min_h_full()
                .child(section_label("Pre-request script", palette))
                .child(editable(
                    self,
                    FieldTarget::Script("pre"),
                    code_surface(string(&request["scripts"]["pre"]), 72., palette),
                    false,
                    "Pre-request script",
                    cx,
                ))
                .child(section_label("Tests", palette))
                .child(editable(
                    self,
                    FieldTarget::Script("test"),
                    code_surface(string(&request["scripts"]["test"]), 72., palette),
                    false,
                    "Tests",
                    cx,
                )),
            _ => {
                let section = kv_table(self, params, Table::Query, cx).child(add_caption(
                    "+ Add param",
                    Table::Query,
                    palette,
                    cx,
                ));
                if path_vars.is_empty() {
                    section
                } else {
                    section
                        .child(
                            div()
                                .mt(px(14.))
                                .child(section_label("Path Variables", palette)),
                        )
                        .child(kv_table(self, path_vars, Table::Path, cx))
                }
            }
        };

        div()
            .id("request-panel")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(palette.panel)
            .child(
                panel_head("Request", palette)
                    .child(div().flex_1())
                    .child(
                        toolbar_label("Cookies", palette)
                            .id("cookies-drawer")
                            .cursor_pointer()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_cookies(window, cx)),
                            ),
                    )
                    .child(
                        toolbar_label("Code", palette)
                            .id("code-drawer")
                            .cursor_pointer()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_code(window, cx)),
                            ),
                    ),
            )
            .child(tabs)
            .child(
                div()
                    .id("request-section")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(14.))
                    .pt(px(10.))
                    .pb(px(12.))
                    .child(section),
            )
    }

    pub(super) fn response_panel(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let palette = &self.palette;
        let response = self.active_response();
        let wire = response
            .and_then(|response| response.as_ref().ok())
            .map(|response| &response.wire);
        let stage =
            match response {
                Some(Err(error)) => idle_response(Some(error), palette),
                Some(Ok(response)) => match self.res_section {
                    2 => centered_message("HTML and PDF previews are still being ported.", palette),
                    3 => response_headers(&response.wire.headers, self.mask_secrets, palette),
                    4 => centered_message("No tests ran for this response.", palette),
                    _ => div()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .overflow_hidden()
                        .font_family("monospace")
                        .text_size(px(12.))
                        .children(self.body.as_ref().map(|state| {
                            Input::new(state).h_full().appearance(false).disabled(true)
                        }))
                        .into_any_element(),
                },
                _ if self.is_sending() => centered_message("Sending request…", palette),
                _ => idle_response(None, palette),
            };

        let (status, color) = match response {
            Some(Ok(response)) => (
                format!("{} {}", response.wire.status, response.wire.status_text),
                match response.wire.status {
                    500.. => palette.error,
                    400..=499 => palette.warn,
                    300..=399 => palette.info,
                    _ => palette.ok,
                },
            ),
            Some(Err(_)) => ("failed".to_owned(), palette.error),
            _ if self.is_sending() => ("Sending".to_owned(), palette.dim),
            _ => ("No response".to_owned(), palette.dim),
        };
        let status_bg = if response.is_some() {
            translucent(color, 0.14)
        } else {
            palette.hover
        };
        let status_border = if response.is_some() {
            translucent(color, 0.35)
        } else {
            palette.line
        };
        let mut head = panel_head("Response", palette).child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .font_family("monospace")
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .bg(status_bg)
                .border_1()
                .border_color(status_border)
                .rounded(px(6.))
                .px(px(7.))
                .py(px(2.))
                .child(div().size(px(6.)).rounded_full().bg(color))
                .child(status),
        );
        if let Some(wire) = wire {
            head = head
                .child(metric(format!("{} ms", wire.elapsed_ms), palette))
                .child(metric(format_size(wire.size_bytes), palette));
            if let Some(note) = &wire.note {
                head = head.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(11.))
                        .text_color(palette.warn)
                        .child(note.clone()),
                );
            }
        }
        head = head.child(div().flex_1());
        if wire.is_some() {
            head = head
                .child(
                    div()
                        .id("copy-response")
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(26.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|style| style.bg(palette.hover))
                        .child(icon("copy", palette.dim, 14.))
                        .on_click(cx.listener(|this, _, _, cx| {
                            let Some(Ok(response)) = this.active_response() else {
                                return;
                            };
                            let wire = &response.wire;
                            let text = match this.res_section {
                                3 => wire
                                    .headers
                                    .iter()
                                    .map(|header| format!("{}: {}", header.key, header.value))
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                                4 => "No tests ran for this response.".to_owned(),
                                _ => wire.body.clone(),
                            };
                            if !text.is_empty() {
                                cx.write_to_clipboard(ClipboardItem::new_string(text));
                            }
                        })),
                )
                .child(
                    super::icon_button(
                        "save-response",
                        "export",
                        palette.faint,
                        palette.hover,
                        26.,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.save_response_body(cx))),
                );
        }
        head = head.child(self.request_examples(cx));

        let counts = [0, 0, 0, wire.map_or(0, |wire| wire.headers.len()), 0];
        div()
            .id("response-panel")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(palette.bg)
            .child(head)
            .children(self.example_banner(cx))
            .child(section_tabs(
                "response",
                RESPONSE_TABS,
                counts,
                self.res_section,
                palette,
                cx,
            ))
            .child(stage)
            .children(wire.filter(|wire| wire.truncated).map(|_| {
                div()
                    .border_t_1()
                    .border_color(palette.line)
                    .px(px(14.))
                    .py(px(8.))
                    .bg(palette.raised)
                    .text_size(px(12.))
                    .text_color(palette.warn)
                    .child("Response truncated at the configured size limit.")
            }))
    }
}

fn panel_head(title: &'static str, palette: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .h(px(match palette.appearance {
            crate::appearance::Appearance::Terminal => 32.,
            crate::appearance::Appearance::Studio => 43.,
            _ => 39.,
        }))
        .flex_shrink_0()
        .min_w_0()
        .px(px(14.))
        .gap(px(10.))
        .child(
            div()
                .font_family(palette.appearance.heading_font())
                .text_size(px(palette.appearance.heading()))
                .font_weight(FontWeight::BOLD)
                .text_color(palette.text)
                .child(title),
        )
}

fn section_tabs(
    panel: &'static str,
    labels: [&'static str; 5],
    counts: [usize; 5],
    selected: usize,
    palette: &Palette,
    cx: &mut Context<Workbench>,
) -> Stateful<Div> {
    div()
        .id(panel)
        .flex()
        .items_center()
        .h(px(42.))
        .gap(px(2.))
        .flex_shrink_0()
        .px(px(14.))
        .border_b_1()
        .border_color(palette.soft_line)
        .overflow_x_scroll()
        .children(
            labels
                .into_iter()
                .zip(counts)
                .enumerate()
                .map(|(index, (label, count))| {
                    div()
                        .id((panel, index))
                        .relative()
                        .flex()
                        .items_center()
                        .h_full()
                        .flex_shrink_0()
                        .px(px(11.))
                        .gap(px(5.))
                        .cursor_pointer()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if selected == index {
                            palette.text
                        } else {
                            palette.dim
                        })
                        .hover(|style| style.text_color(palette.text))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if panel == "request" {
                                if this.req_section != index {
                                    this.panel_editor = None;
                                    this.req_section = index;
                                }
                            } else {
                                this.res_section = index;
                                this.refresh_body(window, cx);
                            }
                            cx.notify();
                        }))
                        .child(label)
                        .children((count > 0).then(|| {
                            div()
                                .rounded_full()
                                .bg(palette.hover)
                                .border_1()
                                .border_color(palette.line)
                                .px(px(5.))
                                .text_size(px(10.5))
                                .font_family("monospace")
                                .text_color(palette.dim)
                                .child(count.to_string())
                        }))
                        .children((selected == index).then(|| {
                            div()
                                .absolute()
                                .bottom(px(0.))
                                .left(px(9.))
                                .right(px(9.))
                                .h(px(2.))
                                .rounded_full()
                                .bg(palette.accent)
                        }))
                }),
        )
}

fn toolbar_label(label: &'static str, palette: &Palette) -> Div {
    div()
        .px(px(6.))
        .text_size(px(12.))
        .text_color(palette.faint)
        .child(label)
}

fn metric(label: String, palette: &Palette) -> Div {
    div()
        .text_size(px(11.))
        .font_family("monospace")
        .text_color(palette.dim)
        .child(label)
}

fn section_label(label: &'static str, palette: &Palette) -> Div {
    div()
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(palette.faint)
        .child(label)
}

fn add_caption(
    label: &'static str,
    table: Table,
    palette: &Palette,
    cx: &mut Context<Workbench>,
) -> Div {
    div().flex().justify_center().child(
        div()
            .id(label)
            .py(px(8.))
            .text_size(px(13.))
            .text_color(palette.cyan)
            .cursor_pointer()
            .on_click(
                cx.listener(move |this, _, window, cx| this.add_request_row(table, window, cx)),
            )
            .child(label),
    )
}

fn code_surface(text: &str, min_height: f32, palette: &Palette) -> Div {
    div()
        .min_h(px(min_height))
        .border_1()
        .border_color(palette.line)
        .rounded(px(6.))
        .bg(palette.input)
        .p_2()
        .font_family("monospace")
        .text_size(px(12.))
        .text_color(palette.text)
        .child(text.to_owned())
}

fn auth_panel(workbench: &Workbench, request: &Value, cx: &mut Context<Workbench>) -> Div {
    let palette = &workbench.palette;
    let auth = &request["auth"];
    let kind = match auth["type"].as_str() {
        Some("apikey") => "apikey",
        Some("basic") => "basic",
        Some("bearer") => "bearer",
        Some("oauth2") => "oauth2",
        Some("noauth") => "noauth",
        _ => "inherit",
    };
    let types = [
        ("inherit", "Inherit auth from parent"),
        ("noauth", "No Auth"),
        ("bearer", "Bearer Token"),
        ("basic", "Basic Auth"),
        ("apikey", "API Key"),
        ("oauth2", "OAuth 2.0"),
    ];
    let label = types
        .iter()
        .find_map(|(value, label)| (*value == kind).then_some(*label))
        .unwrap_or("Inherit auth from parent");
    let fields: &[(&str, &str, bool)] = match kind {
        "apikey" => &[
            ("Key", "key", false),
            ("Value", "value", true),
            ("Add to", "addTo", false),
        ],
        "basic" => &[
            ("Username", "username", false),
            ("Password", "password", true),
        ],
        "bearer" => &[("Token", "token", true)],
        "oauth2" => &[
            ("Authorization URL", "authorizeUrl", false),
            ("Token URL", "tokenUrl", false),
            ("Client ID", "clientId", false),
            ("Scope", "scope", false),
            ("Redirect port (0 = random)", "redirectPort", false),
        ],
        _ => &[],
    };
    let view = cx.entity().downgrade();
    let auth_select = Button::new("auth-type")
        .outline()
        .small()
        .w(px(206.))
        .h(px(32.))
        .label(label)
        .child(icon("chevron-down", palette.faint, 12.))
        .dropdown_menu(move |mut menu, _, _| {
            for (id, label) in types {
                let view = view.clone();
                menu = menu.item(PopupMenuItem::new(label).checked(kind == id).on_click(
                    move |_, _, cx| {
                        let _ = view.update(cx, |this, cx| {
                            this.panel_editor = None;
                            this.set_request_field(FieldTarget::AuthType, json!(id), cx);
                        });
                    },
                ));
            }
            menu
        });
    let mut panel = div().flex().flex_col().gap_2().child(auth_select);
    let is_code = auth["oauth2"]["grantType"] == "authorization_code";
    if kind == "oauth2" {
        let view = cx.entity().downgrade();
        panel = panel.child(super::oauth_ui::oauth_choice(
            "oauth-grant",
            is_code,
            super::oauth_ui::GRANTS,
            move |code, _, cx| {
                let _ = view.update(cx, |app, cx| {
                    app.set_request_field(
                        FieldTarget::Auth("oauth2", "grantType"),
                        json!(if code {
                            "authorization_code"
                        } else {
                            "client_credentials"
                        }),
                        cx,
                    );
                });
            },
        ));
    }
    for &(name, key, secret) in fields {
        if kind == "oauth2" && !is_code && matches!(key, "authorizeUrl" | "redirectPort") {
            continue;
        }
        let masked = workbench.mask_secrets && secret;
        let number = auth[kind][key].as_u64().map(|n| n.to_string());
        let value = masked_value(
            number
                .as_deref()
                .unwrap_or_else(|| string(&auth[kind][key])),
            masked,
        );
        panel = panel.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(section_label(name, palette))
                .child(editable(
                    workbench,
                    FieldTarget::Auth(kind, key),
                    code_surface(value, 32., palette),
                    masked,
                    name,
                    cx,
                )),
        );
    }
    if kind == "oauth2" {
        let view = cx.entity().downgrade();
        panel = panel.child(super::oauth_ui::oauth_choice(
            "oauth-client-auth",
            auth["oauth2"]["clientAuthIn"] == "basic",
            super::oauth_ui::CLIENT_AUTH,
            move |basic, _, cx| {
                let _ = view.update(cx, |app, cx| {
                    app.set_request_field(
                        FieldTarget::Auth("oauth2", "clientAuthIn"),
                        json!(if basic { "basic" } else { "body" }),
                        cx,
                    );
                });
            },
        ));
    }
    panel.child(workbench.oauth_controls(cx))
}

fn body_panel(workbench: &Workbench, body: &Value, cx: &mut Context<Workbench>) -> Div {
    let palette = &workbench.palette;
    let mode = body["mode"].as_str().unwrap_or("none");
    let controls =
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .mb(px(10.))
            .children(
                ["none", "form-data", "urlencoded", "raw"]
                    .into_iter()
                    .map(|label| {
                        let active = label == mode;
                        let border = if active { palette.accent } else { palette.line };
                        div()
                            .id(label)
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .px(px(8.))
                            .py(px(4.))
                            .rounded(px(8.))
                            .text_size(px(12.))
                            .cursor_pointer()
                            .text_color(if active { palette.text } else { palette.dim })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.panel_editor = None;
                                this.set_request_field(FieldTarget::Body("mode"), json!(label), cx);
                            }))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(13.))
                                    .rounded_full()
                                    .border_1()
                                    .border_color(border)
                                    .children(active.then(|| {
                                        div().size(px(5.)).rounded_full().bg(palette.accent)
                                    })),
                            )
                            .child(label)
                    }),
            );
    let panel = div().flex().flex_col().min_h_full().child(controls);
    match mode {
        "raw" => panel
            .child(section_label("Content type", palette))
            .child(editable(
                workbench,
                FieldTarget::Body("contentType"),
                cell(string(&body["contentType"]).to_owned()),
                false,
                "Content type",
                cx,
            ))
            .child(editable(
                workbench,
                FieldTarget::Body("raw"),
                code_surface(string(&body["raw"]), 132., palette),
                false,
                "Request body",
                cx,
            )),
        "form-data" | "urlencoded" => {
            let table = if mode == "form-data" {
                Table::Multipart
            } else {
                Table::UrlEncoded
            };
            panel
                .child(kv_table(workbench, values(&body["form"]), table, cx))
                .child(add_caption("+ Add field", table, palette, cx))
        }
        _ => panel.child(div().text_size(px(12.)).text_color(palette.faint).child(
            "This request has no body. Switch to form-data, urlencoded, or raw to add one.",
        )),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Table {
    Query,
    Headers,
    Multipart,
    UrlEncoded,
    Path,
}

fn editable(
    workbench: &Workbench,
    target: FieldTarget,
    fallback: Div,
    masked: bool,
    placeholder: &'static str,
    cx: &mut Context<Workbench>,
) -> Div {
    if let Some(input) = workbench.field_input(target) {
        let editor = Input::new(input).appearance(false).small();
        return if matches!(target, FieldTarget::Body("raw") | FieldTarget::Script(_)) {
            div().w_full().child(editor.h(px(132.)))
        } else {
            div().flex_1().min_w_0().child(editor)
        };
    }
    let id = gpui::SharedString::from(format!("{target:?}"));
    div().flex_1().min_w_0().child(
        div()
            .id(id)
            .cursor_text()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.edit_field(target, masked, placeholder, window, cx)
            }))
            .child(fallback),
    )
}

fn kv_table(
    workbench: &Workbench,
    rows: &[Value],
    table: Table,
    cx: &mut Context<Workbench>,
) -> Div {
    let palette = &workbench.palette;
    let is_path = table == Table::Path;
    let has_description = matches!(table, Table::Query | Table::Multipart | Table::UrlEncoded);
    let has_type = table == Table::Multipart;
    let columns = div()
        .flex()
        .items_center()
        .h(px(26.))
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(palette.faint)
        .children((!is_path).then(|| div().w(px(28.)).flex_none()))
        .child(cell("Key"))
        .children(has_type.then(|| cell("Type").flex_none().w(px(62.))))
        .child(cell("Value"))
        .children(has_description.then(|| cell("Description")))
        .children((!is_path).then(|| div().w(px(23.)).flex_none()));
    let mut table_body = div().flex().flex_col().child(columns);
    for (index, row) in rows.iter().enumerate() {
        let key = string(&row["key"]);
        let enabled = row["enabled"] != false;
        let is_file = has_type && row["type"] == "file";
        let mask_value = workbench.mask_secrets
            && (row["secret"] == true || table == Table::Headers && sensitive_name(key));
        let value = masked_value(string(&row["value"]), mask_value);
        let displayed = if is_file {
            value
                .rsplit(['/', '\\'])
                .next()
                .filter(|name| !name.is_empty())
                .unwrap_or("Select file…")
        } else {
            value
        };
        let description = string(&row["description"]);
        let mut element = div()
            .flex()
            .items_center()
            .h(px(32.))
            .border_b_1()
            .border_color(palette.soft_line)
            .font_family("monospace")
            .text_size(px(12.))
            .text_color(if enabled { palette.text } else { palette.faint });
        if is_path {
            element = element.child(cell(format!(":{key}")).text_color(palette.dim));
        } else {
            element = element
                .child(
                    div()
                        .id(("toggle-row", index))
                        .cursor_pointer()
                        .child(checkbox(enabled, palette))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_request_field(
                                FieldTarget::Row(table, index, "enabled"),
                                json!(!enabled),
                                cx,
                            )
                        })),
                )
                .child(editable(
                    workbench,
                    FieldTarget::Row(table, index, "key"),
                    cell(key.to_owned()),
                    false,
                    "Key",
                    cx,
                ));
        }
        if has_type {
            element = element.child(
                div()
                    .id(("form-type", index))
                    .w(px(62.))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_request_field(
                            FieldTarget::Row(table, index, "type"),
                            json!(if is_file { "text" } else { "file" }),
                            cx,
                        )
                    }))
                    .child(cell(if is_file { "File" } else { "Text" })),
            );
        }
        element = element.child(editable(
            workbench,
            FieldTarget::Row(table, index, "value"),
            cell(displayed.to_owned()),
            mask_value,
            if is_file { "File path" } else { "Value" },
            cx,
        ));
        if has_description {
            let placeholder = if description.is_empty() {
                cell("Description").text_color(palette.faint)
            } else {
                cell(description.to_owned())
            };
            element = element.child(editable(
                workbench,
                FieldTarget::Row(table, index, "description"),
                placeholder,
                false,
                "Description",
                cx,
            ));
        }
        if !is_path {
            element =
                element.child(
                    div()
                        .id(("remove-row", index))
                        .flex()
                        .justify_center()
                        .w(px(23.))
                        .flex_none()
                        .cursor_pointer()
                        .child(icon("x", palette.faint, 12.))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.remove_request_row(table, index, cx)
                        })),
                );
        }
        table_body = table_body.child(element);
    }
    table_body
}

fn checkbox(checked: bool, palette: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(28.))
        .flex_shrink_0()
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(14.))
                .rounded(px(4.))
                .border_1()
                .border_color(if checked { palette.text } else { palette.line })
                .bg(if checked { palette.text } else { palette.input })
                .children(checked.then(|| icon("check", palette.input, 10.))),
        )
}

fn cell(text: impl Into<gpui::SharedString>) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .px(px(8.))
        .truncate()
        .child(text.into())
}

fn response_headers(
    headers: &[postlite_core::HeaderPair],
    masked: bool,
    palette: &Palette,
) -> AnyElement {
    div()
        .id("response-header-table")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .px(px(14.))
        .py(px(8.))
        .font_family("monospace")
        .text_size(px(12.))
        .children(headers.iter().map(|header| {
            div()
                .flex()
                .items_center()
                .min_h(px(28.))
                .border_b_1()
                .border_color(palette.soft_line)
                .child(cell(header.key.clone()).text_color(palette.dim))
                .child(
                    cell(
                        masked_value(&header.value, masked && sensitive_name(&header.key))
                            .to_owned(),
                    )
                    .text_color(palette.text),
                )
        }))
        .into_any_element()
}

fn centered_message(message: &'static str, palette: &Palette) -> AnyElement {
    div()
        .flex()
        .flex_1()
        .min_h_0()
        .items_center()
        .justify_center()
        .text_size(px(12.))
        .text_color(palette.dim)
        .child(message)
        .into_any_element()
}

fn idle_response(error: Option<&str>, palette: &Palette) -> AnyElement {
    let card = div().flex().flex_col().items_center().gap(px(8.));
    let card = if let Some(error) = error {
        card.max_w(px(560.))
            .px(px(24.))
            .py(px(18.))
            .border_1()
            .border_color(translucent(palette.error, 0.32))
            .rounded(px(12.))
            .bg(translucent(palette.error, 0.05))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(34.))
                    .border_1()
                    .border_color(translucent(palette.error, 0.32))
                    .rounded(px(8.))
                    .bg(translucent(palette.error, 0.12))
                    .child(icon("x", palette.error, 16.)),
            )
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Request failed"),
            )
            .child(
                div()
                    .max_w_full()
                    .font_family("monospace")
                    .text_size(px(12.))
                    .text_color(palette.error)
                    .child(error.to_owned()),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(palette.dim)
                    .child("Check the URL, connection, and TLS settings, then send again."),
            )
    } else {
        card.child(icon("braces", palette.accent, 27.))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("No response yet"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(palette.dim)
                    .child("Send a request: status, timing, and body land here."),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .mt(px(6.))
                    .text_size(px(11.))
                    .text_color(palette.faint)
                    .child(keycap("Ctrl", palette))
                    .child("+")
                    .child(keycap("↵", palette))
                    .child(" to send"),
            )
    };
    div()
        .relative()
        .flex()
        .flex_1()
        .min_h_0()
        .items_center()
        .justify_center()
        .overflow_hidden()
        .p(px(24.))
        .child(dot_grid(palette))
        .child(card)
        .into_any_element()
}

fn keycap(text: &'static str, palette: &Palette) -> Div {
    div()
        .rounded(px(4.))
        .border_1()
        .border_color(palette.line)
        .bg(palette.hover)
        .px(px(5.))
        .font_family("monospace")
        .text_color(palette.dim)
        .child(text)
}

fn dot_grid(palette: &Palette) -> impl IntoElement {
    let color = translucent(palette.dim, 0.14);
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let step = px(22.);
            let mut y = bounds.origin.y + step / 2.;
            while y < bounds.bottom() {
                let mut x = bounds.origin.x + step / 2.;
                while x < bounds.right() {
                    window.paint_quad(fill(
                        Bounds::new(point(x, y), size(px(1.6), px(1.6))),
                        Hsla::from(color),
                    ));
                    x += step;
                }
                y += step;
            }
        },
    )
    .absolute()
    .inset_0()
    .size_full()
}

fn translucent(color: Rgba, a: f32) -> Rgba {
    Rgba { a, ..color }
}

fn sensitive_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "authorization" | "proxy-authorization" | "cookie" | "set-cookie"
    ) || ["token", "secret", "key"]
        .into_iter()
        .any(|part| lower.contains(part))
}

fn masked_value(value: &str, mask: bool) -> &str {
    if mask && !value.is_empty() {
        SECRET_MASK
    } else {
        value
    }
}

fn values(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

fn string(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}

fn counted(row: &Value) -> bool {
    row["enabled"] != false && !string(&row["key"]).is_empty()
}

fn format_size(bytes: usize) -> String {
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1_048_575 => format!("{:.1} kB", bytes as f64 / 1024.),
        _ => format!("{:.2} MB", bytes as f64 / 1_048_576.),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_header_mask_matches_existing_sensitive_names() {
        for key in [
            "Authorization",
            "Proxy-Authorization",
            "cookie",
            "Set-Cookie",
            "X-API-KEY",
            "refresh_TOKEN",
            "secret",
        ] {
            assert_eq!(
                masked_value("private value", sensitive_name(key)),
                SECRET_MASK
            );
        }
        assert_eq!(
            masked_value("visible", sensitive_name("Content-Type")),
            "visible"
        );
        assert_eq!(masked_value("", true), "");
    }

    #[test]
    fn response_size_uses_the_existing_display_units() {
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.0 kB");
        assert_eq!(format_size(1_048_576), "1.00 MB");
    }
}
