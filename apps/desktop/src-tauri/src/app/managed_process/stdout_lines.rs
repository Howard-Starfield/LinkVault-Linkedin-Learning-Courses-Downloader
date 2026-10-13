//! Bounded, line-framed observation of a helper's stdout.
//!
//! The supervisor's stdout reader thread normally only captures bytes for the
//! final [`super::ManagedProcessOutput`]. A caller that wants live progress
//! also gets complete lines while the helper is still running. Everything here
//! is lossy by design and never affects the captured stdout: lines longer than
//! [`MAX_OBSERVED_LINE_BYTES`] are dropped and a full queue drops new lines, so
//! a pathological helper cannot exhaust memory or stall its own pipe.

use std::io::{self, Read};
use std::sync::mpsc::SyncSender;

/// Longest line, without its terminator, that is forwarded to an observer.
pub(super) const MAX_OBSERVED_LINE_BYTES: usize = 4 * 1024;
/// Complete lines buffered between the reader thread and the poll loop.
pub(super) const OBSERVED_LINE_QUEUE: usize = 1024;

/// Splits a byte stream into lines across arbitrary read boundaries.
///
/// `\n` terminates a line and one trailing `\r` is removed, so CRLF output is
/// handled. Empty lines are skipped. A line that exceeds
/// [`MAX_OBSERVED_LINE_BYTES`] is discarded up to its terminator.
#[derive(Debug, Default)]
pub(super) struct LineSplitter {
    pending: Vec<u8>,
    overflowed: bool,
}

impl LineSplitter {
    pub(super) fn push(&mut self, bytes: &[u8], emit: &mut dyn FnMut(String)) {
        for segment in bytes.split_inclusive(|byte| *byte == b'\n') {
            let terminated = segment.last() == Some(&b'\n');
            let body = if terminated {
                &segment[..segment.len() - 1]
            } else {
                segment
            };
            if !self.overflowed {
                if body.len() > MAX_OBSERVED_LINE_BYTES - self.pending.len() {
                    self.overflowed = true;
                    self.pending.clear();
                } else {
                    self.pending.extend_from_slice(body);
                }
            }
            if terminated {
                self.complete(emit);
            }
        }
    }

    /// Flush an unterminated final line at end of stream.
    pub(super) fn finish(&mut self, emit: &mut dyn FnMut(String)) {
        self.complete(emit);
    }

    fn complete(&mut self, emit: &mut dyn FnMut(String)) {
        if !self.overflowed {
            if self.pending.last() == Some(&b'\r') {
                self.pending.pop();
            }
            if !self.pending.is_empty() {
                emit(String::from_utf8_lossy(&self.pending).into_owned());
            }
        }
        self.pending.clear();
        self.overflowed = false;
    }
}

/// Same capture and cap semantics as [`super::read_bounded`], additionally
/// forwarding complete lines to `lines`. Lines keep flowing after the capture
/// limit is reached. A full or disconnected queue drops the line.
pub(super) fn read_bounded_observed<R: Read>(
    reader: R,
    limit: usize,
    lines: &SyncSender<String>,
) -> io::Result<(Vec<u8>, bool)> {
    let mut splitter = LineSplitter::default();
    let mut forward = |line: String| {
        let _ = lines.try_send(line);
    };
    let result = super::read_bounded_with(reader, limit, &mut |chunk| {
        splitter.push(chunk, &mut forward);
    });
    splitter.finish(&mut forward);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    fn split(chunks: &[&[u8]]) -> Vec<String> {
        let mut splitter = LineSplitter::default();
        let mut lines = Vec::new();
        for chunk in chunks {
            splitter.push(chunk, &mut |line| lines.push(line));
        }
        splitter.finish(&mut |line| lines.push(line));
        lines
    }

    #[test]
    fn splits_lf_and_crlf_lines_and_skips_empty_ones() {
        assert_eq!(
            split(&[b"one\ntwo\r\n\r\n\nthree\n"]),
            vec!["one", "two", "three"]
        );
    }

    #[test]
    fn reassembles_lines_split_across_reads_including_a_split_crlf() {
        assert_eq!(
            split(&[b"al", b"pha\r", b"\nbe", b"ta\ngam", b"ma"]),
            vec!["alpha", "beta", "gamma"]
        );
    }

    #[test]
    fn unterminated_final_line_is_flushed_only_at_finish() {
        let mut splitter = LineSplitter::default();
        let mut lines = Vec::new();
        splitter.push(b"done\npartial", &mut |line| lines.push(line));
        assert_eq!(lines, vec!["done"]);
        splitter.finish(&mut |line| lines.push(line));
        assert_eq!(lines, vec!["done", "partial"]);
    }

    #[test]
    fn over_long_lines_are_dropped_without_losing_the_next_line() {
        let long = vec![b'x'; MAX_OBSERVED_LINE_BYTES + 1];
        let exact = vec![b'y'; MAX_OBSERVED_LINE_BYTES];
        let mut input = long.clone();
        input.extend_from_slice(b"\nok\n");
        input.extend_from_slice(&exact);
        input.push(b'\n');
        // Feed the long line in two reads to prove the discard spans reads.
        let (first, second) = input.split_at(MAX_OBSERVED_LINE_BYTES / 2);
        let lines = split(&[first, second]);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "ok");
        assert_eq!(lines[1].len(), MAX_OBSERVED_LINE_BYTES);
    }

    #[test]
    fn unterminated_over_long_tail_is_dropped_at_finish() {
        let long = vec![b'x'; MAX_OBSERVED_LINE_BYTES * 3];
        assert!(split(&[&long]).is_empty());
    }

    #[test]
    fn non_utf8_bytes_are_replaced_not_rejected() {
        assert_eq!(split(&[b"ok \xff\n"]), vec!["ok \u{fffd}"]);
    }

    #[test]
    fn observed_reader_keeps_forwarding_after_the_capture_limit() {
        let (sender, receiver) = sync_channel(OBSERVED_LINE_QUEUE);
        let input = b"first line\nsecond line\nthird line\n".as_slice();
        let (captured, truncated) = read_bounded_observed(input, 8, &sender).unwrap();
        assert_eq!(captured, b"first li");
        assert!(truncated);
        let lines = receiver.try_iter().collect::<Vec<_>>();
        assert_eq!(lines, vec!["first line", "second line", "third line"]);
    }

    #[test]
    fn full_queue_drops_lines_instead_of_blocking() {
        let (sender, receiver) = sync_channel(2);
        let input = b"a\nb\nc\nd\n".as_slice();
        let (captured, truncated) = read_bounded_observed(input, 1024, &sender).unwrap();
        assert_eq!(captured, b"a\nb\nc\nd\n");
        assert!(!truncated);
        assert_eq!(receiver.try_iter().collect::<Vec<_>>(), vec!["a", "b"]);
    }
}
