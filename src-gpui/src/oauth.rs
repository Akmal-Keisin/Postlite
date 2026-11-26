use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use postlite_core::session::{OAuth2Config, OAuthCapture, Secrets, TokenOut, open_in_browser};
use rand::{TryRngCore, rngs::OsRng};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex as AsyncMutex, Notify, oneshot};
use url::Url;

const CANCELLED: &str = "authorization was cancelled";

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Grant {
    ClientCredentials,
    AuthorizationCode,
}

impl Grant {
    fn name(self) -> &'static str {
        match self {
            Self::ClientCredentials => "client_credentials",
            Self::AuthorizationCode => "authorization_code",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct TokenConfig {
    grant: Grant,
    token_url: String,
    client_id: String,
    scope: String,
    client_auth_in: String,
}

#[derive(Clone)]
pub struct Config {
    owner: String,
    token: TokenConfig,
    authorize_url: String,
    redirect_port: u16,
}

impl Config {
    pub fn from_auth(owner: &str, oauth: &Value) -> Result<Self, String> {
        if owner.is_empty() {
            return Err("OAuth 2.0 credentials have no owner".into());
        }
        let field = |key: &str| oauth[key].as_str().unwrap_or_default().to_owned();
        let grant = match oauth["grantType"].as_str().unwrap_or("client_credentials") {
            "client_credentials" => Grant::ClientCredentials,
            "authorization_code" => Grant::AuthorizationCode,
            _ => return Err("unsupported OAuth 2.0 grant type".into()),
        };
        let client_auth_in = if oauth["clientAuthIn"] == "basic" {
            "basic"
        } else {
            "body"
        };
        let redirect_port = match (grant, &oauth["redirectPort"]) {
            (Grant::ClientCredentials, _) | (_, Value::Null) => 0,
            (_, Value::String(port)) if port.trim().is_empty() => 0,
            (_, Value::String(port)) => port.trim().parse().map_err(|_| "invalid redirect port")?,
            (_, port) => port
                .as_u64()
                .and_then(|port| u16::try_from(port).ok())
                .ok_or("invalid redirect port")?,
        };
        Ok(Self {
            owner: owner.to_owned(),
            token: TokenConfig {
                grant,
                token_url: field("tokenUrl"),
                client_id: field("clientId"),
                scope: field("scope"),
                client_auth_in: client_auth_in.to_owned(),
            },
            authorize_url: field("authorizeUrl"),
            redirect_port,
        })
    }

    fn exchange_config(&self) -> OAuth2Config {
        OAuth2Config {
            node_id: self.owner.clone(),
            grant_type: self.token.grant.name().into(),
            token_url: self.token.token_url.clone(),
            client_id: self.token.client_id.clone(),
            scope: self.token.scope.clone(),
            client_auth_in: self.token.client_auth_in.clone(),
            ..OAuth2Config::default()
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum Status {
    Exchanging,
    WaitingForBrowser {
        authorization_url: String,
        redirect_uri: String,
    },
    Ready,
    Failed(String),
    Cancelled,
}

#[derive(Clone)]
struct CachedToken {
    access_token: String,
    expires_at: Option<Instant>,
    refresh_token: Option<String>,
}

impl CachedToken {
    fn new(token: TokenOut, previous_refresh: Option<String>) -> Self {
        let now = Instant::now();
        Self {
            access_token: token.access_token,
            expires_at: token.expires_in.map(|seconds| {
                now.checked_add(Duration::from_secs(seconds.saturating_sub(60)))
                    .unwrap_or(now)
            }),
            refresh_token: token.refresh_token.or(previous_refresh),
        }
    }

    fn valid(&self) -> bool {
        self.expires_at
            .is_none_or(|expires| expires > Instant::now())
    }
}

struct Pending {
    capture_id: String,
    redirect_uri: String,
    expected_state: String,
    manual: Option<oneshot::Sender<String>>,
}

#[derive(Default)]
struct Owner {
    gate: Arc<AsyncMutex<()>>,
    generation: u64,
    cache: HashMap<TokenConfig, CachedToken>,
    active: Option<Arc<Notify>>,
    pending: Option<Pending>,
    status: Option<Status>,
}

pub struct OAuthSession {
    secrets: Arc<Secrets>,
    capture: OAuthCapture,
    owners: Mutex<HashMap<String, Owner>>,
}

impl OAuthSession {
    pub fn new(secrets: Arc<Secrets>) -> Self {
        Self {
            secrets,
            capture: OAuthCapture::default(),
            owners: Mutex::default(),
        }
    }

    pub fn status(&self, owner: &str) -> Option<Status> {
        self.owners.lock().unwrap().get(owner)?.status.clone()
    }

    pub fn cached(&self, config: &Config) -> Option<String> {
        let owners = self.owners.lock().unwrap();
        let token = owners.get(&config.owner)?.cache.get(&config.token)?;
        token.valid().then(|| token.access_token.clone())
    }

    pub fn cancel(&self, owner: &str) {
        self.stop(owner, false);
    }

    pub fn invalidate(&self, owner: &str) {
        self.stop(owner, true);
    }

    fn stop(&self, owner: &str, clear_cache: bool) {
        let mut owners = self.owners.lock().unwrap();
        let Some(owner) = owners.get_mut(owner) else {
            return;
        };
        owner.generation = owner.generation.wrapping_add(1);
        if clear_cache {
            owner.cache.clear();
            owner.status = None;
        }
        if let Some(active) = owner.active.take() {
            owner.status = Some(Status::Cancelled);
            active.notify_one();
        }
        if let Some(pending) = owner.pending.take() {
            self.capture.cancel(&pending.capture_id);
        }
    }

    pub fn submit_code(&self, owner: &str, text: &str) -> Result<(), String> {
        let mut owners = self.owners.lock().unwrap();
        let pending = owners
            .get_mut(owner)
            .and_then(|owner| owner.pending.as_mut())
            .ok_or("start Authorize before pasting a code")?;
        let code = pasted_code(text, &pending.redirect_uri, &pending.expected_state)?;
        pending
            .manual
            .take()
            .ok_or("authorization code has already been submitted")?
            .send(code)
            .map_err(|_| CANCELLED.into())
    }

    pub async fn acquire(&self, config: Config) -> Result<String, String> {
        self.run(config, false, open_in_browser).await
    }

    pub async fn authorize(&self, config: Config) -> Result<String, String> {
        let force = config.token.grant == Grant::AuthorizationCode;
        self.run(config, force, open_in_browser).await
    }

    async fn run(
        &self,
        config: Config,
        force: bool,
        open_browser: impl FnOnce(&str) -> Result<(), String>,
    ) -> Result<String, String> {
        let (gate, generation) = {
            let mut owners = self.owners.lock().unwrap();
            let owner = owners.entry(config.owner.clone()).or_default();
            (owner.gate.clone(), owner.generation)
        };
        let _exclusive = gate.lock().await;
        let (cached, cancelled) = {
            let mut owners = self.owners.lock().unwrap();
            let owner = owners.get_mut(&config.owner).unwrap();
            if owner.generation != generation {
                return Err(CANCELLED.into());
            }
            let cached = (!force).then(|| owner.cache.get(&config.token)).flatten();
            if let Some(token) = cached.filter(|token| token.valid()) {
                owner.status = Some(Status::Ready);
                return Ok(token.access_token.clone());
            }
            let cancelled = Arc::new(Notify::new());
            owner.active = Some(cancelled.clone());
            owner.status = Some(Status::Exchanging);
            (cached.cloned(), cancelled)
        };
        let attempt = Attempt {
            session: self,
            owner: &config.owner,
            generation,
        };
        let result = tokio::select! {
            biased;
            _ = cancelled.notified() => Err(CANCELLED.into()),
            result = attempt.exchange(&config, cached, open_browser) => result,
        };
        let mut owners = self.owners.lock().unwrap();
        let owner = owners.get_mut(&config.owner).unwrap();
        if owner.generation != generation {
            return Err(CANCELLED.into());
        }
        owner.active = None;
        owner.pending = None;
        match result {
            Ok(token) => {
                let access_token = token.access_token.clone();
                owner.cache.insert(config.token, token);
                owner.status = Some(Status::Ready);
                Ok(access_token)
            }
            Err(error) => {
                owner.status = Some(Status::Failed(error.clone()));
                Err(error)
            }
        }
    }
}

struct Attempt<'a> {
    session: &'a OAuthSession,
    owner: &'a str,
    generation: u64,
}

impl Attempt<'_> {
    fn update(&self, update: impl FnOnce(&mut Owner)) -> Result<(), String> {
        let mut owners = self.session.owners.lock().unwrap();
        let owner = owners.get_mut(self.owner).unwrap();
        if owner.generation != self.generation {
            return Err(CANCELLED.into());
        }
        update(owner);
        Ok(())
    }

    async fn exchange(
        &self,
        config: &Config,
        cached: Option<CachedToken>,
        open_browser: impl FnOnce(&str) -> Result<(), String>,
    ) -> Result<CachedToken, String> {
        let mut request = config.exchange_config();
        let secrets = &self.session.secrets;
        if config.token.grant == Grant::ClientCredentials {
            return secrets
                .exchange(&request)
                .await
                .map(|token| CachedToken::new(token, None));
        }
        if let Some(refresh) = cached.and_then(|token| token.refresh_token) {
            let mut refresh_request = request.clone();
            refresh_request.grant_type = "refresh_token".into();
            refresh_request.refresh_token = refresh.clone();
            if let Ok(token) = secrets.exchange(&refresh_request).await {
                return Ok(CachedToken::new(token, Some(refresh)));
            }
            self.update(|owner| {
                owner.cache.remove(&config.token);
            })?;
        }

        let verifier = random_urlsafe::<32>()?;
        let expected_state = random_urlsafe::<16>()?;
        let capture = &self.session.capture;
        let redirect = capture.begin(config.redirect_port)?;
        let listener = CaptureListener(capture, &redirect.capture_id);
        let authorization_url =
            authorization_url(config, &redirect.redirect_uri, &expected_state, &verifier)?;
        let (manual, pasted) = oneshot::channel();
        self.update(|owner| {
            owner.pending = Some(Pending {
                capture_id: redirect.capture_id.clone(),
                redirect_uri: redirect.redirect_uri.clone(),
                expected_state: expected_state.clone(),
                manual: Some(manual),
            });
            owner.status = Some(Status::WaitingForBrowser {
                authorization_url: authorization_url.to_string(),
                redirect_uri: redirect.redirect_uri.clone(),
            });
        })?;
        open_browser(authorization_url.as_str())?;
        let code = tokio::select! {
            biased;
            code = pasted => code.map_err(|_| CANCELLED.to_owned()),
            redirect = capture.await_redirect(&redirect.capture_id, &expected_state) => {
                redirect.map(|(code, _state)| code)
            }
        }?;
        drop(listener);
        self.update(|owner| {
            owner.pending = None;
            owner.status = Some(Status::Exchanging);
        })?;
        request.code = code;
        request.redirect_uri = redirect.redirect_uri;
        request.code_verifier = verifier;
        secrets
            .exchange(&request)
            .await
            .map(|token| CachedToken::new(token, None))
    }
}

impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        let _ = self.update(|owner| {
            if owner.active.take().is_some() {
                owner.pending = None;
                owner.status = Some(Status::Cancelled);
            }
        });
    }
}

struct CaptureListener<'a>(&'a OAuthCapture, &'a str);

impl Drop for CaptureListener<'_> {
    fn drop(&mut self) {
        self.0.cancel(self.1);
    }
}

fn random_urlsafe<const N: usize>() -> Result<String, String> {
    let mut bytes = [0; N];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|error| format!("could not generate OAuth 2.0 randomness: {error}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn authorization_url(
    config: &Config,
    redirect: &str,
    state: &str,
    verifier: &str,
) -> Result<Url, String> {
    let mut authorize = Url::parse(config.authorize_url.trim())
        .map_err(|error| format!("invalid authorize URL: {error}"))?;
    if !matches!(authorize.scheme(), "http" | "https") {
        return Err("authorize URL must use HTTP or HTTPS".into());
    }
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let parameters = [
        ("response_type", "code"),
        ("client_id", config.token.client_id.as_str()),
        ("redirect_uri", redirect),
        ("scope", config.token.scope.as_str()),
        ("state", state),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
    ];
    let existing = authorize
        .query_pairs()
        .into_owned()
        .filter(|(key, _)| !parameters.iter().any(|(name, _)| key == name))
        .collect::<Vec<_>>();
    authorize
        .query_pairs_mut()
        .clear()
        .extend_pairs(existing)
        .extend_pairs(parameters);
    Ok(authorize)
}

fn pasted_code(text: &str, redirect: &str, expected_state: &str) -> Result<String, String> {
    let text = text.trim();
    let mut code = text.to_owned();
    if let Some((scheme, _)) = text.split_once("://")
        && (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
    {
        let callback = Url::parse(text).map_err(|_| "invalid callback URL")?;
        let redirect = Url::parse(redirect).map_err(|_| "invalid loopback redirect URL")?;
        if callback.origin() != redirect.origin() || callback.path() != redirect.path() {
            return Err("callback URL does not match the loopback redirect".into());
        }
        let pairs = callback.query_pairs().into_owned().collect::<Vec<_>>();
        let mut states = pairs.iter().filter(|(key, _)| key == "state");
        if states
            .next()
            .is_none_or(|(_, state)| state != expected_state)
            || states.next().is_some()
        {
            return Err("state mismatch in the pasted redirect".into());
        }
        let mut codes = pairs.into_iter().filter(|(key, _)| key == "code");
        code = codes.next().map_or_else(String::new, |(_, value)| value);
        if codes.next().is_some() {
            return Err("callback URL contains multiple authorization codes".into());
        }
    }
    if code.is_empty() {
        return Err("authorization code is missing".into());
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    type Form = HashMap<String, String>;

    fn token_server(
        responses: Vec<(&'static str, &'static str)>,
    ) -> (String, thread::JoinHandle<Vec<Form>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/token", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let server = thread::spawn(move || {
            let mut forms = Vec::new();
            for (status, body) in responses {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "no token request arrived");
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("{error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut length = None;
                loop {
                    let mut line = String::new();
                    assert_ne!(reader.read_line(&mut line).unwrap(), 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':')
                        && key.eq_ignore_ascii_case("content-length")
                    {
                        length = Some(value.trim().parse().unwrap());
                    }
                }
                let mut bytes = vec![0; length.unwrap()];
                reader.read_exact(&mut bytes).unwrap();
                forms.push(url::form_urlencoded::parse(&bytes).into_owned().collect());
                write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            forms
        });
        (endpoint, server)
    }

    fn code_config(owner: &str, endpoint: &str) -> Config {
        Config::from_auth(
            owner,
            &json!({
                "grantType": "authorization_code",
                "authorizeUrl": "https://example.test/login",
                "tokenUrl": endpoint,
                "clientId": "application"
            }),
        )
        .unwrap()
    }

    #[test]
    fn builds_a_pkce_authorization_url_and_preserves_provider_parameters() {
        let config = Config::from_auth(
            "request",
            &json!({
                "grantType": "authorization_code",
                "authorizeUrl": "https://example.test/login?prompt=select_account&state=old&state=older",
                "clientId": "application",
                "scope": "one two",
            }),
        )
        .unwrap();
        let url = authorization_url(
            &config,
            "http://127.0.0.1:12345/callback",
            "state-1",
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
        )
        .unwrap();
        let pairs = url.query_pairs().into_owned().collect::<Vec<_>>();
        assert_eq!(pairs.iter().filter(|(name, _)| name == "state").count(), 1);
        let pairs = pairs
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(pairs["prompt"], "select_account");
        assert_eq!(pairs["response_type"], "code");
        assert_eq!(pairs["client_id"], "application");
        assert_eq!(pairs["scope"], "one two");
        assert_eq!(pairs["state"], "state-1");
        assert_eq!(pairs["redirect_uri"], "http://127.0.0.1:12345/callback");
        assert_eq!(pairs["code_challenge_method"], "S256");
        assert_eq!(
            pairs["code_challenge"],
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn validates_pasted_redirect_origin_path_and_state() {
        let redirect = "http://127.0.0.1:12345/callback";
        let state = "expected";
        assert_eq!(
            pasted_code(" raw-code ", redirect, state).unwrap(),
            "raw-code"
        );
        assert_eq!(
            pasted_code(
                &format!("{redirect}?code=one%2Ftwo&state={state}"),
                redirect,
                state
            )
            .unwrap(),
            "one/two"
        );
        for url in [
            format!("{redirect}?code=one&state=wrong"),
            format!("{redirect}?code=one&state={state}&state=wrong"),
            format!("{redirect}?state={state}"),
            format!("http://localhost:12345/callback?code=one&state={state}"),
            format!("http://127.0.0.1:12345/other?code=one&state={state}"),
            format!("https://127.0.0.1:12345/callback?code=one&state={state}"),
        ] {
            assert!(pasted_code(&url, redirect, state).is_err(), "{url}");
        }
    }

    #[tokio::test]
    async fn caches_client_credentials_by_owner_and_token_config() {
        let (endpoint, server) = token_server(vec![
            ("200 OK", r#"{"access_token":"first","expires_in":3600}"#),
            ("200 OK", r#"{"access_token":"other-scope"}"#),
            ("200 OK", r#"{"access_token":"other-owner"}"#),
            ("401 Unauthorized", r#"{"error":"invalid_client"}"#),
            ("200 OK", r#"{"access_token":"retry"}"#),
        ]);
        let secrets = Arc::new(Secrets::memory_only());
        secrets
            .set_oauth_secret("first-owner", Some("first-secret"))
            .unwrap();
        secrets
            .set_oauth_secret("other-owner", Some("other-secret"))
            .unwrap();
        let session = OAuthSession::new(secrets.clone());
        let config = |owner, scope| {
            Config::from_auth(
                owner,
                &json!({ "tokenUrl": endpoint, "clientId": "application", "scope": scope }),
            )
            .unwrap()
        };
        let first = config("first-owner", "one");
        assert_eq!(session.acquire(first.clone()).await.unwrap(), "first");
        assert_eq!(session.acquire(first.clone()).await.unwrap(), "first");
        assert_eq!(
            session.acquire(config("first-owner", "two")).await.unwrap(),
            "other-scope"
        );
        assert_eq!(
            session.acquire(config("other-owner", "one")).await.unwrap(),
            "other-owner"
        );
        assert_eq!(session.cached(&first).as_deref(), Some("first"));
        secrets
            .set_oauth_secret("first-owner", Some("new-secret"))
            .unwrap();
        session.invalidate("first-owner");
        assert!(session.cached(&first).is_none());
        assert!(session.acquire(first.clone()).await.is_err());
        assert!(matches!(
            session.status("first-owner"),
            Some(Status::Failed(_))
        ));
        assert_eq!(session.acquire(first).await.unwrap(), "retry");
        let forms = server.join().unwrap();
        for (form, (scope, secret)) in forms.iter().zip([
            ("one", "first-secret"),
            ("two", "first-secret"),
            ("one", "other-secret"),
            ("one", "new-secret"),
            ("one", "new-secret"),
        ]) {
            assert_eq!(form["grant_type"], "client_credentials");
            assert_eq!(form["scope"], scope);
            assert_eq!(form["client_secret"], secret);
        }
    }

    #[tokio::test]
    async fn exchanges_a_pasted_callback_then_refreshes_an_expired_token() {
        let (endpoint, server) = token_server(vec![
            (
                "200 OK",
                r#"{"access_token":"initial","expires_in":59,"refresh_token":"refresh-me"}"#,
            ),
            ("200 OK", r#"{"access_token":"refreshed","expires_in":59}"#),
            ("401 Unauthorized", r#"{"error":"invalid_grant"}"#),
            ("200 OK", r#"{"access_token":"reauthorized"}"#),
        ]);
        let session = OAuthSession::new(Arc::new(Secrets::memory_only()));
        let config = code_config("request", &endpoint);
        let mut challenge = String::new();
        let initial = session
            .run(config.clone(), false, |url| {
                let url = Url::parse(url).unwrap();
                let pairs = url.query_pairs().into_owned().collect::<Form>();
                challenge = pairs["code_challenge"].clone();
                let redirect = &pairs["redirect_uri"];
                let state = &pairs["state"];
                assert!(
                    session
                        .submit_code("request", &format!("{redirect}?code=b&state=wrong"))
                        .is_err()
                );
                session.submit_code("request", &format!("{redirect}?code=manual&state={state}"))
            })
            .await
            .unwrap();
        assert_eq!(initial, "initial");
        assert!(session.cached(&config).is_none());
        let refreshed = session
            .run(config.clone(), false, |_| {
                panic!("refresh should skip the browser")
            })
            .await
            .unwrap();
        assert_eq!(refreshed, "refreshed");
        let reauthorized = session.run(config, false, |url| {
            let url = Url::parse(url).unwrap();
            let pairs = url.query_pairs().into_owned().collect::<Form>();
            let redirect = Url::parse(&pairs["redirect_uri"]).unwrap();
            let port = redirect.port().unwrap();
            let state = &pairs["state"];
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(stream, "GET /callback?code=browser&state={state} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").unwrap();
            Ok(())
        }).await.unwrap();
        assert_eq!(reauthorized, "reauthorized");
        let forms = server.join().unwrap();
        assert_eq!(forms[0]["code"], "manual");
        assert_eq!(
            URL_SAFE_NO_PAD.encode(Sha256::digest(forms[0]["code_verifier"].as_bytes())),
            challenge
        );
        for form in &forms[1..3] {
            assert_eq!(form["grant_type"], "refresh_token");
            assert_eq!(form["refresh_token"], "refresh-me");
        }
        assert_eq!(forms[3]["grant_type"], "authorization_code");
        assert_eq!(forms[3]["code"], "browser");
    }

    #[tokio::test]
    async fn cancellation_releases_the_loopback_port_and_allows_retry() {
        let session = OAuthSession::new(Arc::new(Secrets::memory_only()));
        let mut config = code_config("request", "");
        let (started, waiting) = oneshot::channel();
        let (result, port) = tokio::join!(
            session.run(config.clone(), false, |url| {
                let url = Url::parse(url).unwrap();
                let pairs = url.query_pairs().into_owned().collect::<Form>();
                let port = Url::parse(&pairs["redirect_uri"]).unwrap().port().unwrap();
                started.send(port).unwrap();
                Ok(())
            }),
            async {
                let port = waiting.await.unwrap();
                session.cancel("request");
                port
            },
        );
        assert_eq!(result.unwrap_err(), CANCELLED);
        assert!(matches!(session.status("request"), Some(Status::Cancelled)));
        assert!(session.submit_code("request", "late-code").is_err());
        tokio::time::timeout(Duration::from_secs(2), async {
            while TcpListener::bind(("127.0.0.1", port)).is_err() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("cancelled capture should release its socket");
        config.redirect_port = port;
        let result = session
            .run(config, false, |_| Err("browser retry reached".into()))
            .await;
        assert_eq!(result.unwrap_err(), "browser retry reached");
    }
}
