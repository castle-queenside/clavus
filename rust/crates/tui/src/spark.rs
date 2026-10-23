//! The shape of a project's history, drawn as a row of small bars.
//!
//! A count of snapshots says how much history a project has. It does
//! not say whether the project is *alive*. A set with twelve
//! snapshots, all taken in one afternoon, has been done with; a set
//! with four, spread over a year, is still being worked on.
//!
//! So each project carries a sparkline: its own snapshots, oldest on
//! the left and newest on the right, bucketed across the project's own
//! lifetime. The row reads left to right like everything else in the
//! list, and a project you are working on right now ends in a bar on
//! the far right.

/// The eight bar heights, shortest first.
pub const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// The shortest span a history is drawn over.
///
/// A producer takes six snapshots in one afternoon. Divided evenly
/// across the project's own lifetime, those six land in six different
/// buckets and the chart becomes noise — a row of bars that says
/// nothing. With a day as the floor, work done in one sitting reads as
/// one burst, and work spread over months reads as a spread. The
/// difference between the two is the thing worth seeing.
pub const MIN_SPAN: f64 = 86_400.0;

/// The shape of a history: one height per cell, `0` meaning empty.
///
/// The timestamps are the moments the snapshots were taken. The
/// history is divided into `cells` spans of time, from the oldest
/// snapshot to the newest, and each snapshot lands in the span it
/// belongs to. The busiest span is a full bar, so the shape is
/// relative to the project's own busiest moment — a project with
/// three snapshots in one afternoon and a project with three hundred
/// across six years both read clearly.
pub fn sparkline(timestamps: &[f64], cells: usize) -> Vec<u8> {
    if cells == 0 {
        return Vec::new();
    }
    if timestamps.is_empty() {
        return vec![0; cells];
    }

    let mut sorted: Vec<f64> = timestamps
        .iter()
        .copied()
        .filter(|t| t.is_finite())
        .collect();
    if sorted.is_empty() {
        return vec![0; cells];
    }
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let first = sorted[0];
    let last = sorted[sorted.len() - 1];
    let span = (last - first).max(MIN_SPAN);

    let mut buckets = vec![0u32; cells];
    // The axis runs from the start of the span to the newest snapshot,
    // so the newest is always in the last bucket.
    let start = last - span;
    for ts in &sorted {
        let pos = ((ts - start) / span) * (cells as f64 - 1.0);
        let idx = pos.round().max(0.0) as usize;
        buckets[idx.min(cells - 1)] += 1;
    }

    let peak = buckets.iter().copied().max().unwrap_or(0);
    if peak == 0 {
        return vec![0; cells];
    }
    buckets
        .iter()
        .map(|&c| {
            if c == 0 {
                0
            } else {
                // The busiest span is a full bar; the quietest is one
                // step, so a single snapshot still shows as *something*.
                let scaled = ((c as f64 / peak as f64) * 8.0).round() as u8;
                scaled.clamp(1, 8)
            }
        })
        .collect()
}

/// The character for a bar height. Zero is blank.
pub fn bar(height: u8) -> char {
    if height == 0 {
        ' '
    } else {
        BARS[(height as usize - 1).min(BARS.len() - 1)]
    }
}

/// The sparkline as a string.
pub fn render(timestamps: &[f64], cells: usize) -> String {
    sparkline(timestamps, cells)
        .into_iter()
        .map(bar)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_history_is_all_blank() {
        assert_eq!(sparkline(&[], 8), vec![0; 8]);
    }

    #[test]
    fn one_snapshot_is_one_bar() {
        let s = sparkline(&[100.0], 8);
        assert_eq!(s.iter().filter(|&&h| h > 0).count(), 1);
        assert_eq!(s[7], 8, "the only snapshot is the newest, so it is last");
    }

    #[test]
    fn snapshots_at_one_moment_are_one_burst() {
        // Three snapshots in the same instant: there was no time
        // between them, so they are one burst at the newest end.
        let s = sparkline(&[50.0, 50.0, 50.0], 8);
        assert_eq!(s[7], 8, "the burst is a full bar");
        assert!(s[..7].iter().all(|&h| h == 0), "and nothing else: {:?}", s);
    }

    #[test]
    fn work_done_in_one_sitting_is_one_burst_and_not_a_row_of_noise() {
        // Six snapshots a second apart. Divided evenly across six
        // seconds they would land in six buckets and the chart would
        // say nothing. With a day as the floor they are one burst.
        let stamps: Vec<f64> = (0..6).map(|i| 1000.0 + i as f64).collect();
        let s = sparkline(&stamps, 8);
        assert_eq!(s[7], 8, "the whole sitting is one bar");
        assert!(s[..7].iter().all(|&h| h == 0), "and no noise: {:?}", s);
    }

    #[test]
    fn work_spread_over_months_is_spread_out() {
        // Six snapshots, one a month, over half a year.
        let month = 30.0 * 86_400.0;
        let stamps: Vec<f64> = (0..6).map(|i| i as f64 * month).collect();
        let s = sparkline(&stamps, 8);
        assert!(s[0] > 0, "the oldest is on the left: {:?}", s);
        assert!(s[7] > 0, "the newest is on the right: {:?}", s);
        assert!(
            s.iter().filter(|&&h| h > 0).count() >= 4,
            "and the work is spread across the chart: {:?}",
            s
        );
    }

    #[test]
    fn the_newest_snapshot_is_always_on_the_right() {
        // A hundred days apart, so the two are really in different
        // parts of the project's life.
        let day = 86_400.0;
        let s = sparkline(&[0.0, 100.0 * day], 8);
        assert_eq!(s[7], 8, "the newest is on the right");
        assert_eq!(s[0], 8, "the oldest is on the left");
        assert!(s[1..7].iter().all(|&h| h == 0), "nothing in between");
    }
    #[test]
    fn a_busy_span_is_taller_than_a_quiet_one() {
        // Three snapshots at the start of the project's life, four at
        // the end.
        let stamps = vec![0.0, 1.0, 2.0, 100.0, 100.0, 100.0, 100.0];
        let s = sparkline(&stamps, 8);
        assert_eq!(s[7], 8, "the busiest span is a full bar");
        assert!(s[0] < s[7], "the quiet span is shorter: {:?}", s);
    }

    #[test]
    fn zero_cells_is_empty() {
        assert!(sparkline(&[1.0], 0).is_empty());
    }

    #[test]
    fn a_bar_is_the_right_character() {
        assert_eq!(bar(0), ' ');
        assert_eq!(bar(1), '▁');
        assert_eq!(bar(8), '█');
    }
}
