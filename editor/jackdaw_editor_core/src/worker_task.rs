//! Work on a pool thread whose panic stays there.
//!
//! The task pool re-raises a future's panic on whichever thread polls its task,
//! fallible or not, so the result comes back over a channel and the task is never
//! polled. A panic drops the sender, which is how the owner finds out.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Mutex, PoisonError};

use bevy::tasks::{Task, TaskPool};

/// What became of a [`WorkerTask`].
pub(crate) enum WorkerOutcome<T> {
    Running,
    Done(T),
    Panicked,
}

/// Work on a pool thread. Dropped, it cancels work that has not started.
pub(crate) struct WorkerTask<T> {
    outcome: Mutex<Receiver<T>>,
    _task: Option<Task<()>>,
}

impl<T: Send + 'static> WorkerTask<T> {
    pub(crate) fn spawn(pool: &TaskPool, work: impl FnOnce() -> T + Send + 'static) -> Self {
        let (sender, outcome) = mpsc::channel();
        let task = pool.spawn(async move {
            sender.send(work()).ok();
        });
        Self {
            outcome: Mutex::new(outcome),
            _task: Some(task),
        }
    }

    /// A task whose work is already done.
    pub(crate) fn done(value: T) -> Self {
        let (sender, outcome) = mpsc::channel();
        sender.send(value).ok();
        Self {
            outcome: Mutex::new(outcome),
            _task: None,
        }
    }

    pub(crate) fn outcome(&mut self) -> WorkerOutcome<T> {
        let outcome = self
            .outcome
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        match outcome.try_recv() {
            Ok(value) => WorkerOutcome::Done(value),
            Err(TryRecvError::Empty) => WorkerOutcome::Running,
            Err(TryRecvError::Disconnected) => WorkerOutcome::Panicked,
        }
    }
}
