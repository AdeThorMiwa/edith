//! The secret store: API keys and tokens live in the operating system's keychain, never in a
//! project file, a preferences file, a log or a command result.
//!
//! - macOS: the login Keychain. Windows: Credential Manager. Linux/FreeBSD: the Secret Service
//!   (GNOME Keyring, KWallet) over D-Bus.
//! - There is no plaintext fallback. If the OS store is missing, saving fails with a clear
//!   message.
//! - [`SecretStore::set`] is for the Settings UI only; commands may call [`SecretStore::has`] and
//!   [`SecretStore::delete`] but never [`SecretStore::get`], which exists for the code that
//!   sends a key to its own provider.

use crate::{NetError, redact};
use keyring_core::{CredentialStore, Entry};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use zeroize::Zeroizing;

/// The keychain "service" every Edith entry is filed under.
pub const SERVICE: &str = crate::brand::KEYCHAIN_SERVICE;
const MAX_VALUE: usize = 8 * 1024;

/// Secret names are lower-case words joined by `.`, `_` or `-`, up to 64 characters, such as
/// `provider.openrouter.api_key`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
}

pub struct SecretStore {
    store: Arc<CredentialStore>,
}

impl SecretStore {
    /// A store over any `keyring-core` credential store (tests use the crate's mock store).
    pub fn with_store(store: Arc<CredentialStore>) -> Self {
        Self { store }
    }

    /// The operating system's store. Fails (never panics) when there is none, for example a
    /// Linux session without a Secret Service.
    pub fn os() -> Result<Self, NetError> {
        Ok(Self { store: os_store()? })
    }

    fn entry(&self, name: &str) -> Result<Entry, NetError> {
        if !valid_name(name) {
            return Err(NetError::Secret("invalid secret name".to_string()));
        }
        self.store.build(SERVICE, name, None).map_err(map_err)
    }

    /// Saves `value` (surrounding whitespace is trimmed, because pasted keys often carry a
    /// newline). Refuses empty values and anything over 8 KiB.
    pub fn set(&self, name: &str, value: &str) -> Result<(), NetError> {
        let value = value.trim();
        if value.is_empty() || value.len() > MAX_VALUE || value.chars().any(char::is_control) {
            return Err(NetError::Secret("the secret is empty, too long or contains control characters".to_string()));
        }
        self.entry(name)?.set_password(value).map_err(map_err)
    }

    /// Whether a secret with this name is stored. Does not read the secret where the store can
    /// search by name (macOS then shows no keychain prompt for it).
    pub fn has(&self, name: &str) -> Result<bool, NetError> {
        let entry = self.entry(name)?;
        let spec = HashMap::from([("service", SERVICE), ("user", name)]);
        match self.store.search(&spec) {
            // Stores may match loosely (Windows treats the values as patterns), so insist on the exact name.
            Ok(found) => Ok(found.iter().any(|e| e.get_specifiers().is_some_and(|(svc, user)| svc == SERVICE && user == name))),
            Err(keyring_core::Error::NotSupportedByStore(_)) => match entry.get_secret() {
                Ok(_) => Ok(true),
                Err(keyring_core::Error::NoEntry) => Ok(false),
                Err(e) => Err(map_err(e)),
            },
            Err(e) => Err(map_err(e)),
        }
    }

    /// The secret, zeroed from memory when dropped. Only for code that sends it to the provider
    /// it belongs to; never return it from a command, log it or put it in an error.
    pub fn get(&self, name: &str) -> Result<Option<Zeroizing<String>>, NetError> {
        match self.entry(name)?.get_password() {
            Ok(v) => Ok(Some(Zeroizing::new(v))),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(map_err(e)),
        }
    }

    /// Deletes the secret. `Ok(false)` when there was none.
    pub fn delete(&self, name: &str) -> Result<bool, NetError> {
        match self.entry(name)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring_core::Error::NoEntry) => Ok(false),
            Err(e) => Err(map_err(e)),
        }
    }
}

static INSTALLED: OnceLock<SecretStore> = OnceLock::new();

/// Makes `store` the process-wide store instead of the OS keychain. For tests and tools: call it
/// before the first [`os`]. Returns `false` (and changes nothing) if a store is already installed.
pub fn install(store: SecretStore) -> bool {
    INSTALLED.set(store).is_ok()
}

/// A store that lives in memory only (lost at exit). For tests; never a fallback for the keychain.
pub fn in_memory() -> Result<SecretStore, NetError> {
    let mock = keyring_core::mock::Store::new().map_err(map_err)?;
    Ok(SecretStore::with_store(Arc::new(NoSearch(mock))))
}

/// The mock store with search switched off (its search matches loosely): `has` falls back to
/// reading the secret, which is how several real stores behave too.
struct NoSearch(Arc<keyring_core::mock::Store>);

impl keyring_core::api::CredentialStoreApi for NoSearch {
    fn vendor(&self) -> String {
        "edith in-memory store".to_string()
    }
    fn id(&self) -> String {
        "in-memory".to_string()
    }
    fn build(&self, service: &str, user: &str, modifiers: Option<&HashMap<&str, &str>>) -> keyring_core::Result<Entry> {
        self.0.build(service, user, modifiers)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// The process-wide store: the installed one, else the operating system's, created on first use.
pub fn os() -> Result<&'static SecretStore, NetError> {
    if let Some(s) = INSTALLED.get() {
        return Ok(s);
    }
    static OS: OnceLock<Result<SecretStore, NetError>> = OnceLock::new();
    OS.get_or_init(SecretStore::os).as_ref().map_err(Clone::clone)
}

fn map_err(e: keyring_core::Error) -> NetError {
    use keyring_core::Error as E;
    let msg = match e {
        E::NoStorageAccess(_) => "the keychain is locked or access was denied".to_string(),
        E::NoDefaultStore => "no keychain is available on this system".to_string(),
        E::TooLong(..) => "the secret is too long for the keychain".to_string(),
        E::Invalid(..) => "the keychain refused the request".to_string(),
        // Never include `BadEncoding`/`BadDataFormat` payloads: they hold the stored bytes.
        E::BadEncoding(_) | E::BadDataFormat(..) => "the stored value is not readable".to_string(),
        other => redact::message(&other.to_string()),
    };
    NetError::Secret(msg)
}

#[cfg(target_os = "macos")]
fn os_store() -> Result<Arc<CredentialStore>, NetError> {
    let store: Arc<CredentialStore> = apple_native_keyring_store::keychain::Store::new().map_err(map_err)?;
    Ok(store)
}

#[cfg(target_os = "windows")]
fn os_store() -> Result<Arc<CredentialStore>, NetError> {
    let store: Arc<CredentialStore> = windows_native_keyring_store::Store::new().map_err(map_err)?;
    Ok(store)
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn os_store() -> Result<Arc<CredentialStore>, NetError> {
    let store: Arc<CredentialStore> = zbus_secret_service_keyring_store::Store::new().map_err(map_err)?;
    Ok(store)
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux", target_os = "freebsd")))]
fn os_store() -> Result<Arc<CredentialStore>, NetError> {
    Err(NetError::Secret("no keychain is available on this system".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyring_core::api::CredentialStoreApi;

    fn mock() -> SecretStore {
        in_memory().unwrap()
    }

    #[test]
    fn round_trip_set_has_get_delete() {
        let s = mock();
        assert!(!s.has("provider.x.api_key").unwrap());
        assert_eq!(s.get("provider.x.api_key").unwrap().as_deref().map(String::as_str), None);
        s.set("provider.x.api_key", "  sk-test-123\n").unwrap();
        assert!(s.has("provider.x.api_key").unwrap());
        assert_eq!(s.get("provider.x.api_key").unwrap().as_deref().map(String::as_str), Some("sk-test-123"));
        assert!(s.delete("provider.x.api_key").unwrap());
        assert!(!s.has("provider.x.api_key").unwrap());
        assert!(!s.delete("provider.x.api_key").unwrap());
    }

    #[test]
    fn names_and_values_are_validated() {
        let s = mock();
        for bad in ["", "Has Space", "UPPER", "a/b", "a\n", &"a".repeat(65), "é"] {
            assert!(matches!(s.set(bad, "v"), Err(NetError::Secret(_))), "{bad:?}");
            assert!(s.has(bad).is_err() && s.get(bad).is_err() && s.delete(bad).is_err());
        }
        for bad in ["", "   ", "a\u{0}b", "a\nb", &"x".repeat(MAX_VALUE + 1)] {
            assert!(matches!(s.set("ok.name", bad), Err(NetError::Secret(_))), "{bad:?}");
        }
        assert!(!s.has("ok.name").unwrap());
    }

    #[test]
    fn errors_never_contain_the_secret() {
        let s = mock();
        let e = s.set("ok.name", &"x".repeat(MAX_VALUE + 1)).unwrap_err().to_string();
        assert!(!e.contains("xxxx"), "{e}");
        let e = map_err(keyring_core::Error::BadEncoding(b"sk-LEAKED".to_vec())).to_string();
        assert!(!e.contains("LEAKED"), "{e}");
    }

    #[test]
    fn a_store_failure_is_an_error_not_a_panic() {
        let store = keyring_core::mock::Store::new().expect("mock store");
        let s = SecretStore::with_store(store.clone());
        let entry = store.build(SERVICE, "bad.entry", None).unwrap();
        let cred = entry.as_any().downcast_ref::<keyring_core::mock::Cred>().unwrap();
        cred.set_error(keyring_core::Error::NoStorageAccess("locked".into()));
        let e = s.get("bad.entry").unwrap_err();
        assert!(matches!(&e, NetError::Secret(m) if m.contains("locked or access was denied")), "{e:?}");
    }

    #[test]
    fn search_hits_must_match_the_exact_name() {
        // The mock store registers an entry when it is built and matches loosely: a store that
        // supports search but matches by substring must not make `has("a")` true because of "ab".
        let store = keyring_core::mock::Store::new().expect("mock store");
        let s = SecretStore::with_store(store.clone());
        s.set("ab", "v").unwrap();
        assert!(s.has("ab").unwrap());
        let other = store.build(SERVICE, "abc", None).unwrap();
        other.set_password("v").unwrap();
        let spec = HashMap::from([("service", SERVICE), ("user", "ab")]);
        assert_eq!(store.search(&spec).unwrap().len(), 2, "the mock matches loosely");
        s.delete("abc").unwrap();
        assert!(s.has("ab").unwrap());
        s.delete("ab").unwrap();
    }

    #[test]
    fn names_are_checked_by_valid_name() {
        assert!(valid_name("provider.openrouter.api_key") && valid_name("a-b_c.1"));
        assert!(!valid_name("") && !valid_name("A") && !valid_name("a b"));
    }

    /// Talks to the real login keychain with a throwaway item; macOS may ask for access.
    /// `cargo test -p edith-net --features keychain -- --ignored real_os_keychain`
    #[test]
    #[ignore = "uses the real OS keychain"]
    fn real_os_keychain_round_trip() {
        let s = SecretStore::os().unwrap();
        let name = format!("test.round-trip-{}", std::process::id());
        s.set(&name, "throwaway-value").unwrap();
        assert!(s.has(&name).unwrap());
        assert_eq!(s.get(&name).unwrap().as_deref().map(String::as_str), Some("throwaway-value"));
        assert!(s.delete(&name).unwrap());
        assert!(!s.has(&name).unwrap());
    }
}
