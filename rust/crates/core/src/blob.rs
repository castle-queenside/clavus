//! Content-addressed blob storage.
//!
//! Blobs are stored at `~/.clavus/objects/{2-char-prefix}/{hash}`.
//! SHA256 of the content is used as the address. Same content always
//! maps to the same address (deduplication by construction).

use crate::types::{ClavusError, Snapshot};
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
const PROJECTS_DIR: &str = "projects";
const SNAPSHOTS_DIR: &str = "snapshots";
const INDEX_FILE: &str = "index.json";
const LOCK_SUFFIX: &str = ".lock";

/// Content-addressed storage engine.
#[derive(Debug, Clone)]
pub struct BlobStore {
    pub root: PathBuf,
    objects_dir: PathBuf,
    refs_dir: PathBuf,
    index_path: PathBuf,
}

/// Build the error returned when a hash cannot be used as a file name.
fn invalid_hash(hash: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("invalid blob hash: {:?}", hash),
    )
}

impl BlobStore {
    // ─── Identifier validation ──────────────────────────────────────

    /// Check that `hash` is a plain lowercase hex blob hash or hash prefix.
    ///
    /// Hashes arrive from remote peers over the sync protocol and are used
    /// to build file paths, so a crafted hash must not be able to escape
    /// the object store.
    pub fn is_valid_hash(hash: &str) -> bool {
        (2..=64).contains(&hash.len())
            && hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }

    /// Check that `name` is usable as a reference file name.
    ///
    /// Ref names become file names under the refs directory, and a
    /// snapshot's parent is used as a ref name — that comes from a remote
    /// peer, so it must not be able to escape the refs directory.
    ///
    /// A `/` separates path segments (a project's HEAD lives at
    /// `<project>/HEAD`), so each segment is checked individually.
    pub fn is_valid_ref_name(name: &str) -> bool {
        if name.is_empty() || name.len() > 128 || name.contains('\\') {
            return false;
        }
        name.split('/').all(|segment| {
            !segment.is_empty() && segment != "." && segment != ".."
        })
    }

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

    /// Open a store for reading, without creating anything.
    ///
    /// A command that only reads must not leave a directory tree behind.
    /// `new` creates the store's directories; this does not.
    pub fn open(root: PathBuf) -> Self {
        Self {
            objects_dir: root.join(OBJECTS_DIR),
            refs_dir: root.join(REFS_DIR),
            index_path: root.join(INDEX_FILE),
            root,
        }
    }

    /// Whether the store's directories can be read.
    ///
    /// A store that cannot be read is not an empty store. The history
    /// of a project is in there, and a caller that says "no snapshots"
    /// when the directory is unreadable tells the user a lie.
    pub fn check_readable(&self) -> io::Result<()> {
        match fs::read_dir(&self.root) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // The store was never written to. That is empty, not
                // broken — but the *parent* has to be readable, or the
                // store would look empty when it is merely hidden.
                if let Some(parent) = self.root.parent() {
                    if let Err(e) = fs::read_dir(parent) {
                        if e.kind() != io::ErrorKind::NotFound {
                            return Err(e);
                        }
                    }
                }
            }
            Err(e) => return Err(e),
        }
        if self.refs_dir.exists() {
            fs::read_dir(&self.refs_dir)?;
        }
        Ok(())
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
        let Some(path) = self.blob_path(&hash) else {
            return hash;
        };
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

    /// The hash of a project file's bytes.
    ///
    /// It is the same hash a snapshot of that file carries, so it can
    /// be used to ask a relay which project a file belongs to — even
    /// when the file sits in a different folder on another machine.
    pub fn content_hash_of_file(path: &std::path::Path) -> Option<String> {
        let data = std::fs::read(path).ok()?;
        Some(Self::sha256(&data))
    }

    // ─── Path helpers ────────────────────────────────────────────────

    fn blob_path(&self, hash: &str) -> Option<PathBuf> {
        if hash.len() != 64 || !Self::is_valid_hash(hash) {
            return None;
        }
        Some(self.objects_dir.join(&hash[..2]).join(hash))
    }

    /// Path to the metadata file for a snapshot.
    pub fn meta_path(&self, hash: &str) -> Option<PathBuf> {
        if hash.len() != 64 || !Self::is_valid_hash(hash) {
            return None;
        }
        Some(self.objects_dir.join(&hash[..2]).join(format!("{}.meta", hash)))
    }

    /// Path to the sample metadata file.
    pub fn sample_meta_path(&self, hash: &str) -> Option<PathBuf> {
        if hash.len() != 64 || !Self::is_valid_hash(hash) {
            return None;
        }
        Some(self.objects_dir.join(&hash[..2]).join(format!("{}.sample", hash)))
    }

    /// Resolve a short (8–63 char) hash to the full 64-char path.
    /// Returns `None` if no matching blob exists.
    fn resolve_path(&self, hash: &str) -> Option<PathBuf> {
        if !Self::is_valid_hash(hash) {
            return None;
        }
        if hash.len() == 64 {
            return self.blob_path(hash);
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

    /// The directory holding one project's snapshot records.
    ///
    /// Snapshot records are per project, not per content. Two projects
    /// that contain the same bytes are two different projects with two
    /// different histories, and sharing one record lost the second
    /// project's message, timestamp and parent link.
    pub fn project_meta_dir(&self, project_key: &str) -> Option<PathBuf> {
        if project_key.is_empty() || project_key.contains('/') || project_key.contains('\\') {
            return None;
        }
        Some(
            self.root
                .join(PROJECTS_DIR)
                .join(project_key)
                .join(SNAPSHOTS_DIR),
        )
    }

    /// Write snapshot metadata for one project atomically (rename over .tmp).
    pub fn write_meta(&self, project_key: &str, snapshot: &Snapshot) -> io::Result<()> {
        // The hash becomes a file name, and a hash can come from a remote
        // peer — so it is checked before it is used as a path.
        if !Self::is_valid_hash(&snapshot.hash) || snapshot.hash.len() != 64 {
            return Err(invalid_hash(&snapshot.hash));
        }
        let dir = self
            .project_meta_dir(project_key)
            .ok_or_else(|| invalid_hash(&snapshot.hash))?;
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.json", snapshot.hash));
        let tmp = PathBuf::from(path.to_string_lossy().to_string() + ".tmp");
        let json = serde_json::to_string_pretty(snapshot)?;
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &path)?;
        debug!("Wrote snapshot meta: {}", snapshot.short_hash(8));
        Ok(())
    }

    /// Load one project's snapshot metadata. Returns `None` if not found or corrupt.
    pub fn load_meta(&self, project_key: &str, hash: &str) -> Option<Snapshot> {
        let dir = self.project_meta_dir(project_key)?;
        if !Self::is_valid_hash(hash) || hash.len() != 64 {
            return None;
        }
        let path = dir.join(format!("{}.json", hash));
        if !path.exists() {
            return None;
        }
        let data = fs::read_to_string(&path).ok()?;
        serde_json::from_str(&data).ok()
    }

    /// List the hashes of every snapshot record one project has.
    ///
    /// Used to resolve a short hash prefix to a full snapshot hash.
    pub fn list_meta_hashes(&self, project_key: &str) -> Vec<String> {
        let mut out = Vec::new();
        let Some(dir) = self.project_meta_dir(project_key) else {
            return out;
        };
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if let Some(stripped) = name.strip_suffix(".json") {
                    if stripped.len() == 64 {
                        out.push(stripped.to_string());
                    }
                }
            }
        }
        out.sort();
        out
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
        let path = self
            .blob_path(&hash)
            .ok_or_else(|| invalid_hash(&hash))?;

        let filename = file_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        // Write metadata FIRST — prevents orphan blob if crash occurs here
        let meta_path = self
            .sample_meta_path(&hash)
            .ok_or_else(|| invalid_hash(&hash))?;
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

    /// Store the bytes of a sample that arrived from somewhere else.
    ///
    /// A collaborator's audio comes over the wire, so there is no file
    /// to read: the bytes are stored under their own hash, together
    /// with the path the sample has inside the project.
    pub fn put_sample_bytes(
        &self,
        data: &[u8],
        filename: &str,
        relative_path: &str,
    ) -> io::Result<String> {
        let hash = Self::sha256(data);
        let path = self
            .blob_path(&hash)
            .ok_or_else(|| invalid_hash(&hash))?;
        let meta_path = self
            .sample_meta_path(&hash)
            .ok_or_else(|| invalid_hash(&hash))?;
        if let Some(parent) = meta_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let meta = format!("{}\n{}\n", filename, relative_path.replace('\\', "/"));
        fs::write(&meta_path, meta)?;
        if !path.exists() {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            atomic_write(&path, data)?;
        }
        Ok(hash)
    }

    /// Get the original filename for a stored sample.
    pub fn sample_filename(&self, hash: &str) -> Option<String> {
        let meta = fs::read_to_string(self.sample_meta_path(hash)?).ok()?;
        meta.lines().next().map(|s| s.to_string())
    }

    /// Get the relative path for a stored sample.
    pub fn sample_relpath(&self, hash: &str) -> Option<String> {
        let meta = fs::read_to_string(self.sample_meta_path(hash)?).ok()?;
        let mut lines = meta.lines();
        lines.next(); // skip filename
        lines.next().map(|s| s.replace('\\', "/"))
    }

    // ─── Refs (HEAD, branches, tags) ─────────────────────────────────

    /// The directory the content-addressed blobs live in.
    pub fn objects_dir(&self) -> PathBuf {
        self.objects_dir.clone()
    }

    /// Write a named reference.
    pub fn update_ref(&self, name: &str, hash: &str) -> io::Result<()> {
        if !Self::is_valid_ref_name(name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid ref name: {:?}", name),
            ));
        }
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
        if !Self::is_valid_ref_name(name) {
            return None;
        }
        let path = self.refs_dir.join(name);
        fs::read_to_string(&path).ok().map(|s| s.trim().to_string())
    }

    /// Delete a named reference.
    pub fn delete_ref(&self, name: &str) -> io::Result<()> {
        if !Self::is_valid_ref_name(name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid ref name: {:?}", name),
            ));
        }
        let path = self.refs_dir.join(name);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    // ─── Index (project registry) ───────────────────────────────────

    /// Read the full index.
    ///
    /// A store that was never written has no index file, and that is
    /// an empty index — the result is `Ok` with nothing in it. An
    /// index file that exists but cannot be read or parsed is a
    /// *broken* store, and it is reported as an error: an empty map
    /// would say "this store has no projects", which is a confident
    /// falsehood about the user's data.
    pub fn read_index(&self) -> Result<HashMap<String, serde_json::Value>, ClavusError> {
        // A store that was never written has no index file. That is an
        // empty index, not a broken one — so the file is read directly
        // and a missing file is told apart from an unreadable one.
        let text = match fs::read_to_string(&self.index_path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
            Err(e) => return Err(ClavusError::Io(e)),
        };
        serde_json::from_str(&text).map_err(|e| {
            ClavusError::IndexCorrupted(format!("{}: {}", self.index_path.display(), e))
        })
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
pub fn atomic_write(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = PathBuf::from(path.to_string_lossy().to_string() + ".tmp");
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path)?;
    Ok(())
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
