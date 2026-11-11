#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod jar;
mod store;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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
    /// The workspace whose cookie jar this request belongs to. Empty means the request is not
    /// inside a workspace, and then no jar is attached at all.
    workspace_id: String,
    /// `None` = direct, unchanged behaviour. Credentials are deliberately not here: they live in
    /// the `global` encrypted store (§8) and are injected through `Ctx`, never through this JSON.
    proxy: Option<ProxySettings>,
}

/// The non-secret half of the proxy setting (§9). `username`/`password` are stored separately.
#[derive(Deserialize, Serialize, Clone, Default)]
#[serde(default)]
struct ProxySettings {
    mode: String,
    url: String,
    bypass: String,
}

/// The secret half, kept in the `global` encrypted store and stripped from every workspace save.
#[derive(Deserialize, Serialize, Clone, Default)]
#[serde(default)]
struct ProxyCredentials {
    username: String,
    password: String,
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
            workspace_id: String::new(),
            proxy: None,
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
        if let Some(proxy) = &self.proxy {
            match proxy.mode.as_str() {
                "manual" => {
                    if proxy.url.trim().is_empty() {
                        return Err("proxy url is required in manual mode".into());
                    }
                    // The same parse the send path uses, so a malformed url is an error before
                    // anything leaves the app, like the other invalid ranges.
                    reqwest::Proxy::all(&proxy.url)
                        .map_err(|e| format!("invalid proxy url: {e}"))?;
                }
                "environment" => {}
                other => return Err(format!("unknown proxy mode: {other}")),
            }
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
    /// A non-error line the UI should surface, e.g. "no proxy in environment" (§9). Distinct from
    /// `error`: the request still ran.
    note: Option<String>,
}

const MAX_BODY_BYTES: usize = 2_000_000;

/// An environment reader, injected so the environment-proxy lookup can be faked in tests without
/// mutating the process environment (which would race across parallel tests).
type EnvReader = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

fn system_env() -> EnvReader {
    Arc::new(|name: &str| std::env::var(name).ok())
}

/// Everything a send needs beyond the request itself.
struct Ctx {
    jar: Option<Arc<jar::PostliteJar>>,
    proxy_credentials: Option<ProxyCredentials>,
    env: EnvReader,
}

impl Ctx {
    #[cfg(test)]
    fn no_jar() -> Self {
        Self {
            jar: None,
            proxy_credentials: None,
            env: system_env(),
        }
    }
}

/// A workspace's jar plus the reason it is not persisting, if any, so the drawer can show it
/// every time rather than only in the moment the file was found damaged.
#[derive(Clone)]
struct OpenedJar {
    jar: Arc<jar::PostliteJar>,
    note: Option<String>,
}

/// What `cookie_list` hands the drawer. Values are real; masking happens in the view (§10).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CookieSnapshot {
    rows: Vec<jar::CookieRow>,
    note: Option<String>,
    persisting: bool,
}

/// Which cookies `cookie_delete` removes. A field left out matches anything, so `{}` clears the
/// jar and `{domain}` clears one domain.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct CookieFilter {
    domain: Option<String>,
    path: Option<String>,
    name: Option<String>,
}

/// One open jar per workspace id, kept for the life of the window so cookie state survives
/// between requests and each workspace's file is read once.
struct JarManager {
    dir: PathBuf,
    keys: Box<dyn Fn(&str) -> Arc<dyn store::KeySource> + Send + Sync>,
    jars: Mutex<HashMap<String, OpenedJar>>,
    /// The global `Persist cookies` switch (§11). Applies to jars opened later and, through
    /// `set_persist`, to every jar already open.
    persist: AtomicBool,
}

impl JarManager {
    /// Production: each workspace's data key lives in the OS secret store under this app's own
    /// identifier, so an isolated build can never read or delete another build's credentials.
    fn new(dir: PathBuf, identifier: String) -> Self {
        Self {
            dir,
            keys: Box::new(move |workspace_id| {
                match store::KeyringSource::new(&identifier, workspace_id) {
                    // No usable OS secret store degrades the jar to memory-only, it does not refuse it.
                    Ok(source) => Arc::new(source),
                    Err(_) => Arc::new(store::NoSecretStoreKeys),
                }
            }),
            jars: Mutex::new(HashMap::new()),
            persist: AtomicBool::new(true),
        }
    }

    fn jar(&self, workspace_id: &str) -> Option<Arc<jar::PostliteJar>> {
        self.opened(workspace_id)
            .map(|opened| opened.jar.clone())
    }

    /// What the drawer shows: the live rows, why the jar is not persisting if it isn't, and whether
    /// persistence is even switched on.
    fn snapshot(&self, workspace_id: &str) -> Result<CookieSnapshot, String> {
        let opened = self
            .opened(workspace_id)
            .ok_or("cookie list needs a workspace id")?;
        Ok(CookieSnapshot {
            rows: opened.jar.rows(),
            note: opened.note.clone(),
            persisting: opened.jar.persists(),
        })
    }

    fn remove(&self, workspace_id: &str, filter: &CookieFilter) -> Result<usize, String> {
        let opened = self
            .opened(workspace_id)
            .ok_or("cookie delete needs a workspace id")?;
        let removed = opened.jar.remove_matching(
            filter.domain.as_deref(),
            filter.path.as_deref(),
            filter.name.as_deref(),
        );
        // A user-facing delete must be on disk before the drawer redraws, not inside a debounce.
        opened.jar.flush();
        Ok(removed)
    }

    /// Delete a workspace's cookie state, called from the workspace lifecycle (P8): memory, file and key.
    fn purge(&self, workspace_id: &str) -> Result<(), String> {
        let opened = self
            .opened(workspace_id)
            .ok_or("cookie purge needs a workspace id")?;
        opened.jar.clear_all();
        Ok(())
    }

    /// Apply the global `Persist cookies` switch to every jar already open; jars opened later read
    /// the same flag in `opened`. Turning it off drops each jar's saved file so nothing resurrects.
    fn set_persist(&self, on: bool) {
        self.persist.store(on, Ordering::Relaxed);
        for opened in self.jars.lock().unwrap().values() {
            opened.jar.set_persist(on);
        }
    }

    /// The jar plus its note, opening it on first use. An empty workspace id resolves to nothing
    /// and must never reach the secret store.
    fn opened(&self, workspace_id: &str) -> Option<OpenedJar> {
        if workspace_id.is_empty() {
            return None;
        }
        let mut jars = self.jars.lock().unwrap();
        if let Some(opened) = jars.get(workspace_id) {
            return Some(opened.clone());
        }

        let keys = (self.keys)(workspace_id);
        let (jar, note) = match jar::PostliteJar::open(&self.dir, workspace_id, keys, self.persist.load(Ordering::Relaxed)) {
            jar::JarOpen::Restored(jar) => (jar, None),
            jar::JarOpen::Empty(jar, note) => (jar, Some(note)),
        };
        let opened = OpenedJar { jar, note };
        jars.insert(workspace_id.to_owned(), opened.clone());
        Some(opened)
    }
}

#[tauri::command]
async fn http_send(app: AppHandle, request: RequestPayload) -> ResponsePayload {
    let workspace_id = request
        .settings
        .as_ref()
        .map(|settings| settings.workspace_id.clone())
        .unwrap_or_default();
    // Credentials come from the global encrypted store, never from the request JSON. A store that
    // cannot be read degrades to "no auth" rather than blocking the send.
    let proxy_credentials = load_credentials(&app.state::<Secrets>().store())
        .ok()
        .flatten();
    let ctx = Ctx {
        jar: app.state::<JarManager>().jar(&workspace_id),
        proxy_credentials,
        env: system_env(),
    };
    run_request(request, &ctx).await
}

/// The proxy a send should use, plus an optional note. `None` proxy means direct.
struct ResolvedProxy {
    proxy: Option<reqwest::Proxy>,
    note: Option<String>,
}

/// Turn the proxy setting into a reqwest `Proxy` (§9). Manual builds from the url and applies the
/// bypass list and any stored credentials; environment reads the variables ourselves because
/// reqwest will not (§2), and says so when it finds nothing.
fn resolve_proxy(
    settings: Option<&ProxySettings>,
    credentials: Option<&ProxyCredentials>,
    env: &EnvReader,
    target_url: &str,
) -> Result<ResolvedProxy, String> {
    let env = env.as_ref();
    let Some(settings) = settings else {
        return Ok(ResolvedProxy { proxy: None, note: None });
    };
    match settings.mode.as_str() {
        "manual" => {
            let mut proxy = reqwest::Proxy::all(&settings.url)
                .map_err(|e| format!("invalid proxy url: {e}"))?;
            if !settings.bypass.trim().is_empty() {
                proxy = proxy.no_proxy(reqwest::NoProxy::from_string(&settings.bypass));
            }
            if let Some(creds) = credentials {
                if !creds.username.is_empty() {
                    proxy = proxy.basic_auth(&creds.username, &creds.password);
                }
            }
            Ok(ResolvedProxy { proxy: Some(proxy), note: None })
        }
        "environment" => {
            let https = url::Url::parse(target_url)
                .map(|u| u.scheme() == "https")
                .unwrap_or(false);
            // Uppercase then lowercase, scheme-specific first, ALL_PROXY as the fallback.
            let names: &[&str] = if https {
                &["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
            } else {
                &["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"]
            };
            let found = names
                .iter()
                .find_map(|name| env(name).filter(|value| !value.trim().is_empty()));
            let Some(value) = found else {
                return Ok(ResolvedProxy {
                    proxy: None,
                    note: Some("no proxy in environment".into()),
                });
            };
            let mut proxy = reqwest::Proxy::all(&value)
                .map_err(|e| format!("invalid proxy url in environment: {e}"))?;
            // NO_PROXY through the same reader so tests can fake it; equivalent to from_env().
            let bypass = ["NO_PROXY", "no_proxy"]
                .iter()
                .find_map(|name| env(name))
                .filter(|value| !value.trim().is_empty());
            if let Some(list) = bypass {
                proxy = proxy.no_proxy(reqwest::NoProxy::from_string(&list));
            }
            Ok(ResolvedProxy { proxy: Some(proxy), note: None })
        }
        other => Err(format!("unknown proxy mode: {other}")),
    }
}

async fn run_request(request: RequestPayload, ctx: &Ctx) -> ResponsePayload {
    let empty = ResponsePayload {
        status: 0,
        status_text: String::new(),
        headers: Vec::new(),
        body: String::new(),
        truncated: false,
        elapsed_ms: 0,
        size_bytes: 0,
        error: None,
        note: None,
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

    let resolved = match resolve_proxy(
        settings.proxy.as_ref(),
        ctx.proxy_credentials.as_ref(),
        &ctx.env,
        &request.url,
    ) {
        Ok(resolved) => resolved,
        Err(error) => return with_error(empty, error),
    };

    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_millis(settings.request_timeout_ms as u64))
        .redirect(if settings.follow_redirects {
            reqwest::redirect::Policy::limited(settings.max_redirects as usize)
        } else {
            reqwest::redirect::Policy::none()
        })
        .danger_accept_invalid_certs(!settings.ssl_verify);
    // Without a jar reqwest has none either (its default is `cookie_store: None`), so cookies
    // neither travel nor accumulate for a request outside a workspace.
    if let Some(jar) = &ctx.jar {
        builder = builder.cookie_provider(jar.clone());
    }
    if let Some(proxy) = resolved.proxy {
        builder = builder.proxy(proxy);
    }
    let client = match builder.build() {
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
        note: resolved.note,
    }
}

fn with_error(mut response: ResponsePayload, error: String) -> ResponsePayload {
    response.error = Some(error);
    response
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

/// Where the `global` encrypted store lives. Proxy credentials are the only thing in it (§8); the
/// per-workspace cookie jars have their own files. Kept as plain data so the keyring is only touched
/// when a store is actually opened, and tests can build one over a fake key source instead.
struct Secrets {
    dir: PathBuf,
    identifier: String,
}

impl Secrets {
    fn store(&self) -> store::EncryptedStore {
        let keys: Arc<dyn store::KeySource> = match store::KeyringSource::new(&self.identifier, "global") {
            Ok(source) => Arc::new(source),
            Err(_) => Arc::new(store::NoSecretStoreKeys),
        };
        store::EncryptedStore::open(&self.dir, "global", keys)
    }
}

fn save_credentials(store: &store::EncryptedStore, creds: &ProxyCredentials) -> Result<(), String> {
    let bytes = serde_json::to_vec(creds).map_err(|e| e.to_string())?;
    store.save(&bytes).map_err(|e| e.message())
}

fn load_credentials(store: &store::EncryptedStore) -> Result<Option<ProxyCredentials>, String> {
    match store.load() {
        Ok(None) => Ok(None),
        Ok(Some(bytes)) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("saved proxy credentials could not be read: {e}")),
        Err(e) => Err(e.message()),
    }
}

#[tauri::command]
fn proxy_credentials_set(app: AppHandle, username: String, password: String) -> Result<(), String> {
    let creds = ProxyCredentials { username, password };
    save_credentials(&app.state::<Secrets>().store(), &creds)
}

#[tauri::command]
fn proxy_credentials_get(app: AppHandle) -> Result<Option<ProxyCredentials>, String> {
    load_credentials(&app.state::<Secrets>().store())
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
    use crate::store::{KeySource, TempDir};
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
                "HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
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
                "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".into(),
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
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".into(),
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
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".into(),
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
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".into(),
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
    fn proxy_credentials_round_trip_encrypted_in_the_global_store() {
        let dir = TempDir::new("proxy-creds");
        let keys = store::FakeKeySource::empty();
        let store = store::EncryptedStore::open(dir.path(), "global", keys.clone());
        assert!(load_credentials(&store).unwrap().is_none());

        let creds = ProxyCredentials { username: "user".into(), password: "hunter2-secret".into() };
        save_credentials(&store, &creds).unwrap();
        let loaded = load_credentials(&store).unwrap().unwrap();
        assert_eq!(loaded.username, "user");
        assert_eq!(loaded.password, "hunter2-secret");
        assert!(keys.get().unwrap().is_some(), "the data key was created");

        let bytes = std::fs::read(dir.path().join("global.bin")).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains("hunter2-secret"),
            "the password must not sit in the file in plaintext"
        );
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
    fn memory_jar() -> Arc<jar::PostliteJar> {
        // Persistence is off, so this path is never created or written to.
        let dir = std::env::temp_dir().join("postlite-ctx-test-jar-is-memory-only");
        let opened = jar::PostliteJar::open(&dir, "workspace-a", store::FakeKeySource::empty(), false);
        let jar = match opened {
            jar::JarOpen::Restored(jar) | jar::JarOpen::Empty(jar, _) => jar,
        };
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
        for attach_jar in [false, true] {
            let (url, server) = serve("HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".into());
            let mut payload = request();
            payload.url = url;
            let ctx = Ctx {
                jar: attach_jar.then(memory_jar),
                proxy_credentials: None,
                env: system_env(),
            };
            let response = run_request(payload, &ctx).await;
            assert!(response.error.is_none(), "{:?}", response.error);

            let received = server.join().unwrap();
            if attach_jar {
                assert_eq!(cookie_header(&received).as_deref(), Some("sid=abc"));
            } else {
                assert_eq!(cookie_header(&received), None, "{received}");
            }
        }
    }

    #[tokio::test]
    async fn a_response_set_cookie_lands_in_the_attached_jar() {
        let jar = memory_jar();
        let (url, server) = serve(
            "HTTP/1.1 204 No Content\r\nSet-Cookie: theme=dark; Path=/; Max-Age=60\r\nConnection: close\r\n\r\n".into(),
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

    type TestKeys = Arc<Mutex<HashMap<String, Arc<store::FakeKeySource>>>>;

    fn test_keys() -> TestKeys {
        Arc::new(Mutex::new(HashMap::new()))
    }

    /// A manager whose data keys are in-memory fake keyrings, one per workspace id, so a test can
    /// neither touch the machine's real credential store nor share one workspace's key with another.
    fn manager_with(dir: &std::path::Path, keys: TestKeys) -> JarManager {
        JarManager {
            dir: dir.to_path_buf(),
            keys: Box::new(move |workspace_id: &str| {
                let source = keys
                    .lock()
                    .unwrap()
                    .entry(workspace_id.to_owned())
                    .or_insert_with(store::FakeKeySource::empty)
                    .clone();
                source as Arc<dyn store::KeySource>
            }),
            jars: Mutex::new(HashMap::new()),
            persist: AtomicBool::new(true),
        }
    }

    /// Hand a workspace's jar one response header, the way a server would, without starting one.
    fn receive(manager: &JarManager, workspace_id: &str, set_cookie: &str, url: &str) {
        let jar = manager.jar(workspace_id).expect("a workspace id opens a jar");
        let header = HeaderValue::from_str(set_cookie).unwrap();
        let url = url::Url::parse(url).unwrap();
        reqwest::cookie::CookieStore::set_cookies(&*jar, &mut [&header].into_iter(), &url);
        // Flush rather than wait out the coalescing window, so the disk matches the list now.
        jar.flush();
    }

    fn has_key(keys: &TestKeys, workspace_id: &str) -> Option<Vec<u8>> {
        keys.lock()
            .unwrap()
            .get(workspace_id)
            .map(|source| store::KeySource::get(&**source).unwrap())
            .flatten()
    }

    #[test]
    fn only_a_workspace_id_reaches_a_jar() {
        let dir = TempDir::new("cmd-scope");
        let keys = test_keys();
        let manager = manager_with(dir.path(), keys.clone());

        // The id that arrives when no workspace is selected must not reach the map, the secret
        // store, or the disk — it is the one input that would otherwise merge every workspace.
        assert!(manager.jar("").is_none());
        assert!(manager.snapshot("").is_err());
        assert!(manager.remove("", &CookieFilter::default()).is_err());
        assert!(manager.purge("").is_err());
        assert!(keys.lock().unwrap().is_empty(), "{:?}", keys.lock().unwrap().keys());
        assert!(dir.files().is_empty(), "{:?}", dir.files());

        let one = manager.jar("workspace-a").unwrap();
        assert!(Arc::ptr_eq(&one, &manager.jar("workspace-a").unwrap()), "opened once");
        assert!(!Arc::ptr_eq(&one, &manager.jar("workspace-b").unwrap()), "one jar per workspace");
    }

    #[test]
    fn cookie_list_reports_the_rows_and_whether_they_are_saved() {
        let dir = TempDir::new("cmd-list");
        let manager = manager_with(dir.path(), test_keys());

        let empty = manager.snapshot("workspace-a").unwrap();
        assert!(empty.rows.is_empty());
        assert_eq!(empty.note, None);
        assert!(empty.persisting);

        receive(&manager, "workspace-a", "a=1; Path=/x; Max-Age=60", "http://example.com/x");
        receive(&manager, "workspace-a", "b=2; Path=/", "http://other.test/");

        let snapshot = manager.snapshot("workspace-a").unwrap();
        assert_eq!(snapshot.rows.len(), 2, "{:?}", snapshot.rows);
        // Rows come out grouped by domain, which is what the Cookie Manager draws.
        assert_eq!(snapshot.rows[0].domain, "example.com");
        assert_eq!(snapshot.rows[0].path, "/x");
        assert!(!snapshot.rows[0].session, "Max-Age makes it persistent");
        assert!(snapshot.rows[0].expires_ms.is_some());
        assert_eq!(snapshot.rows[1].domain, "other.test");
        assert!(snapshot.rows[1].session, "no expiry means it dies with the app");

        // The drawer reads camelCase, and a value it shows is the real value: masking is its job.
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["rows"][0]["name"], "a");
        assert_eq!(json["rows"][0]["value"], "1");
        assert_eq!(json["rows"][0]["expiresMs"], snapshot.rows[0].expires_ms.unwrap());
        assert!(json["rows"][0]["session"].is_boolean());
        assert_eq!(json["persisting"], true);
        assert!(json["note"].is_null());
    }

    #[test]
    fn cookie_delete_widens_from_one_cookie_to_the_whole_jar_and_the_disk_follows() {
        let dir = TempDir::new("cmd-delete");
        let keys = test_keys();
        let manager = manager_with(dir.path(), keys.clone());
        receive(&manager, "workspace-a", "a=1; Path=/x; Max-Age=60", "http://example.com/x");
        receive(&manager, "workspace-a", "b=2; Path=/y; Max-Age=60", "http://example.com/y");
        receive(&manager, "workspace-a", "c=3; Path=/; Max-Age=60", "http://other.test/");
        assert_eq!(manager.snapshot("workspace-a").unwrap().rows.len(), 3);

        let removed = |manager: &JarManager, domain: Option<&str>, path: Option<&str>, name: Option<&str>| {
            manager
                .remove(
                    "workspace-a",
                    &CookieFilter {
                        domain: domain.map(str::to_owned),
                        path: path.map(str::to_owned),
                        name: name.map(str::to_owned),
                    },
                )
                .unwrap()
        };

        // One row, then the same row again: a second click deletes nothing and says so.
        assert_eq!(removed(&manager, Some("example.com"), Some("/x"), Some("a")), 1);
        assert_eq!(removed(&manager, Some("example.com"), Some("/x"), Some("a")), 0);
        // One domain, which is the Cookie Manager's per-domain delete.
        assert_eq!(removed(&manager, Some("example.com"), None, None), 1);
        assert_eq!(
            manager
                .snapshot("workspace-a")
                .unwrap()
                .rows
                .iter()
                .map(|row| row.name.clone())
                .collect::<Vec<_>>(),
            ["c"]
        );

        // Nothing may still be on disk: a delete redraws from the list, and the list is the truth.
        let reopened = manager_with(dir.path(), keys.clone());
        assert_eq!(reopened.snapshot("workspace-a").unwrap().rows.len(), 1);

        // The empty filter is Settings' "Clear All Cookies" for this workspace's jar.
        assert_eq!(removed(&manager, None, None, None), 1);
        assert!(manager.snapshot("workspace-a").unwrap().rows.is_empty());
    }

    #[test]
    fn cookie_purge_leaves_neither_file_nor_credential() {
        let dir = TempDir::new("cmd-purge");
        let keys = test_keys();
        let manager = manager_with(dir.path(), keys.clone());
        receive(&manager, "workspace-a", "a=1; Path=/; Max-Age=60", "http://example.com/");
        receive(&manager, "workspace-b", "b=2; Path=/; Max-Age=60", "http://example.com/");
        assert_eq!(dir.files(), ["workspace-a.bin", "workspace-b.bin"]);

        manager.purge("workspace-a").unwrap();

        assert_eq!(dir.files(), ["workspace-b.bin"], "only that workspace's file");
        assert!(has_key(&keys, "workspace-a").is_none(), "and its key");
        assert!(has_key(&keys, "workspace-b").is_some(), "the neighbour keeps its key");
        // Reopening finds nothing to restore, so a purged workspace cannot come back with its old cookies.
        assert!(manager_with(dir.path(), keys.clone())
            .snapshot("workspace-a")
            .unwrap()
            .rows
            .is_empty());
    }

    #[test]
    fn persist_switch_stops_writes_and_drops_the_saved_jar() {
        let dir = TempDir::new("cmd-persist");
        let keys = test_keys();
        let manager = manager_with(dir.path(), keys.clone());
        receive(&manager, "workspace-a", "a=1; Path=/; Max-Age=60", "http://example.com/");
        assert_eq!(dir.files(), ["workspace-a.bin"]);
        assert!(manager.snapshot("workspace-a").unwrap().persisting);

        manager.set_persist(false);
        // The session jar keeps its cookies, but the saved file is gone so nothing resurrects.
        let off = manager.snapshot("workspace-a").unwrap();
        assert!(!off.persisting);
        assert_eq!(off.rows.len(), 1, "the jar still works for the session");
        assert!(dir.files().is_empty(), "turning persistence off drops the saved jar");
        receive(&manager, "workspace-a", "b=2; Path=/; Max-Age=60", "http://example.com/");
        assert!(dir.files().is_empty(), "nothing is written while persistence is off");

        // A jar opened after the switch inherits it.
        assert!(!manager.snapshot("workspace-b").unwrap().persisting);

        manager.set_persist(true);
        assert!(manager.snapshot("workspace-a").unwrap().persisting);
        manager.jar("workspace-a").unwrap().flush();
        assert_eq!(dir.files(), ["workspace-a.bin"], "re-enabling puts the current jar back");
    }

    #[test]
    fn a_file_that_cannot_be_decrypted_keeps_saying_so() {
        let dir = TempDir::new("cmd-corrupt");
        let manager = manager_with(dir.path(), test_keys());
        receive(&manager, "workspace-a", "a=1; Path=/; Max-Age=60", "http://example.com/");

        // A jar opened with a key that is not the one the file was written with.
        let stranger = manager_with(dir.path(), test_keys());
        let first = stranger.snapshot("workspace-a").unwrap();
        assert!(first.rows.is_empty(), "the damaged file is not loaded");
        assert!(first.persisting);
        let note = first.note.clone().unwrap_or_default();
        assert!(note.contains("could not be read"), "{note}");
        assert!(
            dir.files().iter().any(|name| name.ends_with(".corrupt")),
            "{:?}",
            dir.files()
        );

        // The note stays after the first call: the drawer must never show an unexplained empty list.
        assert_eq!(stranger.snapshot("workspace-a").unwrap().note, Some(note));
    }
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let data = app.path().app_data_dir()?;
            let identifier = app.config().identifier.clone();
            app.manage(JarManager::new(data.join("cookies"), identifier.clone()));
            app.manage(Secrets { dir: data.join("secrets"), identifier });
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
            proxy_credentials_get
        ])
        .run(tauri::generate_context!())
        .expect("failed to run postlite");
}
