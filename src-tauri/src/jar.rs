//! Tier 2B cookie jar (design §7): reqwest's `CookieStore` over the same engine reqwest uses
//! internally, with encrypted persistence and the session/persistent split that engine already
//! enforces.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use reqwest::header::HeaderValue;
use serde::Serialize;
use url::Url;

use crate::store::{EncryptedStore, KeySource, StoreError};

/// Writes coalesce: a burst of responses produces one file write, and the tail is caught by
/// `flush()` on workspace switch and window close.
const FLUSH_INTERVAL: Duration = Duration::from_millis(500);

/// One row of the Cookie Manager drawer. Values are real here; masking is a display concern (§10).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CookieRow {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<String>,
    /// Unix milliseconds, `None` for a session cookie.
    pub expires_ms: Option<i64>,
    pub session: bool,
}

pub struct PostliteJar {
    inner: RwLock<cookie_store::CookieStore>,
    store: EncryptedStore,
    persist: AtomicBool,
    dirty: Mutex<bool>,
    last_write: Mutex<Option<Instant>>,
}

/// Why a jar is not persisting, in words the drawer can show instead of an empty list. A restored
/// jar needs no note: the drawer counts `rows()`.
pub enum JarOpen {
    Restored(Arc<PostliteJar>),
    Empty(Arc<PostliteJar>, String),
}

impl PostliteJar {
    pub fn open(dir: &Path, name: &str, keys: Arc<dyn KeySource>, persist: bool) -> JarOpen {
        let jar = Arc::new(Self {
            inner: RwLock::new(cookie_store::CookieStore::new()),
            store: EncryptedStore::open(dir, name, keys),
            persist: AtomicBool::new(persist),
            dirty: Mutex::new(false),
            last_write: Mutex::new(None),
        });

        match jar.store.load() {
            Ok(None) if persist && !jar.store.secret_store_usable() => JarOpen::Empty(
                jar,
                "cookies are not saved: no secret store found on this system".into(),
            ),
            Ok(None) => JarOpen::Restored(jar),
            Ok(Some(bytes)) => match cookie_store::serde::json::load(bytes.as_slice()) {
                Ok(loaded) => {
                    *jar.inner.write().unwrap() = loaded;
                    JarOpen::Restored(jar)
                }
                Err(e) => JarOpen::Empty(jar, format!("saved cookies could not be read: {e}")),
            },
            Err(StoreError::NoSecretStore) => JarOpen::Empty(
                jar,
                "cookies are not saved: no secret store found on this system".into(),
            ),
            Err(e) => JarOpen::Empty(jar, e.message()),
        }
    }

    pub fn set_persist(&self, on: bool) {
        self.persist.store(on, Ordering::Relaxed);
        let mut dirty = self.dirty.lock().unwrap();
        if on {
            // Re-enabling must not wait for the next response to put the current jar back on disk.
            *dirty = true;
        } else {
            // Turning persistence off must not leave a jar behind that would come back later.
            *dirty = false;
            let _ = self.store.delete();
        }
    }

    pub fn persists(&self) -> bool {
        self.persist.load(Ordering::Relaxed)
    }

    /// Write now if anything changed and the coalescing window has passed.
    pub fn maybe_flush(&self) {
        if !self.dirty_now() {
            return;
        }
        let due = match *self.last_write.lock().unwrap() {
            None => true,
            Some(previous) => previous.elapsed() >= FLUSH_INTERVAL,
        };
        if due {
            self.write();
        }
    }

    /// Write regardless of the coalescing window.
    pub fn flush(&self) {
        if *self.dirty.lock().unwrap() {
            self.write();
        }
    }

    pub fn rows(&self) -> Vec<CookieRow> {
        let store = match self.inner.read() {
            Ok(guard) => guard,
            Err(_) => return Vec::new(),
        };
        let mut rows: Vec<CookieRow> = store
            .iter_unexpired()
            .map(|cookie| CookieRow {
                name: cookie.name().to_owned(),
                value: cookie.value().to_owned(),
                domain: String::from(&cookie.domain),
                path: String::from(&cookie.path),
                secure: cookie.secure().unwrap_or(false),
                http_only: cookie.http_only().unwrap_or(false),
                same_site: cookie
                    .same_site()
                    .map(|site| format!("{site:?}").to_ascii_lowercase()),
                expires_ms: match &cookie.expires {
                    cookie_store::CookieExpiration::AtUtc(at) => Some(at.unix_timestamp() * 1000),
                    cookie_store::CookieExpiration::SessionEnd => None,
                },
                session: !cookie.is_persistent(),
            })
            .collect();
        rows.sort_by(|a, b| (&a.domain, &a.path, &a.name).cmp(&(&b.domain, &b.path, &b.name)));
        rows
    }

    /// Delete every cookie matching all of the given fields; a field left out matches anything, so
    /// the all-`None` filter is "everything in this jar" (§7). Returns how many went.
    pub fn remove_matching(
        &self,
        domain: Option<&str>,
        path: Option<&str>,
        name: Option<&str>,
    ) -> usize {
        let targets: Vec<(String, String, String)> = match self.inner.read() {
            Ok(store) => store
                .iter_any()
                .filter(|cookie| {
                    domain.map_or(true, |want| String::from(&cookie.domain) == want)
                        && path.map_or(true, |want| String::from(&cookie.path) == want)
                        && name.map_or(true, |want| cookie.name() == want)
                })
                .map(|cookie| {
                    (
                        String::from(&cookie.domain),
                        String::from(&cookie.path),
                        cookie.name().to_owned(),
                    )
                })
                .collect(),
            Err(_) => return 0,
        };
        if targets.is_empty() {
            return 0;
        }
        if let Ok(mut store) = self.inner.write() {
            for (domain, path, name) in &targets {
                store.remove(domain, path, name);
            }
        }
        self.mark_dirty();
        targets.len()
    }

    /// Forget everything for this workspace: memory, the encrypted file, and its key.
    pub fn clear_all(&self) {
        if let Ok(mut store) = self.inner.write() {
            store.clear();
        }
        *self.dirty.lock().unwrap() = false;
        let _ = self.store.delete();
    }

    fn dirty_now(&self) -> bool {
        self.persists() && *self.dirty.lock().unwrap()
    }

    fn mark_dirty(&self) {
        // With persistence off there is nothing to write, and a stale flag would survive into
        // the moment persistence is turned back on.
        if self.persists() {
            *self.dirty.lock().unwrap() = true;
        }
    }

    fn write(&self) {
        let mut bytes = Vec::new();
        let outcome = {
            let store = match self.inner.read() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            cookie_store::serde::json::save(&store, &mut bytes).map(|_| self.store.save(&bytes))
        };
        match outcome {
            Ok(Ok(())) | Ok(Err(StoreError::NoSecretStore)) => {
                // Nothing was written because there is nowhere safe to write it; the drawer says so.
                *self.dirty.lock().unwrap() = false;
                *self.last_write.lock().unwrap() = Some(Instant::now());
            }
            _ => {}
        }
    }
}

/// RFC 6265 §5.2.3 refuses a `Domain` that is a public suffix. The engine only checks that the
/// domain is a suffix of the host, and postlite ships no public-suffix list, so the widest
/// remaining case is closed here: a domain shorter than the host needs more than one label,
/// which stops one server poisoning every `.com` site in the jar with `Domain=com`. Domains
/// like `co.uk` stay a documented gap until a suffix list is bundled.
fn domain_acceptable(cookie: &cookie_store::RawCookie, host: &str) -> bool {
    let Some(domain) = cookie
        .domain()
        .map(|value| value.trim_start_matches('.').to_ascii_lowercase())
    else {
        return true;
    };
    domain == host.to_ascii_lowercase() || domain.split('.').count() >= 2
}

impl reqwest::cookie::CookieStore for PostliteJar {
    fn set_cookies(&self, cookie_headers: &mut dyn Iterator<Item = &HeaderValue>, url: &Url) {
        // The RawCookie path, never `cookie_store::Cookie::parse`: only this one lets an expired
        // Set-Cookie delete the cookie it replaces, which is how logout works.
        let parsed: Vec<cookie_store::RawCookie> = cookie_headers
            .filter_map(|value| value.to_str().ok())
            .filter_map(|value| cookie_store::RawCookie::parse(value.to_owned()).ok())
            .filter(|cookie| domain_acceptable(cookie, url.host_str().unwrap_or("")))
            .collect();
        if parsed.is_empty() {
            return;
        }

        {
            let mut store = match self.inner.write() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            store.store_response_cookies(parsed.into_iter(), url);
        }

        // Any received cookie may add, replace or delete, and one that merely aged out changes
        // what the next serialisation writes; counting the store would miss all three.
        self.mark_dirty();
        self.maybe_flush();
    }

    fn cookies(&self, url: &Url) -> Option<HeaderValue> {
        let joined = self
            .inner
            .read()
            .ok()?
            .get_request_values(url)
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        if joined.is_empty() {
            return None;
        }
        HeaderValue::from_str(&joined).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{FakeKeySource, NoSecretStoreKeys, TempDir};
    use reqwest::header::HeaderValue;
    use std::fs;

    fn open_jar(dir: &TempDir, keys: Arc<dyn KeySource>) -> Arc<PostliteJar> {
        match PostliteJar::open(dir.path(), "workspace-a", keys, true) {
            JarOpen::Restored(jar) | JarOpen::Empty(jar, _) => jar,
        }
    }

    /// Feed one `Set-Cookie` header exactly the way reqwest does.
    fn receive(jar: &PostliteJar, set_cookie: &str, from: &str) {
        let value = HeaderValue::from_str(set_cookie).unwrap();
        let mut iter = [&value].into_iter();
        reqwest::cookie::CookieStore::set_cookies(jar, &mut iter, &Url::parse(from).unwrap());
    }

    fn sent(jar: &PostliteJar, to: &str) -> String {
        reqwest::cookie::CookieStore::cookies(jar, &Url::parse(to).unwrap())
            .map(|value| value.to_str().unwrap().to_owned())
            .unwrap_or_default()
    }

    #[test]
    fn replays_for_the_same_host_and_not_for_a_sibling() {
        let dir = TempDir::new("replay");
        let jar = open_jar(&dir, FakeKeySource::empty());
        receive(&jar, "sid=abc; Path=/", "http://example.com/login");

        assert_eq!(sent(&jar, "http://example.com/account"), "sid=abc");
        assert_eq!(sent(&jar, "http://notexample.com/account"), "");
        assert_eq!(sent(&jar, "http://other.com/account"), "");
    }

    #[test]
    fn path_scope_is_honoured() {
        let dir = TempDir::new("path");
        let jar = open_jar(&dir, FakeKeySource::empty());
        receive(&jar, "api=1; Path=/api; Max-Age=60", "http://example.com/api/login");
        // With no Path attribute the scope is the request URL's directory, not the whole site.
        receive(&jar, "root=2; Max-Age=60", "http://example.com/login");

        // Which cookies are in scope is the rule; the order inside the header is not contractual.
        let orders = sent(&jar, "http://example.com/api/orders");
        let mut in_api: Vec<&str> = orders.split("; ").collect();
        in_api.sort();
        assert_eq!(in_api, ["api=1", "root=2"]);
        assert_eq!(sent(&jar, "http://example.com/docs"), "root=2");
    }

    #[test]
    fn a_host_only_cookie_does_not_leak_to_a_subdomain() {
        let dir = TempDir::new("hostonly");
        let jar = open_jar(&dir, FakeKeySource::empty());
        receive(&jar, "sid=abc; Path=/", "http://shop.example.com/a");

        assert_eq!(sent(&jar, "http://shop.example.com/b"), "sid=abc");
        assert_eq!(sent(&jar, "http://other.shop.example.com/b"), "");
    }

    #[test]
    fn a_domain_that_is_not_the_host_may_not_be_a_bare_tld() {
        let dir = TempDir::new("domain");
        let jar = open_jar(&dir, FakeKeySource::empty());
        for domain in ["other.com", "example.org", "com", ".com"] {
            receive(&jar, &format!("sid=abc; Domain={domain}; Path=/"), "http://example.com/login");
        }
        assert!(jar.rows().is_empty(), "{:?}", jar.rows());

        // A genuine subdomain scope still works.
        receive(&jar, "sub=1; Domain=example.com; Path=/", "http://shop.example.com/login");
        assert_eq!(sent(&jar, "http://shop.example.com/a"), "sub=1");

        // ...and a single-label host is legitimate, so Domain=localhost must not be refused.
        let local = TempDir::new("local");
        let local_jar = open_jar(&local, FakeKeySource::empty());
        receive(&local_jar, "sid=abc; Domain=localhost; Path=/", "http://localhost:8080/login");
        assert_eq!(sent(&local_jar, "http://localhost:8080/a"), "sid=abc");
    }

    #[test]
    fn max_age_wins_over_a_past_expires() {
        let dir = TempDir::new("expiry");
        let jar = open_jar(&dir, FakeKeySource::empty());
        receive(&jar, "v=1; Max-Age=3600; Expires=Mon, 01 Jan 1990 00:00:00 GMT", "http://example.com/a");
        assert_eq!(sent(&jar, "http://example.com/a"), "v=1", "Max-Age must win over a past Expires");
    }

    #[test]
    fn a_cookie_that_arrives_expired_is_not_stored() {
        let dir = TempDir::new("stale");
        let jar = open_jar(&dir, FakeKeySource::empty());
        receive(&jar, "gone=1; Expires=Mon, 01 Jan 1990 00:00:00 GMT", "http://example.com/b");
        assert_eq!(sent(&jar, "http://example.com/b"), "");
        assert!(jar.rows().is_empty(), "{:?}", jar.rows());
    }

    #[test]
    fn a_secure_cookie_survives_only_on_https() {
        let dir = TempDir::new("secure");
        let jar = open_jar(&dir, FakeKeySource::empty());
        receive(&jar, "tok=1; Secure; Max-Age=60", "http://example.com/a");
        assert_eq!(sent(&jar, "http://example.com/a"), "", "a Secure cookie must not go over plain http");

        receive(&jar, "tok=1; Secure; Max-Age=60", "https://example.com/a");
        assert_eq!(sent(&jar, "https://example.com/a"), "tok=1");
        assert_eq!(sent(&jar, "http://example.com/a"), "");
    }

    #[test]
    fn persistent_cookies_survive_a_restart() {
        let dir = TempDir::new("restart");
        let keys = FakeKeySource::empty();
        receive(&open_jar(&dir, keys.clone()), "theme=dark; Max-Age=3600", "http://example.com/a");
        open_jar(&dir, keys.clone()).flush();

        let reopened = open_jar(&dir, keys);
        assert_eq!(sent(&reopened, "http://example.com/a"), "theme=dark");
        assert_eq!(reopened.rows().len(), 1);
    }

    #[test]
    fn session_cookies_never_reach_the_disk() {
        let dir = TempDir::new("session");
        let keys = FakeKeySource::empty();
        let jar = open_jar(&dir, keys.clone());
        receive(&jar, "sid=abc; Path=/", "http://example.com/login");
        assert!(jar.rows()[0].session);
        jar.flush();

        let reopened = open_jar(&dir, keys);
        assert_eq!(sent(&reopened, "http://example.com/a"), "");
        assert!(reopened.rows().is_empty());

        let on_disk = fs::read(dir.path().join("workspace-a.bin")).unwrap();
        assert!(!String::from_utf8_lossy(&on_disk).contains("abc"));
    }

    #[test]
    fn an_expired_set_cookie_deletes_the_stored_one() {
        let dir = TempDir::new("logout");
        let keys = FakeKeySource::empty();
        let jar = open_jar(&dir, keys.clone());
        receive(&jar, "sid=abc; Path=/; Max-Age=3600", "http://example.com/a");
        receive(&jar, "sid=; Path=/; Max-Age=0", "http://example.com/logout");

        assert_eq!(sent(&jar, "http://example.com/a"), "");
        jar.flush();
        assert!(open_jar(&dir, keys).rows().is_empty());
    }

    #[test]
    fn writes_coalesce_until_the_interval_passes_or_a_flush_forces_one() {
        let dir = TempDir::new("coalesce");
        let jar = open_jar(&dir, FakeKeySource::empty());

        receive(&jar, "a=1; Max-Age=60", "http://example.com/a");
        jar.maybe_flush();
        let first = fs::read(dir.path().join("workspace-a.bin")).unwrap();

        receive(&jar, "b=2; Max-Age=60", "http://example.com/b");
        jar.maybe_flush();
        let same = fs::read(dir.path().join("workspace-a.bin")).unwrap();
        assert_eq!(first, same, "a burst inside the window must not rewrite the file");

        jar.flush();
        let forced = fs::read(dir.path().join("workspace-a.bin")).unwrap();
        assert_ne!(first, forced, "flush must write the pending change");
        assert_eq!(jar.rows().len(), 2);
    }

    #[test]
    fn persistence_off_keeps_the_session_but_writes_nothing() {
        let dir = TempDir::new("off");
        let keys = FakeKeySource::empty();
        let jar = open_jar(&dir, keys.clone());
        jar.set_persist(false);
        receive(&jar, "sid=abc; Max-Age=3600", "http://example.com/a");
        jar.flush();

        assert_eq!(sent(&jar, "http://example.com/a"), "sid=abc");
        assert!(dir.files().is_empty(), "{:?}", dir.files());

        assert!(open_jar(&dir, keys).rows().is_empty());
    }

    #[test]
    fn no_secret_store_keeps_cookies_in_memory_only_and_says_why() {
        let dir = TempDir::new("nostore");
        let note = match PostliteJar::open(dir.path(), "workspace-a", Arc::new(NoSecretStoreKeys), true) {
            JarOpen::Empty(_, note) => note,
            JarOpen::Restored(_) => panic!("expected the no-store note"),
        };
        assert!(note.contains("no secret store"), "{note}");

        let jar = match PostliteJar::open(dir.path(), "workspace-a", Arc::new(NoSecretStoreKeys), true) {
            JarOpen::Empty(jar, _) | JarOpen::Restored(jar) => jar,
        };
        receive(&jar, "sid=abc; Max-Age=3600", "http://example.com/a");
        jar.flush();

        assert_eq!(sent(&jar, "http://example.com/a"), "sid=abc");
        assert!(dir.files().is_empty(), "{:?}", dir.files());
    }

    #[test]
    fn a_deleting_filter_widens_from_one_cookie_to_a_whole_domain() {
        let dir = TempDir::new("delete");
        let keys = FakeKeySource::empty();
        let jar = open_jar(&dir, keys.clone());
        receive(&jar, "a=1; Path=/x; Max-Age=60", "http://example.com/a");
        receive(&jar, "b=2; Path=/y; Max-Age=60", "http://example.com/b");
        receive(&jar, "c=3; Path=/; Max-Age=60", "http://other.test/c");
        jar.flush();

        assert_eq!(jar.remove_matching(Some("example.com"), Some("/x"), Some("a")), 1);
        assert_eq!(
            jar.remove_matching(Some("example.com"), Some("/x"), Some("a")),
            0,
            "deleting twice is not a change"
        );
        // One field left out matches anything under it: this is the drawer's "clear this domain".
        assert_eq!(jar.remove_matching(Some("example.com"), None, None), 1);
        assert_eq!(jar.rows().len(), 1);
        jar.flush();
        assert_eq!(
            open_jar(&dir, keys.clone()).rows().len(),
            1,
            "the disk must follow what the list shows"
        );

        jar.clear_all();
        assert!(jar.rows().is_empty());
        assert!(dir.files().is_empty(), "{:?}", dir.files());
        assert_eq!(keys.get().unwrap(), None, "the key must go with the jar");
    }

    #[test]
    fn rows_carry_the_attributes_the_manager_shows() {
        let dir = TempDir::new("rows");
        let jar = open_jar(&dir, FakeKeySource::empty());
        receive(
            &jar,
            "sid=abc; Path=/api; Secure; HttpOnly; SameSite=Lax; Max-Age=3600",
            "https://example.com/login",
        );
        let row = &jar.rows()[0];
        assert_eq!(row.name, "sid");
        assert_eq!(row.value, "abc", "the row holds the real value; masking is display-only");
        assert_eq!(row.domain, "example.com");
        assert_eq!(row.path, "/api");
        assert!(row.secure && row.http_only);
        assert_eq!(row.same_site.as_deref(), Some("lax"));
        assert!(!row.session);
        assert!(row.expires_ms.unwrap() > 0);
    }

    #[test]
    fn a_renamed_corrupt_file_leaves_the_jar_empty_but_visible() {
        let dir = TempDir::new("corrupt");
        fs::write(dir.path().join("workspace-a.bin"), b"not a cookie store").unwrap();
        let note = match PostliteJar::open(dir.path(), "workspace-a", FakeKeySource::empty(), true) {
            JarOpen::Empty(_, note) => note,
            JarOpen::Restored(_) => panic!("expected a corrupt-store note"),
        };
        assert!(note.contains("could not be read"), "{note}");
        assert!(dir.path().join("workspace-a.bin.corrupt").exists());
    }
}
