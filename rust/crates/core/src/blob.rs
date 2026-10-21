//! Content-addressed blob storage.
//!
//! Blobs are stored at `~/.clavus/objects/{2-char-prefix}/{hash}`.
//! SHA256 of the content is used as the address. Same content always
//! maps to the same address (deduplication by construction).

use crate::types::Snapshot;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Root directory for the Clavus store.
pub const DEFAULT_CLAVUS_DIR: &str = ".clavus";
const OBJECTS_DIR: &str = "objects";
const REFS_DIR: &str = "refs";
const INDEX_FILE: &str = "index.json";
#[allow(dead_code)]
const CONFIG_FILE: &str = "config.json"; // TODO: use in config management
const LOCK_SUFFIX: &str = ".lock";

/// Content-addressed storage engine.
#[derive(Debug, Clone)]
pub struct BlobStore {
    pub root: PathBuf,
    objects_dir: PathBuf,
    refs_dir: PathBuf,
    index_path: PathBuf,
}

impl BlobStore {
    // ─── Initialization ──────────────────────────────────────────────

    /// Open or create a Clavus store at `root` (default: `~/.clavus`).
    pub fn new(root: PathBuf) -> io::Result<Self> {
        let store = Self {
            objects_dir: root.join(OBJECTS_DIR),
            refs_dir: root.join(REFS_DIR),
            index_path: root.join(INDEX_FILE),
            root,
        };
        store.init()?;
        Ok(store)
    }

    /// Initialize the store directories. Safe to call repeatedly.
    pub fn init(&self) -> io::Result<()> {
        fs::create_dir_all(&self.root)?;
        fs::create_dir_all(&self.objects_dir)?;
        fs::create_dir_all(&self.refs_dir)?;
        debug!("BlobStore initialized at {}", self.root.display());
        Ok(())
    }

    // ─── Blob I/O ────────────────────────────────────────────────────

    /// Store `data` and return its SHA256 hash.
    ///
    /// SAFETY: writes atomically (temp file → rename) to avoid orphan blobs.
    /// If a crash occurs between blob-write and metadata-write, the orphan blob
    /// is harmless (metadata-written-first order). This is why `store_sample`
    /// writes the `.sample` metadata file BEFORE the blob data.
    pub fn put(&self, data: &[u8]) -> String {
        let hash = Self::sha256(data);
        let path = self.blob_path(&hash);
        if !path.exists() {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Err(e) = atomic_write(&path, data) {
                warn!("put failed: {}", e);
            }
        }
        hash
    }

    /// Retrieve a blob by hash. Returns `None` if not found.
    pub fn get(&self, hash: &str) -> Option<Vec<u8>> {
        let path = self.resolve_path(hash)?;
        fs::read(path).ok()
    }

    /// Check if a blob exists.
    pub fn has(&self, hash: &str) -> bool {
        self.resolve_path(hash).map(|p| p.exists()).unwrap_or(false)
    }

    /// Compute SHA256 of `data` as a hex string.
    pub fn sha256(data: &[u8]) -> String {
        format!("{:064x}", Sha256::digest(data))
    }

    // ─── Path helpers ────────────────────────────────────────────────

    fn blob_path(&self, hash: &str) -> PathBuf {
        self.objects_dir.join(&hash[..2]).join(hash)
    }

    /// List the hashes of every stored snapshot metadata file.
    ///
    /// Used to resolve a short hash prefix to a full snapshot hash.
    pub fn list_meta_hashes(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(dirs) = fs::read_dir(&self.objects_dir) {
            for dir in dirs.flatten() {
                if !dir.path().is_dir() {
                    continue;
                }
                if let Ok(entries) = fs::read_dir(dir.path()) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if let Some(stripped) = name.strip_suffix(".meta") {
                            if stripped.len() == 64 {
                                out.push(stripped.to_string());
                            }
                        }
                    }
                }
            }
        }
        out.sort();
        out
    }

    /// Path to the metadata file for a snapshot.
    pub fn meta_path(&self, hash: &str) -> PathBuf {
        self.objects_dir.join(&hash[..2]).join(format!("{}.meta", hash))
    }

    /// Path to the sample metadata file.
    pub fn sample_meta_path(&self, hash: &str) -> PathBuf {
        self.objects_dir.join(&hash[..2]).join(format!("{}.sample", hash))
    }

    /// Resolve a short (8–63 char) hash to the full 64-char path.
    /// Returns `None` if no matching blob exists.
    fn resolve_path(&self, hash: &str) -> Option<PathBuf> {
        if hash.len() == 64 && !hash.contains('.') {
            let p = self.blob_path(hash);
            return Some(p);
        }

        if hash.len() < 2 {
            return None;
        }
        let prefix_dir = self.objects_dir.join(&hash[..2]);
        if !prefix_dir.is_dir() {
            return None;
        }

        for entry in fs::read_dir(&prefix_dir).ok()? {
            let entry = entry.ok()?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            // Only match bare blobs (no .meta or .sample suffix)
            if name_str.starts_with(hash) && !name_str.contains('.') && name_str.len() == 64 {
                return Some(entry.path());
            }
        }
        None
    }

    // ─── Snapshot metadata ──────────────────────────────────────────

    /// Write snapshot metadata atomically (rename over .tmp).
    pub fn write_meta(&self, snapshot: &Snapshot) -> io::Result<()> {
        let path = self.meta_path(&snapshot.hash);
        // Create prefix directory if it doesn't exist (e.g. objects/ab/)
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = PathBuf::from(path.to_string_lossy().to_string() + ".tmp");
        let json = serde_json::to_string_pretty(snapshot)?;
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &path)?;
        debug!("Wrote snapshot meta: {}", snapshot.short_hash(8));
        Ok(())
    }

    /// Load snapshot metadata. Returns `None` if not found or corrupt.
    pub fn load_meta(&self, hash: &str) -> Option<Snapshot> {
        let path = self.meta_path(hash);
        if !path.exists() {
            return None;
        }
        let data = fs::read_to_string(&path).ok()?;
        serde_json::from_str(&data).ok()
    }

    // ─── Sample blob + metadata ──────────────────────────────────────

    /// Store an audio sample blob with its filename metadata.
    ///
    /// Writes metadata BEFORE the blob data to prevent orphan blobs:
    /// if a crash occurs after metadata-write but before blob-write, the orphan
    /// metadata file is harmless (BlobStore won't find a blob without metadata).
    pub fn store_sample(
        &self,
        file_path: &Path,
        relative_path: &str,
    ) -> io::Result<(String, String)> {
        let data = fs::read(file_path)?;
        let hash = Self::sha256(&data);
        let path = self.blob_path(&hash);

        let filename = file_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        // Write metadata FIRST — prevents orphan blob if crash occurs here
        let meta_path = self.sample_meta_path(&hash);
        if let Some(parent) = meta_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let meta = format!("{}\n{}", filename, relative_path.replace('\\', "/"));
        fs::write(&meta_path, meta)?;

        // Then write blob atomically
        if !path.exists() {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            atomic_write(&path, &data)?;
        }

        Ok((hash, filename))
    }

    /// Get the original filename for a stored sample.
    pub fn sample_filename(&self, hash: &str) -> Option<String> {
        let meta = fs::read_to_string(self.sample_meta_path(hash)).ok()?;
        meta.lines().next().map(|s| s.to_string())
    }

    /// Get the relative path for a stored sample.
    pub fn sample_relpath(&self, hash: &str) -> Option<String> {
        let meta = fs::read_to_string(self.sample_meta_path(hash)).ok()?;
        let mut lines = meta.lines();
        lines.next(); // skip filename
        lines.next().map(|s| s.replace('\\', "/"))
    }

    // ─── Refs (HEAD, branches, tags) ─────────────────────────────────

    /// Write a named reference.
    pub fn update_ref(&self, name: &str, hash: &str) -> io::Result<()> {
        let path = self.refs_dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, hash)?;
        debug!("Updated ref {} → {}", name, hash);
        Ok(())
    }

    /// Read a named reference.
    pub fn read_ref(&self, name: &str) -> Option<String> {
        let path = self.refs_dir.join(name);
        fs::read_to_string(&path).ok().map(|s| s.trim().to_string())
    }

    /// Delete a named reference.
    pub fn delete_ref(&self, name: &str) -> io::Result<()> {
        let path = self.refs_dir.join(name);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    // ─── Index (project registry) ───────────────────────────────────

    /// Read the full index. Returns empty map if missing.
    pub fn read_index(&self) -> HashMap<String, serde_json::Value> {
        if !self.index_path.exists() {
            return HashMap::new();
        }
        fs::read_to_string(&self.index_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Write the full index atomically.
    pub fn write_index(&self, index: &HashMap<String, serde_json::Value>) -> io::Result<()> {
        let tmp = PathBuf::from(self.index_path.to_string_lossy().to_string() + ".tmp");
        let json = serde_json::to_string_pretty(index)?;
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &self.index_path)?;
        Ok(())
    }

    // ─── Cross-process locking ─────────────────────────────────────

    /// Acquire an exclusive lock on the index.
    /// Uses `flock` on Unix. Falls back to no-lock on failure.
    #[cfg(unix)]
    pub fn lock_index(&self) -> Option<std::fs::File> {
        use std::os::unix::io::AsRawFd;
        let lock_path = self.root.join(format!("{}{}", INDEX_FILE, LOCK_SUFFIX));
        let _ = fs::create_dir_all(lock_path.parent()?);
        let file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .ok()?;
        let fd = file.as_raw_fd();
        let res = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if res == 0 {
            Some(file)
        } else {
            None
        }
    }

    #[cfg(not(unix))]
    pub fn lock_index(&self) -> Option<std::fs::File> {
        let lock_path = self.root.join(format!("{}{}", INDEX_FILE, LOCK_SUFFIX));
        let _ = fs::create_dir_all(lock_path.parent()?);
        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(&lock_path)
            .ok()
    }
}

// ─── Atomic write helper ────────────────────────────────────────────────

/// Write `data` to `path` atomically: writes to `path.tmp` first, then renames.
/// If the process crashes mid-write, the original file is intact.
fn atomic_write(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = PathBuf::from(path.to_string_lossy().to_string() + ".tmp");
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

// Unix flock(2) — called via libc::flock in lock_index()
#[cfg(unix)]
#[allow(dead_code)]
#[link(name = "c")]
extern "C" {
    fn flock(fd: libc::c_int, operation: libc::c_int) -> libc::c_int;
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_store() -> (BlobStore, TempDir) {
        let tmp = TempDir::new().unwrap();
        let store = BlobStore::new(tmp.path().to_path_buf()).unwrap();
        (store, tmp)
    }

    #[test]
    fn test_put_get() {
        let (store, _tmp) = temp_store();
        let data = b"hello world";
        let hash = store.put(data);
        assert_eq!(hash.len(), 64);
        assert_eq!(store.get(&hash), Some(data.to_vec()));
    }

    #[test]
    fn test_deduplication() {
        let (store, _tmp) = temp_store();
        let h1 = store.put(b"foo");
        let h2 = store.put(b"foo");
        assert_eq!(h1, h2);
        let h3 = store.put(b"bar");
        assert_ne!(h3, h1);
    }

    #[test]
    fn test_short_hash() {
        let (store, _tmp) = temp_store();
        let hash = store.put(b"test data");
        assert!(store.has(&hash[..8]));
        assert!(store.has(&hash[..4]));
    }

    #[test]
    fn test_refs() {
        let (store, _tmp) = temp_store();
        store.update_ref("main", "abc123").unwrap();
        assert_eq!(store.read_ref("main"), Some("abc123".to_string()));
        store.delete_ref("main").unwrap();
        assert_eq!(store.read_ref("main"), None);
    }

    #[test]
    fn test_sha256() {
        assert_eq!(
            BlobStore::sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            BlobStore::sha256(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}
