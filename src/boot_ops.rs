//! `JACKDAW_RUN_OP`: run operators once at startup, with no mouse.
//!
//! Everything the editor does goes through an operator, and until now
//! every operator needed a click. That makes half the editor untestable
//! by anything but a human: a CI job cannot scatter a terrain, a bug
//! report cannot be replayed, and an agent capturing evidence has to
//! aim a synthetic pointer at a button.
//!
//! ```text
//! JACKDAW_RUN_OP="terrain.scatter density=1.2 seed=7"
//! JACKDAW_RUN_OP="terrain.scatter seed=1; viewport.screenshot path=/tmp/a.png"
//! ```
//!
//! Same family as [`crate::project::ENV_OPEN_PROJECT`],
//! [`crate::screenshot::ENV_SHOT`] and `JACKDAW_AUTO_OPEN`: read once at
//! startup, acted on after the editor settles. Unlike `JACKDAW_SHOT`
//! this one does **not** exit afterwards -- an operator run is usually
//! setup for something else (a screenshot, a manual look), not the whole
//! job.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_scene_types::PropertyValue;

/// Names operators to run once the editor has settled.
pub const ENV_RUN_OP: &str = "JACKDAW_RUN_OP";

/// Frames to let pass after entering the editor before the first
/// operator runs. Matches [`crate::screenshot`]'s settle count: the
/// panels, the opened scene and the first rendered frame all have to
/// exist before an operator that reads the scene means anything.
const SETTLE_FRAMES: u32 = 90;

/// Frames between one clause and the next.
///
/// Not one per frame: a later clause routinely depends on the earlier
/// one having *finished*, not merely having run. An operator that queues
/// its real work through `Commands` needs a frame boundary, and one that
/// spawns glTF instances needs however long the asset server takes to
/// load and render them before a screenshot of the result means
/// anything. Same settle budget as the first clause, for the same
/// reason.
const GAP_FRAMES: u32 = SETTLE_FRAMES;

pub(crate) fn plugin(app: &mut App) {
    if let Some(spec) = std::env::var_os(ENV_RUN_OP).and_then(|v| v.into_string().ok()) {
        let queue = parse_run_ops(&spec);
        if queue.is_empty() {
            warn!("{ENV_RUN_OP} is set but names no operator: {spec:?}");
        }
        app.insert_resource(BootOpQueue { queue, frames: 0 });
    }
    app.add_systems(
        Update,
        drive_boot_ops.run_if(in_state(crate::AppState::Editor)),
    );
}

/// One parsed `<id> [key=value ...]` clause.
#[derive(Clone, Debug, PartialEq)]
pub struct BootOp {
    pub id: String,
    pub params: Vec<(String, PropertyValue)>,
}

/// The pending startup runs. Absent when [`ENV_RUN_OP`] is unset, which
/// is every interactive launch.
#[derive(Resource)]
struct BootOpQueue {
    queue: Vec<BootOp>,
    frames: u32,
}

/// Parse a `JACKDAW_RUN_OP` value.
///
/// Clauses are separated by `;`, tokens within a clause by whitespace,
/// and the first token is the operator id. A token containing `=` is a
/// parameter; anything else is ignored with a warning rather than
/// failing the boot, because a typo in an environment variable should
/// not cost a whole editor launch.
///
/// Values are typed by what they look like, not by a declaration:
/// `true`/`false` become `Bool`, an integer becomes `Int`, a decimal
/// becomes `Float`, everything else stays `String`. There is no quoting,
/// so a value cannot contain a space -- comma-separated lists (the shape
/// every list parameter in the editor already uses) work, sentences do
/// not.
pub fn parse_run_ops(spec: &str) -> Vec<BootOp> {
    let mut out = Vec::new();
    for clause in spec.split(';') {
        let mut tokens = clause.split_whitespace();
        let Some(id) = tokens.next() else {
            continue;
        };
        let mut params = Vec::new();
        for token in tokens {
            match token.split_once('=') {
                Some((key, value)) if !key.is_empty() => {
                    params.push((key.to_string(), parse_value(value)));
                }
                _ => warn!("{ENV_RUN_OP}: ignoring token {token:?}, expected key=value"),
            }
        }
        out.push(BootOp {
            id: id.to_string(),
            params,
        });
    }
    out
}

/// Type a parameter value by its spelling.
///
/// `i64` is tried before `f64` so `seed=7` arrives as an `Int` and
/// matches `OperatorParameters::as_int`; an operator that wants a float
/// there would read `7` as an int and miss it, which is why every
/// numeric operator parameter in the editor declares one type or the
/// other and callers write `1.0` when they mean a float.
fn parse_value(raw: &str) -> PropertyValue {
    match raw {
        "true" => return PropertyValue::Bool(true),
        "false" => return PropertyValue::Bool(false),
        _ => {}
    }
    if let Ok(int) = raw.parse::<i64>() {
        return PropertyValue::Int(int);
    }
    if let Ok(float) = raw.parse::<f64>() {
        return PropertyValue::Float(float);
    }
    PropertyValue::String(raw.to_string().into())
}

/// Count settle frames, then drain the queue one clause every
/// [`GAP_FRAMES`].
fn drive_boot_ops(world: &mut World) {
    let Some(queue) = world.get_resource::<BootOpQueue>() else {
        return;
    };
    if queue.queue.is_empty() {
        return;
    }
    let frames = queue.frames + 1;
    world.resource_mut::<BootOpQueue>().frames = frames;
    if frames < SETTLE_FRAMES || !(frames - SETTLE_FRAMES).is_multiple_of(GAP_FRAMES) {
        return;
    }

    let op = world.resource_mut::<BootOpQueue>().queue.remove(0);
    let mut call = world.operator(op.id.clone());
    for (key, value) in op.params {
        call = call.param(key, value);
    }
    match call.call() {
        Ok(result) => info!("{ENV_RUN_OP}: {} -> {result:?}", op.id),
        Err(err) => error!("{ENV_RUN_OP}: {} failed: {err}", op.id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn param(key: &str, value: PropertyValue) -> (String, PropertyValue) {
        (key.to_string(), value)
    }

    #[test]
    fn a_bare_operator_id_parses_with_no_parameters() {
        assert_eq!(
            parse_run_ops("terrain.scatter"),
            vec![BootOp {
                id: "terrain.scatter".to_string(),
                params: Vec::new(),
            }]
        );
    }

    #[test]
    fn parameter_values_are_typed_by_their_spelling() {
        let ops = parse_run_ops(
            "terrain.scatter seed=7 density=1.5 random_yaw=true align_to_normal=false \
             terrain=Ground accept=1,2",
        );
        assert_eq!(ops.len(), 1);
        assert_eq!(
            ops[0].params,
            vec![
                param("seed", PropertyValue::Int(7)),
                param("density", PropertyValue::Float(1.5)),
                param("random_yaw", PropertyValue::Bool(true)),
                param("align_to_normal", PropertyValue::Bool(false)),
                param("terrain", PropertyValue::String("Ground".into())),
                param("accept", PropertyValue::String("1,2".into())),
            ]
        );
    }

    #[test]
    fn semicolons_separate_a_sequence_of_runs() {
        let ops = parse_run_ops("terrain.scatter seed=3 ; viewport.screenshot path=/tmp/a.png");
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0].id, "terrain.scatter");
        assert_eq!(ops[0].params, vec![param("seed", PropertyValue::Int(3))]);
        assert_eq!(ops[1].id, "viewport.screenshot");
        assert_eq!(
            ops[1].params,
            vec![param("path", PropertyValue::String("/tmp/a.png".into()))]
        );
    }

    /// A malformed token is skipped rather than failing the whole boot.
    #[test]
    fn a_token_without_an_equals_is_dropped_and_the_rest_survives() {
        let ops = parse_run_ops("terrain.scatter nonsense seed=4");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].params, vec![param("seed", PropertyValue::Int(4))]);
    }

    #[test]
    fn an_empty_or_blank_spec_parses_to_nothing_runnable() {
        assert!(parse_run_ops("").is_empty());
        assert!(parse_run_ops("   ;  ").is_empty());
    }

    /// A negative number still types as an int, so `weight_channel=-1`
    /// reaches the operator as the "off" value it expects.
    #[test]
    fn negative_numbers_stay_numeric() {
        let ops = parse_run_ops("terrain.scatter weight_channel=-1 offset=-0.5");
        assert_eq!(
            ops[0].params,
            vec![
                param("weight_channel", PropertyValue::Int(-1)),
                param("offset", PropertyValue::Float(-0.5)),
            ]
        );
    }

    /// A path with an `=` in it keeps everything after the first one, so
    /// a query-string-ish value survives.
    #[test]
    fn only_the_first_equals_splits_a_parameter() {
        let ops = parse_run_ops("op.id key=a=b");
        assert_eq!(
            ops[0].params,
            vec![param("key", PropertyValue::String("a=b".into()))]
        );
    }
}
