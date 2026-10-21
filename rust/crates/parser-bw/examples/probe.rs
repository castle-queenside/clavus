fn main() {
    let path = std::env::args().nth(1).unwrap();
    let p = clavus_parser_bw::BitwigParser::new(std::path::Path::new(&path)).unwrap();
    let pr = &p.project;
    println!("tracks: {}  markers: {}  bpm: {}  version: {:?}",
        pr.track_count, pr.markers.len(), pr.bpm, pr.schema_version);
    for t in pr.tracks.iter() {
        println!("  track '{}' type={} muted={} solo={} devices={:?}",
            t.name, t.track_type, t.is_muted, t.is_solo,
            t.devices.iter().map(|d| d.name.clone()).collect::<Vec<_>>());
    }
    for m in pr.markers.iter() {
        println!("  marker {} @ {}", m.name, m.time);
    }
}
