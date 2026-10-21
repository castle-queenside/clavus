//! Hostile-server tests for the sync client.
//!
//! A real relay is well-behaved. These tests point the client at servers
//! that send garbage, truncate messages, hang up mid-frame, or claim absurd
//! payload sizes. The client must fail cleanly — never panic, never hang
//! forever, never read unbounded memory.

use clavus_sync::client::{SyncClient, SyncMsg};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

/// Start a server that accepts one connection and runs `handler`.
fn hostile_server<F>(handler: F) -> String
where
    F: FnOnce(std::net::TcpStream) + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().unwrap().to_string();
    thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            handler(stream);
        }
    });
    addr
}

/// Wait for a message with a bounded timeout.
fn recv_within(client: &SyncClient, timeout: Duration) -> Option<SyncMsg> {
    client.recv_timeout(timeout)
}

// ─── Malformed payloads ────────────────────────────────────────────────────

#[test]
fn server_sends_invalid_json() {
    let addr = hostile_server(|mut stream| {
        let _ = stream.write_all(&[0, 0, 0, 5]);
        let _ = stream.write_all(b"{{{{{");
        let _ = stream.flush();
        // Keep the connection open briefly so the client reads the frame.
        thread::sleep(Duration::from_millis(200));
    });

    let client = SyncClient::connect(&addr).expect("connect");
    // The client must not panic. The receiver thread drops the bad frame;
    // the client simply never receives a message.
    let _ = recv_within(&client, Duration::from_millis(500));
    client.close();
}

#[test]
fn server_sends_json_that_is_not_a_sync_message() {
    let addr = hostile_server(|mut stream| {
        let payload = br#"{"not":"a sync message"}"#;
        let _ = stream.write_all(&(payload.len() as u32).to_be_bytes());
        let _ = stream.write_all(payload);
        let _ = stream.flush();
        thread::sleep(Duration::from_millis(200));
    });

    let client = SyncClient::connect(&addr).expect("connect");
    let _ = recv_within(&client, Duration::from_millis(500));
    client.close();
}

#[test]
fn server_claims_absurd_payload_size() {
    let addr = hostile_server(|mut stream| {
        // 4 GB length prefix — must be rejected before allocating.
        let _ = stream.write_all(&u32::MAX.to_be_bytes());
        let _ = stream.flush();
        thread::sleep(Duration::from_millis(200));
    });

    let client = SyncClient::connect(&addr).expect("connect");
    let _ = recv_within(&client, Duration::from_millis(500));
    client.close();
}

#[test]
fn server_claims_payload_just_over_the_limit() {
    let addr = hostile_server(|mut stream| {
        // 10 MB + 1 — one byte over MAX_PAYLOAD.
        let len = 10 * 1024 * 1024 + 1u32;
        let _ = stream.write_all(&len.to_be_bytes());
        let _ = stream.flush();
        thread::sleep(Duration::from_millis(200));
    });

    let client = SyncClient::connect(&addr).expect("connect");
    let _ = recv_within(&client, Duration::from_millis(500));
    client.close();
}

#[test]
fn server_hangs_up_immediately() {
    let addr = hostile_server(|stream| {
        drop(stream);
    });

    let client = SyncClient::connect(&addr).expect("connect");
    let _ = recv_within(&client, Duration::from_millis(500));
    client.close();
}

#[test]
fn server_hangs_up_mid_frame() {
    let addr = hostile_server(|mut stream| {
        // Length prefix promising 100 bytes, then hang up.
        let _ = stream.write_all(&100u32.to_be_bytes());
        let _ = stream.write_all(b"partial");
        let _ = stream.flush();
        drop(stream);
    });

    let client = SyncClient::connect(&addr).expect("connect");
    let _ = recv_within(&client, Duration::from_millis(500));
    client.close();
}

#[test]
fn server_sends_only_a_partial_length_prefix() {
    let addr = hostile_server(|mut stream| {
        let _ = stream.write_all(&[0, 0]);
        let _ = stream.flush();
        drop(stream);
    });

    let client = SyncClient::connect(&addr).expect("connect");
    let _ = recv_within(&client, Duration::from_millis(500));
    client.close();
}

#[test]
fn server_sends_random_bytes() {
    let addr = hostile_server(|mut stream| {
        // A plausible length followed by binary noise.
        let _ = stream.write_all(&64u32.to_be_bytes());
        let noise: Vec<u8> = (0..64u8).map(|i| i.wrapping_mul(37)).collect();
        let _ = stream.write_all(&noise);
        let _ = stream.flush();
        thread::sleep(Duration::from_millis(200));
    });

    let client = SyncClient::connect(&addr).expect("connect");
    let _ = recv_within(&client, Duration::from_millis(500));
    client.close();
}

// ─── Connection behaviour ──────────────────────────────────────────────────

#[test]
fn connect_to_closed_port_fails_cleanly() {
    // Bind and immediately drop the listener to get a port nobody is on.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    drop(listener);

    let result = SyncClient::connect(&addr);
    assert!(result.is_err(), "connecting to a dead port should fail");
}

#[test]
fn connect_to_unresolvable_host_fails_cleanly() {
    let result = SyncClient::connect("this-host-does-not-exist.invalid:4477");
    assert!(result.is_err(), "unresolvable host should fail");
}

#[test]
fn recv_timeout_returns_none_when_idle() {
    let addr = hostile_server(|stream| {
        // Hold the connection open without sending anything.
        thread::sleep(Duration::from_millis(600));
        drop(stream);
    });

    let client = SyncClient::connect(&addr).expect("connect");
    let start = std::time::Instant::now();
    let msg = client.recv_timeout(Duration::from_millis(150));
    let elapsed = start.elapsed();

    assert!(msg.is_none(), "should time out with no message");
    assert!(
        elapsed >= Duration::from_millis(100),
        "should actually wait for the timeout, waited {:?}",
        elapsed
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "should not wait far past the timeout, waited {:?}",
        elapsed
    );
}

#[test]
fn send_after_close_does_not_panic() {
    let addr = hostile_server(|stream| {
        thread::sleep(Duration::from_millis(300));
        drop(stream);
    });

    let client = SyncClient::connect(&addr).expect("connect");
    client.close();
    // Sending on a closed connection must return an error, not panic.
    let result = client.send(SyncMsg::Synced);
    assert!(result.is_err(), "send after close should error");
}

#[test]
fn drop_is_safe_and_marks_closed() {
    let addr = hostile_server(|stream| {
        thread::sleep(Duration::from_millis(500));
        drop(stream);
    });

    let client = SyncClient::connect(&addr).expect("connect");
    assert!(!client.is_closed());
    client.close();
    assert!(client.is_closed());
}

#[test]
fn many_messages_are_delivered_in_order() {
    let addr = hostile_server(|mut stream| {
        for i in 0..25u32 {
            let payload = format!(r#"{{"type":"event","project":"p{}","event":{{"kind":"new_snapshot","project":"p{}","hash":"h{}"}}}}"#, i, i, i);
            let _ = stream.write_all(&(payload.len() as u32).to_be_bytes());
            let _ = stream.write_all(payload.as_bytes());
        }
        let _ = stream.flush();
        thread::sleep(Duration::from_millis(400));
    });

    let client = SyncClient::connect(&addr).expect("connect");

    let mut received = 0;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while received < 25 && std::time::Instant::now() < deadline {
        if client.recv_timeout(Duration::from_millis(200)).is_some() {
            received += 1;
        }
    }
    assert_eq!(received, 25, "all frames should be delivered");
    client.close();
}

#[test]
fn concurrent_clients_each_get_their_own_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    // Server echoes each client's name back inside a SyncMsg.
    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(s) => s,
                Err(_) => break,
            };
            thread::spawn(move || {
                let mut len_buf = [0u8; 4];
                if stream.read_exact(&mut len_buf).is_err() {
                    return;
                }
                let n = u32::from_be_bytes(len_buf) as usize;
                let mut data = vec![0u8; n];
                if stream.read_exact(&mut data).is_err() {
                    return;
                }
                if let Ok(msg) = serde_json::from_slice::<SyncMsg>(&data) {
                    if let SyncMsg::Hello { client_name, .. } = msg {
                        let reply = format!(
                            r#"{{"type":"event","project":"{}","event":{{"kind":"new_snapshot","project":"{}","hash":"h"}}}}"#,
                            client_name, client_name
                        );
                        let _ = stream.write_all(&(reply.len() as u32).to_be_bytes());
                        let _ = stream.write_all(reply.as_bytes());
                        let _ = stream.flush();
                    }
                }
            });
        }
    });

    let mut handles = Vec::new();
    for i in 0..8 {
        let addr = addr.clone();
        handles.push(thread::spawn(move || {
            let client = SyncClient::connect(&addr).expect("connect");
            client
                .hello(&format!("client-{}", i), vec![])
                .expect("hello");
            let msg = client.recv_timeout(Duration::from_secs(3));
            client.close();
            msg
        }));
    }

    for (i, h) in handles.into_iter().enumerate() {
        let msg = h.join().expect("thread");
        match msg {
            Some(SyncMsg::Event { project, .. }) => {
                assert_eq!(project, format!("client-{}", i));
            }
            other => panic!("unexpected message: {:?}", other),
        }
    }
}
