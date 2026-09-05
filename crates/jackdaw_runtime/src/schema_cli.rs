//! Answering the editor's "what types do you have?" question from
//! inside the game.
//!
//! The editor runs the game with [`SCHEMA_FLAG`], and
//! [`JackdawPlugin`](crate::JackdawPlugin) answers it, writes the schema to
//! stdout and exits.

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

/// This binary's reflected types, read from the link-time inventory alone, as
/// the JSON wire format the editor reads. No `App` is involved, so this cannot
/// see registered functions.
pub fn extract_schema_json() -> Result<String, serde_json::Error> {
    let schema = jackdaw_schema::extract_derived_schema();
    serde_json::to_string(&schema)
}

/// The schema of a built app's `World`.
///
/// Reads the app's own type registry, a superset of the link-time inventory,
/// plus the function registry, which exists only on a built `App`. Falls back
/// to the inventory if the world has no type registry.
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

/// Prints `schema` on stdout and ends the process. Never returns.
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

/// Dumps the schema and exits once the game's `App` is fully built, when the
/// flag was passed; otherwise leaves `app` untouched.
///
/// The dump happens in the app runner, which `App::run` reaches after every
/// plugin's `build` and before `finish`, so the function registry the game's
/// own plugins fill is complete and no window has opened. Two fallbacks cover
/// a build that never gets there: a panic hook printing the inventory-only
/// schema and exiting 0, and a `PreStartup` system for a later plugin that
/// replaces the runner.
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

/// Prints the inventory schema and exits 0 if the app panics on its way to the
/// dump.
///
/// Exit 0 is load-bearing: the driver discards stdout entirely when the
/// extractor exits non-zero. The panic still reaches stderr through the
/// previous hook, and a runner dump landing later supersedes this one.
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

    /// A function a binding could call, registered the way a game registers its
    /// own.
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

    /// The hook's own body ends the process, so what is pinned here is the
    /// payload it prints.
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
