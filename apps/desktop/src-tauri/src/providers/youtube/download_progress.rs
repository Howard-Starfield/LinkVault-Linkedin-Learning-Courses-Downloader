//! Live yt-dlp download progress: parse, aggregate, throttle.
//!
//! yt-dlp is asked to print LinkVault-prefixed records (see [`PROGRESS_TEMPLATE`]
//! and [`FORMATS_PRINT`]); the managed-process port streams them line by line.
//! Only complete prefixed lines are decoded, into bounded numbers, and every
//! other line is ignored. This module is pure: it launches nothing and reads no
//! clock, so callers pass `Instant`s in.
//!
//! Observed with the locked yt-dlp (2026.08.19) against a short public video:
//!
//! ```text
//! LVPROGRESS 1024 NA 1024 269         <- total unknown, estimate == bytes so far
//! LVPROGRESS 7168 NA 9216.0 269       <- estimates are floats and swing wildly
//! LVPROGRESS 232744 232744 NA 269     <- last line of a stream: exact total
//! LVPROGRESS 1024 117526 NA 139       <- next stream restarts at 0 (new format id)
//! LVPROGRESS 440 NA 440 NA            <- subtitle download: format id "NA"
//! LVFORMATS 160, 139                  <- requested formats, once before download
//! ```

use std::time::{Duration, Instant};

/// yt-dlp `--progress-template` for download progress records.
pub const PROGRESS_TEMPLATE: &str = "download:LVPROGRESS %(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.total_bytes_estimate)s %(info.format_id)s";
/// yt-dlp `--print` record listing the formats about to be downloaded.
pub const FORMATS_PRINT: &str = "before_dl:LVFORMATS %(requested_formats.:.format_id|)l";

const PROGRESS_PREFIX: &str = "LVPROGRESS ";
const FORMATS_PREFIX: &str = "LVFORMATS";
const UNAVAILABLE: &str = "NA";
/// Larger values are treated as garbage rather than real byte counts (1 PiB).
const MAX_BYTES: u64 = 1 << 50;
const MAX_FORMAT_ID_CHARS: usize = 64;
/// Streams (video, audio, ...) tracked per item; further ones are ignored.
const MAX_STREAMS: usize = 8;
/// Non-terminal publishes per second the spec allows.
const MIN_PUBLISH_INTERVAL: Duration = Duration::from_millis(250);
/// While a requested stream has not started its size is unknown, so do not
/// claim the item is nearly done.
const UNSTARTED_STREAM_RATIO_CAP: f64 = 0.9;
/// Estimated totals are rough (HLS estimates swing by several times), so never
/// reach completion on an estimate alone.
const ESTIMATE_RATIO_CAP: f64 = 0.95;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamSample {
    pub format_id: String,
    pub downloaded: u64,
    pub total: Option<u64>,
    /// `total` came from yt-dlp's estimate, not an exact size.
    pub total_is_estimate: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgressRecord {
    Stream(StreamSample),
    /// Number of formats yt-dlp will download for the item.
    Formats(usize),
}

fn parse_bytes(token: &str) -> Option<u64> {
    if token == UNAVAILABLE {
        return None;
    }
    let value = match token.parse::<u64>() {
        Ok(value) => value,
        Err(_) => {
            let float = token.parse::<f64>().ok()?;
            if !float.is_finite() || float < 0.0 || float > MAX_BYTES as f64 {
                return None;
            }
            float as u64
        }
    };
    (value <= MAX_BYTES).then_some(value)
}

fn valid_format_id(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= MAX_FORMAT_ID_CHARS
        && value
            .chars()
            .all(|character| character.is_ascii_graphic() || character == ' ')
}

/// Decode one stdout line. Anything that is not a well-formed LinkVault record
/// returns `None`, as do subtitle downloads (format id `NA`).
pub fn parse_record(line: &str) -> Option<ProgressRecord> {
    if let Some(rest) = line.strip_prefix(PROGRESS_PREFIX) {
        let mut fields = rest.splitn(4, ' ');
        let downloaded = parse_bytes(fields.next()?)?;
        let total = parse_bytes(fields.next()?);
        let estimate = parse_bytes(fields.next()?);
        let format_id = fields.next()?.trim();
        if format_id == UNAVAILABLE || !valid_format_id(format_id) {
            return None;
        }
        let (total, total_is_estimate) = match (total, estimate) {
            (Some(total), _) => (Some(total), false),
            (None, Some(estimate)) => (Some(estimate), true),
            (None, None) => (None, false),
        };
        return Some(ProgressRecord::Stream(StreamSample {
            format_id: format_id.to_string(),
            downloaded,
            total,
            total_is_estimate,
        }));
    }
    let rest = line.strip_prefix(FORMATS_PREFIX)?;
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    let count = rest
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .count();
    Some(ProgressRecord::Formats(count.clamp(1, MAX_STREAMS)))
}

#[derive(Clone, Debug)]
struct Stream {
    format_id: String,
    downloaded: u64,
    total: Option<u64>,
    exact: bool,
}

/// What the UI should show for the download so far.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgressReading {
    pub bytes_completed: u64,
    /// `None` while any stream's size, or a requested stream, is unknown.
    pub bytes_total: Option<u64>,
    /// Share of the download done, `0.0..=1.0`, never decreasing.
    pub ratio: f64,
}

/// Per-item aggregate over every stream yt-dlp downloads (typically video then
/// audio, each with its own counter starting at zero), plus the publish
/// throttle.
#[derive(Debug, Default)]
pub struct DownloadProgress {
    streams: Vec<Stream>,
    expected_streams: Option<usize>,
    ratio_floor: f64,
    published: Option<(Instant, ProgressReading)>,
}

impl DownloadProgress {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one stdout line. Returns a reading when it should be published now:
    /// something changed and at most four publishes per second are respected.
    pub fn observe_line(&mut self, line: &str, now: Instant) -> Option<ProgressReading> {
        match parse_record(line)? {
            ProgressRecord::Formats(count) => self.expected_streams = Some(count),
            ProgressRecord::Stream(sample) => self.apply(sample),
        }
        let reading = self.reading()?;
        let due = match &self.published {
            None => true,
            Some((at, last)) => {
                last != &reading && now.saturating_duration_since(*at) >= MIN_PUBLISH_INTERVAL
            }
        };
        if !due {
            return None;
        }
        self.published = Some((now, reading.clone()));
        Some(reading)
    }

    /// The latest reading if it was held back by the throttle. Call once when
    /// the download ends so the final state is not lost.
    pub fn flush(&mut self, now: Instant) -> Option<ProgressReading> {
        let reading = self.reading()?;
        if self
            .published
            .as_ref()
            .is_some_and(|(_, last)| last == &reading)
        {
            return None;
        }
        self.published = Some((now, reading.clone()));
        Some(reading)
    }

    fn apply(&mut self, sample: StreamSample) {
        let index = match self
            .streams
            .iter()
            .position(|stream| stream.format_id == sample.format_id)
        {
            Some(index) => index,
            None if self.streams.len() < MAX_STREAMS => {
                self.streams.push(Stream {
                    format_id: sample.format_id.clone(),
                    downloaded: 0,
                    total: None,
                    exact: false,
                });
                self.streams.len() - 1
            }
            None => return,
        };
        let stream = &mut self.streams[index];
        // A stream restarting from zero (retry) must not move bytes backwards.
        stream.downloaded = stream.downloaded.max(sample.downloaded);
        if !(stream.exact && sample.total_is_estimate) {
            stream.total = sample.total.map(|total| total.max(stream.downloaded));
            stream.exact = sample.total.is_some() && !sample.total_is_estimate;
        }
    }

    fn reading(&mut self) -> Option<ProgressReading> {
        if self.streams.is_empty() {
            return None;
        }
        let bytes_completed = self.streams.iter().map(|stream| stream.downloaded).sum();
        let all_totals_known = self.streams.iter().all(|stream| stream.total.is_some());
        let all_started = self
            .expected_streams
            .map_or(true, |expected| self.streams.len() >= expected);
        let known_total = all_totals_known
            .then(|| {
                self.streams
                    .iter()
                    .map(|stream| stream.total.unwrap_or(0))
                    .sum::<u64>()
            })
            .filter(|total| *total > 0);
        if let Some(total) = known_total {
            let mut ratio = bytes_completed as f64 / total as f64;
            if !all_started {
                ratio = ratio.min(UNSTARTED_STREAM_RATIO_CAP);
            }
            if self.streams.iter().any(|stream| !stream.exact) {
                ratio = ratio.min(ESTIMATE_RATIO_CAP);
            }
            self.ratio_floor = self.ratio_floor.max(ratio.clamp(0.0, 1.0));
        }
        // Without a known total (a stream has just begun) the ratio is held.
        Some(ProgressReading {
            bytes_completed,
            bytes_total: known_total.filter(|_| all_started),
            ratio: self.ratio_floor,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(
        format_id: &str,
        downloaded: u64,
        total: Option<u64>,
        estimate: bool,
    ) -> ProgressRecord {
        ProgressRecord::Stream(StreamSample {
            format_id: format_id.to_string(),
            downloaded,
            total,
            total_is_estimate: estimate,
        })
    }

    #[test]
    fn parses_exact_total_lines() {
        assert_eq!(
            parse_record("LVPROGRESS 1024 195278 NA 160"),
            Some(stream("160", 1024, Some(195278), false))
        );
    }

    #[test]
    fn falls_back_to_the_estimate_when_the_total_is_na() {
        assert_eq!(
            parse_record("LVPROGRESS 7168 NA 9216.0 269"),
            Some(stream("269", 7168, Some(9216), true))
        );
        assert_eq!(
            parse_record("LVPROGRESS 1024 NA 1024 269"),
            Some(stream("269", 1024, Some(1024), true))
        );
    }

    #[test]
    fn unknown_total_and_estimate_yield_no_total() {
        assert_eq!(
            parse_record("LVPROGRESS 500 NA NA 139-drc"),
            Some(stream("139-drc", 500, None, false))
        );
    }

    #[test]
    fn exact_total_wins_over_a_present_estimate() {
        assert_eq!(
            parse_record("LVPROGRESS 10 100 5000.0 18"),
            Some(stream("18", 10, Some(100), false))
        );
    }

    #[test]
    fn ignores_subtitle_records_and_unprefixed_output() {
        assert_eq!(parse_record("LVPROGRESS 440 NA 440 NA"), None);
        for line in [
            "",
            "[download] Destination: C:\\x\\media.f160.mp4",
            "[download]  42.0% of 43.60MiB at 1.2MiB/s ETA 00:12",
            " LVPROGRESS 1 2 3 4",
            "LVPROGRESS",
            "LVPROGRESS ",
            "LVPROGRESSX 1 2 3 4",
        ] {
            assert_eq!(parse_record(line), None, "{line:?}");
        }
    }

    #[test]
    fn rejects_malformed_numbers_and_ids() {
        for line in [
            "LVPROGRESS NA 100 NA 160",
            "LVPROGRESS -5 100 NA 160",
            "LVPROGRESS abc 100 NA 160",
            "LVPROGRESS 1 2 NA",
            "LVPROGRESS 1 2",
            "LVPROGRESS inf NA NA 160",
            "LVPROGRESS 1e300 NA NA 160",
            "LVPROGRESS 99999999999999999999 NA NA 160",
            "LVPROGRESS 1 NA NA bad\u{1b}id",
        ] {
            assert_eq!(parse_record(line), None, "{line:?}");
        }
        let long_id = "x".repeat(MAX_FORMAT_ID_CHARS + 1);
        assert_eq!(parse_record(&format!("LVPROGRESS 1 2 NA {long_id}")), None);
    }

    #[test]
    fn parses_the_requested_format_count() {
        assert_eq!(
            parse_record("LVFORMATS 160, 139"),
            Some(ProgressRecord::Formats(2))
        );
        assert_eq!(
            parse_record("LVFORMATS 18"),
            Some(ProgressRecord::Formats(1))
        );
        assert_eq!(parse_record("LVFORMATS "), Some(ProgressRecord::Formats(1)));
        assert_eq!(parse_record("LVFORMATS"), Some(ProgressRecord::Formats(1)));
        assert_eq!(parse_record("LVFORMATSX 1"), None);
        let many = (0..40)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        assert_eq!(
            parse_record(&format!("LVFORMATS {many}")),
            Some(ProgressRecord::Formats(MAX_STREAMS))
        );
    }

    struct Clock(Instant);

    impl Clock {
        fn at(&self, millis: u64) -> Instant {
            self.0 + Duration::from_millis(millis)
        }
    }

    fn clock() -> Clock {
        Clock(Instant::now())
    }

    /// Feed lines 1 s apart so the throttle never interferes, and collect the
    /// ratio of every published reading.
    fn ratios(tracker: &mut DownloadProgress, lines: &[&str]) -> Vec<f64> {
        let clock = clock();
        lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| tracker.observe_line(line, clock.at(index as u64 * 1000)))
            .map(|reading| reading.ratio)
            .collect()
    }

    #[test]
    fn single_stream_progress_maps_to_bytes_and_ratio() {
        let mut tracker = DownloadProgress::new();
        let clock = clock();
        let first = tracker
            .observe_line("LVPROGRESS 25 100 NA 18", clock.at(0))
            .unwrap();
        assert_eq!(first.bytes_completed, 25);
        assert_eq!(first.bytes_total, Some(100));
        assert!((first.ratio - 0.25).abs() < 1e-9);
        let last = tracker
            .observe_line("LVPROGRESS 100 100 NA 18", clock.at(1000))
            .unwrap();
        assert_eq!(last.bytes_completed, 100);
        assert!((last.ratio - 1.0).abs() < 1e-9);
    }

    #[test]
    fn two_streams_sum_and_the_ratio_never_drops_when_the_second_starts() {
        let mut tracker = DownloadProgress::new();
        let observed = ratios(
            &mut tracker,
            &[
                "LVFORMATS 160, 139",
                "LVPROGRESS 50 100 NA 160",
                "LVPROGRESS 100 100 NA 160",
                // Audio begins at zero with an unknown size: hold the ratio.
                "LVPROGRESS 0 NA NA 139",
                "LVPROGRESS 10 40 NA 139",
                "LVPROGRESS 40 40 NA 139",
            ],
        );
        assert!(
            observed.windows(2).all(|pair| pair[0] <= pair[1]),
            "ratio dropped: {observed:?}"
        );
        // Video done but audio not started: capped, not complete.
        assert!(observed
            .iter()
            .any(|ratio| (ratio - UNSTARTED_STREAM_RATIO_CAP).abs() < 1e-9));
        assert!((observed.last().unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn totals_and_bytes_aggregate_across_streams() {
        let mut tracker = DownloadProgress::new();
        let clock = clock();
        tracker.observe_line("LVFORMATS 160, 139", clock.at(0));
        let video = tracker
            .observe_line("LVPROGRESS 100 100 NA 160", clock.at(1000))
            .unwrap();
        // The audio stream is still to come, so the total is not claimed yet.
        assert_eq!(video.bytes_total, None);
        // A stream that has just started changes nothing visible: no publish.
        assert_eq!(
            tracker.observe_line("LVPROGRESS 0 NA NA 139", clock.at(2000)),
            None
        );
        let audio = tracker
            .observe_line("LVPROGRESS 20 40 NA 139", clock.at(3000))
            .unwrap();
        assert_eq!(audio.bytes_completed, 120);
        assert_eq!(audio.bytes_total, Some(140));
    }

    #[test]
    fn a_single_stream_without_a_format_list_reports_its_total() {
        let mut tracker = DownloadProgress::new();
        let reading = tracker
            .observe_line("LVPROGRESS 5 10 NA 18", clock().at(0))
            .unwrap();
        assert_eq!(reading.bytes_total, Some(10));
    }

    #[test]
    fn a_restarted_stream_never_moves_bytes_or_ratio_backwards() {
        let mut tracker = DownloadProgress::new();
        let clock = clock();
        let before = tracker
            .observe_line("LVPROGRESS 60 100 NA 18", clock.at(0))
            .unwrap();
        // The same format id restarting at a lower count changes nothing
        // visible, so nothing is published.
        assert_eq!(
            tracker.observe_line("LVPROGRESS 3 100 NA 18", clock.at(1000)),
            None
        );
        let after = tracker
            .observe_line("LVPROGRESS 70 100 NA 18", clock.at(2000))
            .unwrap();
        assert!(after.bytes_completed >= before.bytes_completed);
        assert!(after.ratio >= before.ratio);
    }

    #[test]
    fn estimate_only_totals_never_report_completion() {
        let mut tracker = DownloadProgress::new();
        let observed = ratios(
            &mut tracker,
            &[
                "LVPROGRESS 1024 NA 1024 269",
                "LVPROGRESS 232744 NA 326368.0 269",
            ],
        );
        assert!(observed
            .iter()
            .all(|ratio| *ratio <= ESTIMATE_RATIO_CAP + 1e-9));
        let mut tracker = DownloadProgress::new();
        let reading = tracker
            .observe_line("LVPROGRESS 232744 232744 NA 269", clock().at(0))
            .unwrap();
        assert!((reading.ratio - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_noisy_estimate_cannot_pull_the_ratio_down() {
        let mut tracker = DownloadProgress::new();
        let observed = ratios(
            &mut tracker,
            &[
                "LVPROGRESS 64512 NA 95232.0 269",
                "LVPROGRESS 66552 NA 399312.0 269",
                "LVPROGRESS 67576 NA 99828.0 269",
            ],
        );
        assert_eq!(observed.len(), 3);
        assert!(
            observed.windows(2).all(|pair| pair[0] <= pair[1]),
            "{observed:?}"
        );
    }

    #[test]
    fn exact_total_is_not_downgraded_by_a_later_estimate() {
        let mut tracker = DownloadProgress::new();
        let clock = clock();
        tracker.observe_line("LVPROGRESS 10 100 NA 18", clock.at(0));
        let reading = tracker
            .observe_line("LVPROGRESS 20 NA 9000.0 18", clock.at(1000))
            .unwrap();
        assert_eq!(reading.bytes_total, Some(100));
    }

    #[test]
    fn subtitle_records_do_not_count_as_a_stream() {
        let mut tracker = DownloadProgress::new();
        let clock = clock();
        assert_eq!(
            tracker.observe_line("LVPROGRESS 440 440 NA NA", clock.at(0)),
            None
        );
        let reading = tracker
            .observe_line("LVPROGRESS 10 100 NA 18", clock.at(1000))
            .unwrap();
        assert_eq!(reading.bytes_completed, 10);
    }

    #[test]
    fn total_is_never_below_the_bytes_already_downloaded() {
        let mut tracker = DownloadProgress::new();
        let reading = tracker
            .observe_line("LVPROGRESS 500 NA 100.0 18", clock().at(0))
            .unwrap();
        assert_eq!(reading.bytes_total, Some(500));
    }

    #[test]
    fn publishes_at_most_four_updates_per_second_and_skips_unchanged_ones() {
        let mut tracker = DownloadProgress::new();
        let clock = clock();
        let mut published = Vec::new();
        // 100 distinct lines at 10 ms spacing span one second.
        for step in 0..100u64 {
            let line = format!("LVPROGRESS {} 1000 NA 18", step * 10 + 1);
            if let Some(reading) = tracker.observe_line(&line, clock.at(step * 10)) {
                published.push((step * 10, reading.bytes_completed));
            }
        }
        // First sample is immediate, then one per 250 ms: t = 0, 250, 500, 750.
        assert_eq!(
            published.iter().map(|(at, _)| *at).collect::<Vec<_>>(),
            vec![0, 250, 500, 750]
        );
        // The newest reading was held back; the next line after the interval
        // releases it, and an identical repeat after that publishes nothing.
        let released = tracker
            .observe_line("LVPROGRESS 991 1000 NA 18", clock.at(5000))
            .unwrap();
        assert_eq!(released.bytes_completed, 991);
        let repeat = tracker.observe_line("LVPROGRESS 991 1000 NA 18", clock.at(6000));
        assert_eq!(repeat, None);
    }

    #[test]
    fn flush_returns_only_a_held_back_reading() {
        let mut tracker = DownloadProgress::new();
        let clock = clock();
        assert_eq!(tracker.flush(clock.at(0)), None, "nothing observed yet");
        tracker
            .observe_line("LVPROGRESS 10 100 NA 18", clock.at(0))
            .unwrap();
        assert_eq!(
            tracker.observe_line("LVPROGRESS 100 100 NA 18", clock.at(50)),
            None
        );
        let flushed = tracker.flush(clock.at(60)).unwrap();
        assert_eq!(flushed.bytes_completed, 100);
        assert_eq!(tracker.flush(clock.at(70)), None, "already published");
    }

    #[test]
    fn stream_cardinality_is_bounded() {
        let mut tracker = DownloadProgress::new();
        let clock = clock();
        for index in 0..(MAX_STREAMS as u64 + 10) {
            tracker.observe_line(
                &format!("LVPROGRESS 1 10 NA f{index}"),
                clock.at(index * 1000),
            );
        }
        assert_eq!(tracker.streams.len(), MAX_STREAMS);
    }
}
