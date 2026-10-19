//! Clavus sync client.
//!
//! Maintains a local relay connection, handles push/pull of snapshots and cues.

pub mod client;

pub use client::{SyncClient, SyncMsg, SyncEvent};
