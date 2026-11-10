#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CACHE_CONTROL, PRAGMA};
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
    #[serde(default)]
    settings: Option<HttpSettings>,
}

#[derive(Deserialize)]
#[serde(default)]
struct HttpSettings {
    // Signed inputs let negative values return the same response errors as other invalid ranges.
    request_timeout_ms: i64,
    follow_redirects: bool,
    max_redirects: i64,
    ssl_verify: bool,
    disable_cache: bool,
    max_response_size_kb: i64,
}

impl Default for HttpSettings {
    fn default() -> Self {
        Self {
            request_timeout_ms: 60_000,
            follow_redirects: true,
            max_redirects: 10,
            ssl_verify: true,
            disable_cache: false,
            max_response_size_kb: 2_000,
        }
    }
}

impl HttpSettings {
    fn validate(&self) -> Result<(), String> {
        if !(1..=3_600_000).contains(&self.request_timeout_ms) {
            return Err("request_timeout_ms must be between 1 and 3600000".into());
        }
        if !(0..=100).contains(&self.max_redirects) {
            return Err("max_redirects must be between 0 and 100".into());
        }
        if !(1..=102_400).contains(&self.max_response_size_kb) {
            return Err("max_response_size_kb must be between 1 and 102400".into());
        }
        Ok(())
    }
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

    let has_settings = request.settings.is_some();
    let settings = request.settings.unwrap_or_default();
    if let Err(error) = settings.validate() {
        return with_error(empty, error);
    }
    // Older callers used a decimal byte cap, not the settings block's binary KB.
    let max_body_bytes = if has_settings {
        settings.max_response_size_kb as usize * 1024
    } else {
        MAX_BODY_BYTES
    };

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_millis(settings.request_timeout_ms as u64))
        .redirect(if settings.follow_redirects {
            reqwest::redirect::Policy::limited(settings.max_redirects as usize)
        } else {
            reqwest::redirect::Policy::none()
        })
        .danger_accept_invalid_certs(!settings.ssl_verify)
        .build()
    {
        Ok(c) => c,
        Err(e) => return with_error(empty, e.to_string()),
    };

    let mut headers = HeaderMap::new();
    for header in request.headers {
        if header.key.trim().is_empty() {
            continue;
        }
        let name = match HeaderName::from_bytes(header.key.as_bytes()) {
            Ok(name) => name,
            Err(e) => return with_error(empty, e.to_string()),
        };
        let value = match HeaderValue::from_str(&header.value) {
            Ok(value) => value,
            Err(e) => return with_error(empty, e.to_string()),
        };
        headers.append(name, value);
    }
    if settings.disable_cache {
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));
    }
    let mut builder = client.request(method, &request.url).headers(headers);
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
    let truncated = size_bytes > max_body_bytes;
    let slice = &bytes[..size_bytes.min(max_body_bytes)];
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn request() -> RequestPayload {
        serde_json::from_value(json!({
            "method": "GET", "url": "http://127.0.0.1:1/", "headers": []
        }))
        .unwrap()
    }

    // Loopback only; the test binary never starts Tauri or loads a workspace.
    fn serve(response: String) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut received = Vec::new();
            let mut buffer = [0; 1024];
            while !received.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).unwrap();
                assert_ne!(count, 0, "connection closed before request headers");
                received.extend_from_slice(&buffer[..count]);
            }
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8(received).unwrap()
        });
        (url, handle)
    }

    #[test]
    fn missing_and_empty_settings_use_expected_defaults() {
        assert!(request().settings.is_none());
        let settings: HttpSettings = serde_json::from_value(json!({})).unwrap();
        assert_eq!(settings.request_timeout_ms, 60_000);
        assert!(settings.follow_redirects);
        assert_eq!(settings.max_redirects, 10);
        assert!(settings.ssl_verify);
        assert!(!settings.disable_cache);
        assert_eq!(settings.max_response_size_kb, 2_000);
    }

    #[test]
    fn snake_case_settings_override_defaults_independently() {
        let partial: HttpSettings =
            serde_json::from_value(json!({"disable_cache": true})).unwrap();
        assert!(partial.disable_cache);
        assert_eq!(partial.request_timeout_ms, 60_000);
        assert!(partial.follow_redirects);
        assert_eq!(partial.max_redirects, 10);
        assert!(partial.ssl_verify);
        assert_eq!(partial.max_response_size_kb, 2_000);

        let settings: HttpSettings = serde_json::from_value(json!({
            "request_timeout_ms": 1234,
            "follow_redirects": false,
            "max_redirects": 0,
            "ssl_verify": false,
            "disable_cache": true,
            "max_response_size_kb": 1
        }))
        .unwrap();
        assert_eq!(settings.request_timeout_ms, 1234);
        assert!(!settings.follow_redirects);
        assert_eq!(settings.max_redirects, 0);
        assert!(!settings.ssl_verify);
        assert!(settings.disable_cache);
        assert_eq!(settings.max_response_size_kb, 1);
    }

    #[test]
    fn numeric_range_endpoints_are_valid() {
        for (timeout, redirects, size) in [(1, 0, 1), (3_600_000, 100, 102_400)] {
            let settings = HttpSettings {
                request_timeout_ms: timeout,
                max_redirects: redirects,
                max_response_size_kb: size,
                ..HttpSettings::default()
            };
            assert!(settings.validate().is_ok());
        }
    }

    #[tokio::test]
    async fn invalid_ranges_return_response_errors_before_sending() {
        for (field, values) in [
            ("request_timeout_ms", vec![-1, 0, 3_600_001, i64::MAX]),
            ("max_redirects", vec![-1, 101, i64::MAX]),
            ("max_response_size_kb", vec![-1, 0, 102_401, i64::MAX]),
        ] {
            for value in values {
                let mut payload = request();
                payload.settings = Some(
                    serde_json::from_value(json!({(field): value, "follow_redirects": false}))
                        .unwrap(),
                );
                let response = http_send(payload).await;
                assert_eq!(response.status, 0);
                assert!(response.error.unwrap().starts_with(field), "{field}={value}");
                assert_eq!(response.size_bytes, 0);
                assert!(response.body.is_empty());
            }
        }
    }

    #[tokio::test]
    async fn response_caps_preserve_full_size_and_legacy_bytes() {
        for (settings, size, cap) in [
            (None, 2_049_000, 2_000_000),
            (Some(serde_json::Value::Null), 2_049_000, 2_000_000),
            (Some(json!({})), 2_049_000, 2_048_000),
            (Some(json!({"max_response_size_kb": 1})), 1024, 1024),
            (Some(json!({"max_response_size_kb": 1})), 1025, 1024),
        ] {
            let (url, server) = serve(format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n{}",
                "x".repeat(size)
            ));
            let mut payload = json!({"method": "GET", "url": url, "headers": []});
            if let Some(settings) = settings {
                payload["settings"] = settings;
            }
            let response = http_send(serde_json::from_value(payload).unwrap()).await;
            assert!(response.error.is_none(), "{:?}", response.error);
            assert_eq!(response.status, 200);
            assert_eq!(response.size_bytes, size);
            assert_eq!(response.body.len(), cap);
            assert_eq!(response.truncated, size > cap);
            server.join().unwrap();
        }
    }

    #[tokio::test]
    async fn disable_cache_replaces_all_existing_values_only_when_enabled() {
        for disable_cache in [false, true] {
            let (url, server) =
                serve("HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".into());
            let mut payload = request();
            payload.url = url;
            payload.settings = Some(HttpSettings {
                disable_cache,
                ..HttpSettings::default()
            });
            payload.headers = [
                ("Cache-Control", "max-age=600"),
                ("cache-control", "public"),
                ("Pragma", "cache"),
                ("PRAGMA", "custom"),
                ("X-Test", "one"),
                ("x-test", "two"),
            ]
            .into_iter()
            .map(|(key, value)| HeaderPair {
                key: key.into(),
                value: value.into(),
            })
            .collect();
            let response = http_send(payload).await;
            assert!(response.error.is_none(), "{:?}", response.error);
            let received = server.join().unwrap().to_ascii_lowercase();
            let values = |name: &str| -> Vec<&str> {
                received
                    .lines()
                    .filter_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        (key == name).then_some(value.trim())
                    })
                    .collect()
            };
            if disable_cache {
                assert_eq!(values("cache-control"), ["no-cache"]);
                assert_eq!(values("pragma"), ["no-cache"]);
            } else {
                assert_eq!(values("cache-control"), ["max-age=600", "public"]);
                assert_eq!(values("pragma"), ["cache", "custom"]);
            }
            assert_eq!(values("x-test"), ["one", "two"]);
        }
    }

    #[tokio::test]
    async fn redirects_respect_follow_flag_and_zero_limit() {
        for follow_redirects in [false, true] {
            let (url, server) = serve(
                "HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
            );
            let mut payload = request();
            payload.url = url;
            payload.settings = Some(HttpSettings {
                follow_redirects,
                max_redirects: 0,
                ..HttpSettings::default()
            });
            let response = http_send(payload).await;
            if follow_redirects {
                assert_eq!(response.status, 0);
                assert!(response.error.unwrap().contains("redirect"));
            } else {
                assert_eq!(response.status, 302);
                assert!(response.error.is_none());
            }
            server.join().unwrap();
        }
    }

    #[tokio::test]
    async fn request_timeout_applies_while_waiting_for_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut payload = request();
        payload.url = format!("http://{}/", listener.local_addr().unwrap());
        payload.settings = Some(HttpSettings {
            request_timeout_ms: 50,
            ..HttpSettings::default()
        });
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            // Keep the socket open without sending a response until the client times out.
            wait.recv_timeout(Duration::from_secs(5)).unwrap();
            drop(stream);
        });
        let response = http_send(payload).await;
        release.send(()).unwrap();
        server.join().unwrap();
        assert_eq!(response.status, 0);
        assert!(response.error.is_some());
    }

    #[tokio::test]
    async fn default_and_custom_policies_follow_redirects() {
        for settings in [
            None,
            Some(HttpSettings {
                max_redirects: 1,
                ssl_verify: false,
                ..HttpSettings::default()
            }),
        ] {
            let (destination, final_server) = serve(
                "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".into(),
            );
            let (url, redirect_server) = serve(format!(
                "HTTP/1.1 302 Found\r\nLocation: {destination}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            ));
            let mut payload = request();
            payload.url = url;
            payload.settings = settings;
            let response = http_send(payload).await;
            assert!(response.error.is_none(), "{:?}", response.error);
            assert_eq!(response.status, 200);
            assert_eq!(response.body, "ok");
            redirect_server.join().unwrap();
            final_server.join().unwrap();
        }
    }
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![http_send, load_workspace, save_workspace])
        .run(tauri::generate_context!())
        .expect("failed to run postlite");
}
