//! Generic BRP poller shared by the debugger panels. A panel adds
//! `BrpPollPlugin::<R>::new(method, interval)` for its target resource `R` and
//! then reads `Option<Res<R>>`; the poller calls one BRP method on the shared
//! game connection at a fixed cadence and writes the deserialized reply into
//! `R`. Method-agnostic: nothing here names a specific method or resource.

use std::marker::PhantomData;

use bevy::{prelude::*, tasks::Task, tasks::futures_lite::future};
use serde::de::DeserializeOwned;

use super::super::{ConnectionManager, brp};

/// Default poll cadence in seconds for panels that do not need a custom rate.
pub const DEFAULT_POLL_INTERVAL: f32 = 0.5;

/// Per-resource poll settings and interval accumulator.
#[derive(Resource)]
struct PollConfig<R: Send + Sync + 'static> {
    method: &'static str,
    interval: f32,
    elapsed: f32,
    _marker: PhantomData<fn() -> R>,
}

/// In-flight BRP request feeding resource `R`.
#[derive(Resource)]
struct PollTask<R: Send + Sync + 'static> {
    task: Task<Result<serde_json::Value, anyhow::Error>>,
    _marker: PhantomData<fn() -> R>,
}

/// Poll one BRP method on the shared connection and deserialize its reply into
/// the target resource `R`. Registers nothing until it is added; see
/// `BrpPollPlugin`.
///
/// Modeled on `super::super` `heartbeat_system`: at most one request is in
/// flight, the timer advances only between requests, and the whole system is a
/// no-op while disconnected.
pub struct BrpPollPlugin<R> {
    method: &'static str,
    interval: f32,
    _marker: PhantomData<fn() -> R>,
}

impl<R> BrpPollPlugin<R> {
    /// Poll `method` every `interval` seconds and store each reply in `R`.
    pub fn new(method: &'static str, interval: f32) -> Self {
        Self {
            method,
            interval,
            _marker: PhantomData,
        }
    }
}

impl<R> Plugin for BrpPollPlugin<R>
where
    R: Resource + DeserializeOwned,
{
    fn build(&self, app: &mut App) {
        app.insert_resource(PollConfig::<R> {
            method: self.method,
            interval: self.interval,
            elapsed: 0.0,
            _marker: PhantomData,
        })
        .add_systems(Update, poll_system::<R>);
    }
}

fn poll_system<R>(
    mut commands: Commands,
    manager: Res<ConnectionManager>,
    time: Res<Time>,
    mut config: ResMut<PollConfig<R>>,
    task: Option<ResMut<PollTask<R>>>,
) where
    R: Resource + DeserializeOwned,
{
    if !manager.is_connected() {
        return;
    }

    if let Some(mut task) = task {
        if let Some(result) = future::block_on(future::poll_once(&mut task.task)) {
            commands.remove_resource::<PollTask<R>>();
            match result {
                Ok(value) => match serde_json::from_value::<R>(value) {
                    Ok(resource) => commands.insert_resource(resource),
                    Err(e) => warn!("BRP poll {} deserialize failed: {e}", config.method),
                },
                Err(e) => warn!("BRP poll {} failed: {e}", config.method),
            }
        }
        return;
    }

    config.elapsed += time.delta_secs();
    if config.elapsed >= config.interval {
        config.elapsed = 0.0;
        let task = brp::brp_request(&manager.endpoint, config.method, None);
        commands.insert_resource(PollTask::<R> {
            task,
            _marker: PhantomData,
        });
    }
}
