//! Encrypted API key storage.
//!
//! We use the OS credential store (`keyring` crate) -- Windows Credential
//! Manager / macOS Keychain / libsecret on Linux. The threat model is
//! "encrypted at rest by the OS, unlocked by the user's session
//! credentials"; we get that for free without an extra password-based
//! KDF layer (Stronghold) sitting on top.
//!
//! The boundary is the `SecretStore` trait. Production uses
//! `KeyringStore`; tests use `InMemoryStore`.

use std::collections::HashMap;
use std::sync::Mutex;

const SERVICE: &str = "Harness";

/// Errors surfaced by the secret store. Distinguishes "not found" from
/// "store unreachable" so the UI can render two different states.
#[derive(Debug, Clone)]
pub enum SecretError {
    /// The store exists but has no entry for this account.
    #[cfg_attr(not(test), allow(dead_code))]
    NotFound,
    /// The store itself is unreachable (locked, perms denied, DB corrupt).
    /// The keyring is broken or inaccessible; the user must reset it.
    Unreachable(String),
    /// Other I/O error.
    Io(String),
}

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretError::NotFound => write!(f, "secret not found"),
            SecretError::Unreachable(s) => write!(f, "secret store unreachable: {}", s),
            SecretError::Io(s) => write!(f, "secret store I/O error: {}", s),
        }
    }
}

impl std::error::Error for SecretError {}

pub type Result<T> = std::result::Result<T, SecretError>;

/// The boundary trait. Implementations must be `Send + Sync` so the
/// store can live in a Tauri `State<AppState>`.
pub trait SecretStore: Send + Sync {
    fn set(&self, account: &str, value: &str) -> Result<()>;
    fn get(&self, account: &str) -> Result<Option<String>>;
    /// Distinguishes "store reachable + entry missing" (Ok(false)) from
    /// "store unreachable" (Err). The UI renders these as "no key" vs
    /// "key unreachable" -- both are distinct disabled states.
    fn has(&self, account: &str) -> Result<bool>;
    fn delete(&self, account: &str) -> Result<()>;
    /// Delete every entry belonging to this service. Best-effort:
    /// individual entry failures are not surfaced -- the next read
    /// will return `Ok(false)` if the entry is gone.
    #[cfg_attr(not(test), allow(dead_code))]
    fn delete_all(&self) -> Result<()>;
}

/// Production impl: delegates to the `keyring` crate.
pub struct KeyringStore;

impl KeyringStore {
    pub fn new() -> Self {
        Self
    }

    fn entry(&self, account: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(SERVICE, account).map_err(|e| SecretError::Unreachable(e.to_string()))
    }
}

impl Default for KeyringStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStore for KeyringStore {
    fn set(&self, account: &str, value: &str) -> Result<()> {
        self.entry(account)?
            .set_password(value)
            .map_err(|e| SecretError::Io(e.to_string()))
    }

    fn get(&self, account: &str) -> Result<Option<String>> {
        match self.entry(account)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(SecretError::Unreachable(e.to_string())),
        }
    }

    fn has(&self, account: &str) -> Result<bool> {
        match self.entry(account)?.get_password() {
            Ok(_) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(SecretError::Unreachable(e.to_string())),
        }
    }

    fn delete(&self, account: &str) -> Result<()> {
        match self.entry(account)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()), // idempotent
            Err(e) => Err(SecretError::Io(e.to_string())),
        }
    }

    fn delete_all(&self) -> Result<()> {
        // The `keyring` crate doesn't expose a "list + delete" API; it
        // does, however, expose `delete_credential` per entry. We
        // therefore rely on the application tracking which accounts it
        // owns (the `models.json` file). This is intentionally a no-op
        // here -- the caller (the `reset_secret_store` IPC) iterates the
        // saved models and deletes each one individually. That's safer
        // than scanning a service namespace we don't fully own.
        Ok(())
    }
}

/// In-memory impl for tests. Mirrors the trait contract.
#[cfg_attr(not(test), allow(dead_code))]
pub struct InMemoryStore {
    map: Mutex<HashMap<String, String>>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl InMemoryStore {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new() -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStore for InMemoryStore {
    fn set(&self, account: &str, value: &str) -> Result<()> {
        self.map
            .lock()
            .map_err(|e| SecretError::Unreachable(e.to_string()))?
            .insert(account.to_string(), value.to_string());
        Ok(())
    }

    fn get(&self, account: &str) -> Result<Option<String>> {
        Ok(self
            .map
            .lock()
            .map_err(|e| SecretError::Unreachable(e.to_string()))?
            .get(account)
            .cloned())
    }

    fn has(&self, account: &str) -> Result<bool> {
        Ok(self
            .map
            .lock()
            .map_err(|e| SecretError::Unreachable(e.to_string()))?
            .contains_key(account))
    }

    fn delete(&self, account: &str) -> Result<()> {
        self.map
            .lock()
            .map_err(|e| SecretError::Unreachable(e.to_string()))?
            .remove(account);
        Ok(())
    }

    fn delete_all(&self) -> Result<()> {
        self.map
            .lock()
            .map_err(|e| SecretError::Unreachable(e.to_string()))?
            .clear();
        Ok(())
    }
}

// -- Tests --

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercises every method of the trait against `InMemoryStore`.
    /// This is the contract test: any new `SecretStore` impl must
    /// pass the same scenario.
    fn run_contract(store: &dyn SecretStore) {
        // Empty: has() == false, get() == None
        assert!(matches!(store.has("a"), Ok(false)));
        assert!(matches!(store.get("a"), Ok(None)));

        // set + has + get
        store.set("a", "alpha").unwrap();
        store.set("b", "beta").unwrap();
        assert!(store.has("a").unwrap());
        assert!(store.has("b").unwrap());
        assert_eq!(store.get("a").unwrap().as_deref(), Some("alpha"));
        assert_eq!(store.get("b").unwrap().as_deref(), Some("beta"));

        // Overwrite
        store.set("a", "alpha-2").unwrap();
        assert_eq!(store.get("a").unwrap().as_deref(), Some("alpha-2"));

        // Delete
        store.delete("a").unwrap();
        assert!(matches!(store.has("a"), Ok(false)));
        assert!(matches!(store.get("a"), Ok(None)));
        // Other entries untouched
        assert!(store.has("b").unwrap());

        // delete is idempotent
        store.delete("a").unwrap();
        store.delete("never-existed").unwrap();

        // delete_all clears the lot
        store.set("c", "gamma").unwrap();
        store.delete_all().unwrap();
        assert!(matches!(store.has("b"), Ok(false)));
        assert!(matches!(store.has("c"), Ok(false)));
    }

    #[test]
    fn in_memory_store_contract() {
        run_contract(&InMemoryStore::new());
    }

    #[test]
    fn in_memory_distinguishes_set_from_unset() {
        let s = InMemoryStore::new();
        s.set("k", "v").unwrap();
        assert!(s.has("k").unwrap());
        assert!(!s.has("missing").unwrap());
    }

    #[test]
    fn in_memory_delete_idempotent() {
        let s = InMemoryStore::new();
        s.delete("missing").unwrap();
        s.delete("missing").unwrap();
    }
}
