//! Encrypted-at-rest local store: an AES-256-GCM blob file whose key lives only in the OS
//! secret store (Tier 2B design §8). Used by the per-workspace cookie jars and by the global
//! store that holds proxy credentials.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit, Nonce as AesNonce};
use aes_gcm::Aes256Gcm;

const MAGIC: &[u8; 4] = b"PLCS";
const VERSION: u8 = 1;
const HEADER_LEN: usize = MAGIC.len() + 1 + 12;
const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;

#[derive(Debug, PartialEq, Eq)]
pub enum StoreError {
    /// No usable OS secret store. Callers must degrade visibly and must never write plaintext.
    NoSecretStore,
    /// The file exists but is not something this build can decrypt. The bad bytes are moved
    /// aside to `<name>.bin.corrupt` and the caller starts from an empty store.
    Corrupt(String),
    Io(String),
}

impl StoreError {
    pub fn message(&self) -> String {
        match self {
            StoreError::NoSecretStore => "no secret store available".into(),
            StoreError::Corrupt(reason) => format!("saved cookies could not be read: {reason}"),
            StoreError::Io(reason) => reason.clone(),
        }
    }
}

/// Where the 32-byte data key lives. Split out so tests never touch the real credential store
/// on the developer's machine.
pub trait KeySource: Send + Sync {
    fn get(&self) -> Result<Option<Vec<u8>>, StoreError>;
    fn set(&self, key: &[u8]) -> Result<(), StoreError>;
    fn remove(&self) -> Result<(), StoreError>;
}

pub struct EncryptedStore {
    file: PathBuf,
    keys: std::sync::Arc<dyn KeySource>,
}

impl EncryptedStore {
    pub fn open(dir: &Path, name: &str, keys: std::sync::Arc<dyn KeySource>) -> Self {
        Self {
            file: dir.join(format!("{name}.bin")),
            keys,
        }
    }

    /// Encrypts `plaintext` and writes it atomically, creating the data key on first use.
    pub fn save(&self, plaintext: &[u8]) -> Result<(), StoreError> {
        let key = self.key()?;
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::fill(&mut nonce_bytes[..]);

        let body = self
            .cipher(&key)?
            .encrypt(&nonce_of(&nonce_bytes)?, plaintext)
            .map_err(|_| StoreError::Corrupt("encryption failed".into()))?;

        let mut bytes = Vec::with_capacity(HEADER_LEN + body.len());
        bytes.extend_from_slice(MAGIC);
        bytes.push(VERSION);
        bytes.extend_from_slice(&nonce_bytes);
        bytes.extend_from_slice(&body);

        fs::create_dir_all(self.file.parent().unwrap_or(Path::new(".")))
            .map_err(|e| StoreError::Io(e.to_string()))?;
        let staging = self.staging_path();
        let written = fs::File::create(&staging)
            .and_then(|mut file| {
                file.write_all(&bytes)?;
                file.sync_all()
            })
            .and_then(|_| fs::rename(&staging, &self.file));
        if let Err(e) = written {
            let _ = fs::remove_file(&staging);
            return Err(StoreError::Io(e.to_string()));
        }
        Ok(())
    }

    /// `Ok(None)` when nothing has been stored yet. A file this build cannot decrypt is moved
    /// aside and reported, never silently ignored.
    pub fn load(&self) -> Result<Option<Vec<u8>>, StoreError> {
        let bytes = match fs::read(&self.file) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(StoreError::Io(e.to_string())),
        };
        if bytes.len() < HEADER_LEN {
            return Err(self.quarantine("file too short"));
        }
        if &bytes[..MAGIC.len()] != MAGIC {
            return Err(self.quarantine("bad magic"));
        }
        if bytes[MAGIC.len()] != VERSION {
            return Err(self.quarantine("unsupported version"));
        }
        let key = self.key()?;
        let body = &bytes[HEADER_LEN..];
        match self
            .cipher(&key)?
            .decrypt(&nonce_of(&bytes[MAGIC.len() + 1..HEADER_LEN])?, body)
        {
            Ok(plaintext) => Ok(Some(plaintext)),
            Err(_) => Err(self.quarantine("wrong key or tampered contents")),
        }
    }

    /// Whether a key could be read from the OS secret store, or created there on first write.
    /// `false` means nothing saved through this store will survive, and the UI must say so.
    pub fn secret_store_usable(&self) -> bool {
        match self.keys.get() {
            Ok(Some(key)) => key.len() == KEY_LEN,
            Ok(None) => true,
            Err(_) => false,
        }
    }

    /// Removes the data file and its key, so nothing of this store survives.
    pub fn delete(&self) -> Result<(), StoreError> {
        match fs::remove_file(&self.file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(StoreError::Io(e.to_string())),
        }
        self.keys.remove()
    }

    pub fn corrupt_path(&self) -> PathBuf {
        self.file.with_extension("bin.corrupt")
    }

    fn key(&self) -> Result<Vec<u8>, StoreError> {
        if let Some(key) = self.keys.get()? {
            if key.len() == KEY_LEN {
                return Ok(key);
            }
            return Err(StoreError::NoSecretStore);
        }
        let mut key = vec![0u8; KEY_LEN];
        rand::fill(key.as_mut_slice());
        self.keys.set(&key)?;
        Ok(key)
    }

    fn cipher(&self, key: &[u8]) -> Result<Aes256Gcm, StoreError> {
        Aes256Gcm::new_from_slice(key).map_err(|_| StoreError::NoSecretStore)
    }

    fn staging_path(&self) -> PathBuf {
        self.file.with_extension("bin.staging")
    }

    fn quarantine(&self, reason: &str) -> StoreError {
        let _ = fs::rename(&self.file, self.corrupt_path());
        StoreError::Corrupt(reason.to_string())
    }
}

fn nonce_of(bytes: &[u8]) -> Result<AesNonce<Aes256Gcm>, StoreError> {
    AesNonce::<Aes256Gcm>::try_from(bytes).map_err(|_| StoreError::Corrupt("bad nonce length".into()))
}

/// The real OS secret store, namespaced by the app identifier so that an isolated verify build
/// can never read or delete the developer's own credentials (design §8).
pub struct KeyringSource {
    entry: keyring::Entry,
}

impl KeyringSource {
    /// `Err(NoSecretStore)` when the platform has no usable credential store right now.
    pub fn new(identifier: &str, name: &str) -> Result<Self, StoreError> {
        if cfg!(debug_assertions) && std::env::var_os("POSTLITE_FORCE_NO_SECRET_STORE").is_some() {
            return Err(StoreError::NoSecretStore);
        }
        let entry = keyring::Entry::new(&format!("{identifier}.postlite.store"), name)
            .map_err(|_| StoreError::NoSecretStore)?;
        match keyring::Entry::store_status() {
            &Ok(()) => Ok(Self { entry }),
            _ => Err(StoreError::NoSecretStore),
        }
    }
}

impl KeySource for KeyringSource {
    fn get(&self) -> Result<Option<Vec<u8>>, StoreError> {
        match self.entry.get_secret() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(StoreError::NoSecretStore),
        }
    }

    fn set(&self, key: &[u8]) -> Result<(), StoreError> {
        self.entry.set_secret(key).map_err(|_| StoreError::NoSecretStore)
    }

    fn remove(&self) -> Result<(), StoreError> {
        match self.entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(StoreError::NoSecretStore),
        }
    }
}

/// The key source on a machine whose OS has no usable secret store: every read and write refuses,
/// so an encrypted store over it stays in memory and reports why instead of writing plaintext.
pub struct NoSecretStoreKeys;

impl KeySource for NoSecretStoreKeys {
    fn get(&self) -> Result<Option<Vec<u8>>, StoreError> {
        Err(StoreError::NoSecretStore)
    }

    fn set(&self, key: &[u8]) -> Result<(), StoreError> {
        let _ = key;
        Err(StoreError::NoSecretStore)
    }

    fn remove(&self) -> Result<(), StoreError> {
        Ok(())
    }
}

/// Test-only key source: an in-memory keyring.
#[cfg(test)]
pub struct FakeKeySource {
    key: std::sync::Mutex<Option<Vec<u8>>>,
}

#[cfg(test)]
impl FakeKeySource {
    pub fn empty() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            key: std::sync::Mutex::new(None),
        })
    }

    pub fn with_key(key: Vec<u8>) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            key: std::sync::Mutex::new(Some(key)),
        })
    }
}

#[cfg(test)]
impl KeySource for FakeKeySource {
    fn get(&self) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self.key.lock().unwrap().clone())
    }

    fn set(&self, key: &[u8]) -> Result<(), StoreError> {
        *self.key.lock().unwrap() = Some(key.to_vec());
        Ok(())
    }

    fn remove(&self) -> Result<(), StoreError> {
        *self.key.lock().unwrap() = None;
        Ok(())
    }
}

/// A throwaway directory for the tests in this binary, removed when the test ends. Shared because
/// the store, the jar and the commands all need one.
#[cfg(test)]
pub struct TempDir(PathBuf);

#[cfg(test)]
impl TempDir {
    pub fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "postlite-test-{label}-{}-{}",
            std::process::id(),
            nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn files(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

#[cfg(test)]
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const SECRET: &[u8] = b"[{\"name\":\"sid\",\"value\":\"session-token-secret\"}]";

    #[test]
    fn round_trips_and_creates_the_key_on_first_use() {
        let dir = TempDir::new("roundtrip");
        let keys = FakeKeySource::empty();
        let store = EncryptedStore::open(dir.path(), "workspace-a", keys.clone());

        assert_eq!(store.load().unwrap(), None);
        store.save(SECRET).unwrap();

        assert_eq!(store.load().unwrap().as_deref(), Some(SECRET));
        assert_eq!(keys.get().unwrap().unwrap().len(), KEY_LEN);
    }

    #[test]
    fn written_bytes_never_contain_the_plaintext() {
        let dir = TempDir::new("plaintext");
        let store = EncryptedStore::open(dir.path(), "workspace-a", FakeKeySource::empty());
        store.save(SECRET).unwrap();

        let on_disk = fs::read(dir.path().join("workspace-a.bin")).unwrap();
        let text = String::from_utf8_lossy(&on_disk);
        assert!(!text.contains("session-token-secret"), "{text}");
        assert!(!text.contains("sid\""), "{text}");
    }

    #[test]
    fn each_write_uses_a_fresh_nonce() {
        let dir = TempDir::new("nonce");
        let keys = FakeKeySource::with_key(vec![7u8; KEY_LEN]);
        let store = EncryptedStore::open(dir.path(), "workspace-a", keys);

        store.save(SECRET).unwrap();
        let first = fs::read(dir.path().join("workspace-a.bin")).unwrap();
        store.save(SECRET).unwrap();
        let second = fs::read(dir.path().join("workspace-a.bin")).unwrap();

        assert_ne!(first, second);
        assert_eq!(store.load().unwrap().as_deref(), Some(SECRET));
    }

    #[test]
    fn wrong_key_is_quarantined_and_starts_empty() {
        let dir = TempDir::new("wrongkey");
        EncryptedStore::open(dir.path(), "workspace-a", FakeKeySource::with_key(vec![1u8; KEY_LEN]))
            .save(SECRET)
            .unwrap();

        let other = EncryptedStore::open(
            dir.path(),
            "workspace-a",
            FakeKeySource::with_key(vec![2u8; KEY_LEN]),
        );
        assert!(matches!(
            other.load(),
            Err(StoreError::Corrupt(_))
        ));
        assert!(dir.path().join("workspace-a.bin.corrupt").exists());
        assert!(!dir.path().join("workspace-a.bin").exists());
    }

    #[test]
    fn truncated_and_bad_magic_and_version_are_quarantined() {
        for (label, bytes) in [
            ("short", b"PLCS\x01abc".to_vec()),
            (
                "magic",
                [b"XXXX".to_vec(), b"\x01".to_vec(), vec![0u8; NONCE_LEN], vec![9u8; 32]].concat(),
            ),
            (
                "version",
                [MAGIC.to_vec(), b"\x02".to_vec(), vec![0u8; NONCE_LEN], vec![9u8; 32]].concat(),
            ),
        ] {
            let dir = TempDir::new(label);
            fs::write(dir.path().join("workspace-a.bin"), &bytes).unwrap();
            let store = EncryptedStore::open(dir.path(), "workspace-a", FakeKeySource::empty());
            assert!(
                matches!(store.load(), Err(StoreError::Corrupt(_))),
                "{label} must be reported corrupt"
            );
            assert!(dir.path().join("workspace-a.bin.corrupt").exists(), "{label}");
        }
    }

    #[test]
    fn no_secret_store_refuses_to_write_but_reads_an_absent_file_as_empty() {
        let dir = TempDir::new("nostore");
        let store = EncryptedStore::open(dir.path(), "workspace-a", Arc::new(NoSecretStoreKeys));

        assert_eq!(store.save(SECRET), Err(StoreError::NoSecretStore));
        assert_eq!(store.load().unwrap(), None);
        assert!(!dir.path().join("workspace-a.bin").exists());
        assert!(!dir.path().join("workspace-a.bin.corrupt").exists());

        // A file that cannot be decrypted without a key is reported, not invented.
        fs::write(dir.path().join("workspace-a.bin"), [MAGIC.to_vec(), b"\x01".to_vec(), vec![0u8; 44]].concat()).unwrap();
        assert!(matches!(store.load(), Err(StoreError::NoSecretStore)));
    }

    #[test]
    fn delete_removes_file_and_key() {
        let dir = TempDir::new("delete");
        let keys = FakeKeySource::empty();
        let store = EncryptedStore::open(dir.path(), "workspace-a", keys.clone());
        store.save(SECRET).unwrap();

        store.delete().unwrap();

        assert!(!dir.path().join("workspace-a.bin").exists());
        assert_eq!(keys.get().unwrap(), None);
        // Deleting twice, or before anything existed, is not an error.
        store.delete().unwrap();
    }

    #[test]
    fn a_short_key_in_the_store_is_refused_rather_than_padded() {
        let dir = TempDir::new("shortkey");
        let store = EncryptedStore::open(dir.path(), "workspace-a", FakeKeySource::with_key(vec![3u8; 8]));
        assert_eq!(store.save(SECRET), Err(StoreError::NoSecretStore));
    }

    #[test]
    fn usable_only_when_a_key_can_be_read_or_created() {
        let dir = TempDir::new("usable");
        assert!(EncryptedStore::open(dir.path(), "workspace-a", FakeKeySource::empty()).secret_store_usable());
        let short = EncryptedStore::open(dir.path(), "workspace-a", FakeKeySource::with_key(vec![3u8; 8]));
        assert!(!short.secret_store_usable());
        let none = EncryptedStore::open(dir.path(), "workspace-a", Arc::new(NoSecretStoreKeys));
        assert!(!none.secret_store_usable());
    }
}
