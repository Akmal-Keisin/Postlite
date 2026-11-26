use serde_json::{Value, json};
use uuid::Uuid;

use crate::workspace::Workspace;

pub fn create_collection(workspace: &mut Workspace, name: &str) -> String {
    let id = Uuid::new_v4().to_string();
    workspace.collections.push(json!({
        "id": id, "name": name.trim(), "items": [], "auth": {"type":"noauth"},
        "variables": [], "scripts": {"pre":"", "test":""}, "environmentId": null, "favorite": false,
    }));
    id
}

pub fn create_environment(workspace: &mut Workspace, name: &str) -> String {
    let id = Uuid::new_v4().to_string();
    workspace
        .environments
        .push(json!({"id":id,"name":name.trim(),"variables":[]}));
    id
}

pub fn create_folder(workspace: &mut Workspace, parent_id: &str, name: &str) -> Option<String> {
    let items = children_mut(find_node_mut(&mut workspace.collections, parent_id)?)?;
    let id = Uuid::new_v4().to_string();
    items.push(json!({
        "kind":"folder", "id":id, "name":name.trim(), "items":[],
        "auth":{"type":"noauth"}, "variables":[], "scripts":{"pre":"","test":""},
    }));
    Some(id)
}

pub fn update_request(
    workspace: &mut Workspace,
    collection_id: &str,
    request: &Value,
) -> Result<(), String> {
    let request = request
        .as_object()
        .ok_or("Request data must be a JSON object")?;
    let id = request
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .ok_or("The request has no ID")?;
    if request
        .get("kind")
        .is_some_and(|kind| !kind.is_null() && kind != "request")
    {
        return Err("Only request items can be saved in a collection".to_owned());
    }
    let mut updated = request.clone();
    updated.insert("kind".to_owned(), json!("request"));

    if let Some(saved) = find_node_mut(&mut workspace.collections, id) {
        if saved["kind"] != "request" {
            return Err("The request ID already belongs to another item".to_owned());
        }
        if updated.get("examples").is_none_or(Value::is_null) {
            updated.insert(
                "examples".to_owned(),
                saved.get("examples").cloned().unwrap_or_else(|| json!([])),
            );
        }
        let object = saved
            .as_object_mut()
            .ok_or("Saved request data was unreadable")?;
        object.extend(updated);
    } else {
        let items = find_node_mut(&mut workspace.collections, collection_id)
            .and_then(children_mut)
            .ok_or("Choose a collection or folder to save the request")?;
        items.push(Value::Object(updated));
    }
    Ok(())
}

pub fn rename_node(workspace: &mut Workspace, id: &str, name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() {
        return false;
    }
    let node = find_node_mut(&mut workspace.collections, id).or_else(|| {
        workspace
            .environments
            .iter_mut()
            .find(|environment| has_id(environment, id))
    });
    let Some(node) = node else { return false };
    let is_request = node["kind"] == "request";
    node["name"] = json!(name);
    if is_request {
        for tab in workspace
            .tabs
            .iter_mut()
            .filter(|tab| tab["kind"] != "environment" && has_id(tab, id))
        {
            tab["name"] = json!(name);
        }
    }
    true
}

pub fn delete_node(workspace: &mut Workspace, id: &str) -> bool {
    if remove_node(&mut workspace.collections, id) {
        return true;
    }
    let old_len = workspace.environments.len();
    workspace
        .environments
        .retain(|environment| !has_id(environment, id));
    if workspace.environments.len() == old_len {
        return false;
    }
    for collection in workspace
        .collections
        .iter_mut()
        .filter(|collection| collection["environmentId"] == id)
    {
        collection["environmentId"] = Value::Null;
    }
    let active = workspace.active_tab;
    let mut retained_active = None;
    let mut old_index = 0;
    let mut new_index = 0;
    workspace.tabs.retain(|tab| {
        let keep = tab["kind"] != "environment" || tab["envId"] != id;
        if keep {
            if active == Some(old_index) {
                retained_active = Some(new_index);
            }
            new_index += 1;
        }
        old_index += 1;
        keep
    });
    workspace.active_tab = retained_active.or_else(|| workspace.tabs.len().checked_sub(1));
    true
}

fn has_id(node: &Value, id: &str) -> bool {
    node["id"].as_str() == Some(id)
}

fn children_mut(node: &mut Value) -> Option<&mut Vec<Value>> {
    match node["kind"].as_str() {
        Some("request" | "environment") => None,
        _ => node.get_mut("items")?.as_array_mut(),
    }
}

pub(crate) fn find_node_mut<'a>(nodes: &'a mut [Value], id: &str) -> Option<&'a mut Value> {
    for node in nodes {
        if has_id(node, id) {
            return Some(node);
        }
        if let Some(child) = children_mut(node).and_then(|children| find_node_mut(children, id)) {
            return Some(child);
        }
    }
    None
}

fn remove_node(nodes: &mut Vec<Value>, id: &str) -> bool {
    if let Some(index) = nodes.iter().position(|node| has_id(node, id)) {
        nodes.remove(index);
        true
    } else {
        nodes
            .iter_mut()
            .filter_map(children_mut)
            .any(|children| remove_node(children, id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn creates_unique_collections_environments_and_nested_folders() {
        let mut workspace = Workspace::default();
        let collection = create_collection(&mut workspace, "  API ");
        let environment = create_environment(&mut workspace, "Local");
        let folder = create_folder(&mut workspace, &collection, "Reports").unwrap();
        let nested = create_folder(&mut workspace, &folder, "2026").unwrap();

        assert_eq!(workspace.collections[0]["name"], "API");
        assert_eq!(workspace.environments[0]["name"], "Local");
        assert_eq!(
            workspace.collections[0]["items"][0]["items"][0]["id"],
            nested
        );
        assert_eq!(
            workspace.collections[0]["items"][0]["items"][0]["auth"]["type"],
            "noauth"
        );
        let ids = [&collection, &environment, &folder, &nested];
        assert_eq!(
            ids.into_iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            4
        );
        assert!(
            ids.into_iter()
                .all(|id| uuid::Uuid::parse_str(id.as_str()).is_ok())
        );
        assert!(create_folder(&mut workspace, "missing", "Invalid").is_none());
    }

    #[test]
    fn saves_existing_request_in_place_and_preserves_saved_examples() {
        let mut workspace = Workspace::default();
        workspace.collections = vec![json!({"id":"collection", "items":[
            {"id":"folder", "kind":"folder", "items":[
                {"id":"saved", "kind":"request", "url":"old", "examples":[{"id":"a"}],"plugin":"preserved"}
            ]}
        ]})];
        let updated = json!({"id":"saved", "url":"new", "examples":null});
        update_request(&mut workspace, "unrelated", &updated).unwrap();
        let found = workspace.find_request("saved").unwrap();
        assert_eq!(found.ancestors[0]["id"], "folder");
        assert_eq!(found.request["url"], "new");
        assert_eq!(found.request["plugin"], "preserved");
        assert_eq!(found.request["examples"], json!([{"id":"a"}]));

        update_request(
            &mut workspace,
            "collection",
            &json!({"id":"saved", "examples":[]}),
        )
        .unwrap();
        assert_eq!(
            workspace.find_request("saved").unwrap().request["examples"],
            json!([])
        );
    }

    #[test]
    fn adds_new_request_only_to_a_valid_container() {
        let mut workspace = Workspace::default();
        workspace.collections = vec![json!({"id":"collection", "items":[
            {"id":"folder", "kind":"folder", "items":[]}
        ]})];
        update_request(
            &mut workspace,
            "folder",
            &json!({"id":"scratch","url":"http://example.test"}),
        )
        .unwrap();
        let saved = workspace.find_request("scratch").unwrap();
        assert_eq!(saved.ancestors[0]["id"], "folder");
        assert_eq!(saved.request["kind"], "request");
        assert!(update_request(&mut workspace, "scratch", &json!({"id":"wrong"})).is_err());
        assert!(
            update_request(
                &mut workspace,
                "folder",
                &json!({"id":"wrong","kind":"environment"})
            )
            .is_err()
        );
        assert!(update_request(&mut workspace, "folder", &json!({"url":"missing ID"})).is_err());
        assert!(workspace.find_request("wrong").is_none());
    }

    #[test]
    fn renaming_request_updates_open_tabs_but_deleting_tree_node_keeps_them() {
        let mut workspace = Workspace::default();
        let request = json!({"id":"request","kind":"request","name":"Before"});
        workspace.collections = vec![json!({"id":"collection","items":[request.clone()]})];
        workspace.tabs.push(request);
        workspace.active_tab = Some(0);

        assert!(rename_node(&mut workspace, "request", "After"));
        assert!(!rename_node(&mut workspace, "request", "   "));
        assert_eq!(workspace.tabs[0]["name"], "After");
        assert_eq!(
            workspace.find_request("request").unwrap().request["name"],
            "After"
        );
        assert!(delete_node(&mut workspace, "request"));
        assert!(workspace.find_request("request").is_none());
        assert_eq!(workspace.tabs[0]["name"], "After");
    }

    #[test]
    fn deleting_an_environment_clears_binding_and_reselects_a_surviving_tab() {
        let mut workspace = Workspace::default();
        workspace.environments = vec![json!({"id":"prod"})];
        workspace.collections = vec![json!({"id":"collection","environmentId":"prod","items":[]})];
        workspace.tabs = vec![
            json!({"kind":"environment","envId":"prod"}),
            json!({"kind":"request","id":"first"}),
            json!({"kind":"request","id":"second"}),
        ];
        workspace.active_tab = Some(0);

        assert!(delete_node(&mut workspace, "prod"));
        assert!(workspace.environments.is_empty());
        assert!(workspace.collections[0]["environmentId"].is_null());
        assert_eq!(workspace.tabs.len(), 2);
        assert_eq!(workspace.active_tab, Some(1));
        assert_eq!(workspace.tabs[1]["id"], "second");
    }
}
