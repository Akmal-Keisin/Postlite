use super::Workbench;
use crate::appearance::icon;
use gpui::{Context, IntoElement, ParentElement, Styled, div, px};
use gpui_component::{
    Sizable,
    button::Button,
    menu::{DropdownMenu, PopupMenuItem},
};
use regex::Regex;
use serde_json::Value;
use std::{collections::HashSet, sync::LazyLock};

static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{([^{}]+?)\}\}").expect("literal variable regex"));

impl Workbench {
    pub(super) fn variable_peek(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let live = self.url.read(cx).value();
        let request = self.active_request()?;
        let workspace = self.workspace();
        let located = request["id"]
            .as_str()
            .and_then(|id| workspace.find_request(id));
        let mut scopes = Vec::new();
        if let Some(item) = located {
            scopes.push((
                super::name(item.collection, "Collection").to_owned(),
                item.collection,
            ));
            if let Some(env) = self.bound_environment(item.collection) {
                scopes.push((
                    format!("Environment: {}", super::name(env, "Environment")),
                    env,
                ));
            }
            for ancestor in item.ancestors {
                scopes.push((super::name(ancestor, "Folder").to_owned(), ancestor));
            }
        }
        let mask = self.doc.settings["privacy"]["maskSecretVariables"] != false;
        let mut used = HashSet::new();
        let entries: Vec<_> = PLACEHOLDER
            .captures_iter(&live)
            .filter_map(|captures| {
                let key = captures.get(1)?.as_str().trim();
                if key.is_empty() || !used.insert(key) {
                    return None;
                }
                let resolved = scopes.iter().rev().find_map(|(label, node)| {
                    let row = node["variables"].as_array()?.iter().rev().find(|row| {
                        row["enabled"] != false
                            && row["key"].as_str().is_some_and(|name| name.trim() == key)
                    })?;
                    let value = if mask && row["secret"] == true {
                        "••••••••".into()
                    } else {
                        display(&row["value"])
                    };
                    Some(format!("{key}   {value}   ({label})"))
                });
                Some(resolved.unwrap_or_else(|| {
                    if matches!(key, "$guid" | "$timestamp" | "$isoTimestamp" | "$randomInt") {
                        format!("{key}   (dynamic at send time)")
                    } else {
                        format!("{key}   (unresolved)")
                    }
                }))
            })
            .collect();
        if entries.is_empty() {
            return None;
        }
        let p = &self.palette;
        let count = entries.len();
        Some(
            Button::new("variable-peek")
                .outline()
                .small()
                .h(px(22.))
                .mr(px(5.))
                .text_color(p.cyan)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .text_size(px(11.))
                        .child(icon("braces", p.cyan, 13.))
                        .child(count.to_string()),
                )
                .dropdown_menu(move |mut menu, _, _| {
                    for item in &entries {
                        menu = menu.item(PopupMenuItem::label(item.clone()));
                    }
                    menu
                }),
        )
    }
}

fn display(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "\"\"".into(),
        value => value.to_string(),
    }
}
