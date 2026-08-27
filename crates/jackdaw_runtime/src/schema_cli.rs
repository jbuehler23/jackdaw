//! Answering the editor's "what types do you have?" question from
//! inside the game.
//!
//! The request is an ordinary command line argument: the editor
//! runs it with [`SCHEMA_FLAG`], and [`JackdawPlugin`](crate::JackdawPlugin)
//! answers it, writes the schema to stdout and exits. Games do not need
//! to handle the flag themselves.

#![expect(
    clippy::print_stdout,
    reason = "the schema payload IS this mode's output; the editor reads it off stdout"
)]
#![expect(
    clippy::print_stderr,
    reason = "schema extraction failures must reach the editor via stderr"
)]

use bevy::app::{App, AppExit, PreStartup};
use bevy::ecs::reflect::{AppFunctionRegistry, AppTypeRegistry};
use bevy::ecs::world::World;
use jackdaw_schema::ProjectSchema;

pub use jackdaw_schema::SCHEMA_FLAG;

/// Whether this process was asked to report its schema.
pub fn schema_extraction_requested() -> bool {
    std::env::args().any(|arg| arg == SCHEMA_FLAG)
}

/// This binary's reflected types, read from the link-time inventory
/// alone, as the JSON wire format the editor reads.
///
/// No `App` is involved, so this cannot see registered functions.
pub fn extract_schema_json() -> Result<String, serde_json::Error> {
    let schema = jackdaw_schema::extract_derived_schema();
    serde_json::to_string(&schema)
}

/// The schema of a built app's `World`.
///
/// Reads the app's own type registry rather than the link-time
/// inventory (it is a superset: auto-registered types plus anything the
/// game registered by hand) and the function registry, which exists
/// only on a built `App`. Falls back to the inventory if the world has
/// no type registry.
pub fn extract_schema_from_world(world: &World) -> ProjectSchema {
    let mut schema = match world.get_resource::<AppTypeRegistry>() {
        Some(registry) => jackdaw_schema::extract_from_registry(&registry.read()),
        None => jackdaw_schema::extract_derived_schema(),
    };
    if let Some(functions) = world.get_resource::<AppFunctionRegistry>() {
        schema.functions = jackdaw_schema::extract_functions(&functions.read());
    }
    schema
}

/// Print `schema` on stdout and end the process. Never returns.
fn print_and_exit(schema: &ProjectSchema) -> ! {
    match serde_json::to_string(schema) {
        Ok(json) => {
            println!("{json}");
            std::process::exit(0);
        }
        Err(err) => {
            eprintln!("schema extraction failed: {err}");
            std::process::exit(1);
        }
    }
}

/// Dump and exit once the game's `App` is fully built, when the flag
/// was passed; otherwise leave `app` untouched so startup continues.
///
/// Functions are registered by the game's own plugins, so a dump taken
/// while `JackdawPlugin` is building would miss every plugin added
/// after it. The dump therefore happens in the app runner: `App::run`
/// hands the built app to its runner *before* `finish`, so every
/// plugin's `build` has run, leaving the type and function registries
/// complete, while nothing has opened a window or started a frame.
///
/// Letting the rest of the app build is what buys the function
/// registry, and is also where the risk sits, so two fallbacks cover it:
///
/// - A panic hook prints the link-time inventory schema and exits 0, so
///   a game that panics while building still reports its components and
///   resources. It cannot report functions, since nothing has
///   registered them yet, and it does not help if a plugin *blocks*
///   rather than panics; the driver's deadline covers that.
/// - A `PreStartup` system catches a plugin added after this one
///   replacing the runner (`WinitPlugin` does). Reaching `PreStartup`
///   means the app ran `finish` and `cleanup` and opened a window, which
///   is what the runner swap exists to avoid, and on a machine that
///   cannot open one this system never runs. It turns a hung build into
///   a window flash where a window is possible, and into a non-zero exit
///   (plus the inventory fallback, if the failure is a panic) where it
///   is not.
pub fn extract_schema_and_exit_if_requested(app: &mut App) {
    if !schema_extraction_requested() {
        return;
    }
    install_inventory_fallback_hook();
    app.set_runner(|app| -> AppExit { print_and_exit(&extract_schema_from_world(app.world())) });
    app.add_systems(PreStartup, dump_schema_and_exit);
}

/// The inventory-only schema as one JSON line, or `None` if it will not
/// serialize. This is what a panicking build falls back to.
fn inventory_fallback_json() -> Option<String> {
    extract_schema_json().ok()
}

/// Print the inventory schema and exit 0 if the app panics on its way
/// to the dump.
///
/// Exit 0 is load-bearing: the driver discards stdout entirely when the
/// extractor exits non-zero, so a fallback payload behind a failing
/// status would never be read. The panic itself still reaches stderr
/// through the previous hook, and the editor's schema is only ever as
/// good as the last parseable line on stdout, so a runner dump that
/// lands later supersedes this one.
fn install_inventory_fallback_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        if let Some(json) = inventory_fallback_json() {
            println!("{json}");
        }
        std::process::exit(0);
    }));
}

fn dump_schema_and_exit(world: &World) {
    print_and_exit(&extract_schema_from_world(world));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;

    /// A function a binding could call, registered the way a game
    /// registers its own.
    fn double(value: f32) -> f32 {
        value * 2.0
    }

    #[derive(Event, Reflect)]
    #[reflect(Event, Default)]
    struct Fired {
        entity: Entity,
        amount: f32,
    }

    impl Default for Fired {
        fn default() -> Self {
            Self {
                entity: Entity::PLACEHOLDER,
                amount: 0.0,
            }
        }
    }

    #[test]
    fn a_built_app_reports_its_registered_functions() {
        let mut app = App::new();
        app.register_function_with_name("my_game::double", double);
        let schema = extract_schema_from_world(app.world());
        let found = schema
            .functions
            .iter()
            .find(|f| f.name == "my_game::double")
            .expect("the registered function is reported");
        assert_eq!(found.arg_type_paths, ["f32"]);
        assert_eq!(found.return_type_path, "f32");
    }

    /// The hook's own body cannot run in-process (it ends the
    /// process), so what is pinned here is the payload it prints: a
    /// parseable schema line carrying the types the link-time inventory
    /// knows.
    #[test]
    fn the_panic_fallback_prints_a_populated_schema_line() {
        let line = inventory_fallback_json().expect("the inventory serializes");
        let schema = jackdaw_schema::parse_from_stdout(line.as_bytes())
            .expect("the fallback line is a parseable schema");
        assert!(
            !schema.components.is_empty(),
            "the inventory must carry components, or the fallback is worthless"
        );
        assert!(
            schema.functions.is_empty(),
            "nothing has registered a function this early; the dump must not imply otherwise"
        );
    }

    #[test]
    fn a_built_app_reports_its_events() {
        let mut app = App::new();
        app.register_type::<Fired>();
        let schema = extract_schema_from_world(app.world());
        let event = schema
            .events
            .iter()
            .find(|e| e.short_name == "Fired")
            .expect("the registered event is reported");
        assert_eq!(event.entity_fields, vec!["entity".to_string()]);
        assert!(event.fills_gaps);
        assert!(event.fields.iter().any(|f| f.name == "amount"));
    }
}
