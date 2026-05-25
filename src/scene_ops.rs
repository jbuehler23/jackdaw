//! Scene I/O operators: new / open / save / save as / save selection
//! as prefab / open recent.
//!
//! These wrap the existing free functions in [`crate::scene_io`] so they
//! can be dispatched uniformly through the operator API (menu, keybind,
//! F3 command palette, extension code). BEI bindings for the
//! usual Ctrl+N / Ctrl+O / Ctrl+S / Ctrl+Shift+S keybinds are attached
//! here.

use bevy::prelude::*;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;

use crate::core_extension::CoreExtensionInputContext;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<SceneNewOp>()
        .register_operator::<SceneOpenOp>()
        .register_operator::<SceneSaveOp>()
        .register_operator::<SceneSaveAsOp>()
        .register_operator::<SceneSaveSelectionAsPrefabOp>()
        .register_operator::<SceneOpenRecentOp>();

    let ext = ctx.id();
    ctx.entity_mut().world_scope(|world| {
        world.spawn((
            Action::<SceneNewOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(
                KeyCode::KeyN.with_mod_keys(ModKeys::CONTROL | ModKeys::SHIFT),
                Press::default(),
            )],
        ));
        world.spawn((
            Action::<SceneOpenOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(
                KeyCode::KeyO.with_mod_keys(ModKeys::CONTROL),
                Press::default(),
            )],
        ));
        world.spawn((
            Action::<SceneSaveOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(
                KeyCode::KeyS.with_mod_keys(ModKeys::CONTROL),
                Press::default(),
            )],
        ));
        world.spawn((
            Action::<SceneSaveAsOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(
                KeyCode::KeyS.with_mod_keys(ModKeys::CONTROL | ModKeys::SHIFT),
                Press::default(),
            )],
        ));
    });
}

#[operator(id = "scene.new", label = "New")]
pub(crate) fn scene_new(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        crate::scene_io::new_scene(world);
    });
    OperatorResult::Finished
}

#[operator(id = "scene.open", label = "Open")]
pub fn scene_open(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        crate::scene_io::load_scene(world);
    });
    OperatorResult::Finished
}

#[operator(id = "scene.save", label = "Save", allows_undo = false)]
pub fn scene_save(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        crate::scene_io::save_scene(world);
    });
    OperatorResult::Finished
}

#[operator(id = "scene.save_as", label = "Save As...", allows_undo = false)]
pub fn scene_save_as(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        crate::scene_io::save_scene_as(world);
    });
    OperatorResult::Finished
}

#[operator(
    id = "scene.save_selection_as_prefab",
    label = "Save Selection as Prefab",
    allows_undo = false
)]
pub(crate) fn scene_save_selection_as_prefab(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let selection: Vec<Entity> = world
            .resource::<crate::selection::Selection>()
            .entities
            .clone();
        if selection.is_empty() {
            warn!("scene.save_selection_as_prefab: empty selection");
            return;
        }
        let name = selection
            .first()
            .and_then(|e| world.get::<Name>(*e).map(|n| n.as_str().to_string()))
            .unwrap_or_else(|| "prefab".to_string());
        let target = match world.get_resource::<crate::project::ProjectRoot>() {
            Some(root) => root.root.join("assets/prefabs").join(format!("{name}.jsn")),
            None => std::path::PathBuf::from(format!("{name}.jsn")),
        };
        crate::prefab::operators::save_as_prefab_from_selection(world, &selection, &target);
    });
    OperatorResult::Finished
}

#[operator(id = "scene.open_recent", label = "Open Recent...")]
pub(crate) fn scene_open_recent(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        crate::open_recent_dialog(world);
    });
    OperatorResult::Finished
}
