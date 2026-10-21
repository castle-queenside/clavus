//! Dump a parsed .als project as JSON, for differential testing against
//! the Python reference implementation.
//!
//! Usage: dump_json <path-to.als>

use serde_json::json;

fn main() {
    let path = std::env::args().nth(1).expect("usage: dump_json <file.als>");
    let parser = clavus_parser_als::AlsParser::new(std::path::Path::new(&path))
        .expect("parse failed");
    let p = &parser.project;

    let tracks: Vec<_> = p
        .tracks
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "type": t.track_type,
                "muted": t.is_muted,
                "solo": t.is_solo,
                "frozen": t.is_frozen,
                "devices": t.devices.iter().map(|d| d.name.clone()).collect::<Vec<_>>(),
                "sends": t.sends.keys().collect::<Vec<_>>(),
            })
        })
        .collect();

    let returns: Vec<_> = p
        .return_tracks
        .iter()
        .map(|t| json!({ "name": t.name, "type": t.track_type }))
        .collect();

    let markers: Vec<_> = p
        .markers
        .iter()
        .map(|m| json!({ "time": m.time, "name": m.name }))
        .collect();

    let out = json!({
        "track_count": p.track_count,
        "bpm": p.bpm,
        "has_master": p.master_track.is_some(),
        "tracks": tracks,
        "return_tracks": returns,
        "markers": markers,
    });

    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}
