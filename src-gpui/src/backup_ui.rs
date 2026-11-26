use super::Workbench;
use crate::backup;
use gpui::{Context, ParentElement, PathPromptOptions, Styled, Window, div, px};
use gpui_component::{
    WindowExt,
    button::{Button, ButtonVariants},
};
use serde_json::Value;

impl Workbench {
    pub(super) fn request_backup_export(&self, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let entity = entity.clone();
            dialog.title("Export full backup").confirm()
                .child("The backup includes your workspace data and any saved tokens or secret variables. Store it securely.")
                .on_ok(move |_, _, cx| {
                    let _ = entity.update(cx, |view, cx| view.export_backup(cx));
                    true
                })
        });
    }

    fn export_backup(&mut self, cx: &mut Context<Self>) {
        self.sync_url(cx);
        let data = backup::export(&self.doc);
        let filename = format!(
            "postlite.backup.{}.json",
            chrono::Utc::now().format("%Y-%m-%d")
        );
        self.save_document(data, &filename, cx);
    }

    pub(super) fn save_document(&mut self, document: Value, name: &str, cx: &mut Context<Self>) {
        let location = std::env::current_dir().unwrap_or_default();
        let picker = cx.prompt_for_new_path(&location, Some(name));
        cx.spawn(async move |entity, cx| {
            let result = match picker.await {
                Ok(Ok(Some(path))) => {
                    cx.background_executor()
                        .spawn(async move {
                            let data = serde_json::to_vec_pretty(&document)
                                .map_err(|error| error.to_string())?;
                            std::fs::write(&path, data).map_err(|error| {
                                format!("Could not export {}: {error}", path.display())
                            })
                        })
                        .await
                }
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(error)) => Err(format!("Could not open Save As: {error}")),
            };
            let _ = entity.update(cx, |view, cx| {
                view.error = result.err();
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn request_backup_restore(&self, window: &mut Window, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose a Postlite backup".into()),
        });
        cx.spawn_in(window, async move |entity, cx| {
            let result = match picker.await {
                Ok(Ok(Some(mut paths))) => {
                    let Some(path) = paths.pop() else {
                        return;
                    };
                    cx.background_executor()
                        .spawn(async move {
                            let bytes = std::fs::read(&path).map_err(|error| {
                                format!("Could not read {}: {error}", path.display())
                            })?;
                            let backup = serde_json::from_slice(&bytes)
                                .map_err(|error| format!("Invalid backup JSON: {error}"))?;
                            backup::decode(&backup)
                        })
                        .await
                }
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(error)) => Err(format!("Could not open backup chooser: {error}")),
            };
            let _ = entity.update_in(cx, |view, window, cx| match result {
                Ok(payload) => view.restore_dialog(payload, window, cx),
                Err(error) => {
                    view.error = Some(error);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn restore_dialog(&self, payload: Value, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity().downgrade();
        let count = payload["workspaces"].as_array().map_or(1, Vec::len);
        window.open_dialog(cx, move |dialog, _, _| {
            let merge = entity.clone();
            let replace = entity.clone();
            let merge_payload = payload.clone();
            let replace_payload = payload.clone();
            dialog.title("Restore from backup").width(px(465.))
                .child(format!("The backup contains {count} workspace(s). Merge adds them alongside your existing workspaces."))
                .child(div().flex().justify_end().gap(px(10.)).child(
                    Button::new("backup-replace").outline().label("Replace everything").on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        let payload = replace_payload.clone();
                        let entity = replace.clone();
                        window.open_dialog(cx, move |dialog, _, _| {
                            let payload = payload.clone();
                            let entity = entity.clone();
                            dialog.title("Replace everything?").confirm()
                                .child("This replaces the current GPUI workspace data and preferences with the backup.")
                                .on_ok(move |_, window, cx| {
                                    let _ = entity.update(cx, |view, cx| view.restore(payload.clone(), true, window, cx));
                                    true
                                })
                        });
                    })
                ).child(
                    Button::new("backup-merge").primary().label("Merge").on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        let _ = merge.update(cx, |view, cx| view.restore(merge_payload.clone(), false, window, cx));
                    })
                ))
        });
    }

    fn restore(
        &mut self,
        payload: Value,
        replace: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_url(cx);
        let result = if replace {
            backup::replace(payload).map(|document| {
                self.dismiss_cookies(window, cx);
                let owners = self
                    .doc
                    .workspaces
                    .iter()
                    .flat_map(|workspace| {
                        super::oauth_ui::oauth_owners(
                            &workspace.id,
                            workspace.collections.iter().chain(&workspace.tabs),
                        )
                    })
                    .collect();
                self.forget_oauth(owners, cx);
                self.doc = document;
                self.sync_cookie_preference(cx);
                self.document_generation = uuid::Uuid::new_v4();
                self.results.clear();
                self.pending.clear();
                self.expanded.clear();
                self.palette = super::settings_ui::theme(&self.doc.settings, window, cx);
                self.mask_secrets = self.doc.settings["privacy"]["maskSensitiveHeaders"] != false;
                self.present_active(window, cx);
            })
        } else {
            backup::merge(&mut self.doc, payload).map(|_| ())
        };
        match result {
            Ok(()) => {
                self.changed(cx);
                self.save_profile(cx);
            }
            Err(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }
}
