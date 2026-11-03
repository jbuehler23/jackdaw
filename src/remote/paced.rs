//! `jackdaw/paced_batch`: a batch played one call at a time with a pause
//! between calls, so a viewer sees it build. It is the same batch as
//! `jackdaw/batch` (one undo entry, references, atomic, stages, checks), held
//! open across frames.

use std::time::{Duration, Instant};

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::remote::{BrpError, BrpResult};
use jackdaw_api_internal::operator::{BatchTransaction, HistoryHold};
use serde_json::Value;

use super::server::{BatchPlan, BatchRunner, check_enabled, invalid_params};

/// The longest pause a paced batch may ask for between calls.
pub const MAX_PACE_MS: u64 = 2000;

/// How many frames an answer is kept for a caller that has not collected it,
/// and how long a collected one is remembered so the poll that follows it is
/// not read as a new batch. Counted from when it was made or sent, never
/// renewed by polling.
const KEEP_FRAMES: u32 = 600;

/// Why the history is held while a paced batch plays.
const HOLD_REASON: &str = "a paced batch is playing; its undo entry stays open until it ends";

/// The paced batch playing, if any, and the answers of those that ended.
#[derive(Resource, Default)]
pub struct PacedBatches {
    playing: Option<PacedRun>,
    answers: HashMap<String, PacedAnswer>,
}

impl PacedBatches {
    /// Whether a paced batch is still playing.
    pub fn is_playing(&self) -> bool {
        self.playing.is_some()
    }
}

/// Refuse a remote call that would change the scene while a paced batch
/// plays: anything it did would land inside the batch's undo entry.
pub(super) fn refuse_while_playing(world: &World, method: &str) -> Result<(), BrpError> {
    if world
        .get_resource::<PacedBatches>()
        .is_some_and(PacedBatches::is_playing)
    {
        return Err(invalid_params(format!(
            "{method} is refused while a paced batch plays, since it would join the batch's \
             undo entry; `wait` with `until: \"idle\"` holds until the batch ends"
        )));
    }
    Ok(())
}

struct PacedRun {
    request: String,
    plan: BatchPlan,
    runner: BatchRunner,
    transaction: BatchTransaction,
    pace: Duration,
    next_at: Instant,
}

enum PacedAnswer {
    Ready { answer: BrpResult, frame: u32 },
    Sent { frame: u32 },
}

fn frame_now(world: &World) -> u32 {
    world
        .get_resource::<bevy::diagnostic::FrameCount>()
        .map_or(0, |count| count.0)
}

/// Start a paced batch, then answer once its last call has run.
///
/// Takes everything `jackdaw/batch` takes but `dry_run`, plus `pace_ms` (the
/// pause between calls, 0 to [`MAX_PACE_MS`]) and a `request` id the caller
/// keeps for the batch's whole run. One paced batch plays at a time.
pub fn paced_batch_handler(
    In(params): In<Option<Value>>,
    world: &mut World,
) -> BrpResult<Option<Value>> {
    check_enabled(world)?;
    let params = params.ok_or_else(|| invalid_params("expected {\"calls\": [...]}"))?;
    let Some(request) = params.get("request").and_then(Value::as_str) else {
        return Err(invalid_params(
            "a paced batch needs a `request` id, so a repeated poll is not read as a second batch",
        ));
    };
    let request = request.to_string();
    let frame = frame_now(world);
    let mut paced = world.get_resource_or_init::<PacedBatches>();
    if let Some(answer) = paced.answers.get_mut(&request) {
        // The poll the HTTP layer makes after an answer goes out lands on the
        // `Sent` arm too; its error reaches no one.
        return match std::mem::replace(answer, PacedAnswer::Sent { frame }) {
            PacedAnswer::Ready { answer, .. } => answer.map(Some),
            PacedAnswer::Sent { frame: sent } => {
                *answer = PacedAnswer::Sent { frame: sent };
                Err(invalid_params(format!(
                    "request `{request}` was already answered; give each paced batch its own id"
                )))
            }
        };
    }
    match &paced.playing {
        Some(run) if run.request == request => return Ok(None),
        Some(_) => {
            return Err(invalid_params(
                "another paced batch is still playing; wait for it to end",
            ));
        }
        None => {}
    }

    let pace = pace_param(&params)?;
    if params.get("dry_run") == Some(&Value::Bool(true)) {
        return Err(invalid_params(
            "`dry_run` cannot be paced: a rehearsal is taken back before anyone sees it",
        ));
    }
    let (plan, prepared) = BatchPlan::from_params(world, &params)?;
    let transaction = BatchTransaction::begin(world, plan.label.clone(), plan.atomic);
    world
        .get_resource_or_init::<HistoryHold>()
        .hold(HOLD_REASON);
    let runner = BatchRunner::start(world, prepared, plan.stage.as_ref());
    world.resource_mut::<PacedBatches>().playing = Some(PacedRun {
        request,
        plan,
        runner,
        transaction,
        pace,
        next_at: Instant::now(),
    });
    Ok(None)
}

fn pace_param(params: &Value) -> Result<Duration, BrpError> {
    let pace = params.get("pace_ms").unwrap_or(&Value::Null);
    let millis = pace.as_u64().ok_or_else(|| {
        invalid_params(format!(
            "`pace_ms` = {pace} is not a whole number of milliseconds"
        ))
    })?;
    if millis > MAX_PACE_MS {
        return Err(invalid_params(format!(
            "`pace_ms` = {millis} is over the {MAX_PACE_MS} ms limit"
        )));
    }
    Ok(Duration::from_millis(millis))
}

/// Run the playing batch's next call once its pause is over, and close the
/// batch after its last. Runs whether or not the caller is still polling, so
/// a caller that leaves does not leave the history span open.
pub fn drive_paced_batch(world: &mut World) {
    let frame = frame_now(world);
    let Some(mut paced) = world.get_resource_mut::<PacedBatches>() else {
        return;
    };
    paced.answers.retain(|_, answer| {
        let (PacedAnswer::Ready { frame: at, .. } | PacedAnswer::Sent { frame: at }) = answer;
        frame.saturating_sub(*at) < KEEP_FRAMES
    });
    if paced
        .playing
        .as_ref()
        .is_none_or(|run| Instant::now() < run.next_at)
    {
        return;
    }
    let Some(mut run) = paced.playing.take() else {
        return;
    };
    run.runner.step(world);
    if !run.runner.is_done() {
        run.next_at = Instant::now() + run.pace;
        world.resource_mut::<PacedBatches>().playing = Some(run);
        return;
    }
    let PacedRun {
        request,
        plan,
        runner,
        transaction,
        ..
    } = run;
    let done = runner.finish(world, &plan.checks);
    let keep = plan.keeps(&done);
    world.get_resource_or_init::<HistoryHold>().release();
    transaction.finish(world, keep);
    let answer = plan.answer(done);
    world
        .resource_mut::<PacedBatches>()
        .answers
        .insert(request, PacedAnswer::Ready { answer, frame });
}
