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
