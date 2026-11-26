use gpui::{AppContext, Context, Entity, ParentElement, Styled, Window, div, px};
use gpui_component::{
    WindowExt,
    dialog::DialogButtonProps,
    input::{Input, InputState},
};
use postlite_core::ProxyCredentials;
use serde_json::json;

use super::Workbench;

impl Workbench {
    pub(super) fn refresh_cookies(&self, cx: &mut Context<Self>) {
        if let Some(drawer) = &self.cookies_drawer {
            let _ = drawer.update(cx, |drawer, cx| drawer.refresh(cx));
        }
    }

    pub(super) fn dismiss_cookies(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .cookies_drawer
            .take()
            .and_then(|drawer| drawer.upgrade())
            .is_some()
        {
            window.close_sheet(cx);
        }
    }

    pub(super) fn sync_cookie_preference(&self, cx: &mut Context<Self>) {
        let session = self.session.clone();
        session.request_cookie_setting(self.doc.settings["request"]["persistCookies"] != false);
        let update = cx.background_executor().spawn(async move {
            session.update_cookie_setting();
        });
        cx.spawn(async move |this, cx| {
            update.await;
            let _ = this.update(cx, |this, cx| this.refresh_cookies(cx));
        })
        .detach();
    }

    pub(super) fn confirm_clear_cookies(&self, window: &mut Window, cx: &mut Context<Self>) {
        let workspace = self.workspace().id.clone();
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let view = view.clone();
            let workspace = workspace.clone();
            dialog
                .title("Clear cookies?")
                .confirm()
                .child("Remove all cookies for this workspace.")
                .on_ok(move |_, _, cx| {
                    let _ = view.update(cx, |this, cx| this.purge_cookies(workspace.clone(), cx));
                    true
                })
        });
    }

    pub(super) fn purge_cookies(&self, workspace: String, cx: &mut Context<Self>) {
        let session = self.session.clone();
        let task = cx
            .background_executor()
            .spawn(async move { session.cookies.purge(&workspace) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    this.error = Some(error);
                    cx.notify();
                }
                this.refresh_cookies(cx);
            });
        })
        .detach();
    }

    pub(super) fn open_proxy(&self, window: &mut Window, cx: &mut Context<Self>) {
        let preset = &self.doc.settings["request"]["proxy"];
        let url = text_input(
            "Proxy URL",
            preset["url"].as_str().unwrap_or_default(),
            false,
            window,
            cx,
        );
        let bypass = text_input(
            "Bypass hosts",
            preset["bypass"].as_str().unwrap_or_default(),
            false,
            window,
            cx,
        );
        let username = text_input("Username", "", false, window, cx);
        let password = text_input("Password", "", true, window, cx);
        let original = cx.new(|_| None::<ProxyCredentials>);
        let notice = cx.new(|_| None::<String>);
        let secret_store = self.session.secrets.clone();
        let load = cx
            .background_executor()
            .spawn(async move { secret_store.proxy_credentials() });
        let (user, pass, baseline, load_notice) = (
            username.clone(),
            password.clone(),
            original.clone(),
            notice.clone(),
        );
        cx.spawn_in(window, async move |this, cx| {
            let result = load.await;
            let _ = this.update_in(cx, |_, window, cx| {
                match result {
                    Ok(credentials) => {
                        let credentials = credentials.unwrap_or_default();
                        if user.read(cx).value().is_empty() && pass.read(cx).value().is_empty() {
                            user.update(cx, |input, cx| {
                                input.set_value(credentials.username.clone(), window, cx)
                            });
                            pass.update(cx, |input, cx| {
                                input.set_value(credentials.password.clone(), window, cx)
                            });
                        }
                        baseline.update(cx, |value, _| *value = Some(credentials));
                    }
                    Err(error) => load_notice.update(cx, |value, _| *value = Some(error)),
                }
                window.refresh();
            });
        })
        .detach();

        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let (url, bypass, username, password) =
                (url.clone(), bypass.clone(), username.clone(), password.clone());
            let view = view.clone();
            let original = original.clone();
            let notice = notice.clone();
            let mut form = div().flex().flex_col().gap(px(9.))
                .child(Input::new(&url))
                .child(Input::new(&bypass))
                .child(Input::new(&username))
                .child(Input::new(&password));
            if let Some(error) = notice.read(cx) {
                form = form.child(error.clone());
            }
            dialog
                .title("Manual proxy")
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("Save"))
                .child(form)
                .on_ok(move |_, window, cx| {
                    let proxy_url = url.read(cx).value().trim().to_owned();
                    if let Err(error) = validate_proxy_url(&proxy_url) {
                        notice.update(cx, |message, _| *message = Some(error.into()));
                        window.refresh();
                        return false;
                    }
                    let bypass = bypass.read(cx).value().trim().to_owned();
                    let credentials = ProxyCredentials {
                        username: username.read(cx).value().to_string(),
                        password: password.read(cx).value().to_string(),
                    };
                    let previous = original.read(cx);
                    let changed = previous.as_ref().map_or(
                        !credentials.username.is_empty() || !credentials.password.is_empty(),
                        |old| old.username != credentials.username || old.password != credentials.password,
                    );
                    view.update(cx, |this, cx| {
                        let session = this.session.clone();
                        let generation = this.document_generation;
                        let save = cx.background_executor().spawn(async move {
                            if changed { session.secrets.set_proxy_credentials(&credentials) } else { Ok(()) }
                        });
                        cx.spawn(async move |this, cx| {
                            let result = save.await;
                            let _ = this.update(cx, |this, cx| {
                                if this.document_generation != generation { return; }
                                match result {
                                    Ok(()) => {
                                        this.doc.settings["request"]["proxy"] =
                                            json!({"mode": "manual", "url": proxy_url, "bypass": bypass});
                                        this.changed(cx);
                                    }
                                    Err(error) => {
                                        this.error = Some(error);
                                        cx.notify();
                                    }
                                }
                            });
                        }).detach();
                    }).is_ok()
                })
        });
    }
}

pub(super) fn text_input(
    label: &'static str,
    initial: &str,
    masked: bool,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> Entity<InputState> {
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(label)
            .masked(masked)
    });
    input.update(cx, |state, cx| {
        state.set_value(initial.to_owned(), window, cx)
    });
    input
}

fn validate_proxy_url(raw: &str) -> Result<(), &'static str> {
    let url = url::Url::parse(raw).map_err(|_| "Enter a valid proxy URL.")?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Enter proxy credentials in the username and password fields.");
    }
    if url.host().is_none() || !matches!(url.scheme(), "http" | "https") {
        return Err("Enter an HTTP or HTTPS proxy URL.");
    }
    Ok(())
}
