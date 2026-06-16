#![feature(try_trait_v2)]
#![feature(try_trait_v2_residual)]
//! Public API for Jackdaw editor extensions.
//!
//! Extensions are entities. An extension entity holds an [`lifecycle::Extension`]
//! component, and every registration (operators, windows, BEI contexts,
//! panel extensions) spawns child entities under it. Unloading an
//! extension is `world.entity_mut(ext).despawn()`; Bevy cascades through
//! the children and a few observers handle the non-ECS cleanup.
//!
//! Minimal extension:
//!
//! ```ignore
//! use bevy::prelude::*;
//! use bevy_enhanced_input::prelude::*;
//! use jackdaw_api::prelude::*;
//!
//! #[operator(id = "sample.place_cube")]
//! fn place_cube(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
//!     commands.spawn((Name::new("Cube"), Transform::default()));
//!     OperatorResult::Finished
//! }
//!
//! #[derive(Component, Default)]
//! struct SamplePluginContext;
//!
//! #[derive(Default)]
//! struct MyCoolExtension;
//!
//! impl JackdawExtension for MyCoolExtension {
//!     fn name() -> String { "The coolest extension".into() }
//!     fn register(&self, ctx: &mut ExtensionContext) {
//!         ctx.register_operator::<PlaceCubeOp>();
//!         ctx.spawn((
//!             SamplePluginContext,
//!             actions!(SamplePluginContext[
//!                 Action::<PlaceCubeOp>::new(),
//!                 bindings![KeyCode::KeyC],
//!             ]),
//!         ));
//!     }
//!     fn register_input_context(&self, app: &mut App) {
//!         app.add_input_context::<SamplePluginContext>();
//!     }
//! }
//! ```

pub mod entity_icons;
mod export;
pub mod extensions_config;
pub mod ffi;
pub mod inspector;
pub mod keymap;
pub mod keymap_conditions;
pub mod lifecycle;
pub mod operator;
pub mod paths;
pub mod pie;
mod registries;
pub mod runtime;
pub mod snapshot;

use std::borrow::Cow;
use std::sync::Arc;

use bevy::ecs::{system::IntoObserverSystem, world::EntityWorldMut};
use bevy::prelude::*;
use bevy_enhanced_input::prelude::{
    Action, ActionOf, ActionSettings, Fire, InputConditionAppExt as _,
};
use jackdaw_panels::{
    DockWindowDescriptor, WindowRegistry, WorkspaceDescriptor, WorkspaceRegistry,
};

use operator::{CallOperatorSettings, Operator};
use registries::WindowExtensionRegistry;
use snapshot::{ActiveSnapshotter, SceneSnapshot};

pub use entity_icons::EntityIconRegistry;
pub use jackdaw_api_macros as macros;
pub use jackdaw_api_macros::operator;
pub use jackdaw_jsn as jsn;
pub use lucide_icons;

use crate::lifecycle::{ExtensionResourceOf, OperatorAction, ResourceId};
use crate::operator::OperatorCommandsExt as _;
use crate::{
    lifecycle::{
        ExtensionKind, OperatorEntity, RegisteredMenuEntry, RegisteredWindow,
        RegisteredWindowExtension, RegisteredWorkspace,
    },
    operator::ExecutionContext,
};

pub use jackdaw_panels::area::{DefaultArea, ToAnchorId};
pub use keymap::{
    ActiveKeymapPreset, BuiltinActions, DefaultKeymap, KeymapApplyReport, KeymapPreset,
    PresetBinding, PresetContext, PresetInput, PresetPhase, PresetSpawnedBinding,
    apply_keymap_preset, load_active_keymap_preset, mouse_button_from_name, mouse_button_name,
    save_active_keymap_preset,
};
pub use keymap_conditions::{DoubleClick, ScrollTick};
pub use lifecycle::{ActiveModalOperator, Extension, ExtensionCatalog};
pub use operator::{CallOperatorError, OperatorResult, OperatorWorldExt};
pub use pie::PlayState;
pub use snapshot::SceneSnapshotter;

/// Re-exports plugin authors will want in one import.
pub mod prelude {
    pub use crate::{
        ExtensionContext, ExtensionPoint, JackdawExtension, MenuEntryDescriptor, PanelContext,
        WindowDescriptor,
        lifecycle::{
            ActiveModalQuery, Extension, ExtensionAppExt as _, ExtensionCatalog, ExtensionKind,
            RegisteredMenuEntry, RegisteredWindow,
        },
        macros::operator,
        operator::{
            CallOperatorSettings, ExecutionContext, Operator, OperatorCommandsExt as _,
            OperatorParameters, OperatorResult, OperatorSignature, OperatorSystemId,
            OperatorWorldExt as _, ParamSpec,
        },
        pie::PlayState,
        runtime::{GameApp, GamePlugin, GameRegistered, GameRegistry, GameSystems},
        snapshot::{ActiveSnapshotter, SceneSnapshot, SceneSnapshotter},
    };
    // BEI types extension authors need for `actions!` / `bindings!` / observers.
    pub use bevy_enhanced_input::prelude::*;
    // Re-export Bevy's SystemId here so Operator impls don't need to import it.
    pub use bevy::ecs::system::SystemId;
}

/// Trait implemented by every extension. Declares the extension's name
/// and registration logic; the framework handles everything else.
pub trait JackdawExtension: Send + Sync + 'static {
    /// A unique identifier for this extension. This will be used to refer to the extension internally.
    /// The prefix `"jackdaw."` as well as the name `jackdaw` itself are reserved for built-in extensions.
    fn id(&self) -> String;

    /// A human-readable name for this extension. This will be displayed in UIs.
    fn label(&self) -> String {
        self.id()
    }

    /// A human-readable description for this extension. This will be displayed in UIs.
    fn description(&self) -> String {
        "".to_string()
    }

    /// Classify this extension. Defaults to [`ExtensionKind::Regular`].
    ///
    /// The Extensions dialog reads this to split the list into Built-in
    /// and Custom sections. Reserved as a future hook for marketplace
    /// categories.
    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Regular
    }

    /// Hook for one-time BEI input-context registration.
    ///
    /// Called once per catalog entry at app startup, before any
    /// `register()` call. BEI's `add_input_context::<C>()` must run
    /// exactly once per context type per app lifetime, so it cannot live
    /// inside `register` which runs on every enable.
    ///
    /// Defaults to no-op; override only if the extension adds BEI
    /// contexts.
    // FIXME: this leaks memory when the extension is disabled
    #[expect(unused_variables, reason = "The default implementation does nothing")]
    fn register_input_context(&self, app: &mut App) {}

    /// Main registration logic. Called each time the extension is
    /// enabled. Spawn operators, windows, BEI action entities, and any
    /// other owned state here.
    fn register(&self, ctx: &mut ExtensionContext);

    /// Optional hook called before the extension entity despawns.
    ///
    /// Child-entity cleanup handles registered windows, operators, BEI
    /// contexts, and observers automatically. Override only for non-ECS
    /// state (file handles, network sessions, and the like).
    #[expect(unused_variables, reason = "The default implementation does nothing")]
    fn unregister(&self, world: &mut World, extension_entity: Entity) {}
}

/// Passed to [`JackdawExtension::register`]. Holds the extension entity
/// and provides helpers that spawn child entities under it.
///
/// Wraps `&mut World` rather than `&mut App` because extensions may be
/// loaded from world-only contexts such as the Extensions dialog's
/// enable/disable observer. One-time setup that genuinely requires App
/// access (BEI input-context registration) runs through
/// [`JackdawExtension::register_input_context`] at catalog-registration
/// time.
pub struct ExtensionContext<'a> {
    world: &'a mut World,
    extension_entity: Entity,
}

impl<'a> ExtensionContext<'a> {
    pub fn new(world: &'a mut World, extension_entity: Entity) -> Self {
        Self {
            world,
            extension_entity,
        }
    }

    /// Calls [`World::init_resource`] to initialize a resource, ensuring that it is removed on unload.
    pub fn init_resource<T: Resource + Default>(&mut self) -> &mut Self {
        let id = self.world.init_resource::<T>();
        self.world.spawn(ExtensionResourceOf {
            entity: self.id(),
            resource_id: ResourceId(id),
        });
        self
    }

    /// Calls [`World::insert_resource`] to initialize a resource, ensuring that it is removed on unload.
    pub fn insert_resource<T: Resource>(&mut self, resource: T) -> &mut Self {
        self.world.insert_resource(resource);
        let id = self
            .world
            .resource_id::<T>()
            .expect("resource_id should be Some since resource was just inserted");
        self.world.spawn(ExtensionResourceOf {
            entity: self.id(),
            resource_id: ResourceId(id),
        });
        self
    }

    /// Calls [`World::add_observer`] to initialize an observer, ensuring that it is removed on unload.
    pub fn add_observer<E: Event, B: Bundle, M>(
        &mut self,
        system: impl IntoObserverSystem<E, B, M>,
    ) -> &mut Self {
        self.entity_mut().with_child(Observer::new(system));
        self
    }

    /// The root [`lifecycle::Extension`] entity.
    ///
    /// See also: [`ExtensionContext::entity`] and [`ExtensionContext::entity_mut`].
    pub fn id(&self) -> Entity {
        self.extension_entity
    }

    /// Register a dock window. Spawns a [`RegisteredWindow`] marker
    /// entity as a child of the extension entity; a cleanup observer
    /// calls `WindowRegistry::unregister` when the marker despawns.
    pub fn register_window(&mut self, descriptor: WindowDescriptor) -> &mut Self {
        let ext = self.extension_entity;
        let dock_descriptor = DockWindowDescriptor {
            id: descriptor.id.clone(),
            name: descriptor.name,
            icon: descriptor.icon,
            default_area: descriptor.default_area.anchor_id().to_string(),
            priority: descriptor.priority.unwrap_or(100),
            build: descriptor.build,
        };
        self.world
            .resource_mut::<WindowRegistry>()
            .register(dock_descriptor);
        self.world
            .spawn((RegisteredWindow { id: descriptor.id }, ChildOf(ext)));
        self
    }

    /// Register a workspace.
    pub fn register_workspace(&mut self, descriptor: WorkspaceDescriptor) -> &mut Self {
        let ext = self.extension_entity;
        let id = descriptor.id.clone();
        self.world
            .resource_mut::<WorkspaceRegistry>()
            .register(descriptor);
        self.world.spawn((RegisteredWorkspace { id }, ChildOf(ext)));
        self
    }

    /// Spawn an entity as a child of the extension entity. Typically
    /// used for BEI context entities with action bindings:
    /// `ctx.spawn((MyContext, actions!(MyContext[...])))`.
    ///
    /// The returned [`EntityWorldMut`] lets the caller keep adding
    /// components or children. Anything spawned this way is torn down
    /// when the extension unloads.
    pub fn spawn<'w>(&'w mut self, bundle: impl Bundle) -> EntityWorldMut<'w> {
        let ext = self.extension_entity;
        let mut ec = self.world.spawn(bundle);
        ec.insert(ChildOf(ext));
        ec
    }

    /// Get the extension's root entity. Useful for inserting components that you want to
    /// be torn down on unload.
    pub fn entity<'w>(&'w self) -> EntityRef<'w> {
        self.world.entity(self.extension_entity)
    }

    /// Get the extension's root entity mutably. Useful for inserting components that you want to
    /// be torn down on unload.
    pub fn entity_mut<'w>(&'w mut self) -> EntityWorldMut<'w> {
        self.world.entity_mut(self.extension_entity)
    }

    /// Register an operator. Spawns an `OperatorEntity` as a child
    /// of the extension entity and a `Fire<O>` observer that dispatches the
    /// operator through [`crate::OperatorWorldExt::operator`]. BEI binding
    /// modifiers on the actions shape timing (press / release / hold).
    pub fn register_operator<O: Operator>(&mut self) -> &mut Self {
        let ext = self.extension_entity;

        let (execute, invoke, availability_check, cancel) = {
            let mut queue = bevy::ecs::world::CommandQueue::default();
            let mut commands = Commands::new(&mut queue, self.world);
            let execute = O::register_execute(&mut commands);
            let invoke = O::register_invoke(&mut commands);
            let availability_check = O::register_availability_check(&mut commands);
            let cancel = O::register_cancel(&mut commands);
            queue.apply(self.world);
            (execute, invoke, availability_check, cancel)
        };

        self.world.spawn((
            OperatorEntity {
                id: O::ID,
                label: O::LABEL,
                description: O::DESCRIPTION,
                parameters: O::PARAMETERS,
                execute,
                invoke,
                availability_check,
                cancel,
                modal: O::MODAL,
                allows_undo: O::ALLOWS_UNDO,
            },
            ChildOf(ext),
            children![
                Observer::new(move |_: On<Fire<O>>, mut commands: Commands| {
                    commands
                        .operator(O::ID)
                        .settings(CallOperatorSettings {
                            execution_context: ExecutionContext::Invoke,
                            creates_history_entry: true,
                        })
                        .call();
                },),
                // Auto-tag any BEI action entity for this operator with
                // `OperatorAction(Op::ID)` so id-keyed lookups (tooltip
                // keybind discovery, future command palette) can find the
                // bindings without naming the typed `Action<Op>`. The
                // observer covers future spawns; the immediate query pass
                // below covers entities already spawned before this call
                // (some `add_to_extension` modules spawn actions first and
                // register the operator afterwards).
                Observer::new(move |trigger: On<Add, Action<O>>, mut commands: Commands| {
                    commands
                        .entity(trigger.event_target())
                        .insert(OperatorAction(O::ID));
                })
            ],
        ));

        if let Err(err) = self.world.run_system_cached(tag_existing_actions::<O>) {
            error!("Failed to tag existing actions: {}", err);
        }

        self
    }

    /// Record an operator's default key bindings into [`DefaultKeymap`] and
    /// spawn its BEI action entity WITHOUT bindings. Bindings are attached
    /// later by the keymap applier from the active preset, so presets are
    /// the single source of binding truth. The action belongs to the
    /// extension's input context `C`.
    ///
    /// Duplicate-input rows are silently skipped so that re-enabling an
    /// extension after `apply_active_keymap` re-runs does not accumulate
    /// duplicate entries in the defaults list.
    ///
    /// The context component `C` must live on the extension's root entity; that is where `ActionOf` points.
    pub fn bind_operator<C: bevy::prelude::Component, O: crate::Operator>(
        &mut self,
        defaults: impl IntoIterator<Item = crate::keymap::PresetInput>,
    ) -> &mut Self {
        self.action_for::<C, O>();
        let mut keymap = self
            .world
            .get_resource_or_init::<crate::keymap::DefaultKeymap>();
        for input in defaults {
            if keymap
                .bindings
                .iter()
                .any(|b| b.operator == O::ID && b.input == input)
            {
                warn!(
                    "duplicate default binding for operator '{}'; skipping (idempotent re-registration)",
                    O::ID
                );
                continue;
            }
            keymap.bindings.push(crate::keymap::PresetBinding {
                operator: O::ID.to_string(),
                input,
                phase: crate::keymap::PresetPhase::Press,
                context: crate::keymap::PresetContext::Operators,
            });
        }
        self
    }

    /// Spawn an operator's action entity with no default binding. The
    /// operator stays reachable through menus and the command palette
    /// and can be bound by presets or user rebinds.
    ///
    /// The context component `C` must live on the extension's root entity; that is where `ActionOf` points.
    ///
    /// `require_reset` guards against operators firing from already-held keys when bindings or
    /// contexts are (re)applied.
    pub fn action_for<C: bevy::prelude::Component, O: crate::Operator>(&mut self) -> &mut Self {
        let ext = self.extension_entity;
        self.spawn((
            Action::<O>::new(),
            ActionOf::<C>::new(ext),
            ActionSettings {
                require_reset: true,
                ..Default::default()
            },
        ));
        self
    }

    /// Inject a section into an existing window (e.g. add a sub-section to
    /// the Inspector window). Section runs with `In<PanelContext>` each time
    /// the window re-renders.
    pub fn extend_window(
        &mut self,
        id: impl Into<Cow<'static, str>>,
        build: impl Fn(&mut ChildSpawner) + Send + Sync + 'static,
    ) -> &mut Self {
        let ext = self.extension_entity;
        let id = id.into();
        let mut registry = self.world.resource_mut::<WindowExtensionRegistry>();
        let section_index = registry.get(&id).count();
        registry.add(id.clone(), build);
        self.world.spawn((
            RegisteredWindowExtension {
                window_id: id,
                section_index,
            },
            ChildOf(ext),
        ));
        self
    }

    /// Contribute an entry to one of the editor's top-level menus
    /// (`"Add"`, `"Tools"`, etc.). Clicking the entry dispatches the
    /// referenced operator.
    pub fn register_menu_entry_manual(&mut self, descriptor: MenuEntryDescriptor) -> &mut Self {
        let ext = self.extension_entity;
        self.world.spawn((
            RegisteredMenuEntry {
                menu: descriptor.menu,
                label: descriptor.label,
                operator_id: descriptor.operator_id,
            },
            ChildOf(ext),
        ));
        self
    }

    /// Convenience that registers a menu entry using `O::LABEL` and
    /// `O::ID` from the operator type, so callers only need to supply the
    /// menu name. Equivalent to calling
    /// [`Self::register_menu_entry_manual`] with a full [`MenuEntryDescriptor`].
    pub fn register_menu_entry<O: Operator>(&mut self, menu: TopLevelMenu) -> &mut Self {
        self.register_menu_entry_manual(MenuEntryDescriptor {
            menu,
            label: O::LABEL.to_string(),
            operator_id: O::ID,
        })
    }

    /// Register the Lucide icon shown in the outliner for entities carrying
    /// the given component type path. First registered match wins.
    pub fn register_entity_icon(
        &mut self,
        type_path: impl Into<String>,
        icon: lucide_icons::Icon,
    ) -> &mut Self {
        // Insert on demand so a seeding extension that registers before the
        // outliner plugin inits the resource still lands its entries, rather
        // than silently dropping them.
        self.world
            .get_resource_or_insert_with(EntityIconRegistry::default)
            .register(type_path, icon);
        self
    }

    /// Add an inspector category tab. The six built-in categories are
    /// pre-registered; this appends or replaces by id.
    pub fn register_inspector_category(
        &mut self,
        category: crate::inspector::InspectorCategory,
    ) -> &mut Self {
        self.world
            .get_resource_or_insert_with(|| {
                let mut r = crate::inspector::InspectorRegistry::default();
                crate::inspector::seed_default_categories(&mut r);
                r
            })
            .register_category(category);
        self
    }

    /// Route a component type into an inspector category by id. The default
    /// category for unmapped components is "components".
    pub fn register_component_category<T: bevy::reflect::TypePath>(
        &mut self,
        category_id: impl Into<std::borrow::Cow<'static, str>>,
    ) -> &mut Self {
        let type_path = T::type_path().to_string();
        self.world
            .get_resource_or_insert_with(|| {
                let mut r = crate::inspector::InspectorRegistry::default();
                crate::inspector::seed_default_categories(&mut r);
                r
            })
            .set_component_category(type_path, category_id);
        self
    }
}

/// Tag any `Action<O>` entities that already exist with `OperatorAction(O::ID)`.
/// Run from `register_operator` to cover the case where action entities were
/// spawned before the operator was registered. Bevy caches the `QueryState`
/// across calls when this is invoked via `run_system_cached`.
fn tag_existing_actions<O: Operator>(
    world: &mut World,
    actions: &mut QueryState<Entity, With<Action<O>>>,
) {
    let existing: Vec<Entity> = actions.iter(world).collect();
    for entity in existing {
        world.entity_mut(entity).insert(OperatorAction(O::ID));
    }
}

/// Top level menus available for menu bar entries.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TopLevelMenu {
    File,
    Edit,
    View,
    Add,
    Tools,
    Window,
    Custom(String),
}

impl TopLevelMenu {
    /// Returns the unique ID of the menu, used internally by the UI.
    pub fn id(&self) -> String {
        match self {
            TopLevelMenu::File => "File".to_string(),
            TopLevelMenu::Edit => "Edit".to_string(),
            TopLevelMenu::Add => "Add".to_string(),
            TopLevelMenu::View => "View".to_string(),
            TopLevelMenu::Tools => "Tools".to_string(),
            TopLevelMenu::Window => "Window".to_string(),
            TopLevelMenu::Custom(id) => id.clone(),
        }
    }

    /// Returns the order of the menu, used to sort menu items in the UI.
    pub fn order(&self) -> u8 {
        match self {
            TopLevelMenu::File => 0,
            TopLevelMenu::Edit => 1,
            TopLevelMenu::Add => 2,
            TopLevelMenu::View => 3,
            TopLevelMenu::Tools => 4,
            TopLevelMenu::Window => 5,
            TopLevelMenu::Custom(_) => 6,
        }
    }
}

/// Extension-facing descriptor for a menu bar entry. See
/// [`ExtensionContext::register_menu_entry_manual`].
pub struct MenuEntryDescriptor {
    /// Top-level menu.
    pub menu: TopLevelMenu,
    /// Text shown on the menu item.
    pub label: String,
    /// ID of an operator registered on the same extension, or any other
    /// loaded extension. Operator IDs are global. Clicking the menu
    /// entry dispatches this operator.
    pub operator_id: &'static str,
}

/// Extension-facing descriptor for a dock window. Mirrors
/// [`jackdaw_panels::DockWindowDescriptor`] but with `default_area`
/// optional: third-party extensions leave it `None` so their windows are
/// not auto-placed, while built-in Jackdaw extensions set it to preserve
/// the default layout.
pub struct WindowDescriptor {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub default_area: Option<DefaultArea>,
    pub priority: Option<i32>,
    pub build: Arc<dyn Fn(&mut ChildSpawner) + Send + Sync + 'static>,
}

impl WindowDescriptor {
    /// Creates a new `WindowDescriptor` with the given unique `id`.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            id: id.clone(),
            name: id,
            ..default()
        }
    }

    /// Sets the name of the window shown in the UI.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Sets the icon of the window.
    #[must_use]
    pub fn with_icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Sets the default area of the window used when adding the window.
    #[must_use]
    pub fn with_default_area(mut self, area: impl Into<Option<DefaultArea>>) -> Self {
        self.default_area = area.into();
        self
    }

    /// Sets the priority of the window.
    #[must_use]
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = Some(priority);
        self
    }

    /// Sets the build function for the window, which is used for building the window's UI.
    #[must_use]
    pub fn with_build(mut self, build: impl Fn(&mut ChildSpawner) + Send + Sync + 'static) -> Self {
        self.build = Arc::new(build);
        self
    }
}

impl Default for WindowDescriptor {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            icon: None,
            default_area: None,
            priority: None,
            build: Arc::new(|_| {}),
        }
    }
}

/// Marker trait for panels that accept extension sections.
pub trait ExtensionPoint: 'static {
    const ID: &'static str;
}

pub struct InspectorWindow;
impl ExtensionPoint for InspectorWindow {
    const ID: &'static str = "jackdaw.inspector.components";
}

pub struct HierarchyWindow;
impl ExtensionPoint for HierarchyWindow {
    const ID: &'static str = "jackdaw.hierarchy";
}

/// Context passed to a panel-extension section when it's rendered.
pub struct PanelContext {
    pub window_id: String,
    pub panel_entity: Entity,
}

/// Plugin that wires up the extension framework into the editor.
///
/// Adds BEI, sets up the required resources (`OperatorIndex`,
/// `PanelExtensionRegistry`, `ExtensionCatalog`, `ActiveModalOperator`),
/// and registers the cleanup observers that keep non-ECS state in sync
/// when extension entities are despawned.
///
/// Also runs `tick_modal_operator` each frame in Update so modal
/// operators (Blender-style grab/rotate/scale) re-run their invoke
/// system until they return `Finished` or `Cancelled`.
pub struct ExtensionLoaderPlugin;

impl Plugin for ExtensionLoaderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((lifecycle::plugin, operator::plugin, registries::plugin));
        app.add_input_condition::<DoubleClick>();
        app.add_input_condition::<ScrollTick>();
    }
}

#[cfg(test)]
mod tests {
    use bevy::prelude::*;
    use bevy_enhanced_input::prelude::*;

    use super::*;
    use crate::keymap::{DefaultKeymap, PresetInput, PresetPhase};
    use crate::operator::{Operator, OperatorParameters, OperatorResult, OperatorSystemId};

    // Minimal input context used only in these tests.
    #[derive(Component, Default)]
    struct TestCtx;

    // Minimal operator: no-op execute, manually implements Operator so the
    // test stays self-contained without spawning SystemIds or running schedules.
    #[derive(Default, InputAction)]
    #[action_output(bool)]
    struct TestOp;

    impl Operator for TestOp {
        const ID: &'static str = "test.op";
        const LABEL: &'static str = "Test Op";

        fn register_execute(commands: &mut Commands) -> OperatorSystemId {
            commands.register_system(|_: In<OperatorParameters>| -> OperatorResult {
                OperatorResult::Finished
            })
        }
    }

    fn make_ctx(world: &mut World) -> (Entity, ExtensionContext<'_>) {
        let ext = world.spawn_empty().id();
        let ctx = ExtensionContext::new(world, ext);
        (ext, ctx)
    }

    #[test]
    fn bind_operator_records_defaults_and_spawns_action() {
        let mut world = World::default();
        let (ext, mut ctx) = make_ctx(&mut world);

        ctx.bind_operator::<TestCtx, TestOp>([
            PresetInput::key("KeyQ"),
            PresetInput::key("KeyT").ctrl(),
        ]);

        // DefaultKeymap has exactly 2 entries with the right operator id and inputs.
        let keymap = world
            .get_resource::<DefaultKeymap>()
            .expect("DefaultKeymap must exist after bind_operator");
        assert_eq!(keymap.bindings.len(), 2);
        assert_eq!(keymap.bindings[0].operator, "test.op");
        assert_eq!(keymap.bindings[0].input, PresetInput::key("KeyQ"));
        assert_eq!(keymap.bindings[0].phase, PresetPhase::Press);
        assert_eq!(keymap.bindings[1].operator, "test.op");
        assert_eq!(keymap.bindings[1].input, PresetInput::key("KeyT").ctrl());
        assert_eq!(keymap.bindings[1].phase, PresetPhase::Press);

        // Exactly one Action<TestOp> entity exists.
        let action_count = world.query::<&Action<TestOp>>().iter(&world).count();
        assert_eq!(action_count, 1);

        // No Binding components exist on any entity (action spawned without bindings).
        let binding_count = world
            .query::<&bevy_enhanced_input::prelude::Binding>()
            .iter(&world)
            .count();
        assert_eq!(binding_count, 0);

        // The spawned action entity is a child of the extension root entity.
        let (_, child_of) = world
            .query_filtered::<(Entity, &ChildOf), With<Action<TestOp>>>()
            .single(&world)
            .expect("exactly one Action<TestOp> must exist");
        assert_eq!(child_of.parent(), ext);
    }

    #[test]
    fn action_for_spawns_action_with_no_keymap_entry() {
        let mut world = World::default();
        let (_, mut ctx) = make_ctx(&mut world);

        ctx.action_for::<TestCtx, TestOp>();

        // No DefaultKeymap entry recorded.
        let entry_count = world
            .get_resource::<DefaultKeymap>()
            .map_or(0, |km| km.bindings.len());
        assert_eq!(entry_count, 0);

        // Exactly one Action<TestOp> entity exists.
        let action_count = world.query::<&Action<TestOp>>().iter(&world).count();
        assert_eq!(action_count, 1);
    }
}
