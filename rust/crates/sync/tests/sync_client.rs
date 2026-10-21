//! Tests for clavus-sync SyncClient.

use clavus_sync::SyncMsg;

#[test]
fn test_conflict_msg_deserializes() {
    let json = r#"{"type":"conflict","project":"MyProject","relay_head":"abc123","push_head":"def456"}"#;
    let msg: SyncMsg = serde_json::from_str(json).unwrap();
    match msg {
        SyncMsg::Conflict {
            project,
            relay_head,
            push_head,
        } => {
            assert_eq!(project, "MyProject");
            assert_eq!(relay_head, "abc123");
            assert_eq!(push_head, "def456");
        }
        other => panic!("expected Conflict, got {:?}", other),
    }
}

#[test]
fn test_synced_msg_deserializes() {
    let json = r#"{"type":"synced"}"#;
    let msg: SyncMsg = serde_json::from_str(json).unwrap();
    assert!(matches!(msg, SyncMsg::Synced));
}

#[test]
fn test_push_msg_deserializes() {
    let json = r#"{"type":"push","snapshots":[],"cues":[]}"#;
    let msg: SyncMsg = serde_json::from_str(json).unwrap();
    match msg {
        SyncMsg::Push { snapshots, cues } => {
            assert!(snapshots.is_empty());
            assert!(cues.is_empty());
        }
        other => panic!("expected Push, got {:?}", other),
    }
}

#[test]
fn test_all_syncmsg_variants_roundtrip_json() {
    // Synced
    let json = serde_json::to_string(&SyncMsg::Synced).unwrap();
    let parsed: SyncMsg = serde_json::from_str(&json).unwrap();
    assert!(matches!(parsed, SyncMsg::Synced));

    // Conflict
    let conflict = SyncMsg::Conflict {
        project: "TestProj".to_string(),
        relay_head: "head1".to_string(),
        push_head: "head2".to_string(),
    };
    let json = serde_json::to_string(&conflict).unwrap();
    let parsed: SyncMsg = serde_json::from_str(&json).unwrap();
    assert!(matches!(parsed, SyncMsg::Conflict { .. }));

    // Push (empty)
    let push = SyncMsg::Push {
        snapshots: vec![],
        cues: vec![],
    };
    let json = serde_json::to_string(&push).unwrap();
    let parsed: SyncMsg = serde_json::from_str(&json).unwrap();
    assert!(matches!(parsed, SyncMsg::Push { .. }));
}

#[test]
fn test_hello_serialization() {
    let json = r#"{"type":"hello","client_name":"test","known_heads":[]}"#;
    let msg: SyncMsg = serde_json::from_str(json).unwrap();
    match msg {
        SyncMsg::Hello { client_name, known_heads } => {
            assert_eq!(client_name, "test");
            assert!(known_heads.is_empty());
        }
        other => panic!("expected Hello, got {:?}", other),
    }
}

#[test]
fn test_hello_with_heads_serialization() {
    let json = r#"{"type":"hello","client_name":"cli","known_heads":["h1","h2"]}"#;
    let msg: SyncMsg = serde_json::from_str(json).unwrap();
    match msg {
        SyncMsg::Hello { client_name, known_heads } => {
            assert_eq!(client_name, "cli");
            assert_eq!(known_heads, vec!["h1", "h2"]);
        }
        other => panic!("expected Hello, got {:?}", other),
    }
}
