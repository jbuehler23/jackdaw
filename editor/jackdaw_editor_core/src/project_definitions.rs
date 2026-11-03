//! The asset kinds the open project brings with it.
//!
//! The editor never loads project code, so a kind whose type lives in the
//! game's crates comes from the schema the last build reported: every type the
//! game registers as a reflected asset is one, named after itself. Nothing is
//! declared anywhere, and a kind the editor has compiled in keeps the type it
//! already owns.

use bevy::prelude::*;
use jackdaw_api::prelude::{AssetKind, AssetKinds};

use crate::project_types::ProjectTypes;

/// The kinds the open project's schema brought, so they go when it closes.
#[derive(Resource, Default)]
pub struct ProjectDefinitionKinds(Vec<String>);

/// Register a kind for every asset type the project reported, and unregister
/// the ones it no longer does. A type the editor has compiled in is left to
/// its own kind, which the project never takes over and never takes away.
pub fn register_project_definitions(world: &mut World) {
    let reported: Vec<String> = world
        .get_resource::<ProjectTypes>()
        .map(|types| {
            types
                .assets()
                .map(|schema| schema.type_path.clone())
                .collect()
        })
        .unwrap_or_default();
    let previous = world
        .get_resource::<ProjectDefinitionKinds>()
        .map(|registered| registered.0.clone())
        .unwrap_or_default();

    let mut registered: Vec<String> = Vec::new();
    for type_path in reported {
        let kind = AssetKind::from_schema(&type_path);
        let id = kind.kind.clone();
        let taken = world
            .get_resource::<AssetKinds>()
            .and_then(|kinds| kinds.by_kind(&id))
            .filter(|known| known.type_path != type_path)
            .map(|known| known.type_path.clone());
        if let Some(taken) = taken {
            warn!("{type_path} would be called '{id}', which {taken} already answers to");
            continue;
        }
        world.get_resource_or_init::<AssetKinds>().register(kind);
        let landed = world
            .get_resource::<AssetKinds>()
            .and_then(|kinds| kinds.by_kind(&id))
            .is_some_and(|known| known.type_path == type_path && known.schema_backed());
        if landed {
            registered.push(id);
        }
    }

    for gone in previous.iter().filter(|kind| !registered.contains(kind)) {
        unregister_schema_kind(world, gone);
    }
    world.get_resource_or_init::<ProjectDefinitionKinds>().0 = registered;
}

/// Drop a kind the project brought, leaving alone an id something else has
/// taken over in the meantime.
fn unregister_schema_kind(world: &mut World, kind: &str) {
    let mut kinds = world.get_resource_or_init::<AssetKinds>();
    if kinds.by_kind(kind).is_some_and(AssetKind::schema_backed) {
        kinds.unregister(kind);
    }
}

/// Drop every kind the open project brought, for a project on its way out.
pub fn forget_project_definitions(world: &mut World) {
    let Some(kinds) = world
        .get_resource_mut::<ProjectDefinitionKinds>()
        .map(|mut registered| std::mem::take(&mut registered.0))
    else {
        return;
    };
    for kind in kinds {
        unregister_schema_kind(world, &kind);
    }
}
