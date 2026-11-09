//! What each game instance printed. A launched game's stdout and stderr are
//! read on threads of their own, echoed to the editor's own streams, and kept
//! as plain lines in a bounded buffer per instance for the Game Output window
//! and remote callers. A buffer is emptied when Play launches its instance
//! again and kept after the game exits.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
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

/// One line a game printed, without terminal escape sequences.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputLine {
    /// Counts up from 0 over every line the instance has printed, across
    /// launches, so a reader can ask for the lines after the last it saw.
    pub seq: u64,
    pub stream: OutputStream,
    pub text: String,
}

#[derive(Default)]
struct InstanceOutput {
    lines: VecDeque<OutputLine>,
    next_seq: u64,
    /// Changes whenever the buffer is emptied, so a reader holding lines knows
    /// to start over.
    generation: u64,
    /// When the instance was last launched, against the store's launch count.
    launch: u64,
}

impl InstanceOutput {
    fn empty(&mut self) {
        self.lines.clear();
        self.generation += 1;
    }
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
    /// The buffer's generation; see [`GameOutput::tail`].
    pub generation: u64,
    /// The sequence number of the oldest line still kept.
    pub first_seq: u64,
    /// The sequence number the next line will take.
    pub next_seq: u64,
    /// Whether matching lines were left out to keep to the limit.
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
    /// Returns the generation the launch's lines are written under.
    fn begin(&self, key: &InstanceKey) -> u64 {
        let generation = {
            let mut store = self.store();
            store.launches += 1;
            let launch = store.launches;
            let output = store.instances.entry(key.clone()).or_default();
            output.empty();
            output.launch = launch;
            output.generation
        };
        self.touch();
        generation
    }

    /// Add one raw line from `key`'s launch of `generation`. A line from an
    /// earlier launch, read after the buffer was emptied, is dropped.
    fn push(&self, key: &InstanceKey, generation: u64, stream: OutputStream, raw: &str) {
        {
            let mut store = self.store();
            let Some(output) = store.instances.get_mut(key) else {
                return;
            };
            if output.generation != generation {
                return;
            }
            if output.lines.len() == OUTPUT_LINES {
                output.lines.pop_front();
            }
            let seq = output.next_seq;
            output.next_seq += 1;
            output.lines.push_back(OutputLine {
                seq,
                stream,
                text: plain_line(raw, OUTPUT_LINE_CHARS),
            });
        }
        self.touch();
    }

    /// Empty `key`'s buffer. Its sequence numbers carry on.
    pub fn clear(&self, key: &InstanceKey) {
        if let Some(output) = self.store().instances.get_mut(key) {
            output.empty();
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

    /// The last `limit` lines of `key` after the sequence number `after` whose
    /// text contains `filter`, ignoring case.
    pub fn tail(
        &self,
        key: &InstanceKey,
        after: Option<u64>,
        filter: &str,
        limit: usize,
    ) -> Option<OutputPage> {
        let store = self.store();
        let output = store.instances.get(key)?;
        let filter = filter.to_lowercase();
        let mut lines: Vec<OutputLine> = output
            .lines
            .iter()
            .rev()
            .take_while(|line| after.is_none_or(|after| line.seq > after))
            .filter(|line| filter.is_empty() || line.text.to_lowercase().contains(&filter))
            .take(limit.saturating_add(1))
            .cloned()
            .collect();
        let more = lines.len() > limit;
        lines.truncate(limit);
        lines.reverse();
        Some(page(output, lines, more))
    }

    /// Up to `limit` lines of `key` from the sequence number `since` on, or its
    /// last `limit` lines without `since`.
    pub fn page(&self, key: &InstanceKey, since: Option<u64>, limit: usize) -> Option<OutputPage> {
        let Some(since) = since else {
            return self.tail(key, None, "", limit);
        };
        let store = self.store();
        let output = store.instances.get(key)?;
        let mut lines: Vec<OutputLine> = output
            .lines
            .iter()
            .skip_while(|line| line.seq < since)
            .take(limit.saturating_add(1))
            .cloned()
            .collect();
        let more = lines.len() > limit;
        lines.truncate(limit);
        Some(page(output, lines, more))
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
            .map(|line| line.text.clone())
            .collect();
        lines.reverse();
        lines
    }

    /// Start keeping `child`'s piped stdout and stderr as `key`'s output,
    /// emptying what an earlier launch of `key` left.
    pub(crate) fn capture(&self, key: &InstanceKey, child: &mut Child) -> OutputCapture {
        let generation = self.begin(key);
        let mut readers = Vec::with_capacity(2);
        if let Some(stdout) = child.stdout.take() {
            readers.push(self.read_stream(key, generation, OutputStream::Stdout, stdout));
        }
        if let Some(stderr) = child.stderr.take() {
            readers.push(self.read_stream(key, generation, OutputStream::Stderr, stderr));
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
        generation: u64,
        stream: OutputStream,
        pipe: impl Read + Send + 'static,
    ) -> JoinHandle<()> {
        let output = self.clone();
        let key = key.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(pipe);
            let mut raw = Vec::new();
            loop {
                raw.clear();
                match reader.read_until(b'\n', &mut raw) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                echo(stream, &raw);
                let line = String::from_utf8_lossy(&raw);
                let line = line.trim_end_matches(['\n', '\r']);
                output.push(&key, generation, stream, line);
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

/// Write a game's line to the editor's own stream of the same kind.
fn echo(stream: OutputStream, raw: &[u8]) {
    let written = match stream {
        OutputStream::Stdout => {
            let mut out = std::io::stdout().lock();
            out.write_all(raw).and_then(|()| terminate(&mut out, raw))
        }
        OutputStream::Stderr => {
            let mut out = std::io::stderr().lock();
            out.write_all(raw).and_then(|()| terminate(&mut out, raw))
        }
    };
    written.ok();
}

fn terminate(out: &mut impl Write, raw: &[u8]) -> std::io::Result<()> {
    if raw.ends_with(b"\n") {
        Ok(())
    } else {
        out.write_all(b"\n")
    }
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
        page.lines.iter().map(|line| line.text.as_str()).collect()
    }

    #[test]
    fn a_buffer_keeps_its_newest_lines_up_to_the_cap() {
        let output = GameOutput::default();
        let generation = output.begin(&key(1));
        for i in 0..OUTPUT_LINES + 10 {
            output.push(
                &key(1),
                generation,
                OutputStream::Stdout,
                &format!("line {i}"),
            );
        }
        let page = output
            .tail(&key(1), None, "", usize::MAX)
            .expect("a buffer");
        assert_eq!(page.lines.len(), OUTPUT_LINES);
        assert_eq!(page.lines[0].text, "line 10");
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
    fn each_line_keeps_the_stream_it_came_from() {
        let output = GameOutput::default();
        let generation = output.begin(&key(1));
        output.push(&key(1), generation, OutputStream::Stdout, "out");
        output.push(&key(1), generation, OutputStream::Stderr, "err");
        let page = output.tail(&key(1), None, "", 10).expect("a buffer");
        let streams: Vec<OutputStream> = page.lines.iter().map(|line| line.stream).collect();
        assert_eq!(streams, [OutputStream::Stdout, OutputStream::Stderr]);
        assert_eq!(output.last_lines(&key(1), OutputStream::Stderr, 8), ["err"]);
    }

    #[test]
    fn a_relaunch_empties_the_buffer_and_drops_the_old_launchs_late_lines() {
        let output = GameOutput::default();
        let first = output.begin(&key(1));
        output.push(&key(1), first, OutputStream::Stdout, "first run");
        let second = output.begin(&key(1));
        output.push(&key(1), first, OutputStream::Stdout, "late line");
        output.push(&key(1), second, OutputStream::Stdout, "second run");
        let page = output.tail(&key(1), None, "", 10).expect("a buffer");
        assert_eq!(texts(&page), ["second run"]);
        assert_eq!(page.lines[0].seq, 1, "sequence numbers carry on");
    }

    #[test]
    fn reads_page_from_a_sequence_number_and_filter_the_tail() {
        let output = GameOutput::default();
        let generation = output.begin(&key(1));
        for text in ["Loading", "ready", "loaded level", "tick"] {
            output.push(&key(1), generation, OutputStream::Stdout, text);
        }
        let page = output.page(&key(1), Some(1), 2).expect("a buffer");
        assert_eq!(texts(&page), ["ready", "loaded level"]);
        assert!(page.more);
        let page = output.tail(&key(1), None, "LOAD", 10).expect("a buffer");
        assert_eq!(texts(&page), ["Loading", "loaded level"]);
        let page = output.tail(&key(1), Some(2), "", 10).expect("a buffer");
        assert_eq!(texts(&page), ["tick"]);
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
        let page = output.tail(&key(1), None, "", 10).expect("a buffer");
        let mut lines = texts(&page);
        lines.sort_unstable();
        assert_eq!(lines, ["err", "no newline", "out"]);
    }
}
