//! The Offline switch and the request log.

use crate::NetError;
use serde::Serialize;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// Entries kept in the log; older ones are dropped.
pub const LOG_CAP: usize = 500;

/// How a request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok,
    Cached,
    Offline,
    Failed,
}

/// One logged request. Holds no query string, header, body or secret.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LogEntry {
    pub seq: u64,
    /// Milliseconds since the Unix epoch.
    pub at_ms: u64,
    pub method: String,
    /// `scheme://host/path` only (see [`crate::redact::url`]).
    pub url: String,
    pub outcome: Outcome,
    pub status: Option<u16>,
    pub bytes: u64,
    pub millis: u64,
    /// Redacted error text for failed requests.
    pub error: Option<String>,
}

/// Offline flag plus request log. Cheap to share; all methods take `&self`.
#[derive(Debug, Default)]
pub struct NetState {
    offline: AtomicBool,
    seq: AtomicU64,
    log: Mutex<VecDeque<LogEntry>>,
    /// The marker file that remembers Offline mode across restarts (see [`NetState::persist_in`]).
    marker: Mutex<Option<PathBuf>>,
}

/// File name of the Offline marker inside the data directory: the file exists while Offline.
pub const OFFLINE_MARKER: &str = "network-offline";

impl NetState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn offline(&self) -> bool {
        self.offline.load(Ordering::SeqCst)
    }

    pub fn set_offline(&self, on: bool) {
        self.offline.store(on, Ordering::SeqCst);
        let marker = self.marker.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(path) = marker {
            // Best effort: a read-only data directory only means the choice is not remembered.
            if on {
                let _ = std::fs::write(&path, b"offline\n");
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    /// Remembers Offline mode in `dir` from now on, and adopts what a previous run left there.
    /// Call once at start-up, before any request.
    pub fn persist_in(&self, dir: &Path) {
        let path = dir.join(OFFLINE_MARKER);
        let was_offline = path.exists();
        *self.marker.lock().unwrap_or_else(PoisonError::into_inner) = Some(path);
        self.offline.store(was_offline, Ordering::SeqCst);
    }

    /// `Err(Offline)` while Offline mode is on. Every outgoing call starts with this.
    pub fn check(&self) -> Result<(), NetError> {
        if self.offline() { Err(NetError::Offline) } else { Ok(()) }
    }

    /// Appends an entry (the URL and error text are redacted here, once more, as a safety net).
    #[allow(clippy::too_many_arguments)]
    pub fn record(&self, method: &str, url: &str, outcome: Outcome, status: Option<u16>, bytes: u64, millis: u64, error: Option<&str>) {
        let entry = LogEntry {
            seq: self.seq.fetch_add(1, Ordering::SeqCst).saturating_add(1),
            at_ms: now_ms(),
            method: method.chars().filter(char::is_ascii_alphabetic).take(10).collect::<String>().to_ascii_uppercase(),
            url: crate::redact::url(url),
            outcome,
            status,
            bytes,
            millis,
            error: error.map(crate::redact::message),
        };
        let mut log = self.log.lock().unwrap_or_else(PoisonError::into_inner);
        if log.len() >= LOG_CAP {
            log.pop_front();
        }
        log.push_back(entry);
    }

    /// The newest `limit` entries (all when `None`), oldest first.
    pub fn entries(&self, limit: Option<usize>) -> Vec<LogEntry> {
        let log = self.log.lock().unwrap_or_else(PoisonError::into_inner);
        let skip = limit.map_or(0, |l| log.len().saturating_sub(l));
        log.iter().skip(skip).cloned().collect()
    }

    pub fn clear_log(&self) {
        self.log.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The process-wide state used by the app and the commands.
pub fn shared() -> &'static Arc<NetState> {
    static SHARED: OnceLock<Arc<NetState>> = OnceLock::new();
    SHARED.get_or_init(|| Arc::new(NetState::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_blocks_check() {
        let s = NetState::new();
        assert_eq!(s.check(), Ok(()));
        s.set_offline(true);
        assert_eq!(s.check(), Err(NetError::Offline));
        s.set_offline(false);
        assert!(s.check().is_ok());
    }

    #[test]
    fn log_is_redacted_and_capped() {
        let s = NetState::new();
        s.record("get", "https://u:p@h.test/x?key=SECRET", Outcome::Failed, None, 0, 5, Some("bad Bearer sk-1 token=abc"));
        let e = s.entries(None);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].url, "https://h.test/x");
        assert_eq!(e[0].method, "GET");
        let text = format!("{:?}", e[0]);
        for leak in ["SECRET", "sk-1", "abc", "u:p"] {
            assert!(!text.contains(leak), "{text}");
        }
        for i in 0..(LOG_CAP + 20) {
            s.record("GET", "https://h.test/", Outcome::Ok, Some(200), i as u64, 1, None);
        }
        let all = s.entries(None);
        assert_eq!(all.len(), LOG_CAP);
        assert!(all.windows(2).all(|w| w[0].seq < w[1].seq));
        assert_eq!(s.entries(Some(3)).len(), 3);
        s.clear_log();
        assert!(s.entries(None).is_empty());
    }

    #[test]
    fn offline_mode_is_remembered_in_the_data_directory() {
        let dir = std::env::temp_dir().join(format!("fc-net-offline-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = NetState::new();
        a.persist_in(&dir);
        assert!(!a.offline());
        a.set_offline(true);
        assert!(dir.join(OFFLINE_MARKER).exists());
        let b = NetState::new();
        b.persist_in(&dir);
        assert!(b.offline(), "a new run starts Offline when the last one ended Offline");
        b.set_offline(false);
        assert!(!dir.join(OFFLINE_MARKER).exists());
        // A missing directory never panics and still works in memory.
        let c = NetState::new();
        c.persist_in(&dir.join("missing"));
        c.set_offline(true);
        assert!(c.offline());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shared_is_one_instance() {
        assert!(std::ptr::eq(shared(), shared()));
    }
}
