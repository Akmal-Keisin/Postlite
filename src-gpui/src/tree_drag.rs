//! Drag-and-drop ordering in the collection tree. Any row (collection, folder,
//! or request) moves among its own siblings, as in the Tauri app; an accent
//! rule marks where it will land. Alt+Up/Down is the keyboard equivalent.

use super::{Workbench, items, val};
use crate::appearance::{Palette, icon, mono};
use gpui::{
    AppContext, Context, InteractiveElement, IntoElement, ParentElement, Render, Stateful,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use serde_json::Value;

#[derive(Clone, Debug)]
pub(super) struct TreeDrag {
    pub id: String,
    /// Id of the containing collection or folder; empty for a collection.
    pub parent: String,
    pub index: usize,
    pub label: String,
    pub method: Option<String>,
}

/// The floating chip that follows the pointer while dragging.
struct DragGhost {
    drag: TreeDrag,
    palette: Palette,
}

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let p = &self.palette;
        let mut chip = div()
            .flex()
            .items_center()
            .gap(px(7.))
            .px(px(10.))
            .h(px(28.))
            .rounded(px(p.appearance.row_radius.clamp(4., 8.)))
            .border_1()
            .border_color(p.accent)
            .bg(p.raised)
            .shadow_md()
            .text_size(px(12.))
            .text_color(p.text)
            .opacity(0.95);
        chip = match &self.drag.method {
            Some(method) => chip.child(
                div()
                    .font_family(mono())
                    .text_size(px(10.5))
                    .text_color(p.method(method))
                    .child(method.clone()),
            ),
            None => chip.child(icon(
                if self.drag.parent.is_empty() {
                    "stack"
                } else {
                    "folder"
                },
                p.accent,
                13.,
            )),
        };
        chip.child(self.drag.label.clone())
    }
}

impl Workbench {
    pub(super) fn draggable_row(
        &self,
        row: Stateful<gpui::Div>,
        drag: TreeDrag,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui::Div> {
        let accent = self.palette.accent;
        let (id, parent, index) = (drag.id.clone(), drag.parent.clone(), drag.index);
        let target_parent = parent.clone();
        let palette = self.palette.clone();
        row.on_drag(drag, move |drag, _, _, cx| {
            let ghost = DragGhost {
                drag: drag.clone(),
                palette: palette.clone(),
            };
            cx.new(|_| ghost)
        })
        .drag_over::<TreeDrag>(move |style, dragged, _, _| {
            if dragged.parent != parent || dragged.id == id {
                return style;
            }
            // Moving down lands after this row, moving up lands before it.
            if dragged.index < index {
                style.border_b_2().border_color(accent)
            } else {
                style.border_t_2().border_color(accent)
            }
        })
        .on_drop(cx.listener(move |this, dragged: &TreeDrag, _, cx| {
            if dragged.parent == target_parent && dragged.index != index {
                this.move_tree_item(&dragged.id, index, cx);
            }
        }))
    }

    fn move_tree_item(&mut self, id: &str, to: usize, cx: &mut Context<Self>) {
        if move_within(&mut self.workspace_mut().collections, id, to) {
            self.changed(cx);
        }
    }
}

/// Moves the node with `id` to position `to` among its siblings, at whatever
/// depth it lives. Returns whether the tree changed.
pub(super) fn move_within(nodes: &mut Vec<Value>, id: &str, to: usize) -> bool {
    if let Some(from) = nodes.iter().position(|node| val(node, "id") == id) {
        if from == to || to >= nodes.len() {
            return false;
        }
        let node = nodes.remove(from);
        nodes.insert(to, node);
        return true;
    }
    nodes.iter_mut().any(|node| {
        items(node).is_some()
            && node["items"]
                .as_array_mut()
                .is_some_and(|children| move_within(children, id, to))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ids(nodes: &[Value]) -> Vec<&str> {
        nodes.iter().map(|node| val(node, "id")).collect()
    }

    #[test]
    fn dropping_moves_a_row_to_the_target_position_among_its_siblings() {
        let mut tree = vec![
            json!({"id": "c1", "items": [{"id": "a"}, {"id": "b"}, {"id": "c"}]}),
            json!({"id": "c2", "items": []}),
        ];
        assert!(move_within(&mut tree, "a", 2));
        assert_eq!(ids(tree[0]["items"].as_array().unwrap()), ["b", "c", "a"]);
        assert!(move_within(&mut tree, "a", 0));
        assert_eq!(ids(tree[0]["items"].as_array().unwrap()), ["a", "b", "c"]);
        assert!(move_within(&mut tree, "c2", 0));
        assert_eq!(ids(&tree), ["c2", "c1"]);
        assert!(!move_within(&mut tree, "b", 1));
        assert!(!move_within(&mut tree, "b", 9));
        assert!(!move_within(&mut tree, "missing", 0));
    }
}
