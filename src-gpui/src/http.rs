use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use postlite_core::session::{JarManager, Secrets};
use postlite_core::{RequestContext, RequestPayload, run_request, system_env};
use serde_json::Value;
use tokio::runtime::Runtime;

static RUNTIME: OnceLock<Result<Runtime, String>> = OnceLock::new();

pub struct Response {
    pub wire: postlite_core::ResponsePayload,
    pub pretty: Option<String>,
}

impl Response {
    pub fn text(&self, pretty: bool) -> &str {
        if pretty {
            self.pretty.as_deref().unwrap_or(&self.wire.body)
        } else {
            &self.wire.body
        }
    }
}

pub struct Session {
    pub cookies: JarManager,
    pub secrets: Arc<Secrets>,
    pub oauth: crate::oauth::OAuthSession,
    cookie_setting: Mutex<bool>,
    persist_requested: AtomicBool,
}

impl Session {
    pub fn open(profile: Option<&Path>, persist_cookies: bool) -> Self {
        let (cookies, secrets) = profile
            .and_then(crate::workspace::session_profile)
            .map_or_else(
                || (JarManager::memory_only(), Secrets::memory_only()),
                |(directory, service)| {
                    (
                        JarManager::new(directory.join("cookies"), service.clone()),
                        Secrets::new(directory.join("secrets"), service),
                    )
                },
            );
        cookies.set_persist(persist_cookies);
        let secrets = Arc::new(secrets);
        let oauth = crate::oauth::OAuthSession::new(secrets.clone());
        Self {
            cookies,
            secrets,
            oauth,
            cookie_setting: Mutex::new(persist_cookies),
            persist_requested: AtomicBool::new(persist_cookies),
        }
    }

    #[cfg(test)]
    fn memory_only() -> Self {
        Self::open(None, false)
    }

    pub fn request_cookie_setting(&self, on: bool) {
        self.persist_requested.store(on, Ordering::Release);
    }

    pub fn update_cookie_setting(&self) {
        if let Ok(mut current) = self.cookie_setting.lock() {
            let on = self.persist_requested.load(Ordering::Acquire);
            if *current != on {
                self.cookies.set_persist(on);
                *current = on;
            }
        }
    }

    pub async fn dispatch(
        self: Arc<Self>,
        mut payload: Value,
        oauth: Option<crate::oauth::Config>,
    ) -> Result<Response, String> {
        if let Some(config) = oauth {
            let token = self.oauth.acquire(config).await?;
            add_bearer(&mut payload, &token);
        }
        self.send(payload).await
    }

    async fn send(self: Arc<Self>, payload: Value) -> Result<Response, String> {
        let request: RequestPayload =
            serde_json::from_value(payload).map_err(|error| error.to_string())?;
        let settings = request.settings.as_ref();
        let workspace_id = settings
            .map(|setting| setting.workspace_id.clone())
            .unwrap_or_default();
        let manual_proxy = settings
            .and_then(|settings| settings.proxy.as_ref())
            .is_some_and(|proxy| proxy.mode == "manual");
        let context = tokio::task::spawn_blocking(move || {
            self.update_cookie_setting();
            RequestContext {
                jar: self.cookies.jar(&workspace_id),
                proxy_credentials: if manual_proxy {
                    self.secrets.proxy_credentials().ok().flatten()
                } else {
                    None
                },
                env: system_env(),
            }
        })
        .await
        .map_err(|error| error.to_string())?;
        let wire = run_request(request, &context).await;
        if let Some(error) = wire.error.as_deref() {
            return Err(error.to_owned());
        }
        let pretty = pretty(&wire.body, wire.truncated);
        Ok(Response { wire, pretty })
    }
}

pub fn add_bearer(payload: &mut Value, token: &str) {
    if let Some(headers) = payload["headers"].as_array_mut() {
        headers.push(
            serde_json::json!({ "key": "Authorization", "value": format!("Bearer {token}") }),
        );
    }
}

pub fn runtime() -> Result<&'static Runtime, String> {
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)
}

pub fn pretty(body: &str, truncated: bool) -> Option<String> {
    if !truncated && body.len() <= 256 * 1024 {
        serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|value| serde_json::to_string_pretty(&value).ok())
            .filter(|pretty| pretty.len() <= 2_000_000)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    fn serve(
        count: usize,
        response: impl Fn(usize, &str) -> (u16, &str, String) + Send + 'static,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for attempt in 0..count {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                    head.push_str(&line);
                }
                let (status, headers, body) = response(attempt, &head);
                write!(stream, "HTTP/1.1 {status} Response\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}", body.len()).unwrap();
            }
        });
        (endpoint, server)
    }

    fn header<'a>(head: &'a str, name: &str) -> &'a str {
        head.lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map_or("", |(_, value)| value.trim())
    }

    #[test]
    fn counts_large_chunked_response_and_keeps_only_the_capped_binary_bytes() {
        const CHUNK_BYTES: usize = 64 * 1024;
        const CHUNKS: usize = 256;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let timeout = Some(std::time::Duration::from_secs(5));
            stream.set_read_timeout(timeout).unwrap();
            stream.set_write_timeout(timeout).unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() != 0 && line != "\r\n" {
                line.clear();
            }
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            let chunk = [0xff; CHUNK_BYTES];
            for _ in 0..CHUNKS {
                write!(stream, "{CHUNK_BYTES:x}\r\n").unwrap();
                stream.write_all(&chunk).unwrap();
                stream.write_all(b"\r\n").unwrap();
            }
            stream.write_all(b"0\r\n\r\n").unwrap();
        });
        let session = Arc::new(Session::memory_only());
        let response = runtime().unwrap().block_on(session.dispatch(
            json!({"method":"GET","url":url,"headers":[],"settings":{"max_response_size_kb":1}}),
            None,
        )).unwrap().wire;
        assert_eq!(response.status, 200);
        assert_eq!(response.size_bytes, CHUNK_BYTES * CHUNKS);
        assert!(response.truncated);
        assert_eq!(
            postlite_core::b64_decode(response.body_b64.as_deref().unwrap()).unwrap(),
            [0xff; 1024]
        );
        server.join().unwrap();
    }

    #[test]
    fn sends_workspace_cookies_and_respects_an_explicit_cookie_header() {
        let (endpoint, server) = serve(5, |attempt, head| {
            let set = if attempt == 0 {
                "Set-Cookie: session=first; Path=/; HttpOnly\r\n"
            } else {
                ""
            };
            (200, set, header(head, "cookie").into())
        });
        let session = Arc::new(Session::memory_only());
        let send = |workspace_id, headers| {
            runtime()
                .unwrap()
                .block_on(session.clone().dispatch(
                    json!({"method":"GET","url":endpoint,
            "headers":headers,"settings":{"workspace_id":workspace_id}}),
                    None,
                ))
                .unwrap()
                .wire
                .body
        };
        assert_eq!(send("first", json!([])), "");
        assert_eq!(send("first", json!([])), "session=first");
        assert_eq!(send("second", json!([])), "");
        assert_eq!(
            send("first", json!([{"key":"Cookie","value":"session=manual"}])),
            "session=manual"
        );
        session.cookies.purge("first").unwrap();
        assert_eq!(send("first", json!([])), "");
        server.join().unwrap();
    }

    #[test]
    fn sends_saved_manual_proxy_credentials_and_honors_the_bypass() {
        let (proxy, proxy_server) = serve(1, |_, head| {
            assert!(head.starts_with("GET http://example.test/ HTTP/1.1"));
            (200, "", header(head, "proxy-authorization").into())
        });
        let (target, target_server) = serve(1, |_, head| {
            assert!(head.starts_with("GET / HTTP/1.1"));
            (200, "", header(head, "proxy-authorization").into())
        });
        let session = Arc::new(Session::memory_only());
        session
            .secrets
            .set_proxy_credentials(&postlite_core::ProxyCredentials {
                username: "proxy-user".into(),
                password: "password".into(),
            })
            .unwrap();
        let send = |url: &str| {
            let payload = json!({"method":"GET","url":url,"headers":[],
                "settings":{"proxy":{"mode":"manual","url":proxy,"bypass":"127.0.0.1"}}});
            runtime()
                .unwrap()
                .block_on(session.clone().dispatch(payload, None))
                .unwrap()
                .wire
                .body
        };
        assert_eq!(
            send("http://example.test/"),
            "Basic cHJveHktdXNlcjpwYXNzd29yZA=="
        );
        assert_eq!(send(&target), "");
        proxy_server.join().unwrap();
        target_server.join().unwrap();
    }

    #[test]
    fn dispatch_acquires_and_reuses_inherited_oauth_and_stops_on_token_failure() {
        let request = json!({"id":"request","auth":{"type":"inherit"}});
        let owner = |token_url: &str| {
            json!({
                "id":"collection","auth":{"type":"oauth2","oauth2":{"grantType":"client_credentials",
                    "tokenUrl": token_url,"clientId":"client","clientAuthIn":"basic"}}
            })
        };
        let folder = json!({"id":"folder","auth":{"type":"inherit"}});
        let lookup = |owner: &Value| {
            crate::prepare::oauth_config(&request, Some(owner), &[&folder], "workspace").unwrap()
        };
        let session = Arc::new(Session::memory_only());
        session
            .secrets
            .set_oauth_secret(
                &crate::prepare::scoped_owner("workspace", "collection"),
                Some("secret"),
            )
            .unwrap();
        let (token, token_server) = serve(1, |_, head| {
            assert_eq!(header(head, "authorization"), "Basic Y2xpZW50OnNlY3JldA==");
            (
                200,
                "Content-Type: application/json\r\n",
                r#"{"access_token":"fixture","expires_in":3600}"#.into(),
            )
        });
        let (target, target_server) =
            serve(2, |_, head| (200, "", header(head, "authorization").into()));
        let send = |url, oauth| {
            runtime().unwrap().block_on(session.clone().dispatch(
            json!({"method":"GET","url":url,"headers":[],"settings":{"request_timeout_ms":1000}}), oauth
        ))
        };
        for _ in 0..2 {
            assert_eq!(
                send(target.clone(), lookup(&owner(&token)))
                    .unwrap()
                    .wire
                    .body,
                "Bearer fixture"
            );
        }
        token_server.join().unwrap();
        target_server.join().unwrap();

        let (bad_token, server) = serve(1, |_, _| (401, "", "secret server body".into()));
        let unused_target = TcpListener::bind("127.0.0.1:0").unwrap();
        unused_target.set_nonblocking(true).unwrap();
        let outcome = send(
            format!("http://{}/", unused_target.local_addr().unwrap()),
            lookup(&owner(&bad_token)),
        );
        assert!(
            matches!(outcome, Err(ref error) if error.contains("401") && !error.contains("secret server body"))
        );
        assert!(
            matches!(unused_target.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
        server.join().unwrap();
    }
}
