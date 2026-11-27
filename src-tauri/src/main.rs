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
    /// "file" reads `value` as a disk path and uploads its bytes; anything else is a text part.
    #[serde(default, rename = "type")]
    kind: String,
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
    /// Base64 of the capped body when it is not valid UTF-8 (binary content);
    /// `None` for text bodies, where `body` already is the exact bytes. Save
    /// response writes this so binary files survive the IPC round-trip.
    body_b64: Option<String>,
    truncated: bool,
    elapsed_ms: u128,
    size_bytes: usize,
    error: Option<String>,
    /// A non-error line the UI should surface, e.g. "no proxy in environment" (§9). Distinct from
    /// `error`: the request still ran.
    note: Option<String>,
}

const MAX_BODY_BYTES: usize = 2_000_000;
const POSTLITE_USER_AGENT: &str = "Postlite";

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
        body_b64: None,
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
        .user_agent(POSTLITE_USER_AGENT)
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
    if request
        .form
        .iter()
        .any(|field| !field.key.trim().is_empty() && (field.kind != "file" || !field.value.trim().is_empty()))
    {
        let mut form = reqwest::multipart::Form::new();
        for field in request.form.iter().filter(|f| !f.key.trim().is_empty()) {
            if field.kind == "file" {
                if field.value.trim().is_empty() {
                    continue;
                }
                let path = std::path::Path::new(&field.value);
                let bytes = match tokio::fs::read(path).await {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        return with_error(empty, format!("could not read file for \"{}\": {e}", field.key));
                    }
                };
                let (name, mime) = file_part_meta(path);
                let part = reqwest::multipart::Part::bytes(bytes)
                    .file_name(name)
                    .mime_str(mime)
                    .map_err(|e| e.to_string());
                let part = match part {
                    Ok(part) => part,
                    Err(e) => return with_error(empty, e),
                };
                form = form.part(field.key.clone(), part);
            } else {
                form = form.text(field.key.clone(), field.value.clone());
            }
        }
        builder = builder.multipart(form);
    } else if let Some(body) = request.body.filter(|b| !b.is_empty()) {
        builder = builder.body(body);
    }

    let started = Instant::now();
    let mut response = match builder.send().await {
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

    // Count the entire decoded body while retaining only the displayable prefix.
    let mut size_bytes = 0usize;
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                size_bytes = size_bytes.saturating_add(chunk.len());
                let remaining = max_body_bytes - bytes.len();
                bytes.extend_from_slice(&chunk[..remaining.min(chunk.len())]);
            }
            Ok(None) => break,
            Err(e) => return with_error(empty, format!("failed reading body: {e}")),
        }
    }
    let truncated = size_bytes > max_body_bytes;
    let (body, body_b64) = match String::from_utf8(bytes) {
        Ok(text) => (text, None),
        Err(error) => {
            let bytes = error.as_bytes();
            (
                String::from_utf8_lossy(bytes).into_owned(),
                Some(b64_encode(bytes)),
            )
        }
    };

    ResponsePayload {
        status: status.as_u16(),
        status_text: status.canonical_reason().unwrap_or_default().to_string(),
        headers,
        body,
        body_b64,
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

/// Minimal base64 (RFC 4648, standard alphabet with padding) for byte-faithful
/// response bodies over the JSON IPC and binary saves. Hand-rolled so the
/// dependency list stays untouched.
fn b64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(chunk[0]) << 16) | (u32::from(second) << 8) | u32::from(third);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

fn b64_decode(text: &str) -> Result<Vec<u8>, String> {
    fn value(byte: u8) -> Result<u32, String> {
        match byte {
            b'A'..=b'Z' => Ok(u32::from(byte - b'A')),
            b'a'..=b'z' => Ok(u32::from(byte - b'a' + 26)),
            b'0'..=b'9' => Ok(u32::from(byte - b'0' + 52)),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err(format!("invalid base64 byte {byte}")),
        }
    }
    let text = text.trim();
    // Up to two padding characters are allowed; anything else must be data.
    let stripped = text.strip_suffix('=').unwrap_or(text);
    let stripped = stripped.strip_suffix('=').unwrap_or(stripped);
    if text.len() - stripped.len() != text.chars().filter(|c| *c == '=').count() {
        return Err("base64 padding only allowed at the end".to_string());
    }
    let mut out = Vec::with_capacity(stripped.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in stripped.bytes() {
        buffer = (buffer << 6) | value(byte)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    if bits >= 6 {
        return Err("truncated base64 payload".to_string());
    }
    Ok(out)
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
        self.store_named("global")
    }

    // One encrypted store per name: "global" for proxy credentials, per-node
    // stores for oauth2 client secrets.
    fn store_named(&self, name: &str) -> store::EncryptedStore {
        let keys: Arc<dyn store::KeySource> = match store::KeyringSource::new(&self.identifier, name) {
            Ok(source) => Arc::new(source),
            Err(_) => Arc::new(store::NoSecretStoreKeys),
        };
        store::EncryptedStore::open(&self.dir, name, keys)
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

/* ------------------------------------------------------------------- oauth2 */
/* Tokens are acquired through stateless commands; the frontend owns the
   session cache. The client secret lives in a per-node encrypted store (OS
   keyring via the store pattern) and is read server-side, so it never
   round-trips to the frontend or lands in workspace.json. */

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OAuth2Config {
    #[serde(default)]
    node_id: String,
    #[serde(default)]
    grant_type: String,
    #[serde(default)]
    token_url: String,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    client_auth_in: String, // "body" (default) | "basic"
    #[serde(default)]
    code: String,
    #[serde(default)]
    redirect_uri: String,
    #[serde(default)]
    code_verifier: String,
    #[serde(default)]
    refresh_token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TokenOut {
    access_token: String,
    token_type: String,
    expires_in: Option<u64>,
    refresh_token: Option<String>,
}

fn oauth2_store_name(node_id: &str) -> Result<String, String> {
    if node_id.is_empty() {
        return Err("oauth2: node id is empty".to_string());
    }
    let mut name = String::from("oauth2-v2-");
    for byte in node_id.as_bytes() {
        use std::fmt::Write;
        write!(name, "{byte:02x}").unwrap();
    }
    Ok(name)
}

fn oauth2_legacy_store_name(node_id: &str) -> Option<String> {
    // Only the identity mapping is safe to migrate; uppercase and punctuation could have collided.
    if !node_id.is_empty()
        && node_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        Some(format!("oauth2-{node_id}"))
    } else {
        None
    }
}

fn migrate_oauth2_secret(
    current: &store::EncryptedStore,
    legacy: Option<&store::EncryptedStore>,
) -> Result<(), String> {
    let Some(legacy) = legacy else {
        return Ok(());
    };
    if let Some(bytes) = legacy.load().map_err(|e| e.message())? {
        if current.load().map_err(|e| e.message())?.is_none() {
            current.save(&bytes).map_err(|e| e.message())?;
        }
        legacy.delete().map_err(|e| e.message())?;
    }
    Ok(())
}

fn oauth2_store(app: &AppHandle, node_id: &str) -> Result<store::EncryptedStore, String> {
    let secrets = app.state::<Secrets>();
    let current = secrets.store_named(&oauth2_store_name(node_id)?);
    let legacy = oauth2_legacy_store_name(node_id).map(|name| secrets.store_named(&name));
    migrate_oauth2_secret(&current, legacy.as_ref())?;
    Ok(current)
}

fn oauth2_store_secret(app: &AppHandle, node_id: &str, secret: Option<&str>) -> Result<(), String> {
    let store = oauth2_store(app, node_id)?;
    match secret {
        Some(value) if !value.is_empty() => {
            let bytes = serde_json::json!({ "secret": value });
            store
                .save(serde_json::to_vec(&bytes).map_err(|e| e.to_string())?.as_slice())
                .map_err(|e| e.message())
        }
        _ => store.delete().map_err(|e| e.message()),
    }
}

fn oauth2_load_secret(app: &AppHandle, node_id: &str) -> Result<Option<String>, String> {
    let store = oauth2_store(app, node_id)?;
    match store.load() {
        Ok(None) => Ok(None),
        Ok(Some(bytes)) => Ok(serde_json::from_slice::<serde_json::Value>(&bytes)
            .map_err(|e| format!("saved oauth2 secret could not be read: {e}"))?
            .get("secret")
            .and_then(|v| v.as_str())
            .map(String::from)),
        Err(e) => Err(e.message()),
    }
}

const MAX_OAUTH2_TOKEN_RESPONSE_BYTES: usize = 256 * 1024;

/// The token endpoint POST: form-encoded per RFC 6749, client auth in the body
/// or as an HTTP Basic header. Redirects are never followed with credentials.
async fn oauth2_token_request(config: &OAuth2Config, client_secret: Option<&str>) -> Result<TokenOut, String> {
    oauth2_token_request_with_timeout(config, client_secret, Duration::from_secs(30)).await
}

async fn oauth2_token_request_with_timeout(
    config: &OAuth2Config,
    client_secret: Option<&str>,
    timeout: Duration,
) -> Result<TokenOut, String> {
    if config.token_url.is_empty() {
        return Err("oauth2: token URL is empty".to_string());
    }
    let grant = match config.grant_type.as_str() {
        "client_credentials" => "grant_type=client_credentials".to_string(),
        "authorization_code" => {
            if config.code.is_empty() {
                return Err("oauth2: authorization code is missing".to_string());
            }
            let mut pairs = vec![
                "grant_type=authorization_code".to_string(),
                format!("code={}", urlencoded(&config.code)),
            ];
            if !config.redirect_uri.is_empty() {
                pairs.push(format!("redirect_uri={}", urlencoded(&config.redirect_uri)));
            }
            if !config.code_verifier.is_empty() {
                pairs.push(format!("code_verifier={}", urlencoded(&config.code_verifier)));
            }
            pairs.join("&")
        }
        "refresh_token" => {
            if config.refresh_token.is_empty() {
                return Err("oauth2: refresh token is missing".to_string());
            }
            format!("grant_type=refresh_token&refresh_token={}", urlencoded(&config.refresh_token))
        }
        other => return Err(format!("oauth2: unsupported grant type {other}")),
    };
    let mut form = grant;
    if !config.scope.is_empty() {
        form = format!("{form}&scope={}", urlencoded(&config.scope));
    }
    let client = reqwest::Client::builder()
        .user_agent(POSTLITE_USER_AGENT)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(timeout)
        .build()
        .map_err(|e| format!("oauth2: could not create the token client: {e}"))?;
    let mut request = client.post(&config.token_url);
    let use_basic = client_secret.is_some() && config.client_auth_in == "basic";
    if config.grant_type == "client_credentials" || config.grant_type == "refresh_token" {
        // Credential-bearing grants: the client id (and secret) ride in the
        // body unless basic auth is configured.
        if use_basic {
            request = request.basic_auth(&config.client_id, client_secret);
        } else {
            form = format!("{form}&client_id={}", urlencoded(&config.client_id));
            if let Some(secret) = client_secret {
                form = format!("{form}&client_secret={}", urlencoded(secret));
            }
        }
    } else if !config.client_id.is_empty() {
        // The code exchange carries the client id; a confidential client adds
        // its secret (body or basic per the same toggle).
        if use_basic {
            request = request.basic_auth(&config.client_id, client_secret);
        } else {
            form = format!("{form}&client_id={}", urlencoded(&config.client_id));
            if let Some(secret) = client_secret {
                form = format!("{form}&client_secret={}", urlencoded(secret));
            }
        }
    }
    request = request
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "application/json")
        .body(form);
    let mut response = request
        .send()
        .await
        .map_err(|e| {
            let reason = if e.is_timeout() {
                "timed out".to_string()
            } else {
                e.without_url().to_string()
            };
            format!("oauth2: token request failed: {reason}")
        })?;
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > MAX_OAUTH2_TOKEN_RESPONSE_BYTES as u64)
    {
        return Err("oauth2: the token response was too large".to_string());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("oauth2: could not read the token response: {}", e.without_url()))?
    {
        if bytes.len() + chunk.len() > MAX_OAUTH2_TOKEN_RESPONSE_BYTES {
            return Err("oauth2: the token response was too large".to_string());
        }
        bytes.extend_from_slice(&chunk);
    }
    let body = String::from_utf8(bytes)
        .map_err(|_| "oauth2: the token response was not valid UTF-8".to_string())?;
    if !status.is_success() {
        return Err(format!(
            "oauth2: token endpoint returned {} ({})",
            status,
            oauth2_error_code(&body)
        ));
    }
    let payload: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| "oauth2: token endpoint did not return JSON".to_string())?;
    let access_token = payload
        .get("access_token")
        .and_then(|v| v.as_str())
        .filter(|value| !value.is_empty())
        .map(String::from)
        .ok_or_else(|| "oauth2: token response carried no access_token".to_string())?;
    Ok(TokenOut {
        access_token,
        token_type: payload
            .get("token_type")
            .and_then(|v| v.as_str())
            .unwrap_or("Bearer")
            .to_string(),
        expires_in: payload.get("expires_in").and_then(|v| v.as_u64()),
        refresh_token: payload
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(String::from),
    })
}

fn sanitized_oauth2_error_code(code: &str) -> String {
    if !code.is_empty()
        && code.len() <= 64
        && code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_'))
    {
        code.to_string()
    } else {
        "request_failed".to_string()
    }
}

fn oauth2_error_code(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("error")?.as_str().map(String::from))
        .map(|code| sanitized_oauth2_error_code(&code))
        .unwrap_or_else(|| "request_failed".to_string())
}

fn urlencoded(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[tauri::command]
async fn oauth2_token(app: AppHandle, config: OAuth2Config) -> Result<TokenOut, String> {
    let secret = if config.client_id.is_empty() {
        None
    } else {
        oauth2_load_secret(&app, &config.node_id)?
    };
    oauth2_token_request(&config, secret.as_deref()).await
}

#[tauri::command]
fn oauth2_secret_set(app: AppHandle, node_id: String, secret: String) -> Result<(), String> {
    oauth2_store_secret(&app, &node_id, Some(&secret))
}

#[tauri::command]
fn oauth2_secret_get(app: AppHandle, node_id: String) -> Result<Option<String>, String> {
    oauth2_load_secret(&app, &node_id)
}

#[tauri::command]
fn oauth2_secret_clear(app: AppHandle, node_id: String) -> Result<(), String> {
    oauth2_store_secret(&app, &node_id, None)
}

/* Loopback redirect capture for the authorization-code flow: each begin owns
   an isolated listener and cancellation flag until its await consumes it. */

struct OAuthCaptureSlot {
    listener: Option<std::net::TcpListener>,
    cancelled: Arc<AtomicBool>,
}

#[derive(Default)]
struct OAuthCapture {
    slots: Mutex<HashMap<String, OAuthCaptureSlot>>,
}

impl OAuthCapture {
    fn set(&self, listener: std::net::TcpListener) -> Result<String, String> {
        let mut id = [0u8; 16];
        rand::fill(&mut id);
        let capture_id = id.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        let slot = OAuthCaptureSlot {
            listener: Some(listener),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        self.slots
            .lock()
            .map_err(|_| "oauth2: capture state was poisoned".to_string())
            .map(|mut slots| {
                slots.insert(capture_id.clone(), slot);
                capture_id
            })
    }

    fn take(&self, capture_id: &str) -> Option<(std::net::TcpListener, Arc<AtomicBool>)> {
        let mut slots = self.slots.lock().ok()?;
        let slot = slots.get_mut(capture_id)?;
        let listener = slot.listener.take()?;
        Some((listener, slot.cancelled.clone()))
    }

    fn clear(&self, capture_id: &str) {
        if let Ok(mut slots) = self.slots.lock() {
            if let Some(slot) = slots.remove(capture_id) {
                slot.cancelled.store(true, Ordering::SeqCst);
            }
        }
    }

    fn finish(&self, capture_id: &str) {
        if let Ok(mut slots) = self.slots.lock() {
            slots.remove(capture_id);
        }
    }
}

fn percent_decode(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let high = *bytes.get(index + 1).ok_or_else(|| "oauth2: incomplete percent escape".to_string())?;
                let low = *bytes.get(index + 2).ok_or_else(|| "oauth2: incomplete percent escape".to_string())?;
                let hex = |byte: u8| match byte {
                    b'0'..=b'9' => Some(byte - b'0'),
                    b'a'..=b'f' => Some(byte - b'a' + 10),
                    b'A'..=b'F' => Some(byte - b'A' + 10),
                    _ => None,
                };
                let high = hex(high).ok_or_else(|| "oauth2: invalid percent escape".to_string())?;
                let low = hex(low).ok_or_else(|| "oauth2: invalid percent escape".to_string())?;
                decoded.push(high * 16 + low);
                index += 3;
            }
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).map_err(|_| "oauth2: redirect query was not valid UTF-8".to_string())
}

#[derive(Debug, PartialEq, Eq)]
struct CapturedRedirect {
    code: String,
    state: String,
    error: Option<String>,
}

fn parse_capture_query(path: &str) -> Result<Option<CapturedRedirect>, String> {
    let query = path.split('?').nth(1).unwrap_or("");
    let mut code = String::new();
    let mut state = String::new();
    let mut error = None;
    for pair in query.split('&') {
        let (raw_key, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = percent_decode(raw_key)?;
        let value = percent_decode(raw_value)?;
        match key.as_str() {
            "code" => code = value,
            "state" => state = value,
            "error" if !value.is_empty() => error = Some(value),
            _ => {}
        }
    }
    if code.is_empty() && error.is_none() {
        return Ok(None);
    }
    Ok(Some(CapturedRedirect { code, state, error }))
}

const MAX_OAUTH2_REDIRECT_HEADER_BYTES: usize = 16 * 1024;
const OAUTH2_REDIRECT_CONNECTION_TIMEOUT: Duration = Duration::from_secs(2);

fn read_capture_request(
    stream: &mut std::net::TcpStream,
    deadline: Instant,
    connection_deadline: Instant,
    cancelled: &AtomicBool,
) -> Result<Option<CapturedRedirect>, String> {
    use std::io::{ErrorKind, Read};
    let mut received = Vec::new();
    let mut buffer = [0; 1024];
    while !received.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
        if cancelled.load(Ordering::SeqCst) {
            return Err("oauth2: authorization was cancelled".to_string());
        }
        if Instant::now() >= deadline {
            return Err("oauth2: timed out waiting for the redirect".to_string());
        }
        if Instant::now() >= connection_deadline {
            return Ok(None);
        }
        match stream.read(&mut buffer) {
            Ok(0) => return Ok(None),
            Ok(count) => {
                if received.len() + count > MAX_OAUTH2_REDIRECT_HEADER_BYTES {
                    return Err("oauth2: the redirect headers were too large".to_string());
                }
                received.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::BrokenPipe
                ) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(format!("oauth2: could not read the redirect: {error}")),
        }
    }
    let request = String::from_utf8_lossy(&received);
    let request_line = request.lines().next().unwrap_or("");
    let path = request_line.split_whitespace().nth(1).unwrap_or("");
    Ok(parse_capture_query(path).ok().flatten())
}

fn capture_authorization(
    listener: &std::net::TcpListener,
    timeout: Duration,
    cancelled: &AtomicBool,
) -> Result<CapturedRedirect, String> {
    use std::io::{ErrorKind, Write};
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("oauth2: could not configure the loopback listener: {e}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        if cancelled.load(Ordering::SeqCst) {
            return Err("oauth2: authorization was cancelled".to_string());
        }
        if Instant::now() >= deadline {
            return Err("oauth2: timed out waiting for the redirect".to_string());
        }
        let mut stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("oauth2: redirect capture failed: {error}")),
        };
        stream
            .set_nonblocking(true)
            .map_err(|e| format!("oauth2: could not configure the redirect socket: {e}"))?;
        let connection_deadline = (Instant::now() + OAUTH2_REDIRECT_CONNECTION_TIMEOUT).min(deadline);
        let Some(redirect) = read_capture_request(&mut stream, deadline, connection_deadline, cancelled)? else {
            continue;
        };
        if cancelled.load(Ordering::SeqCst) {
            return Err("oauth2: authorization was cancelled".to_string());
        }
        let message = if redirect.error.is_some() {
            "Authorization was not completed - you can close this window."
        } else {
            "Authorization captured - you can close this window."
        };
        let _ = stream.write_all(
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\nconnection: close\r\n\r\n\
                 <html><body><h3>postlite</h3><p>{message}</p></body></html>"
            )
            .as_bytes(),
        );
        return Ok(redirect);
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RedirectBegin {
    redirect_uri: String,
    port: u16,
    capture_id: String,
}

#[tauri::command]
fn oauth2_authorize_begin(app: AppHandle, port: u16) -> Result<RedirectBegin, String> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| format!("oauth2: could not bind the loopback port: {e}"))?;
    let bound = listener.local_addr().map_err(|e| e.to_string())?.port();
    let capture_id = app.state::<OAuthCapture>().set(listener)?;
    Ok(RedirectBegin {
        redirect_uri: format!("http://127.0.0.1:{bound}/callback"),
        port: bound,
        capture_id,
    })
}

#[tauri::command]
async fn oauth2_authorize_await(
    app: AppHandle,
    capture_id: String,
    expected_state: String,
) -> Result<(String, String), String> {
    let capture = app.state::<OAuthCapture>();
    let Some((listener, cancelled)) = capture.take(&capture_id) else {
        return Err("oauth2: this redirect capture is no longer running".to_string());
    };
    // spawn_blocking: the accept loop must not freeze the main thread while
    // the webview waits.
    let result = tauri::async_runtime::spawn_blocking(move || {
        capture_authorization(&listener, Duration::from_secs(120), &cancelled)
    })
    .await
    .map_err(|e| e.to_string());
    capture.finish(&capture_id);
    let redirect = result??;
    // Defense in depth: the JS flow validates too, but the capture itself
    // refuses a redirect whose state does not match the request.
    if redirect.state != expected_state {
        return Err("oauth2: state mismatch — the redirect did not originate from this request".to_string());
    }
    if let Some(error) = redirect.error {
        return Err(format!(
            "oauth2: authorization was denied ({})",
            sanitized_oauth2_error_code(&error)
        ));
    }
    Ok((redirect.code, redirect.state))
}

#[tauri::command]
fn oauth2_authorize_cancel(app: AppHandle, capture_id: String) -> Result<(), String> {
    app.state::<OAuthCapture>().clear(&capture_id);
    Ok(())
}

#[tauri::command]
fn open_in_browser(url: String) -> Result<(), String> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("open_in_browser: only http(s) URLs are allowed".to_string());
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("powershell")
            .env("POSTLITE_OPEN_URL", &url)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Process -FilePath $env:POSTLITE_OPEN_URL",
            ])
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(&url).spawn().map_err(|e| e.to_string())?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(&url).spawn().map_err(|e| e.to_string())?;
    }
    Ok(())
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

/// Filename and part Content-Type for an uploaded file, guessed from the extension.
fn file_part_meta(path: &std::path::Path) -> (String, &'static str) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    let mime = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("pdf") => "application/pdf",
        Some("zip") => "application/zip",
        Some("json") => "application/json",
        Some("csv") => "text/csv",
        Some("txt") => "text/plain",
        Some("html") | Some("htm") => "text/html",
        Some("xml") => "application/xml",
        Some("mp3") => "audio/mpeg",
        Some("mp4") => "video/mp4",
        _ => "application/octet-stream",
    };
    (name, mime)
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

// Keep workspace disk I/O and IPC serialization off the GTK event thread.
#[cfg_attr(target_os = "linux", tauri::command(async))]
#[cfg_attr(not(target_os = "linux"), tauri::command)]
fn load_workspace(app: AppHandle) -> Option<String> {
    workspace_file(&app)
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
}

#[cfg_attr(target_os = "linux", tauri::command(async))]
#[cfg_attr(not(target_os = "linux"), tauri::command)]
fn save_workspace(app: AppHandle, state: String) -> Result<(), String> {
    // Concurrent IPC calls must serialize access to the shared staging file.
    #[cfg(target_os = "linux")]
    let _save = {
        static SAVE: Mutex<()> = Mutex::new(());
        SAVE.lock().map_err(|e| e.to_string())?
    };
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
        let response = run_request(payload, &Ctx::no_jar()).await;
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
    async fn large_chunked_response_preserves_size_and_capped_bytes() {
        const CHUNK_SIZE: usize = 64 * 1024;
        const CHUNKS: usize = 1024;
        const CAP: usize = 1024;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut payload = request();
        payload.url = format!("http://{}/", listener.local_addr().unwrap());
        payload.settings = Some(HttpSettings {
            max_response_size_kb: 1,
            ..Default::default()
        });
        let server = thread::spawn(move || {
            use std::io::{BufRead, BufReader};

            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() != 0 && line != "\r\n" {
                line.clear();
            }

            stream
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n")
                .unwrap();
            let chunk = [0xff; CHUNK_SIZE];
            for _ in 0..CHUNKS {
                write!(stream, "{CHUNK_SIZE:x}\r\n").unwrap();
                stream.write_all(&chunk).unwrap();
                stream.write_all(b"\r\n").unwrap();
            }
            stream.write_all(b"0\r\n\r\n").unwrap();
        });

        let response = run_request(payload, &Ctx::no_jar()).await;
        assert!(response.error.is_none(), "{:?}", response.error);
        assert_eq!(response.status, 200);
        assert!(response.truncated);
        assert_eq!(response.size_bytes, CHUNK_SIZE * CHUNKS);
        assert_eq!(
            b64_decode(response.body_b64.as_deref().unwrap()).unwrap(),
            [0xff; CAP]
        );
        server.join().unwrap();
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
            let (url, server) = serve("HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
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
        let dir = TempDir::new("form-upload");
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

    fn oauth2_config(grant_type: &str, client_auth_in: &str) -> OAuth2Config {
        serde_json::from_value(json!({
            "nodeId": "node1", "grantType": grant_type, "tokenUrl": "",
            "clientId": "cid", "scope": "read write", "clientAuthIn": client_auth_in,
        }))
        .unwrap()
    }

    fn serve_mirror() -> (String, thread::JoinHandle<()>, std::sync::Arc<std::sync::Mutex<String>>) {
        let received = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let mirror = received.clone();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0; 4096];
            let count = stream.read(&mut buffer).unwrap();
            *mirror.lock().unwrap() = String::from_utf8_lossy(&buffer[..count]).into_owned();
            let body = r#"{"access_token":"tok-1","token_type":"Bearer","expires_in":3600,"refresh_token":"ref-1"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        (url, server, received)
    }

    #[tokio::test]
    async fn client_credentials_body_auth_sends_form_fields() {
        let (url, server, received) = serve_mirror();
        let mut config = oauth2_config("client_credentials", "body");
        config.token_url = url;
        let token = oauth2_token_request(&config, Some("sekrit")).await.unwrap();
        server.join().unwrap();
        let request = received.lock().unwrap().clone();
        assert!(request.to_ascii_lowercase().contains("content-type: application/x-www-form-urlencoded"), "{request}");
        assert!(request.contains("grant_type=client_credentials"), "{request}");
        assert!(request.contains("client_id=cid"), "{request}");
        assert!(request.contains("client_secret=sekrit"), "{request}");
        assert!(request.contains("scope=read+write"), "{request}");
        assert_eq!(token.access_token, "tok-1");
        assert_eq!(token.expires_in, Some(3600));
    }

    #[tokio::test]
    async fn client_credentials_basic_auth_sends_basic_header() {
        let (url, server, received) = serve_mirror();
        let mut config = oauth2_config("client_credentials", "basic");
        config.token_url = url;
        let token = oauth2_token_request(&config, Some("sekrit")).await.unwrap();
        server.join().unwrap();
        let request = received.lock().unwrap().clone();
        assert!(request.to_ascii_lowercase().contains("authorization: basic y2lkonnla3jpda=="), "{request}");
        assert!(!request.contains("client_secret="), "{request}");
        assert_eq!(token.access_token, "tok-1");
    }

    #[tokio::test]
    async fn authorization_code_exchange_carries_code_and_verifier() {
        let (url, server, received) = serve_mirror();
        let mut config = oauth2_config("authorization_code", "body");
        config.token_url = url;
        config.code = "abc123".into();
        config.redirect_uri = "http://127.0.0.1:9/callback".into();
        config.code_verifier = "verifier-xyz".into();
        let token = oauth2_token_request(&config, None).await.unwrap();
        server.join().unwrap();
        let request = received.lock().unwrap().clone();
        assert!(request.contains("grant_type=authorization_code"), "{request}");
        assert!(request.contains("code=abc123"), "{request}");
        assert!(request.contains("redirect_uri="), "{request}");
        assert!(request.contains("code_verifier=verifier-xyz"), "{request}");
        assert!(request.contains("client_id=cid"), "{request}");
        assert_eq!(token.refresh_token.as_deref(), Some("ref-1"));
    }

    #[tokio::test]
    async fn token_endpoint_errors_surface_status_without_response_details() {
        let body = r#"{"error":"invalid_client","error_description":"wrong-secret"}"#;
        let (url, server) = serve(format!(
            "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        ));
        let mut config = oauth2_config("client_credentials", "body");
        config.token_url = url;
        let error = oauth2_token_request(&config, Some("wrong-secret")).await.err().unwrap();
        server.join().unwrap();
        assert!(error.contains("401"), "{error}");
        assert!(error.contains("invalid_client"), "{error}");
        assert!(!error.contains("wrong-secret"), "{error}");
        assert!(!error.contains("error_description"), "{error}");
    }

    #[tokio::test]
    async fn token_endpoint_errors_do_not_reflect_untrusted_bodies() {
        let body = "invalid_client: super-secret-token";
        let (url, server) = serve(format!(
            "HTTP/1.1 400 Bad Request\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        ));
        let mut config = oauth2_config("client_credentials", "body");
        config.token_url = url;
        let error = oauth2_token_request(&config, Some("super-secret-token")).await.err().unwrap();
        server.join().unwrap();
        assert!(error.contains("request_failed"), "{error}");
        assert!(!error.contains("super-secret-token"), "{error}");
    }

    #[tokio::test]
    async fn credential_bearing_token_requests_do_not_follow_redirects() {
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let (url, server) = serve(format!(
            "HTTP/1.1 302 Found\r\nlocation: http://{}/capture\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            target.local_addr().unwrap()
        ));
        let mut config = oauth2_config("client_credentials", "body");
        config.token_url = url;
        let error = oauth2_token_request(&config, Some("sekrit")).await.err().unwrap();
        server.join().unwrap();
        assert!(error.contains("302"), "{error}");
        assert!(matches!(
            target.accept(),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
        ));
    }

    #[tokio::test]
    async fn token_requests_have_a_finite_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            thread::sleep(Duration::from_millis(150));
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}",
            );
        });
        let mut config = oauth2_config("client_credentials", "body");
        config.token_url = url;
        let error = oauth2_token_request_with_timeout(&config, Some("sekrit"), Duration::from_millis(30))
            .await
            .err()
            .unwrap();
        server.join().unwrap();
        assert!(error.to_ascii_lowercase().contains("timed out"), "{error}");
    }

    #[tokio::test]
    async fn token_responses_over_the_size_limit_are_rejected() {
        let (url, server) = serve(format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            MAX_OAUTH2_TOKEN_RESPONSE_BYTES + 1
        ));
        let mut config = oauth2_config("client_credentials", "body");
        config.token_url = url;
        let error = oauth2_token_request(&config, Some("sekrit"))
            .await
            .err()
            .unwrap();
        server.join().unwrap();
        assert!(error.contains("too large"), "{error}");
    }

    #[test]
    fn oauth2_store_names_are_injective_and_legacy_migration_is_unambiguous() {
        assert_eq!(
            oauth2_store_name("request:a").unwrap(),
            format!("oauth2-v2-{}", "request:a".as_bytes().iter().map(|byte| format!("{byte:02x}")).collect::<String>())
        );
        assert_ne!(oauth2_store_name("request:a").unwrap(), oauth2_store_name("requestA").unwrap());
        assert_ne!(oauth2_store_name("request-a").unwrap(), oauth2_store_name("requesta").unwrap());
        assert!(oauth2_store_name("").is_err());
        assert_eq!(oauth2_legacy_store_name("node1").as_deref(), Some("oauth2-node1"));
        assert_eq!(oauth2_legacy_store_name("Node1"), None);
        assert_eq!(oauth2_legacy_store_name("node:1"), None);
        assert_eq!(oauth2_legacy_store_name(""), None);
    }

    #[test]
    fn oauth2_secret_migration_moves_legacy_bytes_without_overwriting_current_data() {
        let temp = TempDir::new("oauth2-migration");
        let old_bytes = br#"{"secret":"old"}"#;
        let current_bytes = br#"{"secret":"current"}"#;
        let legacy = store::EncryptedStore::open(
            temp.path(),
            "oauth2-node1",
            store::FakeKeySource::empty(),
        );
        let current = store::EncryptedStore::open(
            temp.path(),
            &oauth2_store_name("node1").unwrap(),
            store::FakeKeySource::empty(),
        );
        legacy.save(old_bytes).unwrap();
        migrate_oauth2_secret(&current, Some(&legacy)).unwrap();
        assert_eq!(current.load().unwrap().as_deref(), Some(old_bytes.as_slice()));
        assert_eq!(legacy.load().unwrap(), None);

        legacy.save(old_bytes).unwrap();
        current.save(current_bytes).unwrap();
        migrate_oauth2_secret(&current, Some(&legacy)).unwrap();
        assert_eq!(current.load().unwrap().as_deref(), Some(current_bytes.as_slice()));
        assert_eq!(legacy.load().unwrap(), None);
    }

    #[test]
    fn per_node_secret_store_roundtrip_is_isolated() {
        let temp = TempDir::new("oauth2-test");
        let keys = store::FakeKeySource::empty();
        let open = |node_id| {
            store::EncryptedStore::open(
                temp.path(),
                &oauth2_store_name(node_id).unwrap(),
                keys.clone(),
            )
        };
        let secret = br#"{"secret":"s1"}"#;
        open("abc").save(secret).unwrap();
        assert_eq!(open("abc").load().unwrap().as_deref(), Some(secret.as_slice()));
        assert_eq!(open("def").load().unwrap(), None);
    }

    #[test]
    fn capture_parses_code_and_state() {
        assert_eq!(
            parse_capture_query("/callback?code=abc&state=xyz").unwrap(),
            Some(CapturedRedirect {
                code: "abc".to_string(),
                state: "xyz".to_string(),
                error: None,
            })
        );
        assert_eq!(
            parse_capture_query("/callback?code=a%2Bb%20c&state=s%26t%3D1").unwrap(),
            Some(CapturedRedirect {
                code: "a+b c".to_string(),
                state: "s&t=1".to_string(),
                error: None,
            })
        );
        assert_eq!(
            parse_capture_query("/callback?error=access_denied&state=xyz").unwrap(),
            Some(CapturedRedirect {
                code: String::new(),
                state: "xyz".to_string(),
                error: Some("access_denied".to_string()),
            })
        );
        assert_eq!(parse_capture_query("/callback?unrelated=1").unwrap(), None);
        assert!(parse_capture_query("/callback?code=%ZZ").is_err());
    }

    #[test]
    fn capture_returns_a_denial_without_waiting_for_the_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let capture = thread::spawn(move || {
            capture_authorization(
                &listener,
                Duration::from_secs(2),
                &AtomicBool::new(false),
            )
        });
        let mut client = std::net::TcpStream::connect(address).unwrap();
        client
            .write_all(b"GET /callback?error=access_denied&state=xyz HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(response.contains("Authorization was not completed"), "{response}");
        let redirect = capture.join().unwrap().unwrap();
        assert_eq!(redirect.state, "xyz");
        assert_eq!(redirect.error.as_deref(), Some("access_denied"));
        assert!(redirect.code.is_empty());
    }

    #[test]
    fn capture_times_out_without_waiting_for_a_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let error = capture_authorization(
            &listener,
            Duration::from_millis(40),
            &AtomicBool::new(false),
        )
        .err()
        .unwrap();
        assert!(error.contains("timed out"), "{error}");
    }

    #[test]
    fn capture_cancel_interrupts_the_wait() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let waiter = cancelled.clone();
        let handle = thread::spawn(move || {
            capture_authorization(&listener, Duration::from_secs(120), &waiter)
        });
        thread::sleep(Duration::from_millis(30));
        cancelled.store(true, Ordering::SeqCst);
        let error = handle.join().unwrap().err().unwrap();
        assert!(error.contains("cancelled"), "{error}");
    }

    #[test]
    fn capture_cancel_interrupts_a_partial_header_read() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let waiter = cancelled.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = thread::spawn(move || {
            let result = capture_authorization(&listener, Duration::from_secs(120), &waiter);
            tx.send(result).unwrap();
        });
        let mut client = std::net::TcpStream::connect(address).unwrap();
        client
            .write_all(b"GET /callback?code=abc&state=xyz HTTP/1.1\r\nHost: localhost\r\n")
            .unwrap();
        thread::sleep(Duration::from_millis(30));
        cancelled.store(true, Ordering::SeqCst);
        let error = rx.recv_timeout(Duration::from_millis(500)).unwrap().err().unwrap();
        handle.join().unwrap();
        assert!(error.contains("cancelled"), "{error}");
    }

    #[test]
    fn an_idle_partial_connection_does_not_block_the_real_redirect() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let capture = thread::spawn(move || {
            capture_authorization(
                &listener,
                Duration::from_secs(5),
                &AtomicBool::new(false),
            )
        });
        let mut idle = std::net::TcpStream::connect(address).unwrap();
        idle.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        idle.write_all(b"GET /callback?code=abc&state=xyz HTTP/1.1\r\n")
            .unwrap();
        thread::sleep(Duration::from_millis(50));
        let mut redirect = std::net::TcpStream::connect(address).unwrap();
        redirect
            .write_all(b"GET /callback?code=real&state=state HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        redirect.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        let redirect = capture.join().unwrap().unwrap();
        assert_eq!(redirect.code, "real");
        assert_eq!(redirect.state, "state");
        assert_eq!(redirect.error, None);
    }

    #[test]
    fn capture_rejects_oversized_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let capture = thread::spawn(move || {
            capture_authorization(
                &listener,
                Duration::from_secs(2),
                &AtomicBool::new(false),
            )
        });
        let mut client = std::net::TcpStream::connect(address).unwrap();
        client
            .write_all(&vec![b'x'; MAX_OAUTH2_REDIRECT_HEADER_BYTES + 1])
            .unwrap();
        let error = capture.join().unwrap().err().unwrap();
        assert!(error.contains("too large"), "{error}");
    }

    #[test]
    fn capture_slots_are_isolated_by_id() {
        let capture = OAuthCapture::default();
        let first_id = capture.set(TcpListener::bind("127.0.0.1:0").unwrap()).unwrap();
        let second_id = capture.set(TcpListener::bind("127.0.0.1:0").unwrap()).unwrap();
        assert_ne!(first_id, second_id);
        let (_, first_cancelled) = capture.take(&first_id).unwrap();
        capture.clear(&first_id);
        assert!(first_cancelled.load(Ordering::SeqCst));
        let (_, second_cancelled) = capture.take(&second_id).unwrap();
        assert!(!second_cancelled.load(Ordering::SeqCst));
        assert!(capture.take(&second_id).is_none());
    }

    #[test]
    fn capture_reads_a_delayed_loopback_redirect() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let capture = thread::spawn(move || {
            capture_authorization(
                &listener,
                Duration::from_secs(2),
                &AtomicBool::new(false),
            )
        });
        let mut client = std::net::TcpStream::connect(address).unwrap();
        client
            .write_all(b"GET /callback?code=a%2Bb&state=xyz HTTP/1.1\r\n")
            .unwrap();
        thread::sleep(Duration::from_millis(75));
        client.write_all(b"Host: localhost\r\n\r\n").unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        let redirect = capture.join().unwrap().unwrap();
        assert_eq!(redirect.code, "a+b");
        assert_eq!(redirect.state, "xyz");
        assert_eq!(redirect.error, None);
    }

    #[test]
    fn capture_reads_a_real_loopback_redirect() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let capture = thread::spawn(move || {
            capture_authorization(
                &listener,
                Duration::from_secs(2),
                &AtomicBool::new(false),
            )
        });
        let mut client = std::net::TcpStream::connect(address).unwrap();
        client
            .write_all(b"GET /callback?code=a%2Bb&state=xyz HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        let redirect = capture.join().unwrap().unwrap();
        assert_eq!(redirect.code, "a+b");
        assert_eq!(redirect.state, "xyz");
        assert_eq!(redirect.error, None);
    }
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let data = app.path().app_data_dir()?;
            let identifier = app.config().identifier.clone();
            app.manage(JarManager::new(data.join("cookies"), identifier.clone()));
            app.manage(Secrets { dir: data.join("secrets"), identifier });
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
