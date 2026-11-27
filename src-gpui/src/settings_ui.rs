use super::{Workbench, layout::Axis, shortcuts::SHORTCUTS};
use crate::appearance::{Appearance, Palette, primary_fill};
use gpui::{
    App, AppContext, Context, Div, Entity, FocusHandle, Focusable, FontWeight, InteractiveElement,
    IntoElement, ParentElement, PathPromptOptions, Render, Rgba, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window, WindowAppearance, div,
    prelude::FluentBuilder, px,
};
use gpui_component::{
    Disableable, Sizable, Theme, ThemeMode, WindowExt,
    button::{Button, ButtonVariants},
    dialog::DialogButtonProps,
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    switch::Switch,
};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    General,
    Appearance,
    Request,
    Response,
    Privacy,
    Shortcuts,
    Transfer,
    About,
}

const PAGES: [(Page, &str); 8] = [
    (Page::General, "General"),
    (Page::Appearance, "Appearance"),
    (Page::Request, "Request"),
    (Page::Response, "Response"),
    (Page::Privacy, "Privacy"),
    (Page::Shortcuts, "Shortcuts"),
    (Page::Transfer, "Import / Export"),
    (Page::About, "About"),
];

type Path = &'static [&'static str];

#[derive(Clone, Copy)]
enum Action {
    ClearCookies,
    ImportPostman,
    ExportCollection,
    ExportEnvironment,
    ImportSettings,
    ExportSettings,
    ExportBackup,
    RestoreBackup,
    ResetSettings,
}

enum Control {
    Toggle(Path, bool),
    Choice(Path, &'static [(&'static str, &'static str)]),
    Number(Path, i64, i64, i64),
    Styles,
    Accents,
    Proxy,
    Actions(&'static [(Action, &'static str)]),
    Shortcut(&'static [&'static str]),
    Fact(&'static str),
}

/// One searchable unit of the settings window: a label, help text, and the
/// control that edits it. A blank label renders the control on its own.
struct Pref {
    page: Page,
    group: &'static str,
    label: &'static str,
    help: &'static str,
    control: Control,
    warning: bool,
}

const fn pref(page: Page, label: &'static str, help: &'static str, control: Control) -> Pref {
    Pref {
        page,
        group: "",
        label,
        help,
        control,
        warning: false,
    }
}

const fn grouped(
    page: Page,
    group: &'static str,
    label: &'static str,
    help: &'static str,
    control: Control,
) -> Pref {
    Pref {
        page,
        group,
        label,
        help,
        control,
        warning: false,
    }
}

const METHODS: &[(&str, &str)] = &[
    ("GET", "GET"),
    ("POST", "POST"),
    ("PUT", "PUT"),
    ("PATCH", "PATCH"),
    ("DELETE", "DELETE"),
    ("HEAD", "HEAD"),
    ("OPTIONS", "OPTIONS"),
    ("QUERY", "QUERY"),
];

const STYLES: [(Appearance, &str, &str, &str); 4] = [
    (
        Appearance::Workbench,
        "workbench",
        "Workbench",
        "Warm surfaces and balanced spacing.",
    ),
    (
        Appearance::Terminal,
        "terminal",
        "Terminal",
        "Compact rows, square edges, and monospace type.",
    ),
    (
        Appearance::Studio,
        "studio",
        "Studio",
        "Soft panels, larger controls, and more breathing room.",
    ),
    (
        Appearance::Ledger,
        "ledger",
        "Ledger",
        "Serif headings and ruled sections for easy scanning.",
    ),
];

const ACCENTS: [(&str, &str); 8] = [
    ("ember", "Ember"),
    ("ocean", "Ocean"),
    ("mint", "Mint"),
    ("violet", "Violet"),
    ("rose", "Rose"),
    ("amber", "Amber"),
    ("teal", "Teal"),
    ("magenta", "Magenta"),
];

const PREFS: &[Pref] = &[
    pref(
        Page::General,
        "Auto-save workspace",
        "Save typing edits automatically. When off, structural changes and closing the window still save. Use Save now for pending edits.",
        Control::Toggle(&["general", "autoSave"], true),
    ),
    pref(
        Page::General,
        "Auto-save delay (ms)",
        "Wait after the last edit before saving. 100 to 60,000 ms.",
        Control::Number(&["general", "autoSaveIntervalMs"], 100, 60_000, 350),
    ),
    pref(
        Page::General,
        "Confirm before deleting",
        "Ask before deleting collections, folders, requests, or environments, and before closing tabs with unsaved changes.",
        Control::Toggle(&["general", "confirmBeforeDelete"], true),
    ),
    pref(
        Page::General,
        "Restore last session",
        "Reopen your request tabs at launch. Turning this off does not delete saved collections.",
        Control::Toggle(&["general", "restoreLastSession"], true),
    ),
    pref(
        Page::General,
        "Sidebar width (px)",
        "180 to 480 px. You can also drag the sidebar edge.",
        Control::Number(&["sidebarWidth"], 180, 480, 276),
    ),
    pref(
        Page::Appearance,
        "Workbench style",
        "Choose the typography, spacing, and shapes used across the app.",
        Control::Styles,
    ),
    pref(
        Page::Appearance,
        "Theme",
        "System follows your operating system's appearance.",
        Control::Choice(
            &["general", "theme"],
            &[("dark", "Dark"), ("light", "Light"), ("system", "System")],
        ),
    ),
    pref(
        Page::Appearance,
        "Accent color",
        "Recolors buttons, highlights, and the brand mark. Every preset has a tuned dark and light variant; method and status colors stay fixed.",
        Control::Accents,
    ),
    pref(
        Page::Request,
        "Default method",
        "Used for new requests only.",
        Control::Choice(&["request", "defaultMethod"], METHODS),
    ),
    pref(
        Page::Request,
        "Request timeout (ms)",
        "1 to 3,600,000 ms. 60,000 ms is one minute.",
        Control::Number(&["request", "requestTimeoutMs"], 1, 3_600_000, 60_000),
    ),
    pref(
        Page::Request,
        "Follow redirects",
        "",
        Control::Toggle(&["request", "followRedirects"], true),
    ),
    pref(
        Page::Request,
        "Maximum redirects",
        "0 to 100. Applies when following redirects.",
        Control::Number(&["request", "maxRedirects"], 0, 100, 10),
    ),
    Pref {
        warning: true,
        ..pref(
            Page::Request,
            "Verify TLS certificates",
            "Security: turning this off accepts invalid certificates and can expose requests to interception. Use only for trusted local testing.",
            Control::Toggle(&["request", "sslVerify"], true),
        )
    },
    pref(
        Page::Request,
        "Disable cache",
        "Send Cache-Control: no-cache and Pragma: no-cache headers. This does not clear server caches.",
        Control::Toggle(&["request", "disableCache"], false),
    ),
    grouped(
        Page::Request,
        "Cookies",
        "Persist cookies",
        "Save persistent cookies encrypted between sessions; session cookies always die when the app closes. Off keeps cookies for this session only and saves nothing.",
        Control::Toggle(&["request", "persistCookies"], true),
    ),
    grouped(
        Page::Request,
        "Cookies",
        "",
        "",
        Control::Actions(&[(Action::ClearCookies, "Clear all cookies for this workspace")]),
    ),
    grouped(
        Page::Request,
        "Proxy",
        "Mode",
        "Direct connects to the target. Manual asks for a proxy URL, credentials, and bypass hosts; credentials stay in the OS secret store.",
        Control::Proxy,
    ),
    pref(
        Page::Response,
        "Default view",
        "",
        Control::Choice(
            &["response", "defaultView"],
            &[("pretty", "Pretty"), ("raw", "Raw"), ("preview", "Preview")],
        ),
    ),
    pref(
        Page::Response,
        "Wrap response text",
        "",
        Control::Toggle(&["response", "wordWrap"], false),
    ),
    pref(
        Page::Response,
        "Show response time",
        "",
        Control::Toggle(&["response", "showTime"], true),
    ),
    pref(
        Page::Response,
        "Show response size",
        "",
        Control::Toggle(&["response", "showSize"], true),
    ),
    pref(
        Page::Response,
        "Response size limit (KB)",
        "1 to 102,400 KB. Larger bodies are truncated, with a notice in the response viewer.",
        Control::Number(&["response", "maxResponseSizeKB"], 1, 102_400, 2_000),
    ),
    pref(
        Page::Privacy,
        "Mask sensitive headers",
        "Hide authorization, cookies, and token, secret, or key values in the request header grid, Auth fields, the response header table, and the cookie manager. Masking changes display only; requests and copies carry real values.",
        Control::Toggle(&["privacy", "maskSensitiveHeaders"], true),
    ),
    pref(
        Page::Privacy,
        "Mask secret variables",
        "Hide values marked as secret wherever variables are displayed. Actual values are still sent in requests.",
        Control::Toggle(&["privacy", "maskSecretVariables"], true),
    ),
    grouped(
        Page::Transfer,
        "Postman collection",
        "Collections and environments",
        "Import Postman v2 collection or environment files, or export one from this workspace.",
        Control::Actions(&[
            (Action::ImportPostman, "Import…"),
            (Action::ExportCollection, "Export collection"),
            (Action::ExportEnvironment, "Export environment"),
        ]),
    ),
    grouped(
        Page::Transfer,
        "Application settings",
        "Settings file",
        "Preferences only. This file is not a backup of collections, environments, or open tabs.",
        Control::Actions(&[
            (Action::ImportSettings, "Import settings"),
            (Action::ExportSettings, "Export settings"),
        ]),
    ),
    grouped(
        Page::Transfer,
        "Full backup",
        "Backup file",
        "Every workspace in one file: collections, environments, open tabs, and preferences.",
        Control::Actions(&[
            (Action::ExportBackup, "Export backup"),
            (Action::RestoreBackup, "Import backup"),
        ]),
    ),
    pref(
        Page::About,
        "Application version",
        "",
        Control::Fact(concat!("Postlite ", "0.2.8", " · GPUI")),
    ),
    pref(Page::About, "License", "", Control::Fact("MIT License")),
    pref(
        Page::About,
        "Reset application settings",
        "Restore default preferences, including sidebar width. Your collections, environments, and saved requests are not deleted.",
        Control::Actions(&[(Action::ResetSettings, "Reset application settings")]),
    ),
];

const EXPORT_WARNING: &str = "Collection, environment, and backup exports include secrets unchanged, including auth tokens and secret variables. Store these files securely and do not commit them to source control.";

/// The Shortcuts page is generated from the live shortcut table, so each entry
/// becomes a searchable row too.
fn shortcut_prefs() -> impl Iterator<Item = Pref> {
    SHORTCUTS
        .iter()
        .map(|&(_, label, keys)| pref(Page::Shortcuts, label, "", Control::Shortcut(keys)))
}

fn read(settings: &Value, path: Path) -> &Value {
    path.iter().fold(settings, |value, key| &value[*key])
}

fn number_value(settings: &Value, path: Path, min: i64, max: i64, default: i64) -> i64 {
    read(settings, path)
        .as_f64()
        .filter(|value| value.is_finite())
        .map_or(default, |value| (value.round() as i64).clamp(min, max))
}

fn matches(pref: &Pref, query: &str) -> bool {
    query.is_empty()
        || [pref.label, pref.help, pref.group, page_name(pref.page)]
            .iter()
            .any(|text| text.to_lowercase().contains(query))
        || match pref.control {
            Control::Choice(_, options) => options
                .iter()
                .any(|(_, name)| name.to_lowercase().contains(query)),
            Control::Styles => STYLES
                .iter()
                .any(|(_, _, name, _)| name.to_lowercase().contains(query)),
            Control::Accents => ACCENTS
                .iter()
                .any(|(_, name)| name.to_lowercase().contains(query)),
            Control::Actions(actions) => actions
                .iter()
                .any(|(_, name)| name.to_lowercase().contains(query)),
            _ => false,
        }
}

fn page_name(page: Page) -> &'static str {
    PAGES
        .iter()
        .find(|(candidate, _)| *candidate == page)
        .map_or("", |(_, name)| name)
}

/// The application settings window. It keeps its own page, search, and number
/// fields, and re-renders whenever the workbench changes.
pub(super) struct Preferences {
    workbench: WeakEntity<Workbench>,
    page: Page,
    search: Entity<InputState>,
    numbers: Vec<(Path, i64, i64, i64, Entity<InputState>)>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl Preferences {
    /// `settings` is passed in because the workbench is mid-update while it
    /// opens this window and cannot be read yet.
    fn new(
        workbench: &Entity<Workbench>,
        settings: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search settings… (Ctrl+F or /)"));
        let mut subscriptions = vec![
            cx.observe(workbench, |_, _, cx| cx.notify()),
            cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        let mut numbers = Vec::new();
        for item in PREFS {
            let Control::Number(path, min, max, default) = item.control else {
                continue;
            };
            let current = number_value(settings, path, min, max, default).to_string();
            let input = cx.new(|cx| InputState::new(window, cx).default_value(current));
            let weak = workbench.downgrade();
            subscriptions.push(cx.subscribe_in(
                &input,
                window,
                move |_, input, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        let value = input.read(cx).value().trim().parse::<i64>();
                        if let Ok(value) = value.map_err(drop).and_then(|value| {
                            (min..=max).contains(&value).then_some(value).ok_or(())
                        }) {
                            let _ = weak.update(cx, |app, cx| {
                                write_path(app, path, json!(value), window, cx)
                            });
                        }
                    }
                    // Leaving the field shows the value that is actually in use.
                    InputEvent::Blur => {
                        let Some(app) = weak.upgrade() else { return };
                        let shown =
                            number_value(&app.read(cx).doc.settings, path, min, max, default);
                        input.update(cx, |input, cx| {
                            input.set_value(shown.to_string(), window, cx)
                        });
                    }
                    _ => {}
                },
            ));
            numbers.push((path, min, max, default, input));
        }
        Self {
            workbench: workbench.downgrade(),
            page: Page::General,
            search,
            numbers,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    fn query(&self, cx: &App) -> String {
        self.search.read(cx).value().trim().to_lowercase()
    }

    fn typing(&self, window: &Window, cx: &App) -> bool {
        std::iter::once(&self.search)
            .chain(self.numbers.iter().map(|number| &number.4))
            .any(|input| input.read(cx).focus_handle(cx).is_focused(window))
    }
}

impl Focusable for Preferences {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Preferences {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(app) = self.workbench.upgrade() else {
            return div();
        };
        let (palette, settings) = {
            let app = app.read(cx);
            (app.palette.clone(), app.doc.settings.clone())
        };
        let (p, settings) = (&palette, &settings);
        let query = self.query(cx);
        let shortcuts: Vec<Pref> = shortcut_prefs().collect();
        let all: Vec<&Pref> = PREFS.iter().chain(shortcuts.iter()).collect();
        let searching = !query.is_empty();
        let visible: Vec<&Pref> = all
            .iter()
            .copied()
            .filter(|item| {
                if searching {
                    matches(item, &query)
                } else {
                    item.page == self.page
                }
            })
            .collect();

        let mut nav = div().flex().flex_col().gap(px(2.)).w(px(168.)).flex_none();
        for (index, &(page, name)) in PAGES.iter().enumerate() {
            let hits = all
                .iter()
                .filter(|item| item.page == page && searching && matches(item, &query))
                .count();
            let active =
                !searching && page == self.page || searching && hits > 0 && page == self.page;
            nav = nav.child(
                div()
                    .id(("preferences-page", index))
                    .flex()
                    .items_center()
                    .h(px(32.))
                    .px(px(10.))
                    .rounded(px(p.appearance.row_radius.clamp(4., 8.)))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .text_color(if active { p.text } else { p.dim })
                    .when(active, |item| {
                        item.bg(p.selected).font_weight(FontWeight::SEMIBOLD)
                    })
                    .when(searching && hits == 0, |item| item.text_color(p.faint))
                    .hover(|item| item.bg(p.hover))
                    .child(div().flex_1().child(name))
                    .when(searching && hits > 0, |item| {
                        item.child(
                            div()
                                .px(px(6.))
                                .rounded(px(p.appearance.chip_radius.min(8.)))
                                .bg(p.raised)
                                .text_size(px(11.))
                                .text_color(p.dim)
                                .child(hits.to_string()),
                        )
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.page = page;
                        this.search
                            .update(cx, |input, cx| input.set_value("", window, cx));
                        cx.notify();
                    })),
            );
        }

        let mut content = div()
            .id("preferences-content")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .pr(px(6.))
            .flex()
            .flex_col();
        if searching && visible.is_empty() {
            content = content.child(div().pt(px(24.)).text_color(p.dim).child(format!(
                "No settings match “{}”. Try another word, or clear the search to see everything.",
                self.search.read(cx).value()
            )));
        }
        let mut page = None;
        let mut group = "";
        for item in &visible {
            if page != Some(item.page) {
                page = Some(item.page);
                group = "";
                content = content.child(page_title(page_name(item.page), p));
                if item.page == Page::Shortcuts {
                    content =
                        content.child(note("Fixed keyboard shortcuts. Ctrl is Cmd on macOS.", p));
                }
                if item.page == Page::Transfer {
                    content = content.child(warning(EXPORT_WARNING, p));
                }
            }
            if item.group != group {
                group = item.group;
                if !group.is_empty() {
                    content = content.child(
                        div()
                            .mt(px(14.))
                            .pb(px(4.))
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.faint)
                            .child(group.to_uppercase()),
                    );
                }
            }
            content = content.child(self.row(item, settings, p, window, cx));
        }

        div()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .gap(px(12.))
            // Capture so a focused field cannot swallow Ctrl+F first.
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                let keystroke = &event.keystroke;
                let find = keystroke.modifiers.secondary() && keystroke.key == "f";
                let slash = keystroke.key == "/"
                    && !keystroke.modifiers.modified()
                    && !this.typing(window, cx);
                if find || slash {
                    cx.stop_propagation();
                    this.search.update(cx, |input, cx| input.focus(window, cx));
                    // Repaint so the field registers as the window's text
                    // target; a cached frame would keep text going nowhere.
                    cx.notify();
                }
            }))
            .child(
                div().text_size(px(12.)).text_color(p.dim).child(
                    "Preferences apply across all workspaces. Changes take effect immediately.",
                ),
            )
            .child(Input::new(&self.search).cleanable(true))
            .child(
                div()
                    .flex()
                    .gap(px(18.))
                    .h(px(500.))
                    .pt(px(12.))
                    .border_t_1()
                    .border_color(p.line)
                    .child(nav)
                    .child(content),
            )
    }
}

impl Preferences {
    fn row(
        &self,
        item: &Pref,
        settings: &Value,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let stacked = matches!(item.control, Control::Styles | Control::Accents);
        let mut copy = div().flex().flex_col().gap(px(3.)).flex_1().min_w_0();
        if !item.label.is_empty() {
            copy = copy.child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.text)
                    .child(item.label),
            );
        }
        if !item.help.is_empty() {
            copy = copy.child(
                div()
                    .text_size(px(12.))
                    .line_height(px(17.))
                    .text_color(if item.warning { p.warn } else { p.dim })
                    .child(item.help),
            );
        }
        let control = self.control(item, settings, p, window, cx);
        let row = div()
            .py(px(11.))
            .border_b_1()
            .border_color(p.soft_line)
            .flex()
            .gap(px(16.));
        if stacked {
            row.flex_col().gap(px(10.)).child(copy).child(control)
        } else {
            row.items_center()
                .child(copy)
                .child(div().flex_none().child(control))
        }
    }

    fn control(
        &self,
        item: &Pref,
        settings: &Value,
        p: &Palette,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let weak = self.workbench.clone();
        match item.control {
            Control::Toggle(path, default) => {
                let checked = read(settings, path).as_bool().unwrap_or(default);
                Switch::new(path[path.len() - 1])
                    .checked(checked)
                    .on_click(move |checked, window, cx| {
                        let _ = weak.update(cx, |app, cx| {
                            write_path(app, path, json!(*checked), window, cx)
                        });
                    })
                    .into_any_element()
            }
            Control::Choice(path, options) => {
                let current = read(settings, path)
                    .as_str()
                    .unwrap_or(options[0].0)
                    .to_owned();
                let label = options
                    .iter()
                    .find(|(value, _)| *value == current)
                    .map_or(options[0].1, |(_, name)| name);
                let label = label.to_owned();
                choice_menu(path[path.len() - 1], &label, p, move |menu| {
                    let mut menu = menu;
                    for &(value, name) in options {
                        let weak = weak.clone();
                        menu =
                            menu.item(PopupMenuItem::new(name).checked(value == current).on_click(
                                move |_, window, cx| {
                                    let _ = weak.update(cx, |app, cx| {
                                        write_path(app, path, json!(value), window, cx)
                                    });
                                },
                            ));
                    }
                    menu
                })
                .into_any_element()
            }
            Control::Number(path, ..) => {
                let input = self
                    .numbers
                    .iter()
                    .find(|number| number.0 == path)
                    .map(|number| number.4.clone());
                div()
                    .w(px(132.))
                    .children(input.map(|input| Input::new(&input).small()))
                    .into_any_element()
            }
            Control::Styles => style_cards(settings, p, weak, cx).into_any_element(),
            Control::Accents => accent_swatches(settings, p, weak).into_any_element(),
            Control::Proxy => {
                let mode = settings["request"]["proxy"]["mode"]
                    .as_str()
                    .unwrap_or("direct")
                    .to_owned();
                let options: &[(&str, &str)] = &[
                    ("direct", "Direct (no proxy)"),
                    ("manual", "Manual…"),
                    ("environment", "Environment variables"),
                ];
                let label = options
                    .iter()
                    .find(|(value, _)| *value == mode.as_str())
                    .map_or("Direct (no proxy)", |(_, name)| name);
                choice_menu(
                    "proxy-mode",
                    label.trim_end_matches('…'),
                    p,
                    move |mut menu| {
                        for &(value, name) in options {
                            let weak = weak.clone();
                            menu = menu.item(
                                PopupMenuItem::new(name).checked(value == mode).on_click(
                                    move |_, window, cx| {
                                        let _ = weak.update(cx, |app, cx| {
                                            if value == "manual" {
                                                app.open_proxy(window, cx);
                                            } else {
                                                let mut proxy =
                                                    app.doc.settings["request"]["proxy"].clone();
                                                if !proxy.is_object() {
                                                    proxy = json!({});
                                                }
                                                proxy["mode"] = json!(value);
                                                app.setting("request", "proxy", proxy, window, cx);
                                            }
                                        });
                                    },
                                ),
                            );
                        }
                        menu
                    },
                )
                .into_any_element()
            }
            Control::Actions(actions) => div()
                .flex()
                .flex_wrap()
                .gap(px(8.))
                .children(actions.iter().enumerate().map(|(index, &(action, label))| {
                    let weak = weak.clone();
                    let mut button = Button::new((label, index)).outline().small().label(label);
                    let collections = matches!(action, Action::ExportCollection);
                    if collections || matches!(action, Action::ExportEnvironment) {
                        let entries: Vec<(String, String)> = weak
                            .upgrade()
                            .map(|app| {
                                let workspace = app.read(cx).workspace();
                                let list = if collections {
                                    &workspace.collections
                                } else {
                                    &workspace.environments
                                };
                                list.iter()
                                    .map(|entry| {
                                        (
                                            entry["id"].as_str().unwrap_or_default().to_owned(),
                                            entry["name"].as_str().unwrap_or("Untitled").to_owned(),
                                        )
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        if entries.is_empty() {
                            return button.disabled(true).into_any_element();
                        }
                        return button
                            .dropdown_menu(move |mut menu, _, _| {
                                for (id, name) in &entries {
                                    let (weak, id) = (weak.clone(), id.clone());
                                    menu = menu.item(PopupMenuItem::new(name.clone()).on_click(
                                        move |_, _, cx| {
                                            let _ = weak
                                                .update(cx, |app, cx| app.export_resource(&id, cx));
                                        },
                                    ));
                                }
                                menu
                            })
                            .into_any_element();
                    }
                    if matches!(action, Action::ResetSettings | Action::ClearCookies) {
                        button = button.danger();
                    }
                    button
                        .on_click(move |_, window, cx| {
                            let _ = weak.update(cx, |app, cx| run_action(app, action, window, cx));
                        })
                        .into_any_element()
                }))
                .into_any_element(),
            Control::Shortcut(keys) => div()
                .flex()
                .items_center()
                .gap(px(4.))
                .children(keys.iter().enumerate().flat_map(|(index, key)| {
                    let plus = (index > 0).then(|| {
                        div()
                            .text_size(px(11.))
                            .text_color(p.faint)
                            .child("+")
                            .into_any_element()
                    });
                    plus.into_iter()
                        .chain(std::iter::once(keycap(key, p).into_any_element()))
                }))
                .into_any_element(),
            Control::Fact(text) => div()
                .text_size(px(13.))
                .text_color(p.dim)
                .child(text)
                .into_any_element(),
        }
    }
}

fn page_title(name: &'static str, p: &Palette) -> Div {
    div()
        .pt(px(4.))
        .pb(px(6.))
        .font_weight(FontWeight::BOLD)
        .font_family(p.appearance.heading_font.family())
        .text_size(px(p.appearance.heading.max(15.)))
        .text_color(p.text)
        .child(name)
}

fn note(text: &'static str, p: &Palette) -> Div {
    div()
        .pb(px(4.))
        .text_size(px(12.))
        .text_color(p.dim)
        .child(text)
}

fn warning(text: &'static str, p: &Palette) -> Div {
    div()
        .my(px(4.))
        .p(px(10.))
        .rounded(px(p.appearance.chip_radius.min(8.)))
        .border_1()
        .border_color(translucent(p.warn, 0.4))
        .bg(translucent(p.warn, 0.08))
        .text_size(px(12.))
        .text_color(p.text)
        .child(text)
}

fn translucent(color: Rgba, alpha: f32) -> Rgba {
    Rgba { a: alpha, ..color }
}

fn keycap(key: &str, p: &Palette) -> Div {
    div()
        .px(px(7.))
        .py(px(2.))
        .rounded(px(p.appearance.chip_radius.min(5.)))
        .border_1()
        .border_color(p.line)
        .bg(p.raised)
        .font_family(crate::appearance::mono())
        .text_size(px(11.))
        .text_color(p.text)
        .child(SharedString::from(key.to_owned()))
}

fn choice_menu(
    id: &'static str,
    label: &str,
    p: &Palette,
    menu: impl Fn(gpui_component::menu::PopupMenu) -> gpui_component::menu::PopupMenu + 'static,
) -> impl IntoElement {
    Button::new(id)
        .outline()
        .small()
        .min_w(px(132.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().flex_1().child(label.to_owned()))
                .child(crate::appearance::icon("chevron-down", p.faint, 12.)),
        )
        .dropdown_menu(move |popup, _, _| menu(popup))
}

/// Four cards that preview each workbench style in its own type, surfaces,
/// and corner shape, using the current theme and accent.
fn style_cards(settings: &Value, p: &Palette, weak: WeakEntity<Workbench>, _: &App) -> Div {
    let current = Appearance::read(settings);
    div()
        .grid()
        .grid_cols(2)
        .gap(px(10.))
        .children(STYLES.iter().map(|&(appearance, value, name, detail)| {
            let mut preview_settings = settings.clone();
            preview_settings["general"]["appearance"] = json!(value);
            let look = Palette::new(&preview_settings, p.dark);
            let metrics = appearance.metrics();
            let selected = appearance == current;
            let radius = metrics.radius.min(10.);
            let weak = weak.clone();
            div()
                .id(value)
                .flex()
                .flex_col()
                .gap(px(8.))
                .p(px(10.))
                .rounded(px(10.))
                .border_1()
                .border_color(if selected { p.accent } else { p.line })
                .when(selected, |card| card.bg(p.selected))
                .cursor_pointer()
                .hover(|card| card.border_color(p.accent))
                .on_click(move |_, window, cx| {
                    let _ = weak.update(cx, |app, cx| {
                        app.setting("general", "appearance", json!(value), window, cx)
                    });
                })
                .child(
                    // A miniature workbench: chrome strip, tab, address bar, and send.
                    div()
                        .h(px(54.))
                        .rounded(px(radius.min(8.)))
                        .overflow_hidden()
                        .border_1()
                        .border_color(look.line)
                        .bg(look.bg)
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .h(px(12.))
                                .bg(look.side)
                                .border_b_1()
                                .border_color(look.line),
                        )
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .gap(px(5.))
                                .px(px(8.))
                                .bg(look.panel)
                                .child(
                                    div()
                                        .flex_1()
                                        .h(px(14.))
                                        .rounded(px(radius.min(7.)))
                                        .border_1()
                                        .border_color(look.line)
                                        .bg(look.input),
                                )
                                .child(
                                    div()
                                        .w(px(26.))
                                        .h(px(14.))
                                        .rounded(px(radius.min(7.)))
                                        .bg(look.primary),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .font_family(metrics.heading_font.family())
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(14.))
                                .text_color(p.text)
                                .child(name),
                        )
                        .when(selected, |line| {
                            line.child(crate::appearance::icon("check", p.accent, 13.))
                        }),
                )
                .child(div().text_size(px(12.)).text_color(p.dim).child(detail))
        }))
}

fn accent_swatches(settings: &Value, p: &Palette, weak: WeakEntity<Workbench>) -> Div {
    let current = settings["general"]["accent"].as_str().unwrap_or("ember");
    div()
        .flex()
        .flex_wrap()
        .gap(px(8.))
        .children(ACCENTS.iter().map(|&(value, name)| {
            let selected = value == current;
            let hue = primary_fill(value, p.dark);
            let weak = weak.clone();
            div()
                .id(value)
                .flex()
                .items_center()
                .gap(px(7.))
                .h(px(32.))
                .pl(px(6.))
                .pr(px(11.))
                .rounded_full()
                .border_1()
                .border_color(if selected { hue } else { p.line })
                .when(selected, |chip| chip.bg(p.selected))
                .cursor_pointer()
                .hover(|chip| chip.bg(p.hover))
                .on_click(move |_, window, cx| {
                    let _ = weak.update(cx, |app, cx| {
                        app.setting("general", "accent", json!(value), window, cx)
                    });
                })
                .child(
                    div()
                        .size(px(20.))
                        .rounded_full()
                        .bg(hue)
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(selected, |dot| {
                            dot.child(crate::appearance::icon(
                                "check",
                                Palette::new(&json!({"general": {"accent": value}}), p.dark)
                                    .primary_text,
                                12.,
                            ))
                        }),
                )
                .child(div().text_size(px(12.)).text_color(p.text).child(name))
        }))
}

fn write_path(
    app: &mut Workbench,
    path: Path,
    value: Value,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    match path {
        [section, name] => app.setting(section, name, value, window, cx),
        ["sidebarWidth"] => {
            Axis::Sidebar.write(
                &mut app.doc.settings,
                value.as_f64().map(|width| width as f32),
            );
            app.changed(cx);
        }
        _ => {}
    }
}

fn run_action(
    app: &mut Workbench,
    action: Action,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    match action {
        Action::ClearCookies => app.confirm_clear_cookies(window, cx),
        Action::ImportPostman => app.import_postman(cx),
        Action::ExportCollection | Action::ExportEnvironment => {}
        Action::ExportSettings => {
            let mut settings = app.doc.settings.clone();
            if let Some(object) = settings.as_object_mut() {
                object.remove("gpuiLayout");
            }
            app.save_document(
                json!({"kind": "postlite.settings", "version": 1, "settings": settings}),
                "postlite.settings.json",
                cx,
            );
        }
        Action::ImportSettings => app.import_settings(window, cx),
        Action::ExportBackup => app.request_backup_export(window, cx),
        Action::RestoreBackup => app.request_backup_restore(window, cx),
        Action::ResetSettings => {
            let view = cx.entity().downgrade();
            window.open_dialog(cx, move |dialog, _, _| {
                let view = view.clone();
                dialog
                    .title("Reset application settings?")
                    .confirm()
                    .button_props(DialogButtonProps::default().ok_text("Reset"))
                    .child("Restore default preferences, including sidebar width. Collections, environments, and saved requests are kept.")
                    .on_ok(move |_, window, cx| {
                        let _ = view.update(cx, |app, cx| {
                            let layout = app.doc.settings["gpuiLayout"].take();
                            app.replace_settings(json!({}), window, cx);
                            if !layout.is_null() {
                                app.doc.settings["gpuiLayout"] = layout;
                            }
                        });
                        true
                    })
            });
        }
    }
}

impl Workbench {
    pub(super) fn open_settings(&self, window: &mut Window, cx: &mut Context<Self>) {
        let workbench = cx.entity();
        let preferences = cx.new(|cx| Preferences::new(&workbench, &self.doc.settings, window, cx));
        let save = workbench.downgrade();
        let content = preferences.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let save = save.clone();
            dialog
                .title("Application settings")
                .width(px(880.))
                .margin_top(px(40.))
                .overlay_closable(true)
                .button_props(DialogButtonProps::default().ok_text("Done"))
                .footer(move |ok, _, window, cx| {
                    let save = save.clone();
                    vec![
                        Button::new("save-workspace-now")
                            .outline()
                            .label("Save workspace now")
                            .on_click(move |_, _, cx| {
                                let _ = save.update(cx, |app, cx| app.save_profile(cx));
                            })
                            .into_any_element(),
                        ok(window, cx),
                    ]
                })
                .child(content.clone())
        });
        let focus = preferences.read(cx).focus.clone();
        window.focus(&focus);
    }

    /// Replaces every preference at once (import or reset) and reapplies the
    /// theme, masking, cookie, and response settings that depend on them.
    fn replace_settings(&mut self, settings: Value, window: &mut Window, cx: &mut Context<Self>) {
        self.doc.settings = settings;
        self.palette = theme(&self.doc.settings, window, cx);
        self.sync_cookie_preference(cx);
        self.res_section = self.default_response_section();
        self.refresh_body(window, cx);
        self.mask_secrets = self.doc.settings["privacy"]["maskSensitiveHeaders"]
            .as_bool()
            .unwrap_or(true);
        self.changed(cx);
    }

    fn import_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import settings".into()),
        });
        cx.spawn_in(window, async move |view, cx| {
            let Ok(Ok(Some(paths))) = prompt.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let parsed = cx
                .background_executor()
                .spawn(async move { parse_settings_file(&path) })
                .await;
            let _ = view.update_in(cx, |this, window, cx| match parsed {
                Ok(mut settings) => {
                    let layout = this.doc.settings["gpuiLayout"].clone();
                    if !layout.is_null() {
                        settings["gpuiLayout"] = layout;
                    }
                    this.replace_settings(settings, window, cx);
                }
                Err(error) => {
                    this.error = Some(error);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn setting(
        &mut self,
        section: &str,
        name: &str,
        value: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.doc.settings[section].is_object() {
            self.doc.settings[section] = json!({});
        }
        self.doc.settings[section][name] = value;
        if (section, name) == ("request", "persistCookies") {
            self.sync_cookie_preference(cx);
        }
        if section == "general" && matches!(name, "theme" | "appearance" | "accent") {
            self.palette = theme(&self.doc.settings, window, cx);
        }
        if (section, name) == ("response", "defaultView") {
            self.res_section = self.default_response_section();
            self.refresh_body(window, cx);
        }
        if (section, name) == ("response", "wordWrap")
            && let Some(body) = &self.body
        {
            let wrap = self.doc.settings["response"]["wordWrap"] != false;
            body.update(cx, |input, cx| input.set_soft_wrap(wrap, window, cx));
        }
        self.mask_secrets = self.doc.settings["privacy"]["maskSensitiveHeaders"]
            .as_bool()
            .unwrap_or(true);
        self.changed(cx);
        if name == "autoSave" {
            self.save_profile(cx);
        }
    }
}

pub(super) fn theme(settings: &Value, window: &Window, cx: &mut App) -> Palette {
    let dark = match settings["general"]["theme"].as_str() {
        Some("light") => false,
        Some("system") => matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ),
        _ => true,
    };
    let palette = Palette::new(settings, dark);
    Theme::change(
        if dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        },
        None,
        cx,
    );
    let theme = Theme::global_mut(cx);
    theme.primary = palette.primary.into();
    theme.primary_foreground = palette.primary_text.into();
    theme.primary_hover = palette.primary_hover.into();
    theme.primary_active = palette.primary_active.into();
    theme.secondary = palette.raised.into();
    theme.secondary_foreground = palette.text.into();
    theme.secondary_hover = palette.hover.into();
    theme.secondary_active = palette.selected.into();
    theme.accent = palette.selected.into();
    theme.accent_foreground = palette.text.into();
    theme.popover = palette.panel.into();
    theme.popover_foreground = palette.text.into();
    theme.muted = palette.raised.into();
    theme.muted_foreground = palette.dim.into();
    theme.list = palette.panel.into();
    theme.list_hover = palette.hover.into();
    theme.list_active = palette.selected.into();
    theme.list_active_border = palette.accent.into();
    theme.background = palette.input.into();
    theme.foreground = palette.text.into();
    theme.border = palette.line.into();
    theme.input = palette.line.into();
    theme.ring = palette.accent.into();
    palette
}

/// Reads a `postlite.settings` file written by either desktop app.
fn parse_settings_file(path: &std::path::Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("Could not read the settings file: {error}"))?;
    parse_settings(&text)
}

fn parse_settings(text: &str) -> Result<Value, String> {
    let document: Value = serde_json::from_str(text)
        .map_err(|error| format!("The settings file is not valid JSON: {error}"))?;
    match (
        &document["kind"],
        &document["version"],
        &document["settings"],
    ) {
        (kind, version, settings @ Value::Object(_))
            if kind == "postlite.settings" && version == 1 =>
        {
            Ok(settings.clone())
        }
        _ => Err("Not a supported Postlite settings file.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_files_round_trip_and_reject_other_documents() {
        let file = json!({"kind": "postlite.settings", "version": 1,
            "settings": {"general": {"accent": "teal"}}});
        assert_eq!(
            parse_settings(&file.to_string()).unwrap()["general"]["accent"],
            "teal"
        );
        assert!(parse_settings(r#"{"kind":"postlite.backup","version":1,"settings":{}}"#).is_err());
        assert!(
            parse_settings(r#"{"kind":"postlite.settings","version":1,"settings":[]}"#).is_err()
        );
        assert!(parse_settings("not json").is_err());
    }

    #[test]
    fn numbers_clamp_to_their_documented_range() {
        let settings =
            json!({"request": {"maxRedirects": 500.4}, "general": {"autoSaveIntervalMs": "x"}});
        assert_eq!(
            number_value(&settings, &["request", "maxRedirects"], 0, 100, 10),
            100
        );
        assert_eq!(
            number_value(
                &settings,
                &["general", "autoSaveIntervalMs"],
                100,
                60_000,
                350
            ),
            350
        );
    }

    #[test]
    fn search_finds_rows_by_label_help_and_option_names() {
        let hits = |query: &str| PREFS.iter().filter(|item| matches(item, query)).count();
        assert!(hits("tls") >= 1);
        assert!(hits("magenta") == 1);
        assert!(hits("ledger") == 1);
        assert_eq!(hits("zzz-nothing"), 0);
    }
}
