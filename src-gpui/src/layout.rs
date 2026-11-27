use super::Workbench;
use gpui::{
    Context, CursorStyle, Div, FocusHandle, InteractiveElement, MouseButton, ParentElement, Pixels,
    Stateful, Styled, Window, div, px,
};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Axis {
    Sidebar,
    Width,
    Height,
}

impl Axis {
    fn id(self) -> &'static str {
        match self {
            Self::Sidebar => "sidebar-divider",
            Self::Width => "split-divider",
            Self::Height => "stacked-divider",
        }
    }

    fn bounds(self, available: f32) -> (f32, f32, f32) {
        match self {
            Self::Sidebar => (180., 480., 276.),
            Self::Width => (380., (available - 408.).max(380.), available * 0.48),
            Self::Height => (120., (available - 128.).max(120.), available * 0.5),
        }
    }

    fn pointer(self, position: gpui::Point<Pixels>) -> f32 {
        f32::from(if self == Self::Height {
            position.y
        } else {
            position.x
        })
    }

    fn direction(self) -> f32 {
        if self == Self::Sidebar { 1. } else { -1. }
    }

    fn preferred(self, settings: &Value) -> Option<f32> {
        let value = match self {
            Self::Sidebar => &settings["sidebarWidth"],
            Self::Width => &settings["gpuiLayout"]["responseWidth"],
            Self::Height => &settings["gpuiLayout"]["responseHeight"],
        };
        value
            .as_f64()
            .filter(|value| *value > 0.)
            .map(|value| value as f32)
    }

    pub(super) fn write(self, settings: &mut Value, value: Option<f32>) {
        if self == Self::Sidebar {
            settings["sidebarWidth"] = json!(value.unwrap_or(276.));
            return;
        }
        if !settings["gpuiLayout"].is_object() {
            settings["gpuiLayout"] = json!({});
        }
        let key = if self == Self::Width {
            "responseWidth"
        } else {
            "responseHeight"
        };
        settings["gpuiLayout"][key] = json!(value);
    }
}

pub(super) struct Layout {
    sidebar: FocusHandle,
    width: FocusHandle,
    height: FocusHandle,
    drag: Option<Drag>,
}

#[derive(Clone, Copy)]
struct Drag {
    axis: Axis,
    pointer: f32,
    start: f32,
}

impl Layout {
    pub(super) fn new(cx: &mut Context<Workbench>) -> Self {
        Self {
            sidebar: cx.focus_handle(),
            width: cx.focus_handle(),
            height: cx.focus_handle(),
            drag: None,
        }
    }

    fn focus(&self, axis: Axis) -> &FocusHandle {
        match axis {
            Axis::Sidebar => &self.sidebar,
            Axis::Width => &self.width,
            Axis::Height => &self.height,
        }
    }
}

impl Workbench {
    pub(super) fn divider(
        &self,
        axis: Axis,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = &self.palette;
        let resize = if axis == Axis::Height {
            div().w_full().h(px(8.)).cursor(CursorStyle::ResizeUpDown)
        } else {
            let divider = div()
                .w(px(8.))
                .h_full()
                .cursor(CursorStyle::ResizeLeftRight);
            if axis == Axis::Sidebar {
                divider.mx(px(-4.)).relative()
            } else {
                divider
            }
        };
        let size = self.layout_size(axis, window);
        let focus = self.layout.focus(axis);
        let grip = if axis == Axis::Height {
            div().w_full().h(px(2.))
        } else {
            div().h_full().w(px(2.))
        };
        resize
            .id(axis.id())
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .track_focus(focus)
            .tab_index(0)
            .child(grip.bg(p.line))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    this.layout.focus(axis).focus(window);
                    if event.click_count == 2 {
                        axis.write(&mut this.doc.settings, None);
                        this.changed(cx);
                    } else {
                        this.layout.drag = Some(Drag {
                            axis,
                            pointer: axis.pointer(event.position),
                            start: size,
                        });
                    }
                }),
            )
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    let offset = match (axis, event.keystroke.key.as_str()) {
                        (Axis::Sidebar, "left")
                        | (Axis::Width, "right")
                        | (Axis::Height, "down") => -16.,
                        (Axis::Sidebar, "right") | (Axis::Width, "left") | (Axis::Height, "up") => {
                            16.
                        }
                        _ => return,
                    };
                    cx.stop_propagation();
                    this.update_layout_size(axis, this.layout_size(axis, window) + offset, window);
                    this.changed(cx);
                }),
            )
    }

    pub(super) fn layout_size(&self, axis: Axis, window: &Window) -> f32 {
        let (minimum, maximum, initial) = axis.bounds(self.layout_space(axis, window));
        axis.preferred(&self.doc.settings)
            .unwrap_or(initial)
            .clamp(minimum, maximum)
            .round()
    }

    fn layout_space(&self, axis: Axis, window: &Window) -> f32 {
        let viewport = window.viewport_size();
        let appearance = self.palette.appearance;
        match axis {
            Axis::Sidebar => f32::from(viewport.width),
            Axis::Width => {
                f32::from(viewport.width)
                    - 48.
                    - if self.rail_visible {
                        self.sidebar_width()
                    } else {
                        0.
                    }
            }
            Axis::Height => {
                f32::from(viewport.height)
                    - appearance.top
                    - appearance.status
                    - appearance.tab_strip
                    - self.address_height()
            }
        }
    }

    fn update_layout_size(&mut self, axis: Axis, wanted: f32, window: &Window) {
        let (minimum, maximum, _) = axis.bounds(self.layout_space(axis, window));
        axis.write(
            &mut self.doc.settings,
            Some(wanted.clamp(minimum, maximum).round()),
        );
    }

    pub(super) fn move_divider(
        &mut self,
        event: &gpui::MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.layout.drag else {
            return;
        };
        if !event.dragging() {
            self.finish_divider(cx);
            return;
        }
        let difference = drag.axis.pointer(event.position) - drag.pointer;
        self.update_layout_size(
            drag.axis,
            drag.start + difference * drag.axis.direction(),
            window,
        );
        cx.notify();
    }

    pub(super) fn finish_divider(&mut self, cx: &mut Context<Self>) {
        if self.layout.drag.take().is_some() {
            self.changed(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_axes_keep_separate_sizes_and_keep_room_for_the_request() {
        let mut settings = json!({});
        Axis::Width.write(&mut settings, Some(465.));
        Axis::Height.write(&mut settings, Some(210.));
        assert_eq!(Axis::Width.preferred(&settings), Some(465.));
        assert_eq!(Axis::Height.preferred(&settings), Some(210.));
        assert_eq!(Axis::Width.bounds(840.).1, 432.);
        assert_eq!(Axis::Height.bounds(490.).1, 362.);
        Axis::Width.write(&mut settings, None);
        assert_eq!(Axis::Width.preferred(&settings), None);
        assert_eq!(Axis::Height.preferred(&settings), Some(210.));
    }
}
