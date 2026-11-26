//! Profile-scoped cookie, credential, and OAuth support shared by the desktop apps.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{jar, store, ProxyCredentials, POSTLITE_USER_AGENT};

pub use crate::jar::{CookieRow, PostliteJar};

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
pub struct CookieSnapshot {
    pub rows: Vec<jar::CookieRow>,
    pub note: Option<String>,
    pub persisting: bool,
}

/// Which cookies `cookie_delete` removes. A field left out matches anything, so `{}` clears the
/// jar and `{domain}` clears one domain.
#[derive(Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct CookieFilter {
    pub domain: Option<String>,
    pub path: Option<String>,
    pub name: Option<String>,
}

/// One open jar per workspace id, kept for the life of the window so cookie state survives
/// between requests and each workspace's file is read once.
pub struct JarManager {
    storage: Option<EncryptedStorage>,
    jars: Mutex<HashMap<String, OpenedJar>>,
    /// The global `Persist cookies` switch (§11). Applies to jars opened later and, through
    /// `set_persist`, to every jar already open.
    persist: AtomicBool,
}

struct EncryptedStorage {
    dir: PathBuf,
    keys: Box<dyn Fn(&str) -> Arc<dyn store::KeySource> + Send + Sync>,
}

impl EncryptedStorage {
    fn keyring(dir: PathBuf, identifier: String) -> Self {
        Self {
            dir,
            keys: Box::new(
                move |name| match store::KeyringSource::new(&identifier, name) {
                    Ok(source) => Arc::new(source),
                    Err(_) => Arc::new(store::NoSecretStoreKeys),
                },
            ),
        }
    }

    fn open(&self, name: &str) -> store::EncryptedStore {
        store::EncryptedStore::open(&self.dir, name, (self.keys)(name))
    }
}

impl JarManager {
    /// Production: each workspace's data key lives in the OS secret store under this app's own
    /// identifier, so an isolated build can never read or delete another build's credentials.
    pub fn new(dir: PathBuf, identifier: String) -> Self {
        Self {
            storage: Some(EncryptedStorage::keyring(dir, identifier)),
            jars: Mutex::new(HashMap::new()),
            persist: AtomicBool::new(true),
        }
    }

    /// Keep each workspace's cookies in memory without accessing disk or the OS keyring.
    pub fn memory_only() -> Self {
        Self {
            storage: None,
            jars: Mutex::new(HashMap::new()),
            persist: AtomicBool::new(false),
        }
    }

    /// Save pending writes before switching workspaces or closing the app.
    pub fn flush_all(&self) {
        for opened in self.jars.lock().unwrap().values() {
            opened.jar.flush();
        }
    }

    pub fn jar(&self, workspace_id: &str) -> Option<Arc<jar::PostliteJar>> {
        self.opened(workspace_id).map(|opened| opened.jar.clone())
    }

    /// What the drawer shows: the live rows, why the jar is not persisting if it isn't, and whether
    /// persistence is even switched on.
    pub fn snapshot(&self, workspace_id: &str) -> Result<CookieSnapshot, String> {
        let opened = self
            .opened(workspace_id)
            .ok_or("cookie list needs a workspace id")?;
        Ok(CookieSnapshot {
            rows: opened.jar.rows(),
            note: opened.note.clone(),
            persisting: opened.jar.persists(),
        })
    }

    pub fn remove(&self, workspace_id: &str, filter: &CookieFilter) -> Result<usize, String> {
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
    pub fn purge(&self, workspace_id: &str) -> Result<(), String> {
        let opened = self
            .opened(workspace_id)
            .ok_or("cookie purge needs a workspace id")?;
        opened.jar.clear_all();
        Ok(())
    }

    /// Apply the global `Persist cookies` switch to every jar already open; jars opened later read
    /// the same flag in `opened`. Turning it off drops each jar's saved file so nothing resurrects.
    pub fn set_persist(&self, on: bool) {
        let jars = self.jars.lock().unwrap();
        self.persist.store(on, Ordering::Relaxed);
        for opened in jars.values() {
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

        let open = match self.storage.as_ref() {
            Some(storage) if is_safe_workspace_id(workspace_id) => PostliteJar::open(
                &storage.dir,
                workspace_id,
                (storage.keys)(workspace_id),
                self.persist.load(Ordering::Relaxed),
            ),
            Some(_) => jar::JarOpen::Empty(
                PostliteJar::memory_only(),
                "cookies are kept in memory for this workspace id".into(),
            ),
            None => jar::JarOpen::Empty(
                PostliteJar::memory_only(),
                "cookies are kept in memory in this session".into(),
            ),
        };
        let (jar, note) = match open {
            jar::JarOpen::Restored(jar) => (jar, None),
            jar::JarOpen::Empty(jar, note) => (jar, Some(note)),
        };
        let opened = OpenedJar { jar, note };
        jars.insert(workspace_id.to_owned(), opened.clone());
        Some(opened)
    }
}

// Keep the original filenames for generated IDs and reject ambiguous path components
// while still giving imported workspaces a usable session jar.
fn is_safe_workspace_id(id: &str) -> bool {
    if id.len() > 240
        || id.starts_with('.')
        || id.ends_with('.')
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-.".contains(&b))
    {
        return false;
    }
    let first = id.split('.').next().unwrap_or_default();
    !matches!(first, "" | "con" | "prn" | "aux" | "nul")
        && !((first.starts_with("com") || first.starts_with("lpt"))
            && first.len() == 4
            && first.as_bytes()[3].is_ascii_digit())
}

/// Proxy and OAuth credentials for one app profile.
#[derive(Clone)]
pub struct Secrets {
    storage: Arc<Mutex<SecretStorage>>,
}

enum SecretStorage {
    Encrypted(EncryptedStorage),
    Memory {
        proxy: Option<ProxyCredentials>,
        oauth: HashMap<String, String>,
    },
}

impl Secrets {
    /// Open profile-specific credential stores lazily. Their encryption keys live in the OS keyring.
    pub fn new(dir: PathBuf, identifier: String) -> Self {
        Self {
            storage: Arc::new(Mutex::new(SecretStorage::Encrypted(
                EncryptedStorage::keyring(dir, identifier),
            ))),
        }
    }

    /// Use ephemeral credentials without accessing disk or the OS keyring.
    pub fn memory_only() -> Self {
        Self {
            storage: Arc::new(Mutex::new(SecretStorage::Memory {
                proxy: None,
                oauth: HashMap::new(),
            })),
        }
    }

    pub fn proxy_credentials(&self) -> Result<Option<ProxyCredentials>, String> {
        match &*self.storage.lock().unwrap() {
            SecretStorage::Encrypted(storage) => load_credentials(&storage.open("global")),
            SecretStorage::Memory { proxy, .. } => Ok(proxy.clone()),
        }
    }

    pub fn set_proxy_credentials(&self, credentials: &ProxyCredentials) -> Result<(), String> {
        match &mut *self.storage.lock().unwrap() {
            SecretStorage::Encrypted(storage) => {
                save_credentials(&storage.open("global"), credentials)
            }
            SecretStorage::Memory { proxy, .. } => {
                *proxy = Some(credentials.clone());
                Ok(())
            }
        }
    }

    pub fn oauth_secret(&self, node_id: &str) -> Result<Option<String>, String> {
        let name = oauth2_store_name(node_id)?;
        match &*self.storage.lock().unwrap() {
            SecretStorage::Encrypted(storage) => {
                let store = oauth2_store(storage, node_id, &name)?;
                match store.load() {
                    Ok(None) => Ok(None),
                    Ok(Some(bytes)) => Ok(serde_json::from_slice::<serde_json::Value>(&bytes)
                        .map_err(|e| format!("saved oauth2 secret could not be read: {e}"))?
                        .get("secret")
                        .and_then(|value| value.as_str())
                        .map(String::from)),
                    Err(e) => Err(e.message()),
                }
            }
            SecretStorage::Memory { oauth, .. } => Ok(oauth.get(node_id).cloned()),
        }
    }

    pub fn set_oauth_secret(&self, node_id: &str, secret: Option<&str>) -> Result<(), String> {
        let name = oauth2_store_name(node_id)?;
        let secret = secret.filter(|value| !value.is_empty());
        match &mut *self.storage.lock().unwrap() {
            SecretStorage::Encrypted(storage) => {
                let store = oauth2_store(storage, node_id, &name)?;
                match secret {
                    Some(value) => {
                        let bytes = serde_json::json!({ "secret": value });
                        store
                            .save(&serde_json::to_vec(&bytes).map_err(|e| e.to_string())?)
                            .map_err(|e| e.message())
                    }
                    None => store.delete().map_err(|e| e.message()),
                }
            }
            SecretStorage::Memory { oauth, .. } => {
                if let Some(value) = secret {
                    oauth.insert(node_id.to_owned(), value.to_owned());
                } else {
                    oauth.remove(node_id);
                }
                Ok(())
            }
        }
    }

    /// Exchange a grant using the client secret from this profile, when one was configured.
    pub async fn exchange(&self, config: &OAuth2Config) -> Result<TokenOut, String> {
        let secret = if config.client_id.is_empty() {
            None
        } else {
            let secrets = self.clone();
            let owner = config.node_id.clone();
            tokio::task::spawn_blocking(move || secrets.oauth_secret(&owner))
                .await
                .map_err(|error| format!("oauth2: could not load the client secret: {error}"))??
        };
        oauth2_token_request(config, secret.as_deref()).await
    }
}

fn oauth2_store(
    storage: &EncryptedStorage,
    node_id: &str,
    name: &str,
) -> Result<store::EncryptedStore, String> {
    let current = storage.open(name);
    let legacy = oauth2_legacy_store_name(node_id).map(|name| storage.open(&name));
    migrate_oauth2_secret(&current, legacy.as_ref())?;
    Ok(current)
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

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuth2Config {
    #[serde(default)]
    pub node_id: String,
    #[serde(default)]
    pub grant_type: String,
    #[serde(default)]
    pub token_url: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub client_auth_in: String, // "body" (default) | "basic"
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub redirect_uri: String,
    #[serde(default)]
    pub code_verifier: String,
    #[serde(default)]
    pub refresh_token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenOut {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: Option<u64>,
    pub refresh_token: Option<String>,
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

const MAX_OAUTH2_TOKEN_RESPONSE_BYTES: usize = 256 * 1024;

/// The token endpoint POST: form-encoded per RFC 6749, client auth in the body
/// or as an HTTP Basic header. Redirects are never followed with credentials.
async fn oauth2_token_request(
    config: &OAuth2Config,
    client_secret: Option<&str>,
) -> Result<TokenOut, String> {
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
                pairs.push(format!(
                    "code_verifier={}",
                    urlencoded(&config.code_verifier)
                ));
            }
            pairs.join("&")
        }
        "refresh_token" => {
            if config.refresh_token.is_empty() {
                return Err("oauth2: refresh token is missing".to_string());
            }
            format!(
                "grant_type=refresh_token&refresh_token={}",
                urlencoded(&config.refresh_token)
            )
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
    let mut response = request.send().await.map_err(|e| {
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
    while let Some(chunk) = response.chunk().await.map_err(|e| {
        format!(
            "oauth2: could not read the token response: {}",
            e.without_url()
        )
    })? {
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
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/* Loopback redirect capture for the authorization-code flow: each begin owns
an isolated listener and cancellation flag until its await consumes it. */

struct OAuthCaptureSlot {
    listener: Option<std::net::TcpListener>,
    cancelled: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct OAuthCapture {
    slots: Mutex<HashMap<String, OAuthCaptureSlot>>,
}

impl OAuthCapture {
    pub fn begin(&self, port: u16) -> Result<RedirectBegin, String> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", port))
            .map_err(|e| format!("oauth2: could not bind the loopback port: {e}"))?;
        let bound = listener.local_addr().map_err(|e| e.to_string())?.port();
        let capture_id = self.set(listener)?;
        Ok(RedirectBegin {
            redirect_uri: format!("http://127.0.0.1:{bound}/callback"),
            port: bound,
            capture_id,
        })
    }

    pub async fn await_redirect(
        &self,
        capture_id: &str,
        expected_state: &str,
    ) -> Result<(String, String), String> {
        let Some((listener, cancelled)) = self.take(capture_id) else {
            return Err("oauth2: this redirect capture is no longer running".to_string());
        };
        // Keep the loopback wait off the async executor's worker threads.
        let result = tokio::task::spawn_blocking(move || {
            capture_authorization(&listener, Duration::from_secs(120), &cancelled)
        })
        .await
        .map_err(|e| e.to_string());
        self.finish(capture_id);
        let redirect = result??;
        if redirect.state != expected_state {
            return Err(
                "oauth2: state mismatch — the redirect did not originate from this request"
                    .to_string(),
            );
        }
        if let Some(error) = redirect.error {
            return Err(format!(
                "oauth2: authorization was denied ({})",
                sanitized_oauth2_error_code(&error)
            ));
        }
        Ok((redirect.code, redirect.state))
    }

    fn set(&self, listener: std::net::TcpListener) -> Result<String, String> {
        let mut id = [0u8; 16];
        rand::fill(&mut id);
        let capture_id = id
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
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

    pub fn cancel(&self, capture_id: &str) {
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
                let high = *bytes
                    .get(index + 1)
                    .ok_or_else(|| "oauth2: incomplete percent escape".to_string())?;
                let low = *bytes
                    .get(index + 2)
                    .ok_or_else(|| "oauth2: incomplete percent escape".to_string())?;
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
                    ErrorKind::ConnectionReset
                        | ErrorKind::ConnectionAborted
                        | ErrorKind::BrokenPipe
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
        let connection_deadline =
            (Instant::now() + OAUTH2_REDIRECT_CONNECTION_TIMEOUT).min(deadline);
        let Some(redirect) =
            read_capture_request(&mut stream, deadline, connection_deadline, cancelled)?
        else {
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
pub struct RedirectBegin {
    pub redirect_uri: String,
    pub port: u16,
    pub capture_id: String,
}

pub fn open_in_browser(url: &str) -> Result<(), String> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("open_in_browser: only http(s) URLs are allowed".to_string());
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("powershell")
            .env("POSTLITE_OPEN_URL", url)
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
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
