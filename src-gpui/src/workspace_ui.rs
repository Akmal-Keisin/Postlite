use super::{Rail, Workbench, name, val};
use gpui::{
    AppContext, Context, InteractiveElement, ParentElement, PathPromptOptions,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use gpui_component::{
    Sizable, WindowExt,
    button::{Button, ButtonVariants},
    input::{Input, InputState},
    menu::{PopupMenu, PopupMenuItem},
};
use serde_json::{Value, json};
use std::path::Path;

use crate::{postman, workspace::Workspace, workspace_edit};

#[derive(Clone)]
pub(super) enum NameAction {
    Workspace,
    WorkspaceRename(String),
    Collection,
    Environment,
    Folder(String),
    Rename(String),
    Variable {
        environment: String,
        index: usize,
        field: &'static str,
    },
}

impl Workbench {
    pub(super) fn ask_name(
        &mut self,
        title: &'static str,
        action: NameAction,
        initial: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let masked = if let NameAction::Variable {
            environment,
            index,
            field: "value",
        } = &action
        {
            self.variable_resource(environment)
                .is_some_and(|node| node["variables"][*index]["secret"] == true)
        } else {
            false
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(title)
                .masked(masked)
        });
        input.update(cx, |state, cx| {
            state.set_value(initial.to_owned(), window, cx)
        });
        let dialog_input = input.clone();
        let workbench = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let state = dialog_input.clone();
            let view = workbench.clone();
            let action = action.clone();
            dialog
                .title(title)
                .confirm()
                .child(Input::new(&dialog_input))
                .on_ok(move |_, window, cx| {
                    let value = state.read(cx).value().to_string();
                    let value = if matches!(action, NameAction::Variable { field: "value", .. }) {
                        value.as_str()
                    } else {
                        value.trim()
                    };
                    if value.is_empty() && !matches!(action, NameAction::Variable { .. }) {
                        return false;
                    }
                    view.update(cx, |this, cx| this.named(action.clone(), value, window, cx))
                        .is_ok()
                })
        });
        input.update(cx, |state, cx| state.focus(window, cx));
    }

    fn named(
        &mut self,
        action: NameAction,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            NameAction::Workspace => {
                let index = self.doc.workspaces.len();
                self.doc
                    .workspaces
                    .push(Workspace::new(uuid::Uuid::new_v4().to_string(), value));
                self.activate_workspace(index, window, cx);
            }
            NameAction::WorkspaceRename(id) => {
                if let Some(workspace) = self
                    .doc
                    .workspaces
                    .iter_mut()
                    .find(|workspace| workspace.id == id)
                {
                    workspace.name = value.to_owned();
                    self.changed(cx);
                }
            }
            NameAction::Collection => {
                let id = workspace_edit::create_collection(self.workspace_mut(), value);
                self.expanded.insert(id);
                self.active_rail = Rail::Collections;
                self.rail_visible = true;
                self.changed(cx);
            }
            NameAction::Environment => {
                let id = workspace_edit::create_environment(self.workspace_mut(), value);
                self.open_tab(json!({"kind":"environment","envId":id}), window, cx);
            }
            NameAction::Folder(parent) => {
                if let Some(id) =
                    workspace_edit::create_folder(self.workspace_mut(), &parent, value)
                {
                    self.expanded.extend([parent, id]);
                    self.changed(cx);
                }
            }
            NameAction::Rename(id) => {
                if workspace_edit::rename_node(self.workspace_mut(), &id, value) {
                    self.changed(cx);
                }
            }
            NameAction::Variable {
                environment,
                index,
                field,
            } => {
                if let Some(row) = self.environment_row(&environment, index) {
                    row[field] = json!(value);
                    self.changed(cx);
                }
            }
        }
    }

    pub(super) fn remove_resource(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.doc.settings["general"]["confirmBeforeDelete"] == false {
            self.delete_resource(&id, window, cx);
            return;
        }
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let view = view.clone();
            let id = id.clone();
            dialog
                .title("Delete item?")
                .confirm()
                .child("This removes the item from the workspace.")
                .on_ok(move |_, window, cx| {
                    let _ = view.update(cx, |this, cx| this.delete_resource(&id, window, cx));
                    true
                })
        });
    }

    fn delete_resource(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_url(cx);
        let workspace = self.workspace();
        let owners =
            super::oauth_ui::oauth_owners(&workspace.id, find_node(&workspace.collections, id));
        if workspace_edit::delete_node(self.workspace_mut(), id) {
            self.forget_oauth(owners, cx);
            self.expanded.remove(id);
            self.present_active(window, cx);
            self.changed(cx);
        }
    }

    fn variable_resource(&self, id: &str) -> Option<&Value> {
        let workspace = self.workspace();
        workspace
            .environments
            .iter()
            .find(|env| env["id"] == id)
            .or_else(|| find_node(&workspace.collections, id))
    }

    fn variable_resource_mut(&mut self, id: &str) -> Option<&mut Value> {
        let workspace = self.workspace_mut();
        workspace
            .environments
            .iter_mut()
            .find(|env| env["id"] == id)
            .or_else(|| workspace_edit::find_node_mut(&mut workspace.collections, id))
    }

    pub(super) fn environment_row(&mut self, id: &str, index: usize) -> Option<&mut Value> {
        self.variable_resource_mut(id)?["variables"]
            .as_array_mut()?
            .get_mut(index)
    }

    pub(super) fn add_variable(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(resource) = self.variable_resource_mut(&id) else {
            return;
        };
        if !resource["variables"].is_array() {
            resource["variables"] = json!([]);
        }
        if let Some(rows) = resource["variables"].as_array_mut() {
            let index = rows.len();
            rows.push(json!({"key":"","value":"","enabled":true,"secret":false}));
            self.changed(cx);
            self.ask_name(
                "Variable key",
                NameAction::Variable {
                    environment: id,
                    index,
                    field: "key",
                },
                "",
                window,
                cx,
            );
        }
    }

    pub(super) fn remove_variable(&mut self, id: &str, index: usize, cx: &mut Context<Self>) {
        if let Some(rows) = self
            .variable_resource_mut(id)
            .and_then(|node| node["variables"].as_array_mut())
            && index < rows.len()
        {
            rows.remove(index);
            self.changed(cx);
        }
    }

    fn variable_dialog(&self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let Some(view) = weak.upgrade() else {
                return dialog;
            };
            let state = view.read(cx);
            let Some(resource) = state.variable_resource(&id) else {
                return dialog;
            };
            let p = &state.palette;
            let mut table = div()
                .id("scope-variables")
                .max_h(px(380.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .px_2()
                        .py_1()
                        .child(div().flex_1().child("Key"))
                        .child(div().flex_1().child("Value"))
                        .child(div().w(px(152.)).child("Options")),
                );
            if let Some(rows) = resource["variables"].as_array() {
                for (index, row) in rows.iter().enumerate() {
                    let mut line = div()
                        .flex()
                        .items_center()
                        .px_2()
                        .py_1()
                        .border_b_1()
                        .border_color(p.line);
                    for (field, caption) in [("key", "Variable key"), ("value", "Variable value")] {
                        let actual = val(row, field).to_owned();
                        let shown = if field == "value"
                            && row["secret"] == true
                            && state.doc.settings["privacy"]["maskSecretVariables"] != false
                        {
                            "••••••••".to_owned()
                        } else if actual.is_empty() {
                            caption.to_owned()
                        } else {
                            actual.clone()
                        };
                        let identifier = id.clone();
                        let entity = weak.clone();
                        line = line.child(
                            div()
                                .id((field, index))
                                .flex_1()
                                .cursor_text()
                                .child(shown)
                                .on_click(move |_, window, cx| {
                                    let _ = entity.update(cx, |this, cx| {
                                        this.ask_name(
                                            caption,
                                            NameAction::Variable {
                                                environment: identifier.clone(),
                                                index,
                                                field,
                                            },
                                            &actual,
                                            window,
                                            cx,
                                        )
                                    });
                                }),
                        );
                    }
                    for (field, default, caption) in
                        [("enabled", true, "Enabled"), ("secret", false, "Secret")]
                    {
                        let checked = row[field].as_bool().unwrap_or(default);
                        let identifier = id.clone();
                        let entity = weak.clone();
                        line = line.child(
                            Button::new((field, index))
                                .ghost()
                                .small()
                                .label(format!("{} {caption}", if checked { "✓" } else { "○" }))
                                .on_click(move |_, _, cx| {
                                    let _ = entity.update(cx, |this, cx| {
                                        if let Some(row) = this.environment_row(&identifier, index)
                                        {
                                            row[field] = json!(!checked);
                                            this.changed(cx);
                                        }
                                    });
                                }),
                        );
                    }
                    let identifier = id.clone();
                    let entity = weak.clone();
                    table = table.child(
                        line.child(
                            Button::new(("delete-var", index))
                                .ghost()
                                .small()
                                .label("×")
                                .on_click(move |_, _, cx| {
                                    let _ = entity.update(cx, |this, cx| {
                                        this.remove_variable(&identifier, index, cx)
                                    });
                                }),
                        ),
                    );
                }
            }
            let entity = weak.clone();
            let resource_id = id.clone();
            dialog
                .width(px(685.))
                .title(format!("{} variables", name(resource, "Collection")))
                .alert()
                .child(table)
                .child(
                    Button::new("scope-add")
                        .ghost()
                        .small()
                        .label("+ Add variable")
                        .on_click(move |_, window, cx| {
                            let _ = entity.update(cx, |this, cx| {
                                this.add_variable(resource_id.clone(), window, cx)
                            });
                        }),
                )
        });
    }

    pub(super) fn confirm_workspace_delete(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let entity = weak.clone();
            let id = id.clone();
            dialog
                .title("Delete workspace?")
                .confirm()
                .child("This removes its collections, environments, tabs, and history.")
                .on_ok(move |_, window, cx| {
                    let _ = entity.update(cx, |this, cx| this.delete_workspace(&id, window, cx));
                    true
                })
        });
    }

    fn delete_workspace(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self
            .doc
            .workspaces
            .iter()
            .position(|workspace| workspace.id == id)
        else {
            return;
        };
        self.sync_url(cx);
        let old_active = self.doc.active_workspace;
        if index == old_active {
            self.dismiss_cookies(window, cx);
        }
        let removed = self.doc.workspaces.remove(index);
        self.forget_oauth(
            super::oauth_ui::oauth_owners(
                &removed.id,
                removed.collections.iter().chain(&removed.tabs),
            ),
            cx,
        );
        self.purge_cookies(removed.id, cx);
        self.results.retain(|(workspace, _), _| workspace != id);
        self.pending.retain(|(workspace, _), _| workspace != id);
        if self.doc.workspaces.is_empty() {
            self.doc.workspaces.push(Workspace::new(
                uuid::Uuid::new_v4().to_string(),
                "My Workspace",
            ));
        }
        self.doc.active_workspace = if index < old_active {
            old_active - 1
        } else {
            old_active.min(self.doc.workspaces.len() - 1)
        };
        self.expanded.clear();
        self.present_active(window, cx);
        self.changed(cx);
    }

    pub(super) fn clear_history(&self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let entity = weak.clone();
            dialog
                .title("Clear history?")
                .confirm()
                .child("This removes all request history for this workspace.")
                .on_ok(move |_, _, cx| {
                    let _ = entity.update(cx, |this, cx| {
                        this.workspace_mut().history.clear();
                        this.changed(cx);
                    });
                    true
                })
        });
    }

    pub(super) fn save_request_to(&mut self, id: &str, cx: &mut Context<Self>) {
        self.sync_url(cx);
        let Some(request) = self.active_request().cloned() else {
            return;
        };
        match workspace_edit::update_request(self.workspace_mut(), id, &request) {
            Ok(()) => {
                self.expanded.insert(id.to_owned());
                self.changed(cx);
                self.save_profile(cx);
            }
            Err(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }

    pub(super) fn import_postman(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Import Postman JSON".into()),
        });
        cx.spawn(async move |view, cx| {
            let result = match prompt.await {
                Ok(Ok(Some(paths))) => {
                    let task = cx.background_executor().spawn(async move {
                        paths
                            .iter()
                            .map(|path| import_path(path))
                            .collect::<Vec<_>>()
                    });
                    task.await
                }
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(error)) => vec![Err(format!("Could not open import dialog: {error}"))],
            };
            let _ = view.update(cx, |this, cx| {
                let mut changed = false;
                let mut reports = Vec::new();
                for outcome in result {
                    match outcome {
                        Ok(Import::Collection(import)) => {
                            if import.skipped_auth > 0 {
                                reports.push(format!(
                                    "Skipped {} requests with unsupported authentication",
                                    import.skipped_auth
                                ));
                            }
                            this.expanded
                                .insert(val(&import.collection, "id").to_owned());
                            this.workspace_mut().collections.push(import.collection);
                            changed = true;
                        }
                        Ok(Import::Environment(environment)) => {
                            this.workspace_mut().environments.push(environment);
                            changed = true;
                        }
                        Err(error) => reports.push(error),
                    }
                }
                this.error = (!reports.is_empty()).then(|| reports.join(" · "));
                if changed {
                    this.changed(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn export_resource(&mut self, id: &str, cx: &mut Context<Self>) {
        let workspace = self.workspace();
        let (label, document) = if let Some(environment) =
            workspace.environments.iter().find(|item| item["id"] == id)
        {
            (
                name(environment, "environment").to_owned(),
                postman::export_environment(environment),
            )
        } else if let Some(collection) = workspace.collections.iter().find(|item| item["id"] == id)
        {
            (
                name(collection, "collection").to_owned(),
                postman::export_collection(collection, self.bound_environment(collection)),
            )
        } else {
            return;
        };
        let mut file_name = label.replace(
            |character: char| character.is_control() || "/\\:*?\"<>|".contains(character),
            "_",
        );
        file_name.push_str(".json");
        self.save_document(document, &file_name, cx);
    }

    pub(super) fn resource_menu(
        id: String,
        label: String,
        folder: bool,
        export: bool,
        cx: &Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let view = cx.entity().downgrade();
        move |mut menu, _, _| {
            let identifier = id.clone();
            let text = label.clone();
            let target = view.clone();
            menu = menu.item(PopupMenuItem::new("Rename").on_click(move |_, window, cx| {
                let _ = target.update(cx, |this, cx| {
                    this.ask_name(
                        "Rename",
                        NameAction::Rename(identifier.clone()),
                        &text,
                        window,
                        cx,
                    )
                });
            }));
            if folder {
                let target = view.clone();
                let source = id.clone();
                menu = menu.item(PopupMenuItem::new("OAuth 2.0 settings").on_click(
                    move |_, window, cx| {
                        let _ = target.update(cx, |this, cx| {
                            if let Some(node) = find_node(&this.workspace().collections, &source) {
                                this.edit_parent_oauth(
                                    &source,
                                    &node["auth"]["oauth2"],
                                    window,
                                    cx,
                                );
                            }
                        });
                    },
                ));
                let entity = view.clone();
                let source = id.clone();
                menu = menu.item(
                    PopupMenuItem::new("Variables").on_click(move |_, window, cx| {
                        let _ = entity.update(cx, |this, cx| {
                            this.variable_dialog(source.clone(), window, cx)
                        });
                    }),
                );
                let target = view.clone();
                let parent = id.clone();
                menu = menu.item(PopupMenuItem::new("New folder").on_click(
                    move |_, window, cx| {
                        let _ = target.update(cx, |this, cx| {
                            this.ask_name(
                                "New folder",
                                NameAction::Folder(parent.clone()),
                                "",
                                window,
                                cx,
                            )
                        });
                    },
                ));
            }
            if export {
                let identifier = id.clone();
                let target = view.clone();
                menu = menu.item(PopupMenuItem::new("Export").on_click(move |_, _, cx| {
                    let _ = target.update(cx, |this, cx| this.export_resource(&identifier, cx));
                }));
            }
            let target = view.clone();
            let identifier = id.clone();
            menu.item(PopupMenuItem::new("Delete").on_click(move |_, window, cx| {
                let _ = target.update(cx, |this, cx| {
                    this.remove_resource(identifier.clone(), window, cx)
                });
            }))
        }
    }
}

enum Import {
    Collection(postman::CollectionImport),
    Environment(Value),
}

fn import_path(path: &Path) -> Result<Import, String> {
    let result = std::fs::read(path)
        .map_err(|error| error.to_string())
        .and_then(|data| serde_json::from_slice(&data).map_err(|error| error.to_string()))
        .and_then(|document| {
            if postman::is_collection(&document) {
                postman::import_collection(&document).map(Import::Collection)
            } else if postman::is_environment(&document) {
                postman::import_environment(&document).map(Import::Environment)
            } else {
                Err("Unsupported Postman JSON".into())
            }
        });
    result.map_err(|error| format!("{}: {error}", path.display()))
}

fn find_node<'a>(nodes: &'a [Value], id: &str) -> Option<&'a Value> {
    nodes.iter().find_map(|node| {
        if node["id"] == id {
            Some(node)
        } else {
            node["items"]
                .as_array()
                .and_then(|children| find_node(children, id))
        }
    })
}
