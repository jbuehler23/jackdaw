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

mod export;
pub mod ffi;
pub mod lifecycle;
pub mod operator;
pub mod pie;
mod registries;
pub mod runtime;
pub mod snapshot;

use std::sync::Arc;

use bevy::ecs::{system::IntoObserverSystem, world::EntityWorldMut};
use bevy::prelude::*;
use jackdaw_panels::{
    DockWindowDescriptor, WindowRegistry, WorkspaceDescriptor, WorkspaceRegistry,
};

use operator::{CallOperatorSettings, Operator, OperatorWorldExt};
use registries::PanelExtensionRegistry;
use snapshot::{ActiveSnapshotter, SceneSnapshot};

pub use jackdaw_api_macros as macros;
pub use jackdaw_api_macros::operator;
pub use jackdaw_jsn as jsn;

use crate::lifecycle::{ExtensionResourceOf, ResourceId};
use crate::{
    lifecycle::{
        ExtensionKind, OperatorEntity, RegisteredMenuEntry, RegisteredPanelExtension,
        RegisteredWindow, RegisteredWorkspace,
    },
    operator::ExecutionContext,
};

pub use lifecycle::{ActiveModalOperator, Extension, ExtensionCatalog};
pub use operator::{CallOperatorError, OperatorResult, OperatorWorldExt as _};
pub use pie::PlayState;
pub use snapshot::SceneSnapshotter;

/// Re-exports plugin authors will want in one import.
pub mod prelude {
    pub use crate::{
        ExtensionContext, ExtensionPoint, JackdawExtension, MenuEntryDescriptor, PanelContext,
        SectionBuildFn, WindowDescriptor,
        lifecycle::{
            ActiveModalQuery, Extension, ExtensionAppExt as _, ExtensionCatalog, ExtensionKind,
            RegisteredMenuEntry, RegisteredWindow,
        },
        macros::operator,
        operator::{
            CallOperatorSettings, ExecutionContext, Operator, OperatorCommandsExt as _,
            OperatorParameters, OperatorResult, OperatorSystemId, OperatorWorldExt as _,
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
            default_area: descriptor.default_area.unwrap_or_default(),
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

        let op_entity = self
            .world
            .spawn((
                OperatorEntity {
                    id: O::ID,
                    label: O::LABEL,
                    description: O::DESCRIPTION,
                    execute,
                    invoke,
                    availability_check,
                    cancel,
                    modal: O::MODAL,
                    allows_undo: O::ALLOWS_UNDO,
                },
                ChildOf(ext),
            ))
            .id();

        let observer = Observer::new(
            move |_: bevy::prelude::On<bevy_enhanced_input::prelude::Fire<O>>,
                  mut commands: Commands| {
                commands.queue(move |world: &mut World| {
                    world
                        .operator(O::ID)
                        .settings(CallOperatorSettings {
                            execution_context: ExecutionContext::Invoke,
                            creates_history_entry: true,
                        })
                        .call()
                });
            },
        );
        self.world.spawn((observer, ChildOf(op_entity)));

        self
    }

    /// Inject a section into an existing panel (e.g. add a sub-section to
    /// the Inspector window). Section runs with `In<PanelContext>` each time
    /// the panel re-renders.
    pub fn extend_window<W: ExtensionPoint>(&mut self, section: SectionBuildFn) -> &mut Self {
        let ext = self.extension_entity;
        let panel_id = W::ID.to_string();
        let mut registry = self.world.resource_mut::<PanelExtensionRegistry>();
        let section_index = registry.get(&panel_id).len();
        registry.add(panel_id.clone(), section);
        self.world.spawn((
            RegisteredPanelExtension {
                panel_id,
                section_index,
            },
            ChildOf(ext),
        ));
        self
    }

    /// Contribute an entry to one of the editor's top-level menus
    /// (`"Add"`, `"Tools"`, etc.). Clicking the entry dispatches the
    /// referenced operator.
    pub fn register_menu_entry(&mut self, descriptor: MenuEntryDescriptor) -> &mut Self {
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
    /// [`Self::register_menu_entry`] with a full [`MenuEntryDescriptor`].
    pub fn menu_entry_for<O: Operator>(&mut self, menu: impl Into<String>) -> &mut Self {
        self.register_menu_entry(MenuEntryDescriptor {
            menu: menu.into(),
            label: O::LABEL.to_string(),
            operator_id: O::ID,
        })
    }
}

/// Extension-facing descriptor for a menu bar entry. See
/// [`ExtensionContext::register_menu_entry`].
pub struct MenuEntryDescriptor {
    /// Top-level menu name (`"Add"`, `"Tools"`, etc.).
    pub menu: String,
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
    pub default_area: Option<String>,
    pub priority: Option<i32>,
    pub build: Arc<dyn Fn(&mut World, Entity) + Send + Sync>,
}

impl Default for WindowDescriptor {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            icon: None,
            default_area: None,
            priority: None,
            build: Arc::new(|_, _| {}),
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

pub type SectionBuildFn = Arc<dyn Fn(&mut World, PanelContext) + Send + Sync>;

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
    }
}
