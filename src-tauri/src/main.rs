#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use postlite_core::session::{
    self, CookieFilter, CookieSnapshot, JarManager, OAuth2Config, OAuthCapture, PostliteJar,
    RedirectBegin, Secrets, TokenOut,
};
use postlite_core::{
    b64_decode, run_request, system_env, ProxyCredentials, RequestContext, RequestPayload,
    ResponsePayload,
};
use tauri::{AppHandle, Manager};

type Ctx = RequestContext<PostliteJar>;

#[tauri::command]
async fn http_send(app: AppHandle, request: RequestPayload) -> ResponsePayload {
    let workspace_id = request
        .settings
        .as_ref()
        .map(|settings| settings.workspace_id.clone())
        .unwrap_or_default();
    // Credentials come from the global encrypted store, never from the request JSON. A store that
    // cannot be read degrades to "no auth" rather than blocking the send.
    let proxy_credentials = app.state::<Secrets>().proxy_credentials().ok().flatten();
    let ctx = Ctx {
        jar: app.state::<JarManager>().jar(&workspace_id),
        proxy_credentials,
        env: system_env(),
    };
    run_request(request, &ctx).await
}

#[tauri::command]
fn cookie_list(app: AppHandle, workspace_id: String) -> Result<CookieSnapshot, String> {
    app.state::<JarManager>().snapshot(&workspace_id)
}

#[tauri::command]
fn cookie_delete(app: AppHandle, workspace_id: String, filter: CookieFilter) -> Result<usize, String> {
    app.state::<JarManager>().remove(&workspace_id, &filter)
}

#[tauri::command]
fn cookie_purge(app: AppHandle, workspace_id: String) -> Result<(), String> {
    app.state::<JarManager>().purge(&workspace_id)
}

#[tauri::command]
fn cookie_set_persist(app: AppHandle, on: bool) -> Result<(), String> {
    app.state::<JarManager>().set_persist(on);
    Ok(())
}

#[tauri::command]
fn proxy_credentials_set(app: AppHandle, username: String, password: String) -> Result<(), String> {
    app.state::<Secrets>()
        .set_proxy_credentials(&ProxyCredentials { username, password })
}

#[tauri::command]
fn proxy_credentials_get(app: AppHandle) -> Result<Option<ProxyCredentials>, String> {
    app.state::<Secrets>().proxy_credentials()
}

#[tauri::command]
async fn oauth2_token(app: AppHandle, config: OAuth2Config) -> Result<TokenOut, String> {
    app.state::<Secrets>().exchange(&config).await
}

#[tauri::command]
fn oauth2_secret_set(app: AppHandle, node_id: String, secret: String) -> Result<(), String> {
    app.state::<Secrets>()
        .set_oauth_secret(&node_id, Some(&secret))
}

#[tauri::command]
fn oauth2_secret_get(app: AppHandle, node_id: String) -> Result<Option<String>, String> {
    app.state::<Secrets>().oauth_secret(&node_id)
}

#[tauri::command]
fn oauth2_secret_clear(app: AppHandle, node_id: String) -> Result<(), String> {
    app.state::<Secrets>().set_oauth_secret(&node_id, None)
}

#[tauri::command]
fn oauth2_authorize_begin(app: AppHandle, port: u16) -> Result<RedirectBegin, String> {
    app.state::<OAuthCapture>().begin(port)
}

#[tauri::command]
async fn oauth2_authorize_await(
    app: AppHandle,
    capture_id: String,
    expected_state: String,
) -> Result<(String, String), String> {
    app.state::<OAuthCapture>()
        .await_redirect(&capture_id, &expected_state)
        .await
}

#[tauri::command]
fn oauth2_authorize_cancel(app: AppHandle, capture_id: String) -> Result<(), String> {
    app.state::<OAuthCapture>().cancel(&capture_id);
    Ok(())
}

#[tauri::command]
fn open_in_browser(url: String) -> Result<(), String> {
    session::open_in_browser(&url)
}

// The user picks the destination; a bare typed name gets the filter's extension
// because native save dialogs do not consistently append it themselves.
fn export_path(chosen: PathBuf, extensions: &[String]) -> PathBuf {
    let mut path = chosen;
    if path.extension().is_none() {
        if let Some(ext) = extensions.first() {
            path.set_extension(ext);
        }
    }
    path
}


#[tauri::command]
async fn pick_file() -> Result<Option<String>, String> {
    let Some(handle) = rfd::AsyncFileDialog::new()
        .set_title("Select file to upload")
        .pick_file()
        .await
    else {
        return Ok(None);
    };
    Ok(Some(handle.path().to_string_lossy().into_owned()))
}

#[tauri::command]
async fn save_export_file(
    default_name: String,
    filter_name: String,
    extensions: Vec<String>,
    contents: String,
    contents_b64: Option<String>,
) -> Result<Option<String>, String> {
    let mut dialog = rfd::AsyncFileDialog::new()
        .set_title("Export from postlite")
        .set_file_name(&default_name);
    if !extensions.is_empty() {
        dialog = dialog.add_filter(&filter_name, &extensions);
    }
    let Some(handle) = dialog.save_file().await else {
        return Ok(None);
    };
    let path = export_path(handle.path().to_path_buf(), &extensions);
    // Binary saves arrive base64-encoded (response bodies that are not valid
    // UTF-8); everything else is written as text exactly as handed over.
    let payload: Vec<u8> = match contents_b64.as_deref() {
        Some(b64) => b64_decode(b64)?,
        None => contents.into_bytes(),
    };
    let target = path.clone();
    tauri::async_runtime::spawn_blocking(move || std::fs::write(&target, payload))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("could not write the export: {e}"))?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Remove `username`/`password` from every `proxy` object in a workspace save (§8), leaving request
/// auth and everything else untouched. Returns the original string when there was nothing to strip,
/// so a save without proxy credentials stays byte-for-byte what the caller sent.
fn strip_proxy_credentials(state: &str) -> String {
    let mut value: serde_json::Value = match serde_json::from_str(state) {
        Ok(value) => value,
        Err(_) => return state.to_owned(),
    };
    if strip_creds(&mut value) {
        serde_json::to_string(&value).unwrap_or_else(|_| state.to_owned())
    } else {
        state.to_owned()
    }
}

fn strip_creds(value: &mut serde_json::Value) -> bool {
    let mut removed = false;
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if key == "proxy" {
                    if let serde_json::Value::Object(proxy) = child {
                        removed |= proxy.remove("username").is_some();
                        removed |= proxy.remove("password").is_some();
                    }
                }
                removed |= strip_creds(child);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                removed |= strip_creds(item);
            }
        }
        _ => {}
    }
    removed
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
    // Proxy credentials belong to the global store, never to the portable workspace file (§8).
    let cleaned = strip_proxy_credentials(&state);
    std::fs::write(&staging, cleaned).map_err(|e| e.to_string())?;
    std::fs::rename(&staging, &path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use postlite_core::{
        b64_encode, file_part_meta, resolve_proxy, EnvReader, FormPair, HeaderPair, HttpSettings,
        ProxySettings,
    };
    use reqwest::header::HeaderValue;
    use serde_json::json;
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    fn request() -> RequestPayload {
        serde_json::from_value(json!({
            "method": "GET", "url": "http://127.0.0.1:1/", "headers": []
        }))
        .unwrap()
    }

    // Loopback only; the test binary never starts Tauri or loads a workspace.
    // Accepts text or raw bytes so binary-body tests can serve exact payloads.
    fn serve(response: impl Into<Vec<u8>> + Send + 'static) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let response = response.into();
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
            stream.write_all(&response).unwrap();
            String::from_utf8(received).unwrap()
        });
        (url, handle)
    }

    #[test]
    fn b64_roundtrip_covers_edge_lengths() {
        // Deterministic xorshift bytes over every length 0..=16 covers the
        // chunk-boundary cases (0, 1, 2 mod 3) of the encoder.
        let mut seed = 0x9E37_79B9u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for len in 0..=16usize {
            let data: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            assert_eq!(b64_decode(&b64_encode(&data)).unwrap(), data);
        }
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(b64_decode("Zm9vYmFy").unwrap(), b"foobar");
    }

    #[test]
    fn b64_decode_rejects_bad_payloads() {
        assert!(b64_decode("A").is_err()); // leftover bits, not a full byte
        assert!(b64_decode("AB!C").is_err()); // invalid character
        assert!(b64_decode("AB=CD").is_err()); // padding in the middle
    }

    #[tokio::test]
    async fn binary_bodies_carry_base64_and_text_bodies_do_not() {
        let bytes: Vec<u8> = vec![0x50, 0x4b, 0x03, 0x04, 0xff, 0x00, 0xfe, 0x80, 0x01];
        let head = b"HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ncontent-length: 9\r\n\r\n";
        let mut raw = head.to_vec();
        raw.extend(bytes.iter().copied());
        let (url, server) = serve(raw);
        let mut payload = request();
        payload.url = url;
        let response = run_request(payload, &Ctx::no_jar()).await;
        server.join().unwrap();
        let body_b64 = response.body_b64.expect("binary body must carry base64");
        assert_eq!(b64_decode(&body_b64).unwrap(), bytes);
        assert!(response.body.contains('\u{FFFD}')); // the lossy view still renders

        let (url, server) = serve("HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 5\r\n\r\nhello".to_string());
        let mut payload = request();
        payload.url = url;
        let response = postlite_core::send(payload).await;
        server.join().unwrap();
        assert_eq!(response.body, "hello");
        assert_eq!(response.body_b64, None);
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
        assert!(settings.workspace_id.is_empty());
        assert!(settings.proxy.is_none());
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
                let response = run_request(payload, &Ctx::no_jar()).await;
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
            let response = run_request(serde_json::from_value(payload).unwrap(), &Ctx::no_jar()).await;
            assert!(response.error.is_none(), "{:?}", response.error);
            assert_eq!(response.status, 200);
            assert_eq!(response.size_bytes, size);
            assert_eq!(response.body.len(), cap);
            assert_eq!(response.truncated, size > cap);
            server.join().unwrap();
        }
    }

    #[tokio::test]
    async fn outgoing_headers_default_and_preserve_explicit_values() {
        for custom in [false, true] {
            let (url, server) = serve("HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
            let mut payload = request();
            payload.url = url;
            if custom {
                payload.headers = [
                    ("uSeR-Agent", "CustomClient/1.0"),
                    ("Accept", "application/json"),
                    ("Accept-Encoding", "identity"),
                ]
                .into_iter()
                .map(|(key, value)| HeaderPair {
                    key: key.into(),
                    value: value.into(),
                })
                .collect();
            }
            let response = run_request(payload, &Ctx::no_jar()).await;
            assert_eq!(response.status, 204, "{:?}", response.error);
            let received = server.join().unwrap();
            let values = |name: &str| -> Vec<&str> {
                received
                    .lines()
                    .filter_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case(name).then_some(value.trim())
                    })
                    .collect()
            };
            if custom {
                assert_eq!(values("user-agent"), ["CustomClient/1.0"]);
                assert_eq!(values("accept"), ["application/json"]);
                assert_eq!(values("accept-encoding"), ["identity"]);
            } else {
                assert_eq!(values("user-agent"), ["Postlite"]);
                assert_eq!(values("accept"), ["*/*"]);
                let encodings = values("accept-encoding");
                assert_eq!(encodings.len(), 1);
                assert!(encodings[0]
                    .split(',')
                    .any(|encoding| encoding.trim() == "gzip"));
            }
        }
    }

    #[tokio::test]
    async fn disable_cache_replaces_all_existing_values_only_when_enabled() {
        for disable_cache in [false, true] {
            let (url, server) =
                serve("HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
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
            let response = run_request(payload, &Ctx::no_jar()).await;
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
                "HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let mut payload = request();
            payload.url = url;
            payload.settings = Some(HttpSettings {
                follow_redirects,
                max_redirects: 0,
                ..HttpSettings::default()
            });
            let response = run_request(payload, &Ctx::no_jar()).await;
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
        let response = run_request(payload, &Ctx::no_jar()).await;
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
                "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
            );
            let (url, redirect_server) = serve(format!(
                "HTTP/1.1 302 Found\r\nLocation: {destination}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            ));
            let mut payload = request();
            payload.url = url;
            payload.settings = settings;
            let response = run_request(payload, &Ctx::no_jar()).await;
            assert!(response.error.is_none(), "{:?}", response.error);
            assert_eq!(response.status, 200);
            assert_eq!(response.body, "ok");
            redirect_server.join().unwrap();
            final_server.join().unwrap();
        }
    }

    /// An environment reader backed by a fixed map, so proxy lookup never touches the real process
    /// environment (which parallel tests would race on).
    fn fake_env<K: Into<String>, V: Into<String>>(
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> EnvReader {
        let map: HashMap<String, String> = pairs
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
        Arc::new(move |name| map.get(name).cloned())
    }

    fn manual(url: &str, bypass: &str) -> ProxySettings {
        ProxySettings { mode: "manual".into(), url: url.into(), bypass: bypass.into() }
    }

    #[test]
    fn proxy_defaults_to_direct_and_validation_gates_the_mode() {
        assert!(HttpSettings::default().proxy.is_none());
        assert!(HttpSettings::default().validate().is_ok());

        let environment = HttpSettings {
            proxy: Some(ProxySettings { mode: "environment".into(), ..Default::default() }),
            ..Default::default()
        };
        assert!(environment.validate().is_ok());

        let unknown = HttpSettings {
            proxy: Some(ProxySettings { mode: "system".into(), ..Default::default() }),
            ..Default::default()
        };
        assert!(unknown.validate().is_err(), "only manual and environment exist");

        let empty_url = HttpSettings {
            proxy: Some(manual("   ", "")),
            ..Default::default()
        };
        assert!(empty_url.validate().is_err());

        let malformed = HttpSettings {
            proxy: Some(manual("not a url", "")),
            ..Default::default()
        };
        assert!(malformed.validate().is_err(), "a bad url is an error before sending");
    }

    #[test]
    fn manual_proxy_builds_with_bypass_and_credentials() {
        let resolved = resolve_proxy(
            Some(&manual("http://127.0.0.1:8888", "localhost,.internal")),
            Some(&ProxyCredentials { username: "u".into(), password: "p".into() }),
            &fake_env(Vec::<(String, String)>::new()),
            "http://example.com/",
        )
        .unwrap();
        assert!(resolved.proxy.is_some());
        assert_eq!(resolved.note, None);

        // No credentials, no bypass: still a valid proxy.
        let bare = resolve_proxy(
            Some(&manual("http://127.0.0.1:8888", "")),
            None,
            &fake_env(Vec::<(String, String)>::new()),
            "http://example.com/",
        )
        .unwrap();
        assert!(bare.proxy.is_some());

        let bad = resolve_proxy(
            Some(&manual("not a url", "")),
            None,
            &fake_env(Vec::<(String, String)>::new()),
            "http://example.com/",
        );
        assert!(bad.is_err());
    }

    #[test]
    fn environment_proxy_is_case_insensitive_and_falls_back_to_all() {
        let settings = ProxySettings { mode: "environment".into(), ..Default::default() };
        let empty = fake_env(Vec::<(String, String)>::new());

        // Uppercase scheme var for an https target.
        let upper = fake_env([("HTTPS_PROXY", "http://upper:1")]);
        assert!(resolve_proxy(Some(&settings), None, &upper, "https://example.com/").unwrap().proxy.is_some());
        // Lowercase alone is enough.
        let lower = fake_env([("https_proxy", "http://lower:2")]);
        assert!(resolve_proxy(Some(&settings), None, &lower, "https://example.com/").unwrap().proxy.is_some());
        // An http target reads HTTP_PROXY, not HTTPS_PROXY.
        let http_only = fake_env([("HTTP_PROXY", "http://http:3")]);
        assert!(resolve_proxy(Some(&settings), None, &http_only, "http://example.com/").unwrap().proxy.is_some());
        assert!(resolve_proxy(Some(&settings), None, &http_only, "https://example.com/").unwrap().proxy.is_none());
        // ALL_PROXY is the fallback when the scheme var is absent.
        let all = fake_env([("all_proxy", "http://all:4")]);
        assert!(resolve_proxy(Some(&settings), None, &all, "http://example.com/").unwrap().proxy.is_some());
        assert!(resolve_proxy(Some(&settings), None, &empty, "https://example.com/").unwrap().proxy.is_none());
    }

    #[test]
    fn environment_with_nothing_set_goes_direct_and_says_so() {
        let settings = ProxySettings { mode: "environment".into(), ..Default::default() };
        let resolved = resolve_proxy(
            Some(&settings),
            None,
            &fake_env(Vec::<(String, String)>::new()),
            "http://example.com/",
        )
        .unwrap();
        assert!(resolved.proxy.is_none());
        assert_eq!(resolved.note.as_deref(), Some("no proxy in environment"));

        // No proxy setting at all is direct and silent.
        let none = resolve_proxy(None, None, &fake_env(Vec::<(String, String)>::new()), "http://example.com/").unwrap();
        assert!(none.proxy.is_none());
        assert_eq!(none.note, None);
    }

    #[tokio::test]
    async fn a_manual_proxy_receives_the_request_and_its_credentials() {
        let (proxy_url, proxy_server) = serve(
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
        );
        let mut payload = request();
        // A target that is never resolved: the proxy intercepts it.
        payload.url = "http://example.com/some/path".into();
        payload.settings = Some(HttpSettings {
            proxy: Some(manual(&proxy_url, "")),
            ..Default::default()
        });
        let ctx = Ctx {
            jar: None,
            proxy_credentials: Some(ProxyCredentials { username: "u".into(), password: "p".into() }),
            env: system_env(),
        };
        let response = run_request(payload, &ctx).await;
        assert!(response.error.is_none(), "{:?}", response.error);
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "ok");

        let received = proxy_server.join().unwrap();
        // Absolute-form request line proves it went through the proxy, not direct.
        assert!(received.starts_with("GET http://example.com/some/path"), "{received}");
        assert!(received.to_ascii_lowercase().contains("proxy-authorization: basic"), "{received}");
    }

    #[tokio::test]
    async fn a_bypassed_host_goes_direct_not_through_the_proxy() {
        let (target_url, target_server) = serve(
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
        );
        let proxy_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy_url = format!("http://{}/", proxy_listener.local_addr().unwrap());
        proxy_listener.set_nonblocking(true).unwrap();

        let mut payload = request();
        payload.url = target_url;
        payload.settings = Some(HttpSettings {
            proxy: Some(manual(&proxy_url, "127.0.0.1")),
            ..Default::default()
        });
        let response = run_request(payload, &Ctx::no_jar()).await;
        assert!(response.error.is_none(), "{:?}", response.error);
        assert_eq!(response.body, "ok");

        let received = target_server.join().unwrap();
        assert!(received.starts_with("GET /"), "a direct request uses a relative URI: {received}");
        assert!(proxy_listener.accept().is_err(), "the bypassed host reached the proxy");
    }

    #[tokio::test]
    async fn environment_prefers_uppercase_scheme_var_over_lowercase_and_all() {
        let (upper_url, upper_server) = serve(
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
        );
        let lower = TcpListener::bind("127.0.0.1:0").unwrap();
        lower.set_nonblocking(true).unwrap();
        let all = TcpListener::bind("127.0.0.1:0").unwrap();
        all.set_nonblocking(true).unwrap();
        let env = fake_env(vec![
            ("HTTP_PROXY".to_string(), upper_url),
            ("http_proxy".to_string(), format!("http://{}/", lower.local_addr().unwrap())),
            ("ALL_PROXY".to_string(), format!("http://{}/", all.local_addr().unwrap())),
        ]);

        let mut payload = request();
        payload.url = "http://example.com/".into();
        payload.settings = Some(HttpSettings {
            proxy: Some(ProxySettings { mode: "environment".into(), ..Default::default() }),
            ..Default::default()
        });
        let ctx = Ctx { jar: None, proxy_credentials: None, env };
        let response = run_request(payload, &ctx).await;
        assert!(response.error.is_none(), "{:?}", response.error);

        let received = upper_server.join().unwrap();
        assert!(received.starts_with("GET http://example.com/"), "{received}");
        assert!(lower.accept().is_err(), "lowercase var won over uppercase");
        assert!(all.accept().is_err(), "ALL_PROXY won over the scheme var");
    }

    #[test]
    fn save_workspace_strips_proxy_credentials_but_keeps_request_auth() {
        let state = json!({
            "settings": { "request": { "proxy": {
                "mode": "manual", "url": "http://p:1", "bypass": "", "username": "u", "password": "p"
            } } },
            "collections": [ { "requests": [ { "auth": {
                "type": "basic", "basic": { "username": "alice", "password": "secret" }
            } } ] } ]
        })
        .to_string();

        let cleaned = strip_proxy_credentials(&state);
        let value: serde_json::Value = serde_json::from_str(&cleaned).unwrap();
        let proxy = &value["settings"]["request"]["proxy"];
        assert_eq!(proxy["mode"], "manual");
        assert_eq!(proxy["url"], "http://p:1");
        assert!(proxy.get("username").is_none(), "proxy username must not be saved");
        assert!(proxy.get("password").is_none(), "proxy password must not be saved");
        // Request auth is a different secret with a different home; it stays.
        let basic = &value["collections"][0]["requests"][0]["auth"]["basic"];
        assert_eq!(basic["username"], "alice");
        assert_eq!(basic["password"], "secret");
    }

    #[test]
    fn a_save_without_proxy_credentials_is_left_byte_for_byte() {
        let state = json!({
            "settings": { "request": { "proxy": { "mode": "manual", "url": "http://p:1" } } }
        })
        .to_string();
        assert_eq!(strip_proxy_credentials(&state), state);
    }

    /// A jar that holds one cookie for the loopback host and writes nothing anywhere.
    fn memory_jar() -> Arc<PostliteJar> {
        let jar = JarManager::memory_only().jar("workspace-a").unwrap();
        let set_cookie = HeaderValue::from_str("sid=abc; Path=/; Max-Age=60").unwrap();
        let url = url::Url::parse("http://127.0.0.1:1/").unwrap();
        reqwest::cookie::CookieStore::set_cookies(&*jar, &mut [&set_cookie].into_iter(), &url);
        jar
    }

    fn cookie_header(raw: &str) -> Option<String> {
        raw.lines()
            .find(|line| line.to_ascii_lowercase().starts_with("cookie:"))
            .map(|line| line.split_once(':').unwrap().1.trim().to_owned())
    }

    #[tokio::test]
    async fn an_attached_jar_replays_on_the_wire_and_no_jar_sends_nothing() {
        for (attach_jar, explicit_cookie, expected) in [
            (false, None, None),
            (true, None, Some("sid=abc")),
            (true, Some("sid=explicit"), Some("sid=explicit")),
        ] {
            let (url, server) = serve("HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
            let mut payload = request();
            payload.url = url;
            if let Some(cookie) = explicit_cookie {
                payload.headers.push(HeaderPair {
                    key: "Cookie".into(),
                    value: cookie.into(),
                });
            }
            let ctx = Ctx {
                jar: attach_jar.then(memory_jar),
                proxy_credentials: None,
                env: system_env(),
            };
            let response = run_request(payload, &ctx).await;
            assert!(response.error.is_none(), "{:?}", response.error);

            let received = server.join().unwrap();
            assert_eq!(cookie_header(&received).as_deref(), expected, "{received}");
        }
    }

    #[tokio::test]
    async fn a_response_set_cookie_lands_in_the_attached_jar() {
        let jar = memory_jar();
        let (url, server) = serve(
            "HTTP/1.1 204 No Content\r\nSet-Cookie: theme=dark; Path=/; Max-Age=60\r\nConnection: close\r\n\r\n"
        );
        let mut payload = request();
        payload.url = url;
        let response = run_request(
            payload,
            &Ctx { jar: Some(jar.clone()), proxy_credentials: None, env: system_env() },
        )
        .await;
        assert!(response.error.is_none(), "{:?}", response.error);
        server.join().unwrap();

        assert!(
            jar.rows()
                .iter()
                .any(|row| row.name == "theme" && row.value == "dark"),
            "{:?}",
            jar.rows()
        );
    }

    #[test]
    fn export_path_appends_filter_extension_only_to_bare_names() {
        let json = vec!["json".to_string()];
        assert_eq!(export_path(PathBuf::from("/tmp/backup.json"), &json), PathBuf::from("/tmp/backup.json"));
        assert_eq!(export_path(PathBuf::from("/tmp/backup"), &json), PathBuf::from("/tmp/backup.json"));
        assert_eq!(
            export_path(PathBuf::from("/tmp/archive.tar"), &json),
            PathBuf::from("/tmp/archive.tar"),
            "an extension the user typed stays untouched"
        );
        assert_eq!(export_path(PathBuf::from("/tmp/noext"), &[]), PathBuf::from("/tmp/noext"));
    }

    #[test]
    fn file_part_meta_guesses_filename_and_mime_from_extension() {
        let (name, mime) = file_part_meta(std::path::Path::new("/tmp/Avatar.PNG"));
        assert_eq!(name, "Avatar.PNG");
        assert_eq!(mime, "image/png");
        assert_eq!(file_part_meta(std::path::Path::new("/tmp/data.json")).1, "application/json");
        assert_eq!(file_part_meta(std::path::Path::new("/tmp/noext")).1, "application/octet-stream");
        assert_eq!(file_part_meta(std::path::Path::new("/")).0, "file");
    }

    #[tokio::test]
    async fn form_file_rows_upload_bytes_as_multipart_parts() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("avatar.png");
        std::fs::write(&file, b"\x89PNG\r\n\x1a\nfake-png-bytes").unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut received = Vec::new();
            let mut buffer = [0; 4096];
            while !received.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).unwrap();
                assert_ne!(count, 0, "connection closed before request headers");
                received.extend_from_slice(&buffer[..count]);
            }
            let header_end = received.windows(4).position(|b| b == b"\r\n\r\n").unwrap() + 4;
            let head = String::from_utf8_lossy(&received[..header_end]).into_owned();
            let length: usize = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    if name.trim().eq_ignore_ascii_case("content-length") {
                        value.trim().parse().ok()
                    } else {
                        None
                    }
                })
                .unwrap();
            while received.len() < header_end + length {
                let count = stream.read(&mut buffer).unwrap();
                assert_ne!(count, 0, "connection closed before request body");
                received.extend_from_slice(&buffer[..count]);
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .unwrap();
            (head, received[header_end..header_end + length].to_vec())
        });

        let mut payload = request();
        payload.method = "POST".into();
        payload.url = url;
        payload.form = vec![
            FormPair { key: "note".into(), value: "hello".into(), kind: String::new() },
            FormPair {
                key: "upload".into(),
                value: file.to_string_lossy().into_owned(),
                kind: "file".into(),
            },
            FormPair { key: "skipped".into(), value: "  ".into(), kind: "file".into() },
        ];
        let response = run_request(payload, &Ctx::no_jar()).await;
        assert!(response.error.is_none(), "{:?}", response.error);
        assert_eq!(response.status, 200);
        let (head, body) = server.join().unwrap();
        assert!(head.to_ascii_lowercase().contains("multipart/form-data; boundary="), "{head}");
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("name=\"note\"\r\n\r\nhello"), "{body}");
        assert!(body.contains("name=\"upload\"; filename=\"avatar.png\""), "{body}");
        assert!(body.to_ascii_lowercase().contains("content-type: image/png"), "{body}");
        assert!(body.contains("fake-png-bytes"), "{body}");
        assert!(!body.contains("name=\"skipped\""), "file rows without a path are not sent");
    }

    #[tokio::test]
    async fn form_file_row_with_missing_path_reports_read_error() {
        let mut payload = request();
        payload.method = "POST".into();
        payload.form = vec![FormPair {
            key: "upload".into(),
            value: "/nonexistent/postlite-missing.png".into(),
            kind: "file".into(),
        }];
        let response = run_request(payload, &Ctx::no_jar()).await;
        assert_eq!(response.status, 0);
        let error = response.error.unwrap();
        assert!(error.contains("could not read file"), "{error}");
        assert!(error.contains("upload"), "{error}");
    }
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let data = app.path().app_data_dir()?;
            let identifier = app.config().identifier.clone();
            app.manage(JarManager::new(data.join("cookies"), identifier.clone()));
            app.manage(Secrets::new(data.join("secrets"), identifier));
            app.manage(OAuthCapture::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            http_send,
            load_workspace,
            save_workspace,
            cookie_list,
            cookie_delete,
            cookie_purge,
            cookie_set_persist,
            proxy_credentials_set,
            oauth2_token,
            oauth2_secret_set,
            oauth2_secret_get,
            oauth2_secret_clear,
            oauth2_authorize_begin,
            oauth2_authorize_await,
            oauth2_authorize_cancel,
            open_in_browser,
            proxy_credentials_get,
            save_export_file,
            pick_file
        ])
        .run(tauri::generate_context!())
        .expect("failed to run postlite");
}
