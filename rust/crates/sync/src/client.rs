//! Sync protocol types shared between client and relay.

use clavus_core::types::{Cue, Snapshot};
use serde::{Deserialize, Serialize};

/// Messages in the Clavus sync protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SyncMsg {
    /// Client announces itself and its known head snapshots.
    Hello {
        client_name: String,
        known_heads: Vec<String>, // snapshot hashes
    },
    /// Relay sends updates the client doesn't have.
    Push {
        snapshots: Vec<Snapshot>,
        cues: Vec<Cue>,
    },
    /// Relay asks client to send updates for specific heads.
    Pull { missing: Vec<String> },
    /// Client responds to Pull with the requested objects.
    PullResp {
        snapshots: Vec<Snapshot>,
        cues: Vec<Cue>,
    },
    /// Relay confirms sync is complete.
    Synced,
    /// Keep-alive ping.
    Ping,
    /// Keep-alive pong.
    Pong,
    /// Client requests to subscribe to a project's events.
    Subscribe { project: String },
    /// Relay broadcasts an event to subscribers.
    Event { project: String, event: SyncEvent },
}

/// Events broadcast to subscribed clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SyncEvent {
    NewSnapshot { project: String, hash: String },
    NewCue { project: String, cue_id: String },
    CueResolved { project: String, cue_id: String },
}
