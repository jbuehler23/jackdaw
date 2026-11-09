//! What each game instance printed. A launched game's stdout and stderr are
//! read on threads of their own, echoed to the editor's own streams as they
//! arrive, and kept as plain lines in a bounded buffer per instance for the
//! Game Output window and remote callers. A buffer is emptied when Play
//! launches its instance again and kept after the game exits.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use bevy::prelude::*;

use crate::pie::InstanceKey;

/// How many lines one instance's buffer keeps.
pub const OUTPUT_LINES: usize = 5000;

/// How many characters of one line a buffer keeps.
pub const OUTPUT_LINE_CHARS: usize = 1000;

/// How many bytes of a line still being written are held. Enough for
/// [`OUTPUT_LINE_CHARS`] characters and the escape sequences around them.
const PENDING_LINE_BYTES: usize = OUTPUT_LINE_CHARS * 8;

/// How long a reaped child's readers may take to reach the end of its pipes. A
/// grandchild holding a pipe open would otherwise hold them forever.
const READER_DRAIN: Duration = Duration::from_millis(250);

/// The stream a line came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

impl OutputStream {
    pub fn name(self) -> &'static str {
        match self {
            OutputStream::Stdout => "stdout",
            OutputStream::Stderr => "stderr",
        }
    }
}

/// The log level a line names, when it names one worth marking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputLevel {
    Error,
    Warn,
}

/// The level of a log line, from a level word among its first few words, as
/// in `2026-01-01T00:00:00Z  WARN target: message`.
pub fn line_level(text: &str) -> Option<OutputLevel> {
    text.split_whitespace().take(3).find_map(|word| {
        match word.trim_matches(|c| c == '[' || c == ']' || c == ':') {
            "ERROR" => Some(OutputLevel::Error),
            "WARN" | "WARNING" => Some(OutputLevel::Warn),
            _ => None,
        }
    })
}

/// One line a game printed, without terminal escape sequences.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputLine {
    /// Counts up from 0 over every line the instance has printed in this
    /// editor session, across launches, so a reader can ask for the lines
    /// after the last it saw.
    pub seq: u64,
    pub stream: OutputStream,
    pub text: Arc<str>,
}

#[derive(Default)]
struct InstanceOutput {
    lines: VecDeque<OutputLine>,
    next_seq: u64,
    /// Changes whenever the buffer is emptied, so a reader holding lines knows
    /// to start over.
    generation: u64,
    /// The launch writing into the buffer, against the store's launch count.
    launch: u64,
}

#[derive(Default)]
struct Store {
    instances: HashMap<InstanceKey, InstanceOutput>,
    launches: u64,
}

#[derive(Default)]
struct Shared {
    store: Mutex<Store>,
    /// Bumped on every change, so a view can tell cheaply that it has nothing
    /// new to show.
    revision: AtomicU64,
}

/// Every instance's output buffer, shared with the threads reading the games.
#[derive(Resource, Clone, Default)]
pub struct GameOutput(Arc<Shared>);

/// Lines read from one instance's buffer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputPage {
    pub lines: Vec<OutputLine>,
    /// The buffer's generation, which changes whenever it is emptied.
    pub generation: u64,
    /// The sequence number of the oldest line still kept.
    pub first_seq: u64,
    /// The sequence number the next line will take.
    pub next_seq: u64,
    /// Whether lines were left out to keep to the limit.
    pub more: bool,
}

impl GameOutput {
    fn store(&self) -> MutexGuard<'_, Store> {
        self.0
            .store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn touch(&self) {
        self.0.revision.fetch_add(1, Ordering::Relaxed);
    }

    /// Counts every change to any buffer.
    pub fn revision(&self) -> u64 {
        self.0.revision.load(Ordering::Relaxed)
    }

    /// Empty `key`'s buffer for a fresh launch and make it the latest launched.
    /// Returns the launch token its lines are written under.
    fn begin(&self, key: &InstanceKey) -> u64 {
        let launch = {
            let mut store = self.store();
            store.launches += 1;
            let launch = store.launches;
            let output = store.instances.entry(key.clone()).or_default();
            output.lines.clear();
            output.generation += 1;
            output.launch = launch;
            launch
        };
        self.touch();
        launch
    }

    /// Add one plain line from `key`'s launch `launch`. A line from an earlier
    /// launch, read after the next one began, is dropped.
    fn push(&self, key: &InstanceKey, launch: u64, stream: OutputStream, text: String) {
        let text: Arc<str> = text.into();
        {
            let mut store = self.store();
            let Some(output) = store.instances.get_mut(key) else {
                return;
            };
            if output.launch != launch {
                return;
            }
            if output.lines.len() == OUTPUT_LINES {
                output.lines.pop_front();
            }
            let seq = output.next_seq;
            output.next_seq += 1;
            output.lines.push_back(OutputLine { seq, stream, text });
        }
        self.touch();
    }

    /// Empty `key`'s buffer. Its sequence numbers carry on, and the running
    /// game's later lines still arrive.
    pub fn clear(&self, key: &InstanceKey) {
        if let Some(output) = self.store().instances.get_mut(key) {
            output.lines.clear();
            output.generation += 1;
        }
        self.touch();
    }

    /// The instances that have a buffer, the most recently launched first.
    pub fn keys(&self) -> Vec<InstanceKey> {
        let store = self.store();
        let mut keys: Vec<(&InstanceKey, u64)> = store
            .instances
            .iter()
            .map(|(key, output)| (key, output.launch))
            .collect();
        keys.sort_by_key(|(_, launch)| std::cmp::Reverse(*launch));
        keys.into_iter().map(|(key, _)| key.clone()).collect()
    }

    /// Whether `key` has a buffer.
    pub fn has(&self, key: &InstanceKey) -> bool {
        self.store().instances.contains_key(key)
    }

    /// Every line of `key` after the sequence number `after`.
    pub fn after(&self, key: &InstanceKey, after: Option<u64>) -> Option<OutputPage> {
        let store = self.store();
        let output = store.instances.get(key)?;
        let start = after.map_or(0, |after| {
            output.lines.partition_point(|line| line.seq <= after)
        });
        let lines = output.lines.range(start..).cloned().collect();
        Some(page(output, lines, false))
    }

    /// The last `limit` lines of `key` whose text contains `filter`, ignoring
    /// case.
    pub fn tail(&self, key: &InstanceKey, filter: &str, limit: usize) -> Option<OutputPage> {
        if filter.is_empty() {
            let store = self.store();
            let output = store.instances.get(key)?;
            let start = output.lines.len().saturating_sub(limit);
            let lines = output.lines.range(start..).cloned().collect();
            return Some(page(output, lines, start > 0));
        }
        let mut page = self.after(key, None)?;
        let filter = TextFilter::new(filter);
        page.lines.retain(|line| filter.matches(&line.text));
        let start = page.lines.len().saturating_sub(limit);
        page.more = start > 0;
        page.lines.drain(..start);
        Some(page)
    }

    /// Up to `limit` lines of `key` from the sequence number `since` on, or its
    /// last `limit` lines without `since`.
    pub fn page(&self, key: &InstanceKey, since: Option<u64>, limit: usize) -> Option<OutputPage> {
        let Some(since) = since else {
            return self.tail(key, "", limit);
        };
        let store = self.store();
        let output = store.instances.get(key)?;
        let start = output.lines.partition_point(|line| line.seq < since);
        let end = start.saturating_add(limit).min(output.lines.len());
        let lines = output.lines.range(start..end).cloned().collect();
        Some(page(output, lines, end < output.lines.len()))
    }

    /// The last `count` lines `key` wrote to `stream`.
    pub fn last_lines(&self, key: &InstanceKey, stream: OutputStream, count: usize) -> Vec<String> {
        let store = self.store();
        let Some(output) = store.instances.get(key) else {
            return Vec::new();
        };
        let mut lines: Vec<String> = output
            .lines
            .iter()
            .rev()
            .filter(|line| line.stream == stream)
            .take(count)
            .map(|line| line.text.to_string())
            .collect();
        lines.reverse();
        lines
    }

    /// Start keeping `child`'s piped stdout and stderr as `key`'s output,
    /// emptying what an earlier launch of `key` left.
    pub(crate) fn capture(&self, key: &InstanceKey, child: &mut Child) -> OutputCapture {
        let launch = self.begin(key);
        let mut readers = Vec::with_capacity(2);
        if let Some(stdout) = child.stdout.take() {
            readers.push(self.read_stream(key, launch, OutputStream::Stdout, stdout));
        }
        if let Some(stderr) = child.stderr.take() {
            readers.push(self.read_stream(key, launch, OutputStream::Stderr, stderr));
        }
        OutputCapture {
            output: self.clone(),
            key: key.clone(),
            readers,
        }
    }

    fn read_stream(
        &self,
        key: &InstanceKey,
        launch: u64,
        stream: OutputStream,
        mut pipe: impl Read + Send + 'static,
    ) -> JoinHandle<()> {
        let output = self.clone();
        let key = key.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            let mut lines = LineSplitter::default();
            loop {
                let read = match pipe.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                echo(stream, &chunk[..read]);
                lines.feed(&chunk[..read], |line| {
                    output.push(&key, launch, stream, line);
                });
            }
            if let Some(line) = lines.finish() {
                output.push(&key, launch, stream, line);
            }
        })
    }
}

fn page(output: &InstanceOutput, lines: Vec<OutputLine>, more: bool) -> OutputPage {
    OutputPage {
        lines,
        generation: output.generation,
        first_seq: output
            .lines
            .front()
            .map_or(output.next_seq, |line| line.seq),
        next_seq: output.next_seq,
        more,
    }
}

/// A case-insensitive substring test, without allocating for ASCII text.
pub struct TextFilter {
    needle: String,
    ascii: bool,
}

impl TextFilter {
    pub fn new(filter: &str) -> Self {
        Self {
            needle: filter.to_lowercase(),
            ascii: filter.is_ascii(),
        }
    }

    pub fn matches(&self, text: &str) -> bool {
        if self.needle.is_empty() {
            return true;
        }
        if self.ascii {
            let needle = self.needle.as_bytes();
            return text
                .as_bytes()
                .windows(needle.len())
                .any(|window| window.eq_ignore_ascii_case(needle));
        }
        text.to_lowercase().contains(&self.needle)
    }
}

/// Splits a stream into plain lines. A carriage return starts the line over,
/// as a terminal draws a progress bar, and a line longer than
/// [`PENDING_LINE_BYTES`] keeps only its start.
#[derive(Default)]
struct LineSplitter {
    pending: Vec<u8>,
    returned: bool,
}

impl LineSplitter {
    fn feed(&mut self, bytes: &[u8], mut line: impl FnMut(String)) {
        for &byte in bytes {
            match byte {
                b'\n' => {
                    self.returned = false;
                    line(plain_line(
                        &String::from_utf8_lossy(&self.pending),
                        OUTPUT_LINE_CHARS,
                    ));
                    self.pending.clear();
                }
                b'\r' => self.returned = true,
                _ => {
                    if std::mem::take(&mut self.returned) {
                        self.pending.clear();
                    }
                    if self.pending.len() < PENDING_LINE_BYTES {
                        self.pending.push(byte);
                    }
                }
            }
        }
    }

    fn finish(self) -> Option<String> {
        (!self.pending.is_empty())
            .then(|| plain_line(&String::from_utf8_lossy(&self.pending), OUTPUT_LINE_CHARS))
    }
}

/// Write a game's bytes to the editor's own stream of the same kind.
fn echo(stream: OutputStream, bytes: &[u8]) {
    let written = match stream {
        OutputStream::Stdout => {
            let mut out = std::io::stdout().lock();
            out.write_all(bytes).and_then(|()| out.flush())
        }
        OutputStream::Stderr => std::io::stderr().lock().write_all(bytes),
    };
    written.ok();
}

/// The threads reading one launched child's output into its buffer.
pub(crate) struct OutputCapture {
    output: GameOutput,
    key: InstanceKey,
    readers: Vec<JoinHandle<()>>,
}

impl OutputCapture {
    /// A capture reading nothing, into a buffer of its own.
    #[cfg(test)]
    pub(crate) fn detached(key: &InstanceKey) -> Self {
        let output = GameOutput::default();
        output.begin(key);
        Self {
            output,
            key: key.clone(),
            readers: Vec::new(),
        }
    }

    /// Wait a moment for the readers to take the lines an exited child wrote
    /// last, then return the last `count` lines it wrote to stderr.
    pub(crate) fn finish(self, count: usize) -> Vec<String> {
        let until = Instant::now() + READER_DRAIN;
        while self.readers.iter().any(|reader| !reader.is_finished()) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(2));
        }
        self.output
            .last_lines(&self.key, OutputStream::Stderr, count)
    }
}

/// `line` without terminal escape sequences or control characters, with tabs
/// as spaces, cut to `max_chars` characters.
pub fn plain_line(line: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(line.len().min(max_chars));
    let mut chars = line.chars().peekable();
    let mut kept = 0;
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            } else {
                chars.next();
            }
            continue;
        }
        let c = if c == '\t' { ' ' } else { c };
        if c.is_control() {
            continue;
        }
        if kept == max_chars {
            out.push_str("...");
            break;
        }
        out.push(c);
        kept += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(instance: u32) -> InstanceKey {
        InstanceKey {
            config: "Game".to_string(),
            instance,
        }
    }

    fn texts(page: &OutputPage) -> Vec<&str> {
        page.lines.iter().map(|line| &*line.text).collect()
    }

    fn push(output: &GameOutput, launch: u64, stream: OutputStream, text: &str) {
        output.push(&key(1), launch, stream, text.to_string());
    }

    fn split(chunks: &[&[u8]]) -> Vec<String> {
        let mut splitter = LineSplitter::default();
        let mut lines = Vec::new();
        for chunk in chunks {
            splitter.feed(chunk, |line| lines.push(line));
        }
        lines.extend(splitter.finish());
        lines
    }

    #[test]
    fn a_buffer_keeps_its_newest_lines_up_to_the_cap() {
        let output = GameOutput::default();
        let launch = output.begin(&key(1));
        for i in 0..OUTPUT_LINES + 10 {
            push(&output, launch, OutputStream::Stdout, &format!("line {i}"));
        }
        let page = output.after(&key(1), None).expect("a buffer");
        assert_eq!(page.lines.len(), OUTPUT_LINES);
        assert_eq!(&*page.lines[0].text, "line 10");
        assert_eq!(page.first_seq, 10);
        assert_eq!(page.next_seq, (OUTPUT_LINES + 10) as u64);
    }

    #[test]
    fn lines_lose_escape_sequences_and_are_cut_short() {
        assert_eq!(
            plain_line("\u{1b}[1m\u{1b}[31merror\u{1b}[0m:\tboom", 200),
            "error: boom"
        );
        let cut = plain_line(&"x".repeat(500), 200);
        assert_eq!(cut.len(), 203);
        assert!(cut.ends_with("..."));
    }

    #[test]
    fn a_carriage_return_starts_the_line_over_and_a_long_line_is_held_short() {
        assert_eq!(
            split(&[b"10%\r50%", b"\r100%\r\ndone\n", b"tail"]),
            ["100%", "done", "tail"]
        );
        let long = vec![b'x'; PENDING_LINE_BYTES * 3];
        let lines = split(&[&long, b"\n"]);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), OUTPUT_LINE_CHARS + 3);
    }

    #[test]
    fn each_line_keeps_the_stream_it_came_from() {
        let output = GameOutput::default();
        let launch = output.begin(&key(1));
        push(&output, launch, OutputStream::Stdout, "out");
        push(&output, launch, OutputStream::Stderr, "err");
        let page = output.after(&key(1), None).expect("a buffer");
        let streams: Vec<OutputStream> = page.lines.iter().map(|line| line.stream).collect();
        assert_eq!(streams, [OutputStream::Stdout, OutputStream::Stderr]);
        assert_eq!(output.last_lines(&key(1), OutputStream::Stderr, 8), ["err"]);
    }

    #[test]
    fn a_relaunch_empties_the_buffer_and_drops_the_old_launchs_late_lines() {
        let output = GameOutput::default();
        let first = output.begin(&key(1));
        push(&output, first, OutputStream::Stdout, "first run");
        let second = output.begin(&key(1));
        push(&output, first, OutputStream::Stdout, "late line");
        push(&output, second, OutputStream::Stdout, "second run");
        let page = output.after(&key(1), None).expect("a buffer");
        assert_eq!(texts(&page), ["second run"]);
        assert_eq!(page.lines[0].seq, 1, "sequence numbers carry on");
    }

    #[test]
    fn lines_printed_after_a_clear_still_arrive() {
        let output = GameOutput::default();
        let launch = output.begin(&key(1));
        push(&output, launch, OutputStream::Stdout, "before");
        let generation = output.after(&key(1), None).expect("a buffer").generation;
        output.clear(&key(1));
        push(&output, launch, OutputStream::Stdout, "after");
        let page = output.after(&key(1), None).expect("a buffer");
        assert_eq!(texts(&page), ["after"]);
        assert_ne!(page.generation, generation, "a view starts over");
    }

    #[test]
    fn reads_page_from_a_sequence_number_and_filter_the_tail() {
        let output = GameOutput::default();
        let launch = output.begin(&key(1));
        for text in ["Loading", "ready", "loaded level", "tick"] {
            push(&output, launch, OutputStream::Stdout, text);
        }
        let page = output.page(&key(1), Some(1), 2).expect("a buffer");
        assert_eq!(texts(&page), ["ready", "loaded level"]);
        assert!(page.more);
        let page = output.tail(&key(1), "LOAD", 10).expect("a buffer");
        assert_eq!(texts(&page), ["Loading", "loaded level"]);
        let page = output.after(&key(1), Some(2)).expect("a buffer");
        assert_eq!(texts(&page), ["tick"]);
        let page = output.page(&key(1), None, 0).expect("a buffer");
        assert!(page.lines.is_empty());
        assert_eq!(page.first_seq, 0);
    }

    #[test]
    fn a_line_takes_the_level_its_log_prefix_names() {
        assert_eq!(
            line_level("2026-11-09T01:38:03Z ERROR bevy_asset: missing"),
            Some(OutputLevel::Error)
        );
        assert_eq!(
            line_level("2026-11-09T01:38:03Z  WARN jackdaw_bsn: gone"),
            Some(OutputLevel::Warn)
        );
        assert_eq!(line_level("2026-11-09T01:38:03Z  INFO loaded"), None);
        assert_eq!(line_level("a message about an ERROR somewhere"), None);
    }

    #[test]
    fn the_filter_ignores_case() {
        assert!(TextFilter::new("load").matches("Loaded Level"));
        assert!(!TextFilter::new("load").matches("ready"));
        assert!(TextFilter::new("\u{c4}").matches("\u{e4}"));
    }

    #[test]
    fn instances_list_the_latest_launch_first() {
        let output = GameOutput::default();
        output.begin(&key(1));
        output.begin(&key(2));
        assert_eq!(output.keys(), [key(2), key(1)]);
        output.begin(&key(1));
        assert_eq!(output.keys(), [key(1), key(2)]);
    }

    #[cfg(unix)]
    #[test]
    fn a_captured_child_keeps_both_streams() {
        let mut child = std::process::Command::new("sh")
            .args(["-c", "echo out; echo err >&2; printf 'no newline'"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("a shell");
        let output = GameOutput::default();
        let capture = output.capture(&key(1), &mut child);
        child.wait().expect("the shell exits");
        assert_eq!(capture.finish(8), ["err"]);
        let page = output.after(&key(1), None).expect("a buffer");
        let mut lines = texts(&page);
        lines.sort_unstable();
        assert_eq!(lines, ["err", "no newline", "out"]);
    }
}
