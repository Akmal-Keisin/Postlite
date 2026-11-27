//! The right-click menu for sidebar items. It is drawn here instead of with
//! gpui-component's popup so it can use a tight shadow that does not smear
//! over the rows behind it, plus a header, grouped actions, and icons.

use super::{NameAction, Workbench, workspace_ui::find_node};
use crate::appearance::{Palette, icon};
use gpui::{
    BoxShadow, Context, Div, FontWeight, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, Pixels, Point, Rgba, SharedString, StatefulInteractiveElement,
    Styled, Window, anchored, deferred, div, hsla, point, px,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MenuAction {
    Rename,
    NewFolder,
    Variables,
    OAuth,
    Export,
    Delete,
}

impl MenuAction {
    fn label(self) -> &'static str {
        match self {
            Self::Rename => "Rename",
            Self::NewFolder => "New folder",
            Self::Variables => "Variables",
            Self::OAuth => "OAuth 2.0 settings",
            Self::Export => "Export",
            Self::Delete => "Delete",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Rename => "pencil",
            Self::NewFolder => "folder-plus",
            Self::Variables => "braces",
            Self::OAuth => "gear",
            Self::Export => "export",
            Self::Delete => "trash",
        }
    }

    /// Actions in display order; `None` draws a divider between groups.
    pub(super) fn for_item(container: bool, export: bool) -> Vec<Option<Self>> {
        let mut actions = vec![Some(Self::Rename)];
        if container {
            actions.extend([
                Some(Self::NewFolder),
                None,
                Some(Self::Variables),
                Some(Self::OAuth),
            ]);
        }
        if export {
            actions.extend([None, Some(Self::Export)]);
        }
        actions.extend([None, Some(Self::Delete)]);
        actions
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MenuKind {
    Collection,
    Folder,
    Request,
    Environment,
}

impl MenuKind {
    fn glyph(self) -> &'static str {
        match self {
            Self::Collection => "stack",
            Self::Folder => "folder",
            Self::Request => "file-plus",
            Self::Environment => "globe",
        }
    }
}

pub(super) struct TreeMenu {
    pub id: String,
    pub label: String,
    pub kind: MenuKind,
    pub actions: Vec<Option<MenuAction>>,
    pub position: Point<Pixels>,
}

impl Workbench {
    /// Opens the item menu at the pointer. Bound to right-button presses on
    /// sidebar rows.
    pub(super) fn open_tree_menu(
        &mut self,
        id: String,
        label: String,
        kind: MenuKind,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let container = matches!(kind, MenuKind::Collection | MenuKind::Folder);
        let export = matches!(kind, MenuKind::Collection | MenuKind::Environment);
        self.tree_menu = Some(TreeMenu {
            id,
            label,
            kind,
            actions: MenuAction::for_item(container, export),
            position: event.position,
        });
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn close_tree_menu(&mut self, cx: &mut Context<Self>) {
        if self.tree_menu.take().is_some() {
            cx.notify();
        }
    }

    fn run_menu_action(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.tree_menu.take() else {
            return;
        };
        let id = menu.id;
        match action {
            MenuAction::Rename => {
                self.ask_name("Rename", NameAction::Rename(id), &menu.label, window, cx)
            }
            MenuAction::NewFolder => {
                self.expanded.insert(id.clone());
                self.ask_name("New folder", NameAction::Folder(id), "", window, cx)
            }
            MenuAction::Variables => self.variable_dialog(id, window, cx),
            MenuAction::OAuth => {
                if let Some(node) = find_node(&self.workspace().collections, &id) {
                    let config = node["auth"]["oauth2"].clone();
                    self.edit_parent_oauth(&id, &config, window, cx);
                }
            }
            MenuAction::Export => self.export_resource(&id, cx),
            MenuAction::Delete => self.remove_resource(id, window, cx),
        }
        cx.notify();
    }

    /// The open menu in a top layer, or nothing.
    pub(super) fn tree_menu_layer(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let menu = self.tree_menu.as_ref()?;
        let p = &self.palette;
        let opened_at = menu.position;
        let mut list = div()
            .id("tree-menu")
            // Keep presses on the menu from reaching the rows underneath.
            .occlude()
            .w(px(224.))
            .flex()
            .flex_col()
            .p(px(4.))
            .bg(p.panel)
            .border_1()
            .border_color(p.line)
            .rounded(px(p.appearance.radius.clamp(2., 10.)))
            .shadow(menu_shadow(p))
            .text_size(px(13.))
            .text_color(p.text)
            .font_family(p.appearance.body_font.family())
            // A press anywhere else closes the menu. A right press that just
            // opened a menu on another row arrives here too; keep that one.
            .on_mouse_down_out(cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                let reopened = this
                    .tree_menu
                    .as_ref()
                    .is_some_and(|menu| menu.position != opened_at);
                if !reopened {
                    this.close_tree_menu(cx);
                }
            }))
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(menu_header(menu, p))
            .child(divider(p));
        for (index, action) in menu.actions.iter().enumerate() {
            list = match action {
                None => list.child(divider(p)),
                Some(action) => list.child(menu_item(*action, index, p, cx)),
            };
        }
        Some(
            deferred(
                anchored()
                    .position(menu.position + point(px(2.), px(2.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(list),
            )
            .with_priority(2),
        )
    }
}

/// A close, soft shadow: enough to lift the menu off the sidebar without the
/// wide dark halo of a large blur.
fn menu_shadow(p: &Palette) -> Vec<BoxShadow> {
    let strength = if p.dark { 0.42 } else { 0.14 };
    vec![
        BoxShadow {
            color: hsla(0., 0., 0., strength),
            offset: point(px(0.), px(6.)),
            blur_radius: px(16.),
            spread_radius: px(-6.),
        },
        BoxShadow {
            color: hsla(0., 0., 0., strength * 0.5),
            offset: point(px(0.), px(1.)),
            blur_radius: px(3.),
            spread_radius: px(0.),
        },
    ]
}

fn menu_header(menu: &TreeMenu, p: &Palette) -> Div {
    let kind = match menu.kind {
        MenuKind::Collection => "Collection",
        MenuKind::Folder => "Folder",
        MenuKind::Request => "Request",
        MenuKind::Environment => "Environment",
    };
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(8.))
        .pt(px(4.))
        .pb(px(6.))
        .child(icon(menu.kind.glyph(), p.accent, 14.))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(SharedString::from(menu.label.clone())),
                )
                .child(div().text_size(px(11.)).text_color(p.faint).child(kind)),
        )
}

fn divider(p: &Palette) -> Div {
    div().h(px(1.)).mx(px(4.)).my(px(4.)).bg(p.soft_line)
}

fn menu_item(
    action: MenuAction,
    index: usize,
    p: &Palette,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let danger = action == MenuAction::Delete;
    let (ink, glyph_ink, hover) = if danger {
        (p.error, p.error, Rgba { a: 0.12, ..p.error })
    } else {
        (p.text, p.faint, p.hover)
    };
    div()
        .id(("tree-menu-item", index))
        .flex()
        .items_center()
        .gap(px(10.))
        .h(px(30.))
        .px(px(8.))
        .rounded(px(p.appearance.radius.clamp(2., 6.)))
        .cursor_pointer()
        .text_color(ink)
        .hover(move |style| style.bg(hover))
        .child(icon(action.glyph(), glyph_ink, 14.))
        .child(action.label())
        .on_click(cx.listener(move |this, _, window, cx| this.run_menu_action(action, window, cx)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menus_group_actions_and_end_with_delete() {
        use MenuAction::*;
        assert_eq!(
            MenuAction::for_item(true, true),
            [
                Some(Rename),
                Some(NewFolder),
                None,
                Some(Variables),
                Some(OAuth),
                None,
                Some(Export),
                None,
                Some(Delete)
            ]
        );
        assert_eq!(
            MenuAction::for_item(false, false),
            [Some(Rename), None, Some(Delete)]
        );
    }
}
