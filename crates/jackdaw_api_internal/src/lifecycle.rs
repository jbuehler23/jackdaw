//! Entity-based lifecycle primitives for extensions.
//!
//! An extension is represented as an [`Entity`] carrying an [`Extension`]
//! component. Everything it registers (operators, BEI context entities,
//! dock windows, workspaces) is spawned as a child of that entity.
//! Unloading is `world.entity_mut(ext).despawn()`; Bevy cascades through
//! the children. A small set of observers in `ExtensionLoaderPlugin`
//! handles cleanup that can't be expressed purely as entity despawn:
//! unregistering stored `SystemId`s, removing entries from the dock
//! `WindowRegistry`, and so on.

use std::collections::HashMap;
use std::sync::Arc;

use crate::operator::cancel_active_modal;
use crate::prelude::*;
use bevy::ecs::component::ComponentId;
use bevy::ecs::system::{SystemId, SystemParam};
use bevy::prelude::*;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<ExtensionCatalog>()
        .init_resource::<OperatorIndex>()
        .add_observer(index_operator_on_add)
        .add_observer(deindex_and_cleanup_operator_on_remove)
        .add_observer(cleanup_window_on_remove)
        .add_observer(cleanup_workspace_on_remove)
        .add_observer(cleanup_panel_extension_on_remove)
        .add_observer(cleanup_resource_on_remove);
    app.world_mut().register_component::<ActiveModalOperator>();
}

/// Root component for an extension.
///
/// Despawning this entity tears down all of the extension's child entities:
/// operators, BEI context/action entities, registered windows/workspaces, and
/// observer entities. Non-ECS cleanup (unregistering `SystemId`s, removing
/// entries from `WindowRegistry`) is handled by observers reacting to the
/// child-entity despawns.
#[derive(Component, Debug)]
pub struct Extension {
    pub id: String,
}

/// [`Resource`]s attached to a specific [`Extension`].
/// When the extension is unloaded, these resources are removed via [`World::remove_resource_by_id`].
#[derive(Component, Default, Debug, PartialEq, Eq, Deref)]
#[relationship_target(relationship = ExtensionResourceOf, linked_spawn)]
pub(crate) struct ExtensionResources(Vec<Entity>);

/// Link from an entity representing a [`Resource`] to its owning [`Extension`], ensuring
/// that the resource is removed when the extension is unloaded.
#[derive(Component, Debug, PartialEq, Eq)]
#[relationship(relationship_target = ExtensionResources)]
pub(crate) struct ExtensionResourceOf {
    #[relationship]
    pub(crate) entity: Entity,
    pub(crate) resource_id: ResourceId,
}

/// The [`ComponentId`] of a [`Resource`]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ResourceId(pub(crate) ComponentId);

impl Default for ResourceId {
    fn default() -> Self {
        Self(ComponentId::new(0))
    }
}

/// Child of an [`Extension`]; represents a single operator.
///
/// Holds the `SystemId`s that the dispatcher runs. An observer on
/// `On<Remove, OperatorEntity>` unregisters those systems when this entity
/// despawns, and keeps the `OperatorIndex` in sync.
#[derive(Component, Debug, Clone)]
pub struct OperatorEntity {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    #[expect(dead_code, reason = "This should go into the UI eventually")]
    pub(crate) description: &'static str,
    pub(crate) execute: OperatorSystemId,
    pub(crate) invoke: OperatorSystemId,
    /// Optional system that returns whether the operator can run in
    /// the current editor state. Equivalent to Blender's `poll`.
    pub(crate) availability_check: Option<SystemId<(), bool>>,
    /// Mirrors [`crate::Operator::MODAL`]. Set at registration so the
    /// dispatcher can enter modal mode without re-resolving the generic
    /// operator type.
    pub(crate) cancel: Option<SystemId<()>>,
    pub(crate) modal: bool,
    pub(crate) allows_undo: bool,
}

/// Tracks the currently-active modal operator. Exactly zero or one is
/// active at any time; starting a second modal while one is running is
/// refused.
///
/// The `before_snapshot` is captured when the modal begins; on commit
/// the dispatcher diffs it against a fresh snapshot and pushes a single
/// undo entry, so the entire modal session rolls up into one Ctrl+Z.
#[derive(Component)]
pub struct ActiveModalOperator {
    pub(crate) before_snapshot: Option<Box<dyn SceneSnapshot>>,
}

/// Convenience [`SystemParam`] for querying the active modal operator.
#[derive(SystemParam)]
pub struct ActiveModalQuery<'w, 's> {
    maybe_modal: Option<Single<'w, 's, (&'static OperatorEntity, &'static ActiveModalOperator)>>,
    commands: Commands<'w, 's>,
}

impl<'w, 's> ActiveModalQuery<'w, 's> {
    pub fn is_modal_running(&self) -> bool {
        self.maybe_modal.is_some()
    }

    pub fn is_operator(&self, operator_id: impl AsRef<str>) -> bool {
        self.get_operator()
            .is_some_and(|op| op.id == operator_id.as_ref())
    }
    pub fn get_operator(&self) -> Option<&OperatorEntity> {
        self.get_operator_and_modal().map(|m| m.0)
    }
    pub fn get_modal(&self) -> Option<&ActiveModalOperator> {
        self.get_operator_and_modal().map(|m| m.1)
    }
    pub fn get_operator_and_modal(&self) -> Option<(&OperatorEntity, &ActiveModalOperator)> {
        self.maybe_modal.as_ref().map(|m| (m.0, m.1))
    }

    pub fn cancel(&mut self) {
        self.commands.queue(|world: &mut World| {
            let res: Result = world
                .run_system_cached(cancel_active_modal)
                .map_err(BevyError::from);
            if let Err(err) = res {
                error!("Failed to cancel active modal: {err}");
            }
        });
    }
}

/// Marks an entity as tracking a dock window registration.
///
/// Spawned as a child of the [`Extension`] entity when `register_window` is
/// called. An observer on `On<Remove, RegisteredWindow>` calls
/// `WindowRegistry::unregister(id)` so the window disappears from the
/// add-window popup when the extension unloads.
#[derive(Component, Clone, Debug)]
pub struct RegisteredWindow {
    pub(crate) id: String,
}

/// Marks an entity as tracking a workspace registration.
#[derive(Component, Clone, Debug)]
pub(crate) struct RegisteredWorkspace {
    pub(crate) id: String,
}

/// Marks an entity as tracking a panel-extension registration (a section
/// injected into an existing panel via `ExtensionContext::extend_window`).
#[derive(Component, Clone, Debug)]
pub(crate) struct RegisteredPanelExtension {
    pub(crate) panel_id: String,
    pub(crate) section_index: usize,
}

/// An extension-contributed entry in the editor menu bar.
///
/// Spawned as a child of the [`Extension`] entity via
/// [`crate::ExtensionContext::register_menu_entry`]. The editor's
/// `populate_menu` system queries these and inserts them into the right
/// menu. Clicking one dispatches the referenced operator.
///
/// `menu` is the top-level menu name (`"Add"`, `"Tools"`, etc.). The
/// menu system is flat today; using a path-like string here leaves room
/// for nested menus later without breaking callers.
#[derive(Component, Clone, Debug)]
pub struct RegisteredMenuEntry {
    pub menu: String,
    pub label: String,
    pub operator_id: &'static str,
}

/// Reactive index from operator id → operator entity. Maintained by the
/// `index_operator_on_add` / `deindex_operator_on_remove` observers.
/// Lets the dispatcher resolve an id to a `SystemId` in O(1).
#[derive(Resource, Default, Deref, DerefMut)]
pub(crate) struct OperatorIndex {
    pub(crate) by_id: HashMap<&'static str, Entity>,
}

/// Constructor function for an extension. Stored in [`ExtensionCatalog`].
pub(crate) type ExtensionCtor = Arc<dyn Fn() -> Box<dyn crate::JackdawExtension> + Send + Sync>;

/// Registry of all extensions compiled into this build of Jackdaw.
///
/// Populated once during startup by calling `ExtensionCatalog::register` for
/// each built-in extension. External extensions (if/when dylib loading lands)
/// would register themselves here too. Toggle UIs read the catalog to list
/// available extensions.
#[derive(Resource, Default)]
pub struct ExtensionCatalog {
    entries: HashMap<String, CatalogEntry>,
}

struct CatalogEntry {
    ctor: ExtensionCtor,
}

/// Classifies an entry in the catalog. Surfaced in toggle UIs so
/// Jackdaw-shipped feature areas and third-party extensions can be
/// presented separately. Extensions declare their own kind via
/// [`crate::JackdawExtension::kind`]; registration captures it.
///
/// Defaults to [`ExtensionKind::Regular`]. [`ExtensionKind::Builtin`] is reserved
/// for extensions shipped inside the editor binary.
// Don't change the repr(u8) value of variants; they're used in FFI
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ExtensionKind {
    /// Ships with Jackdaw as a core feature area (scene tree, inspector,
    /// asset browser, etc.). Present in every build.
    Builtin = 0,
    /// Everything else: example extensions bundled for demonstration,
    /// third-party extensions loaded from disk, user-authored addons.
    Regular = 1,
}

impl ExtensionCatalog {
    /// Register a constructor with its declared kind. Most callers
    /// should use [`App::register_extension`] instead, which handles BEI
    /// context registration.
    fn register_extension_internal(
        &mut self,
        ctor: impl Fn() -> Box<dyn crate::JackdawExtension> + Send + Sync + 'static,
    ) {
        let id = ctor().id();
        self.entries.insert(
            id,
            CatalogEntry {
                ctor: Arc::new(ctor),
            },
        );
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// Iterate IDs with their declared [`ExtensionKind`]. Useful for
    /// grouping the Extensions dialog into Built-in and Custom sections.
    pub fn iter_with_content(
        &self,
    ) -> impl Iterator<Item = (String, String, String, ExtensionKind)> {
        self.entries.iter().map(|(id, entry)| {
            let ext = (entry.ctor)();
            (
                id.to_string(),
                ext.label().to_string(),
                ext.description().to_string(),
                ext.kind(),
            )
        })
    }

    /// Look up the declared [`ExtensionKind`] for a registered name.
    pub fn kind(&self, name: &str) -> Option<ExtensionKind> {
        self.entries.get(name).map(|e| (e.ctor)().kind())
    }

    /// Whether the named extension is a Jackdaw-shipped built-in.
    /// Returns `false` for unknown names.
    pub fn is_builtin(&self, name: &str) -> bool {
        self.kind(name) == Some(ExtensionKind::Builtin)
    }

    /// Construct a fresh instance of the named extension, if registered.
    pub fn construct(&self, name: &str) -> Option<Box<dyn crate::JackdawExtension>> {
        self.entries.get(name).map(|e| (e.ctor)())
    }
}

pub trait ExtensionAppExt {
    /// Register an extension into the catalog and perform its one-time BEI
    /// input-context registration.
    ///
    /// Call this once per extension during app setup. Registering the constructor
    /// lets the Plugins dialog list the extension; running
    /// `register_input_context` ensures its BEI context types are known to the
    /// framework. Enabling and disabling the extension later only re-runs
    /// `register()`, never `register_input_context()` (BEI panics on duplicate
    /// registrations).
    ///
    /// See also [`Self::register_extension_with`].
    fn register_extension<T: crate::JackdawExtension + Default>(&mut self) -> &mut Self {
        self.register_extension_with(|| Box::new(T::default()))
    }

    /// Like [`Self::register_extension`], but with a custom constructor.
    fn register_extension_with(
        &mut self,
        ctor: impl Fn() -> Box<dyn crate::JackdawExtension> + Send + Sync + 'static,
    ) -> &mut Self;
}

impl ExtensionAppExt for App {
    fn register_extension_with(
        &mut self,
        ctor: impl Fn() -> Box<dyn crate::JackdawExtension> + Send + Sync + 'static,
    ) -> &mut Self {
        let ext = ctor();
        ext.register_input_context(self);
        self.world_mut()
            .resource_mut::<ExtensionCatalog>()
            .register_extension_internal(ctor);
        self
    }
}

/// Unload an extension. Despawns the root entity; the cascade and
/// cleanup observers handle the rest.
pub fn unload_extension(world: &mut World, ext_entity: Entity) {
    let ext_name = world
        .get::<Extension>(ext_entity)
        .map(|e| e.id.clone())
        .unwrap_or_default();
    info!("Unloading extension: {}", ext_name);

    if let Some(stored) = world.entity_mut(ext_entity).take::<StoredExtension>() {
        stored.0.unregister(world, ext_entity);
    }
    if let Ok(ec) = world.get_entity_mut(ext_entity) {
        ec.despawn();
    }
}

/// Enable a named extension via the catalog. Returns the new extension
/// entity, or `None` if the name is unknown or already loaded.
pub fn enable_extension(world: &mut World, id: &str) -> Option<Entity> {
    {
        let mut query = world.query::<&Extension>();
        if query.iter(world).any(|e| e.id == id) {
            return None;
        }
    }

    let extension = world.resource::<ExtensionCatalog>().construct(id)?;
    Some(load_static_extension(world, extension))
}

/// Load an extension statically. Spawns an `Extension` entity, runs
/// `extension.register()` against it, returns the entity.
///
/// Takes `&mut World` (not `&mut App`) so this can be called from
/// world-scoped contexts like observer callbacks. BEI input context
/// registration belongs in
/// [`crate::JackdawExtension::register_input_context`], which is called
/// at catalog registration time with App access.
pub fn load_static_extension(
    world: &mut World,
    extension: Box<dyn crate::JackdawExtension>,
) -> Entity {
    let id = extension.id();
    info!("Loading extension: {id}");

    let extension_entity = world.spawn(Extension { id }).id();

    let mut ctx = crate::ExtensionContext::new(world, extension_entity);
    extension.register(&mut ctx);

    // Store the extension trait object on the entity so `unload_extension`
    // can call `unregister` before despawn.
    world
        .entity_mut(extension_entity)
        .insert(StoredExtension(extension));

    extension_entity
}

/// Disable a named extension by despawning its root entity.
pub fn disable_extension(world: &mut World, id: &str) -> bool {
    let mut query = world.query::<(Entity, &Extension)>();
    let Some(ext_entity) = query.iter(world).find(|(_, e)| e.id == id).map(|(e, _)| e) else {
        return false;
    };
    unload_extension(world, ext_entity);
    true
}

/// Internal component holding the extension trait object for the duration
/// of its lifetime. Used by [`unload_extension`] to invoke the optional
/// `unregister` hook before despawning. Not part of the public API.
#[derive(Component)]
pub(crate) struct StoredExtension(pub(crate) Box<dyn crate::JackdawExtension>);

/// Register a dylib-loaded extension into a running editor's catalog.
/// The dylib loader uses this after reading the dylib's entry metadata.
/// Operates on `&mut World` (not `&mut App`) because installs happen post-startup.
pub fn register_dylib_extension<F>(world: &mut World, ctor: F)
where
    F: Fn() -> Box<dyn crate::JackdawExtension> + Send + Sync + 'static,
{
    world
        .resource_mut::<ExtensionCatalog>()
        .register_extension_internal(ctor);
}

/// Keep `OperatorIndex` in sync when an operator entity is spawned.
pub(crate) fn index_operator_on_add(
    trigger: On<Add, OperatorEntity>,
    operators: Query<&OperatorEntity>,
    mut index: ResMut<OperatorIndex>,
) {
    if let Ok(op) = operators.get(trigger.event_target()) {
        index.insert(op.id, trigger.event_target());
    }
}

/// Keep `OperatorIndex` in sync and free the operator's `SystemId`s
/// when its entity is removed, so they don't leak across enable /
/// disable cycles.
pub(crate) fn deindex_and_cleanup_operator_on_remove(
    trigger: On<Remove, OperatorEntity>,
    operators: Query<&OperatorEntity>,
    mut index: ResMut<OperatorIndex>,
    mut commands: Commands,
) {
    let Ok(op) = operators.get(trigger.event_target()) else {
        return;
    };
    info!("Unregistering operator: {}", op.id);
    index.remove(op.id);
    let (exec, inv, check) = (op.execute, op.invoke, op.availability_check);
    commands.queue(move |world: &mut World| {
        let _ = world.unregister_system(exec);
        if exec != inv {
            let _ = world.unregister_system(inv);
        }
        if let Some(c) = check {
            let _ = world.unregister_system(c);
        }
    });
}

/// Unregister a dock window from [`jackdaw_panels::WindowRegistry`] and
/// purge it from the live dock tree and every stored workspace tree when
/// its marker entity despawns, so disabling an extension visibly removes
/// its windows.
pub(crate) fn cleanup_window_on_remove(
    trigger: On<Remove, RegisteredWindow>,
    windows: Query<&RegisteredWindow>,
    mut registry: ResMut<jackdaw_panels::WindowRegistry>,
    mut dock_tree: ResMut<jackdaw_panels::tree::DockTree>,
    mut workspaces: ResMut<jackdaw_panels::WorkspaceRegistry>,
) {
    let Ok(w) = windows.get(trigger.event_target()) else {
        return;
    };
    info!("Unregistering window: {}", w.id);
    registry.unregister(&w.id);
    dock_tree.remove_window(&w.id);
    for workspace in workspaces.workspaces.iter_mut() {
        workspace.tree.remove_window(&w.id);
    }
}

/// Unregister a workspace when its marker entity despawns.
pub(crate) fn cleanup_workspace_on_remove(
    trigger: On<Remove, RegisteredWorkspace>,
    workspaces: Query<&RegisteredWorkspace>,
    mut registry: ResMut<jackdaw_panels::WorkspaceRegistry>,
) {
    if let Ok(w) = workspaces.get(trigger.event_target()) {
        registry.unregister(&w.id);
    }
}

/// Remove a panel extension section from the registry when its marker
/// entity despawns.
pub(crate) fn cleanup_panel_extension_on_remove(
    trigger: On<Remove, RegisteredPanelExtension>,
    registrations: Query<&RegisteredPanelExtension>,
    mut registry: ResMut<crate::PanelExtensionRegistry>,
) {
    if let Ok(r) = registrations.get(trigger.event_target()) {
        registry.remove(&r.panel_id, r.section_index);
    }
}

fn cleanup_resource_on_remove(
    trigger: On<Remove, ExtensionResourceOf>,
    resource_id: Query<&ExtensionResourceOf>,
    mut commands: Commands,
) {
    let Ok(relation) = resource_id.get(trigger.entity) else {
        return;
    };
    let component_id = relation.resource_id.0;
    commands.queue(move |world: &mut World| {
        world.remove_resource_by_id(component_id);
    });
}
