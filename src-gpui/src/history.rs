use serde_json::{Value, json};

use crate::{http::Response, workspace::Workspace};

pub fn record(
    workspace: &mut Workspace,
    request: &Value,
    result: &Result<Response, String>,
) -> bool {
    let url = request["url"].as_str().unwrap_or_default();
    if url.trim().is_empty() {
        return false;
    }
    let reference = request["id"]
        .as_str()
        .filter(|id| workspace.find_request(id).is_some());
    let mut snapshot = if reference.is_none() {
        request.clone()
    } else {
        Value::Null
    };
    if snapshot.is_object() {
        snapshot["examples"] = json!([]);
    }
    let (status, status_text, elapsed) = result.as_ref().map_or((0, "", 0), |response| {
        (
            response.wire.status,
            response.wire.status_text.as_str(),
            response.wire.elapsed_ms as u64,
        )
    });
    let entry = json!({
        "id": uuid::Uuid::new_v4().to_string(), "at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "url": url, "method": request["method"].as_str().unwrap_or("GET"),
        "name": request["name"].as_str().unwrap_or_default(), "ref": reference,
        "ok": result.is_ok(), "status": status, "statusText": status_text,
        "elapsedMs": elapsed, "snapshot": snapshot, "runs": 1,
    });
    if let Some(first) = workspace.history.first_mut().filter(|first| {
        ["method", "url", "name", "ref"]
            .iter()
            .all(|key| first[*key] == entry[*key])
    }) {
        for field in ["at", "ok", "status", "statusText", "elapsedMs"] {
            first[field] = entry[field].clone();
        }
        first["runs"] = json!(first["runs"].as_u64().unwrap_or(1).saturating_add(1));
    } else {
        workspace.history.insert(0, entry);
        workspace.history.truncate(100);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_authored_url_and_coalesces_identical_sends() {
        let mut workspace = Workspace::default();
        let request =
            json!({"id":"r", "method":"GET", "url":"{{secret}}/path", "examples":[{"id":"x"}]});
        let result = Err("did not send".to_owned());
        assert!(record(&mut workspace, &request, &result));
        assert!(record(&mut workspace, &request, &result));
        assert_eq!(workspace.history.len(), 1);
        assert_eq!(workspace.history[0]["runs"], 2);
        assert_eq!(workspace.history[0]["url"], "{{secret}}/path");
        assert_eq!(workspace.history[0]["snapshot"]["examples"], json!([]));
    }
}
