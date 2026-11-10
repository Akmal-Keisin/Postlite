#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

#[derive(Deserialize, Serialize, Clone)]
struct HeaderPair {
    key: String,
    value: String,
}

#[derive(Deserialize)]
struct FormPair {
    key: String,
    value: String,
}

#[derive(Deserialize)]
struct RequestPayload {
    method: String,
    url: String,
    headers: Vec<HeaderPair>,
    body: Option<String>,
    #[serde(default)]
    form: Vec<FormPair>,
}

#[derive(Serialize)]
struct ResponsePayload {
    status: u16,
    status_text: String,
    headers: Vec<HeaderPair>,
    body: String,
    truncated: bool,
    elapsed_ms: u128,
    size_bytes: usize,
    error: Option<String>,
}

const MAX_BODY_BYTES: usize = 2_000_000;

#[tauri::command]
async fn http_send(request: RequestPayload) -> ResponsePayload {
    let empty = ResponsePayload {
        status: 0,
        status_text: String::new(),
        headers: Vec::new(),
        body: String::new(),
        truncated: false,
        elapsed_ms: 0,
        size_bytes: 0,
        error: None,
    };

    let method = match reqwest::Method::from_bytes(request.method.as_bytes()) {
        Ok(m) => m,
        Err(_) => return with_error(empty, format!("invalid method: {}", request.method)),
    };

    let client = match reqwest::Client::builder().timeout(Duration::from_secs(60)).build() {
        Ok(c) => c,
        Err(e) => return with_error(empty, e.to_string()),
    };

    let mut builder = client.request(method, &request.url);
    for header in request.headers {
        if header.key.trim().is_empty() {
            continue;
        }
        builder = builder.header(header.key, header.value);
    }
    if request.form.iter().any(|field| !field.key.trim().is_empty()) {
        let mut form = reqwest::multipart::Form::new();
        for field in request.form.iter().filter(|f| !f.key.trim().is_empty()) {
            form = form.text(field.key.clone(), field.value.clone());
        }
        builder = builder.multipart(form);
    } else if let Some(body) = request.body.filter(|b| !b.is_empty()) {
        builder = builder.body(body);
    }

    let started = Instant::now();
    let response = match builder.send().await {
        Ok(r) => r,
        Err(e) => return with_error(empty, e.to_string()),
    };

    let status = response.status();
    let headers = response
        .headers()
        .iter()
        .map(|(k, v)| HeaderPair {
            key: k.as_str().to_string(),
            value: v.to_str().unwrap_or("<binary>").to_string(),
        })
        .collect();

    let bytes = match response.bytes().await {
        Ok(b) => b,
        Err(e) => return with_error(empty, format!("failed reading body: {e}")),
    };

    let size_bytes = bytes.len();
    let truncated = size_bytes > MAX_BODY_BYTES;
    let slice = &bytes[..size_bytes.min(MAX_BODY_BYTES)];
    let body = String::from_utf8_lossy(slice).to_string();

    ResponsePayload {
        status: status.as_u16(),
        status_text: status.canonical_reason().unwrap_or_default().to_string(),
        headers,
        body,
        truncated,
        elapsed_ms: started.elapsed().as_millis(),
        size_bytes,
        error: None,
    }
}

fn with_error(mut response: ResponsePayload, error: String) -> ResponsePayload {
    response.error = Some(error);
    response
}

fn workspace_file(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("workspace.json"))
}

#[tauri::command]
fn load_workspace(app: AppHandle) -> Option<String> {
    workspace_file(&app)
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
}

#[tauri::command]
fn save_workspace(app: AppHandle, state: String) -> Result<(), String> {
    let path = workspace_file(&app)?;
    let staging = path.with_extension("json.staging");
    std::fs::write(&staging, state).map_err(|e| e.to_string())?;
    std::fs::rename(&staging, &path).map_err(|e| e.to_string())
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![http_send, load_workspace, save_workspace])
        .run(tauri::generate_context!())
        .expect("failed to run postlite");
}
