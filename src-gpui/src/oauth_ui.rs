use std::time::Duration;

use gpui::{AppContext, ClipboardItem, Context, Div, ParentElement, Styled, Window, div, px};
use gpui_component::{
    Disableable, Sizable, WindowExt,
    button::{Button, ButtonVariants},
    dialog::DialogButtonProps,
    input::Input,
    menu::{DropdownMenu, PopupMenuItem},
};
use serde_json::{Value, json};

use super::{Workbench, session_ui::text_input};
use crate::{
    http,
    oauth::{Config, Status},
    prepare,
};

impl Workbench {
    pub(super) fn edit_parent_oauth(
        &self,
        id: &str,
        auth: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let fields = [
            ("authorizeUrl", "Authorization URL"),
            ("tokenUrl", "Token URL"),
            ("clientId", "Client ID"),
            ("scope", "Scope"),
            ("redirectPort", "Redirect port (0 = random)"),
        ]
        .map(|(key, label)| {
            let number = auth[key].as_u64().map(|value| value.to_string());
            let initial = auth[key].as_str().or(number.as_deref()).unwrap_or_default();
            (key, text_input(label, initial, false, window, cx))
        });
        let grant = cx.new(|_| auth["grantType"] == "authorization_code");
        let basic = cx.new(|_| auth["clientAuthIn"] == "basic");
        let workspace = self.workspace().id.clone();
        let node_id = id.to_owned();
        let generation = self.document_generation;
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let form = div().flex().flex_col().gap(px(8.))
                .child(oauth_choice("parent-grant", *grant.read(cx), GRANTS, {
                    let grant = grant.clone();
                    move |code, window, cx| {
                        grant.update(cx, |grant, _| *grant = code);
                        window.refresh();
                    }
                }))
                .children(fields.iter().map(|(_, input)| Input::new(input)))
                .child(oauth_choice("parent-client-auth", *basic.read(cx), CLIENT_AUTH, {
                    let basic = basic.clone();
                    move |header, window, cx| {
                        basic.update(cx, |basic, _| *basic = header);
                        window.refresh();
                    }
                }));
            let (fields, grant, basic, workspace, id, view) =
                (fields.clone(), grant.clone(), basic.clone(), workspace.clone(), node_id.clone(), view.clone());
            dialog.title("OAuth 2.0 settings").confirm()
                .button_props(DialogButtonProps::default().ok_text("Save"))
                .child(form)
                .on_ok(move |_, _, cx| {
                    let mut oauth = json!({
                        "grantType": if *grant.read(cx) { "authorization_code" } else { "client_credentials" },
                        "clientAuthIn": if *basic.read(cx) { "basic" } else { "body" },
                    });
                    for (key, input) in &fields {
                        oauth[*key] = json!(input.read(cx).value().trim());
                    }
                    let _ = view.update(cx, |app, cx| {
                        if app.document_generation != generation { return; }
                        let Some(node) = app.doc.workspaces.iter_mut()
                            .find(|item| item.id == workspace)
                            .and_then(|item| crate::workspace_edit::find_node_mut(&mut item.collections, &id))
                        else { return; };
                        node["auth"]["type"] = json!("oauth2");
                        node["auth"]["oauth2"] = oauth;
                        app.session.oauth.invalidate(&prepare::scoped_owner(&workspace, &id));
                        app.changed(cx);
                    });
                    true
                })
        });
    }

    fn active_oauth(&self) -> Option<&Value> {
        let request = self.active_request()?;
        let located = request["id"]
            .as_str()
            .and_then(|id| self.workspace().find_request(id));
        prepare::oauth_owner(
            request,
            located.as_ref().map(|node| node.collection),
            located
                .as_ref()
                .map(|node| node.ancestors.as_slice())
                .unwrap_or_default(),
        )
    }

    pub(super) fn active_oauth_owner(&self) -> Option<String> {
        let id = self.active_oauth()?["id"]
            .as_str()
            .filter(|id| !id.is_empty())?;
        Some(prepare::scoped_owner(&self.workspace().id, id))
    }

    pub(super) fn oauth_controls(&self, cx: &mut Context<Self>) -> Div {
        match (self.active_oauth(), self.active_oauth_owner()) {
            (Some(node), Some(owner)) => {
                self.controls_for_oauth(owner, &node["auth"]["oauth2"], cx)
            }
            _ => div(),
        }
    }

    fn controls_for_oauth(&self, owner: String, auth: &Value, cx: &mut Context<Self>) -> Div {
        let config = Config::from_auth(&owner, auth);
        let invalid = config.as_ref().err().cloned();
        let status = self.session.oauth.status(&owner);
        let busy = matches!(
            status,
            Some(Status::Exchanging | Status::WaitingForBrowser { .. })
        );
        let oauth = auth["grantType"] == "authorization_code";
        let secret_owner = owner.clone();
        let start_owner = owner.clone();
        let begin = Button::new("oauth-authorize")
            .primary()
            .small()
            .disabled(busy || config.is_err())
            .label(if oauth { "Authorize" } else { "Get token" })
            .on_click(cx.listener(move |app, _, _, cx| {
                if let Ok(config) = config.clone() {
                    app.begin_oauth(start_owner.clone(), config, cx);
                }
            }));
        let actions = div()
            .flex()
            .flex_wrap()
            .gap(px(6.))
            .child(
                Button::new("oauth-client-secret")
                    .outline()
                    .small()
                    .label("Client secret")
                    .on_click(cx.listener(move |app, _, window, cx| {
                        app.edit_oauth_secret(secret_owner.clone(), window, cx);
                    })),
            )
            .child(begin);
        let mut group = div().flex().flex_col().gap(px(6.));
        if oauth {
            let port = auth["redirectPort"]
                .as_str()
                .and_then(|port| port.trim().parse::<u16>().ok())
                .or_else(|| {
                    auth["redirectPort"]
                        .as_u64()
                        .and_then(|port| u16::try_from(port).ok())
                });
            group = group.child(
                div()
                    .text_size(px(11.))
                    .text_color(self.palette.faint)
                    .child(match port.filter(|port| *port != 0) {
                        Some(port) => {
                            format!("Redirect URI to register: http://127.0.0.1:{port}/callback")
                        }
                        None => {
                            "Allow a dynamic loopback port for http://127.0.0.1/callback.".into()
                        }
                    }),
            );
        }
        let mut actions = actions;
        if busy {
            let cancel_owner = owner.clone();
            actions = actions.child(
                Button::new("oauth-cancel")
                    .outline()
                    .small()
                    .label("Cancel")
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.session.oauth.cancel(&cancel_owner);
                        cx.notify();
                    })),
            );
        }
        let message = match status {
            Some(Status::WaitingForBrowser { redirect_uri, .. }) => {
                actions = actions
                    .child(
                        Button::new("oauth-paste-code")
                            .outline()
                            .small()
                            .label("Paste code")
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.paste_oauth_code(owner.clone(), window, cx);
                            })),
                    )
                    .child(
                        Button::new("oauth-copy-redirect")
                            .outline()
                            .small()
                            .label("Copy redirect URI")
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    redirect_uri.clone(),
                                ))
                            }),
                    );
                Some("Waiting for browser authorization…".into())
            }
            Some(Status::Exchanging) => Some("Requesting an access token…".into()),
            Some(Status::Ready) => Some("Access token ready".into()),
            Some(Status::Failed(error)) => Some(error),
            Some(Status::Cancelled) => Some("Authorization cancelled".into()),
            None => invalid,
        };
        group.child(actions).children(message.map(|message| {
            div()
                .text_size(px(11.))
                .text_color(self.palette.faint)
                .child(message)
        }))
    }

    fn begin_oauth(&mut self, owner: String, config: Config, cx: &mut Context<Self>) {
        let session = self.session.clone();
        let runtime = match http::runtime() {
            Ok(runtime) => runtime,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let task = runtime.spawn(async move { session.oauth.authorize(config).await });
        self.watch_oauth(owner, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    this.error = Some(error.to_string());
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn watch_oauth(&self, owner: String, cx: &mut Context<Self>) {
        let session = self.session.clone();
        let timer = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let mut starting = 5_u8;
            loop {
                timer.timer(Duration::from_millis(150)).await;
                let running = matches!(
                    session.oauth.status(&owner),
                    Some(Status::Exchanging | Status::WaitingForBrowser { .. })
                );
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
                if !running && starting == 0 {
                    break;
                }
                starting = starting.saturating_sub(1);
            }
        })
        .detach();
    }

    pub(super) fn forget_oauth(&self, owners: Vec<String>, cx: &mut Context<Self>) {
        if owners.is_empty() {
            return;
        }
        for owner in &owners {
            self.session.oauth.invalidate(owner);
        }
        let session = self.session.clone();
        let clear = cx.background_executor().spawn(async move {
            let mut error = None;
            for owner in owners {
                if let Err(message) = session.secrets.set_oauth_secret(&owner, None) {
                    error = Some(message);
                }
            }
            error
        });
        cx.spawn(async move |this, cx| {
            if let Some(error) = clear.await {
                let _ = this.update(cx, |app, cx| {
                    app.error = Some(error);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn edit_oauth_secret(&self, owner: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = text_input("Client secret", "", true, window, cx);
        let ready = cx.new(|_| false);
        let status = cx.new(|_| None::<String>);
        let load = self.session.secrets.clone();
        let key = owner.clone();
        let task = cx
            .background_executor()
            .spawn(async move { load.oauth_secret(&key) });
        let (state, loaded, notice) = (input.clone(), ready.clone(), status.clone());
        cx.spawn_in(window, async move |this, cx| {
            let secret = task.await;
            let _ = this.update_in(cx, |_, window, cx| {
                match secret {
                    Ok(secret) => {
                        if state.read(cx).value().is_empty() {
                            state.update(cx, |input, cx| {
                                input.set_value(secret.unwrap_or_default(), window, cx)
                            });
                        }
                        loaded.update(cx, |ready, _| *ready = true);
                    }
                    Err(error) => notice.update(cx, |notice, _| *notice = Some(error)),
                }
                window.refresh();
            });
        })
        .detach();
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let (input, ready, owner, view) =
                (input.clone(), ready.clone(), owner.clone(), view.clone());
            let message = status.read(cx).clone();
            dialog
                .title("OAuth 2.0 client secret")
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("Save"))
                .child(Input::new(&input))
                .child(message.unwrap_or_else(|| "Leave blank to remove the secret.".into()))
                .on_ok(move |_, _, cx| {
                    let value = input.read(cx).value().to_string();
                    if !*ready.read(cx) && value.is_empty() {
                        return false;
                    }
                    view.update(cx, |app, cx| {
                        let session = app.session.clone();
                        let key = owner.clone();
                        let save = cx.background_executor().spawn(async move {
                            session.secrets.set_oauth_secret(
                                &key,
                                (!value.is_empty()).then_some(value.as_str()),
                            )?;
                            session.oauth.invalidate(&key);
                            Ok::<_, String>(())
                        });
                        cx.spawn(async move |this, cx| {
                            let result = save.await;
                            let _ = this.update(cx, |app, cx| {
                                if let Err(error) = result {
                                    app.error = Some(error);
                                }
                                cx.notify();
                            });
                        })
                        .detach();
                    })
                    .is_ok()
                })
        });
    }

    fn paste_oauth_code(&self, owner: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = text_input("Redirect URL or authorization code", "", true, window, cx);
        let error = cx.new(|_| None::<String>);
        let session = self.session.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let (input, error, session, owner) =
                (input.clone(), error.clone(), session.clone(), owner.clone());
            let mut form = div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(Input::new(&input));
            if let Some(message) = error.read(cx) {
                form = form.child(message.clone());
            }
            dialog
                .title("Paste authorization code")
                .confirm()
                .child(form)
                .on_ok(move |_, window, cx| {
                    match session
                        .oauth
                        .submit_code(&owner, input.read(cx).value().as_ref())
                    {
                        Ok(()) => true,
                        Err(message) => {
                            error.update(cx, |error, _| *error = Some(message));
                            window.refresh();
                            false
                        }
                    }
                })
        });
    }
}

pub(super) const GRANTS: [&str; 2] = ["Client credentials", "Authorization code (PKCE)"];
pub(super) const CLIENT_AUTH: [&str; 2] = ["Send credentials in body", "Send as Basic Auth"];

pub(super) fn oauth_choice(
    id: &'static str,
    selected: bool,
    options: [&'static str; 2],
    select: impl Fn(bool, &mut Window, &mut gpui::App) + Clone + 'static,
) -> impl gpui::IntoElement {
    Button::new(id)
        .small()
        .outline()
        .label(options[usize::from(selected)])
        .dropdown_menu(move |mut menu, _, _| {
            for (index, label) in options.into_iter().enumerate() {
                let select = select.clone();
                let value = index == 1;
                menu = menu.item(
                    PopupMenuItem::new(label)
                        .checked(selected == value)
                        .on_click(move |_, window, cx| select(value, window, cx)),
                );
            }
            menu
        })
}

pub(super) fn oauth_owners<'a>(
    workspace: &str,
    nodes: impl IntoIterator<Item = &'a Value>,
) -> Vec<String> {
    fn collect(workspace: &str, node: &Value, owners: &mut Vec<String>) {
        if node["auth"]["type"] == "oauth2"
            && let Some(id) = node["id"].as_str()
        {
            owners.push(prepare::scoped_owner(workspace, id));
        }
        if let Some(items) = node["items"].as_array() {
            for item in items {
                collect(workspace, item, owners);
            }
        }
    }
    let mut owners = Vec::new();
    for node in nodes {
        collect(workspace, node, &mut owners);
    }
    owners
}
