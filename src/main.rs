use bevy::{
    asset::{AssetPlugin, UnapprovedPathMode},
    ecs::error::ErrorContext,
    image::{ImageAddressMode, ImagePlugin, ImageSamplerDescriptor},
    prelude::*,
};
use jackdaw::prelude::*;

fn main() -> AppExit {
    // CLI mode: `jackdaw <op-id> '<json-params>'` runs an operator
    // headlessly and exits. Detected before the GUI runner sets up
    // anything heavy. The first positional arg containing a `.` is
    // treated as an operator id (operator ids are dotted strings
    // like `project.new`). Anything else falls through to the GUI.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(op_id) = args.first()
        && op_id.contains('.')
    {
        let params_json = args.get(1).map_or("{}", String::as_str);
        return run_headless_operator(op_id, params_json);
    }

    // Install a SIGINT/SIGTERM handler before anything else gets a
    // chance to. Something in the dep tree (wgpu, gilrs, or one of
    // their transitive deps) installs its own `ctrlc` handler that
    // swallows the signal without propagating an exit intent; so
    // by default Ctrl+C in the terminal is a no-op for jackdaw.
    // Claiming the handler first with `std::process::exit(130)`
    // guarantees Ctrl+C actually kills the process.
    //
    // Error ignored: if another handler has already been claimed by
    // the time this runs, that's what bevy also reports ("Skipping
    // installing Ctrl+C handler as one was already installed"),
    // and we can't do anything about it from here.
    let _ = ctrlc::set_handler(|| {
        error!("jackdaw: received Ctrl+C, exiting");
        std::process::exit(130);
    });

    let project_root = jackdaw::project::read_last_project()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // If the parent process respawned us after scaffolding or
    // installing a game, skip the launcher entirely and jump back
    // to wherever the user was. The parent already built +
    // installed the dylib; the startup loader will pick it up
    // normally, so we don't need to rebuild.
    let respawn_skip_build = std::env::var_os(jackdaw::restart::ENV_SKIP_INITIAL_BUILD).is_some();
    let auto_open = if respawn_skip_build {
        jackdaw::project::read_last_project().map(|path| jackdaw::project_select::PendingAutoOpen {
            path,
            skip_build: true,
        })
    } else {
        None
    };

    let mut app = App::new();
    app
        // The default error handler panics, which we never *ever*
        // want to happen to the editor. Log an error instead.
        .set_error_handler(error_handler)
        .add_plugins(
            DefaultPlugins
                .set(AssetPlugin {
                    file_path: project_root.join("assets").to_string_lossy().to_string(),
                    unapproved_path_mode: UnapprovedPathMode::Allow,
                    ..default()
                })
                .set(ImagePlugin {
                    default_sampler: ImageSamplerDescriptor {
                        address_mode_u: ImageAddressMode::Repeat,
                        address_mode_v: ImageAddressMode::Repeat,
                        address_mode_w: ImageAddressMode::Repeat,
                        ..ImageSamplerDescriptor::linear()
                    },
                }),
        )
        .add_plugins(editor_plugins)
        .add_systems(OnEnter(jackdaw::AppState::Editor), spawn_scene);

    if let Some(pending) = auto_open {
        app.insert_resource(pending);
    }

    app.run()
}

/// Build the editor plugin for the prebuilt `jackdaw` binary.
///
/// The dylib loader is always on so users who drop extension `.so`/
/// `.dll`/`.dylib` files into their config directory don't need to
/// rebuild the editor. The in-tree example extensions in
/// `examples/*` are workspace members built as standalone cdylibs ;
/// point the loader at their build output if you want to exercise
/// them, rather than bundling them statically into the editor
/// binary.
fn editor_plugins(app: &mut App) {
    app.add_plugins(EditorPlugins::default());
}

fn spawn_scene(mut commands: Commands) {
    commands.queue(|world: &mut World| {
        jackdaw::scene_io::spawn_default_lighting(world);
    });
}

#[track_caller]
#[inline]
fn error_handler(error: BevyError, ctx: ErrorContext) {
    let msg = format!("{error}");
    if msg.contains("Note that interacting with a despawned entity is the most common cause of this error but there are others") {
        // TODO: Ideally these should not happen. But as-is, we get a lot of them and they are benign, so let's not flood the logs
        bevy::ecs::error::debug(error, ctx);
        return;
    }
    bevy::ecs::error::error(error, ctx);
}

/// Boot a minimal `App` (no windowing, no rendering), register the
/// jackdaw operator catalog, dispatch the requested operator with
/// the JSON-decoded params, then exit.
///
/// Used by CI for the template roundtrip (`jackdaw project.new
/// '{...}'`) and by power users who want to script the editor from
/// the shell. Returns an `AppExit` whose code is 0 on
/// `OperatorResult::Finished` and 1 on any failure path.
///
/// Uses `eprintln!` for diagnostics so output lands on stderr
/// regardless of whether `LogPlugin` is configured for the headless
/// build.
#[expect(
    clippy::print_stderr,
    reason = "CLI mode is a stderr-driven shell tool"
)]
fn run_headless_operator(op_id: &str, params_json: &str) -> AppExit {
    use jackdaw::project_ops; // keep the operator's static-init reachable
    let _ = &project_ops::ProjectNewOp; // referencing the type ensures the operator's `register_*` SystemIds are wired

    let params = match parse_params_json(params_json) {
        Ok(p) => p,
        Err(err) => {
            eprintln!("jackdaw: failed to parse params JSON: {err}");
            return AppExit::error();
        }
    };

    // Headless app: no winit, no render, no UI plugins. We just need
    // the editor's operator catalog wired up so dispatch finds the
    // requested id.
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(EditorPlugins::default());

    // `App::run` would block forever in the standard runner because
    // the editor's GUI plugins set up an event loop. For headless
    // mode we want one update tick (extension registration runs in
    // PostStartup), then the operator dispatch, then exit.
    app.update();

    let world = app.world_mut();
    let mut builder = world.operator(op_id.to_string());
    for (k, v) in params.0 {
        builder = builder.param(k, v);
    }
    let result = builder.call();

    // Run one more update tick so any commands the operator queued
    // actually execute against the world before we exit. (Operators
    // typically use `commands.queue(...)` for the heavy lifting.)
    app.update();

    match result {
        Ok(jackdaw_api::op::OperatorResult::Finished)
        | Ok(jackdaw_api::op::OperatorResult::Running) => AppExit::Success,
        Ok(jackdaw_api::op::OperatorResult::Cancelled) => {
            eprintln!("jackdaw: operator `{op_id}` returned Cancelled");
            AppExit::error()
        }
        Err(err) => {
            eprintln!("jackdaw: operator dispatch failed: {err:?}");
            AppExit::error()
        }
    }
}

/// Parse a JSON object into [`OperatorParameters`]. Supports the
/// subset of `PropertyValue` types the CLI realistically needs:
/// strings, numbers, booleans. Vec / colour / entity values aren't
/// expressible in JSON in a stable form yet; reject those with a
/// clear error.
fn parse_params_json(s: &str) -> Result<jackdaw_api::op::OperatorParameters, String> {
    use jackdaw_jsn::PropertyValue;
    use serde_json::Value;
    use std::collections::BTreeMap;

    let v: Value = serde_json::from_str(s).map_err(|e| e.to_string())?;
    let Value::Object(map) = v else {
        return Err("top-level must be a JSON object".to_string());
    };

    let mut out: BTreeMap<String, PropertyValue> = BTreeMap::new();
    for (k, v) in map {
        let pv = match v {
            Value::Bool(b) => PropertyValue::Bool(b),
            Value::String(s) => PropertyValue::String(s.into()),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    PropertyValue::Int(i)
                } else if let Some(f) = n.as_f64() {
                    PropertyValue::Float(f)
                } else {
                    return Err(format!("`{k}`: number out of supported range"));
                }
            }
            other => {
                return Err(format!(
                    "`{k}`: unsupported JSON type {other:?} (want bool/number/string)"
                ));
            }
        };
        out.insert(k, pv);
    }
    Ok(jackdaw_api::op::OperatorParameters(out))
}
