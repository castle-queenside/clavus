//! Helpers shared by the CLI's regression tests.
//!
//! They drive the real command implementations with real files; nothing
//! is re-implemented on the test side.

#![allow(dead_code)]

use std::fs;
use std::path::Path;

/// What a command wrote.
pub struct Output {
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    /// Both streams together.
    pub fn all(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

/// Run a command with captured streams.
pub fn run<F>(f: F) -> (Output, anyhow::Result<()>)
where
    F: FnOnce(&mut dyn std::io::Write, &mut dyn std::io::Write) -> anyhow::Result<()>,
{
    let mut out = Vec::new();
    let mut err = Vec::new();
    let result = f(&mut out, &mut err);
    (
        Output {
            stdout: String::from_utf8_lossy(&out).to_string(),
            stderr: String::from_utf8_lossy(&err).to_string(),
        },
        result,
    )
}

/// A minimal Live set with one track.
pub fn als_version(name: &str, bpm: u32) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<LiveSet>
  <Tempo Value="{}" />
  <MasterTrack><Name Value="Master" /><DeviceChain /></MasterTrack>
  <AudioTrack><Name Value="{}" /><DeviceChain /></AudioTrack>
</LiveSet>"#,
        bpm, name
    )
}

/// Write a gzipped `.als`.
pub fn write_als(path: &Path, xml: &str) {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    let file = fs::File::create(path).expect("create als");
    let mut enc = GzEncoder::new(file, Compression::default());
    use std::io::Write;
    enc.write_all(xml.as_bytes()).expect("write als");
    enc.finish().expect("finish als");
}
