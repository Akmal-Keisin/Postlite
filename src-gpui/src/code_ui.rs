use super::Workbench;
use crate::{curl, prepare};
use gpui::{AppContext, ClipboardItem, Context, ParentElement, Styled, Window, div, px};
use gpui_component::{
    Sizable, WindowExt,
    button::{Button, ButtonVariants},
    input::{Input, InputState},
};

impl Workbench {
    pub(super) fn open_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_url(cx);
        let Some(request) = self.active_request() else {
            return;
        };
        let workspace = self.workspace();
        let located = request["id"]
            .as_str()
            .and_then(|id| workspace.find_request(id));
        let collection = located.as_ref().map(|node| node.collection);
        let ancestors = located
            .as_ref()
            .map(|node| node.ancestors.as_slice())
            .unwrap_or_default();
        let environment = collection.and_then(|node| self.bound_environment(node));
        let snippet = prepare::preview(
            request,
            collection,
            ancestors,
            environment,
            &self.doc.settings,
            &workspace.id,
        )
        .map(|mut payload| {
            if let Ok(Some(config)) =
                prepare::oauth_config(request, collection, ancestors, &workspace.id)
                && let Some(token) = self.session.oauth.cached(&config)
            {
                crate::http::add_bearer(&mut payload, &token);
            }
            curl::export(&payload)
        });
        let state = cx.new(|cx| InputState::new(window, cx).multi_line(true).soft_wrap(true));
        state.update(cx, |editor, cx| {
            editor.set_value(
                snippet.as_deref().unwrap_or_else(|error| error).to_owned(),
                window,
                cx,
            )
        });
        let view = cx.entity().downgrade();
        window.open_sheet(cx, move |sheet, _, _| {
            let weak = view.clone();
            let editor = state.clone();
            let snippet = snippet.clone();
            sheet.title("Code").size(px(390.)).child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child("Shell / cURL")
                    .child(Input::new(&editor).h(px(320.)).disabled(true))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .children(snippet.ok().map(|code| {
                                Button::new("copy-curl")
                                    .primary()
                                    .small()
                                    .label("Copy")
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            code.clone(),
                                        ))
                                    })
                            }))
                            .child(
                                Button::new("import-curl")
                                    .outline()
                                    .small()
                                    .label("Import cURL")
                                    .on_click(move |_, window, cx| {
                                        let _ = weak.update(cx, |view, cx| {
                                            view.open_curl_import(window, cx)
                                        });
                                    }),
                            ),
                    ),
            )
        });
    }

    pub(super) fn open_curl_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder("Paste a cURL command")
        });
        let error = cx.new(|_| String::new());
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let state = input.clone();
            let status = error.clone();
            let view = weak.clone();
            dialog
                .title("Import cURL")
                .width(px(680.))
                .confirm()
                .child(Input::new(&input).h(px(215.)))
                .child(div().text_size(px(12.)).child(error.read(cx).clone()))
                .on_ok(move |_, window, cx| {
                    let command = state.read(cx).value();
                    match curl::import(&command) {
                        Ok(import) => {
                            let _ = view.update(cx, |this, cx| {
                                this.open_tab(import.request, window, cx);
                                if !import.skipped.is_empty() {
                                    this.error = Some(format!(
                                        "cURL imported. Not carried over: {}",
                                        import.skipped.join(", ")
                                    ));
                                }
                                cx.notify();
                            });
                            window.close_sheet(cx);
                            true
                        }
                        Err(message) => {
                            status.update(cx, |error, cx| {
                                *error = message;
                                cx.notify();
                            });
                            cx.refresh_windows();
                            false
                        }
                    }
                })
        });
    }
}
