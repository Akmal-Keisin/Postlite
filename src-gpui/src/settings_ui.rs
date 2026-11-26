use super::Workbench;
use crate::appearance::Palette;
use gpui::{
    App, Context, Div, FontWeight, InteractiveElement, ParentElement, StatefulInteractiveElement,
    Styled, WeakEntity, Window, WindowAppearance, div, px,
};
use gpui_component::{
    Sizable, Theme, ThemeMode, WindowExt,
    button::{Button, ButtonVariants},
};
use serde_json::{Value, json};

impl Workbench {
    pub(super) fn open_settings(&self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let Some(view) = weak.upgrade() else {
                return dialog;
            };
            let view = view.read(cx);
            let p = &view.palette;
            let settings = &view.doc.settings;
            let mut form = div()
                .id("postlite-preferences")
                .max_h(px(510.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(15.))
                .pr(px(4.))
                .child(view_title("Appearance", p))
                .child(settings_row(
                    &weak,
                    settings,
                    "general",
                    "appearance",
                    "Workbench style",
                    &[
                        ("workbench", "Workbench"),
                        ("terminal", "Terminal"),
                        ("studio", "Studio"),
                        ("ledger", "Ledger"),
                    ],
                ))
                .child(settings_row(
                    &weak,
                    settings,
                    "general",
                    "theme",
                    "Theme",
                    &[("dark", "Dark"), ("light", "Light"), ("system", "System")],
                ))
                .child(settings_row(
                    &weak,
                    settings,
                    "general",
                    "accent",
                    "Accent",
                    &[
                        ("ember", "Ember"),
                        ("ocean", "Ocean"),
                        ("mint", "Mint"),
                        ("violet", "Violet"),
                        ("rose", "Rose"),
                        ("amber", "Amber"),
                        ("teal", "Teal"),
                        ("magenta", "Magenta"),
                    ],
                ))
                .child(view_title("Request", p))
                .child(settings_row(
                    &weak,
                    settings,
                    "request",
                    "defaultMethod",
                    "Default method",
                    &[
                        ("GET", "GET"),
                        ("POST", "POST"),
                        ("PUT", "PUT"),
                        ("PATCH", "PATCH"),
                        ("DELETE", "DELETE"),
                        ("HEAD", "HEAD"),
                        ("OPTIONS", "OPTIONS"),
                        ("QUERY", "QUERY"),
                    ],
                ))
                .child(settings_row(
                    &weak,
                    &settings["request"],
                    "proxy",
                    "mode",
                    "Proxy",
                    &[
                        ("direct", "Direct"),
                        ("manual", "Manual"),
                        ("environment", "From environment"),
                    ],
                ))
                .child(view_title("Response", p))
                .child(settings_row(
                    &weak,
                    settings,
                    "response",
                    "defaultView",
                    "Default view",
                    &[("pretty", "Pretty"), ("raw", "Raw"), ("preview", "Preview")],
                ))
                .child(view_title("Session and privacy", p));
            for (section, key, caption) in [
                ("general", "restoreLastSession", "Restore last session"),
                ("general", "autoSave", "Auto-save"),
                ("general", "confirmBeforeDelete", "Confirm before deleting"),
                ("request", "followRedirects", "Follow redirects"),
                ("request", "sslVerify", "Verify TLS certificates"),
                (
                    "request",
                    "persistCookies",
                    "Persist cookies between launches",
                ),
                ("privacy", "maskSensitiveHeaders", "Mask sensitive headers"),
                ("privacy", "maskSecretVariables", "Mask secret variables"),
            ] {
                let checked = settings[section][key].as_bool().unwrap_or(true);
                let weak = weak.clone();
                form = form.child(
                    Button::new(key)
                        .outline()
                        .small()
                        .w_full()
                        .label(format!("{} {caption}", if checked { "✓" } else { "○" }))
                        .on_click(move |_, window, cx| {
                            let _ = weak.update(cx, |app, cx| {
                                app.setting(section, key, json!(!checked), window, cx)
                            });
                        }),
                );
            }
            let export = weak.clone();
            let restore = weak.clone();
            let cookies = weak.clone();
            form = form.child(
                Button::new("clear-cookies")
                    .outline()
                    .small()
                    .label("Clear workspace cookies")
                    .on_click(move |_, window, cx| {
                        let _ = cookies.update(cx, |app, cx| app.confirm_clear_cookies(window, cx));
                    }),
            );
            form = form.child(view_title("Import / Export", p)).child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        Button::new("export-backup")
                            .outline()
                            .label("Export backup")
                            .on_click(move |_, window, cx| {
                                let _ = export
                                    .update(cx, |app, cx| app.request_backup_export(window, cx));
                            }),
                    )
                    .child(
                        Button::new("restore-backup")
                            .outline()
                            .label("Restore backup")
                            .on_click(move |_, window, cx| {
                                let _ = restore
                                    .update(cx, |app, cx| app.request_backup_restore(window, cx));
                            }),
                    ),
            );
            dialog
                .title("Application settings")
                .width(px(710.))
                .alert()
                .child(form)
        });
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
        self.mask_secrets = self.doc.settings["privacy"]["maskSensitiveHeaders"]
            .as_bool()
            .unwrap_or(true);
        self.changed(cx);
        if name == "autoSave" {
            self.save_profile(cx);
        }
    }
}

fn settings_row(
    view: &WeakEntity<Workbench>,
    settings: &Value,
    section: &'static str,
    key: &'static str,
    label: &'static str,
    options: &[(&'static str, &'static str)],
) -> Div {
    let selected = settings[section][key].as_str().unwrap_or(options[0].0);
    div()
        .flex()
        .flex_col()
        .gap(px(5.))
        .child(div().text_size(px(12.)).child(label))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(5.))
                .children(options.iter().map(|&(value, name)| {
                    let view = view.clone();
                    let button = Button::new((
                        key,
                        options
                            .iter()
                            .position(|option| option.0 == value)
                            .unwrap_or(0),
                    ))
                    .small()
                    .label(name)
                    .on_click(move |_, window, cx| {
                        let _ = view.update(cx, |app, cx| {
                            if section == "proxy" {
                                if value == "manual" {
                                    app.open_proxy(window, cx);
                                } else {
                                    let mut proxy = app.doc.settings["request"]["proxy"].clone();
                                    if !proxy.is_object() {
                                        proxy = json!({});
                                    }
                                    proxy["mode"] = json!(value);
                                    app.setting("request", "proxy", proxy, window, cx);
                                }
                            } else {
                                app.setting(section, key, json!(value), window, cx);
                            }
                        });
                    });
                    if selected == value {
                        button.primary()
                    } else {
                        button.outline()
                    }
                })),
        )
}

fn view_title(label: &'static str, p: &Palette) -> Div {
    div()
        .font_weight(FontWeight::BOLD)
        .font_family(p.appearance.heading_font())
        .text_size(px(p.appearance.heading()))
        .child(label)
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
    theme.primary = palette.accent.into();
    theme.primary_foreground = palette.accent_text.into();
    theme.background = palette.input.into();
    theme.foreground = palette.text.into();
    theme.border = palette.line.into();
    theme.input = palette.line.into();
    theme.ring = palette.accent.into();
    palette
}
