//! Fixed keyboard shortcuts, shared by the workbench key handler and the
//! Shortcuts page in application settings so the two never drift apart.

use super::{NameAction, Rail, Workbench, items, save_destinations, val};
use gpui::{Context, Keystroke, ParentElement, Styled, Window, div, px};
use gpui_component::{
    WindowExt,
    button::{Button, ButtonVariants},
};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shortcut {
    NewRequest,
    Send,
    Save,
    NewCollection,
    SearchCollections,
    Settings,
    PreviousTab,
    NextTab,
    CloseTab,
    MoveUp,
    MoveDown,
}

/// Every shortcut with its description and key caps, in the order the
/// Shortcuts page lists them.
pub(super) const SHORTCUTS: [(Shortcut, &str, &[&str]); 11] = [
    (Shortcut::NewRequest, "New request", &["Ctrl", "N"]),
    (Shortcut::Send, "Send request", &["Ctrl", "Enter"]),
    (Shortcut::Save, "Save request to collection", &["Ctrl", "S"]),
    (
        Shortcut::NewCollection,
        "New collection",
        &["Ctrl", "Shift", "N"],
    ),
    (
        Shortcut::SearchCollections,
        "Search collections",
        &["Ctrl", "K"],
    ),
    (
        Shortcut::Settings,
        "Open application settings",
        &["Ctrl", ","],
    ),
    (
        Shortcut::PreviousTab,
        "Previous request tab",
        &["Ctrl", "Page Up"],
    ),
    (
        Shortcut::NextTab,
        "Next request tab",
        &["Ctrl", "Page Down"],
    ),
    (Shortcut::CloseTab, "Close active tab", &["Ctrl", "W"]),
    (
        Shortcut::MoveUp,
        "Move the open request up in its folder",
        &["Alt", "↑"],
    ),
    (
        Shortcut::MoveDown,
        "Move the open request down in its folder",
        &["Alt", "↓"],
    ),
];

impl Shortcut {
    /// Maps a keystroke to a shortcut. `Ctrl` means the platform's primary
    /// modifier, so the same table works with `Cmd` on macOS.
    pub(super) fn from_keystroke(keystroke: &Keystroke) -> Option<Self> {
        let modifiers = &keystroke.modifiers;
        let primary = modifiers.secondary();
        let key = keystroke.key.as_str();
        if modifiers.alt && !primary && !modifiers.shift {
            return match key {
                "up" => Some(Self::MoveUp),
                "down" => Some(Self::MoveDown),
                _ => None,
            };
        }
        if !primary || modifiers.alt {
            return None;
        }
        Some(match (key, modifiers.shift) {
            ("n", true) => Self::NewCollection,
            ("n", false) => Self::NewRequest,
            ("enter", _) => Self::Send,
            ("s", false) => Self::Save,
            ("k", false) => Self::SearchCollections,
            (",", false) => Self::Settings,
            ("pageup", false) => Self::PreviousTab,
            ("pagedown", false) => Self::NextTab,
            ("w", false) => Self::CloseTab,
            _ => return None,
        })
    }
}

impl Workbench {
    pub(super) fn run_shortcut(
        &mut self,
        shortcut: Shortcut,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match shortcut {
            Shortcut::NewRequest => self.new_request(window, cx),
            Shortcut::Send => self.send(window, cx),
            Shortcut::Save => self.save_shortcut(window, cx),
            Shortcut::NewCollection => {
                self.ask_name("New collection", NameAction::Collection, "", window, cx)
            }
            Shortcut::SearchCollections => {
                if self.active_rail != Rail::Collections || !self.rail_visible {
                    self.switch_rail(Rail::Collections, window, cx);
                    self.rail_visible = true;
                }
                self.search.update(cx, |input, cx| input.focus(window, cx));
            }
            Shortcut::Settings => self.open_settings(window, cx),
            Shortcut::PreviousTab | Shortcut::NextTab => {
                let count = self.workspace().tabs.len();
                if let (Some(active), true) = (self.workspace().active_tab, count > 1) {
                    let next = if shortcut == Shortcut::NextTab {
                        (active + 1) % count
                    } else {
                        (active + count - 1) % count
                    };
                    self.select_tab(next, window, cx);
                }
            }
            Shortcut::CloseTab => {
                if let Some(active) = self.workspace().active_tab {
                    self.close_tab(active, window, cx);
                }
            }
            Shortcut::MoveUp | Shortcut::MoveDown => {
                let Some(id) = self.active_request().map(|tab| val(tab, "id").to_owned()) else {
                    return;
                };
                let up = shortcut == Shortcut::MoveUp;
                if move_sibling(&mut self.workspace_mut().collections, &id, up) {
                    self.changed(cx);
                }
            }
        }
    }

    /// Saves into the request's own collection, or asks where to save a new
    /// request, matching the Save button.
    fn save_shortcut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_request().is_none() {
            return;
        }
        if let Some(collection) = self.active_collection() {
            let id = val(collection, "id").to_owned();
            self.save_request_to(&id, cx);
            return;
        }
        let mut destinations = Vec::new();
        save_destinations(&self.workspace().collections, "", &mut destinations);
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let mut list = div().flex().flex_col().gap(px(6.));
            for (index, (id, label)) in destinations.iter().enumerate() {
                let (view, id) = (view.clone(), id.clone());
                list = list.child(
                    Button::new(("save-destination", index))
                        .outline()
                        .w_full()
                        .label(label.clone())
                        .on_click(move |_, window, cx| {
                            let _ = view.update(cx, |this, cx| this.save_request_to(&id, cx));
                            window.close_dialog(cx);
                        }),
                );
            }
            let view = view.clone();
            list = list.child(
                Button::new("save-new-collection")
                    .ghost()
                    .w_full()
                    .label("New collection…")
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        let _ = view.update(cx, |this, cx| {
                            this.ask_name("New collection", NameAction::Collection, "", window, cx)
                        });
                    }),
            );
            dialog.title("Save request").width(px(420.)).child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child("Store the current tab in a collection or folder.")
                    .child(list),
            )
        });
    }
}

/// Swaps the node with `id` and its previous or next sibling, searching every
/// level of the collection tree. Returns whether anything moved.
fn move_sibling(nodes: &mut [Value], id: &str, up: bool) -> bool {
    if let Some(index) = nodes.iter().position(|node| val(node, "id") == id) {
        let target = if up {
            index.checked_sub(1)
        } else {
            Some(index + 1).filter(|&next| next < nodes.len())
        };
        return target.is_some_and(|target| {
            nodes.swap(index, target);
            true
        });
    }
    nodes.iter_mut().any(|node| {
        items(node).is_some()
            && node["items"]
                .as_array_mut()
                .is_some_and(|children| move_sibling(children, id, up))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Modifiers;
    use serde_json::json;

    fn press(key: &str, modifiers: Modifiers) -> Option<Shortcut> {
        Shortcut::from_keystroke(&Keystroke {
            modifiers,
            key: key.into(),
            key_char: None,
        })
    }

    #[test]
    fn keystrokes_map_to_the_documented_shortcuts() {
        let primary = Modifiers::secondary_key();
        let shifted = Modifiers {
            shift: true,
            ..primary
        };
        assert_eq!(press("n", primary), Some(Shortcut::NewRequest));
        assert_eq!(press("n", shifted), Some(Shortcut::NewCollection));
        assert_eq!(press("enter", primary), Some(Shortcut::Send));
        assert_eq!(press("pagedown", primary), Some(Shortcut::NextTab));
        assert_eq!(press("up", Modifiers::alt()), Some(Shortcut::MoveUp));
        assert_eq!(press("n", Modifiers::none()), None);
        assert_eq!(press("s", shifted), None);
    }

    #[test]
    fn moving_a_request_stays_inside_its_folder() {
        let mut tree = vec![json!({"id": "c", "items": [
            {"id": "a"}, {"id": "f", "items": [{"id": "x"}, {"id": "y"}]}
        ]})];
        assert!(move_sibling(&mut tree, "y", true));
        assert_eq!(tree[0]["items"][1]["items"][0]["id"], "y");
        assert!(!move_sibling(&mut tree, "y", true));
        assert!(!move_sibling(&mut tree, "f", false));
        assert!(move_sibling(&mut tree, "f", true));
        assert_eq!(tree[0]["items"][0]["id"], "f");
    }
}
