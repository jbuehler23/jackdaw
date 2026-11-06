//! The queue carrying every editor-to-game message of one Live instance.
//!
//! A large `ipc-channel` send blocks until the receiver reads, and the game's
//! main thread can itself be blocked sending its state stream to the editor.
//! So the editor's main thread only queues; a writer thread does the sends, in
//! queue order.
//!
//! A stalled game must not pile up one document edit per frame. A
//! [`ControlEvent::NodeComponents`] merges into the queued edit of its node, if
//! there is one behind the last other message: each component keeps its latest
//! value in the earlier edit's place. Edits of different nodes touch different
//! entities, so the merge moves a value ahead of those only. Any other message
//! starts a new stretch, so no edit or value crosses it.
//!
//! A message that takes a while to make, such as a document being written on a
//! worker, holds its place with an [`OutboxSlot`]. What is queued behind it waits
//! until the slot is filled.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

use bevy::log::{debug, error, warn};
use jackdaw_pie_protocol::event::to_bytes;
use jackdaw_pie_protocol::{ControlEvent, IpcChannelTransport, IpcLaneSender, PieChannel};

/// Queued bytes above which the game is reported as not reading.
const BACKLOG_WARN_BYTES: usize = 64 * 1024 * 1024;

/// The main-thread end of a writer thread.
///
/// Dropping it never blocks. If the game was not marked closing, the writer
/// finishes what is queued, then exits and releases its sender, so the game
/// sees the link close once it has read everything. If the game's process
/// dies, the writer's sends fail and it exits the same way. If the stage was
/// marked closing first, the game will not read the backlog, so it is
/// discarded and the writer exits after its current send.
///
/// The exception is a dead game that left a fragmented send unread: on Linux
/// and other Unix-socket platforms `ipc-channel` holds both ends of that
/// send's dedicated channel for the whole call, so it never fails and the
/// writer stays parked until the editor exits. What stays held is the thread,
/// its file descriptors and the in-flight message; the backlog is freed. It
/// blocks nothing else, and the game is gone, so no live peer waits on its
/// sender.
pub(crate) struct GameOutbox {
    shared: Arc<Shared>,
    /// Set once the game is being torn down, when a failed send is expected.
    closing: Arc<AtomicBool>,
}

struct Shared {
    state: Mutex<State>,
    ready: Condvar,
}

/// One encoded message, with its edit when it can still be merged into.
struct Queued {
    bytes: Vec<u8>,
    edit: Option<NodeEdit>,
    /// The slot this place is held for, until it is filled.
    slot: Option<u64>,
}

/// A queued [`ControlEvent::NodeComponents`].
pub(crate) struct NodeEdit {
    pub(crate) node: u64,
    pub(crate) set: Vec<(String, serde_json::Value)>,
    pub(crate) removed: Vec<String>,
}

impl NodeEdit {
    /// Take in a later edit of the same node: a value replaces an earlier value
    /// or removal of its component, and a removal replaces an earlier value.
    fn merge(&mut self, later: NodeEdit) {
        for (path, value) in later.set {
            self.removed.retain(|held| *held != path);
            match self.set.iter_mut().find(|(held, _)| *held == path) {
                Some((_, held)) => *held = value,
                None => self.set.push((path, value)),
            }
        }
        for path in later.removed {
            self.set.retain(|(held, _)| *held != path);
            if !self.removed.contains(&path) {
                self.removed.push(path);
            }
        }
        self.set.sort_by(|a, b| a.0.cmp(&b.0));
    }

    fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        to_bytes(&ControlEvent::NodeComponents {
            node: self.node,
            set: self.set.clone(),
            removed: self.removed.clone(),
        })
    }
}

#[derive(Default)]
struct State {
    queue: VecDeque<Queued>,
    /// Messages taken from the queue so far, so a position stays valid as the
    /// front moves.
    taken: u64,
    /// Where each node's edit sits, for the edits behind the last other
    /// message and not yet taken.
    edits: HashMap<u64, u64>,
    bytes: usize,
    open: bool,
    warned: bool,
    next_slot: u64,
}

impl State {
    /// Returns true when this push first crossed the backlog threshold.
    fn push(&mut self, bytes: Vec<u8>) -> bool {
        self.edits.clear();
        self.append(Queued {
            bytes,
            edit: None,
            slot: None,
        })
    }

    fn reserve(&mut self) -> u64 {
        self.edits.clear();
        let slot = self.next_slot;
        self.next_slot += 1;
        self.queue.push_back(Queued {
            bytes: Vec::new(),
            edit: None,
            slot: Some(slot),
        });
        slot
    }

    /// Put `bytes`, if any, in the place `slot` holds. Returns true when this first
    /// crossed the backlog threshold.
    fn fill(&mut self, slot: u64, bytes: Option<Vec<u8>>) -> bool {
        let Some(queued) = self.queue.iter_mut().find(|held| held.slot == Some(slot)) else {
            return false;
        };
        queued.slot = None;
        queued.bytes = bytes.unwrap_or_default();
        self.bytes = self.bytes.saturating_add(queued.bytes.len());
        self.crossed_threshold()
    }

    /// Queue `edit`, or merge it into the queued edit of its node. Returns true
    /// when this first crossed the backlog threshold.
    fn push_edit(&mut self, edit: NodeEdit) -> bool {
        let position = self.edits.get(&edit.node).copied();
        let Some(queued) = position.and_then(|at| self.queue.get_mut((at - self.taken) as usize))
        else {
            return match edit.encode() {
                Ok(bytes) => {
                    let at = self.taken + self.queue.len() as u64;
                    self.edits.insert(edit.node, at);
                    self.append(Queued {
                        bytes,
                        edit: Some(edit),
                        slot: None,
                    })
                }
                Err(err) => {
                    error!("PIE: failed to encode an edit of node {}: {err}", edit.node);
                    false
                }
            };
        };
        let Some(earlier) = queued.edit.as_mut() else {
            return false;
        };
        earlier.merge(edit);
        match earlier.encode() {
            Ok(bytes) => {
                let was = std::mem::replace(&mut queued.bytes, bytes);
                self.bytes = self
                    .bytes
                    .saturating_sub(was.len())
                    .saturating_add(queued.bytes.len());
            }
            Err(err) => error!("PIE: failed to encode a merged edit: {err}"),
        }
        self.crossed_threshold()
    }

    fn append(&mut self, queued: Queued) -> bool {
        self.bytes = self.bytes.saturating_add(queued.bytes.len());
        self.queue.push_back(queued);
        self.crossed_threshold()
    }

    fn crossed_threshold(&mut self) -> bool {
        if self.warned || self.bytes <= BACKLOG_WARN_BYTES {
            return false;
        }
        self.warned = true;
        true
    }

    /// The next message to send: none while the front holds a place for one, and a
    /// slot filled with nothing is passed over.
    fn pop(&mut self) -> Option<Vec<u8>> {
        loop {
            if self.queue.front()?.slot.is_some() {
                return None;
            }
            let queued = self.queue.pop_front()?;
            if let Some(edit) = &queued.edit
                && self.edits.get(&edit.node) == Some(&self.taken)
            {
                self.edits.remove(&edit.node);
            }
            self.taken += 1;
            self.bytes = self.bytes.saturating_sub(queued.bytes.len());
            if self.bytes < BACKLOG_WARN_BYTES / 2 {
                self.warned = false;
            }
            if !queued.bytes.is_empty() {
                return Some(queued.bytes);
            }
        }
    }
}

impl Shared {
    fn new() -> Self {
        Self {
            state: Mutex::new(State {
                open: true,
                ..State::default()
            }),
            ready: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Returns true when the backlog just crossed the warning threshold.
    fn push(&self, bytes: Vec<u8>) -> bool {
        let crossed = self.lock().push(bytes);
        self.ready.notify_one();
        crossed
    }

    /// [`Self::push`] for a document edit.
    fn push_edit(&self, edit: NodeEdit) -> bool {
        let crossed = self.lock().push_edit(edit);
        self.ready.notify_one();
        crossed
    }

    fn fill(&self, slot: u64, bytes: Option<Vec<u8>>) {
        let crossed = self.lock().fill(slot, bytes);
        self.ready.notify_one();
        if crossed {
            warn_backlog();
        }
    }

    /// Block until a message is queued, or return `None` once closed with nothing
    /// to send.
    fn next(&self) -> Option<Vec<u8>> {
        let mut state = self.lock();
        loop {
            if let Some(bytes) = state.pop() {
                return Some(bytes);
            }
            if !state.open {
                return None;
            }
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn close(&self, discard: bool) {
        let dropped = {
            let mut state = self.lock();
            state.open = false;
            if discard {
                state.bytes = 0;
                state.edits.clear();
                std::mem::take(&mut state.queue)
            } else {
                VecDeque::new()
            }
        };
        drop(dropped);
        self.ready.notify_all();
    }
}

impl GameOutbox {
    /// Start the writer thread for `transport`'s reliable channel.
    pub(crate) fn new(transport: &IpcChannelTransport) -> io::Result<Self> {
        let (outbox, _writer) = Self::start(transport)?;
        Ok(outbox)
    }

    fn start(transport: &IpcChannelTransport) -> io::Result<(Self, JoinHandle<()>)> {
        let shared = Arc::new(Shared::new());
        let closing = Arc::new(AtomicBool::new(false));
        let lane = transport.lane_sender(PieChannel::Reliable);
        let writer = thread::Builder::new()
            .name("pie-outbox".to_string())
            .spawn({
                let shared = shared.clone();
                let closing = closing.clone();
                move || write_queued(&shared, &lane, &closing)
            })?;
        Ok((Self { shared, closing }, writer))
    }

    /// Queue one encoded message behind everything already queued.
    pub(crate) fn queue(&self, bytes: Vec<u8>) {
        if self.shared.push(bytes) {
            warn_backlog();
        }
    }

    /// Queue a document edit, merged into the queued edit of its node when one
    /// is waiting behind the last other message.
    pub(crate) fn queue_edit(&self, edit: NodeEdit) {
        if self.shared.push_edit(edit) {
            warn_backlog();
        }
    }

    /// Hold the next place in the queue for a message made later.
    pub(crate) fn reserve(&self) -> OutboxSlot {
        let slot = self.shared.lock().reserve();
        OutboxSlot {
            shared: self.shared.clone(),
            slot,
            bytes: None,
        }
    }

    /// Note that the game is about to be killed, so failed sends are expected
    /// and the backlog is discarded when the outbox is dropped.
    pub(crate) fn mark_closing(&self) {
        self.closing.store(true, Ordering::Release);
    }
}

/// A place held in a [`GameOutbox`]. Dropped unfilled, it gives the place up, so
/// what waits behind it goes on.
pub(crate) struct OutboxSlot {
    shared: Arc<Shared>,
    slot: u64,
    bytes: Option<Vec<u8>>,
}

impl OutboxSlot {
    /// Put the message in the place held.
    pub(crate) fn fill(mut self, bytes: Vec<u8>) {
        self.bytes = Some(bytes);
    }
}

impl Drop for OutboxSlot {
    fn drop(&mut self) {
        self.shared.fill(self.slot, self.bytes.take());
    }
}

fn warn_backlog() {
    warn!(
        "PIE outbox: over {} MiB queued for the game, which is not reading",
        BACKLOG_WARN_BYTES / (1024 * 1024)
    );
}

impl Drop for GameOutbox {
    fn drop(&mut self) {
        self.shared.close(self.closing.load(Ordering::Acquire));
    }
}

fn write_queued(shared: &Shared, lane: &IpcLaneSender, closing: &AtomicBool) {
    let mut failed = false;
    while let Some(bytes) = shared.next() {
        let Err(err) = lane.try_send(bytes) else {
            continue;
        };
        if failed {
            debug!("PIE outbox: dropping a message, send failed: {err}");
        } else if closing.load(Ordering::Acquire) {
            debug!("PIE outbox: message not delivered to a closing game: {err}");
        } else {
            warn!("PIE outbox: dropping a message, send failed: {err}");
        }
        failed = true;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use jackdaw_pie_protocol::{connect, serve};

    use super::*;

    const LARGE: usize = 8 * 1024 * 1024;
    const GUARD: Duration = Duration::from_secs(30);

    fn link() -> (IpcChannelTransport, IpcChannelTransport) {
        let (server, name) = serve().expect("a rendezvous");
        let accept = thread::spawn(move || server.accept());
        let game = connect(&name).expect("the game connects");
        let editor = accept
            .join()
            .expect("accept ran")
            .expect("the editor accepts");
        (editor, game)
    }

    fn finishes(writer: &JoinHandle<()>) -> bool {
        let deadline = Instant::now() + GUARD;
        while Instant::now() < deadline {
            if writer.is_finished() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    #[cfg_attr(
        not(target_os = "linux"),
        ignore = "relies on the socket buffer of the Unix transport filling up"
    )]
    fn a_writer_stuck_on_a_full_socket_ends_when_the_game_dies() {
        let (editor, game) = link();
        let (outbox, writer) = GameOutbox::start(&editor).expect("a writer");
        for _ in 0..100_000 {
            outbox.queue(vec![0; 1024]);
        }
        drop(outbox);
        drop(editor);
        thread::sleep(Duration::from_millis(200));
        assert!(!writer.is_finished(), "the socket buffer is full");

        drop(game);
        assert!(finishes(&writer), "the send fails once the peer is gone");
    }

    #[test]
    fn queueing_and_dropping_return_whatever_the_writer_is_doing() {
        let (editor, game) = link();
        let (outbox, _writer) = GameOutbox::start(&editor).expect("a writer");
        outbox.queue(vec![0; LARGE]);
        thread::sleep(Duration::from_millis(200));
        drop(game);

        let shared = outbox.shared.clone();
        let (done, finished) = mpsc::channel();
        thread::spawn(move || {
            outbox.queue(vec![1; 1024]);
            outbox.mark_closing();
            drop(outbox);
            drop(editor);
            done.send(()).ok();
        });
        finished
            .recv_timeout(GUARD)
            .expect("queueing and closing return while the writer is stuck");
        assert!(shared.lock().queue.is_empty(), "the backlog is freed");
    }

    #[test]
    fn a_closing_outbox_discards_its_backlog_and_a_plain_drop_drains_it() {
        let drained = Shared::new();
        drained.push(vec![1]);
        drained.push(vec![2]);
        drained.close(false);
        assert_eq!(drained.next(), Some(vec![1]));
        assert_eq!(drained.next(), Some(vec![2]));
        assert_eq!(drained.next(), None);

        let discarded = Shared::new();
        discarded.push(vec![1]);
        discarded.push(vec![2]);
        discarded.close(true);
        assert_eq!(discarded.next(), None);
        assert_eq!(discarded.lock().bytes, 0);
    }

    #[test]
    fn what_is_queued_behind_a_slot_waits_until_it_is_filled() {
        let shared = Arc::new(Shared::new());
        shared.push(vec![1]);
        let slot = shared.lock().reserve();
        shared.push(vec![3]);
        assert_eq!(shared.lock().pop(), Some(vec![1]));
        assert_eq!(shared.lock().pop(), None, "the place is held");

        shared.fill(slot, Some(vec![2]));
        assert_eq!(shared.lock().pop(), Some(vec![2]));
        assert_eq!(shared.lock().pop(), Some(vec![3]));
        assert_eq!(shared.lock().bytes, 0);
    }

    #[test]
    fn a_slot_given_up_lets_what_waits_behind_it_go() {
        let shared = Arc::new(Shared::new());
        let given_up = OutboxSlot {
            slot: shared.lock().reserve(),
            shared: shared.clone(),
            bytes: None,
        };
        shared.push(vec![1]);
        assert_eq!(shared.lock().pop(), None);
        drop(given_up);
        assert_eq!(shared.lock().pop(), Some(vec![1]));
        assert_eq!(shared.lock().pop(), None);
    }

    #[test]
    fn a_closed_outbox_does_not_wait_for_a_slot() {
        let shared = Shared::new();
        shared.lock().reserve();
        shared.push(vec![1]);
        shared.close(false);
        assert_eq!(shared.next(), None);
    }

    #[test]
    fn an_edit_does_not_merge_across_a_slot() {
        let shared = Shared::new();
        let edit = |node| NodeEdit {
            node,
            set: Vec::new(),
            removed: Vec::new(),
        };
        shared.push_edit(edit(5));
        let slot = shared.lock().reserve();
        shared.push_edit(edit(5));
        assert_eq!(shared.lock().queue.len(), 3);
        shared.fill(slot, None);
        assert!(shared.lock().pop().is_some());
        assert!(
            shared.lock().pop().is_some(),
            "the empty slot is passed over"
        );
        assert!(shared.lock().pop().is_none());
    }

    const A: &str = "game::A";
    const B: &str = "game::B";

    fn edit(node: u64, set: &[(&str, i64)], removed: &[&str]) -> NodeEdit {
        NodeEdit {
            node,
            set: set
                .iter()
                .map(|(path, value)| ((*path).to_string(), serde_json::json!(value)))
                .collect(),
            removed: removed.iter().map(ToString::to_string).collect(),
        }
    }

    fn drain(shared: &Shared) -> Vec<ControlEvent> {
        shared.close(false);
        std::iter::from_fn(|| shared.next())
            .map(|bytes| jackdaw_pie_protocol::event::from_bytes(&bytes).expect("a message"))
            .collect()
    }

    fn sent(node: u64, set: &[(&str, i64)], removed: &[&str]) -> ControlEvent {
        let edit = edit(node, set, removed);
        ControlEvent::NodeComponents {
            node: edit.node,
            set: edit.set,
            removed: edit.removed,
        }
    }

    #[test]
    fn repeated_values_for_a_node_and_component_keep_only_the_latest() {
        let shared = Shared::new();
        for value in 0..1000 {
            shared.push_edit(edit(1, &[(A, value)], &[]));
        }
        shared.push_edit(edit(1, &[(B, 7)], &[]));
        assert_eq!(
            shared.lock().queue.len(),
            1,
            "a stalled game holds one message per node"
        );
        assert_eq!(drain(&shared), vec![sent(1, &[(A, 999), (B, 7)], &[])]);
    }

    #[test]
    fn edits_of_several_nodes_merge_per_node() {
        let shared = Shared::new();
        for value in 0..100 {
            shared.push_edit(edit(1, &[(A, value)], &[]));
            shared.push_edit(edit(2, &[(A, value + 1000)], &[]));
        }
        assert_eq!(
            drain(&shared),
            vec![sent(1, &[(A, 99)], &[]), sent(2, &[(A, 1099)], &[])]
        );
    }

    #[test]
    fn a_removal_and_a_value_of_one_component_replace_each_other() {
        let shared = Shared::new();
        shared.push_edit(edit(1, &[(A, 1), (B, 2)], &[]));
        shared.push_edit(edit(1, &[], &[A]));
        assert_eq!(drain(&shared), vec![sent(1, &[(B, 2)], &[A])]);

        let shared = Shared::new();
        shared.push_edit(edit(1, &[], &[A]));
        shared.push_edit(edit(1, &[(A, 3)], &[]));
        assert_eq!(drain(&shared), vec![sent(1, &[(A, 3)], &[])]);
    }

    #[test]
    fn no_edit_or_value_crosses_another_message() {
        let stop = to_bytes(&ControlEvent::Pause).expect("encode");
        let shared = Shared::new();
        shared.push_edit(edit(1, &[(A, 1)], &[]));
        shared.push(stop);
        shared.push_edit(edit(1, &[(A, 2)], &[]));
        shared.push_edit(edit(1, &[(A, 3)], &[]));
        assert_eq!(
            drain(&shared),
            vec![
                sent(1, &[(A, 1)], &[]),
                ControlEvent::Pause,
                sent(1, &[(A, 3)], &[]),
            ]
        );
    }

    /// An edit naming an asset must not merge into an edit queued ahead of the
    /// asset's definition.
    #[test]
    fn an_edit_stays_behind_the_asset_message_it_needs() {
        let assets = ControlEvent::SceneAssets {
            text: "#Stone\ngame::Swatch\n".to_string(),
        };
        let shared = Shared::new();
        shared.push_edit(edit(1, &[(A, 1)], &[]));
        shared.push(to_bytes(&assets).expect("encode"));
        shared.push_edit(edit(1, &[(A, 2)], &[]));
        assert_eq!(
            drain(&shared),
            vec![sent(1, &[(A, 1)], &[]), assets, sent(1, &[(A, 2)], &[])]
        );
    }

    #[test]
    fn an_edit_already_taken_by_the_writer_is_not_merged_into() {
        let shared = Shared::new();
        shared.push_edit(edit(1, &[(A, 1)], &[]));
        shared.push_edit(edit(2, &[(A, 5)], &[]));
        let in_flight = shared.next().expect("the writer takes the first");
        shared.push_edit(edit(1, &[(A, 2)], &[]));
        shared.push_edit(edit(2, &[(A, 6)], &[]));
        assert_eq!(
            jackdaw_pie_protocol::event::from_bytes::<ControlEvent>(&in_flight).expect("a message"),
            sent(1, &[(A, 1)], &[])
        );
        assert_eq!(
            drain(&shared),
            vec![sent(2, &[(A, 6)], &[]), sent(1, &[(A, 2)], &[])]
        );
    }

    #[test]
    fn merging_keeps_the_byte_count_of_what_is_queued() {
        let shared = Shared::new();
        for value in 0..10 {
            shared.push_edit(edit(1, &[(A, value)], &[]));
        }
        let state = shared.lock();
        let held: usize = state.queue.iter().map(|queued| queued.bytes.len()).sum();
        assert_eq!(state.bytes, held);
    }

    #[test]
    fn the_backlog_warning_fires_once_and_rearms_after_draining() {
        let shared = Shared::new();
        let chunk = BACKLOG_WARN_BYTES / 4;
        let mut crossings = 0;
        for _ in 0..6 {
            if shared.push(vec![0; chunk]) {
                crossings += 1;
            }
        }
        assert_eq!(crossings, 1);

        for _ in 0..5 {
            shared.next();
        }
        assert!(!shared.lock().warned, "below half, so re-armed");
        assert!(!shared.push(vec![0; chunk]));
        assert!(shared.push(vec![0; BACKLOG_WARN_BYTES]));
    }
}
