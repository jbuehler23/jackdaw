//! Main crate for the Jackdaw editor.
//! Usage of this crate is meant for headless operation. If you want to interact with the jackdaw API for extensions, use the `jackdaw_api` crate instead.
pub mod add_entity_picker;
pub mod alignment_guides;
pub mod app_ops;
pub mod asset_browser;
pub mod asset_catalog;
pub mod brush;
pub mod brush_drag_ops;
pub mod brush_element_ops;
pub mod build_status;
pub mod builtin_extensions;
pub mod clip_ops;
pub mod command_palette;
pub mod commands;
pub mod custom_properties;
pub mod default_style;
pub mod draw_brush;
pub mod edit_mode_ops;
pub mod entity_ops;
pub mod entity_templates;
pub mod face_grid;
pub mod gizmo_ops;
pub mod gizmos;
pub mod grid_ops;
pub mod hierarchy;
pub mod history_ops;
pub mod inspector;
pub mod keybind_focus;
pub mod keybind_settings;
pub mod keybinds;

use std::{collections::BTreeMap, marker::PhantomData};

pub use inspector::{EditorCategory, EditorDescription, EditorHidden, SkipSerialization};
pub mod core_extension;
pub mod document_ops;
pub mod ext_build;
mod extension_lifecycle;
pub mod extension_resolution;
pub mod extension_watcher;
pub mod extensions_dialog;
pub mod hot_reload;
pub mod layout;
pub mod material_browser;
pub mod material_preview;
pub mod measure_tool;
pub mod modal_transform;
pub mod navmesh;
pub mod new_project;
pub mod operator_tooltip;
pub mod physics_brush_bridge;
pub mod physics_tool;
pub mod pie;
pub mod prefab_picker;
pub mod project;
pub mod project_files;
pub mod project_select;
pub mod reflect_default;
pub mod remote;
pub mod restart;
pub mod scene_io;
pub mod scene_ops;
pub mod sdk_paths;
pub mod selection;
pub mod snapping;
pub mod status_bar;
pub mod terrain;
pub mod transform_ops;
pub mod undo_snapshot;
pub mod view_modes;
pub mod view_ops;
pub mod viewport;
pub mod viewport_overlays;
pub mod viewport_select;
pub mod viewport_util;

use bevy::{
    app::PluginGroupBuilder,
    ecs::system::SystemState,
    feathers::{FeathersPlugins, dark_theme::create_dark_theme, theme::UiTheme},
    input::mouse::{MouseScrollUnit, MouseWheel},
    input_focus::InputDispatchPlugin,
    picking::hover::HoverMap,
    platform::collections::HashMap,
    prelude::*,
};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::{
    ToAnchorId as _,
    lifecycle::{RegisteredMenuEntry, RegisteredWindow},
};
use jackdaw_feathers::dialog::EditorDialog;
use jackdaw_feathers::{EditorFeathersPlugin, button::ButtonOperatorCall};
pub use jackdaw_loader::DylibLoaderPlugin;
use jackdaw_widgets::menu_bar::MenuAction;
use selection::Selection;

/// Everything needed to start using Jackdaw.
pub mod prelude {
    pub use crate::{
        DylibLoaderPlugin, EditorCategory, EditorDescription, EditorHidden, EditorPlugins,
        ExtensionPlugin, SkipSerialization,
    };
    pub use jackdaw_api::prelude::*;

    // Ambient plugins re-exported so binaries can write
    // `add_plugins((PhysicsPlugins::default(), EnhancedInputPlugin))`
    // without direct deps on avian3d / bevy_enhanced_input.
    // Editor plugins assert presence rather than adding, so user
    // code can add the same plugins without conflict.
    pub use avian3d::prelude::PhysicsPlugins;
    pub use bevy_enhanced_input::prelude::EnhancedInputPlugin;
}

/// System set for all editor interaction systems (input handling, viewport clicks,
/// gizmo drags, etc.). Automatically disabled when any dialog is open.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct EditorInteractionSystems;

/// System set for drawing systems. Scheduled in [`PostUpdate`] after all propagation sets.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct JackdawDrawSystems;

/// Run condition: returns `true` when no `EditorDialog` entity exists.
pub fn no_dialog_open(dialogs: Query<(), With<EditorDialog>>) -> bool {
    dialogs.is_empty()
}

#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    #[default]
    ProjectSelect,
    Editor,
}

#[derive(Component, Default)]
pub struct EditorEntity;

/// Marker component for UI overlays that should block viewport camera input
/// (scroll, pan, orbit) while they exist. Add this to any overlay entity
/// (e.g. prefab picker, context menus) to automatically disable camera controls.
#[derive(Component, Default)]
pub struct BlocksCameraInput;

// `EditorHidden` is now defined in `crates/jackdaw_jsn/src/editor_meta.rs`
// alongside `EditorCategory` / `EditorDescription`. It serves both
// roles: as a Bevy `Component` for hiding entities from the hierarchy
// (the original use), and as a `#[reflect(@EditorHidden)]` reflect
// attribute for hiding Component types from the Add Component picker.
// Re-exported via `inspector` module above.

/// Marker component for entities that should not be included in scene serialization.
/// Add this to runtime-generated child entities (brush face meshes, terrain chunks, etc.)
/// that are rebuilt automatically from their parent's component data.
#[derive(Component, Default)]
pub struct NonSerializable;

// `SkipSerialization` is defined in `crates/jackdaw_jsn/src/editor_meta.rs`
// alongside `EditorHidden` so user game crates that only depend on
// `jackdaw_runtime` (the static-template default) can reach it
// without pulling in the full editor crate. Re-exported via
// `inspector` module + `prelude` below.

/// The editor plugin group. Construct with [`EditorPlugins::default`] for the
/// builder, or add the default instance directly with
/// `app.add_plugins(EditorPlugin::default())`.
///
/// The builder lets callers opt out of the built-in extensions and
/// register their own:
///
/// ```ignore
/// App::new()
///     .add_plugins(jackdaw::EditorPlugins::default()
///         .with_extension("my_tool", || Box::new(MyTool))
///         .build())
///     .run();
/// ```
///
/// To drop the built-in feature-area extensions (Scene Tree, Asset
/// Browser, etc.):
///
/// ```ignore
/// App::new()
///     .add_plugins(jackdaw::EditorPlugins::default()
///         .with_builtin_extensions(false)
///         .with_extension("my_tool", || Box::new(MyTool))
///         .build())
///     .run();
/// ```
///
/// To additionally load extensions from disk at startup (dynamic
/// library extensions dropped into the user's config directory):
///
/// ```ignore
/// App::new()
///     .add_plugins(jackdaw::EditorPlugins::default()
///         .with_dylib_loader()
///         .build())
///     .run();
/// ```
pub struct EditorPlugins {
    /// We're reserving a private field so users need to use [`EditorPlugins::default`],
    /// ensuring forward compatibility in case we add fields in the future.
    _pd: PhantomData<()>,
}

impl Default for EditorPlugins {
    fn default() -> Self {
        Self { _pd: PhantomData }
    }
}

impl PluginGroup for EditorPlugins {
    fn build(self) -> PluginGroupBuilder {
        // DylibLoaderPlugin is intentionally NOT in this group. The
        // launcher binary (`jackdaw`) opts in by adding it directly,
        // because the launcher is the sole consumer of the
        // `~/.config/jackdaw/games/` and `~/.config/jackdaw/extensions/`
        // dylib install dirs. Per-project static editor binaries
        // built from the static-game template use EditorPlugins +
        // their own statically-linked plugin; they should NOT scan
        // those install dirs (the dylibs there were built against
        // a different bevy compilation and panic at FFI boundary
        // when loaded).
        PluginGroupBuilder::start::<Self>()
            .add(EditorCorePlugin)
            .add(ExtensionPlugin::default())
    }
}

/// Plugin required for the Jackdaw's core functionality.
#[derive(Default)]
pub struct EditorCorePlugin;

impl Plugin for EditorCorePlugin {
    fn build(&self, app: &mut App) {
        // Disable `InputDispatchPlugin` from FeathersPlugins
        // because `bevy_ui_text_input`'s `TextInputPlugin` adds
        // it unconditionally and panics on duplicates.
        //
        // `EnhancedInputPlugin` is owned by the hosting binary's
        // `main.rs` (launcher + the static template's
        // `editor.rs.template`). Asserting presence here, rather
        // than adding it, lets user `MyGamePlugin`s add the same
        // plugin without a duplicate-plugin panic.
        debug_assert!(
            app.is_plugin_added::<EnhancedInputPlugin>(),
            "EditorCorePlugin requires EnhancedInputPlugin first; \
             add `EnhancedInputPlugin` in main.rs before EditorPlugins."
        );
        app.init_state::<AppState>().add_plugins((
            FeathersPlugins.build().disable::<InputDispatchPlugin>(),
            EditorFeathersPlugin,
        ));
        app.add_plugins((
            jackdaw_jsn::JsnPlugin {
                runtime_mesh_rebuild: false,
            },
            project_select::ProjectSelectPlugin,
            inspector::InspectorPlugin,
            hierarchy::HierarchyPlugin,
            viewport::ViewportPlugin,
            gizmos::TransformGizmosPlugin,
            commands::CommandHistoryPlugin,
            selection::SelectionPlugin,
            entity_ops::EntityOpsPlugin,
            scene_io::SceneIoPlugin,
            asset_browser::AssetBrowserPlugin,
            viewport_select::ViewportSelectPlugin,
            snapping::SnappingPlugin,
        ))
        .add_plugins(keybinds::KeybindsPlugin)
        .add_plugins(keybind_settings::KeybindSettingsPlugin)
        .add_plugins((
            viewport_overlays::ViewportOverlaysPlugin,
            view_modes::ViewModesPlugin,
            status_bar::StatusBarPlugin,
            project_files::ProjectFilesPlugin,
            modal_transform::ModalTransformPlugin,
            custom_properties::CustomPropertiesPlugin,
            entity_templates::EntityTemplatesPlugin,
            brush::BrushPlugin,
            material_preview::MaterialPreviewPlugin,
            undo_snapshot::plugin,
        ))
        .add_plugins((
            material_browser::MaterialBrowserPlugin,
            measure_tool::MeasureToolPlugin,
            draw_brush::DrawBrushPlugin,
            face_grid::FaceGridPlugin,
            alignment_guides::AlignmentGuidesPlugin,
            navmesh::NavmeshPlugin,
            terrain::TerrainPlugin,
            remote::RemoteConnectionPlugin,
        ))
        .add_plugins(jackdaw_avian_integration::PhysicsOverlaysPlugin::<
            selection::Selected,
        >::new())
        .add_plugins(jackdaw_avian_integration::simulation::PhysicsSimulationPlugin)
        .add_plugins(physics_brush_bridge::PhysicsBrushBridgePlugin)
        .add_plugins(physics_tool::PhysicsToolPlugin)
        .add_plugins(operator_tooltip::OperatorTooltipPlugin)
        .add_plugins(jackdaw_node_graph::NodeGraphPlugin)
        .add_plugins(jackdaw_animation::AnimationPlugin)
        .add_plugins(jackdaw_panels::DockPlugin)
        .add_plugins(jackdaw_api_internal::ExtensionLoaderPlugin)
        .add_plugins(extension_watcher::ExtensionWatcherPlugin)
        .add_plugins(extensions_dialog::ExtensionsDialogPlugin)
        .add_plugins(hot_reload::HotReloadPlugin)
        .add_plugins(pie::PiePlugin)
        // Force-exit on `AppExit`: bypass wgpu device cleanup
        // and AsyncComputeTaskPool shutdown that otherwise hang
        // the process after window close. Hosted here so every
        // editor binary (launcher + user `cargo editor`) gets
        // the same shutdown behaviour.
        .add_systems(
            Last,
            |mut events: bevy::ecs::message::MessageReader<AppExit>| {
                if let Some(exit) = events.read().next() {
                    let code = match exit {
                        AppExit::Success => 0,
                        AppExit::Error(c) => c.get() as i32,
                    };
                    std::process::exit(code);
                }
            },
        )
        .add_systems(Startup, (register_workspaces, sync_icon_font))
        .configure_sets(
            Update,
            EditorInteractionSystems
                .run_if(in_state(AppState::Editor))
                .run_if(no_dialog_open),
        )
        .configure_sets(
            PostUpdate,
            JackdawDrawSystems
                .after(bevy::transform::TransformSystems::Propagate)
                .after(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate)
                .run_if(in_state(crate::AppState::Editor)),
        )
        .insert_resource(UiTheme(create_dark_theme()))
        .init_resource::<layout::ActiveDocument>()
        .init_resource::<layout::SceneViewPreset>()
        .init_resource::<asset_catalog::AssetCatalog>()
        .init_resource::<jackdaw_jsn::SceneJsnAst>()
        .init_resource::<MenuBarDirty>()
        // Always available so the Extensions dialog's runtime
        // "Install from file" path can push into it even when
        // `with_dylib_loader()` wasn't called.
        .init_resource::<jackdaw_loader::LoadedDylibs>()
        .add_observer(flag_menu_dirty_on_window_add)
        .add_observer(flag_menu_dirty_on_window_remove)
        .add_observer(flag_menu_dirty_on_menu_entry_add)
        .add_observer(flag_menu_dirty_on_menu_entry_remove)
        .add_systems(
            OnEnter(AppState::Editor),
            (spawn_layout, init_layout, populate_menu).chain(),
        )
        .add_systems(
            Update,
            rebuild_menu_if_dirty.run_if(in_state(AppState::Editor)),
        )
        .add_systems(OnExit(AppState::Editor), cleanup_editor)
        .add_systems(
            Update,
            (
                send_scroll_events,
                layout::update_toolbar_button_variants,
                layout::update_active_document_display,
                layout::update_tab_strip_highlights,
                auto_hide_internal_entities,
                decorate_timeline_tooltips,
                discover_gltf_clips,
                register_animation_entities_in_ast,
                follow_scene_selection_to_clip,
                sync_selected_keyframes_from_selection,
                auto_save_layout_on_change,
            )
                .run_if(in_state(AppState::Editor)),
        )
        .add_systems(Update, keybind_focus::disable_keyboard_input_when_typing)
        .add_observer(on_workspace_changed)
        .add_observer(on_scroll)
        .add_observer(handle_menu_action)
        .add_observer(on_create_clip_for_selection)
        .add_observer(on_create_blend_graph_for_selection)
        .add_observer(on_clip_selector_change)
        .add_observer(on_clip_name_commit)
        .add_observer(on_duration_input_commit)
        .add_observer(on_timeline_keyframe_click);

        app.add_plugins(extension_lifecycle::plugin);
    }
}

pub struct ExtensionPlugin {
    pub user_extensions: Vec<std::sync::Arc<dyn Fn() -> Box<dyn JackdawExtension> + Send + Sync>>,
    pub enable_builtin_extensiosn: bool,
}

impl Default for ExtensionPlugin {
    fn default() -> Self {
        Self {
            user_extensions: Vec::new(),
            enable_builtin_extensiosn: true,
        }
    }
}

impl ExtensionPlugin {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an extension. May be called any number of times.
    pub fn with_extension<T: JackdawExtension + Default>(mut self) -> Self {
        const {
            assert!(size_of::<T>() == 0, "Extension must be a zero-sized type.");
        }
        self.user_extensions
            .push(std::sync::Arc::new(|| Box::new(T::default())));
        self
    }

    /// Control whether Jackdaw's built-in feature-area extensions
    /// (Scene Tree, Asset Browser, Timeline, Terminal, Inspector) are
    /// registered. Defaults to `true`.
    pub fn with_builtin_extensions(mut self, enable: bool) -> Self {
        self.enable_builtin_extensiosn = enable;
        self
    }
}

impl Plugin for ExtensionPlugin {
    fn build(&self, app: &mut App) {
        // Extension registration runs during `build()` so BEI's
        // `finish()` hook sees every context type. Built-ins override
        // `kind()` to `Builtin`; user-supplied extensions default to
        // `Custom`.
        use jackdaw_api_internal::lifecycle::ExtensionAppExt as _;
        if self.enable_builtin_extensiosn {
            app.add_plugins(core_extension::plugin)
                .register_extension::<builtin_extensions::CoreWindowsExtension>()
                .register_extension::<builtin_extensions::ViewportExtension>()
                .register_extension::<builtin_extensions::AssetBrowserExtension>()
                .register_extension::<builtin_extensions::TimelineExtension>()
                .register_extension::<builtin_extensions::TerminalExtension>()
                .register_extension::<builtin_extensions::InspectorExtension>();
        }

        for ctor in &self.user_extensions {
            let ctor = std::sync::Arc::clone(ctor);
            app.register_extension_with(move || (*ctor)());
        }
    }
}

/// Drained once per frame so multiple registrations coalesce into a
/// single menu-bar rebuild.
#[derive(Resource, Default)]
pub struct MenuBarDirty(pub bool);

fn rebuild_menu_if_dirty(world: &mut World) {
    if !world.resource::<MenuBarDirty>().0 {
        return;
    }
    world.resource_mut::<MenuBarDirty>().0 = false;
    if let Err(err) = world.run_system_cached(populate_menu) {
        error!("Failed to rebuild menu: {err:?}");
    }
}

fn flag_menu_dirty_on_window_add(_: On<Add, RegisteredWindow>, mut dirty: ResMut<MenuBarDirty>) {
    dirty.0 = true;
}

fn flag_menu_dirty_on_window_remove(
    _: On<Remove, RegisteredWindow>,
    mut dirty: ResMut<MenuBarDirty>,
) {
    dirty.0 = true;
}

fn flag_menu_dirty_on_menu_entry_add(
    _: On<Add, RegisteredMenuEntry>,
    mut dirty: ResMut<MenuBarDirty>,
) {
    dirty.0 = true;
}

fn flag_menu_dirty_on_menu_entry_remove(
    _: On<Remove, RegisteredMenuEntry>,
    mut dirty: ResMut<MenuBarDirty>,
) {
    dirty.0 = true;
}

/// Auto-hide unnamed child entities (likely Bevy internals like shadow cascades).
/// Skips GLTF descendants so they appear in the hierarchy panel.
fn auto_hide_internal_entities(
    mut commands: Commands,
    new_entities: Query<
        (Entity, Option<&Name>, Option<&ChildOf>),
        (
            Added<Transform>,
            Without<EditorEntity>,
            Without<EditorHidden>,
            Without<brush::BrushFaceEntity>,
        ),
    >,
    parent_query: Query<&ChildOf>,
    gltf_sources: Query<(), With<entity_ops::GltfSource>>,
) {
    for (entity, name, parent) in &new_entities {
        if name.is_none() && parent.is_some() {
            // Skip GLTF descendants, they'll be shown in the hierarchy.
            let mut current = entity;
            let mut is_gltf_descendant = false;
            while let Ok(&ChildOf(p)) = parent_query.get(current) {
                if gltf_sources.contains(p) {
                    is_gltf_descendant = true;
                    break;
                }
                current = p;
            }
            if is_gltf_descendant {
                continue;
            }

            if let Ok(mut ec) = commands.get_entity(entity) {
                ec.insert(EditorHidden);
            }
        }
    }
}

fn spawn_layout(mut commands: Commands, icon_font: Res<jackdaw_feathers::icons::IconFont>) {
    commands.spawn((Camera2d, EditorEntity));
    commands.spawn(layout::editor_layout(&icon_font));
}

/// Spawn a new keyframe clip on the same target as the currently-
/// selected clip and make it the new selection. Backs the
/// [`ClipNewOp`] operator so the timeline header button and any
/// future keybind / menu entry share one path.
fn spawn_new_clip_for_selection(world: &mut World) {
    let Some((target, target_name)) = selected_clip_target_with_name(world) else {
        return;
    };
    let clip = world
        .spawn((
            jackdaw_animation::Clip::default(),
            Name::new(format!("{target_name} Clip")),
            ChildOf(target),
        ))
        .id();
    world.spawn((
        jackdaw_animation::AnimationTrack::new(
            "bevy_transform::components::transform::Transform",
            "translation",
        ),
        Name::new(format!("{target_name} / translation")),
        ChildOf(clip),
    ));
    if let Some(mut selected) = world.get_resource_mut::<jackdaw_animation::SelectedClip>() {
        selected.0 = Some(clip);
    }
    if let Some(mut dirty) = world.get_resource_mut::<jackdaw_animation::TimelineDirty>() {
        dirty.0 = true;
    }
}

/// Spawn a new blend-graph clip on the same target as the currently-
/// selected clip. Backs the [`ClipNewBlendGraphOp`] operator.
fn spawn_new_blend_graph_for_selection(world: &mut World) {
    let Some((target, target_name)) = selected_clip_target_with_name(world) else {
        return;
    };
    let clip = world
        .spawn((
            jackdaw_animation::Clip::default(),
            jackdaw_animation::AnimationBlendGraph,
            jackdaw_node_graph::NodeGraph {
                title: format!("{target_name} Blend Graph"),
            },
            jackdaw_node_graph::GraphCanvasView::default(),
            Name::new(format!("{target_name} Blend Graph")),
            ChildOf(target),
        ))
        .id();
    world.spawn((
        jackdaw_node_graph::GraphNode {
            node_type: "anim.output".into(),
            position: Vec2::new(400.0, 160.0),
        },
        jackdaw_animation::OutputNode,
        Name::new("Output"),
        ChildOf(clip),
    ));
    if let Some(mut selected) = world.get_resource_mut::<jackdaw_animation::SelectedClip>() {
        selected.0 = Some(clip);
    }
    if let Some(mut dirty) = world.get_resource_mut::<jackdaw_animation::TimelineDirty>() {
        dirty.0 = true;
    }
}

/// Look up the parent target entity of the currently-selected clip
/// along with its `Name`. Returns `None` when no clip is selected or
/// the target has no `Name`.
fn selected_clip_target_with_name(world: &World) -> Option<(Entity, String)> {
    let clip_entity = world.resource::<jackdaw_animation::SelectedClip>().0?;
    let target = world.get::<ChildOf>(clip_entity)?.parent();
    let name = world.get::<Name>(target)?;
    Some((target, name.as_str().to_string()))
}

/// Clip selector combobox changed. Maps the selected index to a
/// clip entity and switches `SelectedClip`.
fn on_clip_selector_change(
    event: On<jackdaw_feathers::combobox::ComboBoxChangeEvent>,
    selectors: Query<&jackdaw_animation::TimelineClipSelector>,
    child_of_query: Query<&ChildOf>,
    mut commands: Commands,
) {
    let mut current = event.entity;
    let mut selector = None;
    for _ in 0..6 {
        if let Ok(s) = selectors.get(current) {
            selector = Some(s);
            break;
        }
        let Ok(parent) = child_of_query.get(current) else {
            break;
        };
        current = parent.parent();
    }
    let Some(selector) = selector else {
        return;
    };
    let idx = event.selected;
    let Some(&clip_entity) = selector.sibling_clips.get(idx) else {
        return;
    };
    commands.queue(move |world: &mut World| {
        if let Some(mut selected) = world.get_resource_mut::<jackdaw_animation::SelectedClip>() {
            selected.0 = Some(clip_entity);
        }
        if let Some(mut dirty) = world.get_resource_mut::<jackdaw_animation::TimelineDirty>() {
            dirty.0 = true;
        }
    });
}

/// Observer: when the inline clip-name `text_edit` commits, route the
/// rename through `SetJsnField` on the `Name` component so it
/// participates in undo and round-trips through JSN.
fn on_clip_name_commit(
    event: On<jackdaw_feathers::text_edit::TextEditCommitEvent>,
    name_inputs: Query<&jackdaw_animation::TimelineClipNameInput>,
    child_of_query: Query<&ChildOf>,
    names: Query<&Name>,
    mut commands: Commands,
) {
    let mut current = event.entity;
    let mut clip_entity = None;
    for _ in 0..6 {
        if let Ok(input) = name_inputs.get(current) {
            clip_entity = Some(input.clip);
            break;
        }
        let Ok(parent) = child_of_query.get(current) else {
            break;
        };
        current = parent.parent();
    }
    let Some(clip_entity) = clip_entity else {
        return;
    };
    let new_name = event.text.clone();
    if new_name.is_empty() {
        return;
    }
    let Ok(old_name) = names.get(clip_entity) else {
        return;
    };
    if old_name.as_str() == new_name {
        return;
    }
    commands.queue(move |world: &mut World| {
        if let Some(mut name) = world.get_mut::<Name>(clip_entity) {
            *name = Name::new(new_name);
        }
        if let Some(mut dirty) = world.get_resource_mut::<jackdaw_animation::TimelineDirty>() {
            dirty.0 = true;
        }
    });
}

/// One-shot decorator: when timeline transport / header buttons
/// appear, stamp them with [`ButtonOperatorCall`] so the editor's
/// click-dispatch observer routes them through the operator API and
/// the rich hover tooltip resolves the operator's label / description
/// / signature. Runs every frame but short-circuits via `Added<T>`
/// filters, so it only fires once per button spawn.
fn decorate_timeline_tooltips(
    play: Query<Entity, Added<jackdaw_animation::TimelinePlayButton>>,
    pause: Query<Entity, Added<jackdaw_animation::TimelinePauseButton>>,
    stop: Query<Entity, Added<jackdaw_animation::TimelineStopButton>>,
    new_clip: Query<Entity, Added<jackdaw_animation::TimelineHeaderNewClipButton>>,
    new_blend: Query<Entity, Added<jackdaw_animation::TimelineHeaderNewBlendGraphButton>>,
    mut commands: Commands,
) {
    for e in &play {
        commands
            .entity(e)
            .insert(ButtonOperatorCall::new(ClipPlayOp::ID));
    }
    for e in &pause {
        commands
            .entity(e)
            .insert(ButtonOperatorCall::new(ClipPauseOp::ID));
    }
    for e in &stop {
        commands
            .entity(e)
            .insert(ButtonOperatorCall::new(ClipStopOp::ID));
    }
    for e in &new_clip {
        commands
            .entity(e)
            .insert(ButtonOperatorCall::new(ClipNewOp::ID));
    }
    for e in &new_blend {
        commands
            .entity(e)
            .insert(ButtonOperatorCall::new(ClipNewBlendGraphOp::ID));
    }
}

/// Observer: when the placeholder "Create Blend Graph" button is
/// clicked, spawn a `Clip + AnimationBlendGraph + NodeGraph +
/// GraphCanvasView + Name` entity parented to the primary selection,
/// plus a default `OutputNode` inside it so the canvas has
/// something to connect to. Mirror of
/// [`on_create_clip_for_selection`] for the node-canvas path.
fn on_create_blend_graph_for_selection(
    event: On<jackdaw_feathers::button::ButtonClickEvent>,
    buttons: Query<(), With<jackdaw_animation::TimelineCreateBlendGraphButton>>,
    selection: Res<selection::Selection>,
    names: Query<&Name>,
    mut commands: Commands,
) {
    if !buttons.contains(event.entity) {
        return;
    }
    let Some(&primary) = selection.entities.last() else {
        warn!("Create Blend Graph: no entity selected");
        return;
    };
    let Ok(name) = names.get(primary) else {
        warn!(
            "Create Blend Graph: selected entity has no Name. Give it one in the inspector first"
        );
        return;
    };
    let target_name = name.as_str().to_string();

    commands.queue(move |world: &mut World| {
        // The blend graph clip is BOTH a `Clip` and a `NodeGraph`.
        // The canvas widget consumes the NodeGraph side of that
        // entity, and the timeline dock consumes the Clip side. That
        // means children are GraphNodes + Connections rather than
        // AnimationTracks, but `compile_clips` already skips entities
        // marked with `AnimationBlendGraph`, and `rebuild_timeline`
        // branches on the same marker to spawn a canvas instead of
        // the keyframe strip.
        let clip_entity = world
            .spawn((
                jackdaw_animation::Clip::default(),
                jackdaw_animation::AnimationBlendGraph,
                jackdaw_node_graph::NodeGraph {
                    title: format!("{target_name} Blend Graph"),
                },
                jackdaw_node_graph::GraphCanvasView::default(),
                Name::new(format!("{target_name} Blend Graph")),
                ChildOf(primary),
            ))
            .id();

        // Default Output node so the canvas isn't empty on creation
        // and the user has a clear target to wire their Clip
        // Reference into. Positioned near the top-right so there's
        // room for source nodes to the left.
        world.spawn((
            jackdaw_node_graph::GraphNode {
                node_type: "anim.output".into(),
                position: Vec2::new(400.0, 160.0),
            },
            jackdaw_animation::OutputNode,
            Name::new("Output"),
            ChildOf(clip_entity),
        ));

        if let Some(mut selected) = world.get_resource_mut::<jackdaw_animation::SelectedClip>() {
            selected.0 = Some(clip_entity);
        }
        if let Some(mut dirty) = world.get_resource_mut::<jackdaw_animation::TimelineDirty>() {
            dirty.0 = true;
        }
    });
}

/// Observer: when the placeholder "Create Clip for Selection" button
/// is clicked, spawn a new `Clip` + `Name` + default `AnimationTrack` for
/// the primary selected entity, directly via `SpawnEntity`. The
/// animation crate deliberately exports no custom commands; this is
/// the minimum-wrapping form of "create a clip."
fn on_create_clip_for_selection(
    event: On<jackdaw_feathers::button::ButtonClickEvent>,
    buttons: Query<(), With<jackdaw_animation::TimelineCreateClipButton>>,
    selection: Res<selection::Selection>,
    names: Query<&Name>,
    mut commands: Commands,
) {
    if !buttons.contains(event.entity) {
        return;
    }
    let Some(&primary) = selection.entities.last() else {
        warn!("Create Clip: no entity selected");
        return;
    };
    let Ok(name) = names.get(primary) else {
        warn!("Create Clip: selected entity has no Name. Give it one in the inspector first");
        return;
    };
    let target_name = name.as_str().to_string();

    commands.queue(move |world: &mut World| {
        // Spawn clip entity *as a child of the target*. The clip's
        // position in the hierarchy is what encodes "this animates
        // that": compile/bind/snapshot all walk up from the clip to
        // the parent to find the target. Deletion cascades naturally
        // and renaming the target can't silently break the clip
        // because the target is a live Entity reference, not a
        // String.
        let clip_entity = world
            .spawn((
                jackdaw_animation::Clip::default(),
                Name::new(format!("{target_name} Clip")),
                ChildOf(primary),
            ))
            .id();

        // Default translation track as a child of the clip.
        world.spawn((
            jackdaw_animation::AnimationTrack::new(
                "bevy_transform::components::transform::Transform",
                "translation",
            ),
            Name::new(format!("{target_name} / translation")),
            ChildOf(clip_entity),
        ));

        if let Some(mut selected) = world.get_resource_mut::<jackdaw_animation::SelectedClip>() {
            selected.0 = Some(clip_entity);
        }
        if let Some(mut dirty) = world.get_resource_mut::<jackdaw_animation::TimelineDirty>() {
            dirty.0 = true;
        }
    });
}

/// Keep [`jackdaw_animation::SelectedClip`] in lockstep with the main
/// editor's [`selection::Selection`] resource so the timeline widget
/// shows the clip relevant to whatever the user is currently working
/// with.
///
/// Two cases are actively updated:
/// - **A.** Primary selection is already an animation entity (clip,
///   track, or keyframe): walk up `ChildOf` until we hit the owning
///   `Clip` marker and select that.
/// - **B.** Primary selection is a regular scene entity: find the
///   first `Clip` among its `Children` and select it. Since clips
///   now live parented to their target, this is a structural lookup
///   rather than a name-based scan.
///
/// **Empty selection is deliberately a no-op.** After deleting a
/// keyframe the main `delete_selected` path clears `Selection`; if
/// we also cleared `SelectedClip` here the timeline would bounce to
/// its placeholder after every keyframe delete. The stale case
/// (deleting a brush cascades through `ChildOf` and takes its clip
/// with it) is already handled by `rebuild_timeline`, which falls
/// through to the placeholder when `clips.get(selected.0)` fails.
///
/// Lives here rather than in `jackdaw_animation` because the animation
/// crate must not import the main editor's `Selection` type.
fn follow_scene_selection_to_clip(
    selection: Res<selection::Selection>,
    mut selected_clip: ResMut<jackdaw_animation::SelectedClip>,
    parents: Query<&ChildOf>,
    entity_children: Query<&Children>,
    clip_marker: Query<(), With<jackdaw_animation::Clip>>,
) {
    if !selection.is_changed() {
        return;
    }
    // Empty selection: keep the current clip active so keyframe
    // deletes (which clear `Selection`) don't also reset the
    // timeline's context.
    let Some(&primary) = selection.entities.last() else {
        return;
    };

    // Case A: primary is a clip/track/keyframe; walk up to the clip.
    let mut cursor = primary;
    for _ in 0..8 {
        if clip_marker.contains(cursor) {
            if selected_clip.0 != Some(cursor) {
                selected_clip.0 = Some(cursor);
            }
            return;
        }
        let Ok(parent) = parents.get(cursor) else {
            break;
        };
        cursor = parent.parent();
    }

    // Case B: primary is a regular scene entity; pick the first Clip
    // child under it.
    if let Ok(children) = entity_children.get(primary) {
        for child in children.iter() {
            if clip_marker.contains(child) {
                if selected_clip.0 != Some(child) {
                    selected_clip.0 = Some(child);
                }
                return;
            }
        }
    }

    // Case C: the selected entity is not an animation entity and has
    // no clip children. Clear the active clip so the timeline shows
    // the placeholder with "Create Clip" / "Create Blend Graph".
    // This is distinct from the empty-selection guard at the top:
    // empty selection preserves the clip (so keyframe deletes don't
    // bounce the timeline), but selecting a clipless entity is an
    // explicit context switch.
    selected_clip.0 = None;
}

/// Typed, undo-aware delete command for animation keyframes.
///
/// We don't reuse [`commands::DespawnEntity`] for keyframes because
/// that path round-trips through Bevy's `DynamicScene::write_to_world`,
/// which doesn't play well with entity ID reuse: after despawn,
/// Bevy may reissue the keyframe's slot to a later-spawned entity,
/// and an undo that restores the snapshot at the original ID can
/// end up clobbering whatever is living at that slot now (the user
/// saw this as "Ctrl+Z deletes my brush").
///
/// This command captures the keyframe's fields directly (`time`,
/// `value`, and parent `track`) and on undo spawns a **fresh**
/// entity with those fields parented to the original track. No
/// ID reuse, no `DynamicScene`, no surprises.
enum DespawnKeyframeCmd {
    Vec3 {
        /// Current entity id. Updated after each undo so redo knows
        /// which live entity to despawn.
        keyframe: Entity,
        track: Entity,
        time: f32,
        value: Vec3,
    },
    Quat {
        keyframe: Entity,
        track: Entity,
        time: f32,
        value: Quat,
    },
    F32 {
        keyframe: Entity,
        track: Entity,
        time: f32,
        value: f32,
    },
}

impl jackdaw_commands::EditorCommand for DespawnKeyframeCmd {
    fn execute(&mut self, world: &mut World) {
        let entity = match self {
            Self::Vec3 { keyframe, .. }
            | Self::Quat { keyframe, .. }
            | Self::F32 { keyframe, .. } => *keyframe,
        };
        if let Ok(ent) = world.get_entity_mut(entity) {
            ent.despawn();
        }
    }

    fn undo(&mut self, world: &mut World) {
        let new_id = match self {
            Self::Vec3 {
                track, time, value, ..
            } => world
                .spawn((
                    jackdaw_animation::Vec3Keyframe {
                        time: *time,
                        value: *value,
                    },
                    ChildOf(*track),
                ))
                .id(),
            Self::Quat {
                track, time, value, ..
            } => world
                .spawn((
                    jackdaw_animation::QuatKeyframe {
                        time: *time,
                        value: *value,
                    },
                    ChildOf(*track),
                ))
                .id(),
            Self::F32 {
                track, time, value, ..
            } => world
                .spawn((
                    jackdaw_animation::F32Keyframe {
                        time: *time,
                        value: *value,
                    },
                    ChildOf(*track),
                ))
                .id(),
        };
        match self {
            Self::Vec3 { keyframe, .. }
            | Self::Quat { keyframe, .. }
            | Self::F32 { keyframe, .. } => *keyframe = new_id,
        }
    }

    fn description(&self) -> &str {
        "Delete keyframe"
    }
}

impl DespawnKeyframeCmd {
    /// Try to build a despawn command for `entity`. Returns `None`
    /// if the entity doesn't have any of the known keyframe
    /// component types, so the caller can fall through to a
    /// generic despawn.
    fn try_from_entity(world: &World, entity: Entity) -> Option<Self> {
        let track = world.get::<ChildOf>(entity).map(ChildOf::parent)?;
        if let Some(kf) = world.get::<jackdaw_animation::Vec3Keyframe>(entity) {
            return Some(Self::Vec3 {
                keyframe: entity,
                track,
                time: kf.time,
                value: kf.value,
            });
        }
        if let Some(kf) = world.get::<jackdaw_animation::QuatKeyframe>(entity) {
            return Some(Self::Quat {
                keyframe: entity,
                track,
                time: kf.time,
                value: kf.value,
            });
        }
        if let Some(kf) = world.get::<jackdaw_animation::F32Keyframe>(entity) {
            return Some(Self::F32 {
                keyframe: entity,
                track,
                time: kf.time,
                value: kf.value,
            });
        }
        None
    }
}

/// Interceptor that runs before the entity-delete operator fires and
/// steals the Delete key for any selected keyframe entities.
/// Each keyframe gets wrapped in a [`DespawnKeyframeCmd`], the
/// commands are grouped and pushed onto the history, and the
/// keyframes are removed from [`selection::Selection`] so the
/// downstream generic delete handler ignores them.
///
/// Mixed selections (keyframes + a scene entity) work: this system
/// handles the keyframes, then `handle_entity_keys` handles the
/// remaining non-keyframe entities normally. Both halves land on
/// the history as independent commands, which is fine: undo
/// reverses them in push order.
/// Delete all keyframe entities currently in [`selection::Selection`]
/// as a single undoable group. Strips them from the selection first so
/// any non-keyframe entities still in the selection get processed by
/// the generic [`entity_ops::EntityDeleteOp`] in the same press.
#[operator(
    id = "clip.delete_keyframes",
    label = "Delete Keyframes",
    description = "Remove the selected animation keyframes.",
    is_available = has_selected_keyframes,
)]
pub(crate) fn clip_delete_keyframes(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let selected: Vec<Entity> = world.resource::<selection::Selection>().entities.clone();
        let mut kf_cmds: Vec<Box<dyn jackdaw_commands::EditorCommand>> = Vec::new();
        let mut keyframe_ids: Vec<Entity> = Vec::new();
        for &entity in &selected {
            if let Some(cmd) = DespawnKeyframeCmd::try_from_entity(world, entity) {
                keyframe_ids.push(entity);
                kf_cmds.push(Box::new(cmd));
            }
        }
        if kf_cmds.is_empty() {
            return;
        }
        {
            let mut selection = world.resource_mut::<selection::Selection>();
            selection.entities.retain(|e| !keyframe_ids.contains(e));
        }
        for entity in &keyframe_ids {
            if let Ok(mut ent) = world.get_entity_mut(*entity) {
                ent.remove::<selection::Selected>();
            }
        }
        for cmd in &mut kf_cmds {
            cmd.execute(world);
        }
        let group = commands::CommandGroup {
            commands: kf_cmds,
            label: "Delete keyframes".to_string(),
        };
        let mut history = world.resource_mut::<jackdaw_commands::CommandHistory>();
        history.push_executed(Box::new(group));
    });
    OperatorResult::Finished
}

fn has_selected_keyframes(
    input_focus: Res<bevy::input_focus::InputFocus>,
    selection: Res<selection::Selection>,
    keyframes: Query<
        (),
        bevy::ecs::query::Or<(
            With<jackdaw_animation::Vec3Keyframe>,
            With<jackdaw_animation::QuatKeyframe>,
            With<jackdaw_animation::F32Keyframe>,
        )>,
    >,
) -> bool {
    if input_focus.0.is_some() {
        return false;
    }
    selection.entities.iter().any(|&e| keyframes.contains(e))
}

fn timeline_with_clip(
    input_focus: Res<bevy::input_focus::InputFocus>,
    active: ActiveModalQuery,
    tree: Res<jackdaw_panels::tree::DockTree>,
    selected_clip: Res<jackdaw_animation::SelectedClip>,
) -> bool {
    if input_focus.0.is_some() || active.is_modal_running() {
        return false;
    }
    if !crate::transform_ops::active_tab_kind_present(&tree, "jackdaw.timeline") {
        return false;
    }
    selected_clip.0.is_some()
}

fn timeline_paste_available(
    input_focus: Res<bevy::input_focus::InputFocus>,
    active: ActiveModalQuery,
    tree: Res<jackdaw_panels::tree::DockTree>,
    selected_clip: Res<jackdaw_animation::SelectedClip>,
    clipboard: Res<jackdaw_animation::KeyframeClipboard>,
) -> bool {
    if input_focus.0.is_some() || active.is_modal_running() {
        return false;
    }
    if !crate::transform_ops::active_tab_kind_present(&tree, "jackdaw.timeline") {
        return false;
    }
    selected_clip.0.is_some() && !clipboard.entries.is_empty()
}

/// Step the playhead one ruler tick to the left.
#[operator(
    id = "clip.timeline.step_left",
    label = "Step Left",
    description = "Step the playhead one tick back.",
    is_available = timeline_with_clip,
    allows_undo = false,
)]
pub(crate) fn clip_timeline_step_left(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| step_timeline(world, -1));
    OperatorResult::Finished
}

/// Step the playhead one ruler tick to the right.
#[operator(
    id = "clip.timeline.step_right",
    label = "Step Right",
    description = "Step the playhead one tick forward.",
    is_available = timeline_with_clip,
    allows_undo = false,
)]
pub(crate) fn clip_timeline_step_right(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| step_timeline(world, 1));
    OperatorResult::Finished
}

/// Jump the playhead to the previous keyframe in the selected clip.
#[operator(
    id = "clip.timeline.jump_prev_keyframe",
    label = "Jump To Previous Keyframe",
    description = "Snap the playhead to the previous keyframe.",
    is_available = timeline_with_clip,
    allows_undo = false,
)]
pub(crate) fn clip_timeline_jump_prev(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| jump_to_keyframe(world, false));
    OperatorResult::Finished
}

/// Jump the playhead to the next keyframe in the selected clip.
#[operator(
    id = "clip.timeline.jump_next_keyframe",
    label = "Jump To Next Keyframe",
    description = "Snap the playhead to the next keyframe.",
    is_available = timeline_with_clip,
    allows_undo = false,
)]
pub(crate) fn clip_timeline_jump_next(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| jump_to_keyframe(world, true));
    OperatorResult::Finished
}

/// Move the playhead to the start of the selected clip.
#[operator(
    id = "clip.timeline.jump_start",
    label = "Jump To Start",
    description = "Move the playhead to the start of the clip.",
    is_available = timeline_with_clip,
    allows_undo = false,
)]
pub(crate) fn clip_timeline_jump_start(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        world.write_message(jackdaw_animation::AnimationSeek(0.0));
    });
    OperatorResult::Finished
}

/// Move the playhead to the end of the selected clip.
#[operator(
    id = "clip.timeline.jump_end",
    label = "Jump To End",
    description = "Move the playhead to the end of the clip.",
    is_available = timeline_with_clip,
    allows_undo = false,
)]
pub(crate) fn clip_timeline_jump_end(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let Some(clip_entity) = world.resource::<jackdaw_animation::SelectedClip>().0 else {
            return;
        };
        let Some(clip) = world.get::<jackdaw_animation::Clip>(clip_entity).copied() else {
            return;
        };
        world.write_message(jackdaw_animation::AnimationSeek(clip.duration.max(0.01)));
    });
    OperatorResult::Finished
}

/// Copy the selected keyframes into the animation clipboard so they
/// can be pasted at a different time.
#[operator(
    id = "clip.copy_keyframes",
    label = "Copy Keyframes",
    description = "Copy the selected keyframes to the clipboard.",
    is_available = has_selected_keyframes,
    allows_undo = false,
)]
pub(crate) fn clip_copy_keyframes(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(copy_selected_keyframes);
    OperatorResult::Finished
}

/// Paste the clipboard keyframes onto the selected clip starting at
/// the playhead.
#[operator(
    id = "clip.paste_keyframes",
    label = "Paste Keyframes",
    description = "Paste keyframes from the clipboard at the playhead.",
    is_available = timeline_paste_available,
)]
pub(crate) fn clip_paste_keyframes(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(paste_clipboard_keyframes);
    OperatorResult::Finished
}

/// Start playback on the active clip.
#[operator(
    id = "clip.play",
    label = "Play",
    description = "Start animation playback.",
    allows_undo = false
)]
pub(crate) fn clip_play(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        world.write_message(jackdaw_animation::AnimationPlay);
    });
    OperatorResult::Finished
}

/// Pause playback on the active clip.
#[operator(
    id = "clip.pause",
    label = "Pause",
    description = "Pause animation playback.",
    allows_undo = false
)]
pub(crate) fn clip_pause(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        world.write_message(jackdaw_animation::AnimationPause);
    });
    OperatorResult::Finished
}

/// Stop playback and rewind the playhead to the start of the clip.
#[operator(
    id = "clip.stop",
    label = "Stop",
    description = "Stop playback and rewind the playhead to the start of the clip.",
    allows_undo = false
)]
pub(crate) fn clip_stop(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        world.write_message(jackdaw_animation::AnimationStop);
    });
    OperatorResult::Finished
}

/// Spawn a new keyframe clip on the same target as the currently
/// selected clip, then make it the new selection.
#[operator(
    id = "clip.new",
    label = "New Clip",
    description = "Create a new keyframe clip alongside the currently-selected clip.",
    is_available = clip_new_available,
)]
pub(crate) fn clip_new(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(spawn_new_clip_for_selection);
    OperatorResult::Finished
}

/// Spawn a new blend-graph clip on the same target as the currently
/// selected clip.
#[operator(
    id = "clip.new_blend_graph",
    label = "New Blend Graph",
    description = "Create a new blend-graph clip alongside the currently-selected clip.",
    is_available = clip_new_available,
)]
pub(crate) fn clip_new_blend_graph(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(spawn_new_blend_graph_for_selection);
    OperatorResult::Finished
}

/// Both [`ClipNewOp`] and [`ClipNewBlendGraphOp`] need a currently-
/// selected clip to source the parent target from.
fn clip_new_available(selected_clip: Res<jackdaw_animation::SelectedClip>) -> bool {
    selected_clip.0.is_some()
}

fn step_timeline(world: &mut World, direction: i32) {
    let Some(clip_entity) = world.resource::<jackdaw_animation::SelectedClip>().0 else {
        return;
    };
    let Some(clip) = world.get::<jackdaw_animation::Clip>(clip_entity).copied() else {
        return;
    };
    let duration = clip.duration.max(0.01);
    let current_time = world
        .resource::<jackdaw_animation::TimelineCursor>()
        .seek_time;
    let step = jackdaw_animation::pick_tick_step(duration);
    let new_time = (current_time + direction as f32 * step).clamp(0.0, duration);
    world.write_message(jackdaw_animation::AnimationSeek(new_time));
}

/// Gather every keyframe time on the clip, across all tracks and
/// all typed keyframe components. Used by the shift+arrow "step to
/// adjacent keyframe" path.
fn collect_clip_keyframe_times(world: &World, clip_entity: Entity) -> Vec<f32> {
    let mut times = Vec::new();
    let Some(clip_children) = world.get::<Children>(clip_entity) else {
        return times;
    };
    let track_entities: Vec<Entity> = clip_children.iter().collect();
    for track in track_entities {
        let Some(track_children) = world.get::<Children>(track) else {
            continue;
        };
        for kf in track_children.iter() {
            if let Some(k) = world.get::<jackdaw_animation::Vec3Keyframe>(kf) {
                times.push(k.time);
            } else if let Some(k) = world.get::<jackdaw_animation::QuatKeyframe>(kf) {
                times.push(k.time);
            } else if let Some(k) = world.get::<jackdaw_animation::F32Keyframe>(kf) {
                times.push(k.time);
            }
        }
    }
    times
}

fn jump_to_keyframe(world: &mut World, forward: bool) {
    let Some(clip_entity) = world.resource::<jackdaw_animation::SelectedClip>().0 else {
        return;
    };
    let Some(clip) = world.get::<jackdaw_animation::Clip>(clip_entity).copied() else {
        return;
    };
    let duration = clip.duration.max(0.01);
    let current_time = world
        .resource::<jackdaw_animation::TimelineCursor>()
        .seek_time;
    let times = collect_clip_keyframe_times(world, clip_entity);
    let new_time = if forward {
        times
            .iter()
            .copied()
            .filter(|t| *t > current_time + 1e-4)
            .fold(duration, f32::min)
    } else {
        times
            .iter()
            .copied()
            .filter(|t| *t < current_time - 1e-4)
            .fold(0.0_f32, f32::max)
    };
    world.write_message(jackdaw_animation::AnimationSeek(new_time));
}

fn copy_selected_keyframes(world: &mut World) {
    let selected: Vec<Entity> = world.resource::<selection::Selection>().entities.clone();
    if selected.is_empty() {
        return;
    }
    let mut entries: Vec<(f32, jackdaw_animation::KeyframeClipboardEntry)> = Vec::new();
    for &entity in &selected {
        let Some(track_entity) = world.get::<ChildOf>(entity).map(ChildOf::parent) else {
            continue;
        };
        let Some(track) = world.get::<jackdaw_animation::AnimationTrack>(track_entity) else {
            continue;
        };
        let component_type_path = track.component_type_path.clone();
        let field_path = track.field_path.clone();

        if let Some(kf) = world.get::<jackdaw_animation::Vec3Keyframe>(entity) {
            entries.push((
                kf.time,
                jackdaw_animation::KeyframeClipboardEntry {
                    component_type_path,
                    field_path,
                    relative_time: kf.time,
                    value: jackdaw_animation::KeyframeValue::Vec3(kf.value),
                },
            ));
        } else if let Some(kf) = world.get::<jackdaw_animation::QuatKeyframe>(entity) {
            entries.push((
                kf.time,
                jackdaw_animation::KeyframeClipboardEntry {
                    component_type_path,
                    field_path,
                    relative_time: kf.time,
                    value: jackdaw_animation::KeyframeValue::Quat(kf.value),
                },
            ));
        } else if let Some(kf) = world.get::<jackdaw_animation::F32Keyframe>(entity) {
            entries.push((
                kf.time,
                jackdaw_animation::KeyframeClipboardEntry {
                    component_type_path,
                    field_path,
                    relative_time: kf.time,
                    value: jackdaw_animation::KeyframeValue::F32(kf.value),
                },
            ));
        }
    }
    if entries.is_empty() {
        return;
    }
    let base = entries
        .iter()
        .map(|(t, _)| *t)
        .fold(f32::INFINITY, f32::min);
    let mut normalized: Vec<jackdaw_animation::KeyframeClipboardEntry> = entries
        .into_iter()
        .map(|(_, mut entry)| {
            entry.relative_time -= base;
            entry
        })
        .collect();
    normalized.sort_by(|a, b| a.relative_time.partial_cmp(&b.relative_time).unwrap());
    let count = normalized.len();
    world
        .resource_mut::<jackdaw_animation::KeyframeClipboard>()
        .entries = normalized;
    info!("Copied {count} keyframe(s) to animation clipboard");
}

fn paste_clipboard_keyframes(world: &mut World) {
    let entries = world
        .resource::<jackdaw_animation::KeyframeClipboard>()
        .entries
        .clone();
    if entries.is_empty() {
        return;
    }
    let Some(clip_entity) = world.resource::<jackdaw_animation::SelectedClip>().0 else {
        return;
    };
    let cursor_time = world
        .resource::<jackdaw_animation::TimelineCursor>()
        .seek_time;

    let mut tracks: Vec<(Entity, String, String)> = Vec::new();
    if let Some(children) = world.get::<Children>(clip_entity) {
        for child in children.iter() {
            if let Some(track) = world.get::<jackdaw_animation::AnimationTrack>(child) {
                tracks.push((
                    child,
                    track.component_type_path.clone(),
                    track.field_path.clone(),
                ));
            }
        }
    }

    let mut cmds: Vec<Box<dyn jackdaw_commands::EditorCommand>> = Vec::new();
    let mut max_paste_time = cursor_time;
    for entry in &entries {
        let track_entity = tracks.iter().find_map(|(e, tp, fp)| {
            (tp == &entry.component_type_path && fp == &entry.field_path).then_some(*e)
        });
        let Some(track_entity) = track_entity else {
            warn!(
                "Paste keyframe: no track for {}.{} on selected clip. Add one via the inspector diamond first",
                entry.component_type_path, entry.field_path,
            );
            continue;
        };
        let paste_time = cursor_time + entry.relative_time;
        max_paste_time = max_paste_time.max(paste_time);
        let cmd: Box<dyn jackdaw_commands::EditorCommand> = match entry.value {
            jackdaw_animation::KeyframeValue::Vec3(v) => Box::new(SpawnKeyframeCmd::Vec3 {
                keyframe: None,
                track: track_entity,
                time: paste_time,
                value: v,
            }),
            jackdaw_animation::KeyframeValue::Quat(q) => Box::new(SpawnKeyframeCmd::Quat {
                keyframe: None,
                track: track_entity,
                time: paste_time,
                value: q,
            }),
            jackdaw_animation::KeyframeValue::F32(f) => Box::new(SpawnKeyframeCmd::F32 {
                keyframe: None,
                track: track_entity,
                time: paste_time,
                value: f,
            }),
        };
        cmds.push(cmd);
    }

    if cmds.is_empty() {
        return;
    }

    if let Some(mut clip) = world.get_mut::<jackdaw_animation::Clip>(clip_entity)
        && max_paste_time > clip.duration
    {
        clip.duration = max_paste_time;
    }

    for cmd in &mut cmds {
        cmd.execute(world);
    }
    let count = cmds.len();
    let group = commands::CommandGroup {
        commands: cmds,
        label: "Paste keyframes".to_string(),
    };
    let mut history = world.resource_mut::<jackdaw_commands::CommandHistory>();
    history.push_executed(Box::new(group));

    if let Some(mut dirty) = world.get_resource_mut::<jackdaw_animation::TimelineDirty>() {
        dirty.0 = true;
    }
    info!("Pasted {count} keyframe(s) from animation clipboard");
}

/// Typed, undo-aware spawn command for animation keyframes. Mirror of
/// [`DespawnKeyframeCmd`]: execute spawns a fresh entity with the
/// stored fields parented to the track, undo despawns it. Same ID-
/// reuse avoidance rationale: direct `world.spawn` rather than
/// `DynamicScene`.
///
/// Internal primitive used by the keyframe paste path
/// (`clip.paste_keyframes`). The user-facing entry point for spawning
/// a keyframe at the current playhead time for a selected entity is
/// the `animation.toggle_keyframe` operator (`src/inspector/ops.rs`).
enum SpawnKeyframeCmd {
    Vec3 {
        /// Filled in by `execute`; `None` before the first execute.
        keyframe: Option<Entity>,
        track: Entity,
        time: f32,
        value: Vec3,
    },
    Quat {
        keyframe: Option<Entity>,
        track: Entity,
        time: f32,
        value: Quat,
    },
    F32 {
        keyframe: Option<Entity>,
        track: Entity,
        time: f32,
        value: f32,
    },
}

impl jackdaw_commands::EditorCommand for SpawnKeyframeCmd {
    fn execute(&mut self, world: &mut World) {
        let new_id = match self {
            Self::Vec3 {
                track, time, value, ..
            } => world
                .spawn((
                    jackdaw_animation::Vec3Keyframe {
                        time: *time,
                        value: *value,
                    },
                    ChildOf(*track),
                ))
                .id(),
            Self::Quat {
                track, time, value, ..
            } => world
                .spawn((
                    jackdaw_animation::QuatKeyframe {
                        time: *time,
                        value: *value,
                    },
                    ChildOf(*track),
                ))
                .id(),
            Self::F32 {
                track, time, value, ..
            } => world
                .spawn((
                    jackdaw_animation::F32Keyframe {
                        time: *time,
                        value: *value,
                    },
                    ChildOf(*track),
                ))
                .id(),
        };
        match self {
            Self::Vec3 { keyframe, .. }
            | Self::Quat { keyframe, .. }
            | Self::F32 { keyframe, .. } => *keyframe = Some(new_id),
        }
    }

    fn undo(&mut self, world: &mut World) {
        let entity = match self {
            Self::Vec3 { keyframe, .. }
            | Self::Quat { keyframe, .. }
            | Self::F32 { keyframe, .. } => *keyframe,
        };
        if let Some(entity) = entity
            && let Ok(ent) = world.get_entity_mut(entity)
        {
            ent.despawn();
        }
    }

    fn description(&self) -> &str {
        "Paste keyframe"
    }
}

/// Observer: clicking a timeline keyframe diamond routes through
/// the main editor's [`selection::Selection`] resource. Ctrl+click
/// toggles into the existing selection; plain click replaces with
/// just the keyframe. Delete is then handled by the main editor's
/// existing `delete_selected` path, which wraps despawns in
/// `DespawnEntity` commands for undo safety. The animation crate
/// deliberately does NOT own a delete key handler, so there's no
/// risk of double-delete when the user has both a scene entity and
/// a keyframe "selected."
///
/// Propagation is stopped so the click doesn't also hit the
/// scrubber and seek the playhead.
fn on_timeline_keyframe_click(
    mut event: On<Pointer<Click>>,
    handles: Query<&jackdaw_animation::TimelineKeyframeHandle>,
    keys: Res<ButtonInput<KeyCode>>,
    mut selection: ResMut<selection::Selection>,
    mut commands: Commands,
) {
    let Ok(handle) = handles.get(event.event_target()) else {
        return;
    };
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    if ctrl {
        selection.toggle(&mut commands, handle.keyframe);
    } else {
        selection.select_single(&mut commands, handle.keyframe);
    }
    event.propagate(false);
}

/// Mirror the main [`selection::Selection`] ->the animation crate's
/// [`jackdaw_animation::SelectedKeyframes`] so the timeline
/// highlight system can tell which diamonds to light up without
/// the animation crate needing to import `Selection` itself.
///
/// Runs only when `Selection` changes. Also filters out entities
/// whose keyframe component type isn't one we know about; non-
/// keyframe selections simply don't land in `SelectedKeyframes`.
fn sync_selected_keyframes_from_selection(
    selection: Res<selection::Selection>,
    mut selected_keyframes: ResMut<jackdaw_animation::SelectedKeyframes>,
    vec3_keyframes: Query<(), With<jackdaw_animation::Vec3Keyframe>>,
    quat_keyframes: Query<(), With<jackdaw_animation::QuatKeyframe>>,
    f32_keyframes: Query<(), With<jackdaw_animation::F32Keyframe>>,
) {
    if !selection.is_changed() {
        return;
    }
    selected_keyframes.entities.clear();
    for &entity in &selection.entities {
        if vec3_keyframes.contains(entity)
            || quat_keyframes.contains(entity)
            || f32_keyframes.contains(entity)
        {
            selected_keyframes.entities.insert(entity);
        }
    }
}

/// Observer: when the timeline header's duration field commits,
/// route the edit through `SetJsnField` so it flows through the AST
/// and participates in undo/redo + save/load. This is the hand-off
/// point between the animation crate (which can't import
/// `SetJsnField`) and the editor binary.
fn on_duration_input_commit(
    event: On<jackdaw_feathers::text_edit::TextEditCommitEvent>,
    duration_inputs: Query<&jackdaw_animation::TimelineDurationInput>,
    child_of_query: Query<&ChildOf>,
    clips: Query<&jackdaw_animation::Clip>,
    mut commands: Commands,
) {
    // The commit event fires on the inner text_input entity; the
    // `TimelineDurationInput` marker sits on the wrapper, so walk
    // up one step to find it. Matches the pattern used by
    // `on_material_param_commit` in material_browser.rs.
    let mut current = event.entity;
    let mut marker_clip: Option<Entity> = None;
    for _ in 0..4 {
        if let Ok(marker) = duration_inputs.get(current) {
            marker_clip = Some(marker.clip);
            break;
        }
        let Ok(child_of) = child_of_query.get(current) else {
            break;
        };
        current = child_of.parent();
    }

    let Some(clip_entity) = marker_clip else {
        return;
    };
    let Ok(new_value) = event.text.trim().parse::<f32>() else {
        return;
    };
    let Ok(clip) = clips.get(clip_entity) else {
        return;
    };
    if (new_value - clip.duration).abs() < f32::EPSILON {
        return;
    }
    let old_json = serde_json::json!(clip.duration);
    let new_json = serde_json::json!(new_value);
    commands.queue(move |world: &mut World| {
        let mut history = world
            .remove_resource::<jackdaw_commands::CommandHistory>()
            .unwrap_or_default();
        history.execute(
            Box::new(commands::SetJsnField {
                entity: clip_entity,
                type_path: "jackdaw_animation::clip::Clip".to_string(),
                field_path: "duration".to_string(),
                old_value: old_json,
                new_value: new_json,
                was_derived: false,
            }),
            world,
        );
        world.insert_resource(history);
    });
}

/// After the animation crate spawns new clip/track/keyframe entities,
/// register them in the JSN AST so they participate in save/load and
/// undo/redo snapshotting. Runs every frame; cheap because
/// `register_entity_in_ast` is a no-op for already-registered entities.
fn register_animation_entities_in_ast(
    world: &mut World,
    params: &mut QueryState<
        Entity,
        Or<(
            Added<jackdaw_animation::Clip>,
            Added<jackdaw_animation::AnimationTrack>,
            Added<jackdaw_animation::Vec3Keyframe>,
            Added<jackdaw_animation::QuatKeyframe>,
            Added<jackdaw_animation::F32Keyframe>,
            Added<jackdaw_animation::GltfClipRef>,
            Added<jackdaw_animation::AnimationBlendGraph>,
            Added<jackdaw_node_graph::GraphNode>,
            Added<jackdaw_node_graph::Connection>,
        )>,
    >,
) {
    let entities: Vec<Entity> = params.iter(world).collect();
    for entity in entities {
        scene_io::register_entity_in_ast(world, entity);
    }
}

/// For every [`GltfSource`] entity whose underlying glTF asset is
/// loaded but has not yet had its clips imported, spawn one
/// [`jackdaw_animation::Clip`] + [`jackdaw_animation::GltfClipRef`]
/// child per entry in `Gltf::named_animations`. Those child entities
/// persist through JSN save/load (just two strings each), so this
/// discovery step only needs to run once per glTF in a given session.
///
/// The guard ("skip if any child already has a `GltfClipRef`") keeps
/// us from resurrecting clips the user deleted within the session.
/// Adding new clips to the glTF file externally requires a scene
/// reload to rediscover them, which matches Blender's "reload glTF"
/// semantics.
///
/// Lives in the main crate rather than `jackdaw_animation` because it
/// needs to read `jackdaw_jsn::GltfSource`, and we'd rather not wire a
/// `jackdaw_jsn` dep into the animation crate.
///
/// [`GltfSource`]: jackdaw_jsn::GltfSource
fn discover_gltf_clips(
    sources: Query<(Entity, &jackdaw_jsn::GltfSource, Option<&Children>)>,
    existing_refs: Query<(), With<jackdaw_animation::GltfClipRef>>,
    asset_server: Res<AssetServer>,
    gltfs: Res<Assets<bevy::gltf::Gltf>>,
    mut commands: Commands,
) {
    for (entity, source, children) in &sources {
        // Skip if this GltfSource already has any imported clip
        // children: discovery has run at least once.
        let any_existing = children
            .into_iter()
            .flatten()
            .any(|&c| existing_refs.contains(c));
        if any_existing {
            continue;
        }

        let handle: Handle<bevy::gltf::Gltf> = asset_server.load(&source.path);
        let Some(gltf) = gltfs.get(&handle) else {
            continue;
        };

        for (clip_name, _clip_handle) in &gltf.named_animations {
            let name_str = clip_name.to_string();
            commands.spawn((
                jackdaw_animation::Clip::default(),
                jackdaw_animation::GltfClipRef {
                    gltf_path: source.path.clone(),
                    clip_name: name_str.clone(),
                },
                Name::new(name_str),
                ChildOf(entity),
            ));
        }
    }
}

fn populate_menu(
    world: &mut World,
    menu_bar_entity: &mut SystemState<
        Single<Entity, With<jackdaw_feathers::menu_bar::MenuBarRoot>>,
    >,
    items: &mut QueryState<Entity, With<jackdaw_widgets::menu_bar::MenuBarItem>>,
) {
    let menu_bar_entity = *menu_bar_entity.get(world);

    // Despawn existing menu-bar items before re-populating. Idempotent on
    // first call (nothing to remove), necessary for rebuilds when the
    // window registry changes (extensions toggled on/off).
    let existing: Vec<Entity> = items.iter(world).collect();
    for entity in existing {
        if let Ok(ec) = world.get_entity_mut(entity) {
            ec.despawn();
        }
    }

    // Collect extension-contributed menu entries for menus OTHER than
    // "Add". The "Add" menu goes through the shared
    // `collect_add_menu_items` helper below so the toolbar and the
    // scene-tree picker present identical content.
    let mut ext_menu_entries = HashMap::<_, Vec<(String, String)>>::new();
    {
        let mut q = world.query::<&RegisteredMenuEntry>();
        for entry in q.iter(world) {
            if entry.menu == TopLevelMenu::Add {
                continue;
            }
            ext_menu_entries
                .entry(entry.menu.clone())
                .or_default()
                .push((
                    format!("{OP_PREFIX}{}", entry.operator_id),
                    entry.label.clone(),
                ));
        }
        for entries in ext_menu_entries.values_mut() {
            entries.sort_by(|a, b| a.1.cmp(&b.1));
        }
    }

    // Collect window entries from WindowRegistry grouped by default_area.
    // Built-in windows have a default_area, extension windows don't (empty string).
    let window_registry = world.resource::<jackdaw_panels::WindowRegistry>();
    let mut by_area: std::collections::BTreeMap<String, Vec<(String, String)>> =
        std::collections::BTreeMap::new();
    for descriptor in window_registry.iter() {
        let area_key = if descriptor.default_area.is_empty() {
            "zz_extensions".to_string()
        } else {
            descriptor.default_area.clone()
        };
        by_area.entry(area_key).or_default().push((
            format!("{OP_PREFIX}window.open?window_id={}", descriptor.id),
            descriptor.name.clone(),
        ));
    }
    // Build the Window menu with separators between area groups, followed
    // by Reset Layout at the bottom.
    let mut window_entries: Vec<(String, String)> = Vec::new();
    let area_order = [
        DefaultArea::Left.anchor_id(),
        DefaultArea::Center.anchor_id(),
        DefaultArea::BottomDock.anchor_id(),
        DefaultArea::RightSidebar.anchor_id(),
        "zz_extensions".to_string(),
    ];
    let mut first = true;
    for area in area_order {
        let Some(entries) = by_area.get(&area) else {
            continue;
        };
        if !first {
            window_entries.push(("---".to_string(), String::new()));
        }
        first = false;
        for (id, name) in entries {
            window_entries.push((id.clone(), name.clone()));
        }
    }
    if !window_entries.is_empty() {
        window_entries.push(("---".to_string(), String::new()));
    }
    window_entries.push((
        format!("{OP_PREFIX}window.reset_layout"),
        "Reset Layout".to_string(),
    ));

    // Build the Add menu from the shared helper so the toolbar and the
    // scene-tree Add Entity picker stay in lockstep. Separators are
    // inserted between categories.
    let add_items = add_entity_picker::collect_add_menu_items(world);
    let mut add_menu: Vec<(String, String)> = Vec::with_capacity(add_items.len() + 8);
    let mut last_category: Option<String> = None;
    for item in add_items {
        let name = item.category.name.unwrap_or_else(|| String::from("None"));
        if last_category.as_deref() != Some(name.as_str()) {
            if last_category.is_some() {
                add_menu.push(("---".into(), String::new()));
            }
            last_category = Some(name.clone());
        }
        add_menu.push((item.action, item.label));
    }

    // Current hot-reload state ->reflect in the menu label.
    let hot_reload_on = world
        .get_resource::<hot_reload::HotReloadEnabled>()
        .map(|h| h.0)
        .unwrap_or(false);
    let hot_reload_label = if hot_reload_on {
        "Hot Reload: On"
    } else {
        "Hot Reload: Off"
    };

    let mut menu_items = [
        (
            TopLevelMenu::File,
            vec![
                op_entry::<scene_ops::SceneNewOp>("New"),
                op_entry::<scene_ops::SceneOpenOp>("Open"),
                separator(),
                op_entry::<scene_ops::SceneSaveOp>("Save"),
                op_entry::<scene_ops::SceneSaveAsOp>("Save As..."),
                separator(),
                op_entry::<scene_ops::SceneSaveSelectionAsTemplateOp>("Save Selection as Template"),
                separator(),
                op_entry::<app_ops::AppOpenKeybindsOp>("Keybinds..."),
                op_entry::<app_ops::AppOpenExtensionsOp>("Extensions..."),
                separator(),
                op_entry::<app_ops::AppToggleHotReloadOp>(hot_reload_label),
                op_entry::<scene_ops::SceneOpenRecentOp>("Open Recent..."),
                op_entry::<app_ops::AppGoHomeOp>("Home"),
            ],
        ),
        (
            TopLevelMenu::Edit,
            vec![
                op_entry::<history_ops::HistoryUndoOp>("Undo"),
                op_entry::<history_ops::HistoryRedoOp>("Redo"),
                separator(),
                op_entry::<entity_ops::EntityDeleteOp>("Delete"),
                op_entry::<entity_ops::EntityDuplicateOp>("Duplicate"),
                separator(),
                op_entry::<draw_brush::BrushJoinOp>("Join (Convex Merge)"),
                op_entry::<draw_brush::BrushCsgSubtractOp>("CSG Subtract"),
                op_entry::<draw_brush::BrushCsgIntersectOp>("CSG Intersect"),
                op_entry::<draw_brush::BrushExtendFaceToBrushOp>("Extend to Brush"),
            ],
        ),
        (
            TopLevelMenu::View,
            vec![
                op_entry::<view_ops::ViewToggleWireframeOp>("Toggle Wireframe"),
                op_entry::<view_ops::ViewToggleBoundingBoxesOp>("Toggle Bounding Boxes"),
                op_entry::<view_ops::ViewCycleBoundingBoxModeOp>("Cycle Bounding Box Mode"),
                op_entry::<view_ops::ViewToggleFaceGridOp>("Toggle Face Grid"),
                op_entry::<view_ops::ViewToggleBrushWireframeOp>("Toggle Brush Wireframe"),
                op_entry::<view_ops::ViewToggleBrushOutlineOp>("Toggle Brush Outline"),
                op_entry::<view_ops::ViewToggleAlignmentGuidesOp>("Toggle Alignment Guides"),
                op_entry::<view_ops::ViewToggleColliderGizmosOp>("Toggle Collider Gizmos"),
                op_entry::<view_ops::ViewToggleHierarchyArrowsOp>("Toggle Hierarchy Arrows"),
                op_entry::<view_ops::ViewTogglePerspOrthoOp>("Toggle Perspective / Orthographic"),
                op_entry::<view_ops::ViewFrameSelectedOp>("Frame Selected"),
                op_entry::<view_ops::ViewFrameAllOp>("Frame All"),
            ],
        ),
        (TopLevelMenu::Add, add_menu),
        (TopLevelMenu::Window, window_entries),
    ]
    .map(|(menu, actions)| (menu.order(), [(menu.id(), actions)].into_iter().collect()))
    .into_iter()
    .collect::<BTreeMap<u8, HashMap<String, Vec<(String, String)>>>>();

    for (menu, actions) in ext_menu_entries {
        menu_items
            .entry(menu.order())
            .or_default()
            .entry(menu.id())
            .or_default()
            .extend(actions);
    }
    let menu_items = menu_items.into_values().flatten();

    jackdaw_feathers::menu_bar::populate_menu_bar(world, menu_bar_entity, menu_items);
}

/// Open a registered dock window by id.
#[operator(
    id = "window.open",
    label = "Open Window",
    description = "Open a dock window.",
    allows_undo = false,
    params(window_id(String, doc = "Catalog id of the dock window to open."))
)]
pub(crate) fn window_open(
    params: In<OperatorParameters>,
    registry: Res<jackdaw_panels::WindowRegistry>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    let Some(window_id) = params.as_str("window_id").map(str::to_string) else {
        return OperatorResult::Cancelled;
    };
    // Reject unknown ids up front so callers get `Cancelled` rather
    // than a silent no-op + `Finished`. Lets the menu/tooltip pipeline
    // distinguish "user opened a window" from "user clicked a stale
    // menu entry whose extension unloaded."
    if registry.get(&window_id).is_none() {
        return OperatorResult::Cancelled;
    }
    commands.queue(move |world: &mut World| {
        open_window_in_default_area(world, &window_id);
    });
    OperatorResult::Finished
}

/// Reset the dock layout to its default.
#[operator(
    id = "window.reset_layout",
    label = "Reset Layout",
    description = "Restore the default panel layout.",
    allows_undo = false
)]
pub(crate) fn window_reset_layout(
    _: In<OperatorParameters>,
    mut commands: bevy::prelude::Commands,
) -> OperatorResult {
    commands.queue(reset_layout);
    OperatorResult::Finished
}

/// Build a menu-entry tuple whose action id is the given operator's
/// `ID` wrapped in the feathers `op:` prefix. Keeps operator ids out
/// of UI code; callers pass the `Op` type, not a hand-typed string.
fn op_entry<O: Operator>(label: impl Into<String>) -> (String, String) {
    (format!("op:{}", O::ID), label.into())
}

/// Menu separator row. Feathers renders any `(---, _)` entry as a
/// horizontal divider.
fn separator() -> (String, String) {
    ("---".to_string(), String::new())
}

/// Dispatch `op:`-prefixed [`MenuAction`] events emitted by callers that
/// don't go through feathers' button click path (e.g. the Add Entity
/// picker). The feathers menu bar dispatches op-actions through
/// [`jackdaw_feathers::button::ButtonOperatorCall`] directly and does
/// not fire `MenuAction` for those, so this handler only sees
/// free-standing `op:` events. Always plain `op:OP_ID` form ;
/// parametrised dispatch goes through `ButtonOperatorCall.params`.
fn handle_menu_action(event: On<MenuAction>, mut commands: Commands) {
    let Some(op_id) = event.action.strip_prefix(OP_PREFIX) else {
        return;
    };
    let op_id = op_id.to_string();
    commands.queue(move |world: &mut World| {
        if let Err(err) = world.operator(op_id.clone()).call() {
            error!("operator dispatch failed for `{op_id}`: {err}");
        }
    });
}

/// TODO: This should not exist. All actions should be operators.
/// Remove this once the operatorification is done.
const OP_PREFIX: &str = "op:";

/// Wrap an entity-spawning closure in a `SpawnEntity` command so Ctrl+Z can undo it.
pub(crate) fn spawn_undoable<F>(world: &mut World, label: &str, spawn: F)
where
    F: Fn(&mut World) -> Entity + Send + Sync + 'static,
{
    let mut cmd: Box<dyn jackdaw_commands::EditorCommand> = Box::new(commands::SpawnEntity {
        spawned: None,
        spawn_fn: Box::new(spawn),
        label: label.to_string(),
    });
    cmd.execute(world);
    world
        .resource_mut::<commands::CommandHistory>()
        .push_executed(cmd);
}

fn cleanup_editor(world: &mut World) {
    // 1. Clear scene entities
    scene_io::clear_scene_entities(world);

    // 2. Despawn all EditorEntity entities
    let editor_entities: Vec<Entity> = world
        .query_filtered::<Entity, With<EditorEntity>>()
        .iter(world)
        .collect();
    for entity in editor_entities {
        if let Ok(ec) = world.get_entity_mut(entity) {
            ec.despawn();
        }
    }

    // 3. Despawn Camera2d entities (editor UI camera)
    let cameras: Vec<Entity> = world
        .query_filtered::<Entity, With<Camera2d>>()
        .iter(world)
        .collect();
    for entity in cameras {
        if let Ok(ec) = world.get_entity_mut(entity) {
            ec.despawn();
        }
    }

    // 4. Despawn any open dialogs
    let dialogs: Vec<Entity> = world
        .query_filtered::<Entity, With<jackdaw_feathers::dialog::EditorDialog>>()
        .iter(world)
        .collect();
    for entity in dialogs {
        if let Ok(ec) = world.get_entity_mut(entity) {
            ec.despawn();
        }
    }

    // 5. Reset resources
    world.insert_resource(scene_io::SceneFilePath::default());
    world.insert_resource(scene_io::SceneDirtyState::default());
    world.insert_resource(Selection::default());
    world.insert_resource(commands::CommandHistory::default());

    // 6. Remove project root
    world.remove_resource::<project::ProjectRoot>();

    // 7. Reset menu bar state
    let dropdown_to_despawn = {
        let mut menu_state = world.resource_mut::<jackdaw_widgets::menu_bar::MenuBarState>();
        menu_state.open_menu = None;
        menu_state.dropdown_entity.take()
    };
    if let Some(dropdown) = dropdown_to_despawn
        && let Ok(ec) = world.get_entity_mut(dropdown)
    {
        ec.despawn();
    }
}

pub(crate) fn open_recent_dialog(world: &mut World) {
    let recent = project::read_recent_projects();
    if recent.projects.is_empty() {
        return;
    }

    let mut dialog_event = jackdaw_feathers::dialog::OpenDialogEvent::new("Open Recent", "")
        .without_cancel()
        .with_close_button(true)
        .without_content_padding();
    dialog_event.action = None;
    world.commands().trigger(dialog_event);
    world.flush();

    // Find the DialogChildrenSlot and spawn rows inside it
    let slot_entity = world
        .query_filtered::<Entity, With<jackdaw_feathers::dialog::DialogChildrenSlot>>()
        .iter(world)
        .next();

    let Some(slot_entity) = slot_entity else {
        return;
    };

    let editor_font = world
        .resource::<jackdaw_feathers::icons::EditorFont>()
        .0
        .clone();

    for entry in &recent.projects {
        let path = entry.path.clone();
        let name = entry.name.clone();
        let path_display = entry.path.to_string_lossy().to_string();
        let font = editor_font.clone();

        let row = world
            .commands()
            .spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    width: Val::Percent(100.0),
                    padding: UiRect::all(Val::Px(10.0)),
                    row_gap: Val::Px(2.0),
                    ..Default::default()
                },
                BackgroundColor(jackdaw_feathers::tokens::TOOLBAR_BG),
                children![
                    (
                        Text::new(name),
                        TextFont {
                            font: font.clone(),
                            font_size: jackdaw_feathers::tokens::FONT_LG,
                            ..Default::default()
                        },
                        TextColor(jackdaw_feathers::tokens::TEXT_PRIMARY),
                        Pickable::IGNORE,
                    ),
                    (
                        Text::new(path_display),
                        TextFont {
                            font,
                            font_size: jackdaw_feathers::tokens::FONT_SM,
                            ..Default::default()
                        },
                        TextColor(jackdaw_feathers::tokens::TEXT_SECONDARY),
                        Pickable::IGNORE,
                    ),
                ],
            ))
            .id();

        // Hover effects
        world.commands().entity(row).observe(
            |hover: On<Pointer<Over>>, mut bg: Query<&mut BackgroundColor>| {
                if let Ok(mut bg) = bg.get_mut(hover.event_target()) {
                    bg.0 = jackdaw_feathers::tokens::HOVER_BG;
                }
            },
        );
        world.commands().entity(row).observe(
            |out: On<Pointer<Out>>, mut bg: Query<&mut BackgroundColor>| {
                if let Ok(mut bg) = bg.get_mut(out.event_target()) {
                    bg.0 = jackdaw_feathers::tokens::TOOLBAR_BG;
                }
            },
        );

        // Click: open the project
        world.commands().entity(row).observe(
            move |_: On<Pointer<Click>>, mut commands: Commands| {
                let path = path.clone();
                commands.insert_resource(project_select::PendingAutoOpen {
                    path: path.clone(),
                    skip_build: false,
                });
                commands.trigger(jackdaw_feathers::dialog::CloseDialogEvent);
                commands.queue(move |world: &mut World| {
                    world
                        .resource_mut::<NextState<AppState>>()
                        .set(AppState::ProjectSelect);
                });
            },
        );

        world.commands().entity(slot_entity).add_child(row);
    }

    world.flush();
}

const SCROLL_LINE_HEIGHT: f32 = 21.0;

#[derive(EntityEvent, Debug)]
#[entity_event(propagate, auto_propagate)]
struct Scroll {
    entity: Entity,
    delta: Vec2,
}

fn send_scroll_events(
    mut mouse_wheel: MessageReader<MouseWheel>,
    hover_map: Res<HoverMap>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
) {
    for event in mouse_wheel.read() {
        let mut delta = -Vec2::new(event.x, event.y);
        if event.unit == MouseScrollUnit::Line {
            delta *= SCROLL_LINE_HEIGHT;
        }
        if keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]) {
            std::mem::swap(&mut delta.x, &mut delta.y);
        }
        for pointer_map in hover_map.values() {
            for entity in pointer_map.keys().copied() {
                commands.trigger(Scroll { entity, delta });
            }
        }
    }
}

fn on_scroll(
    mut scroll: On<Scroll>,
    mut query: Query<(&mut ScrollPosition, &Node, &ComputedNode)>,
) {
    let Ok((mut scroll_position, node, computed)) = query.get_mut(scroll.entity) else {
        return;
    };
    let max_offset = (computed.content_size() - computed.size()) * computed.inverse_scale_factor();
    let delta = &mut scroll.delta;

    if node.overflow.x == OverflowAxis::Scroll && delta.x != 0. {
        let at_limit = if delta.x > 0. {
            scroll_position.x >= max_offset.x
        } else {
            scroll_position.x <= 0.
        };
        if !at_limit {
            scroll_position.x += delta.x;
            delta.x = 0.;
        }
    }

    if node.overflow.y == OverflowAxis::Scroll && delta.y != 0. {
        let at_limit = if delta.y > 0. {
            scroll_position.y >= max_offset.y
        } else {
            scroll_position.y <= 0.
        };
        if !at_limit {
            scroll_position.y += delta.y;
            delta.y = 0.;
        }
    }

    if *delta == Vec2::ZERO {
        scroll.propagate(false);
    }
}

fn register_workspaces(mut registry: ResMut<jackdaw_panels::WorkspaceRegistry>) {
    use jackdaw_feathers::icons::Icon;

    registry.register(jackdaw_panels::WorkspaceDescriptor {
        id: "layout".into(),
        name: "Main scene".into(),
        icon: Some(String::from(Icon::File.unicode())),
        accent_color: Color::srgba(0.35, 0.55, 1.0, 0.8),
        layout: jackdaw_panels::LayoutState::default(),
        tree: jackdaw_panels::tree::DockTree::default(),
    });

    registry.register(jackdaw_panels::WorkspaceDescriptor {
        id: "level_design".into(),
        name: "Level Design".into(),
        icon: Some(String::from(Icon::Box.unicode())),
        accent_color: Color::srgba(0.55, 0.85, 0.45, 0.8),
        layout: jackdaw_panels::LayoutState::default(),
        tree: build_level_design_tree(),
    });

    registry.register(jackdaw_panels::WorkspaceDescriptor {
        id: "animation".into(),
        name: "Animation".into(),
        icon: Some(String::from(Icon::Film.unicode())),
        accent_color: Color::srgba(0.85, 0.55, 0.85, 0.8),
        layout: jackdaw_panels::LayoutState::default(),
        tree: build_animation_tree(),
    });

    registry.register(jackdaw_panels::WorkspaceDescriptor {
        id: "debug".into(),
        name: "Schedule Explorer".into(),
        icon: Some(String::from(Icon::CalendarSearch.unicode())),
        accent_color: Color::srgba(0.8, 0.55, 0.35, 0.8),
        layout: jackdaw_panels::LayoutState::default(),
        tree: jackdaw_panels::tree::DockTree::default(),
    });
}

/// Quad-view workspace: one perspective viewport + three orthographic
/// viewports (the user toggles each to top / front / right via Numpad
/// 7 / 1 / 3 once activated). Inspector on the right, hierarchy +
/// project files on the left, asset/timeline/terminal on the bottom.
fn build_level_design_tree() -> jackdaw_panels::tree::DockTree {
    use jackdaw_panels::DockAreaStyle;
    use jackdaw_panels::tree::{DockLeaf, DockNode, DockSplit, DockTree, SplitAxis};

    let mut tree = DockTree::default();

    let left = tree.insert(DockNode::Leaf(
        DockLeaf::new("left", DockAreaStyle::TabBar)
            .with_windows(vec!["jackdaw.hierarchy".into(), "jackdaw.import".into()])
            .persistent(),
    ));
    let project_files = tree.insert(DockNode::Leaf(
        DockLeaf::new("split.jackdaw.project_files.preset", DockAreaStyle::TabBar)
            .with_windows(vec!["jackdaw.project_files".into()]),
    ));
    let left_split = tree.insert(DockNode::Split(DockSplit {
        axis: SplitAxis::Vertical,
        fraction: 0.75,
        a: left,
        b: project_files,
    }));

    // Quad-view: 4 viewport panels in a 2x2 grid. Each gets its own
    // jackdaw.viewport panel (and thus its own camera + render
    // target). The user adjusts each to top/front/right via Numpad
    // shortcuts after activation.
    let vp_persp = tree.insert(DockNode::Leaf(
        DockLeaf::new("center", DockAreaStyle::TabBar)
            .with_windows(vec!["jackdaw.viewport".into()])
            .persistent(),
    ));
    let vp_top = tree.insert(DockNode::Leaf(
        DockLeaf::new("split.jackdaw.viewport.qv_top", DockAreaStyle::TabBar)
            .with_windows(vec!["jackdaw.viewport".into()]),
    ));
    let vp_front = tree.insert(DockNode::Leaf(
        DockLeaf::new("split.jackdaw.viewport.qv_front", DockAreaStyle::TabBar)
            .with_windows(vec!["jackdaw.viewport".into()]),
    ));
    let vp_right = tree.insert(DockNode::Leaf(
        DockLeaf::new("split.jackdaw.viewport.qv_right", DockAreaStyle::TabBar)
            .with_windows(vec!["jackdaw.viewport".into()]),
    ));
    let top_row = tree.insert(DockNode::Split(DockSplit {
        axis: SplitAxis::Horizontal,
        fraction: 0.5,
        a: vp_persp,
        b: vp_top,
    }));
    let bot_row = tree.insert(DockNode::Split(DockSplit {
        axis: SplitAxis::Horizontal,
        fraction: 0.5,
        a: vp_front,
        b: vp_right,
    }));
    let quad = tree.insert(DockNode::Split(DockSplit {
        axis: SplitAxis::Vertical,
        fraction: 0.5,
        a: top_row,
        b: bot_row,
    }));

    let bottom = tree.insert(DockNode::Leaf(
        DockLeaf::new("bottom_dock", DockAreaStyle::IconSidebar)
            .with_windows(vec![
                "jackdaw.assets".into(),
                "jackdaw.timeline".into(),
                "jackdaw.terminal".into(),
            ])
            .persistent(),
    ));
    let center_over_bottom = tree.insert(DockNode::Split(DockSplit {
        axis: SplitAxis::Vertical,
        fraction: 0.75,
        a: quad,
        b: bottom,
    }));

    let right = tree.insert(DockNode::Leaf(
        DockLeaf::new("right_sidebar", DockAreaStyle::TabBar)
            .with_windows(vec![
                "jackdaw.inspector.components".into(),
                "jackdaw.inspector.materials".into(),
                "jackdaw.inspector.resources".into(),
                "jackdaw.inspector.systems".into(),
            ])
            .persistent(),
    ));
    let center_and_right = tree.insert(DockNode::Split(DockSplit {
        axis: SplitAxis::Horizontal,
        fraction: 0.85,
        a: center_over_bottom,
        b: right,
    }));
    let root = tree.insert(DockNode::Split(DockSplit {
        axis: SplitAxis::Horizontal,
        fraction: 0.15,
        a: left_split,
        b: center_and_right,
    }));
    tree.root = Some(root);
    tree
}

/// Stacked viewports for animation work: top viewport renders the
/// camera POV, bottom viewport is the scene/bone manipulation view.
/// Timeline + asset browser docked at the bottom; hierarchy on the
/// left, inspector on the right.
fn build_animation_tree() -> jackdaw_panels::tree::DockTree {
    use jackdaw_panels::DockAreaStyle;
    use jackdaw_panels::tree::{DockLeaf, DockNode, DockSplit, DockTree, SplitAxis};

    let mut tree = DockTree::default();

    let left = tree.insert(DockNode::Leaf(
        DockLeaf::new("left", DockAreaStyle::TabBar)
            .with_windows(vec!["jackdaw.hierarchy".into()])
            .persistent(),
    ));

    let vp_top = tree.insert(DockNode::Leaf(
        DockLeaf::new("center", DockAreaStyle::TabBar)
            .with_windows(vec!["jackdaw.viewport".into()])
            .persistent(),
    ));
    let vp_bot = tree.insert(DockNode::Leaf(
        DockLeaf::new("split.jackdaw.viewport.anim_scene", DockAreaStyle::TabBar)
            .with_windows(vec!["jackdaw.viewport".into()]),
    ));
    let viewports = tree.insert(DockNode::Split(DockSplit {
        axis: SplitAxis::Vertical,
        fraction: 0.5,
        a: vp_top,
        b: vp_bot,
    }));

    let bottom = tree.insert(DockNode::Leaf(
        DockLeaf::new("bottom_dock", DockAreaStyle::IconSidebar)
            .with_windows(vec!["jackdaw.timeline".into(), "jackdaw.assets".into()])
            .persistent(),
    ));
    let center_over_bottom = tree.insert(DockNode::Split(DockSplit {
        axis: SplitAxis::Vertical,
        fraction: 0.7,
        a: viewports,
        b: bottom,
    }));

    let right = tree.insert(DockNode::Leaf(
        DockLeaf::new("right_sidebar", DockAreaStyle::TabBar)
            .with_windows(vec!["jackdaw.inspector.components".into()])
            .persistent(),
    ));
    let center_and_right = tree.insert(DockNode::Split(DockSplit {
        axis: SplitAxis::Horizontal,
        fraction: 0.85,
        a: center_over_bottom,
        b: right,
    }));
    let root = tree.insert(DockNode::Split(DockSplit {
        axis: SplitAxis::Horizontal,
        fraction: 0.15,
        a: left,
        b: center_and_right,
    }));
    tree.root = Some(root);
    tree
}

fn on_workspace_changed(
    trigger: On<jackdaw_panels::WorkspaceChanged>,
    mut active: ResMut<layout::ActiveDocument>,
) {
    let event = trigger.event();
    active.kind = match event.new.as_str() {
        "debug" => layout::TabKind::ScheduleExplorer,
        // "layout", "level_design", "animation", and any future
        // user-created workspace default to the Scene document.
        _ => layout::TabKind::Scene,
    };
}

#[derive(Resource, Default)]
struct LayoutAutoSaveState {
    pending_since: Option<f64>,
}

fn auto_save_layout_on_change(
    mut commands: Commands,
    mut state: Local<LayoutAutoSaveState>,
    time: Res<Time>,
    panels_changed: Query<Entity, Changed<jackdaw_panels::Panel>>,
    active_changed: Query<Entity, Changed<jackdaw_panels::ActiveDockWindow>>,
    area_added: Query<Entity, Added<jackdaw_panels::DockArea>>,
    mut removed: RemovedComponents<jackdaw_panels::DockArea>,
    tree: Res<jackdaw_panels::tree::DockTree>,
    registry: Res<jackdaw_panels::WorkspaceRegistry>,
) {
    let now = time.elapsed_secs_f64();

    let any_change = !panels_changed.is_empty()
        || !active_changed.is_empty()
        || !area_added.is_empty()
        || removed.read().next().is_some()
        || tree.is_changed()
        || registry.is_changed();

    if any_change {
        state.pending_since = Some(now);
    }

    // Debounce: wait 0.5s of no changes before writing.
    if let Some(since) = state.pending_since
        && now - since >= 0.5
    {
        state.pending_since = None;
        commands.queue(|world: &mut World| {
            scene_io::save_layout_to_project(world);
        });
    }
}

/// Build the final `DockTree` (saved or default-split) BEFORE the
/// reconciler materializes any content. This way each window's `build_fn`
/// runs exactly once into its final home with no rebuild churn, which
/// would otherwise despawn freshly-spawned content while its deferred
/// init systems (`project_files` refresh, `material_browser` scan, etc.)
/// still hold pointers to it.
///
/// Supports three save formats (in priority order):
/// 1. `WorkspacesPersist`: full per-workspace registry (current).
/// 2. Bare `DockTree`: single-workspace layout (older format).
/// 3. None / unparseable: fall through to defaults.
fn init_layout(world: &mut World) {
    let layout_json = world
        .get_resource::<crate::project::ProjectRoot>()
        .and_then(|p| p.config.project.layout.clone());

    let mut loaded_tree = false;
    if let Some(json) = layout_json {
        // Try the per-workspace format first.
        if let Ok(persist) =
            serde_json::from_value::<jackdaw_panels::WorkspacesPersist>(json.clone())
            && !persist.workspaces.is_empty()
        {
            let active_tree = {
                let mut registry = world.resource_mut::<jackdaw_panels::WorkspaceRegistry>();
                persist.apply_to_registry(&mut registry);
                registry.active_workspace().map(|w| w.tree.clone())
            };
            if let Some(tree) = active_tree {
                world.insert_resource(tree);
                loaded_tree = true;
            }
        }
        // Fall back to the older bare-DockTree format.
        if !loaded_tree
            && let Ok(tree) = serde_json::from_value::<jackdaw_panels::tree::DockTree>(json)
        {
            world.insert_resource(tree);
            loaded_tree = true;
        }
    }

    // If the loaded tree has no root, the project file is from before
    // the flat-tree migration (it stored a per-anchor map that no
    // longer deserializes meaningfully). Rebuild defaults; the user
    // gets the canonical layout back and can re-customize. This also
    // covers the "no project file" first-run path.
    if !loaded_tree
        || world
            .resource::<jackdaw_panels::tree::DockTree>()
            .root
            .is_none()
    {
        *world.resource_mut::<jackdaw_panels::tree::DockTree>() =
            jackdaw_panels::tree::DockTree::default();
        build_default_tree(world);
    }

    jackdaw_panels::reconcile::reconcile(world);

    // Make sure the active workspace's `.tree` matches the live tree.
    // Covers both the "fresh defaults" path and the older bare-DockTree
    // load path, so subsequent workspace switches save/restore correctly.
    sync_active_workspace_from_live_tree(world);
}

/// Open `window_id` in its registered `default_area` leaf. If the
/// window already lives in a different leaf, move it there (no dupes).
/// If it isn't in the tree at all, push it onto the target leaf and
/// activate. Pushing populates the target leaf, which restores its
/// visibility automatically.
/// Pick the leaf with the largest relative area, excluding icon
/// sidebars (which are too narrow to host most windows). Walks the
/// split tree from the root and accumulates each split's `fraction`
/// (or `1.0 - fraction` on the secondary side) so the result reflects
/// the leaf's current proportional size in the workspace, not just
/// its position in storage.
fn largest_visible_leaf(
    tree: &jackdaw_panels::tree::DockTree,
) -> Option<jackdaw_panels::tree::NodeId> {
    use jackdaw_panels::DockAreaStyle;
    use jackdaw_panels::tree::{DockNode, NodeId};

    fn walk(
        tree: &jackdaw_panels::tree::DockTree,
        node: NodeId,
        area: f32,
        out: &mut Vec<(NodeId, f32, DockAreaStyle)>,
    ) {
        match tree.get(node) {
            Some(DockNode::Leaf(l)) => out.push((node, area, l.style.clone())),
            Some(DockNode::Split(s)) => {
                walk(tree, s.a, area * s.fraction, out);
                walk(tree, s.b, area * (1.0 - s.fraction), out);
            }
            None => {}
        }
    }

    let root = tree.root?;
    let mut leaves = Vec::new();
    walk(tree, root, 1.0, &mut leaves);
    leaves
        .into_iter()
        .filter(|(_, _, style)| !matches!(style, DockAreaStyle::IconSidebar))
        .max_by(|(_, a, _), (_, b, _)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(id, _, _)| id)
}

fn open_window_in_default_area(world: &mut World, window_id: &str) {
    use jackdaw_panels::tree::DockTree;

    let Some(default_area) = world
        .resource::<jackdaw_panels::WindowRegistry>()
        .get(window_id)
        .map(|d| d.default_area.clone())
    else {
        return;
    };

    let target_leaf = {
        let tree = world.resource::<DockTree>();
        // First choice: the window's canonical area. Second choice:
        // the largest non-IconSidebar leaf, computed by walking the
        // split tree and accumulating fractions. The previous fallback
        // (`tree.leaves().next()`) returned an arbitrary leaf because
        // the underlying storage is a HashMap; a Window-menu open of
        // "Viewport" could silently land in a thin sidebar tab where
        // the user didn't see it.
        let canonical = if default_area.is_empty() {
            None
        } else {
            tree.find_by_area_id(&default_area)
        };
        canonical.or_else(|| {
            let pick = largest_visible_leaf(tree);
            if pick.is_none() {
                warn!(
                    "open_window_in_default_area({window_id}): no leaf matched \
                     `{default_area}` and no visible leaf available as fallback",
                );
            } else if !default_area.is_empty() {
                warn!(
                    "open_window_in_default_area({window_id}): canonical area \
                     `{default_area}` not found; placing in largest visible leaf",
                );
            }
            pick
        })
    };
    let Some(target_leaf) = target_leaf else {
        return;
    };

    let target_is_empty = world
        .resource::<DockTree>()
        .get(target_leaf)
        .and_then(|n| n.as_leaf())
        .map(|l| l.windows.is_empty())
        .unwrap_or(false);

    let mut tree = world.resource_mut::<DockTree>();

    // Normalize: a leaf left over from a collapsed split still
    // carries a synthetic `area_id` ("split.<window>.<id>"). If the
    // user is repopulating the canonical area, restore the canonical
    // id so downstream lookups (save/load diagnostics,
    // `find_by_area_id`) see a consistent value.
    if target_is_empty
        && let Some(leaf) = tree.get_mut(target_leaf).and_then(|n| n.as_leaf_mut())
        && leaf.area_id != default_area
    {
        leaf.area_id = default_area.clone();
    }

    // Each Window-menu click yields a fresh tab. The reconciler picks
    // up the new entry on next tick and rebuilds the panel UI.
    let _ = tree.add_tab(target_leaf, window_id);
}

/// Reset the active workspace to the default seed: clear the live
/// tree, build the canonical layout from registered windows, and
/// reconcile in a single pass. Same path `init_layout` takes for a
/// fresh editor launch.
fn reset_layout(world: &mut World) {
    *world.resource_mut::<jackdaw_panels::tree::DockTree>() =
        jackdaw_panels::tree::DockTree::default();
    build_default_tree(world);
    jackdaw_panels::reconcile::reconcile(world);
    sync_active_workspace_from_live_tree(world);
}

fn sync_active_workspace_from_live_tree(world: &mut World) {
    let live = world.resource::<jackdaw_panels::tree::DockTree>().clone();
    let active_id = world
        .resource::<jackdaw_panels::WorkspaceRegistry>()
        .active
        .clone();
    if let Some(id) = active_id {
        let mut registry = world.resource_mut::<jackdaw_panels::WorkspaceRegistry>();
        if let Some(ws) = registry.get_mut(&id) {
            ws.tree = live;
        }
    }
}

/// First-run / reset layout: build the canonical flat dock tree.
///
/// Shape (mirrors what the old hardcoded outer layout produced, just
/// expressed as nested splits inside one tree):
///
/// ```text
/// root: H-split 0.15
///   |- left            (gets vertically split below if project_files exists)
///   `- H-split 0.85
///       |- V-split 0.8
///       |   |- center        (viewport host, headless)
///       |   `- bottom_dock   (asset / texture / material browsers)
///       `- right_sidebar     (inspector + friends)
/// ```
///
/// Each canonical leaf is populated from `WindowRegistry::by_area`
/// based on the windows registered with that `default_area`. The
/// `center` leaf is empty in Phase 1 (the hardcoded `SceneViewport`
/// is parented into it by `setup_viewport`); Phase 2 will register a
/// viewport panel into it.
///
/// Project Files is split off the bottom of the `left` leaf via the
/// runtime split API so the resulting bottom-left leaf gets a
/// non-persistent synthetic id and collapses naturally back into the
/// rest of the left sidebar if the user closes it.
fn build_default_tree(world: &mut World) {
    use jackdaw_panels::tree::{DockLeaf, DockNode, DockSplit, DockTree, Edge, SplitAxis};
    use jackdaw_panels::{DockAreaStyle, WindowRegistry};

    let windows_for = |area: &str, world: &World| -> Vec<String> {
        world
            .resource::<WindowRegistry>()
            .by_area(area)
            .iter()
            .map(|d| d.id.clone())
            .collect()
    };

    let left_windows = windows_for("left", world);
    let center_windows = windows_for("center", world);
    let bottom_windows = windows_for("bottom_dock", world);
    let right_windows = windows_for("right_sidebar", world);

    let mut tree = world.resource_mut::<DockTree>();

    let left = tree.insert(DockNode::Leaf(
        DockLeaf::new("left", DockAreaStyle::TabBar)
            .with_windows(left_windows.clone())
            .persistent(),
    ));
    // Center hosts viewport panels. Style is TabBar so when the user
    // adds a second viewport (or any other panel) into the center
    // leaf they get a tab strip to switch between them.
    let center = tree.insert(DockNode::Leaf(
        DockLeaf::new("center", DockAreaStyle::TabBar)
            .with_windows(center_windows)
            .persistent(),
    ));
    let bottom = tree.insert(DockNode::Leaf(
        DockLeaf::new("bottom_dock", DockAreaStyle::IconSidebar)
            .with_windows(bottom_windows)
            .persistent(),
    ));
    let right = tree.insert(DockNode::Leaf(
        DockLeaf::new("right_sidebar", DockAreaStyle::TabBar)
            .with_windows(right_windows)
            .persistent(),
    ));

    let center_over_bottom = tree.insert(DockNode::Split(DockSplit {
        axis: SplitAxis::Vertical,
        fraction: 0.8,
        a: center,
        b: bottom,
    }));
    let center_and_right = tree.insert(DockNode::Split(DockSplit {
        axis: SplitAxis::Horizontal,
        fraction: 0.85,
        a: center_over_bottom,
        b: right,
    }));
    let root = tree.insert(DockNode::Split(DockSplit {
        axis: SplitAxis::Horizontal,
        fraction: 0.15,
        a: left,
        b: center_and_right,
    }));
    tree.root = Some(root);

    // Split project_files off the bottom of the left leaf so it lives
    // in its own pane (matching the original hardcoded layout). The
    // new leaf gets a synthetic area_id, so closing project_files
    // collapses it back into the rest of the left sidebar.
    if left_windows.iter().any(|w| w == "jackdaw.project_files") {
        tree.remove_window_kind("jackdaw.project_files");
        if let Some((new_leaf, _)) =
            tree.split(left, Edge::Bottom, "jackdaw.project_files".to_string())
            && let Some(split_id) = tree.parent_of(new_leaf)
        {
            tree.set_fraction(split_id, 0.75);
        }
    }
}

fn sync_icon_font(
    icon_font: Option<Res<jackdaw_feathers::icons::IconFont>>,
    mut commands: Commands,
) {
    if let Some(font) = icon_font {
        commands.insert_resource(jackdaw_panels::IconFontHandle(font.0.clone()));
    }
}
