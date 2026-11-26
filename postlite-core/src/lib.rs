//! Shared, window-independent HTTP transport for the Postlite desktop apps.

use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CACHE_CONTROL, PRAGMA};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone)]
pub struct HeaderPair {
    pub key: String,
    pub value: String,
}

#[derive(Deserialize)]
pub struct FormPair {
    pub key: String,
    pub value: String,
    /// "file" reads `value` as a disk path and uploads its bytes; anything else is a text part.
    #[serde(default, rename = "type")]
    pub kind: String,
}

#[derive(Deserialize)]
pub struct RequestPayload {
    pub method: String,
    pub url: String,
    pub headers: Vec<HeaderPair>,
    pub body: Option<String>,
    #[serde(default)]
    pub form: Vec<FormPair>,
    #[serde(default)]
    pub settings: Option<HttpSettings>,
}

#[derive(Deserialize)]
#[serde(default)]
pub struct HttpSettings {
    // Signed inputs let negative values return the same response errors as other invalid ranges.
    pub request_timeout_ms: i64,
    pub follow_redirects: bool,
    pub max_redirects: i64,
    pub ssl_verify: bool,
    pub disable_cache: bool,
    pub max_response_size_kb: i64,
    /// The workspace whose cookie jar this request belongs to. Empty means the request is not
    /// inside a workspace, and then no jar is attached at all.
    pub workspace_id: String,
    /// `None` = direct, unchanged behaviour. Credentials are deliberately not here: they live in
    /// the `global` encrypted store (§8) and are injected through `RequestContext`, never through
    /// this JSON.
    pub proxy: Option<ProxySettings>,
}

/// The non-secret half of the proxy setting (§9). `username`/`password` are stored separately.
#[derive(Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct ProxySettings {
    pub mode: String,
    pub url: String,
    pub bypass: String,
}

/// The secret half, kept in the `global` encrypted store and stripped from every workspace save.
#[derive(Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct ProxyCredentials {
    pub username: String,
    pub password: String,
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
    pub fn validate(&self) -> Result<(), String> {
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
pub struct ResponsePayload {
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<HeaderPair>,
    pub body: String,
    /// Base64 of the capped body when it is not valid UTF-8 (binary content);
    /// `None` for text bodies, where `body` already is the exact bytes. Save
    /// response writes this so binary files survive the IPC round-trip.
    pub body_b64: Option<String>,
    pub truncated: bool,
    pub elapsed_ms: u128,
    pub size_bytes: usize,
    pub error: Option<String>,
    /// A non-error line the UI should surface, e.g. "no proxy in environment" (§9). Distinct from
    /// `error`: the request still ran.
    pub note: Option<String>,
}

const MAX_BODY_BYTES: usize = 2_000_000;
pub const POSTLITE_USER_AGENT: &str = "Postlite";

/// An environment reader, injected so the environment-proxy lookup can be faked in tests without
/// mutating the process environment (which would race across parallel tests).
pub type EnvReader = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

pub fn system_env() -> EnvReader {
    Arc::new(|name: &str| std::env::var(name).ok())
}

/// Runtime configuration supplied by the desktop host.
pub struct RequestContext<J = reqwest::cookie::Jar> {
    pub jar: Option<Arc<J>>,
    pub proxy_credentials: Option<ProxyCredentials>,
    pub env: EnvReader,
}

impl<J> RequestContext<J> {
    /// Send without persistence or application-managed credentials.
    pub fn no_jar() -> Self {
        Self {
            jar: None,
            proxy_credentials: None,
            env: system_env(),
        }
    }
}

/// Execute a request on the caller's Tokio runtime without desktop state.
pub async fn send(request: RequestPayload) -> ResponsePayload {
    run_request(request, &RequestContext::<reqwest::cookie::Jar>::no_jar()).await
}

/// The proxy a send should use, plus an optional note. `None` proxy means direct.
pub struct ResolvedProxy {
    pub proxy: Option<reqwest::Proxy>,
    pub note: Option<String>,
}

/// Turn the proxy setting into a reqwest `Proxy` (§9). Manual builds from the url and applies the
/// bypass list and any stored credentials; environment reads the variables ourselves because
/// reqwest will not (§2), and says so when it finds nothing.
pub fn resolve_proxy(
    settings: Option<&ProxySettings>,
    credentials: Option<&ProxyCredentials>,
    env: &EnvReader,
    target_url: &str,
) -> Result<ResolvedProxy, String> {
    let env = env.as_ref();
    let Some(settings) = settings else {
        return Ok(ResolvedProxy {
            proxy: None,
            note: None,
        });
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
            Ok(ResolvedProxy {
                proxy: Some(proxy),
                note: None,
            })
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
            Ok(ResolvedProxy {
                proxy: Some(proxy),
                note: None,
            })
        }
        other => Err(format!("unknown proxy mode: {other}")),
    }
}

pub async fn run_request<J: reqwest::cookie::CookieStore + 'static>(
    request: RequestPayload,
    ctx: &RequestContext<J>,
) -> ResponsePayload {
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
    if request.form.iter().any(|field| {
        !field.key.trim().is_empty() && (field.kind != "file" || !field.value.trim().is_empty())
    }) {
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
                        return with_error(
                            empty,
                            format!("could not read file for \"{}\": {e}", field.key),
                        );
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

    // Count all decoded bytes for size reporting while retaining only the configured preview.
    let mut preview = Vec::new();
    let mut size_bytes = 0usize;
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(error) => return with_error(empty, format!("failed reading body: {error}")),
        };
        size_bytes = size_bytes.saturating_add(chunk.len());
        let remaining = max_body_bytes - preview.len();
        preview.extend_from_slice(&chunk[..remaining.min(chunk.len())]);
    }
    let truncated = size_bytes > max_body_bytes;
    let body = String::from_utf8_lossy(&preview).to_string();
    // Text bodies are exact in `body` (lossy of valid UTF-8 is identity); only
    // binary content needs the base64 escape hatch for byte-faithful saves.
    let body_b64 = match std::str::from_utf8(&preview) {
        Ok(_) => None,
        Err(_) => Some(b64_encode(&preview)),
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
pub fn b64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(chunk[0]) << 16) | (u32::from(second) << 8) | u32::from(third);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

pub fn b64_decode(text: &str) -> Result<Vec<u8>, String> {
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

/// Filename and part Content-Type for an uploaded file, guessed from the extension.
pub fn file_part_meta(path: &std::path::Path) -> (String, &'static str) {
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
