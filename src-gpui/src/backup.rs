use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use crate::workspace::{self, Document};

pub fn export(document: &Document) -> Value {
    let mut payload = document.clone().into_snapshot();
    if let Some(workspaces) = payload["workspaces"].as_array_mut() {
        for workspace in workspaces {
            workspace["history"] = json!([]);
        }
    }
    json!({
        "kind": "postlite.backup", "version": 1, "payload": payload,
        "exportedAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    })
}

pub fn decode(backup: &Value) -> Result<Value, String> {
    let payload = &backup["payload"];
    if backup["kind"] != "postlite.backup" || backup["version"] != 1 || !payload.is_object() {
        return Err("Not a supported Postlite backup file.".into());
    }
    if let Some(workspaces) = payload["workspaces"].as_array() {
        if workspaces.is_empty() {
            return Err("The backup holds no workspace data.".into());
        }
    } else if ["collections", "environments", "tabs"]
        .iter()
        .all(|key| payload.get(*key).is_none())
    {
        return Err("The backup holds no workspace data.".into());
    }
    Ok(payload.clone())
}

pub fn replace(payload: Value) -> Result<Document, String> {
    workspace::parse(payload)
}

pub fn merge(document: &mut Document, mut payload: Value) -> Result<usize, String> {
    payload["settings"] = document.settings.clone();
    let imported = workspace::parse(payload)?;
    let count = imported.workspaces.len();
    let mut taken: HashSet<_> = document
        .workspaces
        .iter()
        .map(|workspace| workspace.id.clone())
        .collect();
    for mut imported in imported.workspaces {
        if !taken.insert(imported.id.clone()) {
            imported.id = unique(&mut taken);
        }
        let mut ids = HashMap::new();
        remap_nodes(&mut imported.collections, &mut taken, &mut ids);
        for tab in &mut imported.tabs {
            if tab["kind"] != "environment" {
                let id = tab["id"].as_str().unwrap_or_default().to_owned();
                let remapped = ids.entry(id).or_insert_with(|| unique(&mut taken));
                tab["id"] = json!(remapped);
            }
        }
        for history in &mut imported.history {
            if let Some(reference) = history["ref"].as_str() {
                history["ref"] = json!(ids.get(reference));
            }
            if let Some(id) = history["snapshot"]["id"].as_str() {
                let remapped = ids
                    .entry(id.to_owned())
                    .or_insert_with(|| unique(&mut taken));
                history["snapshot"]["id"] = json!(remapped);
            }
        }
        document.workspaces.push(imported);
    }
    Ok(count)
}

fn remap_nodes(
    items: &mut [Value],
    taken: &mut HashSet<String>,
    ids: &mut HashMap<String, String>,
) {
    for node in items {
        let original = node["id"].as_str().unwrap_or_default().to_owned();
        let id = unique(taken);
        if !original.is_empty() {
            ids.insert(original, id.clone());
        }
        node["id"] = json!(id);
        if let Some(children) = node["items"].as_array_mut() {
            remap_nodes(children, taken, ids);
        }
    }
}

fn unique(taken: &mut HashSet<String>) -> String {
    loop {
        let id = uuid::Uuid::new_v4().to_string();
        if taken.insert(id.clone()) {
            return id;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_keeps_environment_binding_and_secrets_but_excludes_history() {
        let document = workspace::parse(json!({
            "workspaces":[{"id":"w","name":"Test","history":[{"id":"h"}],
                "environments":[{"id":"e","variables":[{"key":"token","value":"keep","secret":true}]}],
                "collections":[{"id":"c","environmentId":"e","items":[{"id":"r","kind":"request"}]}],
                "tabs":[{"id":"r","kind":"request"},{"kind":"environment","envId":"e"}],
                "activeTabIndex":0}],
            "settings":{"request":{"proxy":{"mode":"manual","password":"remove"}}},
        })).unwrap();
        let file = export(&document);
        assert_eq!(file["payload"]["workspaces"][0]["history"], json!([]));
        assert!(!file.to_string().contains("remove"));
        let restored = replace(decode(&file).unwrap()).unwrap();
        assert_eq!(restored.workspaces[0].collections[0]["environmentId"], "e");
        assert_eq!(
            restored.workspaces[0].environments[0]["variables"][0]["value"],
            "keep"
        );
        assert_eq!(restored.workspaces[0].tabs.len(), 2);
    }

    #[test]
    fn merge_preserves_current_preferences_and_remaps_linked_request_tabs() {
        let payload = json!({"settings":{"general":{"theme":"light"}},"workspaces":[{
            "id":"default","collections":[{"id":"old-collection","items":[{"id":"old-request","kind":"request"}]}],
            "tabs":[{"id":"old-request","kind":"request"}],"activeTabIndex":0
        }]});
        let mut current = Document::default();
        current.settings["general"] = json!({"theme":"dark"});
        assert_eq!(merge(&mut current, payload), Ok(1));
        assert_eq!(current.settings["general"]["theme"], "dark");
        assert_eq!(current.active_workspace, 0);
        let imported = &current.workspaces[1];
        assert_ne!(imported.id, "default");
        assert_ne!(imported.collections[0]["id"], "old-collection");
        assert_ne!(imported.tabs[0]["id"], "old-request");
        assert_eq!(
            imported.tabs[0]["id"],
            imported.collections[0]["items"][0]["id"]
        );
    }
}
