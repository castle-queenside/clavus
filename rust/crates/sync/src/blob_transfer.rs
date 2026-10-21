//! The blob half of the sync protocol.
//!
//! The first version of the protocol carried only metadata: a client
//! could see that a snapshot existed, but not the project bytes behind
//! it, so a pulled snapshot could not be restored on the other machine.
//! That made the sync half a story.
//!
//! The project bytes travel as their own messages, in chunks, so a
//! large `.bwproject` does not need one enormous frame:
//!
//! ```text
//!   client                                  relay
//!     │  BlobPush { project, hash, index, total, data }  ──▶
//!     │  BlobPush { project, hash, index, total, data }  ──▶
//!     │  BlobEnd  { project, hash }                      ──▶
//!     │                              ◀──  BlobStored { hash }
//!     │
//!     │  BlobRequest { project, hash }                   ──▶
//!     │                              ◀──  BlobChunk { project, hash, index, total, data }
//!     │                              ◀──  BlobDone { project, hash }
//! ```
//!
//! Every blob is named by the SHA-256 of its contents, and the relay
//! checks that the bytes it received really do hash to the name they
//! were sent under. A peer cannot make the relay file a blob under
//! someone else's name.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// The largest amount of project bytes in one chunk.
///
/// The chunk is base64-encoded inside a JSON frame, so the frame is
/// about a third larger than the chunk itself.
pub const CHUNK_SIZE: usize = 3 * 1024 * 1024;

/// A chunk of a project's bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobChunk {
    /// The project the bytes belong to, by its sync name.
    pub project: String,
    /// The SHA-256 of the whole blob.
    pub hash: String,
    /// Which chunk this is, counting from zero.
    pub index: u32,
    /// How many chunks the whole blob has.
    pub total: u32,
    /// The bytes of this chunk.
    pub data: String,
}

impl BlobChunk {
    /// Build a chunk from raw bytes.
    pub fn new(
        project: &str,
        hash: &str,
        index: u32,
        total: u32,
        data: &[u8],
    ) -> Self {
        Self {
            project: project.to_string(),
            hash: hash.to_string(),
            index,
            total,
            data: BASE64.encode(data),
        }
    }

    /// The bytes of this chunk.
    pub fn bytes(&self) -> Result<Vec<u8>, String> {
        BASE64
            .decode(self.data.as_bytes())
            .map_err(|e| format!("the chunk is not valid base64: {}", e))
    }
}

/// Split a blob into chunks.
pub fn chunk_blob(project: &str, hash: &str, data: &[u8]) -> Vec<BlobChunk> {
    if data.is_empty() {
        return vec![BlobChunk::new(project, hash, 0, 1, data)];
    }
    let total = data.len().div_ceil(CHUNK_SIZE) as u32;
    data.chunks(CHUNK_SIZE)
        .enumerate()
        .map(|(i, chunk)| BlobChunk::new(project, hash, i as u32, total, chunk))
        .collect()
}

/// Collects the chunks of a blob as they arrive.
///
/// The chunks may arrive in any order and a chunk may arrive twice, so
/// the collector places each chunk by its index and only hands over the
/// whole blob once every chunk is there.
#[derive(Debug, Default)]
pub struct BlobAssembler {
    hash: String,
    project: String,
    total: u32,
    chunks: std::collections::BTreeMap<u32, Vec<u8>>,
}

impl BlobAssembler {
    /// A new, empty assembler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Take one chunk.
    ///
    /// Returns the whole blob when the last chunk arrives.
    pub fn add(&mut self, chunk: &BlobChunk) -> Result<Option<Vec<u8>>, String> {
        if !self.hash.is_empty() && self.hash != chunk.hash {
            return Err("chunks of two different blobs were mixed together".into());
        }
        if self.project.is_empty() {
            self.project = chunk.project.clone();
        }
        if chunk.total == 0 {
            return Err("a blob must have at least one chunk".into());
        }
        if chunk.index >= chunk.total {
            return Err(format!(
                "chunk {} is outside the blob's {} chunks",
                chunk.index, chunk.total
            ));
        }
        self.hash = chunk.hash.clone();
        self.total = chunk.total;
        self.chunks.insert(chunk.index, chunk.bytes()?);

        if self.chunks.len() as u32 != self.total {
            return Ok(None);
        }

        let mut whole = Vec::new();
        for i in 0..self.total {
            let Some(part) = self.chunks.get(&i) else {
                return Ok(None);
            };
            whole.extend_from_slice(part);
        }
        Ok(Some(whole))
    }

    /// The hash of the blob being assembled.
    pub fn hash(&self) -> &str {
        &self.hash
    }
}

/// Check that the bytes really are the blob they claim to be.
pub fn verify_blob(hash: &str, data: &[u8]) -> Result<(), String> {
    if !clavus_core::blob::BlobStore::is_valid_hash(hash) {
        return Err(format!("'{}' is not a valid hash", hash));
    }
    let digest = clavus_core::blob::BlobStore::sha256(data);
    if digest != hash {
        return Err(format!(
            "the bytes hash to {} but they were sent as {}",
            digest, hash
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_blob_is_one_chunk() {
        let chunks = chunk_blob("Track.als", &"a".repeat(64), b"hello");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].bytes().unwrap(), b"hello");
    }

    #[test]
    fn a_large_blob_is_split_and_put_back_together() {
        let data: Vec<u8> = (0..(CHUNK_SIZE * 2 + 17)).map(|i| (i % 251) as u8).collect();
        let hash = clavus_core::blob::BlobStore::sha256(&data);
        let chunks = chunk_blob("Track.als", &hash, &data);
        assert_eq!(chunks.len(), 3);

        let mut assembler = BlobAssembler::new();
        let mut whole = None;
        for chunk in &chunks {
            if let Some(done) = assembler.add(chunk).unwrap() {
                whole = Some(done);
            }
        }
        assert_eq!(whole.unwrap(), data);
    }

    #[test]
    fn chunks_may_arrive_out_of_order() {
        let data: Vec<u8> = (0..(CHUNK_SIZE + 5)).map(|i| (i % 251) as u8).collect();
        let hash = clavus_core::blob::BlobStore::sha256(&data);
        let mut chunks = chunk_blob("Track.als", &hash, &data);
        chunks.reverse();

        let mut assembler = BlobAssembler::new();
        let mut whole = None;
        for chunk in &chunks {
            if let Some(done) = assembler.add(chunk).unwrap() {
                whole = Some(done);
            }
        }
        assert_eq!(whole.unwrap(), data);
    }

    #[test]
    fn a_chunk_of_another_blob_is_refused() {
        let mut assembler = BlobAssembler::new();
        let a = BlobChunk::new("Track.als", &"a".repeat(64), 0, 1, b"hello");
        let b = BlobChunk::new("Track.als", &"b".repeat(64), 0, 1, b"hello");
        assembler.add(&a).unwrap();
        assert!(assembler.add(&b).is_err());
    }

    #[test]
    fn a_chunk_outside_the_blob_is_refused() {
        let mut assembler = BlobAssembler::new();
        let c = BlobChunk::new("Track.als", &"a".repeat(64), 5, 2, b"hello");
        assert!(assembler.add(&c).is_err());
    }

    #[test]
    fn bytes_that_do_not_match_their_name_are_refused() {
        let hash = clavus_core::blob::BlobStore::sha256(b"hello");
        assert!(verify_blob(&hash, b"hello").is_ok());
        assert!(verify_blob(&hash, b"goodbye").is_err());
        assert!(verify_blob("not-a-hash", b"hello").is_err());
    }
}
