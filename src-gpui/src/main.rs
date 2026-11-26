#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod http;

use gpui::{
    App, AppContext, Application, Bounds, ClipboardItem, Context, Entity, ParentElement, Render,
    Styled, Subscription, TitlebarOptions, Window, WindowBounds, WindowOptions, div, px, rgb, size,
};
use gpui_component::{
    Disableable, Root, Theme, ThemeMode,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
};

const BACKGROUND: u32 = 0x1b1815;
const PANEL: u32 = 0x251f1b;
const LINE: u32 = 0x483c33;
const TEXT: u32 = 0xfff3e5;
const MUTED: u32 = 0xc1afa0;
const EMBER: u32 = 0xff8964;
const GREEN: u32 = 0x9dd6a8;

struct Workbench {
    url: Entity<InputState>,
    body: Option<Entity<InputState>>,
    result: Option<Result<http::ResponseInfo, String>>,
    sending: bool,
    _enter: Subscription,
}

impl Workbench {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("https://api.example.com"));
        let enter = cx.subscribe_in(&url, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.send(window, cx);
            }
        });
        url.update(cx, |input, cx| input.focus(window, cx));
        Self {
            url,
            body: None,
            result: None,
            sending: false,
            _enter: enter,
        }
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }
        let url = self.url.read(cx).value().trim().to_owned();
        self.sending = true;
        cx.notify();

        let request = cx
            .background_executor()
            .spawn(async move { http::get(&url) });
        cx.spawn_in(window, async move |this, cx| {
            let result = request.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.sending = false;
                this.result = Some(result.map(|response| {
                    let body = this.body.get_or_insert_with(|| {
                        cx.new(|cx| {
                            InputState::new(window, cx)
                                .multi_line(true)
                                .soft_wrap(false)
                        })
                    });
                    body.update(cx, |input, cx| {
                        input.set_value(response.body, window, cx);
                    });
                    response.info
                }));
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for Workbench {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let mut response = div().flex_1().min_h_0().overflow_hidden();
        let mut heading = div().flex().items_center().gap_3().child("Response");
        match (self.sending, &self.result, &self.body) {
            (true, _, _) => {
                response = response
                    .p_4()
                    .text_color(rgb(MUTED))
                    .child("Sending request…")
            }
            (false, Some(Err(error)), _) => {
                response = response.p_4().text_color(rgb(EMBER)).child(error.clone());
            }
            (false, Some(Ok(result)), Some(body)) => {
                let status_color = if result.status.is_success() {
                    GREEN
                } else {
                    EMBER
                };
                heading = heading
                    .child(
                        div()
                            .text_color(rgb(status_color))
                            .child(result.status.to_string()),
                    )
                    .child(div().text_color(rgb(MUTED)).child(result.details.clone()));
                let body = body.clone();
                response =
                    response.child(Input::new(&body).h_full().appearance(false).disabled(true));
                heading = heading.child(div().flex_1()).child(
                    Button::new("copy-response")
                        .label("Copy")
                        .ghost()
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                body.read(cx).value().to_string(),
                            ));
                        }),
                );
            }
            _ => {
                response = response
                    .p_4()
                    .text_color(rgb(MUTED))
                    .child("Send a request to see its response.")
            }
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BACKGROUND))
            .text_color(rgb(TEXT))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_5()
                    .py_4()
                    .border_b_1()
                    .border_color(rgb(LINE))
                    .bg(rgb(PANEL))
                    .child(div().text_color(rgb(EMBER)).child("◆"))
                    .child(div().text_lg().child("Postlite")),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .p_5()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .bg(rgb(PANEL))
                                    .text_color(rgb(GREEN))
                                    .text_sm()
                                    .child("GET"),
                            )
                            .child(div().flex_1().min_w_0().child(Input::new(&self.url)))
                            .child(
                                Button::new("send")
                                    .label("Send")
                                    .primary()
                                    .disabled(self.sending)
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.send(window, cx)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .overflow_hidden()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(LINE))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .min_h(px(44.))
                                    .px_4()
                                    .border_b_1()
                                    .border_color(rgb(LINE))
                                    .bg(rgb(PANEL))
                                    .text_sm()
                                    .child(heading.w_full()),
                            )
                            .child(response),
                    ),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        gpui_component::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        let theme = Theme::global_mut(cx);
        theme.primary = rgb(EMBER).into();
        theme.primary_foreground = rgb(BACKGROUND).into();

        let bounds = Bounds::centered(None, size(px(1240.), px(820.)), cx);
        cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("Postlite GPUI".into()),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(520.), px(360.))),
                ..Default::default()
            },
            |window, cx| {
                let workbench = cx.new(|cx| Workbench::new(window, cx));
                cx.new(|cx| Root::new(workbench, window, cx))
            },
        )
        .expect("could not open the Postlite window");
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
