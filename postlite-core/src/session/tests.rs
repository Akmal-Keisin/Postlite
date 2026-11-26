use super::*;
use crate::store::TempDir;
use reqwest::header::HeaderValue;
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

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
fn proxy_credentials_round_trip_encrypted_in_the_global_store() {
    let dir = TempDir::new("proxy-creds");
    let keys = test_keys();
    let secrets = Secrets {
        storage: Arc::new(Mutex::new(SecretStorage::Encrypted(storage_with(
            dir.path(),
            keys.clone(),
        )))),
    };
    assert!(secrets.proxy_credentials().unwrap().is_none());

    let creds = ProxyCredentials {
        username: "user".into(),
        password: "hunter2-secret".into(),
    };
    secrets.set_proxy_credentials(&creds).unwrap();
    let loaded = secrets.proxy_credentials().unwrap().unwrap();
    assert_eq!(loaded.username, "user");
    assert_eq!(loaded.password, "hunter2-secret");
    assert!(
        has_key(&keys, "global").is_some(),
        "the data key was created"
    );

    let bytes = std::fs::read(dir.path().join("global.bin")).unwrap();
    assert!(
        !String::from_utf8_lossy(&bytes).contains("hunter2-secret"),
        "the password must not sit in the file in plaintext"
    );
}

type TestKeys = Arc<Mutex<HashMap<String, Arc<store::FakeKeySource>>>>;

fn test_keys() -> TestKeys {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Fake keyrings hold one data key per store without touching the OS.
fn storage_with(dir: &std::path::Path, keys: TestKeys) -> EncryptedStorage {
    EncryptedStorage {
        dir: dir.to_path_buf(),
        keys: Box::new(move |name| {
            keys.lock()
                .unwrap()
                .entry(name.to_owned())
                .or_insert_with(store::FakeKeySource::empty)
                .clone()
        }),
    }
}

fn manager_with(dir: &std::path::Path, keys: TestKeys) -> JarManager {
    JarManager {
        storage: Some(storage_with(dir, keys)),
        jars: Mutex::new(HashMap::new()),
        persist: AtomicBool::new(true),
    }
}

/// Hand a workspace's jar one response header, the way a server would, without starting one.
fn receive(manager: &JarManager, workspace_id: &str, set_cookie: &str, url: &str) {
    let jar = manager
        .jar(workspace_id)
        .expect("a workspace id opens a jar");
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
    assert!(
        keys.lock().unwrap().is_empty(),
        "{:?}",
        keys.lock().unwrap().keys()
    );
    assert!(dir.files().is_empty(), "{:?}", dir.files());

    let one = manager.jar("workspace-a").unwrap();
    assert!(
        Arc::ptr_eq(&one, &manager.jar("workspace-a").unwrap()),
        "opened once"
    );
    assert!(
        !Arc::ptr_eq(&one, &manager.jar("workspace-b").unwrap()),
        "one jar per workspace"
    );
}

#[test]
fn unsafe_workspace_ids_stay_isolated_in_memory() {
    let dir = TempDir::new("workspace-id");
    let keys = test_keys();
    let manager = manager_with(dir.path(), keys.clone());
    let unsafe_ids = ["../other", "a/b", "a\\b", "Workspace-a", "con", "name."];

    for id in unsafe_ids {
        receive(&manager, id, "a=1; Max-Age=60", "http://example.com/");
        let snapshot = manager.snapshot(id).unwrap();
        assert_eq!(snapshot.rows.len(), 1);
        assert!(snapshot.note.is_some());
        assert!(!snapshot.persisting);
    }
    manager.set_persist(true);
    manager.flush_all();
    assert!(keys.lock().unwrap().is_empty());
    assert!(dir.files().is_empty());
    manager.purge(unsafe_ids[0]).unwrap();
    assert!(manager.snapshot(unsafe_ids[0]).unwrap().rows.is_empty());
    assert_eq!(manager.snapshot(unsafe_ids[1]).unwrap().rows.len(), 1);
}

#[test]
fn a_memory_only_profile_keeps_cookies_and_credentials_for_this_session() {
    let manager = JarManager::memory_only();
    manager.set_persist(true);
    receive(
        &manager,
        "workspace-a",
        "a=1; Max-Age=60",
        "http://example.com/",
    );
    let snapshot = manager.snapshot("workspace-a").unwrap();
    assert_eq!(snapshot.rows.len(), 1);
    assert!(!snapshot.persisting);
    assert!(snapshot.note.is_some());

    let secrets = Secrets::memory_only();
    let credentials = ProxyCredentials {
        username: "user".into(),
        password: "pass".into(),
    };
    secrets.set_proxy_credentials(&credentials).unwrap();
    let loaded = secrets.proxy_credentials().unwrap().unwrap();
    assert_eq!(loaded.username, credentials.username);
    assert_eq!(loaded.password, credentials.password);
    secrets.set_oauth_secret("owner", Some("secret")).unwrap();
    assert_eq!(
        secrets.oauth_secret("owner").unwrap().as_deref(),
        Some("secret")
    );
    assert!(secrets.oauth_secret("other").unwrap().is_none());
    assert!(Secrets::memory_only()
        .proxy_credentials()
        .unwrap()
        .is_none());
    assert!(Secrets::memory_only()
        .oauth_secret("owner")
        .unwrap()
        .is_none());
    secrets.set_oauth_secret("owner", Some("")).unwrap();
    assert!(secrets.oauth_secret("owner").unwrap().is_none());
}

#[test]
fn cookie_list_reports_the_rows_and_whether_they_are_saved() {
    let dir = TempDir::new("cmd-list");
    let manager = manager_with(dir.path(), test_keys());

    let empty = manager.snapshot("workspace-a").unwrap();
    assert!(empty.rows.is_empty());
    assert_eq!(empty.note, None);
    assert!(empty.persisting);

    receive(
        &manager,
        "workspace-a",
        "a=1; Path=/x; Max-Age=60",
        "http://example.com/x",
    );
    receive(&manager, "workspace-a", "b=2; Path=/", "http://other.test/");

    let snapshot = manager.snapshot("workspace-a").unwrap();
    assert_eq!(snapshot.rows.len(), 2, "{:?}", snapshot.rows);
    // Rows come out grouped by domain, which is what the Cookie Manager draws.
    assert_eq!(snapshot.rows[0].domain, "example.com");
    assert_eq!(snapshot.rows[0].path, "/x");
    assert!(!snapshot.rows[0].session, "Max-Age makes it persistent");
    assert!(snapshot.rows[0].expires_ms.is_some());
    assert_eq!(snapshot.rows[1].domain, "other.test");
    assert!(
        snapshot.rows[1].session,
        "no expiry means it dies with the app"
    );

    // The drawer reads camelCase, and a value it shows is the real value: masking is its job.
    let json = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(json["rows"][0]["name"], "a");
    assert_eq!(json["rows"][0]["value"], "1");
    assert_eq!(
        json["rows"][0]["expiresMs"],
        snapshot.rows[0].expires_ms.unwrap()
    );
    assert!(json["rows"][0]["session"].is_boolean());
    assert_eq!(json["persisting"], true);
    assert!(json["note"].is_null());
}

#[test]
fn cookie_delete_widens_from_one_cookie_to_the_whole_jar_and_the_disk_follows() {
    let dir = TempDir::new("cmd-delete");
    let keys = test_keys();
    let manager = manager_with(dir.path(), keys.clone());
    receive(
        &manager,
        "workspace-a",
        "a=1; Path=/x; Max-Age=60",
        "http://example.com/x",
    );
    receive(
        &manager,
        "workspace-a",
        "b=2; Path=/y; Max-Age=60",
        "http://example.com/y",
    );
    receive(
        &manager,
        "workspace-a",
        "c=3; Path=/; Max-Age=60",
        "http://other.test/",
    );
    assert_eq!(manager.snapshot("workspace-a").unwrap().rows.len(), 3);

    let removed =
        |manager: &JarManager, domain: Option<&str>, path: Option<&str>, name: Option<&str>| {
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
    assert_eq!(
        removed(&manager, Some("example.com"), Some("/x"), Some("a")),
        1
    );
    assert_eq!(
        removed(&manager, Some("example.com"), Some("/x"), Some("a")),
        0
    );
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
    receive(
        &manager,
        "workspace-a",
        "a=1; Path=/; Max-Age=60",
        "http://example.com/",
    );
    receive(
        &manager,
        "workspace-b",
        "b=2; Path=/; Max-Age=60",
        "http://example.com/",
    );
    assert_eq!(dir.files(), ["workspace-a.bin", "workspace-b.bin"]);

    manager.purge("workspace-a").unwrap();

    assert_eq!(
        dir.files(),
        ["workspace-b.bin"],
        "only that workspace's file"
    );
    assert!(has_key(&keys, "workspace-a").is_none(), "and its key");
    assert!(
        has_key(&keys, "workspace-b").is_some(),
        "the neighbour keeps its key"
    );
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
    receive(
        &manager,
        "workspace-a",
        "a=1; Path=/; Max-Age=60",
        "http://example.com/",
    );
    assert_eq!(dir.files(), ["workspace-a.bin"]);
    assert!(manager.snapshot("workspace-a").unwrap().persisting);

    manager.set_persist(false);
    // The session jar keeps its cookies, but the saved file is gone so nothing resurrects.
    let off = manager.snapshot("workspace-a").unwrap();
    assert!(!off.persisting);
    assert_eq!(off.rows.len(), 1, "the jar still works for the session");
    assert!(
        dir.files().is_empty(),
        "turning persistence off drops the saved jar"
    );
    receive(
        &manager,
        "workspace-a",
        "b=2; Path=/; Max-Age=60",
        "http://example.com/",
    );
    assert!(
        dir.files().is_empty(),
        "nothing is written while persistence is off"
    );

    // A jar opened after the switch inherits it.
    assert!(!manager.snapshot("workspace-b").unwrap().persisting);

    manager.set_persist(true);
    assert!(manager.snapshot("workspace-a").unwrap().persisting);
    manager.jar("workspace-a").unwrap().flush();
    assert_eq!(
        dir.files(),
        ["workspace-a.bin"],
        "re-enabling puts the current jar back"
    );
}

#[test]
fn a_file_that_cannot_be_decrypted_keeps_saying_so() {
    let dir = TempDir::new("cmd-corrupt");
    let manager = manager_with(dir.path(), test_keys());
    receive(
        &manager,
        "workspace-a",
        "a=1; Path=/; Max-Age=60",
        "http://example.com/",
    );

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

fn oauth2_config(grant_type: &str, client_auth_in: &str) -> OAuth2Config {
    serde_json::from_value(json!({
        "nodeId": "node1", "grantType": grant_type, "tokenUrl": "",
        "clientId": "cid", "scope": "read write", "clientAuthIn": client_auth_in,
    }))
    .unwrap()
}

fn serve_mirror() -> (
    String,
    thread::JoinHandle<()>,
    std::sync::Arc<std::sync::Mutex<String>>,
) {
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
    let secrets = Secrets::memory_only();
    secrets
        .set_oauth_secret(&config.node_id, Some("sekrit"))
        .unwrap();
    let token = secrets.exchange(&config).await.unwrap();
    server.join().unwrap();
    let request = received.lock().unwrap().clone();
    assert!(
        request
            .to_ascii_lowercase()
            .contains("content-type: application/x-www-form-urlencoded"),
        "{request}"
    );
    assert!(
        request.contains("grant_type=client_credentials"),
        "{request}"
    );
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
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: basic y2lkonnla3jpda=="),
        "{request}"
    );
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
    assert!(
        request.contains("grant_type=authorization_code"),
        "{request}"
    );
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
    let error = oauth2_token_request(&config, Some("wrong-secret"))
        .await
        .err()
        .unwrap();
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
    let error = oauth2_token_request(&config, Some("super-secret-token"))
        .await
        .err()
        .unwrap();
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
    let error = oauth2_token_request(&config, Some("sekrit"))
        .await
        .err()
        .unwrap();
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
    let error =
        oauth2_token_request_with_timeout(&config, Some("sekrit"), Duration::from_millis(30))
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
        format!(
            "oauth2-v2-{}",
            "request:a"
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        )
    );
    assert_ne!(
        oauth2_store_name("request:a").unwrap(),
        oauth2_store_name("requestA").unwrap()
    );
    assert_ne!(
        oauth2_store_name("request-a").unwrap(),
        oauth2_store_name("requesta").unwrap()
    );
    assert!(oauth2_store_name("").is_err());
    assert_eq!(
        oauth2_legacy_store_name("node1").as_deref(),
        Some("oauth2-node1")
    );
    assert_eq!(oauth2_legacy_store_name("Node1"), None);
    assert_eq!(oauth2_legacy_store_name("node:1"), None);
    assert_eq!(oauth2_legacy_store_name(""), None);
}

#[test]
fn oauth2_secret_migration_moves_legacy_bytes_without_overwriting_current_data() {
    let temp = TempDir::new("oauth2-migration");
    let old_bytes = br#"{"secret":"old"}"#;
    let current_bytes = br#"{"secret":"current"}"#;
    let legacy =
        store::EncryptedStore::open(temp.path(), "oauth2-node1", store::FakeKeySource::empty());
    let current = store::EncryptedStore::open(
        temp.path(),
        &oauth2_store_name("node1").unwrap(),
        store::FakeKeySource::empty(),
    );
    legacy.save(old_bytes).unwrap();
    migrate_oauth2_secret(&current, Some(&legacy)).unwrap();
    assert_eq!(
        current.load().unwrap().as_deref(),
        Some(old_bytes.as_slice())
    );
    assert_eq!(legacy.load().unwrap(), None);

    legacy.save(old_bytes).unwrap();
    current.save(current_bytes).unwrap();
    migrate_oauth2_secret(&current, Some(&legacy)).unwrap();
    assert_eq!(
        current.load().unwrap().as_deref(),
        Some(current_bytes.as_slice())
    );
    assert_eq!(legacy.load().unwrap(), None);
}

#[test]
fn per_node_secret_store_roundtrip_is_isolated() {
    let temp = TempDir::new("oauth2-test");
    let secrets = Secrets {
        storage: Arc::new(Mutex::new(SecretStorage::Encrypted(storage_with(
            temp.path(),
            test_keys(),
        )))),
    };
    secrets.set_oauth_secret("abc", Some("s1")).unwrap();
    assert_eq!(secrets.oauth_secret("abc").unwrap().as_deref(), Some("s1"));
    assert!(secrets.oauth_secret("def").unwrap().is_none());
    secrets.set_oauth_secret("abc", None).unwrap();
    assert!(secrets.oauth_secret("abc").unwrap().is_none());
    assert!(temp.files().is_empty());
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
        capture_authorization(&listener, Duration::from_secs(2), &AtomicBool::new(false))
    });
    let mut client = std::net::TcpStream::connect(address).unwrap();
    client
        .write_all(
            b"GET /callback?error=access_denied&state=xyz HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .unwrap();
    let mut response = String::new();
    client.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(
        response.contains("Authorization was not completed"),
        "{response}"
    );
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
    let handle =
        thread::spawn(move || capture_authorization(&listener, Duration::from_secs(120), &waiter));
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
    let error = rx
        .recv_timeout(Duration::from_millis(500))
        .unwrap()
        .err()
        .unwrap();
    handle.join().unwrap();
    assert!(error.contains("cancelled"), "{error}");
}

#[test]
fn an_idle_partial_connection_does_not_block_the_real_redirect() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let capture = thread::spawn(move || {
        capture_authorization(&listener, Duration::from_secs(5), &AtomicBool::new(false))
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
        capture_authorization(&listener, Duration::from_secs(2), &AtomicBool::new(false))
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
    let first_id = capture
        .set(TcpListener::bind("127.0.0.1:0").unwrap())
        .unwrap();
    let second_id = capture
        .set(TcpListener::bind("127.0.0.1:0").unwrap())
        .unwrap();
    assert_ne!(first_id, second_id);
    let (_, first_cancelled) = capture.take(&first_id).unwrap();
    capture.cancel(&first_id);
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
        capture_authorization(&listener, Duration::from_secs(2), &AtomicBool::new(false))
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
        capture_authorization(&listener, Duration::from_secs(2), &AtomicBool::new(false))
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
