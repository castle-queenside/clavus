//! Look at what Clavus sees when it reads a set.
//!
//! With one file it prints everything the parser found. With many it
//! prints one line per file, so a whole library can be checked at once:
//!
//! ```text
//! cargo run -p clavus-parser-als --example probe -- ~/Music/*.als
//! ```
//!
//! This is the tool that found out that every real Ableton set was
//! being read as one track at 120 BPM with no devices and no names.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 1 {
        // One file: print everything.
        let path = &args[0];
        match clavus_parser_als::AlsParser::new(std::path::Path::new(path)) {
            Ok(p) => {
                println!("bpm: {}", p.project.bpm);
                println!("tracks: {}", p.project.tracks.len());
                for t in p.project.tracks.iter().take(25) {
                    println!(
                        "  [{:>8}] {} {:?}",
                        t.track_type,
                        t.name,
                        t.devices
                            .iter()
                            .map(|d| d.name.as_str())
                            .collect::<Vec<_>>()
                    );
                }
            }
            Err(e) => println!("error: {}", e),
        }
        return;
    }

    // Many files: one compact line each, so a whole library can be
    // checked in one run.
    let mut bad = 0usize;
    let mut unnamed = 0usize;
    let mut no_devices = 0usize;
    let mut total_tracks = 0usize;
    let mut total_devices = 0usize;
    let mut bpms: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for path in &args {
        match clavus_parser_als::AlsParser::new(std::path::Path::new(path)) {
            Ok(p) => {
                let pr = &p.project;
                let version = pr.ableton_version.clone().unwrap_or_else(|| "?".into());
                let names = pr.tracks.iter().filter(|t| t.name == "Unnamed").count();
                unnamed += names;
                let devices: usize = pr.tracks.iter().map(|t| t.devices.len()).sum();
                total_tracks += pr.tracks.len();
                total_devices += devices;
                let devs = pr.tracks.iter().filter(|t| t.devices.is_empty()).count();
                no_devices += devs;
                *bpms.entry(format!("{:.1}", pr.bpm)).or_default() += 1;
                if pr.tracks.is_empty() {
                    bad += 1;
                    println!("EMPTY  Live {version:>3}  {path}");
                } else {
                    println!(
                        "ok     Live {version:>3}  bpm {:>6.1}  tracks {:>3}  devices {:>4}  unnamed {names:>2}  {}",
                        pr.bpm,
                        pr.tracks.len(),
                        devices,
                        path.rsplit('/').next().unwrap_or(path)
                    );
                }
            }
            Err(e) => {
                bad += 1;
                println!("ERROR  {path}: {e}");
            }
        }
    }
    eprintln!(
        "\n{args:?} => {} files, {bad} bad, {total_tracks} tracks, {total_devices} devices, {unnamed} unnamed tracks, {no_devices} tracks with no devices",
        args.len()
    );
    let mut sorted: Vec<_> = bpms.into_iter().collect();
    sorted.sort_by_key(|a| std::cmp::Reverse(a.1));
    eprintln!("tempo spread: {:?}", &sorted[..sorted.len().min(12)]);
}
