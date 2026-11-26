#[path = "backup_ui.rs"]
mod backup_ui;
#[path = "code_ui.rs"]
mod code_ui;
#[path = "cookies_ui.rs"]
mod cookies_ui;
#[path = "layout.rs"]
mod layout;
#[path = "oauth_ui.rs"]
mod oauth_ui;
#[path = "panels.rs"]
mod panels;
#[path = "response_ui.rs"]
mod response_ui;
#[path = "session_ui.rs"]
mod session_ui;
#[path = "settings_ui.rs"]
mod settings_ui;
#[path = "variables_ui.rs"]
mod variables_ui;
#[path = "workspace_ui.rs"]
mod workspace_ui;

use std::collections::{HashMap, HashSet};
use std::{path::PathBuf, sync::Arc, time::Duration};

use gpui::{
    AnyWindowHandle, App, AppContext, ClickEvent, Context, Div, ElementId, Entity, FontWeight,
    InteractiveElement, IntoElement, ParentElement, Render, Rgba, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Subscription, Task, Window, div, px,
};
use gpui_component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
};
use layout::{Axis, Layout};
use serde_json::{Value, json};
use workspace_ui::NameAction;

use crate::{
    appearance::{Palette, icon},
    history, http, prepare,
    workspace::{Document, Workspace, save_isolated},
};

const METHODS: &[&str] = &[
    "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "QUERY",
];

#[derive(Clone, Copy, PartialEq)]
enum Rail {
    Collections,
    Environments,
    History,
}

impl Rail {
    fn label(self) -> &'static str {
        match self {
            Self::Collections => "Collections",
            Self::Environments => "Environments",
            Self::History => "History",
        }
    }

    fn filter(self) -> &'static str {
        match self {
            Self::Collections => "Filter collections…",
            Self::Environments => "Filter environments…",
            Self::History => "Filter history…",
        }
    }
}

type RequestKey = (String, String);

pub(super) struct Workbench {
    doc: Document,
    document_generation: uuid::Uuid,
    palette: Palette,
    url: Entity<InputState>,
    search: Entity<InputState>,
    filter: String,
    body: Option<Entity<InputState>>,
    viewing_example: Option<(String, Result<http::Response, String>)>,
    panel_editor: Option<panels::FieldEditor>,
    active_rail: Rail,
    rail_visible: bool,
    expanded: HashSet<String>,
    req_section: usize,
    res_section: usize,
    side_by_side: bool,
    layout: Layout,
    mask_secrets: bool,
    error: Option<String>,
    storage: Option<PathBuf>,
    session: Arc<http::Session>,
    cookies_drawer: Option<gpui::WeakEntity<cookies_ui::CookieDrawer>>,
    revision: u64,
    saved_revision: u64,
    writing: bool,
    write_again: bool,
    save_timer: Option<Task<()>>,
    closing: Option<AnyWindowHandle>,
    results: HashMap<RequestKey, Result<http::Response, String>>,
    pending: HashMap<RequestKey, uuid::Uuid>,
    _subscriptions: Vec<Subscription>,
}

impl Workbench {
    pub(super) fn new(
        mut doc: Document,
        error: Option<String>,
        storage: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        for workspace in &mut doc.workspaces {
            for tab in &mut workspace.tabs {
                if tab["kind"] != "environment" && tab["id"].as_str().is_none() {
                    tab["id"] = json!(uuid::Uuid::new_v4().to_string());
                }
            }
        }
        let palette = settings_ui::theme(&doc.settings, window, cx);
        let system_theme = cx.observe_window_appearance(window, |this, window, cx| {
            if this.doc.settings["general"]["theme"] == "system" {
                this.palette = settings_ui::theme(&this.doc.settings, window, cx);
                cx.notify();
            }
        });
        let side_by_side = doc.settings["gpuiLayout"]["sideBySide"]
            .as_bool()
            .unwrap_or(false);
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("Enter URL or paste text"));
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder(Rail::Collections.filter()));
        let enter = cx.subscribe_in(&url, window, |this, _, event, window, cx| match event {
            InputEvent::PressEnter { .. } => this.send(window, cx),
            InputEvent::Change if this.sync_url(cx) => this.changed(cx),
            InputEvent::Blur => {
                if this.sync_url(cx) {
                    this.changed(cx);
                }
                if let Some(request) = this.active_request() {
                    let base = val(request, "url").to_owned();
                    if this.url.read(cx).value().as_ref() != base {
                        this.url
                            .update(cx, |input, cx| input.set_value(base, window, cx));
                    }
                }
            }
            _ => {}
        });
        let search_change = cx.subscribe_in(&search, window, |this, input, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.filter = input.read(cx).value().trim().to_lowercase();
                cx.notify();
            }
        });
        let mask_secrets = doc.settings["privacy"]["maskSensitiveHeaders"]
            .as_bool()
            .unwrap_or(true);
        let session = Arc::new(http::Session::open(
            storage.as_deref(),
            doc.settings["request"]["persistCookies"] != false,
        ));
        let shutdown = session.clone();
        App::on_app_quit(cx, move |app| {
            let session = shutdown.clone();
            let flush = app.background_executor().spawn(async move {
                session.update_cookie_setting();
                session.cookies.flush_all();
            });
            async move { flush.await }
        })
        .detach();
        let mut workbench = Self {
            doc,
            document_generation: uuid::Uuid::new_v4(),
            palette,
            url,
            search,
            filter: String::new(),
            body: None,
            viewing_example: None,
            panel_editor: None,
            active_rail: Rail::Collections,
            rail_visible: true,
            expanded: HashSet::new(),
            req_section: 0,
            res_section: 0,
            side_by_side,
            layout: Layout::new(cx),
            mask_secrets,
            error,
            storage,
            session,
            cookies_drawer: None,
            revision: 0,
            saved_revision: 0,
            writing: false,
            write_again: false,
            save_timer: None,
            closing: None,
            results: HashMap::new(),
            pending: HashMap::new(),
            _subscriptions: vec![enter, search_change, system_theme],
        };
        workbench.present_active(window, cx);
        workbench
    }

    fn workspace(&self) -> &Workspace {
        &self.doc.workspaces[self.doc.active_workspace]
    }

    fn workspace_mut(&mut self) -> &mut Workspace {
        &mut self.doc.workspaces[self.doc.active_workspace]
    }

    fn active_tab(&self) -> Option<&Value> {
        let workspace = self.workspace();
        workspace.tabs.get(workspace.active_tab?)
    }

    fn active_request(&self) -> Option<&Value> {
        self.active_tab().filter(|tab| tab["kind"] != "environment")
    }

    fn active_request_mut(&mut self) -> Option<&mut Value> {
        let workspace = self.workspace_mut();
        workspace
            .tabs
            .get_mut(workspace.active_tab?)
            .filter(|tab| tab["kind"] != "environment")
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.revision = self.revision.wrapping_add(1);
        if self.storage.is_some()
            && self.doc.settings["general"]["autoSave"]
                .as_bool()
                .unwrap_or(true)
        {
            let delay = self.doc.settings["general"]["autoSaveIntervalMs"]
                .as_u64()
                .unwrap_or(350)
                .clamp(100, 60_000);
            let timer = cx.background_executor().timer(Duration::from_millis(delay));
            self.save_timer = Some(cx.spawn(async move |view, cx| {
                timer.await;
                let _ = view.update(cx, |this, cx| this.save_profile(cx));
            }));
        }
        cx.notify();
    }

    fn save_profile(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.storage.clone() else {
            return;
        };
        if self.writing {
            self.write_again = true;
            return;
        }
        self.writing = true;
        self.error = None;
        let revision = self.revision;
        let document = self.doc.clone();
        let work = cx.background_executor().spawn(async move {
            if let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("Could not create workspace directory: {error}"))?;
            }
            save_isolated(&path, &document)
        });
        cx.spawn(async move |view, cx| {
            let result = work.await;
            let _ = view.update(cx, |this, cx| {
                this.writing = false;
                let saved = result.is_ok();
                match result {
                    Ok(()) => this.saved_revision = revision,
                    Err(error) => {
                        this.error = Some(if this.closing.take().is_some() {
                            format!("Could not close safely: {error}")
                        } else {
                            error
                        });
                    }
                }
                if std::mem::take(&mut this.write_again)
                    || (this.closing.is_some() && this.saved_revision != this.revision)
                {
                    this.save_profile(cx);
                } else if saved && let Some(window) = this.closing.take() {
                    cx.defer(move |cx| {
                        let _ = window.update(cx, |_, window, _| window.remove_window());
                    });
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn prepare_to_close(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        if self.storage.is_none() {
            return true;
        }
        if self.sync_url(cx) {
            self.revision = self.revision.wrapping_add(1);
        }
        if self.revision == self.saved_revision && !self.writing {
            return true;
        }
        if self.closing.is_none() {
            self.closing = Some(window.window_handle());
            self.save_timer = None;
            if !self.writing {
                self.save_profile(cx);
            }
        }
        false
    }

    fn active_key(&self) -> Option<RequestKey> {
        let id = self.active_request()?["id"].as_str()?;
        Some((self.workspace().id.clone(), id.to_owned()))
    }

    fn active_response(&self) -> Option<&Result<http::Response, String>> {
        if let Some((_, example)) = &self.viewing_example {
            return Some(example);
        }
        self.results.get(&self.active_key()?)
    }

    fn is_sending(&self) -> bool {
        self.active_key()
            .is_some_and(|key| self.pending.contains_key(&key))
    }

    fn active_collection(&self) -> Option<&Value> {
        self.workspace()
            .find_request(self.active_request()?["id"].as_str()?)
            .map(|item| item.collection)
    }

    fn bound_environment(&self, collection: &Value) -> Option<&Value> {
        let id = collection["environmentId"].as_str()?;
        self.workspace()
            .environments
            .iter()
            .find(|env| env["id"] == id)
    }

    fn refresh_body(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self
            .active_response()
            .and_then(|r| r.as_ref().ok())
            .map(|r| {
                if self.res_section == 0 {
                    r.pretty.as_ref().unwrap_or(&r.wire.body)
                } else {
                    &r.wire.body
                }
                .clone()
            });
        if let Some(value) = value {
            let body = self.body.get_or_insert_with(|| {
                cx.new(|cx| InputState::new(window, cx).multi_line(true).soft_wrap(true))
            });
            body.update(cx, |input, cx| input.set_value(value, window, cx));
        } else {
            self.body = None;
        }
    }

    fn present_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.panel_editor = None;
        self.viewing_example = None;
        let url = self
            .active_request()
            .and_then(|req| req["url"].as_str())
            .unwrap_or_default()
            .to_owned();
        self.url
            .update(cx, |input, cx| input.set_value(url, window, cx));
        self.req_section = 0;
        self.res_section = match self.doc.settings["response"]["defaultView"].as_str() {
            Some("raw") => 1,
            Some("preview") => 2,
            _ => 0,
        };
        self.refresh_body(window, cx);
        cx.notify();
    }

    fn sync_url(&mut self, cx: &App) -> bool {
        let url = self.url.read(cx).value();
        let Some(tab) = self.active_request_mut() else {
            return false;
        };
        let mut parts = url.split('?');
        let base = parts.next().unwrap_or_default();
        let parsed: Vec<_> = parts.next().unwrap_or_default().split('&').filter(|piece| !piece.is_empty())
            .map(|pair| {
                let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                let key = crate::postman::decode_uri_component(key);
                let description = tab["params"].as_array().into_iter().flatten().rev()
                    .find(|row| row["key"] == key).and_then(|row| row["description"].as_str()).unwrap_or_default();
                json!({"key":key,"value":crate::postman::decode_uri_component(value),"enabled":true,"description":description})
            }).collect();
        let path = crate::postman::path_variables(base, &tab["pathVars"]);
        let mut changed = tab["url"] != base;
        if changed {
            tab["url"] = json!(base);
        }
        if !path.is_empty() || tab.get("pathVars").is_some() {
            let path = json!(path);
            if tab["pathVars"] != path {
                tab["pathVars"] = path;
                changed = true;
            }
        }
        if !parsed.is_empty() {
            let params = json!(parsed);
            if tab["params"] != params {
                tab["params"] = params;
                changed = true;
            }
        }
        changed
    }

    fn activate_workspace(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index != self.doc.active_workspace && index < self.doc.workspaces.len() {
            self.sync_url(cx);
            self.dismiss_cookies(window, cx);
            self.doc.active_workspace = index;
            self.expanded.clear();
            self.filter.clear();
            self.search
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.present_active(window, cx);
            self.changed(cx);
        }
    }

    fn select_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace().active_tab != Some(index) {
            self.sync_url(cx);
            self.workspace_mut().active_tab = Some(index);
            self.present_active(window, cx);
            self.changed(cx);
        }
    }

    fn open_tab(&mut self, value: Value, window: &mut Window, cx: &mut Context<Self>) {
        let kind = value["kind"].as_str().unwrap_or("request");
        let key = if kind == "environment" { "envId" } else { "id" };
        if let Some(index) = self.workspace().tabs.iter().position(|tab| {
            tab["kind"].as_str().unwrap_or("request") == kind && tab[key] == value[key]
        }) {
            self.select_tab(index, window, cx);
            return;
        }
        self.sync_url(cx);
        let workspace = self.workspace_mut();
        workspace.active_tab = Some(workspace.tabs.len());
        workspace.tabs.push(value);
        self.present_active(window, cx);
        self.changed(cx);
    }

    fn open_request(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(request) = self
            .workspace()
            .find_request(id)
            .map(|item| item.request.clone())
        {
            self.open_tab(request, window, cx);
        }
    }

    fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_url(cx);
        let old_active = self.workspace().active_tab;
        let mut result_key = None;
        let current = self.workspace().id.clone();
        let workspace = self.workspace_mut();
        let tab = workspace.tabs.remove(index);
        if tab["kind"] != "environment" {
            result_key = tab["id"].as_str().map(|id| (current, id.to_owned()));
        }
        workspace.active_tab = match old_active {
            _ if workspace.tabs.is_empty() => None,
            Some(active) if index < active => Some(active - 1),
            Some(active) if index == active => Some(active.min(workspace.tabs.len() - 1)),
            active => active,
        };
        if let Some(key) = result_key {
            self.results.remove(&key);
            self.pending.remove(&key);
        }
        self.present_active(window, cx);
        self.changed(cx);
    }

    fn new_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let method = self.doc.settings["request"]["defaultMethod"]
            .as_str()
            .unwrap_or("GET");
        let id = uuid::Uuid::new_v4().to_string();
        self.open_tab(json!({
            "id": id, "kind": "request", "name": "Untitled Request", "method": method, "url": "",
        }), window, cx);
        self.url.update(cx, |input, cx| input.focus(window, cx));
    }

    fn set_method(&mut self, method: &str, cx: &mut Context<Self>) {
        let workspace = self.workspace_mut();
        if let Some(tab) = workspace
            .active_tab
            .and_then(|index| workspace.tabs.get_mut(index))
        {
            tab["method"] = json!(method);
            self.changed(cx);
        }
    }

    fn switch_rail(&mut self, rail: Rail, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_rail == rail {
            self.rail_visible = !self.rail_visible;
        } else {
            self.active_rail = rail;
            self.rail_visible = true;
            self.search.update(cx, |input, cx| {
                input.set_placeholder(rail.filter(), window, cx);
                input.set_value("", window, cx);
            });
            self.filter.clear();
        }
        cx.notify();
    }

    fn bind_environment(
        &mut self,
        collection_id: &str,
        environment: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if let Some(collection) = self
            .workspace_mut()
            .collections
            .iter_mut()
            .find(|item| item["id"] == collection_id)
        {
            collection["environmentId"] = json!(environment);
            self.changed(cx);
        }
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_url(cx);
        self.viewing_example = None;
        let Some(key) = self.active_key() else {
            return;
        };
        if self.pending.contains_key(&key) {
            return;
        }
        let workspace = self.workspace();
        let Some(request) = self.active_request().cloned() else {
            return;
        };
        let located = workspace.find_request(&key.1);
        let collection = located.as_ref().map(|item| item.collection);
        let ancestors = located
            .as_ref()
            .map(|item| item.ancestors.as_slice())
            .unwrap_or_default();
        let environment = collection.and_then(|item| self.bound_environment(item));
        let prepared = prepare::prepare(
            &request,
            collection,
            ancestors,
            environment,
            &self.doc.settings,
            &workspace.id,
        )
        .and_then(|payload| {
            prepare::oauth_config(&request, collection, ancestors, &workspace.id)
                .map(|oauth| (payload, oauth))
        });
        let (payload, oauth) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                let outcome = Err(error);
                if history::record(self.workspace_mut(), &request, &outcome) {
                    self.changed(cx);
                }
                self.results.insert(key, outcome);
                self.refresh_body(window, cx);
                cx.notify();
                return;
            }
        };

        // Each dispatch carries a generation. A closed and reopened tab can share its saved
        // request ID with the old tab while the old request is still finishing.
        let document_generation = self.document_generation;
        let generation = uuid::Uuid::new_v4();
        self.pending.insert(key.clone(), generation);
        if oauth.is_some()
            && let Some(owner) = self.active_oauth_owner()
        {
            self.watch_oauth(owner, cx);
        }
        cx.notify();
        let session = self.session.clone();
        let job = http::runtime().map(|runtime| runtime.spawn(session.dispatch(payload, oauth)));
        cx.spawn_in(window, async move |this, cx| {
            let result = match job {
                Ok(task) => task.await.unwrap_or_else(|error| Err(error.to_string())),
                Err(error) => Err(error),
            };
            let _ = this.update_in(cx, |this, window, cx| {
                if this.document_generation != document_generation {
                    return;
                }
                if this.workspace().id == key.0 {
                    this.refresh_cookies(cx);
                }

                if let Some(workspace) = this
                    .doc
                    .workspaces
                    .iter_mut()
                    .find(|workspace| workspace.id == key.0)
                    && history::record(workspace, &request, &result)
                {
                    this.changed(cx);
                }
                if this.pending.get(&key) == Some(&generation) {
                    this.pending.remove(&key);
                    let is_active = this.active_key().as_ref() == Some(&key);
                    this.results.insert(key, result);
                    if is_active {
                        this.refresh_body(window, cx);
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn top_bar(&self, cx: &mut Context<Self>) -> Div {
        let p = &self.palette;
        let workspace_label = self.workspace().name.clone();
        let current_workspace = self.workspace().id.clone();
        let active = self.doc.active_workspace;
        let workspaces: Vec<_> = self
            .doc
            .workspaces
            .iter()
            .map(|workspace| workspace.name.clone())
            .collect();
        let view = cx.entity().downgrade();
        let workspace_menu = Button::new("workspace-switch")
            .outline()
            .small()
            .h(px(32.))
            .bg(p.side)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(icon("grid", p.accent, 14.))
                    .child(workspace_label.clone())
                    .child(icon("chevron-down", p.faint, 12.)),
            )
            .dropdown_menu(move |mut menu, _, _| {
                for (index, name) in workspaces.iter().enumerate() {
                    let view = view.clone();
                    menu = menu.item(
                        PopupMenuItem::new(name.clone())
                            .checked(index == active)
                            .on_click(move |_, window, cx| {
                                let _ = view.update(cx, |this, cx| {
                                    this.activate_workspace(index, window, cx)
                                });
                            }),
                    );
                }
                let rename = view.clone();
                let delete = view.clone();
                let create = view.clone();
                let id = current_workspace.clone();
                let label = workspace_label.clone();
                menu.item(
                    PopupMenuItem::new("Rename workspace").on_click(move |_, window, cx| {
                        let _ = rename.update(cx, |this, cx| {
                            this.ask_name(
                                "Rename workspace",
                                NameAction::WorkspaceRename(id.clone()),
                                &label,
                                window,
                                cx,
                            )
                        });
                    }),
                )
                .item({
                    let id = current_workspace.clone();
                    PopupMenuItem::new("Delete workspace").on_click(move |_, window, cx| {
                        let _ = delete.update(cx, |this, cx| {
                            this.confirm_workspace_delete(id.clone(), window, cx)
                        });
                    })
                })
                .item(
                    PopupMenuItem::new("New workspace").on_click(move |_, window, cx| {
                        let _ = create.update(cx, |this, cx| {
                            this.ask_name("New workspace", NameAction::Workspace, "", window, cx)
                        });
                    }),
                )
            });
        let mut context = div()
            .flex()
            .items_center()
            .gap(px(7.))
            .pl(px(14.))
            .border_l_1()
            .border_color(p.line)
            .child(workspace_menu);
        if let Some(collection) = self.active_collection() {
            let env = self.bound_environment(collection);
            let env_id = env.and_then(|item| item["id"].as_str()).map(str::to_owned);
            let environment_label = env
                .and_then(|item| item["name"].as_str())
                .unwrap_or("No Environment")
                .to_owned();
            let collection_id = val(collection, "id").to_owned();
            let environments: Vec<_> = std::iter::once((None, "No Environment".to_owned()))
                .chain(self.workspace().environments.iter().filter_map(|env| {
                    Some((
                        Some(env["id"].as_str()?.to_owned()),
                        name(env, "Environment").to_owned(),
                    ))
                }))
                .collect();
            let view = cx.entity().downgrade();
            context = context.child(
                Button::new("environment-switch")
                    .outline()
                    .small()
                    .h(px(32.))
                    .bg(p.side)
                    .child(icon("globe", p.accent, 14.))
                    .label(environment_label)
                    .child(icon("chevron-down", p.faint, 12.))
                    .dropdown_menu(move |mut menu, _, _| {
                        for (id, name) in &environments {
                            let view = view.clone();
                            let collection_id = collection_id.clone();
                            let selected = *id == env_id;
                            let id = id.clone();
                            menu = menu.item(
                                PopupMenuItem::new(name.clone()).checked(selected).on_click(
                                    move |_, _, cx| {
                                        let _ = view.update(cx, |this, cx| {
                                            this.bind_environment(&collection_id, id.as_deref(), cx)
                                        });
                                    },
                                ),
                            );
                        }
                        menu
                    }),
            );
        }
        div()
            .h(px(p.appearance.top()))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(16.))
            .px(px(14.))
            .bg(p.side)
            .border_b_1()
            .border_color(p.line)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(22.))
                            .rounded(px(5.))
                            .bg(p.accent)
                            .child(icon("brand", p.accent_text, 24.)),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .font_family(p.appearance.heading_font())
                            .text_size(px(p.appearance.title()))
                            .child("Postlite"),
                    ),
            )
            .child(context)
            .child(div().flex_1())
            .child(
                Button::new("import")
                    .outline()
                    .small()
                    .h(px(32.))
                    .bg(p.raised)
                    .child(icon("import", p.dim, 14.))
                    .label("Import")
                    .on_click(cx.listener(|this, _, _, cx| this.import_postman(cx))),
            )
            .child(
                icon_button("settings", "gear", p.faint, p.hover, 28.)
                    .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))),
            )
    }

    fn rail(&self, cx: &mut Context<Self>) -> Div {
        let p = &self.palette;
        let mut rail = div()
            .w(px(48.))
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(10.))
            .gap(px(6.))
            .bg(p.bg)
            .border_r_1()
            .border_color(p.line);
        for (kind, glyph) in [
            (Rail::Collections, "stack"),
            (Rail::Environments, "globe"),
            (Rail::History, "clock"),
        ] {
            let selected = kind == self.active_rail && self.rail_visible;
            let mut button = icon_button(
                kind.label(),
                glyph,
                if selected { p.accent } else { p.faint },
                p.hover,
                36.,
            );
            if selected {
                button = button.bg(p.raised);
            }
            rail = rail.child(button.on_click(
                cx.listener(move |this, _, window, cx| this.switch_rail(kind, window, cx)),
            ));
        }
        rail
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> Div {
        let p = &self.palette;
        let mut title = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .h(px(p.appearance.sidebar_heading()))
            .px(px(14.))
            .border_b_1()
            .border_color(p.line)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.active_rail.label()),
            )
            .child(div().flex_1());
        if self.active_rail == Rail::Collections {
            title = title
                .child(
                    icon_button("expand-all", "unfold", p.faint, p.hover, 26.).on_click(
                        cx.listener(|this, _, _, cx| {
                            let mut expanded = HashSet::new();
                            collect_expandable(&this.workspace().collections, &mut expanded);
                            this.expanded = expanded;
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    icon_button("collapse-all", "fold", p.faint, p.hover, 26.).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.expanded.clear();
                            cx.notify();
                        }),
                    ),
                );
        }
        if self.active_rail != Rail::History {
            let rail = self.active_rail;
            title = title.child(
                icon_button("sidebar-add", "plus", p.faint, p.hover, 26.).on_click(cx.listener(
                    move |this, _, window, cx| {
                        let (title, action) = if rail == Rail::Collections {
                            ("New collection", NameAction::Collection)
                        } else {
                            ("New environment", NameAction::Environment)
                        };
                        this.ask_name(title, action, "", window, cx);
                    },
                )),
            );
        } else {
            title = title.child(
                icon_button("clear-history", "trash", p.faint, p.hover, 26.)
                    .on_click(cx.listener(|this, _, window, cx| this.clear_history(window, cx))),
            );
        }
        let mut list = div()
            .id("sidebar-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(6.))
            .pt(px(7.));
        let workspace = self.workspace();
        match self.active_rail {
            Rail::Collections => {
                for collection in &workspace.collections {
                    list = list.children(self.tree(collection, 0, cx));
                }
            }
            Rail::Environments => {
                for env in &workspace.environments {
                    let name = name(env, "Environment");
                    if !name.to_lowercase().contains(&self.filter) {
                        continue;
                    }
                    let id = val(env, "id").to_owned();
                    let menu = Self::resource_menu(id.clone(), name.to_owned(), false, true, cx);
                    list = list.child(
                        tree_row(SharedString::from(format!("env-{id}")), p, 0, false)
                            .child(icon("globe", p.accent, 15.))
                            .child(div().truncate().child(name.to_owned()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_tab(
                                    json!({ "kind": "environment", "envId": id }),
                                    window,
                                    cx,
                                );
                            }))
                            .context_menu(menu),
                    );
                }
            }
            Rail::History => {
                for (index, entry) in workspace.history.iter().enumerate() {
                    let method = val(entry, "method");
                    let title = entry["name"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .unwrap_or(val(entry, "url"));
                    if !title.to_lowercase().contains(&self.filter) {
                        continue;
                    }
                    list = list.child(
                        tree_row(("history", index), p, 0, false)
                            .child(method_chip(method, p, 38.))
                            .child(div().truncate().child(title.to_owned()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let entry = &this.workspace().history[index];
                                if let Some(id) = entry["ref"].as_str().map(str::to_owned) {
                                    this.open_request(&id, window, cx);
                                } else if entry["snapshot"].is_object() {
                                    let mut request = entry["snapshot"].clone();
                                    request["id"] = json!(uuid::Uuid::new_v4().to_string());
                                    this.open_tab(request, window, cx);
                                }
                            })),
                    );
                }
            }
        }
        div()
            .w(px(self.sidebar_width()))
            .flex_none()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(p.side)
            .border_r_1()
            .border_color(p.line)
            .child(title)
            .child(
                div().py(px(4.)).px(px(10.)).child(
                    Input::new(&self.search)
                        .small()
                        .h(px(26.))
                        .bg(p.input)
                        .prefix(icon("search", p.faint, 13.)),
                ),
            )
            .child(list)
    }

    fn sidebar_width(&self) -> f32 {
        self.doc.settings["sidebarWidth"]
            .as_f64()
            .unwrap_or(276.)
            .clamp(180., 480.) as f32
    }

    fn tree(&self, node: &Value, depth: usize, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let p = &self.palette;
        let child_nodes = items(node);
        let has_filter = !self.filter.is_empty();
        if has_filter && !matches_tree(node, &self.filter) {
            return Vec::new();
        }
        let id = val(node, "id").to_owned();
        let key = SharedString::from(format!("tree-{id}"));
        let menu = Self::resource_menu(
            id.clone(),
            name(node, "Item").to_owned(),
            child_nodes.is_some(),
            depth == 0,
            cx,
        );
        if let Some(children) = child_nodes {
            let expanded = has_filter || self.expanded.contains(&id);
            let glyph = if depth == 0 { "stack" } else { "folder" };
            let mut rows = vec![
                tree_row(key, p, depth, false)
                    .child(icon(
                        if expanded { "chevron-down" } else { "caret" },
                        p.faint,
                        12.,
                    ))
                    .child(icon(
                        glyph,
                        if depth == 0 { p.accent } else { p.faint },
                        14.,
                    ))
                    .child(div().truncate().child(name(node, "Folder").to_owned()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.expanded.remove(&id) {
                            this.expanded.insert(id.clone());
                        }
                        cx.notify();
                    }))
                    .context_menu(menu)
                    .into_any_element(),
            ];
            if expanded {
                rows.extend(
                    children
                        .iter()
                        .flat_map(|child| self.tree(child, depth + 1, cx)),
                );
            }
            rows
        } else {
            let selected = self.active_request().is_some_and(|req| req["id"] == id);
            vec![
                tree_row(key, p, depth, selected)
                    .child(method_chip(method(node), p, 34.))
                    .child(
                        div()
                            .truncate()
                            .child(name(node, "Untitled Request").to_owned()),
                    )
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.open_request(&id, window, cx)),
                    )
                    .context_menu(menu)
                    .into_any_element(),
            ]
        }
    }

    fn tab_bar(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = &self.palette;
        let mut tabs = div()
            .id("request-tabs")
            .h(px(p.appearance.tab()))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(3.))
            .overflow_x_scroll()
            .px(px(7.))
            .bg(p.side)
            .border_b_1()
            .border_color(p.line);
        let workspace = self.workspace();
        for (index, tab) in workspace.tabs.iter().enumerate() {
            let selected = workspace.active_tab == Some(index);
            let env = tab["kind"] == "environment";
            let title = if env {
                workspace
                    .environments
                    .iter()
                    .find(|env| env["id"] == tab["envId"])
                    .map(|env| name(env, "Environment"))
                    .unwrap_or("Environment")
            } else {
                name(tab, "Untitled Request")
            };
            let mut item = div()
                .id(("request-tab", index))
                .min_w(px(118.))
                .max_w(px(260.))
                .h(px(33.))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(9.))
                .px(px(8.))
                .rounded_t(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(p.hover));
            if selected {
                item = item.bg(p.panel).border_b_2().border_color(p.accent);
            }
            item = if env {
                item.child(icon("globe", p.cyan, 14.))
            } else {
                item.child(method_chip(method(tab), p, 48.))
            };
            tabs = tabs.child(
                item.child(div().flex_1().truncate().child(title.to_owned()))
                    .child(
                        icon_button(("close-tab", index), "x", p.faint, p.hover, 20.).on_click(
                            cx.listener(move |this, _: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                this.close_tab(index, window, cx);
                            }),
                        ),
                    )
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.select_tab(index, window, cx)),
                    ),
            );
        }
        tabs.child(
            icon_button("new-request-tab", "plus", p.faint, p.hover, 26.)
                .on_click(cx.listener(|this, _, window, cx| this.new_request(window, cx))),
        )
    }

    fn address_height(&self) -> f32 {
        use crate::appearance::Appearance;
        self.palette.appearance.command()
            + if self.palette.appearance == Appearance::Terminal {
                17.
            } else {
                25.
            }
    }

    fn address(&self, can_split: bool, cx: &mut Context<Self>) -> Div {
        let p = &self.palette;
        let request_method = self.active_request().map(method).unwrap_or("GET");
        let view = cx.entity().downgrade();
        let command = p.appearance.command();
        let radius = p.appearance.radius();
        let methods = Button::new("method")
            .ghost()
            .w(px(104.))
            .h(px(command - 2.))
            .text_size(px(12.))
            .label(request_method.to_owned())
            .text_color(p.method(request_method))
            .child(icon("chevron-down", p.faint, 12.))
            .dropdown_menu(move |mut menu, _, _| {
                for name in METHODS {
                    let view = view.clone();
                    menu = menu.item(PopupMenuItem::new(*name).on_click(move |_, _, cx| {
                        let _ = view.update(cx, |this, cx| this.set_method(name, cx));
                    }));
                }
                menu
            });
        div()
            .h(px(self.address_height()))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(7.))
            .px(px(14.))
            .bg(p.raised)
            .border_b_1()
            .border_color(p.line)
            .child(
                div()
                    .h(px(command))
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .rounded(px(radius))
                    .border_1()
                    .border_color(p.line)
                    .bg(p.input)
                    .child(
                        div()
                            .h_full()
                            .flex_none()
                            .border_r_1()
                            .border_color(p.line)
                            .child(methods),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px(px(4.))
                            .font_family("monospace")
                            .child(Input::new(&self.url).appearance(false)),
                    )
                    .children(self.variable_peek(cx)),
            )
            .child(
                Button::new("send")
                    .primary()
                    .label(if self.is_sending() {
                        "Sending…"
                    } else {
                        "Send"
                    })
                    .w(px(94.))
                    .h(px(command))
                    .rounded(px(radius))
                    .text_size(px(13.))
                    .font_weight(FontWeight::BOLD)
                    .disabled(self.is_sending())
                    .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
            )
            .child(self.request_save_button(cx))
            .child(
                Button::new("side-by-side")
                    .ghost()
                    .label("◫  Side by side")
                    .w(px(117.))
                    .disabled(!can_split)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.side_by_side = !this.side_by_side;
                        if !this.doc.settings["gpuiLayout"].is_object() {
                            this.doc.settings["gpuiLayout"] = json!({});
                        }
                        this.doc.settings["gpuiLayout"]["sideBySide"] = json!(this.side_by_side);
                        this.changed(cx);
                    })),
            )
    }

    fn request_save_button(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let button = Button::new("save")
            .outline()
            .label("Save")
            .w(px(66.))
            .h(px(40.));
        if let Some(collection) = self.active_collection() {
            let id = val(collection, "id").to_owned();
            return button
                .on_click(cx.listener(move |this, _, _, cx| this.save_request_to(&id, cx)))
                .into_any_element();
        }
        let mut destinations = Vec::new();
        save_destinations(&self.workspace().collections, "", &mut destinations);
        let view = cx.entity().downgrade();
        button
            .dropdown_menu(move |mut menu, _, _| {
                for (id, label) in &destinations {
                    let view = view.clone();
                    let id = id.clone();
                    menu =
                        menu.item(PopupMenuItem::new(label.clone()).on_click(move |_, _, cx| {
                            let _ = view.update(cx, |this, cx| this.save_request_to(&id, cx));
                        }));
                }
                let view = view.clone();
                menu.item(
                    PopupMenuItem::new("New collection…").on_click(move |_, window, cx| {
                        let _ = view.update(cx, |this, cx| {
                            this.ask_name("New collection", NameAction::Collection, "", window, cx)
                        });
                    }),
                )
            })
            .into_any_element()
    }

    fn environment_panel(&self, tab: &Value, cx: &mut Context<Self>) -> Div {
        let p = &self.palette;
        let Some(env) = self
            .workspace()
            .environments
            .iter()
            .find(|env| env["id"] == tab["envId"])
        else {
            return div().p_4().child("Environment unavailable");
        };
        let mask_variables = self.doc.settings["privacy"]["maskSecretVariables"]
            .as_bool()
            .unwrap_or(true);
        let mut panel = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(14.))
            .p(px(14.))
            .bg(p.panel)
            .child(
                div()
                    .text_size(px(16.))
                    .font_weight(FontWeight::BOLD)
                    .child(name(env, "Environment").to_owned()),
            )
            .child(
                div()
                    .flex()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.faint)
                    .child(div().flex_1().child("Key"))
                    .child(div().flex_1().child("Value")),
            );
        if let Some(variables) = env["variables"].as_array() {
            for (index, row) in variables.iter().enumerate() {
                let env_id = val(env, "id").to_owned();
                let value = if mask_variables && row["secret"] == true {
                    "••••••••"
                } else {
                    val(row, "value")
                };
                let mut cells = div()
                    .flex()
                    .py(px(4.))
                    .border_b_1()
                    .border_color(p.soft_line)
                    .font_family("monospace");
                for (field, label, displayed) in [
                    ("key", "Variable key", val(row, "key")),
                    ("value", "Variable value", value),
                ] {
                    let environment = env_id.clone();
                    let actual = val(row, field).to_owned();
                    cells = cells.child(
                        div()
                            .id((field, index))
                            .flex_1()
                            .cursor_text()
                            .child(displayed.to_owned())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.ask_name(
                                    label,
                                    NameAction::Variable {
                                        environment: environment.clone(),
                                        index,
                                        field,
                                    },
                                    &actual,
                                    window,
                                    cx,
                                );
                            })),
                    );
                }
                for (field, enabled, label) in [
                    ("enabled", row["enabled"] != false, "Enabled"),
                    ("secret", row["secret"] == true, "Secret"),
                ] {
                    let id = env_id.clone();
                    cells = cells.child(
                        div()
                            .id((field, index))
                            .w(px(95.))
                            .cursor_pointer()
                            .text_color(if enabled { p.accent } else { p.faint })
                            .child(format!("{} {label}", if enabled { "✓" } else { "○" }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(row) = this.environment_row(&id, index) {
                                    row[field] = json!(!enabled);
                                    this.changed(cx);
                                }
                            })),
                    );
                }
                cells = cells.child(
                    icon_button(("delete-env-variable", index), "x", p.faint, p.hover, 24.)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.remove_variable(&env_id, index, cx)
                        })),
                );
                panel = panel.child(cells);
            }
        }
        let id = val(env, "id").to_owned();
        panel.child(
            Button::new("add-variable")
                .ghost()
                .label("+ Add variable")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.add_variable(id.clone(), window, cx)
                })),
        )
    }

    fn empty_workspace(&self, cx: &mut Context<Self>) -> Div {
        let p = &self.palette;
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(10.))
            .bg(p.bg)
            .child(icon("file-plus", p.accent, 30.))
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(19.))
                    .child("No request open"),
            )
            .child(
                div()
                    .text_color(p.dim)
                    .child("Open a request from Collections or start a new one."),
            )
            .child(
                Button::new("empty-new-request")
                    .primary()
                    .label("New request")
                    .on_click(cx.listener(|this, _, window, cx| this.new_request(window, cx))),
            )
    }

    fn status(&self, cx: &mut Context<Self>) -> Div {
        let p = &self.palette;
        let workspace = self.workspace();
        let sizes = count(self.doc.workspaces.len(), "workspace")
            + "   "
            + &count(workspace.collections.len(), "collection")
            + "   "
            + &count(workspace.tabs.len(), "open tab");
        div()
            .h(px(p.appearance.status()))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .bg(p.bg)
            .border_t_1()
            .border_color(p.line)
            .text_size(px(11.))
            .text_color(p.faint)
            .child(
                div()
                    .size(px(6.))
                    .rounded_full()
                    .bg(if self.error.is_some() { p.error } else { p.ok }),
            )
            .child(
                div()
                    .truncate()
                    .child(self.error.clone().unwrap_or_else(|| {
                        if self.writing {
                            "Saving…"
                        } else if self.storage.is_none() {
                            "Workspace opened read-only"
                        } else if self.revision != self.saved_revision {
                            "Unsaved"
                        } else {
                            "Workspace loaded"
                        }
                        .to_owned()
                    })),
            )
            .child(
                Button::new("save-profile")
                    .outline()
                    .small()
                    .h(px(20.))
                    .label("Save now")
                    .disabled(self.storage.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.save_profile(cx))),
            )
            .child(div().text_color(p.dim).child(sizes))
            .child(div().flex_1())
            .child(
                div()
                    .font_family("monospace")
                    .child("Postlite 0.2.8 · local-first"),
            )
    }
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &self.palette;
        let mut main = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.tab_bar(cx));
        match self.active_tab() {
            Some(tab) if tab["kind"] == "environment" => {
                main = main.child(self.environment_panel(tab, cx))
            }
            Some(_) => {
                let sidebar = if self.rail_visible {
                    self.sidebar_width()
                } else {
                    0.
                };
                let can_split = window.viewport_size().width >= px(48. + sidebar + 840.);
                let split = can_split && self.side_by_side;
                let request = self.request_panel(cx);
                let response = self.response_panel(cx);
                let axis = if split { Axis::Width } else { Axis::Height };
                let size = px(self.layout_size(axis, window));
                let divider = self.divider(axis, window, cx);
                let panes = div().min_h_0().min_w_0().flex_1().flex();
                let (panes, response) = if split {
                    (panes, response.flex_none().w(size).h_full())
                } else {
                    (panes.flex_col(), response.flex_none().h(size).w_full())
                };
                main = main.child(self.address(can_split, cx)).child(
                    panes
                        .child(request.flex_1().min_h_0().min_w_0())
                        .child(divider)
                        .child(response),
                );
            }
            None => main = main.child(self.empty_workspace(cx)),
        }
        let mut shell = div().flex_1().min_h_0().flex().child(self.rail(cx));
        if self.rail_visible {
            shell = shell
                .child(self.sidebar(cx))
                .child(self.divider(Axis::Sidebar, window, cx));
        }
        div()
            .id("postlite")
            .size_full()
            .flex()
            .flex_col()
            .bg(p.bg)
            .text_color(p.text)
            .text_size(px(13.))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.modifiers.control {
                    match event.keystroke.key.as_str() {
                        "enter" => this.send(window, cx),
                        "," => this.open_settings(window, cx),
                        _ => {}
                    }
                }
            }))
            .on_mouse_move(
                cx.listener(|this, event, window, cx| this.move_divider(event, window, cx)),
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| this.finish_divider(cx)),
            )
            .child(self.top_bar(cx))
            .child(shell.child(main))
            .child(self.status(cx))
    }
}

fn val<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

fn name<'a>(value: &'a Value, fallback: &'static str) -> &'a str {
    value["name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .unwrap_or(fallback)
}

fn method(value: &Value) -> &str {
    value["method"].as_str().unwrap_or("GET")
}

fn items(value: &Value) -> Option<&[Value]> {
    value["items"].as_array().map(Vec::as_slice)
}

fn matches_tree(value: &Value, query: &str) -> bool {
    val(value, "name").to_lowercase().contains(query)
        || val(value, "url").to_lowercase().contains(query)
        || items(value)
            .is_some_and(|children| children.iter().any(|child| matches_tree(child, query)))
}

fn collect_expandable(values: &[Value], expanded: &mut HashSet<String>) {
    for value in values {
        if let Some(children) = items(value) {
            expanded.insert(val(value, "id").to_owned());
            collect_expandable(children, expanded);
        }
    }
}

fn save_destinations(items_to_visit: &[Value], prefix: &str, result: &mut Vec<(String, String)>) {
    for node in items_to_visit {
        if let Some(children) = items(node) {
            let label = format!("{prefix}{}", name(node, "Folder"));
            result.push((val(node, "id").to_owned(), label.clone()));
            save_destinations(children, &format!("{label} / "), result);
        }
    }
}

fn count(amount: usize, singular: &str) -> String {
    format!("{amount} {singular}{}", if amount == 1 { "" } else { "s" })
}

fn method_chip(method: &str, p: &Palette, width: f32) -> Div {
    div()
        .w(px(width))
        .flex_none()
        .font_family("monospace")
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.method(method))
        .child(method.to_owned())
}

fn tree_row(id: impl Into<ElementId>, p: &Palette, depth: usize, selected: bool) -> Stateful<Div> {
    let mut row = div()
        .id(id)
        .h(px(p.appearance.row()))
        .flex()
        .items_center()
        .gap(px(6.))
        .pl(px(12. + depth as f32 * 16.))
        .pr(px(6.))
        .rounded(px(5.))
        .cursor_pointer()
        .hover(|s| s.bg(p.hover));
    if selected {
        row = row.bg(p.raised);
    }
    row
}

fn icon_button(
    id: impl Into<ElementId>,
    glyph: &'static str,
    color: Rgba,
    hover: Rgba,
    side: f32,
) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(side))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .hover(move |s| s.bg(hover))
        .cursor_pointer()
        .child(icon(glyph, color, if side >= 36. { 17. } else { 15. }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_filter_keeps_ancestor_paths_for_nested_matches() {
        let collection = json!({ "name": "Sensors", "id": "c", "items": [
            {"name": "Weather", "id": "f", "items": [
                {"name": "Latest", "id": "r", "url": "https://example.test/weather"}
            ]}
        ] });
        assert!(matches_tree(&collection, "example.test"));
        assert!(!matches_tree(&collection, "other"));
        let mut expanded = HashSet::new();
        collect_expandable(&[collection], &mut expanded);
        assert_eq!(expanded, HashSet::from(["c".to_owned(), "f".to_owned()]));
    }
}
