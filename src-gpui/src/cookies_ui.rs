use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

use chrono::{DateTime, Local};
use gpui::{
    AppContext, ClipboardItem, Context, Div, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Render, Rgba, StatefulInteractiveElement, Styled, WeakEntity, Window, div, px,
};
use gpui_component::{
    Disableable, Sizable, WindowExt,
    button::{Button, ButtonVariant, ButtonVariants},
    dialog::DialogButtonProps,
};
use postlite_core::session::{CookieFilter, CookieRow, CookieSnapshot};

use super::Workbench;
use crate::{
    appearance::{MONO, Palette, icon},
    http::Session,
};

impl Workbench {
    pub(super) fn open_cookies(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut drawer = CookieDrawer {
            owner: cx.entity().downgrade(),
            session: self.session.clone(),
            workspace_id: self.workspace().id.clone(),
            snapshot: None,
            domain: None,
            toggled: HashSet::new(),
            busy: false,
            refresh_pending: false,
            notice: None,
        };
        let state = cx.new(|cx| {
            drawer.refresh(cx);
            drawer
        });
        self.cookies_drawer = Some(state.downgrade());
        window.open_sheet(cx, move |sheet, _, _| {
            sheet.title("Cookies").size(px(320.)).child(state.clone())
        });
    }
}

type CookieKey = (String, String, String);

pub(super) struct CookieDrawer {
    owner: WeakEntity<Workbench>,
    session: Arc<Session>,
    workspace_id: String,
    snapshot: Option<CookieSnapshot>,
    domain: Option<String>,
    toggled: HashSet<CookieKey>,
    busy: bool,
    refresh_pending: bool,
    notice: Option<Result<String, String>>,
}

impl CookieDrawer {
    pub(super) fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            self.refresh_pending = true;
            return;
        }
        self.notice = None;
        self.update_jar(None, cx);
    }

    fn update_jar(&mut self, deletion: Option<(CookieFilter, String)>, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        cx.notify();
        let session = self.session.clone();
        let workspace_id = self.workspace_id.clone();
        let work = cx.background_executor().spawn(async move {
            let notice = deletion.map(|(filter, label)| {
                session
                    .cookies
                    .remove(&workspace_id, &filter)
                    .map(|removed| {
                        if removed == 0 {
                            format!("{label}: nothing to delete.")
                        } else {
                            format!("{label}: {} deleted.", counted(removed, "cookie"))
                        }
                    })
            });
            (session.cookies.snapshot(&workspace_id), notice)
        });
        cx.spawn(async move |entity, cx| {
            let (snapshot, notice) = work.await;
            let _ = entity.update(cx, |drawer, cx| {
                drawer.busy = false;
                drawer.toggled.clear();
                drawer.notice = notice;
                match snapshot {
                    Ok(snapshot) => {
                        if let Some(domain) = &drawer.domain
                            && !snapshot.rows.iter().any(|row| &row.domain == domain)
                        {
                            drawer.domain = None;
                        }
                        drawer.snapshot = Some(snapshot);
                    }
                    Err(error) => {
                        drawer.snapshot = None;
                        drawer.notice = Some(Err(error));
                    }
                }
                cx.notify();
                if std::mem::take(&mut drawer.refresh_pending) {
                    drawer.update_jar(None, cx);
                }
            });
        })
        .detach();
    }

    fn confirm_domain_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(domain) = self.domain.clone() else {
            return;
        };
        let count = self.snapshot.as_ref().map_or(0, |snapshot| {
            snapshot
                .rows
                .iter()
                .filter(|row| row.domain == domain)
                .count()
        });
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let weak = weak.clone();
            let domain = domain.clone();
            dialog
                .title("Delete domain cookies")
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete domain")
                        .ok_variant(ButtonVariant::Danger),
                )
                .child(format!(
                    "Delete {} for {domain} in this workspace? Sites will see you as signed out.",
                    counted(count, "cookie")
                ))
                .on_ok(move |_, _, cx| {
                    let _ = weak.update(cx, |drawer, cx| {
                        drawer.update_jar(
                            Some((
                                CookieFilter {
                                    domain: Some(domain.clone()),
                                    ..Default::default()
                                },
                                domain.clone(),
                            )),
                            cx,
                        );
                    });
                    true
                })
        });
    }

    fn row(
        &self,
        row: &CookieRow,
        index: usize,
        mask: bool,
        palette: &Palette,
        cx: &Context<Self>,
    ) -> Div {
        let key = (row.domain.clone(), row.path.clone(), row.name.clone());
        let masked = mask ^ self.toggled.contains(&key);
        let value = if masked {
            "••••••••"
        } else {
            &row.value
        };
        let copy_key = key.clone();
        let filter = CookieFilter {
            domain: Some(row.domain.clone()),
            path: Some(row.path.clone()),
            name: Some(row.name.clone()),
        };
        let name = row.name.clone();
        let expiry = row
            .expires_ms
            .and_then(DateTime::from_timestamp_millis)
            .filter(|_| !row.session)
            .map(|date| {
                date.with_timezone(&Local)
                    .format("%d %b, %H:%M")
                    .to_string()
            })
            .unwrap_or_else(|| "Session".to_owned());
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(8.))
            .py(px(6.))
            .border_b_1()
            .border_color(palette.soft_line)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(MONO)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(row.name.clone()),
                    )
                    .children(row.secure.then(|| chip("Secure", palette.ok, palette)))
                    .children(
                        row.http_only
                            .then(|| chip("HttpOnly", palette.cyan, palette)),
                    )
                    .children(
                        row.same_site
                            .as_deref()
                            .map(|site| chip(site, palette.violet, palette)),
                    )
                    .child(
                        Button::new(("cookie-delete", index))
                            .ghost()
                            .small()
                            .disabled(self.busy)
                            .tooltip(format!("Delete cookie {}", row.name))
                            .child(icon("trash", palette.faint, 14.))
                            .on_click(cx.listener(move |drawer, _, _, cx| {
                                drawer.update_jar(Some((filter.clone(), name.clone())), cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_normal()
                            .font_family(MONO)
                            .rounded(px(5.))
                            .border_1()
                            .border_color(palette.line)
                            .bg(palette.input)
                            .px(px(7.))
                            .py(px(3.))
                            .child(value.to_owned()),
                    )
                    .child(
                        Button::new(("cookie-toggle", index))
                            .ghost()
                            .small()
                            .tooltip(if masked { "Reveal value" } else { "Hide value" })
                            .child(icon(
                                if masked { "eye" } else { "eye-off" },
                                palette.faint,
                                14.,
                            ))
                            .on_click(cx.listener(move |drawer, _, _, cx| {
                                if !drawer.toggled.remove(&key) {
                                    drawer.toggled.insert(key.clone());
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(("cookie-copy", index))
                            .ghost()
                            .small()
                            .tooltip("Copy value")
                            .child(icon("copy", palette.faint, 14.))
                            .on_click(cx.listener(move |drawer, _, _, cx| {
                                let row = drawer.snapshot.as_ref().and_then(|snapshot| {
                                    snapshot.rows.iter().find(|row| {
                                        row.domain == copy_key.0
                                            && row.path == copy_key.1
                                            && row.name == copy_key.2
                                    })
                                });
                                if let Some(row) = row {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        row.value.clone(),
                                    ));
                                    drawer.notice =
                                        Some(Ok(format!("Cookie \"{}\" copied.", row.name)));
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .text_size(px(10.5))
                    .text_color(palette.dim)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(MONO)
                            .child(row.path.clone()),
                    )
                    .child(div().flex_none().child(expiry)),
            )
    }
}

impl Render for CookieDrawer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(owner) = self.owner.upgrade() else {
            return div();
        };
        let view = owner.read(cx);
        let palette = &view.palette;
        let mask = view.mask_secrets;
        let mut groups = BTreeMap::<&str, Vec<(usize, &CookieRow)>>::new();
        if let Some(snapshot) = &self.snapshot {
            for (index, row) in snapshot.rows.iter().enumerate() {
                groups.entry(&row.domain).or_default().push((index, row));
            }
        }
        let filtered = self.domain.is_some();
        let subtitle = match (&self.snapshot, &self.domain) {
            (Some(snapshot), None) => format!(
                "{} across {}",
                counted(snapshot.rows.len(), "cookie"),
                counted(groups.len(), "domain")
            ),
            (Some(_), Some(domain)) => format!(
                "{} for {domain}",
                counted(groups.get(domain.as_str()).map_or(0, Vec::len), "cookie"),
            ),
            (None, _) if self.busy => "Loading cookies…".to_owned(),
            (None, _) => "Cookies could not be loaded.".to_owned(),
        };
        let persistence_note = self.snapshot.as_ref().and_then(|snapshot| {
            snapshot.note.as_deref().or_else(|| {
                (!snapshot.persisting).then_some(
                    "Persist cookies is off. These cookies are cleared when the app closes.",
                )
            })
        });
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .text_color(palette.text)
            .text_size(px(12.))
            .child(
                div().flex().items_center().gap(px(6.))
                    .child(div().flex_1().text_color(palette.dim).child(subtitle))
                    .child(Button::new("cookie-refresh").small().outline().label("Refresh")
                        .disabled(self.busy).on_click(cx.listener(|drawer, _, _, cx| drawer.refresh(cx)))),
            )
            .children(persistence_note.map(|note| {
                div().p(px(8.)).rounded(px(5.)).bg(palette.raised)
                    .text_color(palette.warn).child(note.to_owned())
            }))
            .children(self.notice.as_ref().map(|notice| {
                let (message, color) = match notice {
                    Ok(message) => (message, palette.dim),
                    Err(message) => (message, palette.error),
                };
                div().text_color(color).child(message.clone())
            }))
            .children(self.domain.as_ref().map(|domain| {
                div().flex().items_center().gap(px(8.)).px(px(8.)).py(px(4.))
                    .bg(palette.raised).rounded(px(5.)).border_1().border_color(palette.line)
                    .child(div().text_color(palette.dim).child("Domain"))
                    .child(div().flex_1().min_w_0().truncate().font_family(MONO).child(domain.clone()))
                    .child(Button::new("cookie-filter-clear").ghost().small().tooltip("Show all domains")
                        .child(icon("x", palette.faint, 14.))
                        .on_click(cx.listener(|drawer, _, _, cx| {
                            drawer.domain = None;
                            cx.notify();
                        })))
            }))
            .children((self.snapshot.is_some() && groups.is_empty()).then(|| {
                div().px(px(8.)).py(px(24.)).text_color(palette.dim)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("No cookies yet."))
                    .child("Send a request to a site that sets cookies and they appear here, grouped by domain.")
            }))
            .children(
                groups.into_iter()
                    .filter(|(domain, _)| self.domain.as_deref().is_none_or(|selected| selected == *domain))
                    .map(|(domain, rows)| {
                        let domain = domain.to_owned();
                        let header_color = if filtered { palette.accent } else { palette.dim };
                        div()
                            .child(
                                div().id(gpui::SharedString::from(domain.clone()))
                                    .flex().items_center().gap(px(8.)).px(px(8.)).py(px(6.))
                                    .rounded(px(5.)).text_color(header_color).cursor_pointer()
                                    .hover(|style| style.bg(palette.hover))
                                    .child(icon("globe", header_color, 14.))
                                    .child(div().flex_1().min_w_0().truncate().font_family(MONO)
                                        .font_weight(FontWeight::SEMIBOLD).child(domain.clone()))
                                    .child(rows.len().to_string())
                                    .on_click(cx.listener(move |drawer, _, _, cx| {
                                        drawer.domain = (drawer.domain.as_ref() != Some(&domain)).then(|| domain.clone());
                                        cx.notify();
                                    })),
                            )
                            .children(rows.into_iter().map(|(index, row)| self.row(row, index, mask, palette, cx)))
                    }),
            )
            .child(
                div().flex().gap(px(8.)).py(px(6.))
                    .child(Button::new("cookie-delete-domain").danger().outline().small()
                        .label("Delete domain").disabled(!filtered || self.busy || self.snapshot.is_none())
                        .on_click(cx.listener(|drawer, _, window, cx| drawer.confirm_domain_delete(window, cx))))
                    .child(Button::new("cookie-show-all").outline().small().label("Show all")
                        .disabled(!filtered).on_click(cx.listener(|drawer, _, _, cx| {
                            drawer.domain = None;
                            cx.notify();
                        }))),
            )
    }
}

fn chip(label: &str, color: Rgba, palette: &Palette) -> Div {
    div()
        .flex_none()
        .rounded(px(10.))
        .border_1()
        .border_color(palette.line)
        .bg(palette.raised)
        .text_color(color)
        .text_size(px(10.5))
        .font_family(MONO)
        .px(px(5.))
        .child(label.to_owned())
}

fn counted(count: usize, unit: &str) -> String {
    let suffix = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{suffix}")
}
