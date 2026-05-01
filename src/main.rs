use bevy::{
    asset::{AssetPlugin, UnapprovedPathMode},
    ecs::error::ErrorContext,
    image::{ImageAddressMode, ImagePlugin, ImageSamplerDescriptor},
    prelude::*,
};
use jackdaw::prelude::*;

fn main() -> AppExit {
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

    // Auto-open the last project on launch. The launcher's project
    // picker is most useful on first launch (no recent projects to
    // resume) and when explicitly switching projects; for the
    // common case of "I just built my game and want to keep
    // editing", going through it again is a wasted click. If the
    // last-recent path no longer exists on disk (deleted, renamed,
    // moved), or doesn't have a `Cargo.toml`, fall back to the
    // picker so the user isn't stuck on an error modal.
    //
    // To force the picker (e.g., to switch projects from the
    // command line), set `JACKDAW_PICK=1` or remove the most
    // recent entry from `~/.config/jackdaw/recent.json`.
    let respawn_skip_build = std::env::var_os(jackdaw::restart::ENV_SKIP_INITIAL_BUILD).is_some();
    let force_picker = std::env::var_os("JACKDAW_PICK").is_some();
    let auto_open = if force_picker {
        None
    } else if respawn_skip_build {
        // Respawn after a scaffold/install: parent already built,
        // so just transition into the editor without a build step.
        jackdaw::project::read_last_project().map(|path| jackdaw::project_select::PendingAutoOpen {
            path,
            skip_build: true,
        })
    } else {
        // Normal launch: re-open the last project, running through
        // the standard build → install/handoff path. Validates the
        // path so a renamed-out-from-under-us project doesn't lock
        // the user in a broken state.
        jackdaw::project::read_last_project()
            .filter(|p| p.is_dir() && p.join("Cargo.toml").is_file())
            .map(|path| jackdaw::project_select::PendingAutoOpen {
                path,
                skip_build: false,
            })
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
