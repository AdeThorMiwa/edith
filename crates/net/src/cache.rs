//! A small on-disk cache for GET responses, keyed by the SHA-256 of the URL. Entries carry the
//! validators (`ETag`, `Last-Modified`) so a repeat request is conditional and a `304` is served
//! from disk. The URL itself is never written to disk, only its hash.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
pub struct Meta {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_type: Option<String>,
}

pub struct Entry {
    pub meta: Meta,
    pub body: Vec<u8>,
}

pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub fn new(dir: &Path) -> Self {
        Self { dir: dir.to_path_buf() }
    }

    fn stem(&self, url: &str) -> PathBuf {
        let digest = Sha256::digest(url.as_bytes());
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        self.dir.join(hex)
    }

    pub fn load(&self, url: &str, max_bytes: u64) -> Option<Entry> {
        let stem = self.stem(url);
        let body_path = stem.with_extension("body");
        if std::fs::metadata(&body_path).ok()?.len() > max_bytes {
            return None;
        }
        let meta: Meta = serde_json::from_slice(&std::fs::read(stem.with_extension("meta")).ok()?).ok()?;
        Some(Entry { meta, body: std::fs::read(body_path).ok()? })
    }

    /// Stores an entry (best effort: a full disk just means no caching). Writes to a temporary
    /// name first, so a crash never leaves a half-written entry that looks valid.
    pub fn store(&self, url: &str, meta: &Meta, body: &[u8]) {
        if std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        let stem = self.stem(url);
        let Ok(meta_json) = serde_json::to_vec(meta) else { return };
        let write = |ext: &str, data: &[u8]| -> std::io::Result<()> {
            let tmp = stem.with_extension(format!("{ext}.tmp"));
            std::fs::write(&tmp, data)?;
            std::fs::rename(&tmp, stem.with_extension(ext))
        };
        // Body first: an entry is valid only when its meta file exists.
        if write("body", body).is_err() || write("meta", &meta_json).is_err() {
            let _ = std::fs::remove_file(stem.with_extension("meta"));
        }
    }
}
